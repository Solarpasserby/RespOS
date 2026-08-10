# RespOS 已确认易错点

## 自动 atime 更新不能复用会刷新 ctime 的显式 utimens 路径

- 状态：已确认并修复 BuildStorm 写放大
- 适用范围：relatime、read/readdir、ctime、ext4 metadata writeback
- 最后验证：2026-08-10
- 证据：RV64 8 GiB/8 核固定 120 秒 attributes 计数 A/B；临时 ext4 文件 stat/cat/touch 专项
- 内容：旧实现每次自动 atime 都同时把 ctime 更新为 now，使下一次 relatime 判断继续满足
  `atime <= ctime`。58,692 次 set-times 中有 58,682 次来自自动 atime，形成约 29 万次块写请求。
  自动访问更新改为不碰 ctime 后，atime 落盘降至 1,185 次，块写请求降至 9,663；显式 `touch -a`
  仍同时更新 ctime。
- 后续影响：自动访问时间和显式 `utimensat/futimens` 必须走语义不同的入口。不能为复用代码让普通
  read 改 ctime，也不能反向让显式时间修改漏掉 ctime；relatime 回归需检查第二次读取不重复落盘。

## 多个 lwext4 时间 setter 会为同一 inode 重复完整路径遍历

- 状态：已确认并由组合 vendor API 修复当前热点
- 适用范围：atime/mtime/ctime、文件写回、Cargo/rustc 高频 metadata 更新
- 最后验证：2026-08-10
- 证据：RV64 8 GiB/8 核固定 120 秒 class A/B；`vendor/lwext4_rust/c/lwext4/src/ext4.c`
- 内容：`ext4_atime_set`、`ext4_mtime_set`、`ext4_ctime_set` 各自获取 path 对应 inode ref 并提交。
  即便 Rust 已持全局 ext4 锁，连续调用仍重复 pathname walk 和 metadata transaction。组合更新把多个
  字段放入一次 inode ref 生命周期，attributes hold 从 54.60 降至 30.46 CPU 秒。
- 后续影响：多字段 metadata 更新应优先审查能否在一个底层 transaction 中完成；不能仅因外层已有锁
  就假设多个 C API 调用成本很低。合并必须保持字段选择、范围过滤、只读检查、错误和持久化语义，不能
  用跳过时间戳更新换取速度。

## 过小 dentry cache 会同时丢掉 inode metadata 和 PageCache identity

- 状态：已确认并由 16K cache 修复当前窗口
- 适用范围：Cargo 深目录树、VFS dentry/inode cache、ext4 lookup/stat、kernel heap 权衡
- 最后验证：2026-08-10
- 证据：1024/8192/16384 项 RV64 8 GiB/8 核 120 秒窗口；lookup 30,616 -> 6,007 ->
  4,019，8192 项 eviction 5,426，16K 为 0
- 内容：全局 dentry cache 满后每次 insert 扫描并任意移除一个只有 cache 引用的叶节点。
  叶 dentry 消失后 inode 的 weak cache 也无法升级，连带丢失 raw metadata 和 inode PageCache；
  后续访问不只多一次 dentry lookup，还会重新读文件页。
- 后续影响：调容量时同时看 lookup calls/ticks、stat miss、PageCache fill/registry、eviction 和
  heap peak。短窗口没有 eviction 不代表完整构建永不达容量，也不应为零 eviction 无界扩容。

## buddy allocator 的 free-list 线性 buddy 查找会在编译负载下吞掉大量 CPU

- 状态：已确认并由 vendor allocator 修复短窗口热点
- 适用范围：kernel heap 高频小对象释放、8 核 Cargo/rustc、allocator 归因
- 最后验证：2026-08-10；RV64 8 核固定 120 秒 A/B
- 证据：旧 allocator dealloc total/core 29.82/26.26 CPU 秒，vendor bitmap+doubly-list 后为
  4.42/2.88 秒
- 内容：总 heap ticks 同时包含 spin-lock wait 和 allocator core，不能只凭总数归因锁竞争。拆分后旧
  `buddy_system_allocator 0.10.0` 的 core 占 dealloc 约 88%，源码对应逐项扫描 free list 寻找 buddy。
- 后续影响：先拆 lock/core 再选择方案；禁止简单关闭 coalesce（长期会碎片/OOM），也不能只在本机
  registry 打补丁。可交付实现必须位于 `vendor/` 并保持完整 split/coalesce 与 Layout 语义。

## bounce buffer 看起来是双拷贝，但不能在无占比和无 fault 协议时直接删除

- 状态：已确认当前不是主要热点
- 适用范围：用户 I/O buffer、copy_to/from_user、read/write/socket 零拷贝设想
- 最后验证：2026-08-10；RV64 8 核固定 120 秒 Cargo 窗口
- 证据：约 106 MB user copy 的 calls/bytes/ticks 合计只有约 0.424 CPU 秒
- 内容：copy helper 本身只执行一次 kernel↔user copy；“第二次”来自 FileOp/Socket 与 kernel bounce
  buffer。helper 同时承担 VMA permission、lazy/COW resolution 和 PTE translation，直接传 user pointer
  会在 kernel page fault、并发 munmap 和部分 I/O side effect 上改变语义。
- 后续影响：性能优化按计数占比排序。若未来做 prepared pages，必须先固定/验证所有 user span，再允许
  file offset 或外部设备产生副作用，并覆盖 EFAULT、跨页 COW、short I/O 与并发地址空间修改。

## 一个 stat 不能为每个字段重新遍历一次完整路径

- 状态：已确认并修复专项性能窗口
- 适用范围：lwext4 `stat` 字段读取、directory lookup、Cargo 深路径元数据负载
- 最后验证：2026-08-10；RV64 8 核固定 120 秒 Cargo 窗口
- 证据：优化前 stat/lookup 占约 84% ext4 lock hold；raw-inode/dirent 优化后 tg-xtask 约
  `2m15--2m19s -> 1m34s`
- 内容：即使 metadata block 已缓存，`ext4_mode_get`、owner/time getters 每次仍执行 pathname walk，
  同一 stat 连续调用六次会把瓶颈从 I/O 变为锁内 CPU。Rust 通过 FFI 逐项扫描父目录同样昂贵，即便
  不再二次查 mode，仍显著慢于让 lwext4 内部按 child path/目录索引完成查找。一次 raw inode 快照既
  减少遍历，也保证各字段来自同一时刻。
- 后续影响：看到 block I/O 已很低但 ext4 hold 仍高时，应按操作和调用数归一，不能继续只调 cache。
  优化不得直接读取未对齐 packed 字段引用；需按值读取并做 endian/high-bit 处理，同时保留 UNKNOWN
  dirent fallback。跨 syscall 缓存 raw inode 时，read/atime、write、truncate、chmod/chown、link/unlink、
  rename 和 orphan 都是必须审计的失效点；父目录还会被 namespace 操作间接修改，失效不完整时
  不应缓存目录。成功 unlink 还必须递减 `nlink_override`。锁序保持
  “释放 ext4 锁后再失效 inode 快照”。

## lwext4 的 16 项 block cache 会把路径元数据放大成海量 4 KiB I/O

- 状态：已确认并修复专项性能窗口
- 适用范围：Cargo 深目录树、lwext4 path lookup/inode/extent、PageCache 与 block I/O 归因
- 最后验证：2026-08-10；RV64 8 核固定 180 秒 Cargo 窗口
- 证据：16 项时 172.6 MB inode data read 对应 4.97 GB block read；4096 项时 221.7 MB data read只对应
  295.8 MB block read
- 内容：`block_read_bytes` 不等于应用文件内容。lwext4 的 file data 使用 direct path，但目录块、inode
  table 和 extent lookup 使用内部 bcache；只有 16 个 4 KiB entry 时，每次重新 open 深路径都会淘汰
  元数据，形成近 29 倍读取放大。只扩大上层 PageCache 或继续合并连续 data blocks无法命中这一层。
- 后续影响：分析文件系统热点时同时记录 PageCache fill bytes、Ext4Inode requested/completed bytes、
  block size buckets 和 ext4 lock hold。cache 容量 A/B 必须比较同窗口完成进度与 heap current/peak；
  单看 block bytes 可能偏好容量不足但进度更慢的配置。

## sigtimedwait 的目标信号通常已被 mask，不能只套普通 interruptible sleep

- 状态：已确认并修复 signal/time 调度风暴
- 适用范围：`rt_sigtimedwait`、`rt_sigsuspend`、busybox/coreutils timeout、进程级 signal 选路
- 最后验证：2026-08-10；RV64 8 核 timeout 专项与 Cargo 固定窗口
- 证据：修复前 600 秒窗口约 1962 万次 `signal_time` yield；修复后 busybox 3 秒 timeout 和干净
  300 秒 Cargo 窗口均为 `signal_time=0`
- 内容：简单把轮询替换为普通可中断阻塞仍会死锁：调用者按 POSIX 习惯先 block wanted signal，而普通
  `check_signal_interrupt()` 会跳过 masked signal。必须另外登记 sigwait wanted set，让投递端选择并
  唤醒 waiter，同时保留该信号 pending 供 sigtimedwait 消费，不能把它转换成 `EINTR`。
- 后续影响：验证必须同时覆盖目标信号唤醒、有限 timeout、非目标信号 EINTR 和信号到达/发布 Blocked
  的 lost-wakeup 窗口；只测“超时最终返回”不足以证明 signal selection 正确。

## 交互式 probe 返回后的 stdin 轮询会污染调度计数

- 状态：已确认
- 适用范围：`user_shell` 下的 `/proc/respos_perf` 专项测量、stdin、scheduler yield/IPI
- 最后验证：2026-08-10；RV64 1 GiB/8 核 `buildstorm_private_map_probe`
- 证据：`os/src/fs/stdio.rs::Stdin::read()` 与 yield 子系统分桶；分开发送命令时约 46--113 万次
  `stdio_yields`，预排 probe/read/quit 后同一工作窗口为 0
- 内容：probe 返回提示符后，若宿主过一段时间才发送读取计数的命令，shell 会在 SBI console 无字符时
  反复 `yield_current_task()`。这些切换发生在被测 workload 之后，却仍落在尚未再次 reset 的窗口内。
- 后续影响：交互式性能 probe 必须把工作命令、读取 `/proc/respos_perf` 和 `quit` 一次性写入串口输入；
  已出现的高 scheduler yield 必须先按调用点分桶，不能直接归因于刚结束的 workload。

## lwext4 读取稀疏洞时不能把未分配块号当物理块 0

- 状态：已确认并修复 tg-xtask 链接产物损坏
- 适用范围：稀疏文件、共享文件 mmap、linker 输出、`ext4_fread()`
- 最后验证：2026-08-08；RV64 8 核专项 probe 与手工 ArceOS release build
- 证据：`vendor/lwext4_rust/c/lwext4/src/ext4.c::ext4_fread()`、
  `os/src/fs/ext4/inode.rs::read_at()`；修复前 lld 新文件的共享映射在任何用户写入前就含稳定的磁盘
  块 0 字节，最终 `.symtab[0]` 非零；修复后该 entry 全零且 `llvm-objcopy` 返回 0
- 内容：extent lookup 对洞返回 `fblock == 0`。旧批量读取路径仍把完整洞块传给 direct block read，
  尾部不足一块的洞也读取物理块 0。两种洞必须显式填零并推进位置，只有非零且物理连续的数据块才能
  合并读取。Rust `read_at()` 预清 buffer 可作为防御，但不能替代后端正确处理洞。
- 后续影响：稀疏回归不仅要 close/reopen 后读洞，还要覆盖新建文件 ftruncate 后立即 MAP_SHARED，
  并在用户首次写入前检查映射内容全零。

## mmap 与 pwrite 共存时不能维护两份普通文件缓存页

- 状态：已确认并修复专项回归
- 适用范围：同一 inode 的 `MAP_SHARED`、read/write/pwrite、truncate
- 最后验证：2026-08-09；RV64 8 核 `buildstorm_file_probe` 与完整 BuildStorm OOM 边界
- 证据：`os/src/fs/page_cache.rs`、`os/src/fs/file.rs`、`os/src/mm/memory_set.rs`
- 内容：只在全局表中共享各个 VMA 的 frame 仍会与 PageCache 形成两份普通文件页，需要 read overlay、
  write mirror 才能维持一致性，并会为累计 mmap 页留下大量弱引用树节点。当前普通文件 mmap 直接克隆
  PageCache 的 `FrameTracker`；read/write 天然访问同页，truncate shrink 清零仍被映射持有的 frame。
  全局弱表及 overlay/update 只用于没有 PageCache 的兼容后备文件，并会清理失效弱引用。
- 后续影响：文件一致性 probe 应包含同页 mmap+pwrite 双向观察与 truncate/regrow 零填充，不能只在
  munmap/close 后比较磁盘内容；PageCache 回收也必须检查 frame 是否仍被 VMA pin。

## 用户栈溢出可能伪装成动态解释器权限故障

- 状态：已确认并修复 tg-xtask 启动 SIGSEGV
- 适用范围：大型动态 ELF、exec 初始栈、相邻 VMA fault
- 最后验证：2026-08-08；旧 BuildStorm 镜像 RV64 8 核
- 证据：fault `sepc=0x9bbbe0`、目标 `0x3000017298` 与当时栈/解释器 VMA 布局；
  `os/src/arch/{rv64,loongarch64}/config/mm.rs`
- 内容：原 512 KiB 栈向下越过 guard 后命中解释器 RX VMA，表象是 loader 区域 store permission fault。
  将栈窗口扩大为 8 MiB 且保持 lazy 后，tg-xtask `--help` 正常退出。
- 后续影响：遇到解释器附近 fault 不应只审查 PT_LOAD flags；先根据 SP 和写地址检查是否为栈越界。

## lwext4 的 truncate 扩容不能只返回成功

- 状态：已确认并修复 BuildStorm minibuild ELF 截断
- 适用范围：稀疏文件、PageCache 离散脏区写回、linker 输出
- 最后验证：2026-08-08；RV64 8 核 `buildstorm_file_probe` 与官方 BuildStorm minibuild
- 证据：`vendor/lwext4_rust/c/lwext4/src/ext4.c::ext4_ftruncate_no_lock()`、
  `os/src/fs/ext4/inode.rs::write_at()`；修复前产物实际结束于 `0x395d30`，而 ELF 节表要求到
  `0x40cd30`，差值 `0x77000` 正是被错误压缩的稀疏区；修复后 `BUILDSTORM_MINIBUILD ok`
- 内容：旧实现对 `old_size < new_size` 返回 `EOK` 却不更新 inode 或 open-file size，随后
  `file_seek()` 又把目标偏移夹到旧 EOF，导致远端脏区被追加到错误位置。增长 truncate 应只更新
  inode size 并保留洞，不需要为洞分配数据块。
- 后续影响：稀疏写回回归必须检查最终长度、洞区为零以及尾部模式数据，不能只检查 write 返回值。

## 可写共享文件映射不能永久冻结 mmap 时的 EOF

- 状态：已确认并修复专项 mmap 扩容回归
- 适用范围：`MAP_SHARED | PROT_WRITE`、mmap 后 ftruncate 扩容、munmap/msync 写回
- 最后验证：2026-08-08；RV64 8 核 `buildstorm_file_probe`
- 证据：`os/src/mm/memory_set.rs::mmap_file_backing()` 与
  `MemorySet::prepare_file_writeback()`
- 内容：映射建立后文件可能扩容，原先超出旧 EOF、但仍位于映射窗口内的页随之变为有效页。VMA
  若把 mmap 时的 file length 永久作为写回上限，会静默丢弃这些页。共享 VMA 应保存完整映射窗口，
  写回时再以当前 EOF 裁剪，避免重扩展后来被截短的文件。
- 后续影响：mmap 文件回归需包含“先映射短文件、后扩容、再写新增尾页”，仅测试固定长度映射不足。

## lwext4 的 superblock 与 inode 操作必须使用同一把全局锁

- 状态：已确认并修复当前 BuildStorm linker 阻塞
- 适用范围：SMP 下所有 lwext4 C API，包括文件/目录操作、sync、statfs 和 shutdown
- 最后验证：2026-08-08；RV64 8 核官方 BuildStorm
- 证据：`os/src/fs/ext4/{inode.rs,super_block.rs}`；修复前 PC 长期位于
  `ext4_dir_write_entry` 的断言路径，修复后 `ld.bfd` 完成且 collect2/gcc/rustc/cargo 正常退出
- 内容：lwext4 的 mount、block cache 与目录遍历状态位于共享 C 对象中。只串行 inode 操作而让
  superblock flush/statfs/shutdown 并发进入，会破坏其内部状态；增加另一把锁也不能建立正确互斥。
- 后续影响：任何新增 lwext4 入口必须复用 `EXT4_OP_LOCK`，不能按 Rust 模块各建一把锁。

## UNIX stream socket 必须显式传播 peer close

- 状态：已确认并修复当前 cargo/rustc 退出阻塞
- 适用范围：`socketpair(AF_UNIX, SOCK_STREAM)`、connect/accept、read/write/poll
- 最后验证：2026-08-08；RV64 8 核官方 BuildStorm
- 证据：`os/src/net/socket.rs`；修复前 cargo 永久等 `recvfrom`，修复后 linker 及完整编译子进程退出
- 内容：仅靠接收队列是否为空无法区分“暂时无数据”和“对端已经关闭”。端点必须共享 close 状态；
  peer close 后 read 先排空已有数据再返回 0，write 返回 `EPIPE`，poll 同时报告相应 readiness。
- 后续影响：修改 UNIX socket clone/drop/connect/pair 时需一起审查端点引用和 close 状态，不能只修
  阻塞 read 分支，否则 poll 或 write 会继续产生不一致行为。

## UNIX socket 的空读/满写/accept 不能用 yield 轮询

- 状态：已确认并修复专项与真实 Cargo 活性窗口
- 适用范围：AF_UNIX socketpair、pathname listener、Cargo/rustc IPC、多核调度
- 最后验证：2026-08-10
- 证据：旧 600 秒窗口最多 69,194 次 unix yield；event waiter 专项为 unix/scheduler yield 0、
  blocking switch 5，真实 Cargo timeout 窗口 net/unix 仍为 0
- 内容：只把 yield 换成 sleep 会增加延迟且仍需轮询；只在条件检查后再登记 waiter 会丢失恰好发生的
  write/read/connect/close。条件状态、waiter 登记和 blocked 发布必须与同一 buffer/pending lock 配合，
  producer 在修改条件后取出 waiter，并在释放数据锁后唤醒。
- 后续影响：验证不能只看“没有 yield”，还要验证 data integrity、buffer-full backpressure、peer EOF/
  EPIPE、nonblock EAGAIN、accept/connect、signal EINTR 和 producer-before-switch 竞态。

## RV64 `sret` 前不能提前恢复 SIE 或 user trap vector

- 状态：已确认并修复
- 适用范围：RV64 trap return、exec 初始上下文、signal frame 恢复、SMP timer interrupt
- 最后验证：2026-08-07；8 核 BuildStorm 运行中 GDB CSR 快照
- 证据：`os/src/arch/rv64/trap/{context.rs,trap.S}`；修复前
  `/tmp/respos-rustc-pc-sample{1,2,3,4,5}.txt`，修复后
  `/tmp/respos-rustc-pc-postfix-sample{1,2,3,4,5}.txt`
- 内容：若 `__restore` 先把 `stvec` 指向 `__trap_from_user`，再原样写回带 `SIE=1` 的
  `sstatus`，timer 可在 `sret` 前进入 user trap 汇编。此时 `sscratch` 和 GPR 尚未形成
  user-entry 契约，`csrrw` 后的保存指令会递归 StorePageFault；表面上多个 vCPU 仍活跃，
  实际任务已不能前进。
- 后续影响：恢复汇编必须在写 `sstatus` 前无条件清 `SIE`，保持 kernel `stvec` 直到所有
  可能 fault 的恢复完成，并由 `sret`/`SPIE` 完成最终中断状态转换。不能只依赖某一种
  TrapContext 构造路径清位，因为 signal return 等路径也可能提供保存上下文。

## exec 必须在旧地址空间中清理 sibling thread 的用户地址

- 状态：顺序已修正，远端 sibling 的协作式终止协议已实现
- 适用范围：多线程 exec、robust futex、`clear_child_tid`、共享 `MemorySet`
- 最后验证：2026-08-07；RV64 8 核 BuildStorm 受控 trace
- 证据：`os/src/task/task.rs::close_other_threads_for_exec()` 与 `exit_process_group()`；
  `os/src/task/processor.rs::publish_saved_handoff()`；`/tmp/respos-buildstorm-user-fault-trace.log`
- 内容：线程保存的 robust-list 和 clear-child-tid 是旧用户映像中的地址。若先替换
  共享 `MemorySet` 的内容，再清理 sibling，内核会把旧地址当作新程序地址写零或写
  `FUTEX_OWNER_DIED`，可破坏刚装载的映像。
- 后续影响：必须在替换 MM 前处理依赖旧用户地址的线程私有状态。当前实现采用四步
  quiescence 协议：`request_termination()` 标记 sibling 不可再被 claim → `remove_task()`
  从调度器摘除 → spin-wait `has_cpu_owner()` 等待远端 CPU 切回 idle → 安全清理。
  `publish_saved_handoff()` 对已标记 `terminate_requested` 的 task 不再重新发布到 ready
  queue，而是静默丢弃。`can_be_claimed_on_cpu()` 和 `try_claim_running_on_cpu()` 均拒绝
  已标记终止的 task。

## exec argv/envp 不能用很小的固定项数上限

- 状态：已确认并修复当前 BuildStorm E2BIG
- 适用范围：`execve`/`execveat`、动态工具链、大环境表
- 最后验证：2026-08-07；RV64 8 核 cargo/rustc
- 证据：`os/src/mm/mod.rs::extract_cstrings_from_user()`；
  `/tmp/respos-minibuild-pipe-output.log`
- 内容：argv 和 envp 共用的 32 项限制使 cargo 启动 rustc 时返回 `E2BIG`，cargo 只能报
  child `never executed`。项数放宽必须同时限制累计字节，否则会把用户指针数转化为无界
  kernel heap allocation。
- 后续影响：调试工具链 exec 必须保留 cargo 捕获的 child stderr；仅看顶层
  `BUILDSTORM_MINIBUILD fail` 会丢失具体 errno。

## 跨 CPU 自旋锁不能防止同 CPU 中断重入

- 状态：已确认并修复当前 heap/timer 路径
- 适用范围：全局 heap、kernel timer trap、任何同时被 syscall 和中断访问的状态
- 最后验证：2026-08-06；RV64 debug、2/4/8 核
- 证据：`/tmp/respos-smp8-gdb-bt1.txt`、`/tmp/respos-smp2-dynamic-bt.txt`；
  `os/src/mm/heap_allocator.rs`、`os/src/arch/rv64/trap/mod.rs`
- 内容：一个 CPU 持有普通 spin lock 时若仍可被使用同一把锁的中断打断，中断会永久等待
  被它自己暂停的临界区。实测的两种形态是 heap dealloc 被 timer 中断后再 alloc，以及
  `ACTIVE_ITIMER_TASKS.remove()` 被 timer 中断后再 lock。其他 CPU 的锁等待只是级联现象。
- 后续影响：GDB 审查要保留“中断栈下方的被打断栈”，不能只看顶层等锁 CPU。修复时要么
  让底层锁关本地中断，要么把中断工作延迟到无锁安全点；不能仅增加跨 CPU 锁或换 allocator。

## `CLONE_VFORK` 必须先登记父进程 blocked 再发布子进程

- 状态：已确认并在当前工作树修复
- 适用范围：glibc `vfork`/`posix_spawn`/`popen`，以及所有带 `CLONE_VFORK` 的 clone 调用
- 最后验证：2026-08-06；RV64、SMP=1 的原语义回归与 SMP=8 BuildStorm 受控 trace
- 证据：`os/src/syscall/process.rs::sys_clone()`、`os/src/task/task.rs::execve()` 与
  `exit_process_group()`；修复后的 4 路 CAgent pass 日志
  `/tmp/cagent_debug_vfork_fix_4_busybox/`
- 内容：Linux 语义要求 vfork 父任务一直阻塞到子任务成功 exec 或 exit。仅调用
  `blocking_and_run_next()` 而不建立子到父的唤醒边，会使父任务错误地等到子命令退出；普通 exit
  的 SIGCHLD 唤醒会掩盖这个错误，造成 `popen` 很慢而 `pclose` 几乎立即返回。SMP 下还有更窄的
  窗口：若先发布 child、后把 parent 加入 blocked 表，child 可先 exec 并把一次性 wake 丢掉。
  cargo/rustfmt 曾表现为捕获输出不返回，但最新 trace 已确认顺序修复后 parent 正常恢复且 rustfmt
  完成退出，仍有独立的 pipe 引用未释放问题；两者不能混作同一个根因。
- 后续影响：vfork 同步必须是一次性且只限该 clone 关系；exec 仅在新映像状态完整后释放，退出路径
  也必须释放以覆盖 exec 失败。父 waiter 必须先登记，child 才能进入 ready queue；不要以普通
  SIGCHLD、yield 或单核通过代替该协议。

## lazy VMA 销毁不能按虚拟跨度逐页扫描

- 状态：回收复杂度已修复，完整 BuildStorm 仍待验证
- 适用范围：进程退出、munmap 辅助路径、稀疏匿名/文件 VMA
- 最后验证：2026-08-06；RV64 8 核 cargo/rustfmt exit trace
- 证据：`os/src/mm/memory_set.rs::MapArea::unmap()`；
  `/tmp/respos-buildstorm-exittrace.log`
- 内容：lazy VMA 可能保留很大的虚拟区间但只有少量 resident frame。销毁时遍历整个 VPNRange 会让
  exit 成本与地址预留大小成正比。Framed area 应只遍历 `data_frames`；Direct area 才按完整
  区间解除既有恒等映射。最新 rustfmt trace 已越过进程退出，但 pipe open-file 仍有额外引用，
  因此 sparse unmap 优化不能作为 EOF 问题已解决的证据。
- 后续影响：对 lazy 结构做回收、fork、mprotect 或统计时优先按 resident metadata 工作；需要改变
  整个 VMA 属性时操作区间元数据，不能隐式扫描每个潜在页。

## 共享进程资源的退出归属不能用 `Arc::strong_count` 猜测

- 状态：已确认并修复当前 BuildStorm 并行 rustc ENOMEM/SIGSEGV 根因
- 适用范围：多线程进程组退出、延迟 TCB drop、zombie、`CLONE_VM`/`CLONE_FILES`
- 最后验证：2026-08-09；RV64 1 GiB/8 核 `frame_reclaim_probe` A/B 与 8 GiB fault-only BuildStorm
- 证据：`os/src/task/task.rs::exit_process_group()`、`user/src/bin/frame_reclaim_probe.rs`；guest
  `/proc/respos_health` 前后值记录于 `current-status.md`
- 内容：worker 可以已离开 thread group，却仍因 context-switch handoff 或 `DEAD_TASKS` 持有 TCB；
  此时它对共享资源的临时强引用不是另一个存活进程。用强引用总数与当前成员数比较会误判外部 owner，
  跳过地址空间清空；zombie leader 再把整个 resident set 固定到父进程 wait/reparent 之后。归属判断
  应扫描 live task identity：只有不同 tgid、仍存活且指向同一资源的任务才阻止本组 teardown。
- 后续影响：不要以扩大 guest 内存或 heap 掩盖线性 frame 泄漏。新增共享 TCB 资源时必须明确区分
  live owner、zombie owner 与退出路径临时引用，并用“分配/触碰—多线程退出—wait—空闲量恢复”短测
  建立门禁。

## pub 镜像不能在 QEMU 运行时由宿主修改

- 状态：已确认
- 适用范围：`img/sdcard-*-pub.img`、QEMU、`debugfs`、`e2fsck`
- 最后验证：2026-08-03
- 证据：RV64 pub 镜像的正常 guest `quit` 后 `e2fsck -fn`；lwext4 卸载路径和
  `/tmp/ext4-flush-pre-e2fsck.log`
- 内容：guest 正常 reboot 会依次停止 journal、卸载 ext4 并等待 virtio-blk FLUSH；只有看到
  QEMU 退出后，宿主才可运行 `e2fsck` 或 `debugfs -w`。强制终止 QEMU、在其持有 raw image
  时修改镜像、或在 gzip 解压尚未完成时运行 fsck，都会造成 journal recovery、短读或元数据
  不一致，不能归因于单个 CAgent syscall。
- 后续影响：每轮写入诊断 runner 前先恢复完整镜像并 `e2fsck -pf`；测试后先正常 guest
  `quit`、确认 QEMU 退出，再离线提取或修改文件。不可恢复的强制停止后应从压缩基线恢复镜像。

## 多核诊断应使用 QEMU `-snapshot`

- 状态：已确认
- 适用范围：RV64 SMP 压力、超时、GDB 暂停和所有不需要持久保存 guest 写入的诊断
- 最后验证：2026-08-05；RV64 `-smp 8 -m 256M` timeout/exit 压力
- 内容：SMP 压力常需超时后暂停 QEMU 或接入 GDB，不能保证 guest 有机会正常卸载 ext4。直接对
  raw pub 镜像运行会把临时 `/tmp`、journal 及非正常退出混入后续结果。
- 后续影响：诊断启动命令在 raw drive 之外加入 `-snapshot`，例如
  `qemu-system-riscv64 ... -snapshot -smp 8 -m 256M`。只有确实需要保留 guest 文件时才退出 snapshot
  模式，并遵守上一节的正常 guest `quit`/离线 fsck 流程。

## `make rv/la` 返回 0 不等于测试通过

- 状态：已确认
- 适用范围：完整 QEMU 测试
- 最后验证：2026-08-01
- 证据：当前 `make rv`/`make la` 均退出 0，但 LTP 各有 600 余项失败
- 内容：顶层命令通过 pipefail 运行 QEMU 并 tee 日志；testrunner 即使记录用例失败仍会跑完并主动
  关机，因此外层成功主要表示 QEMU 正常结束。
- 后续影响：验收必须解析 summary、TBROK、segfault、wrapper exit code 和分组 fail。

## writable file `MAP_SHARED` 拒绝造成全局级联

- 状态：已确认
- 适用范围：basic、lmbench、LTP 以及依赖共享控制页的用户程序
- 最后验证：2026-08-01
- 证据：`os/src/fs/file.rs::mmap_allowed`、`os/src/syscall/mm.rs`、当前双架构日志
- 内容：当前策略返回 `EOPNOTSUPP`。LTP 框架自身使用 fd-backed writable shared page，所以看似
  无关的 getpid/fcntl/fs 等用例在初始化时一起 TBROK。
- 后续影响：不要逐个修数百个 LTP case；先修 shared file mapping 协议。也不要直接删除检查，
  因为旧实现仍有锁内 I/O 和写回错误传播风险。

## MM 锁内后端 I/O 是长期锁序风险

- 状态：已确认
- 适用范围：file fault、shared unmap/writeback
- 最后验证：2026-08-01
- 证据：`os/src/mm/memory_set.rs`、`docs/四天内核重构-ABC-整合审查.md`
- 内容：部分 file backing fault/writeback 仍可能持有 `MemorySet` 写锁访问文件后端；munmap 写回
  错误也没有完整传播协议。
- 后续影响：设计 writable shared 时把文件读写移到地址空间锁外，并在重新加锁后校验映射版本
  或身份，再 commit。

## LTP 的首个框架错误会污染后续结论

- 状态：已确认
- 适用范围：LTP harness 调试
- 最后验证：2026-08-01
- 证据：当前 mmap 初始化级联；`user/src/bin/testrunner.rs`；历史 `pipe2_02` helper 级联记录
- 内容：测试可能在 `tst_test.c` 资源准备、mount、helper exec 或共享控制页阶段失败，尚未进入目标
  syscall assertion。后续大量相同错误不是大量独立内核缺陷。
- 后续影响：先定位第一个不同的失败点，并从 testsuit/image 脚本确认它属于 setup 还是测试主体。

## 未跟踪用户程序仍会进入本地构建

- 状态：已确认
- 适用范围：`user/src/bin/` 与内核内嵌应用
- 最后验证：2026-08-01
- 证据：`user/Makefile` 的 `$(wildcard src/bin/*.rs)`；当前构建日志编译了未跟踪 FS probe
- 内容：Git 未跟踪不等于构建不可见。任何放在 `user/src/bin` 的 `.rs` 都会被构建并可能嵌入内核，
  影响大小、编译时间或应用清单。
- 后续影响：报告可复现结果时附 `git status --short`；干净 CI 与本地 dirty tree 可能产物不同。

## futex cmp-requeue probe 默认构建不是有效竞态测试

- 状态：已确认
- 适用范围：`task_a_futex_cmp_requeue_probe`
- 最后验证：2026-08-06
- 证据：`os/src/task/futex/wait.rs` 的 `TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD`；
  `docs/四天内核重构-ABC-整合审查.md`
- 内容：probe 期待 changer 必定在线性化点前改值；只有用
  `TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD=1` 编译内核才会强制形成该窗口。默认内核直接运行时，
  cmp-requeue 合法地先完成并返回 affected count，probe 会产生伪失败。
- 后续影响：专项测试后必须恢复默认 build。即使专项 build 一次通过，也要记录重复轮数和任何
  timeout；不能删除非收敛样本。

## RV/LA 构建共享活动 Cargo config

- 状态：已确认
- 适用范围：顶层和子目录构建
- 最后验证：2026-08-01
- 证据：`Makefile`、`os/Makefile`、`user/Makefile`
- 内容：构建目标把架构模板复制到同一个 `.cargo/config.toml`。并行运行两个架构构建可能在
  config 被覆盖时使用错误 target/linker 设置。
- 后续影响：双架构顺序构建；若未来需要 `make -j`，先改成互不共享的配置/target dir 协议。

## 比赛镜像是可变测试状态

- 状态：已确认
- 适用范围：mount、文件、LTP、benchmark
- 最后验证：2026-08-01
- 证据：顶层 QEMU 以 raw 可写方式挂载 `img/sdcard-*.img`；`scripts/get_img.sh`
- 内容：测试会创建 `/tmp`、`/etc`、symlink、benchmark 文件和 mount backing。前一轮异常退出可能
  改变后一轮失败形态。
- 后续影响：难以解释的非确定性首先用保留的 `.xz` 恢复镜像，再比较；记录镜像版本和 hash。

## glibc 工具会暴露未覆盖的合法 open flag

- 状态：已确认
- 适用范围：glibc pub 镜像中的 coreutils/CAgent 文件命令
- 最后验证：2026-08-02
- 证据：`os/src/fs/file.rs::OpenFlags`、`os/src/syscall/fs.rs::validate_open_flags()`、
  glibc `/usr/bin/touch` 的 `EINVAL` 复现
- 内容：BusyBox 和 glibc 工具的等价操作不一定使用相同的 open flag。此次 glibc `touch`
  携带 Linux ABI 规定的 `O_NOCTTY`，旧实现因未声明该位在进入 namei 前返回 `EINVAL`。
- 后续影响：遇到用户态工具特定失败时，先保存实际 syscall flags 并对照 Linux ABI；对纯兼容、
  不改变普通文件状态的合法 flag 可接受为 no-op，不能按测试名添加特判。

## 网络 `Connection refused` 可能是 ready 时序而非 ABI

- 状态：已确认并修复（当前工作树）
- 适用范围：多个 client 同时连接同一个 smoltcp TCP listener
- 最后验证：2026-08-03
- 证据：CAgent debug 的 agent exit 255 与 `Connection failed to 127.0.0.1:8080`；
  `os/src/net/listen.rs`；修复后 RV64 三轮官方 CAgent 日志
- 内容：服务端已经 listen 不代表单个 smoltcp listener 能承接同一轮 poll 中的全部 SYN。旧实现等
  userspace `accept` 时才补 listener，第一个握手占用 listener 后，其余并发 SYN 会被 reset。该错误
  会随机落到任意 agent，因而可能伪装成 `kernel`、`cpu` 或 FS 命令失败。
- 后续影响：先保存 agent 原始日志和退出码；若是 connect 失败，不要修改 `uname`、`nproc` 等固定
  命令。监听实现必须按 backlog 提前提供容量，而不是在客户端硬编码重试。

## CAgent 全量 reject 可能来自单线程 LLM server 排队

- 状态：已初步定位，仍需并行/串行对照确认
- 适用范围：题一 `/glibc/cagent_testcode.sh`，`SMP=1` 的 10 个并行 agent
- 最后验证：2026-08-03
- 证据：`testsuit/cagent-test/simple_llm_server.c` 的 `listen(server_fd, 10)`、主循环中的
  `accept` 后直接 `handle_client(client_fd)`；`/tmp/respos-integrated-rv-cagent.log`；当前整合
  提交的单 agent `kernel` 复现
- 内容：官方 runner 同时 fork 10 个 `agent_lite`，但固定 server 不为每个连接创建线程或子进程，
  请求处理是串行的。当前全量运行虽收到请求，却让各项耗时达到约 43–60 秒并超过官方
  20–35 秒 timeout；单 agent `kernel` 能正常获得 `6.10.0-dev` 并以 exit 0 完成。
- 后续影响：不能把这轮 10/10 reject 直接归因于 `uname`、文件系统或 TCP ABI。先用保留日志的
  单项/串行 runner 复现，再单独测 server 并发；A 的 listener backlog 修复仍需保留，因为它
  解决的是连接承载能力，不能消除 server 应用层串行排队。

## 在待恢复 journal 的镜像上写入 debug 文件会丢失

- 状态：已确认
- 适用范围：从 `.gz` 恢复 pub ext4 镜像后使用 `debugfs -w`
- 最后验证：2026-08-03
- 证据：RV64 镜像注入的 `/glibc/cagent_debug.sh` host/image SHA-256 一致，但首次启动 journal
  recovery 后 guest 中变为 0 字节；先运行 `e2fsck -pf` 后重新注入可正常执行
- 内容：压缩包中的 ext4 可能带 `needs journal recovery`。在回放旧 journal 前直接写新 inode，启动
  时的恢复会用旧元数据覆盖该写入。
- 后续影响：只读查看不受影响；临时写镜像前先恢复 journal 并再次校验内容。正式干净回归不要
  注入 debug 文件。

## lwext4 的 CMake 生成目录也跨架构共享

- 状态：已确认
- 适用范围：RV64/LA64 快速切换构建
- 最后验证：2026-08-03
- 证据：`vendor/lwext4_rust/build.rs`、`c/lwext4/Makefile`；失败日志中 LA C compiler 配合 RV
  `ar/ranlib`，手动顺序执行目标架构 `make musl-generic ARCH=...` 后重试成功
- 内容：除了活动 Cargo config，lwext4 两架构还共用 `c/lwext4/build_musl-generic`。切换架构时
  可能出现 C compiler 已切换但 archive 工具仍来自上一架构的混合配置。
- 后续影响：不要并行构建两架构。若顶层构建在 archive 阶段出现交叉工具混用，先顺序执行
  `make musl-generic ARCH=riscv64|loongarch64 -C vendor/lwext4_rust/c/lwext4` 再重试；长期应把
  CMake build dir 按架构拆分。

## libctest wrapper 的 256 需要解码

- 状态：待验证
- 适用范围：musl libctest static/dynamic
- 最后验证：2026-08-01
- 证据：RV/LA 日志均显示 `run-static.sh`、`run-dynamic.sh exited with code 256`
- 内容：可见子用例大量显示 `Pass!`，但 runner 打印的是 wait status/封装值，尚未确认是脚本真实
  exit 1、状态解码问题还是隐藏子用例失败。
- 后续影响：检查 waitpid ABI 和脚本最终命令，不能把“屏幕上都是 Pass”当成 wrapper 成功。

## 高半区模型与 LoongArch 2 MiB 边界历史问题

- 状态：待验证
- 适用范围：LoongArch mmap、TLS、pthread、页表边界
- 最后验证：2026-08-01
- 证据：当前高半区源码已确认；2 MiB TLS 故障仅来自 2026-06 历史 Codex 记忆，当前源码未重现
- 内容：历史调试曾观察到小匿名 mmap 跨 2 MiB 页表边界与 pthread/TLS 故障相关，并采用 LA
  特定 mmap placement 缓解。当前是否仍需要该缓解尚未专项复验。
- 后续影响：若 LA-only pthread fault 重新出现，优先记录 fault VA、TP/TLS、PMD 边界和 mmap
  placement；在复现前不要把旧解释写成当前缺陷。

## rename/unlink 仍受 path-based ext4 后端限制

- 状态：已确认
- 适用范围：多硬链接、rename 覆盖、unlink 后打开文件
- 最后验证：2026-08-01
- 证据：`os/src/fs/ext4/inode.rs`、`os/src/fs/namei.rs`、整合审查
- 内容：open-file 计数已替代 `Arc::strong_count` 猜测，但后端仍通过 renamed/orphan path 兼容
  path API；多别名的完整 inode-handle/事务语义尚未建立。
- 后续影响：不要用更多路径特判宣称完整 POSIX 语义；复杂 rename 需要统一 inode identity/backup
  设计和失败注入。

## 历史测试成绩会快速过期

- 状态：已确认
- 适用范围：README、汇报和分支比较
- 最后验证：2026-08-01
- 证据：README 历史“600 余 LTP”与当前 LTP 初始化失败并存
- 内容：同一仓库的不同 commit、镜像、内存配置、libc 和 runner 清单会产生完全不同的结果。
- 后续影响：任何成绩必须携带 commit、日期、架构、镜像、命令和 summary；旧记忆只用于寻找线索。
