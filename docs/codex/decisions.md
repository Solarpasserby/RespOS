# RespOS 设计决策记录

这里只收录能解释当前代码形态或避免重复踩坑的决策。日期是当前证据最后核验时间，不一定是
最初提出时间。

## ext4 setattr 使用单 inode transaction，缓存只在成功后发布

- 状态：已采用
- 适用范围：chmod/chown/utimens、目录属性、打开后 unlink、hardlink alias
- 最后验证：2026-08-10
- 证据：`vendor/lwext4_rust/c/lwext4/{include/ext4.h,src/ext4.c}`、
  `os/src/fs/{ext4/inode.rs,file.rs}`、`user/src/bin/fs_metadata_probe.rs`
- 内容：mode、owner 与选定时间字段由 `ext4_setattr` 在一次 pathname lookup、inode ref 和 transaction
  中提交；chown 引起的 suid/sgid 清除与 owner 变更同事务完成。Rust 缓存只在返回成功后发布。当前
  fd 属性修改通过 inode-number API 访问底层 inode，unlink 后不再依赖 storage/orphan path。
- 后续影响：不再接受“底层 ENOENT 但内存 override 成功”的兼容语义，也不得重新拆分 chown 的 owner/
  mode 提交。底层时间精度扩展仍是后续独立设计，不用更多路径特判替代。

## nlink=0 inode 按最后 VFS 引用延迟回收，不按 open File 数猜测

- 状态：已采用，崩溃恢复仍待完善
- 适用范围：unlink、rmdir、rename 覆盖、cwd、目录 fd、inode number 复用
- 最后验证：2026-08-10
- 证据：`os/src/fs/{ext4/inode.rs,file.rs,namei.rs}`、`user/src/bin/fs_namespace_probe.rs`、
  `scripts/fs_namespace_probe_linux.c`
- 内容：最后 namespace link 删除时，lower inode 先变为 nlink=0，但不立即 truncate/free。`Ext4Inode`
  的最后一个 Arc Drop 只入队，syscall 前后和 shutdown 安全点在统一 ext4 锁下回收。该 Arc 生命周期
  同时覆盖 File、cwd、Path 与 Dentry，删除了只能覆盖 File 的 `open_files` 计数。
- 后续影响：生命周期正确性优先于“本 syscall 内立刻归还块”。Drop 路径不得直接调用 lwext4；回收失败
  保留队列重试。当前 vendor 未实现完整 orphan-list mount recovery，异常断电窗口明确标记待完善，
  不能宣称等价于 Linux ext4 的崩溃恢复。

## ext4 目录 metadata 用 per-inode generation 失效，dentry cache 保留 16K 工作集

- 状态：已采用，完整 BuildStorm 已验证
- 适用范围：ext4 stat/namei、dentry/inode/PageCache 生命周期、kernel heap 预算
- 最后验证：2026-08-10
- 证据：`os/src/fs/{dentry_cache.rs,ext4/inode.rs}`、两架构 fs config；RV64 8 GiB/8 核固定
  120 秒 1024/8192/16384 项窗口和 RV64 1 GiB namespace/probe 门禁
- 内容：目录 raw inode 允许跨 syscall 缓存，但必须匹配该目录 inode 自身的 metadata generation。
  成功 create/link/symlink/unlink/rename/orphan cleanup 在 lower commit 后只递增实际相关父目录的
  generation；readdir 可能更新 atime，成功后做 inode 局部失效。
  dentry cache 从 1024 增至 16K，以保留 Cargo 树的
  dentry/inode/PageCache identity；16K 窗口无 eviction，heap peak 约 65 MiB/256 MiB。
- 后续影响：新增任何 ext4 namespace 修改必须对所有受影响父目录发布 generation，不能遗漏跨目录
  rename 的任一侧；也不能退回全局 generation 掩盖所有权不清。
  cache 不继续无数据扩容；完整负载需监控 heap peak、dentry eviction 和 PageCache registry。

## kernel heap allocator 必须仓库内 vendor，并以 O(1) membership/unlink 合并普通 free block

- 状态：已采用，长时 BuildStorm soak 待验证
- 适用范围：kernel global allocator、SMP/IRQ-safe allocation、依赖可复现性
- 最后验证：2026-08-10
- 证据：`vendor/respos_buddy_allocator`、`os/src/mm/heap_allocator.rs`；RV64 120 秒 Cargo A/B、双架构
  no-feature build、RV64 1 GiB/8 核四项 probe
- 内容：kernel 不再从 crates.io 使用 `buddy_system_allocator`，而由 `os/Cargo.toml` 显式引用仓库内
  vendor crate。保留 Layout 对齐、8 B 最小 class、buddy split/coalesce、OOM 和 user/actual/total
  accounting；至少 16 B 的 free block 使用 intrusive doubly-linked node，并用 caller-owned bitmap
  O(1) 判断和摘除 buddy。bitmap 由 kernel BSS 提供，不从 global allocator 自举分配；外层仍先关本地
  中断再取得全局 heap lock。
- 后续影响：不能直接修改 `.cargo-home/registry` 或依赖比赛机缓存中的 fork。allocator 变更必须通过
  mixed-layout/乱序释放/完全 coalesce 测试、双架构构建、1 GiB SMP probe 和真实 Cargo soak；不得用
  “更快但不合并”或无界 per-CPU cache 换取短窗口成绩。

## 当前保留 copy_to/from_user 语义边界，不实施未经准备的零拷贝

- 状态：已采用，随热点迁移复核
- 适用范围：read/write/pread/pwrite、socket I/O、lazy/COW user pages、EFAULT/partial I/O
- 最后验证：2026-08-10
- 证据：`os/src/mm/mod.rs`、`os/src/syscall/{fs,net}.rs`；RV64 120 秒 Cargo copy calls/bytes/ticks
- 内容：用户复制 helper 继续逐页验证 VMA 权限、resolve lazy/COW、翻译 PTE 后复制；不能直接解引用
  user VA。文件/socket syscall 的 bounce buffer 是潜在额外复制，但当前窗口 copy 总计仅约 0.424 CPU 秒，
  不足以支持高风险接口重构。
- 后续影响：若以后优化，先设计可复用的 prepared/pinned user-page span 和 scatter/gather FileOp/Socket
  接口，明确 fault-before-side-effect、共享 file offset、short I/O、并发 munmap/COW 和锁顺序；必须有
  专项 ABI/竞态测试后才能替换 bounce buffer。

## ext4 stat 缓存同一 inode 快照，lookup 每次只做一次必要的路径解析

- 状态：已采用，完整 BuildStorm/LTP 回归待验证
- 适用范围：VFS stat/lstat/fstatat、namei lookup、lwext4 raw inode/dirent ABI
- 最后验证：2026-08-10
- 证据：`os/src/fs/ext4/inode.rs`、`os/src/perf.rs`；RV64 8 核固定 120 秒 Cargo A/B、busybox stat 和
  四项无 feature probe
- 内容：非 synthetic inode 的 stat 以一次 `ext4_raw_inode_fill` 返回的 packed inode生成 size、mode、
  owner、link count 和 timestamps，不再为每个字段重走路径。lookup 同样对完整 child path执行一次
  `ext4_raw_inode_fill`，让 lwext4 内部完成按名/目录索引查找，避免 Rust 逐 dirent FFI 扫描和第二次
  mode path walk。on-disk 数值显式按 little-endian 解码，uid/gid/size 高位保留。raw inode 结果在
  同一普通文件/符号链接 `Ext4Inode` 内缓存，read/write/truncate/chmod/chown/link/unlink/
  rename/orphan 成功后失效；失效在释放 ext4 锁后执行。目录在完整父 inode 失效协议建立前
  不跨 syscall 缓存。VFS 内已有 override 与 PageCache 逻辑继续在每次 stat 动态覆盖 raw 值。
- 后续影响：新增 stat/lookup 字段优先从同一 raw inode 快照解释，不应重新引入按字段 path API 或 Rust
  逐目录项扫描。修改 packed inode解释时必须做双架构构建，并补充 symlink、高 uid/gid、大文件与 LTP
  stat/namei ABI 回归。新增修改 inode 元数据的路径必须同时失效快照，不得为命中率容忍陈旧
  size/mode/owner/time/nlink。

## lwext4 元数据 block cache 使用 4096 个 filesystem blocks

- 状态：已采用，完整 BuildStorm 计时待验证
- 适用范围：lwext4 路径遍历、inode/extent 元数据、kernel heap 固定预算、BuildStorm Cargo 树
- 最后验证：2026-08-10
- 证据：`vendor/lwext4_rust/c/lwext4/CMakeLists.txt`、`os/src/perf.rs`；RV64 8 核 16/1024/4096 项
  固定 180 秒 A/B 和无 feature 文件/内存门禁
- 内容：将 `CONFIG_BLOCK_DEV_CACHE_SIZE` 从 16 增至 4096。4 KiB 文件系统下对应约 16 MiB，保存
  lwext4 元数据；普通文件 bulk data 继续走 direct multi-block 路径和内核 PageCache，不用该 cache
  再复制一份。选择 4096 是因为同窗口实际 file-data fill 比 1024 项多约 28%，而不是只按最低块读取量
  选择容量。
- 后续影响：该容量从 256 MiB kernel heap 中常驻占用约 16 MiB；完整 BuildStorm 必须监控 heap peak。
  不得把它误当作可无限扩大的通用文件缓存。若以后为元数据建立独立缓存或复用打开 inode handle，应
  重新测 256/1024/4096 容量曲线，并相应缩回固定 cache。

## signal wait 必须阻塞，并显式登记 sigtimedwait 目标集合

- 状态：已采用，完整 BuildStorm 计时待验证
- 适用范围：`rt_sigtimedwait`、`rt_sigsuspend`、进程/线程级信号投递、timer timeout
- 最后验证：2026-08-10
- 证据：`os/src/syscall/signal.rs`、`os/src/task/task.rs`；RV64 8 核 busybox timeout 专项和 300 秒
  Cargo/perf 窗口
- 内容：信号等待不得用 `yield_current_task()` 轮询 pending/deadline；任务进入 blocked queue，并由目标
  信号、其他可投递信号或 timeout registry 唤醒。由于 sigtimedwait 的目标信号通常被用户 mask，TCB
  必须单独发布 wanted set；进程级投递优先选择对应 waiter，目标信号唤醒时不设置普通 EINTR 标记。
  发布 Blocked 后必须重查所有完成条件，避免信号在“首次检查—入队”之间到达而永久睡眠。
- 后续影响：新增信号等待接口应复用同一阻塞/竞态模型。不能为了让 blocked task 醒来而临时解屏蔽
  wanted set，也不能把目标信号按普通 interrupt 处理，否则 handler/EINTR 会抢先消费 sigtimedwait
  语义。timer 注册必须在所有返回路径清理。

## ext4 多字段时间戳在一次 inode transaction 中提交

- 状态：已采用，当前工作树待提交
- 适用范围：ext4 atime/mtime/ctime 更新、BuildStorm 高频文件写回、lwext4 vendor API
- 最后验证：2026-08-10
- 证据：`vendor/lwext4_rust/c/lwext4/{include/ext4.h,src/ext4.c}`、
  `os/src/fs/ext4/inode.rs`；RV64 8 GiB/8 核固定 120 秒 A/B 与五项无 feature probe
- 内容：一次 VFS `set_times` 需要更新多个字段时，不再为每个字段各做一次 pathname walk 和 inode
  transaction。vendor API 接收字段 mask，在同一个 inode ref 上更新所选 atime/mtime/ctime 后一次提交；
  Rust 层继续负责范围过滤、打开后 unlink 的 ENOENT 兼容和缓存时间语义。read/readdir 的自动 atime
  使用独立入口，只更新 atime；显式 utimens 及 mtime 修改仍更新 ctime。
- 后续影响：不得通过延迟或丢弃可见时间戳来复制本轮收益。自动 atime 不得更新 ctime，否则 relatime
  会在 `atime <= ctime` 上自我触发；显式时间修改又必须更新 ctime。继续优化前需验证跨 reopen、stat
  和同步边界；所有 lwext4 调用仍复用唯一 `EXT4_OP_LOCK`。

## BuildStorm 采用三层级、测量驱动的优化路线

- 状态：已采用
- 适用范围：题目二正确性修复、性能优化、多核调度、FS/MM/I/O 重构
- 最后验证：2026-08-09
- 证据：`docs/codex/buildstorm-smp-plan.md`、`docs/codex/workflows.md`；第一轮提交 `7cb282a`、
  第二轮当前工作树和 RV64 8 核完整运行/短 rustc 回放
- 内容：历史 SMP Phase 0--5 作为正确性前置；性能阶段统一分为低风险热路径与资源闭环、共享瓶颈与
  有效多核扩展、深层 I/O/MM/双架构扩展三层。每层依据固定镜像下的 CPU/jobs 缩放曲线及
  scheduler、ext4、PageCache/fault、TLB、block I/O、host 资源计数选择单一最高热点；并行 rustc
  SIGSEGV 未闭环前不叠加新的性能重构。
- 后续影响：per-CPU runqueue、clean private file frame 共享、ext4 拆锁、异步 VirtIO、ASID 或
  per-CPU allocator 都是候选而非预先批准的实现。没有前后数据、专项语义门禁和无 feature 正式复跑
  的修改不进入下一层；优秀内核只提供机制参考，不能替代 RespOS 当前证据。

## Frame-backed PageCache 保留 128 MiB 工作集并做 64 KiB 顺序预读

- 状态：已采用，干净容量 A/B 与完整 BuildStorm 待验证
- 适用范围：普通文件缓存 miss、lwext4 锁竞争、块请求合并、物理内存预算
- 最后验证：2026-08-10
- 证据：`os/src/fs/page_cache.rs`、`os/src/arch/{rv64,loongarch64}/config/fs.rs`；RV64 8 核
  `buildstorm_file_probe` 和 BuildStorm minibuild
- 内容：PageCache 数据已由物理 frame 承载；在 64 MiB 长窗口持续满载并发生约 53.9 万次 eviction 后，
  全局 resident 上限提高到 128 MiB。
  有底层普通文件的 read miss 一次最多读取连续 16 页，在 PageCache 锁外完成 I/O 后逐页安装；写路径
  不预读。预读插入使用文件长度代次拒绝并发 truncate 的过期快照。
- 后续影响：缓存预算消耗物理页但每页元数据仍消耗 kernel heap；继续扩大前必须同时观察
  `page_cache_pages`、free frames 和 heap peak。不得为减少锁次数而在 `EXT4_OP_LOCK` 之外并发调用
  lwext4；随机 I/O 回归时应重新评估固定 16 页预读是否过量。

## AF_UNIX 阻塞 I/O 与 poll 使用同一条件事件源，不使用 yield polling

- 状态：已采用，pathname/shutdown/EINTR/poll 专项已补
- 适用范围：socketpair/pathname Unix socket read/write/accept、peer close、signal interruption
- 最后验证：2026-08-11
- 证据：`os/src/net/socket.rs`、`user/src/bin/{unix_socket_block_probe,socket_phase5_probe}.rs`、
  `scripts/socket_phase5_probe_linux.c`；RV64 16 GiB/8 核专项
- 内容：reader、writer、accept waiter 分别与接收 buffer 或 listener pending queue 共锁登记；登记后
  发布 blocked，并在切换前处理 producer-wins 和 signal-wins 竞态。写入唤醒 reader、读取唤醒 writer、
  connect 唤醒 accept、shutdown/endpoint drop 唤醒 EOF/EPIPE 对端。非阻塞路径继续直接返回 EAGAIN。
  同一 buffer/listener 的 `PollWaiters` 是 ppoll/epoll 的事件源；HUP/error 由 FileOp 独立发布，即使用户
  未在 interest 中请求也必须返回。pipe 复用相同的 exceptional-event 规则。
- 后续影响：不得改回固定 sleep/yield。新增 shutdown、dup-close 或 poll waiter 时必须一起验证 EOF、
  EPIPE、lost wake、EINTR 和 single-winner；唤醒 scheduler 必须在释放 socket data lock 后执行。

## 只读 MAP_PRIVATE 文件映射共享 PageCache frame

- 状态：已采用，完整 BuildStorm 计时待验证
- 适用范围：普通文件只读/可执行 private mmap、动态库与编译器映像、mprotect
- 最后验证：2026-08-10
- 证据：`os/src/mm/memory_set.rs`、`user/src/bin/buildstorm_private_map_probe.rs`；RV64 1 GiB/8 核
  `perf_counters` A/B 与无 feature private/file/shared-MM/frame-reclaim 门禁
- 内容：没有 WRITE 权限的 file-backed `MAP_PRIVATE` PTE 直接引用 PageCache 的 `FrameTracker`，避免
  多进程对同一动态库逐页分配和复制。原生可写 private mapping 仍使用独立 frame；只读映射通过
  `mprotect(PROT_WRITE)` 升权时，必须先把所有 resident 页私有化，成功后才更新 PTE 权限。
- 后续影响：PageCache reclaim 把这些 frame 引用视为 mmap pin；退出/munmap 释放引用后才能淘汰。
  不得直接给共享 cache frame 增加 WRITE PTE。后续 truncate/SIGBUS 语义完善必须同时覆盖 private 与
  shared cache-frame 映射；完整 BuildStorm 仍需确认 300 MiB 级工具链映像的实际收益。

## BuildStorm kernel heap 使用 256 MiB 容量

- 状态：已采用，已越过旧 OOM 点，完整 BuildStorm 仍受并行 rustc SIGSEGV 阻断
- 适用范围：RV64/LA64 kernel BSS、buddy allocator、高并发用户地址空间元数据
- 最后验证：2026-08-09
- 证据：`os/src/arch/{rv64,loongarch64}/config/mm.rs`；双架构 release、RV64 1/8 GiB 启动
- 内容：128 MiB 完整运行失败时用户 live 请求约 93.5 MiB，但 buddy 按二次幂取整后的实际占用约
  127.94 MiB。当前不替换 allocator，将两架构固定 heap 扩大到 256 MiB，为 resident page 的
  `Arc<FrameTracker>`、BTreeMap 映射元数据和编译并发峰值留出余量。
- 后续影响：这不是泄漏修复；若 256 MiB 仍随累计工作量单调耗尽，应继续查生命周期而不是再次扩容。
  小内存 guest 会少 128 MiB 可分配物理页，但 1 GiB RV64 门禁已通过。

## 普通文件 PageCache 与 MAP_SHARED 共用 FrameTracker

- 状态：已采用，完整 BuildStorm 已越过旧 heap 阻断，当前并行 rustc SIGSEGV 待定位
- 适用范围：普通文件 PageCache、MAP_SHARED、truncate、全局缓存回收
- 最后验证：2026-08-09
- 证据：`os/src/fs/page_cache.rs`、`os/src/fs/file.rs`、`os/src/mm/memory_set.rs`；RV64 8 核
  `buildstorm_file_probe` 与 8 GiB 完整 BuildStorm 日志
- 内容：一个普通文件缓存页由 PageCache 持有一个 `Arc<FrameTracker>`，共享文件映射克隆同一 frame，
  不再维护第二份 mmap frame 和正常文件的全局弱引用索引。PageCache 回收必须把 frame 的额外强引用
  视为 mmap pin；truncate 删除页前必须清零 frame。无 PageCache 文件可保留全局弱表作为兼容后备，
  但失效弱引用必须被清除。
- 后续影响：普通 buffered I/O 与共享映射天然观察同一物理页，不得重新添加 overlay/copy 同步热路径。
  mmap 写回仍由现有 munmap/msync 快照路径标脏和提交；若以后引入 PTE dirty-bit writeback，需要在同一
  PageCache 页上扩展状态，而不是恢复平行缓存。

## 普通 close 不执行全文件系统持久化屏障

- 状态：已采用，Phase 3 已闭合 dirty owner 生命周期
- 适用范围：普通文件 close、PageCache writeback、fsync/sync、ext4 shutdown
- 最后验证：2026-08-11
- 证据：`os/src/fs/file.rs`、`os/src/fs/ext4/super_block.rs`；RV64 8 核
  `buildstorm_file_probe` 与 `/proc/respos_perf`
- 内容：普通 `close(2)` 不等价于 `fsync(2)`，`File::drop()` 只回收 open-file description，不执行
  PageCache lower write 或 filesystem barrier。inode cache 可以继续使用 weak reference；外部 dirty-owner
  表会强持有 inode、PageCache 和 filesystem，直到数据与待提交 mtime/ctime 都干净。显式 fsync/
  fdatasync、sync/syncfs、正常卸载和 shutdown 定义持久化边界；阈值 writeback 只提交数据/元数据，
  不把普通 close 升级成 durability guarantee。
- 后续影响：任何能清 dirty 的路径都必须同时检查 owner 是否可释放；任何能新建 dirty 的路径必须在
  返回用户态前登记强 owner。不能恢复依赖最后 File drop 防丢数据的局部补丁。

## PageCache 写回错误使用 inode 共享序列与 open-file cursor

- 状态：已采用
- 适用范围：PageCache、close、fsync/fdatasync、dup/fork、独立 open
- 最后验证：2026-08-10
- 证据：`os/src/fs/{page_cache.rs,file.rs}`、`user/src/bin/fs_writeback_probe.rs`；RV64
  `debug_traces` 一次性 EIO probe
- 内容：lower 写回失败先保留页 dirty，再递增 PageCache error sequence。每个新 open-file description
  采样当前 sequence；dup/fork 共享 cursor，独立 open 拥有独立 cursor。同步接口在重试写回后推进 cursor，
  使同一旧错误对每个受影响 description 最多报告一次；错误发生后才 open 的 description 不接收历史错。
  debug fault control 只在 `debug_traces` 构建中接受命令，release 不提供故障入口。
- 后续影响：不能把 cursor 放进 `FdEntry`，否则 dup 会错误地各自消费；也不能只在 `fsync` 调用栈上传递
  lower error，否则 close/阈值/未来后台写回的异步错误会静默丢失。后续若需要保留多个不同错误，扩展
  sequence 记录而不是退回全局单个 errno。

## lwext4 连续对齐块使用 multi-block VirtIO 请求

- 状态：已采用，完整 BuildStorm 计时待验证
- 适用范围：lwext4 `KernelDevOp` 到 `BlockDevice` 的读写
- 最后验证：2026-08-09
- 证据：`os/src/drivers/disk.rs`、`os/src/drivers/virtio/block_dev.rs`；RV64 8 核
  `buildstorm_file_probe`
- 内容：对齐且连续的 512-byte blocks 合并为一次 `read_blocks`/`write_blocks` 调用；只有非对齐
  头尾继续单块读取或 read-modify-write。块层计数记录实际 VirtIO 请求数和字节数。
- 后续影响：不得为了继续合并而跨越非连续 LBA 或跳过 partial-block 保留内容；异步队列和多队列属于
  后续独立优化。

## 高频诊断设施必须由 kernel feature 静态隔离

- 状态：已采用
- 适用范围：BuildStorm 性能计数器、进程/退出/pipe/futex/timer 等详细串口 trace
- 最后验证：2026-08-09
- 证据：`os/src/perf.rs`、`os/src/console.rs`、`os/Cargo.toml`；RV64/LA64 feature 组合构建及
  RV64 `/proc/respos_perf`/trace 烟测
- 内容：聚合原子计数只由 `perf_counters` 启用，高频串口路径只由 `debug_traces` 启用；正式计时两者
  均关闭。业务调用统一使用编译为空操作的计数函数和 `debug_trace!`，不得在热点中以运行时布尔值
  保留格式化、timer 采样或原子更新成本。
- 后续影响：临时定位问题时优先扩展现有门控和 proc 汇总，不再新增无条件串口输出或平行计数框架；
  panic、启动和不可恢复错误输出不属于该高频诊断边界。

## 共享高半区内核模型

- 状态：已确认
- 适用范围：RV64/LA64 虚拟内存、启动、linker、页表
- 最后验证：2026-08-01
- 证据：`os/src/linker_riscv.ld`、`os/src/linker_loongarch.ld`、
  `os/src/arch/*/config/mm.rs`、`os/src/main.rs`
- 内容：两个架构保持 `0xffffffc0...` 高半区共享内核布局。LoongArch 从低物理入口建立早期
  分页再跳转高半区，不采用与公共内核假设割裂的独立低地址模型。
- 后续影响：LoongArch 修复应尽量收敛在 arch 层，不能未经全局审计改变公共地址空间模型。

## syscall 保持薄层，领域对象拥有状态机

- 状态：已确认
- 适用范围：MM、task、FS、net syscall
- 最后验证：2026-08-01
- 证据：`os/src/syscall/mm.rs`、`os/src/mm/memory_set.rs`、Git `15fe1a5`；A/B/C 文档
- 内容：syscall 负责 ABI 参数解析、fd/用户指针获取和 errno 映射；VMA、调度、namei、socket
  等状态转换由领域模块维护。
- 后续影响：发现 syscall 文件重复维护缓存、映射表或对象生命周期时，应优先下沉到所有者。

## 用户指针通过页表逐页访问

- 状态：已确认
- 适用范围：copyin/copyout、字符串、iovec、futex 预检查
- 最后验证：2026-08-01
- 证据：`os/src/mm/mod.rs`、Git `15fe1a5`
- 内容：用户地址可能跨页、跨相邻 VMA、处于 lazy 或 COW 状态。通用 copy 先检查整段权限并
  确保页可访问，再通过 PTE 对应 frame 复制，避免 kernel page fault。
- 后续影响：不要在 syscall 中直接构造用户 slice 或逐字节裸解引用。

## 高风险状态修改采用 prepare/copyout/commit

- 状态：已确认
- 适用范围：wait4、shmat、timer、prlimit、rename 等可失败操作
- 最后验证：2026-08-01
- 证据：Git `3aa1fb5`、`15fe1a5`、`cba8e24`；
  `docs/四天内核重构-ABC-整合审查.md`
- 内容：先验证并准备资源，用户 copyout 成功后再提交不可逆状态；失败不能重复累计、提前删除
  zombie、破坏旧映射或泄露半成品对象。
- 后续影响：review 时要逐项列出失败点和其前后的可见状态，不能只检查成功路径。

## descriptor flags 与 open-file status flags 分离

- 状态：已确认
- 适用范围：fdtable、dup、fcntl、exec、CLOEXEC
- 最后验证：2026-08-01
- 证据：`os/src/fs/fdtable.rs`、`os/src/fs/file.rs`、Git `cba8e24`
- 内容：CLOEXEC 属于每个 `FdEntry`；offset 和 O_APPEND/O_NONBLOCK 等 open-file 状态属于共享
  `File`。dup 共享后者，不共享前者。
- 后续影响：任何 fd 复制、进程复制和 fcntl 实现都必须保持这一身份关系。

## namei 以显式 lookup policy 表达最终分量语义

- 状态：已采用
- 适用范围：final symlink、trailing slash、empty path、link/rename/unlink/open
- 最后验证：2026-08-11
- 证据：`os/src/fs/namei.rs`、Phase 4 Linux/RV64 probes
- 内容：路径切分结果之外保留 trailing-slash 目录约束；调用方显式选择是否跟随 final symlink、
  final mount，以及 trailing slash 是否覆盖 no-follow。rename 的旧路径定位目录项自身，link 默认
  链接 symlink inode，标准 lookup/lstat/readlink 则按各自 Linux policy 选择入口。
- 后续影响：不接受所有 syscall 先统一 follow、再根据 inode type 猜测操作对象的实现。新增 policy
  前先用 Linux probe 固定 errno；只增加布尔分支而没有对应调用者语义和测试，不进入 namei 公共层。

## 未实现状态型 ABI 必须诚实失败

- 状态：已确认
- 适用范围：mmap/madvise/special fd/fs flags 等
- 最后验证：2026-08-01
- 证据：A/B/C 重构文档、Git `cba8e24`/`15fe1a5`
- 内容：会改变长期状态或让用户依赖后续语义的能力，未实现时返回明确 errno，不能无条件 0。
  纯 hint/no-op 只有在 ABI 允许且完成参数校验时才可受限接受。
- 后续影响：恢复旧测例不能以重新引入假成功为代价；应修实现或证明用例依赖非目标行为。

## writable file `MAP_SHARED` 使用锁外快照写回

- 状态：已验证（受限 ABI 子集）
- 适用范围：文件 mmap/msync/munmap/writeback；RV64/LA64
- 最后验证：2026-08-02
- 证据：`os/src/mm/memory_set.rs`、`os/src/syscall/mm.rs`、`os/src/syscall/fs.rs`、
  `os/src/task/task.rs`；RV64/LA64 LTP `mmap001` 与 mmap/munmap 子集回归
- 内容：可写 `MAP_SHARED` 不再被无条件拒绝。共享文件页在建立映射前锁外预取；写回时在
  `MemorySet` 锁内复制 resident frame 快照，释放锁后通过 `FileOp` 写入页缓存；`MS_SYNC`
  再执行 `fsync`。munmap、MAP_FIXED 替换、mremap 覆盖/收缩、mprotect 和进程退出复用同一
  写回协议，写回失败可返回给 syscall，且不会在 MM 锁内执行后端 I/O。当前没有硬件 dirty bit，
  所以对 resident writable shared file pages 保守写回。
- 后续影响：`MS_INVALIDATE` 仍明确返回 `EOPNOTSUPP`，因为共享文件 frame 全局缓存没有
  inode-wide 失效协议；文件截断后的 mapped-page `SIGBUS` 规则仍需单独实现，写回时不会因旧
  映射重新扩展当前 EOF。

## 关键回归必须双架构并分析日志

- 状态：已确认
- 适用范围：进入集成/main 的测试门禁
- 最后验证：2026-08-01
- 证据：顶层 `Makefile`、A/B/C 验收文档、当前 `make rv`/`make la` 结果
- 内容：构建门禁至少覆盖 RV/LA；运行门禁使用仓库根目录真实入口。QEMU 正常关机和 make 退出
  0 只表示 runner 结束，必须解析测试 summary 和失败标记。
- 后续影响：提交说明应分别陈述 build、boot、专项 probe、完整 workload，不使用笼统“测试通过”。

## SMP kernel timer 只在无任务锁的安全点执行高层工作

- 状态：已确认
- 适用范围：RV64 SMP timer、timeout、signal、futex/timerfd/POSIX timer registry
- 最后验证：2026-08-06
- 证据：`os/src/arch/rv64/trap/mod.rs`；2 核 `ACTIVE_ITIMER_TASKS` 中断重入 GDB 栈
  `/tmp/respos-smp2-dynamic-bt.txt`；2/4/8 核各三轮退出压力
- 内容：普通 kernel-mode timer trap 只确认并重编程 tick，不调用会获取 task/signal/timer
  锁的 `check_all_task_timers()`。高层 timer work 当前由 user-mode timer trap 和 boot hart 的无 current
  idle context 串行服务。
- 后续影响：不以“把某一把锁换成 NoIrq”逐个遮掩高层中断重入；需更强及时性时改为
  中断只发布 pending，再在明确安全点消费。

## RV64 shootdown 复用目标 OpenSBI 的同步 RFENCE

- 状态：已确认（仅限当前 QEMU/OpenSBI 目标）
- 适用范围：RV64 SMP `MemorySet::flush_tlb`
- 最后验证：2026-08-06
- 证据：RISC-V SBI RFENCE 规范；OpenSBI 1.5.1 `sbi_tlb_request`、`tlb_sync` 实现；
  `smp_shared_mm_probe` 2/8 核日志
- 内容：OS 维护真实 active hart mask；PTE 发布后调用 SBI remote SFENCE。当前 OpenSBI 的
  TLB IPI event 带 per-source sync counter，发起方在远端执行 fence 并 ack 前不返回，因此不再
  叠加一套重复的 S-mode shootdown IPI 队列。
- 后续影响：这是平台契约，不是 SBI 文本对所有 firmware 的无条件完成性推论。更换 firmware、
  绕过 SBI 或加入 ASID 后必须重新审计 request/ack、mask 与 frame 回收顺序。

## filesystem exec 不以扩大固定 kernel heap 支持大 ELF

- 状态：已采用
- 适用范围：filesystem ELF loader、BuildStorm、kernel heap budget
- 最后验证：2026-08-06
- 证据：`os/src/mm/memory_set.rs::try_from_elf_file()`；45,559,552 字节 cargo 的
  `BUILDSTORM_TOOLCHAIN ok` 日志 `/tmp/respos-buildstorm-rv8-file-backed-exec.log`
- 内容：大 ELF 通过 file-backed PT_LOAD 按需装页；不提高 `KERNEL_HEAP_SIZE`、不放宽
  `read_all()` 的半堆保护来承受完整 executable Vec。这样 exec 临时内存随 header 大小而非文件大小
  增长，并把段页生命周期纳入已有 VMA/frame 模型。文件式 exec 的 header/PT_INTERP 元数据前缀
  上限为 1 MiB；超出上限或 PT_LOAD 越过文件末尾的 ELF 返回 `ENOEXEC`，不得触发无界内核分配。
- 后续影响：若扩展到 interpreter 或其他架构，继续传递 file identity/offset/length；不得为单个
  workload 设置路径特判或无界整文件 allocation。

## 优化时保持原有有效测例兼容

- 状态：暂定
- 适用范围：`dev` 后续重构
- 最后验证：2026-08-01
- 证据：当前维护目标；尚待后续提交和回归矩阵固化
- 内容：结构优化应尽量保持已有有效 ABI 测例；如果旧测例依赖取巧实现，可以排除，但必须
  给出源码级原因和替代验证，不能仅因失败就标记为“无效”。
- 后续影响：下一阶段先恢复 LTP harness 可运行性，再以真实失败驱动窄范围修复。

## smoltcp TCP 使用受限 backlog listener 池

- 状态：已验证（当前工作树）
- 适用范围：loopback TCP listen/accept 与并发连接
- 最后验证：2026-08-03
- 证据：`os/src/net/listen.rs`、`os/src/net/mod.rs`、`os/src/net/tcp.rs`；
  `/tmp/cagent-a-rv-run{1,2,3}.log`
- 内容：不在 `connect` 上用测试相关重试掩盖 `ECONNREFUSED`。由于一个 smoltcp listener 同时
  只能承接一个握手，内核按传入 backlog（上限 128、下限 1）预建 listener handle；完成握手的
  handle 进入 accept queue，协议 poll 和 accept 路径负责补充空位。
- 后续影响：backlog 会占用 TCP 收发缓冲内存，不能无限接受用户值；close/unlisten 必须回收仍在
  listener 池和 accept queue 中的所有 handle。若以后替换为原生 SYN queue，应保持相同的
  userspace 可观察并发语义。

## exec 保留调用线程的 signal mask 与 pending set

- 状态：已采用
- 适用范围：exec、signal mask/pending、sigaction/alt stack
- 最后验证：2026-08-11
- 证据：`os/src/task/task.rs::install_exec_image()`、`user/src/bin/signal_phase5_probe.rs`、
  `scripts/signal_phase5_probe_linux.c`；RV64 16 GiB/8 核专项
- 内容：exec 保留调用线程的 blocked mask 和 pending signals；用户安装的 handler 恢复默认，显式
  `SIG_IGN` 保持，alternate signal stack 重置。不得以“新程序不认识旧信号”为由清空 pending set。
- 后续影响：实现非 leader exec 时只保留调用线程自身 pending set，不能合并或继承被终止 sibling 的
  thread-directed pending signals；后续引入独立 process-pending queue 时须分别处理两类所有权。
