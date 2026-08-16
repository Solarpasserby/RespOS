# RespOS 设计决策记录

这里只收录能解释当前代码形态或避免重复踩坑的决策。日期是当前证据最后核验时间，不一定是
最初提出时间。

## 现场赛开发以真实软件首失败驱动，网络以 Git HTTP(S)/SSH 为交付目标

- 状态：已采用
- 适用范围：`ae2f38ce` 之后的现场赛健壮线、Phase 5/POSIX 排序、virtio-net 分工与答辩展示
- 最后验证：2026-08-16
- 证据：官方 2023--2025 现场赛 README；当前源码只有 loopback 网络和 virtio block driver；
  [software-compatibility-network-plan.md](./software-compatibility-network-plan.md)
- 决策：当前主线优先跑 Git/Vim/GCC/rustc，由真实软件的第一个稳定失败触发必要的 POSIX/Linux ABI
  修复；队友优先建立真实 virtio-net，并以 Git HTTP(S) 和 SSH 的 clone/fetch/pull、条件允许时 push
  作为端到端交付。简单 HTTP server 降为可选诊断工具。两条线通过 Git 本地基线和远端 transport 汇合。
- 原因：近三年官方任务已经验证完整软件和网络组合路径，继续无差别扩展接口清单的边际收益低于真实
  workload 驱动；Git 远端操作还能同时覆盖 NIC、DNS、TCP、事件、时间、随机数、TLS/SSH 用户态依赖和
  文件系统一致性。
- 后续影响：不能用 loopback 测试、QEMU 设备参数或单个 syscall 成功宣称网络/应用兼容。未取得绑定
  commit、镜像、架构和日志的结果一律标记 `待验证`；共享入口实行单写入者，凭据不得进入仓库或日志。

## 比赛提测固定在已验证性能提交，Phase 5 健壮版保留给现场赛

- 状态：已采用
- 适用范围：线上最终提测、BuildStorm 评分、Phase 5/POSIX 后续分支
- 最后验证：2026-08-16
- 证据：提测候选 `44f93dbb6bfa6e615cc489a0c4e75309e5d56b94`；健壮版
  `70df39f85b8598c94d805645cd4b236a25e6991d`；两者完整 RV final 与 CPU-clock 分片实验日志见
  [current-status.md](./current-status.md)
- 决策：线上提测使用 `44f93db`，它包含 I/O buffer pool 与 `/proc/uptime` 修复，并已有 CAgent 10/10、
  BuildStorm `633.48s` 的完整证据；`origin/main` 和 `contest/main` 当前也指向该提交。`70df39f8` 保留为
  Phase 5/POSIX 健壮线，供现场赛需求和后续语义工作使用，不在截止前继续叠加未经稳定 A/B 证明的性能
  重构。生成提测产物时必须从显式 `44f93db` 的干净 worktree 构建，不能复用当前分支后来生成的
  `kernel-rv/kernel-la`。
- 原因：健壮版完整 final 功能通过，但同机 8 GiB RV 样本为 `684.79s/729.94s`，相对已验证评分提交
  没有性能优势；继续优化的时间和回归风险高于临近提测的期望收益。现场赛可能更依赖稳定 process
  identity、leader exit/non-leader exec、signal/job-control、mmap/truncate/punch/ENOSPC、时间戳与 rusage
  语义，因此保留而不丢弃该线。
- 后续影响：不得把“线上选旧提交”解释为 Phase 5 修复无效，也不得把两个分支的测试证据混用。若未来
  要把健壮线重新作为评分候选，至少需要固定资源的完整 A/B、相同产物哈希、双架构专项和官方 final
  门禁；没有正收益的优化实验应回退而不是累积。

## `posix_fadvise` 状态属于 open file description，DONTNEED 只失效安全页

- 状态：已采用，当前工作树待提交
- 适用范围：regular file、dup/fork、PageCache、buffered I/O、mmap、writeback error cursor
- 最后验证：2026-08-16
- 证据：Linux `mm/fadvise.c`；`scripts/fadvise_phase5_probe_linux.c`；双架构专项与 LTP 日志见
  [current-status.md](./current-status.md)
- 决策：NORMAL/RANDOM/SEQUENTIAL/NOREUSE 状态保存在共享 `FileInner`。DONTNEED 先执行范围 writeback，
  忽略该请求的 writeback 错误，再只驱逐完整覆盖的 clean 且无外部 frame 引用页；到 EOF 可包含末尾部分
  页。tmpfile 不执行这种 eviction。WILLNEED 是 best effort，当前同步实现不构成永久 ABI 承诺。
- 原因：Linux 的 access mode 是 open-file-description 状态；部分页无 byte-granular cache，dirty/mapped
  页也不能在不丢数据或破坏共享 frame identity 的前提下直接失效。tmpfile 的 PageCache 本身就是数据源。
- 后续影响：未来 async writeback/reclaim 必须保持错误 cursor、dirty generation 和 frame pin 不变量；
  测试不得把 WILLNEED 返回时驻留、DONTNEED 必然释放内存或 writeback 错误同步返回提升为 ABI。

## lazytime 延迟 lower commit，但不延迟共享 inode 可见性

- 状态：已采用，当前工作树待提交
- 适用范围：`MS_LAZYTIME`、relatime、ext4 inode/dentry cache、background expiry/eviction、
  fsync/sync/unmount/reboot
- 最后验证：2026-08-16
- 证据：`os/src/fs/{file.rs,mount.rs,dentry_cache.rs,ext4/{inode,super_block}.rs}`；双架构 release/perf、
  eviction 与 crash-image 日志 `/tmp/respos-{rv,la}-atime-dirtytime-eviction-phase5{,-perf}.log`、
  `/tmp/respos-{rv,la}-lazytime-crash-{prepare,verify}.log`
- 决策：自动 atime 始终先发布到按 inode identity 共享的 metadata cache。非 lazytime 立即 lower commit；
  lazytime 记录 pending generation 并以强引用 registry 保活，文件 fsync 提交自身，filesystem-wide
  同步边界先完成 dirty data/mtime/ctime，再提交该 filesystem 全部 lazy atime 并做底层 barrier。普通
  ext4 File 不保存私有 atime override。首次 dirty 的 monotonic 时间驱动可配置 background expiry；最后
  真实 dentry/file owner 的 eviction 也提交 pending，失败保持 registry owner 并重试。
- 原因：lazytime 的契约是减少 timestamp I/O，同时 stat 必须立即看到新时间；把可见状态放在 fd cache
  会破坏 pathname/其他 fd 的一致性，把 pending 只留 Weak cache 又可能在持久化前丢失 inode。
- 后续影响：background writeback/eviction 必须复用同一 generation 清除规则，并在 lower I/O 前释放
  registry/dentry cache 锁；不得用关闭 stat 可见性、全局每次 read 落盘或只在关机时 best-effort 写回来
  替代。tmpfile 因没有 lower inode，继续单独使用 open-file override。

## System realtime 与硬件 RTC 保持独立，只在启动时显式同步

- 状态：已采用，当前工作树待提交
- 适用范围：`clock_settime/settimeofday`、`RTC_RD_TIME/RTC_SET_TIME`、reboot、filesystem wall clock
- 最后验证：2026-08-16
- 证据：Linux RTC class/goldfish driver 与 QEMU goldfish/LS7A RTC 状态机；
  `/tmp/respos-{rv,la}-rtc-set-phase5-current.log`、`/tmp/respos-{rv,la}-rtc-reset-persist-phase5.log`
- 决策：system realtime 继续由 monotonic + offset 表示，`clock_settime/settimeofday` 只改该 offset；RTC
  ioctl 直接读写平台 RTC 且不改 system clock。启动时唯一一次由 RTC 建立 system offset。reboot restart
  保留同一设备实例的 RTC offset，新 QEMU 进程不伪造电池后备状态。
- 原因：Linux 把 system clock 与 RTC class 作为两个时钟域，用户态 `hwclock --systohc/--hctosys` 或内核
  同步策略才负责显式复制；让任一 set 隐式修改另一域会破坏可观察 ABI，也无法区分 RTC 写回是否成功。
- 后续影响：新增 NTP/11-minute mode、RTC alarm 或真实硬件驱动时必须建立显式同步路径；不得再次让
  `RTC_RD_TIME` 返回 `REALTIME_OFFSET_US`，也不得把 QEMU reset persistence 描述为跨进程掉电保持。

## LA 启动期成对 4 KiB 内核叶默认使用 Global TLB 映射

- 状态：已采用
- 适用范围：LoongArch final kernel root、ASID 切换、软件 TLB refill；RV64 无行为变化
- 最后验证：2026-08-15
- 证据：`os/src/arch/loongarch64/mm/page_table.rs`、`os/src/mm/memory_set.rs`、
  `os/Cargo.toml`；512 MiB PageCache 的 off -> on -> off2 固定 120 秒 A/B 和 LA 4 GiB/12 hart 门禁
- 内容：Cargo 默认启用 `la_global_kernel`，但实际实现只在 LoongArch 编译。最终 kernel root 第一次
  激活前，仅当同一 TLB pair 的偶/奇 4 KiB leaf 都有效时同时设置 G 位；单边 leaf 清 G。两组相同
  工作量包围样本中，on 的 local sfence ticks 稳定下降约 13%，remote wait 下降约 16%--20%。
  高端 RAM 的 2 MiB huge leaf 与运行期新增 kernel-stack leaf 保持 ASID-scoped。
- 后续影响：不得为扩大 Global 覆盖猜测 huge-leaf G 位编码；普通 PTE writer、op=4 shootdown、
  active-hart residency 与 retired-frame completion 必须继续执行。关闭对照需显式使用 Cargo
  `--no-default-features`，不能把 RV 二进制哈希变化误报为 RV MM 行为变化。

## chroot 采用 Linux 的 pathname/permission 优先错误顺序

- 状态：已采用
- 适用范围：`chroot()`、pathname lookup、目录 search permission、privilege 检查
- 最后验证：2026-08-14
- 证据：`scripts/chroot_permission_probe_linux.c`、双架构 musl/glibc `chroot01`--`chroot04`
- 决策：即使调用者没有 chroot privilege，也先复制并解析 pathname，验证目标是可搜索目录；只有
  路径侧检查全部通过后才返回 `EPERM`，并在最后提交新 root。
- 原因：Linux 的可执行契约会优先暴露 `EFAULT/ENOENT/ENOTDIR/EACCES`，LTP `chroot03/04` 依赖该
  顺序；privilege-first 会丢失用户可观察的错误优先级，也妨碍验证失败原子性。
- 后续影响：未来若引入 `CAP_SYS_CHROOT`、user namespace 或 mount namespace，能力判定可以扩展，
  但不得提前遮蔽 pathname 与目录 search 错误。

## pwrite 在 O_APPEND 下选择 Linux ABI，不伪装成纯 POSIX 行为

- 状态：已采用
- 适用范围：`pwrite/pwrite64`、`O_APPEND`、LTP `pwrite04/pwrite04_64`
- 最后验证：2026-08-14
- 证据：`scripts/pwrite_append_probe_linux.c`、双架构 musl/glibc pwrite/pwritev 聚焦日志
- 决策：RespOS 在 `O_APPEND` 下与 Linux 一致，让 `pwrite` 写到当前 EOF 而不修改
  open-file offset。不采用 POSIX 文本中 `O_APPEND` 不影响 `pwrite` 显式 offset 的行为。
- 原因：当前项目以 Linux syscall ABI、实际 musl/glibc workload 和 LTP 为可执行契约；
  Linux 偏离是 LTP 明确测试的平台行为，继续保留纯 POSIX 选位会让四个目标环境都失败。
- 后续影响：覆盖矩阵必须把这一项标记为 Linux 兼容选择，不得将它宣称为严格
  POSIX 一致。mmap/splice 等内核定位写不经 pwrite ABI，不得继承该 append 规则。

## PID/TID 0 保留给 ABI 特殊语义，首个用户任务使用 PID 1

- 状态：已采用，当前工作树待提交
- 适用范围：task id allocator、initproc、session/process group、signal、wait
- 最后验证：2026-08-14
- 决策：`TidAllocator` 从 1 开始分配；initproc 的 TGID/PGID/SID 均为 1。syscall 参数 0 继续由各 ABI
  解释为当前进程、当前进程组或特殊 selector，不对应一个真实用户 TCB。
- 原因：从 0 分配会让整个初始进程树继承 SID/PGID 0，`getsid()` 暴露非正 session id，子进程的
  `setsid()`/process-group leader 关系也失去 Linux/POSIX 含义。只让 `getsid` 接受 0 返回值会隐藏基础
  身份错误。
- 后续影响：代码不得依赖 initproc tid 为 0；新增 pid lookup 要显式处理 selector 0。该决策不解决
  leader 单独 exit 或 non-leader exec，后两者仍需要稳定的 process/thread-group identity。

## socket send 的 EPIPE 与 SIGPIPE 在 syscall flag 层统一收口

- 状态：已采用，当前工作树待提交
- 适用范围：`sendto/sendmsg/sendmmsg`、AF_UNIX/TCP、`MSG_NOSIGNAL`
- 最后验证：2026-08-14
- 证据：`os/src/syscall/net.rs`、`scripts/socket_flags_probe_linux.c`、
  `user/src/bin/socket_flags_probe.rs`；RV64/LA64 4 GiB/2 hart 专项
- 决策：底层 socket 只报告 `EPIPE`；syscall flag 解析层统一决定是否向当前 task 投递 `SIGPIPE`。
  未设置 `MSG_NOSIGNAL` 时保持 Linux 的 `EPIPE + SIGPIPE`，设置后只返回 `EPIPE`。`write(2)` 继续由
  通用 fs syscall 路径负责同一信号语义，不把 signal policy 下沉到传输实现。
- 后续影响：新增 `send/sendmsg` 变体必须复用同一完成路径；不能因为调用最终返回短计数就遗漏后续
  message 的同步 SIGPIPE，也不能让 UDP/其他非 EPIPE 错误误触发该信号。

## `SO_ERROR` 消费 socket-owned pending error，poll 只观察

- 状态：已采用，当前工作树待提交
- 适用范围：TCP asynchronous connect、poll/epoll、getsockopt
- 最后验证：2026-08-14
- 证据：`os/src/net/{tcp.rs,socket.rs}`、`os/src/syscall/net.rs`；Linux/RespOS socket connect probes
- 决策：异步错误和连接状态保存在 `TcpSocket`，由协议推进路径一起提交；poll/epoll 报告 readiness 但
  不改变 error，`getsockopt(SO_ERROR)` 以原子 swap 返回并消费。syscall 层不从当前协议状态临时推断
  errno，也不继续固定返回 0。
- 后续影响：同一错误必须在多次 readiness scan 后仍可读取一次，消费后 `POLLERR` 不应由旧槽继续产生。
  添加真实网络 timeout/unreachable 时扩充错误映射，不能再增加第二套 syscall-local error owner。

## `make all` 固定为线上自动识别提交入口，本地阶段使用显式目标

- 状态：已采用
- 适用范围：顶层构建、线上提交、初赛复测、决赛本地回归
- 最后验证：2026-08-13
- 证据：课程平台实际 `make all` 日志、顶层 `Makefile`、`respos/profile`、
  `user/src/bin/{contest_launcher,testrunner}.rs`；
  `RUSTUP_TOOLCHAIN=nightly-2025-01-18 make check-submit`；双架构初赛/决赛 auto 启动日志
- 内容：`make all` 只顺序构建 `kernel-rv`、`kernel-la` 和包含 `mode=auto` 的
  `disk.img`、`disk-la.img`，不下载镜像、不启动 QEMU，也不接受本地 profile 覆盖。
  `.NOTPARALLEL` 防止双架构共享 Cargo config 竞态。本地使用 `run-{rv,la}-pre`、
  `run-{rv,la}-final`、`run-{rv,la}-diagnostic`；三类目标使用独立辅助盘。
  自动模式先检测官方根盘的 CAgent/BuildStorm 决赛脚本，再检测 musl/glibc basic 初赛脚本；
  preliminary 由 `contest_launcher` exec 内嵌 `testrunner`，final 则绕过它并执行官方根盘的
  CAgent/BuildStorm glibc 脚本。显式本地 profile 可强制阶段，不参与线上提交。
- 后续影响：不得把本地根镜像检查、QEMU 资源或诊断 profile 加进 `make all`。调整提交产物、
  profile 或 final 脚本协议前，必须先取得新的平台日志/公告证据并重新完成 Rust 1.86 双架构构建。

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

## ext4 特殊节点直接创建真实 lower inode，并贯穿 device payload

- 状态：已采用
- 适用范围：ext4 `mknod`/`mknodat`/`mkfifo`、特殊节点 stat/xattr 与命名 FIFO reopen
- 最后验证：2026-08-14
- 证据：`os/src/fs/{namei.rs,ext4/inode.rs}`、`os/src/syscall/fs.rs`、lwext4 `ext4_mknod`、
  `mknod_xattr_probe`；双架构 musl/glibc 13-case mknod/xattr 簇、4-case statx 回归及既有五项 FIFO LTP
- 内容：FIFO、character、block、socket 的 lower namespace entry 直接以对应 ext4 特殊类型创建；
  character/block 的 Linux kernel 32-bit device payload 经 VFS/namei 专用 create 接口传入 lower inode，
  由 stat 原样回报，并由 statx 拆分为 12-bit major/20-bit minor。VFS mode/owner commit 只修改权限与
  所有者，不承担修复 inode type；运行态 FIFO buffer 仍由 `open_named_fifo` 管理。
- 后续影响：特殊 inode 类型必须在 lower create 时正确，不能依赖内存中的 requested type。新增后端若
  无真实 character/block 表示必须返回 `EOPNOTSUPP`，不得回退到 regular placeholder。该决策不承诺
  字符/块设备驱动语义，也不扩展 Linux kernel 32-bit device encoding 的范围。

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

## 保留 copy_to/from_user 语义边界，以有界 I/O buffer pool 减少临时分配

- 状态：已采用，完整 BuildStorm 墙钟 A/B 待验证
- 适用范围：read/write/pread/pwrite、socket I/O、lazy/COW user pages、EFAULT/partial I/O
- 最后验证：2026-08-15
- 证据：`os/src/mm/{mod,io_buffer}.rs`、`os/src/syscall/fs.rs`、`os/src/perf.rs`；RV64
  `/tmp/respos-rv-io-buffer-{off,on}.log`，RV64/LA64 2 hart 专项
- 内容：用户复制 helper 继续逐页验证 VMA 权限、resolve lazy/COW、翻译 PTE 后复制；不能直接解引用
  user VA。文件/socket syscall 的 bounce buffer 是潜在额外复制，但当前窗口 copy 总计仅约 0.424 CPU 秒，
  不足以支持高风险接口重构。通用文件 I/O bounce buffer 现由 `KernelIoBuffer` 管理，
  每 hart 最多缓存一个 64 KiB `Vec`，取还不跨 fault/I/O/调度，miss 仍回退普通分配。
  这是减少 allocator 往返的通用资源管理，不是零拷贝，也不允许按进程/路径/测例分流。
- 后续影响：若以后优化，先设计可复用的 prepared/pinned user-page span 和 scatter/gather FileOp/Socket
  接口，明确 fault-before-side-effect、共享 file offset、short I/O、并发 munmap/COW 和锁顺序；必须有
  专项 ABI/竞态测试后才能替换 bounce buffer。调整 pool 大小或默认开关时必须同时报告
  hit/miss/grow/acquire ticks 和完整 workload 墙钟，不得只用局部命中率宣称 BuildStorm 加速。

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

## ext4 readdir 优先使用已验证的目录项类型，只为 UNKNOWN 重走 child path

- 状态：已采用，完整 BuildStorm final 待验证
- 适用范围：ext4 `getdents64`、Cargo 大目录遍历、lwext4 dirent ABI
- 最后验证：2026-08-14
- 证据：`os/src/fs/ext4/inode.rs`、`os/src/perf.rs`、
  `user/src/bin/fs_namespace_probe.rs`；RV64 同工作量目录遍历 A/B、RV64/LA64 120 秒窗口和无 feature
  专项门禁
- 内容：lwext4 在读取目录项时已经按 superblock `FILETYPE` feature 解析 inode type。已知的
  regular/directory/symlink 等类型直接转换为 VFS `InodeType`，不再对每项调用 pathname-based
  `ext4_mode_get`；只有 `EXT4_DE_UNKNOWN` 才保留原 child path 回退。`.`/`..` 不引入额外 alias
  解析，统一 ext4 锁、iterator 和 offset 语义均不改变。
- 后续影响：不得假设所有 ext4 镜像都有 FILETYPE，也不能把 UNKNOWN 强行猜成 regular；新增类型或
  修改 dirent FFI ABI 时必须保留回退并用真实 `getdents64` 验证 `DT_DIR/DT_REG/DT_LNK`。目录项类型是
  on-disk dirent 自带信息，不等同于批准目录内容缓存；后者仍需 generation、rename/unlink 和并发
  iterator 的独立一致性设计。

## lwext4 元数据 block cache 使用 4096 个 filesystem blocks

- 状态：已采用；8192 容量候选已否决
- 适用范围：lwext4 路径遍历、inode/extent 元数据、kernel heap 固定预算、BuildStorm Cargo 树
- 最后验证：2026-08-15
- 证据：`vendor/lwext4_rust/c/lwext4/CMakeLists.txt`、`os/src/perf.rs`；RV64 8 核 16/1024/4096 项
  固定 180 秒 A/B 和无 feature 文件/内存门禁；LA 8 GiB/12 hart 4096/8192 固定 120 秒 A/B
- 内容：将 `CONFIG_BLOCK_DEV_CACHE_SIZE` 从 16 增至 4096。4 KiB 文件系统下对应约 16 MiB，保存
  lwext4 元数据；普通文件 bulk data 继续走 direct multi-block 路径和内核 PageCache，不用该 cache
  再复制一份。选择 4096 是因为同窗口实际 file-data fill 比 1024 项多约 28%，而不是只按最低块读取量
  选择容量。8192 在相同工作阶段只把块读请求/字节减少约 0.28%/0.05%，PageCache fill 变化约
  +0.02%，未达到候选门槛，因而恢复 4096。
- 后续影响：该容量从 256 MiB kernel heap 中常驻占用约 16 MiB；完整 BuildStorm 必须监控 heap peak。
  不得把它误当作可无限扩大的通用文件缓存。若以后为元数据建立独立缓存或复用打开 inode handle，应
  重新测容量曲线；没有新的 eviction/hit-rate 证据时不再直接扩大到 8192。

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

## syscall restart 先窄化覆盖 `wait4`，由实际 handler 的 SA_RESTART 决定

- 状态：已采用
- 适用范围：`wait4/waitpid`、signal delivery、sigreturn；RV64/LA64
- 最后验证：2026-08-13
- 证据：双架构 `task_a_wait4_probe` 与 LTP `confstr01` 聚焦回归
- 内容：restart 决策不能只在 syscall 返回 `EINTR` 时完成，因为此时尚未确定实际送达哪个 signal。
  trap 层只记录 `wait4` 的原始 arg0，signal 层取出 signal 并读取 action 后，才在 `SA_RESTART` 分支
  改写要保存的 PC/arg0。这样 handler 正常执行，sigreturn 后重做 syscall；无标志分支保持 `EINTR`。
- 后续影响：当前不引入对所有 syscall 通用的内部 `ERESTART*`。扩大覆盖时按 Linux restart class
  逐个加入，并补无标志/有标志、side effect 与双架构 trap ABI 门禁。

2026-08-15 增量：trap 层改为显式 restart-class 查询，在既有 `wait4` 外加入已由 Linux/RV64/LA64
三方 probe 覆盖的
`read/write/readv/writev/accept/accept4/sendto/recvfrom/sendmsg/recvmsg/sendmmsg/recvmmsg`；普通 handler、
`SA_RESTART` 和默认忽略三条路径均通过，向量 I/O、`recvmsg(MSG_WAITALL)` 与 mmsg 同时验证已有进展
优先返回字节数/message count。随后分类器升级为参数感知：null-timeout 的
`FUTEX_WAIT/FUTEX_WAIT_BITSET` 已由三态 Linux/RV64/LA64 probe 纳入；futex 的非空 timeout 仍明确
排除。无 `SO_SNDTIMEO` 的 connect 随后以 AF_UNIX 满 accept queue 三态 probe 纳入；分类器从 fd
读取 Socket timeout 状态，并将该规则推广到整组 socket/read/write 表项：接收方向检查
`SO_RCVTIMEO`，发送方向检查 `SO_SNDTIMEO`。accept/recvfrom/sendto/connect 的 timeout +
`SA_RESTART` Linux/RV64/LA64 对照均保持 `EINTR`；分类在 syscall 入场前快照，与本次操作的 deadline
选择时点一致。其余 timeout 类必须先闭合 partial side effect、
完成竞态与剩余时间证据再加入。

2026-08-15 增量：`recvmmsg` 非空 timeout 经 Linux oracle 证实属于 restart class，且 timeout 在零进展
`EINTR` 时不写回；因此 restart 重新使用原值。该 timeout 只在成功 message 后检查/写回，超期本身不
唤醒无数据 receive。RespOS 明确保留这一 Linux 历史 ABI，并实现 `MSG_WAITFORONE` 首条后 nonblock，
不把 recvmmsg 强行统一成 poll/sleep deadline 模型。

2026-08-15 增量：`nanosleep/ppoll/pselect6/epoll_pwait` 经 Linux/RV64/LA64 对照确认，即使实际 handler
带 `SA_RESTART` 也保持 `EINTR`，默认忽略 signal 才继续等到 timeout；这些调用继续排除在 restart 表外。
relative `nanosleep` 由内核返回 remaining time，不通过重新执行原始 timeout 模拟。signal enqueue 的
`interrupted` 发布改为写后重新验证 pending/interruptible 条件，因为该字段是可撤销 wake hint，不能在
consumer 已消费 signal 后遗留并打断下一 syscall。

2026-08-15 增量：relative/absolute `clock_nanosleep` 也纳入同一非重启门禁；relative signal interruption
写 remainder，`TIMER_ABSTIME` 不写 remainder。stop/job-control 路径采用“先发布 `Stopped`、再通知
parent、最后只 handoff”的提交顺序；handoff 不重写状态，以保留父进程并发 SIGCONT 的 `Ready` 提交。

## 保留 100 Hz 调度 tick，以一次性 deadline 缩短精确 timeout

- 状态：已采用
- 适用范围：RV64/LA64 timer、nanosleep、poll/pselect/epoll、futex
- 最后验证：2026-08-13
- 证据：双架构 8-case LTP 单核专项、`nanosleep01,futex_wait05` 2-hart 专项和 20 轮 clock probe
- 内容：不通过全局提高 `TICKS_PER_SEC` 修复亚 10 ms timeout。保留既有 100 Hz 周期负载，只让有
  精确 waiter 的最早 deadline 缩短 timer-service hart compare；原子最小值是可重建提示，waiter 注册表
  才是权威状态。QEMU compare 提前 800 us 后仅在 trap 内等待到软件 deadline，不允许提前唤醒。
- 后续影响：该选择避免所有 workload 无条件承担高频 tick，但增加了一次性 timer/IPI 协议。修改服务
  hart、idle 或 timer scan 时必须保留单点编程、注册后发布、扫描后重建和无早醒四项不变量；实机需
  独立校准或移除 QEMU 提前量。

## LA64 secondary 调度释放以 boot timer-service 首次编程为提交点

- 状态：已采用
- 适用范围：LA64 SMP cold boot、全局 timer-service、用户 timeout
- 最后验证：2026-08-15
- 证据：raw-counter 诊断、QEMU 10.0.2 LoongArch constant timer 实现，以及 release 初赛 snapshot
  1/2/12 hart `socket_timeout_probe`；详见 [current-status.md](./current-status.md) 顶部专项。
- 内容：secondary 启用本地 IPI 并发布 online bit 后，等待 boot hart 发布 `BOOT_RELEASED`。boot hart
  仅在 bounded hart discovery、timer interrupt enable 和首个 compare 编程都完成后发布 release。
- 后续影响：online publication 与 scheduler admission 是两个独立阶段。后续修改 boot/timer 顺序必须
  保证用户任务不能早于全局 timeout 服务进入运行态；当前证据否定 per-hart `rdtime.d` 偏移假设，
  不引入时钟归一化、任务 affinity 或放宽 timeout 容差。

## ext4 多字段扩展时间戳在一次 inode transaction 中提交

- 状态：已采用，当前工作树待提交
- 适用范围：ext4 atime/mtime/ctime 更新、负秒/2038 后 epoch/纳秒、BuildStorm 高频文件写回、lwext4 vendor API
- 最后验证：2026-08-15
- 证据：`vendor/lwext4_rust/c/lwext4/{include/ext4.h,src/ext4.c}`、
  `os/src/fs/ext4/inode.rs`；RV64 8 GiB/8 核固定 120 秒 A/B、双架构即时与跨重启 probe
- 内容：一次 VFS `set_times` 需要更新多个字段时，不再为每个字段各做一次 pathname walk 和 inode
  transaction。vendor API 接收字段 mask 和完整 sec/nsec，在同一个 inode ref 上写 classic seconds 与
  `*_extra` 后一次提交；所有字段先编码验证，再修改 inode，避免组合 setattr 的部分提交。Rust 层负责
  raw inode signed-low/epoch 解码、打开后 unlink 和缓存时间语义。read/readdir 的自动 atime
  使用独立入口，只更新 atime；显式 utimens 及 mtime 修改仍更新 ctime。
- 后续影响：不得通过延迟或丢弃可见时间戳来复制本轮收益。自动 atime 不得更新 ctime，否则 relatime
  会在 `atime <= ctime` 上自我触发；显式时间修改又必须更新 ctime。继续优化前需验证跨 reopen、stat
  和同步边界；所有 lwext4 调用仍复用唯一 `EXT4_OP_LOCK`。范围处理遵循 Linux
  `timestamp_truncate()`：截断而非 `ERANGE`，端点 nsec 归零；不得恢复成过滤 Option 后静默丢弃某个
  requested timestamp。

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

## AF_UNIX 地址以 raw bytes 在 connect 提交点快照

- 状态：已采用
- 适用范围：pathname/abstract bind/listen/connect/accept、`getsockname/getpeername`
- 最后验证：2026-08-14
- 证据：`os/src/{net/socket.rs,syscall/net.rs}`、`scripts/getpeername_probe_linux.c`、
  `user/src/bin/getpeername_probe.rs`；双架构 2 hart 专项与 musl/glibc 地址 LTP 簇
- 内容：bound/peer address 由各 endpoint 持有 `Option<Vec<u8>>`；前导 NUL 的 abstract 名称是任意字节
  namespace，不转为 UTF-8。connect 成功提交时同时建立 client peer、待 accept endpoint 的 local/peer
  快照；accept 只转移 endpoint。地址查询读取快照，不从可被关闭/复用的 listener registry 反查。
- 后续影响：连接失败必须回滚尚未发布的 peer 快照；dup/fork 继续共享 endpoint 状态。pathname 的
  终止 NUL 只在 ABI writer 生成，不进入 registry identity；abstract 则不得追加终止 NUL。

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

## LoongArch RAM 发现与 direct map 保持架构内聚

- 状态：已采用，4/12 GiB 已验证，36 GiB 平台回归待验证
- 适用范围：LA QEMU virt 启动、kernel direct map、frame allocator
- 最后验证：2026-08-12
- 内容：LA 从 QEMU fw_cfg 获取实际 RAM，失败时保留兼容上限；high RAM 使用 PMD 2 MiB huge
  leaf。公共 MM 只消费 `physical_memory_end()`，不感知 fw_cfg、LA RAM 空洞或 huge PTE 编码。
- 理由：比赛 LA 内存为 36 GiB，固定 12 GiB 会浪费资源；而用 4 KiB 页覆盖 36 GiB 会产生约
  九百万次映射和大量页表页。将发现和编码收敛到 LA arch 层可避免改变 RV64 已验证的 FDT/Sv39
  路径。
- 后续影响：任何 LA RAM 布局变化要先验证 fw_cfg 合约和 PMD 对齐；不得为共享优化改动 RV64
  页表实现而不做独立 A/B 与运行门禁。

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
- 最后验证：2026-08-13
- 证据：`os/src/arch/rv64/trap/mod.rs`；2 核 `ACTIVE_ITIMER_TASKS` 中断重入 GDB 栈
  `/tmp/respos-smp2-dynamic-bt.txt`；2/4/8 核各三轮退出压力
- 内容：普通 kernel-mode timer trap 只确认并重编程 tick，不调用会获取 task/signal/timer
  锁的 `check_all_task_timers()`。高层 timer work 当前由 user-mode timer trap 和 boot hart 的无 current
  idle context 串行服务。对于不会返回用户态、也不会让 boot hart 进入 idle 的 kernel blocking retry，
  允许在明确不持有 FileOp/socket/task/signal/timer 锁的安全点消费同一延迟工作；该入口只在
  timer-service hart 生效，并按 monotonic millisecond 限频。当前调用者是 inet poll fallback 与
  TCP/UDP blocking retry。
- 后续影响：不以“把某一把锁换成 NoIrq”逐个遮掩高层中断重入；需更强及时性时改为
  中断只发布 pending，再在明确安全点消费。任何 kernel 内部 yield 循环都需审计 timer progress。

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

## `CLONE_VM` 决定 vfork 的共享 MM，exec 通过 per-task handle 脱离

- 状态：已采用
- 适用范围：`clone(CLONE_VM|CLONE_VFORK)`、libc `vfork`/`posix_spawn`、exec 与共享 MM 回收
- 最后验证：2026-08-14
- 证据：`os/src/task/task.rs::{clone_,install_exec_image}`；RV64/LA64 双 libc `clone05`、额外
  `vfork01/vfork02` 与 RV64 final CAgent/minibuild
- 决策：只要 flags 含 `CLONE_VM`，child 就与 parent 共享旧 `MemorySet`，`CLONE_VFORK` 不构成例外。
  每个 task 持有自己的可替换 handle；成功 exec 给调用者安装新 handle，parent 保留旧 MM。child
  exec/exit 仍通过独立一次性 vfork completion 唤醒已预先登记 blocked 的 parent。
- 原因：复制 MM 虽能避免早期实现的 exec 覆盖，却违反 child exec/exit 前的共享写可见性；LTP
  `clone05` 会稳定观察到该差异。可替换 handle 已把“共享旧映像”和“exec 私有新映像”分开，无需
  继续保留过期规避。
- 后续影响：MM 回收必须检测线程组外的共享 owner；不得在 child exec 前原地覆盖共享 `MemorySet`，
  也不得把 parent wakeup 与共享可见性混成单一测试结论。non-leader exec 重构必须保持这一 handle
  所有权边界。

## LA64 SysV `SHMLBA` 跟随当前 Linux 的 4 KiB page size

- 状态：已采用
- 适用范围：LA64 `shmat(SHM_RND)` 与不同 libc header 版本
- 最后验证：2026-08-14
- 证据：`os/src/syscall/ipc.rs::sys_shmat()`；Linux `d23b77953f5a`、glibc `cae3c9e3a117`；
  RV64/LA64 双 libc `shmat01,shmdt02`
- 决策：RespOS 的 LA64 `SHMLBA` 保持 `PAGE_SIZE=4096`。不为镜像中 glibc 2.38 编译期的旧
  64 KiB 常量修改全局 rounding，也不按调用二进制或地址形态分流。
- 原因：当前 Linux 已从 64 KiB 改为 page size，当前 glibc 也恢复 generic 定义；同一 syscall 没有
  libc header 版本信息，无法同时满足旧 glibc 与 musl 的冲突期望。内核特判只会制造不可维护的
  非 Linux ABI。
- 后续影响：该 LTP 单项通过需更新 glibc runtime/test image。SysV segment 跨 attach 的共享
  frame/futex identity 与 `IPC_RMID` 基本生命周期已由独立 probe 闭合；并发发布/回收仍需独立验证，
  不能由 rounding 决策代替。

## SysV SHM futex key 使用共享 frame 身份，attach id 只管理 detach

- 状态：已采用
- 适用范围：同一 SysV segment 的重复/跨进程 `shmat`、shared futex、`shmdt`
- 最后验证：2026-08-14
- 证据：`os/src/mm/memory_set.rs::shared_futex_key()`；Linux oracle、RV64/LA64 2-hart
  `sysv_shm_futex_probe`；双架构 musl/glibc `futex_wait01,futex_wake03`
- 决策：SysV SHM 页上的 shared futex 以 resident shared frame 的 PPN 作为 owner，并保留页内 offset；
  每次 `shmat` 唯一的 attach id 仅用于标识一次映射并让 `shmdt` 拆除其全部 VMA 分片。
- 原因：同一 segment 的各 attach 共享同一个 `Arc<FrameTracker>`，但映射实例 id 按设计不同。用后者
  作为同步 identity 会让不同虚拟地址错误地进入不同 futex 队列；frame 身份直接表达当前实际共享的
  backing page，且无需新增全局 segment-to-futex 索引。
- 后续影响：VMA 切分/合并不得丢失 detach 所需 attach id，futex 路径也不得退回映射实例 identity。
  `IPC_RMID` 的显式/exit/exec/fork-inherited 生命周期已独立验证；并发 attach/detach 与 frame 复用压力
  仍须单独覆盖。

## exec/exit 在旧 MM 不可达后显式提交 SysV SHM detach

- 状态：已采用
- 适用范围：SysV SHM、成功 exec、group exit、显式 `shmdt`、fork/`CLONE_VM`
- 最后验证：2026-08-14
- 证据：`os/src/task/task.rs::{install_exec_image,exit_process_group}`、
  `os/src/syscall/ipc.rs::release_shm_attachments()`；Linux/RV64/LA64 2-hart lifecycle probe
- 决策：task 在成功安装新 MM 或完成旧 MM recycle 后，把旧地址空间的 attach id 显式提交给 SysV
  table；显式 `shmdt` 使用同一提交点。table 只有在 live MM 不再持有该 id 时才移除 owner，并仅在
  `IPC_RMID` segment 的全局 attachment 为零时释放 frames。
- 原因：MM 只拥有映射，`SHM_TABLE` 仍独立持有 segment frames；只删除 PTE 或等待 Rust `Drop` 会留下
  可重新 attach 的旧 shmid。反过来，无条件按 task exit 删除会破坏 fork/CLONE_VM peer 仍可访问的
  attachment，因此提交必须发生在 MM 状态改变之后并以 live owner 复核。
- 后续影响：失败的 exec 不提交 detach；共享旧 MM 的 vfork child exec 不能回收 parent mapping。若后续
  引入 per-MM nattch 计数或并发 attach reservation，必须保持“VMA 发布/撤销先于最终 table 回收”的
  提交顺序。

## SysV `shm_nattch` 按唯一 MM 中的 attach identity 计数

- 状态：已采用
- 适用范围：`IPC_STAT/SHM_STAT`、重复 `shmat`、pthread/`CLONE_VM`、fork
- 最后验证：2026-08-14
- 证据：`os/src/syscall/ipc.rs::shm_attach_count()`、`TaskControlBlock::memory_set_arc()`；
  Linux/RV64/LA64 2-hart `sysv_shm_nattch_probe` 与双架构 `shmctl03,07,08`
- 决策：先以 `Arc<MemorySet>` identity 去重 live task，再累计该 MM 中指向 segment frames 的唯一
  attach id。同一 MM 的两次 `shmat` 计 2，共享 MM 的额外线程不重复计数；fork 的独立 MM 复制每个
  inherited attachment，因此两次 attach 在 parent+child 中计 4。
- 原因：`shm_nattch` 描述地址空间 attachment，不描述调度实体数量。按 TCB 扫描会让 pthread 的创建/
  退出凭空改变 metadata，并可能延迟 `IPC_RMID` segment 的最后回收。
- 后续影响：task snapshot 与 MM handle 必须成对读取，不能先取 identity 再通过可能已 exec 的 task
  handle 读取另一个 MM。未来若改为显式 per-MM refcount，仍须保持重复 attach 与 fork 复制的上述计数。

## SysV `shmat` 在 table 中预留后才跨越 MM 提交窗口

- 状态：已采用
- 适用范围：`shmat`、最后 `shmdt`/隐式 detach、`IPC_RMID`、非空 `shmaddr`
- 最后验证：2026-08-14；release、4 GiB/2 hart
- 证据：`os/src/syscall/ipc.rs::sys_shmat()`；Linux/RV64/LA64
  `sysv_shm_attach_race_probe`
- 决策：在 `SHM_TABLE` 下确认 segment 可 attach 后立即增加 `pending_attaches`，再释放 table lock 安装
  VMA；完成时无论成功或失败都在 table 下撤销预留并统一执行延迟删除。segment 只有在 reservation 与
  已安装 attachment 同时归零时可释放。非空 `shmaddr` 且无 `SHM_REMAP` 时采用精确不可覆盖映射，
  冲突按 `shmat` ABI 返回 `EINVAL`，不允许退化为通用 mmap hint。
- 原因：仅提前发布 attach owner 不能被按 MM 扫描的 `shm_nattch` 看见，最后 detach 可在 VMA commit
  前删除 segment，制造“`shmat` 成功但 shmid 已失效”的孤儿映射。持有 table lock 跨越 MM 操作会扩大
  锁序和 fault 临界区；轻量 reservation 提供同一线性化保证，并允许失败路径回滚。
- 后续影响：所有早退必须撤销 reservation 与 owner；回收判断不得只看 `shm_nattch`。扩展
  `SHM_REMAP` 并发、批量 attach 或资源配额时必须保持同一 prepare/commit/rollback 协议。

## LoongArch shootdown 使用每目标 hart 的 generation 槽

- 状态：已采用
- 适用范围：LA SMP、共享 MemorySet、PTE 修改与 frame 回收
- 最后验证：2026-08-13
- 证据：`os/src/arch/loongarch64/smp.rs`、`os/src/mm/memory_set.rs`；LA 12-hart 双
  `smp_shared_mm_probe`、Phase3、2400 短进程 ASID 复用与 30 秒 BuildStorm `perf_counters` 窗口
- 内容：IOCSR IPI vector 1 表示“检查本 hart 的 shootdown 槽”。请求者按 hart id 顺序独占每个
  target slot，发布 generation 和经过校验的 `all`/`address-space`/`range`、ASID、页对齐区间后发送
  IPI 并同步等待 ack；目标在确认前读取同一描述。当前 all/address-space/单页 range/多页 range
  分别执行 op=0/op=4/op=4/op=4。启用 ASID 后，每个 MemorySet 以独立
  residency mask 记录自上次同步失效后可能缓存其 TLB 的 hart；普通 PTE 更新向 residency
  shootdown，完成后收缩为 active mask。只用 active mask 不安全，因为 inactive hart 仍可保留
  同 ASID 的旧项。
- 后续影响：旧数据 frame 批次必须保持在所属 `PageTable`/`MemorySet`，不得恢复为混合多个
  ASID 的全局队列。释放前必须对该地址空间的 residency 完成同步 shootdown；只用 active mask
  仍不安全。LA 关中断内核的锁等待必须服务 pending IPI，handler 不得拿普通锁。Global 映射或
  异步 shootdown 仍须单独验证。

## LoongArch 普通地址空间失效使用 INVTLB op=4

- 状态：已采用
- 适用范围：LA `MemorySet` PTE writer、本地失效、远端 shootdown handler
- 最后验证：2026-08-13
- 证据：`os/src/arch/loongarch64/{register/mod.rs,mod.rs,smp.rs}`、`os/src/mm/memory_set.rs`；
  12-hart 2400 exec rollover、双 shared-MM、Phase3 与 30 秒 BuildStorm `perf_counters` 窗口
- 内容：已校验的 address-space 请求以 ASID 作为 `invtlb op=4` 的 rj 操作数，本地 writer 使用同一
  封装。现有运行期 PTE 不设置 Global 位；root 激活仍使用 op=0，覆盖 boot root Global 映射的转换。
  ASID retired 批次复用发布 `all` 并保持 op=0；非法描述也回退到 op=0，所有 range 使用 op=4。
- 后续影响：不得把 ASID rollover、boot/final root 过渡改为 op=4。启用 Global kernel PTE 前必须
  同时验证成对 G 位与 kernel 映射更新协议。不得把大范围展开为无界的逐页 op=5 循环。

## LoongArch Global kernel mapping 的 default-off 阶段（历史决策）

- 状态：已由 2026-08-15 的包围 A/B 与默认启用决策取代
- 适用范围：LA kernel leaf、ASID op=4、TLB residency、2 MiB direct map
- 最后验证：2026-08-14
- 证据：4 KiB paired-leaf shared-MM/Phase3 专项；12 GiB/12 hart BuildStorm on--off--on 完整日志
  `/tmp/respos-la-global4k-{full-final,ab-off-full-nohostfwd,ab-on2-full-nohostfwd}.log`
- 内容：4 KiB 实验只对启动时已有且偶奇页均有效的 kernel leaf 成对设置 G，高端 2 MiB 与运行期
  kernel stack 保持非 Global；它通过正确性门禁，但完整 axbuild 为
  `1560.36/1410.58/1634.28s`，两次 on 均未胜过 off。短窗口方向相反且 host backing-file cache/负载
  未被完全隔离，当时不足以满足稳定 `>=5%` 收益门槛。更稳定宿主的一对完整 off/on 为
  `1530.37/1418.31s`（约 `7.32%`），但两轮 swap-in/out 约为 `64/217 MiB` 与 `25/0.5 MiB`，且未跑
  R3/off，不能做完全因果归因，因此当时 feature 保持默认关闭；该结论已被 2026-08-15 的
  off -> on -> off2 包围 A/B 取代。op=4/shootdown/frame completion 始终未改变。
- 后续影响：历史反例仍要求后续平台结果同时记录宿主负载和 cache 顺序；4 KiB pair、动态 kernel
  mapping、跨 ASID/跨 hart 继续作为门禁。huge leaf 在编码被架构证据和真实
  高端 RAM 访问独立证明前，仍由 `map_huge_2m()` 显式拒绝 Global。

## LoongArch 叶 PTE 修改由 PageTable 累积失效范围

- 状态：已采用范围传播，op=5 已否决
- 适用范围：LA 页错误、COW、mmap/munmap/mprotect、fork 与远端 shootdown 请求
- 最后验证：2026-08-13
- 证据：`os/src/arch/loongarch64/mm/page_table.rs`、`os/src/mm/memory_set.rs`；12-hart 2400 exec、
  双 shared-MM、Phase3 和 30 秒 BuildStorm range 分布
- 内容：所有成功的 LA 叶 PTE map/unmap/replace/permission/COW 入口在所属 `PageTable` 累积半开
  VPN 包络。flush 同时冻结范围和 retired frame 批次；有范围时发布 range 请求，无范围时退回
  address-space。完整 root activate 清除已由 op=0 覆盖的构建期包络。稀疏修改允许扩大包络，
  但不得缩小到遗漏任一修改页。
- 后续影响：当前所有 range 统一执行一次 op=4。30 秒窗口最大包络 10938 页；重新启用 op=5
  必须先解释完整 BuildStorm 内存破坏并通过相同 final 门禁。

## LoongArch 当前拒绝单页 INVTLB op=5，range 统一使用 op=4

- 状态：op=5 已否决，op=4 回退已采用
- 适用范围：LA `MemorySet` 本地 PTE flush 与远端 range shootdown handler
- 最后验证：2026-08-13
- 证据：LoongArch ISA Volume 1 `INVTLB` 定义；`os/src/arch/loongarch64/{register/mod.rs,mod.rs,smp.rs}`、
  `os/src/mm/memory_set.rs`；12-hart 2400 exec rollover、双 shared-MM、Phase3 与 30 秒 BuildStorm
- 决策：所有 range 执行一次 op=4。发送端范围校验、同步 ack 和 retired-frame 生命周期不变；
  op=5 封装与执行计数从当前实现删除。
- 原因：单页 op=5 通过 2400 exec/shared-MM/Phase3 和 30 秒窗口，但完整 final 在 minibuild 与 std
  构建中出现 stack smashing/smallbin corruption。仅回退 op=4 的相同配置 A/B 恢复 minibuild 并
  越过原崩溃点。ISA/QEMU 操作数审计尚未给出足以证明安全的解释，故选择保守且有界的 op=4。
- 后续影响：root 激活、ASID 批量回收 all 和非法请求不得降级为 op=4/op=5。若引入 Global PTE、
  非 4 KiB 用户叶或重新启用 op=5，必须重新审计匹配语义、页大小与同步回收门禁，并跑完整 final。

## 页表页在最后 active hart 切离 root 后释放

- 状态：已采用
- 适用范围：RV64/LA64 进程退出、`MemorySet` 回收、页表页 frame 生命周期
- 最后验证：2026-08-13
- 证据：`os/src/{mm/memory_set.rs,task/processor.rs,arch/*/mm/page_table.rs}`；LA 12-hart 2400 次
  exec、ASID 多轮复用后双 shared-MM 与 Phase3
- 内容：`recycle_data_pages()` 可在退出 task 仍运行于其用户 root 时发生，因此不能当场
  释放根/中间页表页。当前将页表页移入所属 `PageTable` 的退役槽；调度路径已切到
  per-CPU idle/kernel root 后才清除该 hart active bit，最后一个 bit 的清除者释放退役页表页。
  旧的全局 128 页 quarantine 依赖容量/时间推测安全期，已删除。
- 后续影响：不得在切换 root 之前清 active bit，也不得跳过 `clear_current_hart_active()`。
  页表页的 completion 是 root-switch/active ownership，数据页的 completion 是 PTE shootdown/residency，
  两者不能混为一个“等待若干次分配”的通用 quarantine。

## LoongArch 用户 MemorySet 持有可延迟复用的 10-bit ASID

- 状态：已采用
- 适用范围：LA task switch、MemorySet 生命周期、短进程/exec、TLB shootdown
- 最后验证：2026-08-12
- 证据：`os/src/{arch/loongarch64,mm/memory_set.rs,task/task.rs}`；12-hart 1200 短进程 rollover、
  rollover 后双 shared-MM 与 Phase3、正式 final 短回归
- 内容：ASID 0 保留给 kernel/idle，用户空间分配 1--1023；root 与 ASID 组合为软件 token，普通
  `__switch` 只恢复 PGDL/PGDH/ASID，不完整失效 TLB。退出路径只有在确认无外部 CLONE_VM owner、
  数据页完成全在线失效后才及时退役 ASID；最终 Drop 幂等补漏。编号耗尽时先冻结 retired 批次，
  完成本地和全在线失效后才清除 used 位，禁止未经屏障立即复用。
- 后续影响：CSR.ASID 的 ASIDBITS 高位不得混入 token。现有 ASID 不自动证明 Global PTE、按 ASID/VA
  精确失效或缩小 frame-retirement target 正确；这些优化必须分别通过 shared-MM 与复用压力门禁。
  当前最多支持 1023 个同时存活的独立用户 MemorySet；若扩展上限，必须实现 active-ASID 保留的
  generation rollover，不能复用尚未退役的编号。

## LoongArch 扩展状态采用 per-task first-use gating

- 状态：已采用
- 适用范围：LA FP/LSX trap、task switch、fork/exec、SMP 迁移
- 最后验证：2026-08-12
- 证据：`os/src/arch/loongarch64/trap/{context.rs,trap.S,mod.rs}`；12-hart BusyBox、CAgent、
  shared-MM、Phase3 与 `perf_counters` 窗口
- 内容：exec 后先关闭用户 EUEN.FPE/SXE，首次 FPD/SXD 只标记 trap frame 并重试原指令；未激活任务
  跳过 FP/LSX/FCSR/FCC 保存恢复，激活任务继续使用既有 eager 隔离。内核执行 Rust 前始终重新启用
  扩展。本阶段不引入 per-CPU owner，不允许在未证明跨 hart owner 迁移和内核扩展使用安全前进一步
  改为 fully lazy save/restore。
- 后续影响：fork 复制激活标记和扩展状态，exec 清零；signal mcontext 仍需独立补齐。测得 535 次
  user trap 中扩展 eager save 为 62 次；该数值证明门控生效，不是正式 BuildStorm 加速比。

## CPU clock 采用 scheduler occupancy，并让 POSIX timer 只持有 detached clock

- 状态：已采用
- 适用范围：RV64/LA64 `CLOCK_PROCESS_CPUTIME_ID`、`CLOCK_THREAD_CPUTIME_ID`、POSIX timer
- 最后验证：2026-08-14
- 证据：`os/src/task/{processor,task}.rs`、`os/src/syscall/time.rs`；双架构五目标 LTP 和 2-hart
  `task_a_clock_probe` 20 轮
- 决策：CPU time 只累计 task 实际占用 CPU 的 `__switch` 区间，不使用进程存活 wall time。thread
  clock 每 TCB 独立；process clock 由 `CLONE_THREAD` 组共享，并允许不同 hart 的 live interval 同时
  贡献。CPU timer 捕获仅含 clock state 的 handle，signal owner 仍按 tgid 弱引用进程 leader。
- 原因：wall time 会把 sleep/block 计入 CPU time，无法驱动只应在执行期间到期的 CPU timer；单一
  process running flag 又会漏记 SMP 并行线程。让 timer 强持有 TCB 会连带延长地址空间和资源生命周期，
  detached state 可在退出后冻结 thread clock，同时保持 timer 查询安全。
- 后续影响：fork/new process 必须清零 clock，exec 保留 clock，线程 clone 仅共享 process clock。
  user/system 拆分、跨进程 encoded CPU clock id 与 CPU-time nanosleep 尚未包含在本决策中；实现前需
  单独固定 ABI 与生命周期契约。

## 进程身份采用独立 ProcessState，TaskManager 只索引线程

- 状态：已采用，M2.1 核心路径已通过双架构专项
- 适用范围：PID/TID、thread group、parent/children、wait/zombie、session/pgrp、process signal、exec/exit
- 最后验证：2026-08-15
- 证据：`os/src/task/{process,task}.rs`、`user/src/bin/task_phase5_probe.rs`；
  `/tmp/respos-{rv,la}-process-identity-{race,smp8}.log`
- 决策：每个进程由独立 `Arc<ProcessState>` 表示；live TCB 和 parent children 表负责强持有，
  `ProcessTable[tgid]` 使用 Weak 索引，`TaskManager[tid]` 只索引线程。leader TCB 不是进程生命周期
  owner，也不保留 exited tombstone。exec/group-exit 由 process lifecycle CAS 选出唯一提交者。
- 原因：leader 可以早于 worker 原始退出，non-leader exec 还必须让调用线程接管 TGID。把 PID lookup、
  wait 身份和共享资源继续绑在 `tid == tgid` 的 TCB 上，会导致父进程过早 wait、process-directed signal
  失去目标或不得不永久保留伪 leader。
- 后续影响：按 PID/TGID 的接口必须先查 `ProcessTable`，按 TID 的接口才查 `TaskManager`；最后 member
  才能发布 Zombie，wait copyout 后才 Reaped。共享 handler/resource owner 与旧身份
  双写还要继续迁移，删除兼容字段前必须清点所有读取点并保持双架构专项。

## controlling tty、termios 和输入 line discipline 由 terminal 对象共享持有

- 状态：已采用，M2.2 terminal/line discipline 与孤儿组转换通过双架构专项和全屏 Vim
- 适用范围：console stdio、`/dev/tty`、session/pgrp、tty ioctl、job-control signal
- 最后验证：2026-08-16
- 证据：`os/src/fs/tty.rs`、`user/src/bin/job_control_phase5_probe.rs`、
  `scripts/job_control_phase5_probe_linux.c`
- 决策：terminal 持有 controlling SID、foreground PGID 和 termios；`ProcessState` 只记录关联标记。
  所有 console fd 通过同一 terminal 状态执行 `TIOC*`、`TC*` 及后台读写检查。孤儿组形成属于稳定
  ProcessState 关系变化：`setpgid/setsid/exit+reparent` 提交前后比较 orphan 状态，含 stopped member 的
  新孤儿组统一收到 `SIGHUP` 后 `SIGCONT`。stdio stdin 与 `/dev/tty` 不得各自读取固件串口，必须共享
  canonical/raw 队列；控制字符抽取还必须存在于无 tty reader 的 timer safe point。
- 原因：controlling terminal 和前台组是 session/terminal 关系，不是某个 fd 或某个 leader TCB 的属性；
  按 fd/syscall 分散保存会令 dup/open、leader exit 和 non-leader exec 观察到互相矛盾的前台状态。
- 后续影响：PTY 应实例化同一 terminal/line-discipline 状态机；完整 hangup、flow control、forced steal 与并发
  setpgid/exit 线性化未闭合前，不将本决策解释为完整 POSIX tty 支持。

## 默认忽略与显式 SIG_IGN 不得共用同一 signal action

- 状态：已采用，SIGCHLD 自动回收首轮通过三方专项
- 适用范围：signal action 初始化/exec reset、SIGCHLD、child Zombie/Reaped、wait
- 最后验证：2026-08-15
- 证据：`os/src/signal/sig_handler.rs`、`os/src/task/task.rs`、`scripts/signal_phase5_probe_linux.c`
- 决策：初始 action 始终保存 `SIG_DFL`，默认 action table 只决定递送行为；只有用户明确设置
  `SIG_IGN` 才保存 ignore。SIGCHLD 的显式 ignore 与 `SA_NOCLDWAIT` 在 child exit 发布点自动回收，
  前者不发 signal，后者对已安装 handler 仍发 signal。
- 原因：默认 SIGCHLD 虽然即时行为是忽略，但必须留下 Zombie；将它编码成 `SIG_IGN` 会错误自动回收，
  也无法在 exec reset 和 wait 生命周期中恢复区别。
- 后续影响：其他默认 ignored signal 的 interrupt 判断必须同时检查 `SIG_DFL` 的默认 action；不能仅以
  handler 数值非 `SIG_IGN` 推断它会中断阻塞 syscall。

## 实时 pending 配额先由稳定 ProcessState 统一 thread/process 两级队列

- 状态：已采用；进程内范围通过双架构 probe 与 LTP `tgkill02`
- 适用范围：实时 signal enqueue/consume、`RLIMIT_SIGPENDING`、thread exit
- 最后验证：2026-08-15
- 证据：`os/src/signal/sig_struct.rs`、`os/src/task/{process,task}.rs`、
  `/tmp/respos-{rv,la}-signal-rtqueue-{quota,tgkill02}.log`
- 决策：实时信号入 thread 或 process pending 前都在稳定 ProcessState 原子 reserve，消费或丢弃时
  release；syscall 发送路径使用目标进程 soft limit 并在耗尽时返回 `EAGAIN`。标准信号合并不占多个额度。
- 原因：额度若分别挂在 TCB/pending map 上，process-directed 与多线程队列会各自超限，leader exit 和
  non-leader exec 还会丢失计数 owner。ProcessState 能先闭合单进程内并发与生命周期。
- 后续影响：Linux 按 real UID 跨进程计数；RespOS 尚无对应全局 credential owner，当前决定不宣称该
  范围。以后上移 owner 时保留 reserve-before-enqueue 与 consume/teardown release 的提交顺序。

## 普通 file mmap 使用 live EOF，ELF PT_LOAD 使用 fixed prefix

- 状态：已采用，M3 核心七项与双架构 loader 回归通过
- 适用范围：mmap fault、ELF loader、PageCache、private COW、truncate
- 最后验证：2026-08-15
- 证据：`os/src/mm/memory_set.rs`；`mmap_phase5_probe`、双架构双 libc `mmap05`
- 决策：普通 mmap 每次 fault 依据当前文件长度，允许映射后的文件增长；ELF segment 只在声明的
  `p_filesz` prefix 取文件字节，其余 BSS 永远匿名清零。clean writable MAP_PRIVATE page 使用
  PageCache+只读 COW，只有 store 后才获得匿名 private provenance。
- 原因：冻结普通 mmap 长度会看不到 ftruncate 增长；把 live EOF 反向套到 ELF 则会用后续文件字节
  污染 BSS。没有 clean/COW provenance，又无法同时满足 truncate partial-tail 清零和已写 private bytes
  保留。
- 后续影响：fixed prefix 的 partial last page不能共享未裁剪 PageCache frame；truncate 全页失效必须
  覆盖 clean/shared/private-COW 三类 resident page，并在 File lock 外执行跨地址空间扫描。

## ext4 punch-hole 以物理 extent 和实际 `i_blocks` 为事实源

- 状态：已采用，Linux/RV64/LA64 punch 专项与相邻回归通过
- 适用范围：ext4 `FALLOC_FL_PUNCH_HOLE|FALLOC_FL_KEEP_SIZE`、PageCache、file mmap、stat
- 最后验证：2026-08-16
- 证据：`vendor/lwext4_rust/c/lwext4/src/{ext4,ext4_extent}.c`、`os/src/fs/{file,page_cache}.rs`、
  `mmap_phase5_probe`；`/tmp/respos-{rv,la}-mmap-punch-phase5-errors.log`
- 决策：完整块由 extent allocator 真正释放，非整块边界只对已有 backing 块清零；磁盘 ext4 的
  `st_blocks` 直接导出 inode `i_blocks`。lower、PageCache 与跨 MemorySet invalidation 是同一文件操作的
  三层提交，已 COW private 页按匿名 provenance 保留。
- 原因：只在 PageCache 写零或只剪 extent tree 都可能在 eviction/reopen 后恢复旧数据，按 size 推算
  `st_blocks` 又会隐藏物理块未释放。旧 delayed metadata 若在 extent 修改后提交，还会把 stale inode
  block count 写回。
- 后续影响：extent 破坏性操作前先提交旧 metadata，并在 lower inode ref 落盘后结束 write-back cache；
  跨映射失效必须在 File lock 外执行。default/KEEP_SIZE 预分配仍需独立 unwritten extent，不能复用打洞
  入口或稀疏 truncate 伪装。

## writable shared file PTE 先 write-protect，再在 fault 中建立 backing

- 状态：已采用，RV64/LA64 真实小盘 ENOSPC 与恢复专项通过
- 适用范围：ext4 `MAP_SHARED|PROT_WRITE`、mprotect、truncate/punch、SIGBUS
- 最后验证：2026-08-16
- 证据：`os/src/mm/memory_set.rs`、`os/src/fs/file.rs`、双架构
  `/tmp/respos-{rv,la}-mmap-enospc-phase5-final.log`
- 决策：共享可写 VMA 与 PTE 权限分层；VMA 允许写，但新 resident file PTE 保持只读。store fault 在
  File state lock 下重新确认 live EOF，并为 ext4 页物化真实 block；成功才把 PTE 改为可写，`ENOSPC/EIO`
  作为 bus fault。mprotect write、truncate grow 和 punch refault 都重入同一协议。
- 原因：一开始映成 writable 会让 store 绕过内核，空间错误只能延迟到 msync/fsync，已经无法撤回用户
  已执行的共享写。只在 PageCache 记 reservation 又不能证明物理空间存在。
- 后续影响：支持 unwritten extent 后可替换“写当前字节”的 ext4 backend，但不能移除 PTE fault gate；
  新文件系统必须明确其 reservation/ENOSPC 契约。File lock 必须覆盖 lower reservation 与 live-size
  复核，避免与 truncate 交错后反向扩容。

## user/system CPU 时间使用 trap mode transition，不维护 syscall stopwatch

- 状态：已采用，Linux/RV64/LA64 当前专项通过
- 适用范围：process/thread CPU clock、times/getrusage、wait4 child usage、proc stat
- 最后验证：2026-08-16
- 证据：`os/src/task/{task,process}.rs`、`os/src/arch/{rv64,loongarch64}/trap/mod.rs`、
  `scripts/cpu_accounting_phase5_probe_linux.c`、`task_a_wait4_probe`
- 决策：线程组共享的 process clock 每 hart slot 记录 user/system mode，thread clock 记录调用线程的同类
  两项；scheduler 只负责真实运行区间
  begin/end，user trap 入口/返回负责 mode transition。task 持久化 mode，使阻塞 syscall 恢复后仍计入
  system，直到实际返回用户态。process/thread total CPU clock都等于各自 user+system；Zombie 分别冻结
  process 两项，`RUSAGE_THREAD` 则直接快照当前 thread clock。
- 原因：按 syscall 函数入口/出口计时会遗漏 page fault、signal、timer trap，也无法正确覆盖 syscall 内
  block→resume；另建独立 total/user/system 三套计时则会在 SMP 并行和 exit snapshot 上漂移。
- 后续影响：任何从 user 返回的异常路径必须经过配对 transition；kernel trap 不重复切 mode。增加新的
  架构 trap return 或 CPU hotplug 时，必须一起审计 per-hart slot 和 task 持久 mode。

## rusage 资源字段使用 thread + stable process 双层计数

- 状态：已采用，fault/RSS/context-switch/block-I/O 当前专项通过
- 适用范围：getrusage、wait4/waitid、page fault、scheduler、leader exit/non-leader exec
- 最后验证：2026-08-16
- 决策：事件先记调用 TCB，并同步记 stable `ProcessState`；进程退出前更新 mm RSS 高水位，Zombie 保存
  process snapshot。成功 reap 后才把 child snapshot 提交给 parent，普通字段求和而 maxrss 取最大值。
  `RUSAGE_THREAD` 的 maxrss 复用 process mm high-water。
- 原因：只遍历 live threads 会在 thread exit 后丢账；把所有字段塞进 process 又无法实现
  `RUSAGE_THREAD`；在 wait 扫描阶段提前累计会使 bad user pointer 重试重复计数。RSS 是地址空间属性，
  不存在可靠的 thread-private 拆分。
- 决策：`ru_inblock` 在归属当前 task 的成功 block read submission 上累计；`ru_oublock` 在 disk-backed
  PageCache page 第一次 clean-to-dirty 时累计，重复 dirty write 和最终 writeback 不重复计数。Linux
  `getrusage` 不填 `ru_ixrss/ru_idrss/ru_isrss/ru_nswap/ru_msgsnd/ru_msgrcv/ru_nsignals`，因此这些字段
  明确保持 0，不建立与 Linux 可观察语义冲突的替代 counter。
- 决策：context-switch counter 在 idle loop 已知 next 后提交，而不是在 timer/yield/block 请求 handoff 时
  预增。只有 outgoing 与 next 不同才按 handoff 原因计 voluntary/involuntary；同 task 经 idle stack 选回
  自身不计，exit handoff 不计。
- 原因：Linux 在 scheduler 的 `prev != next` 分支内才增加 switch counter。RespOS 的 idle-stack handoff 是
  SMP context ownership 协议，若按该实现步骤计数，单任务会每 10 ms 虚增 `nivcsw`，且无竞争 yield 也会
  虚增 `nvcsw`。
- 后续影响：任何新增 wait reap 路径都必须同时累计时间与资源，并保持 copyout 失败原子性；`WNOWAIT`
  不得提交 children usage。新增 block backend 或绕过 PageCache 的 buffered-write 路径必须审计归属点。
