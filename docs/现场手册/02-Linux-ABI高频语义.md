# Linux ABI 高频语义

实现一个 syscall 的编号和 happy path 只是起点。现场回归优先检查返回值、精确 errno、错误优先级、
失败原子性、并发提交点、生命周期以及 musl/glibc 可观察行为。完整依据见
[`decisions.md`](../codex/decisions.md) 和 [`pitfalls.md`](../codex/pitfalls.md)。

## 高频检查表

| 领域 | 必查语义 | RespOS 已知高风险点 |
| --- | --- | --- |
| 用户指针 | NULL、跨页、只读/不可执行、部分有效区间 | pathname 的 4096 字节限制不能套给 argv/envp；保留 `copy_to/from_user` 边界 |
| `clone` / pthread | `CLONE_VM/FILES/FS/SIGHAND/THREAD/VFORK`，TID 写回与 clear-tid | vfork 必须先登记父阻塞再发布子进程；共享资源退出不能看 `Arc::strong_count` |
| `execve` | argv/envp 大小、ELF interpreter、线程组收缩、失败保留旧映像 | 先在旧地址空间清理 sibling；用户栈溢出可能伪装成 loader 权限错误 |
| `exit` / `wait*` | zombie、stop/continue、pid 过滤、rusage、EINTR/restart | `wait4` 的 SA_RESTART；无关 zombie 不能使特定 pid wait 忙转 |
| signal | pending、mask、实时信号排队、alt stack、sigreturn | standard 与 realtime 不能共用简单 `BTreeMap<signo,...>`；晚到 EINTR 不能污染下次 syscall |
| futex | private/shared key、原子比较、timeout、requeue、退出唤醒 | 映射实例 identity 不是 shared futex backing identity |
| `mmap` / `mprotect` | offset/length 对齐、MAP_SHARED/PRIVATE、EOF、COW、回滚 | writable `MAP_SHARED` 必须工作；普通 mmap EOF 与 ELF 尾页不同；失败不总能整段回滚 |
| fd / open file | fd table 与 File 分层、dup 后共享 offset/flags、close 生命周期 | inode 存活引用不等于 open File 数；`posix_fadvise` 状态属于 open file description |
| 路径与目录 | dirfd、symlink、chroot、unlink-open、错误优先级 | chroot 先验 privilege 检查会遮蔽 pathname/权限错误；缓存项不证明仍在 namespace |
| read/write | short I/O、EINTR、append、offset 提交、copyout 错误 | Linux 中 `pwrite` + `O_APPEND` 采用 Linux 行为；整次 writev 的 `PIPE_BUF` 原子性 |
| pipe/FIFO | 空读、满写、EOF、EPIPE/SIGPIPE、open 配对 | `O_TRUNC`、open 配对和 EOF 是三层独立语义；不能 yield 轮询 |
| ext4 / cache | inode identity、dirty、writeback error、ENOSPC、truncate/hole | buffered I/O 与 MAP_SHARED 共用 PageCache；不能用 size 推算 `st_blocks` |
| 时间 | realtime/monotonic、精度、absolute/relative、remaining time | 100 Hz tick 不应粗化亚 10ms deadline；RTC metadata 不能继续用 monotonic |
| socket | stream/datagram 差异、blocking/poll、peer close、SIGPIPE、raw address | AF_UNIX abstract 名称不是 UTF-8 pathname；RDHUP 与缓冲读空不是一回事 |
| ABI 布局 | 架构字长、对齐、padding、signedness、command width | RV64/LA64 的 `epoll_event` 不能套 x86 packed 布局；ioctl command 截断为 32-bit unsigned |

## 处理顺序

对每个失败回答以下问题：

1. 用户传入的原始寄存器参数是什么，结构体 ABI 布局是否正确？
2. 参数读取和权限检查按什么顺序发生，哪个 errno 应优先？
3. 哪一步是不可逆的提交点？提交前失败是否保持原状态？
4. syscall 被 signal 打断时应返回、重启还是报告剩余时间？
5. fd、File、inode、进程、线程组、VMA 或 waiter 的生命周期由谁拥有？
6. 返回用户空间前 copyout 失败，前面的状态应该保留还是回滚？
7. musl/glibc 是否在用户态改写参数或吞掉行为，内核实际能观察到什么？

## 常量与结构体的可靠来源

优先级从高到低：

1. 当前目标架构的 Linux UAPI 头文件和可运行 Linux 对照程序；
2. 当前 RespOS 的 `os/src/config.rs`、`os/src/syscall/` 和架构 trap/context 定义；
3. 对应架构 ELF ABI、RISC-V/LoongArch 手册；
4. 网络文章或其他架构实现，只能作为线索。

不要凭记忆抄 syscall 编号、flag 位、signal frame、页表位或结构体 padding。若两架构表现不同，先写一个
最小 C/Rust probe 在各自 Linux 环境打印 `sizeof`、`offsetof`、返回值和 `errno`，再改内核。

## 结果是否可信

- “返回 0”不等于产生了要求的持久化或资源分配结果。
- 单项 LTP 通过不证明实现完整，例如 fallocate 还要检查真实预分配和 `st_blocks`。
- `make rv/la` 返回 0 不等于测试通过，必须读 guest 的 summary 和首个错误。
- 历史分数只属于当时的 commit、镜像、命令、架构和日期，不能直接代表当前 HEAD。
- libc 自身的非可移植行为不能自动成为内核 oracle；尽量使用 syscall 级 probe 和 Linux 对照。
