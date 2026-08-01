# C 组 Day1：文件与 fd 型 ABI 审计台账

> 基线：`3839e4e5b7fd`  
> 审计范围：`os/src/fs/`、`os/src/syscall/fs.rs`、`os/src/syscall/special_fd.rs`，网络 syscall 仅静态分级  
> 分级口径：A=核心语义及回归完整；B=明确的受限实现；C=显式不支持；D=假实现或存在可观察语义错误

## 1. Day1 结论

1. 红名单中的 12 个 fd 型接口都只分配通用空 `SpecialFd`，没有对应子系统的 use、poll/ioctl、fork/exec、close/exit 语义。本次统一降级为 `ENOSYS`。
2. `pread64`、`pwrite64`、`preadv`、`pwritev` 以及 `splice` 的显式 offset 都用 seek/read-or-write/seek-restore 模拟 positioned I/O，会临时修改共享 open-file offset，当前为 D。
3. `F_SETFL` 修改 `FdEntry.flags`，dup/fork 后同一打开文件的 status flags 会分叉，当前为 D；`CLOEXEC` 位于 `FdEntry`，归属正确。
4. `epoll_pwait` 忽略 signal mask 和 size；ready 扫描在 copyout 前消费 ET/ONESHOT；interest 又仅以 fd 数字为 key，close 后复用会操作旧 interest，当前为 D。
5. `msync` 对非空映射只校验后返回成功；`sync_file_range` 在没有 `WRITE` 时返回成功但不等待；两者为 D。普通 `File::fsync` 会触发 page-cache 写回，但 ext4 superblock `sync()` 仍有 `todo!()`。
6. `rename` 在后端 rename 成功前删除目标，后续失败会污染后端和 dentry 状态，是 P0。`unlink` 的后端变更成功后才清 dentry，提交顺序相对清楚。
7. `memfd_create` 有真实数据、seek、seal、dup/close 语义，可保留为 B；但 HUGETLB flags 被静默接受。`timerfd` 有基本 create/set/read/poll 生命周期，可保留为 B；但 `CANCEL_ON_SET` 和 alarm clock 语义不真实。
8. page-cache 写回已使用 `write_version` 防止写回期间的再次修改被错误清 dirty，且慢 I/O 前释放 page lock；这一不变量可以作为 Day2/Day3 的正向基线。

## 2. FS syscall 分级

“支持范围/主要缺口”是本轮承诺边界；没有单独回归证据的接口不评为 A。

| syscall / 家族 | 等级 | 支持范围 / 主要缺口 | 风险 |
| --- | --- | --- | ---: |
| `read`, `write`, `readv`, `writev` | B | 普通 FileOp 和部分 special fd；共享 offset | 3 |
| `pread64`, `pwrite64`, `preadv`, `pwritev` | D | seek/restore 污染共享 offset | 6 |
| `preadv2`, `pwritev2` | D | flags 非 0 会拒绝，但 positioned 分支继承 offset 问题 | 6 |
| `sendfile`, `copy_file_range` | B | 内核缓冲复制；显式 offset 与部分成功语义需专项回归 | 5 |
| `splice` | D | 显式 offset 使用 seek/restore，污染共享 offset | 8 |
| `tee`, `vmsplice` | B | 仅 FileOp/pipe 支持子集；flags 边界需回归 | 7 |
| `fadvise64` | B | 仅参数校验/受限 advice，不承诺真实缓存策略 | 1 |
| `openat`, `openat2` | B | create/open/tmpfile 子集；`OpenFlags::from_bits_truncate` 会吞未知位 | 7 |
| `close` | B | 删除一个 fd 引用；锁清理语义仍需验证 | 4 |
| `close_range` | B | close/CLOEXEC/UNSHARE 子集 | 6 |
| `dup`, `dup3` | B | File Arc 与 offset 共享、CLOEXEC 独立；status flags 模型错误 | 7 |
| `fcntl` | D | dup/FD_CLOEXEC/pipe/seal/lock 子集；`F_SETFL` 错放在 FdEntry | 8 |
| `flock` | B | 内核内存锁表；等待为 yield，退出/最后引用清理需回归 | 7 |
| `fstat`, `fstatat`, `statx` | B | metadata 查询子集，部分 mask/flag 语义简化 | 1 |
| `statfs`, `fstatfs` | B | ext4/部分 special fd | 1 |
| `lseek` | B | SET/CUR/END；无 SEEK_DATA/SEEK_HOLE | 3 |
| `ftruncate`, `truncate` | B | 普通文件/memfd 子集；cache/backend 一致性需回归 | 6 |
| `fallocate` | B | 受限 mode；未实现模式应明确拒绝 | 5 |
| `fchmod`, `fchmodat` | B | 权限模型子集 | 4 |
| `fchown`, `fchownat` | B | 凭据/权限模型子集 | 6 |
| `utimensat` | B | 时间更新子集 | 4 |
| `faccessat` | B | 单 namespace/凭据模型子集 | 3 |
| `setxattr`, `lsetxattr`, `fsetxattr` | B | inode xattr 与 user namespace 限制子集 | 5 |
| `getxattr`, `lgetxattr`, `fgetxattr` | B | 查询与 ERANGE | 1 |
| `listxattr`, `llistxattr`, `flistxattr` | B | 查询与 ERANGE | 1 |
| `removexattr`, `lremovexattr`, `fremovexattr` | B | inode xattr 删除 | 5 |
| `mkdirat`, `mknodat`, `symlinkat`, `linkat` | B | 单 namespace/VFS 子集；`linkat` 含 `/proc/self/fd` 特例 | 7 |
| `renameat2` | D | 不支持 EXCHANGE；NOREPLACE 未传入核心层；失败可先删除目标 | 9 |
| `unlinkat` | B | file/rmdir 子集；打开后 unlink 使用 ext4 orphan 路径 | 8 |
| `chdir`, `fchdir`, `chroot`, `getcwd` | B | 单 mount namespace 子集 | 4 |
| `getdents64`, `readlinkat` | B | 目录缓存/符号链接读取 | 3 |
| `pipe2` | B | 创建、读写、poll、dup/fork/close；copyout 失败会回收两个 fd | 9 |
| `ppoll`, `pselect6` | B | 基本 waiter/timeout/临时 mask；与调度器竞态待 A 组确认 | 9 |
| `epoll_create1`, `epoll_ctl`, `epoll_pwait` | D | identity、EFAULT、ET/ONESHOT、pwait mask 均有缺口 | 11 |
| `eventfd2` | B | read/write/nonblock/poll/dup/close 基本生命周期 | 8 |
| `timerfd_create`, `timerfd_gettime`, `timerfd_settime` | B | 基本 timer；alarm 与 CANCEL_ON_SET 不支持却被接受 | 9 |
| `memfd_create` | B | 数据、offset、seal、mmap hook；HUGETLB 未实现却被接受 | 8 |
| `fsync`, `fdatasync` | B | 普通 File page cache 写回；fdatasync 未区分 metadata | 6 |
| `sync_file_range` | D | WRITE 退化为整文件 fsync，wait-only 假成功 | 6 |
| `msync` | D | 非空映射未执行写回/失效 | 6 |
| `ioctl` | B | 终端/loop/block 等硬编码子集，需继续下沉到 FileOp | 5 |
| `mount`, `umount2` | B | 旧 mount API 的受限文件系统/flags | 8 |
| `inotify_init1`, `signalfd4`, `pidfd_open`, `fanotify_init` | C | Day1 起固定返回 `ENOSYS` | 9 |
| `userfaultfd`, `perf_event_open`, `io_uring_setup`, `bpf` | C | Day1 起固定返回 `ENOSYS` | 9 |
| `fsopen`, `fspick`, `open_tree`, `memfd_secret` | C | Day1 起固定返回 `ENOSYS` | 9 |

## 3. 网络 syscall 静态分级

网络主体不进入本轮重构；以下结论只决定是否存在 P0，不代表完成网络验收。

| syscall / 家族 | 等级 | 静态结论 |
| --- | --- | --- |
| `socket`, `socketpair`, `accept`, `accept4` | B | 都创建真实 Socket；多 fd/copyout 失败有回收路径 |
| `bind`, `listen`, `connect`, `shutdown` | B | TCP/UDP/Unix 子集；`listen` 忽略 backlog |
| `getsockname`, `getpeername` | B | 地址族子集 |
| `sendto`, `recvfrom` | B | flags 被忽略；大 len 会一次性分配 |
| `sendmsg`, `recvmsg`, `sendmmsg`, `recvmmsg` | B | iovec 子集；控制消息、timeout/flags 语义不完整 |
| `setsockopt`, `getsockopt` | D | 多个选项读取后假成功或返回固定值 |

静态审查没有发现需要 C 组立即越界修改网络主体的确定 P0。`setsockopt` 假成功列入网络 owner 的 P1 台账。

## 4. 空壳 fd 批量降级

| 接口 | Day1 前行为 | 缺失生命周期 | Day1 结论 |
| --- | --- | --- | --- |
| `inotify_init1` | 空 `SpecialFd` | watch add/rm、read events、poll | `ENOSYS` |
| `signalfd4` | 空 `SpecialFd` | mask update、read siginfo、poll | `ENOSYS` |
| `pidfd_open` | 空 `SpecialFd` | process identity、poll、send signal、pid reuse | `ENOSYS` |
| `fanotify_init` | 空 `SpecialFd` | mark、event read/response、owner exit | `ENOSYS` |
| `userfaultfd` | 空 `SpecialFd` | ioctl negotiation、range、fault read/wake | `ENOSYS` |
| `perf_event_open` | 空 `SpecialFd` | attr、read/ioctl/mmap、group/owner | `ENOSYS` |
| `io_uring_setup` | 空 `SpecialFd` | ring mmap、register、enter、owner exit | `ENOSYS` |
| `bpf` | 空 `SpecialFd` | map/program identity、commands、close | `ENOSYS` |
| `fsopen` | 空 `SpecialFd` | fsconfig/fsmount state machine | `ENOSYS` |
| `fspick` | 空 `SpecialFd` | filesystem context state machine | `ENOSYS` |
| `open_tree` | 空 `SpecialFd` | mount object、move_mount、mount lifetime | `ENOSYS` |
| `memfd_secret` | 空 regular `SpecialFd` | secret mapping/security contract | `ENOSYS` |

注：任务书红名单写“11 个接口”，逐项实际包含 `bpf` 后共有 12 个，本表按文档的全部条目处理。

## 5. 高风险状态影响审计

### 5.1 `epoll_ctl` / `epoll_pwait`

- 输入：epfd、target fd、event 指针；pwait 另有 events、maxevents、timeout、sigmask、sigsetsize。
- 读取对象：进程 FdTable、EpollFd、target `Arc<dyn FileOp>`。
- 修改对象/共享边界：EpollFd interest map，跨所有 dup/fork 后共享该 epoll File 的引用。
- copyin/copyout：ADD/MOD 先 copyin event；wait 最后逐项 copyout event。
- 第一次状态修改：`scan_ready` 在 copyout 前写 `last_ready`，并可能置 `disabled=true`。
- 失败状态：events copyout EFAULT 会永久消费 ET edge 或 ONESHOT；无回滚。
- identity：map key 仅为 fd 数字但 value 持旧 File。target close 后 fd 复用，DEL/MOD 按新 fd 数字操作旧 interest；ADD 返回错误 EEXIST。
- 锁：持 interest spin lock 调底层 `read_ready`/`write_ready`/`register_poll_waiter`，存在跨对象锁顺序风险。
- signal：pwait 完全忽略 mask 与 size。
- 结论：D，P1。Day2 应拆为 peek-ready → copyout → commit-ready，并定义 `(fd, open-file identity)`。

### 5.2 `timerfd_settime`

- copyin：先读取新 `itimerspec`；copyout：若请求 old value，在 commit 前写回，顺序正确。
- 修改对象：共享 TimerFd state；dup/fork 观察同一 timer。
- flags：未知位返回 EINVAL；`CANCEL_ON_SET` 被接受但没有 realtime discontinuity/cancel 状态。
- clock：alarm clock 被当普通 clock 使用，没有唤醒/权限差异。
- 生命周期：全局表只持 Weak，最后引用释放后可清理；owner exit 不额外泄漏。
- 结论：B（基本 timer），虚假 flags 为 P1；未实现能力应拒绝。

### 5.3 `memfd_create`

- copyin：name 在分配 fd 前完成；fd 分配是提交点，无后续 copyout。
- 共享：dup/fork 共享同一 `SpecialFd`，因此共享 offset、data、seals；通过 proc reopen 创建独立 offset、共享 data/seals。
- CLOEXEC：在 FdEntry 中，exec 可独立关闭。
- flags：HUGETLB/huge-size 位被允许，但实际仍是普通 Vec backing。
- close/exit：Arc 最后引用释放 data；无 owner registry。
- 结论：B；HUGETLB 部分为 D/P1，应 `EOPNOTSUPP`。

### 5.4 `pread64` / `preadv`

- copyin：iov 元数据与全部目标 buffer 可写性在文件状态变化前校验。
- 修改对象：共享 File offset；本应不修改。
- 第一次状态修改：保存 offset 后 `seek(requested)`；普通 read 持续推进 offset。
- 并发：dup/fork/线程可观察临时 offset，并能让最终 restore 覆盖并发 seek/read 的新 offset。
- copyout 失败：`pread64` 的中途 copyout EFAULT 可绕过 restore；`preadv` 经 `sys_read` 也存在同类风险。
- 结论：D，P1。Day2 必须改用 `read_at_offset` 和局部 checked offset。

### 5.5 `pwrite64` / `pwritev`

- copyin：每个 chunk 在 seek 后才 copyin；EFAULT 前已污染共享 offset。
- 修改对象：inode/page cache 与共享 File offset；数据写入不可回滚。
- 并发：与 pread 相同，且 offset 竞争会把数据写到错误位置。
- 部分成功：已有按 total 返回的轮廓，但 restore 错误与并发状态无法可靠解释。
- 结论：D，P0（可能写错位置）。Day2 改用 `write_at_offset`。

### 5.6 `fcntl(F_SETFL)` / dup / fork / exec

- 读取对象：FdEntry；修改对象：仍是单个 FdEntry。
- 正确共享边界：CLOEXEC 属于 descriptor；APPEND/NONBLOCK/DIRECT 属于 open-file description。
- 当前状态：所有位都保存在 `FdEntry.flags`；File 另有创建时不可变 flags。
- dup/fork：克隆 FdEntry，因此后续 F_SETFL 仅影响一个 entry；FileOp 中真正执行 I/O 的 flags 又可能不变。
- exec：`close_on_exec` 读取 FdEntry CLOEXEC，正确。
- 结论：D，P1。Day2 至少让 GETFL/SETFL 读写共享 File status。

### 5.7 `pipe2`

- copyin：flags 完整校验；copyout：两个 fd 数字。
- 提交：先分配 read fd，再分配 write fd；第二次失败回收第一个；copyout 失败回收两者。
- 共享：dup/fork 共享 pipe endpoints；close 一个引用不会关闭最后 endpoint。
- waiter：poll/read/write 与 scheduler 的 wake/timeout/signal 竞态由 A 组协议决定。
- 结论：B；prepare/rollback 结构可作为 fd-pair 创建的正向样例。

### 5.8 `fsync` / `fdatasync` / `sync_file_range` / `msync`

- `File::fsync`：对普通 page cache 调 `PageCache::sync`，慢后端 write 时不持 page/global pages lock；version 相同才清 dirty。
- `fdatasync`：完全等同 fsync，属于明确但较强的受限语义。
- `sync_file_range`：WRITE 退化为整文件 fsync；wait-only 与 flags=0 返回成功但不等待。
- `msync`：验证页对齐、flags、映射范围后对非空范围直接成功。
- superblock：`Ext4SuperBlock::sync()` 为 `todo!()`；当前 grep 未发现生产调用点，但一旦接入全局 sync/shutdown 就会 panic。
- 结论：fsync/fdatasync=B；sync_file_range/msync=D。假成功为 P1，todo 为 P0 防线。

### 5.9 `renameat2`

- copyin：两条路径在核心修改前完成。
- flags：EXCHANGE 拒绝；NOREPLACE 被接受但没有传给 `filename_rename`，可能错误覆盖目标。
- 第一次状态修改：若目标存在，先调用 unlink 并移除目标 dentry/cache；随后才调用后端 `file_rename`。
- 失败状态：后端 rename 失败时，目标已被删除且无回滚，可能数据丢失；旧 dentry 尚在但新目标状态已污染。
- 锁：`NAMEI_MUTATION_LOCK` 串行化本内核的 namei 修改，不能解决错误提交顺序。
- 结论：D，P0。必须让后端 rename 成为原子提交点，成功后再更新 dentry cache。

### 5.10 `unlinkat`

- copyin/校验：路径、flags、mount writable、权限、type 与 sticky bit 均在后端变更前完成。
- 修改：普通路径先后端 unlink，再删除 parent child 与 cache；打开的末链接 ext4 文件走 orphan 路径保留 inode。
- 共享：已打开 File 继续持 inode/page cache；同名重建应通过 cache tree 删除获得新 identity。
- 风险：orphan 判断依赖 `Arc::strong_count > 2`，不是稳定的“打开文件数”协议；需失败注入与生命周期回归。
- 结论：B，P1 风险观察项。

## 6. P0/P1 红名单与处置

| 优先级 | 问题 | Day1 处置 | 后续 owner |
| --- | --- | --- | --- |
| P0 | pwrite 系列共享 offset 竞争可写错位置 | 复现计划已定义 | C / Day2 |
| P0 | splice 显式 offset 竞争可读写错位置 | 审计定位 | C / Day2 |
| P0 | rename 先删除目标、后端 rename 后提交 | 审计定位 | C / Day3 |
| P0 | ext4 superblock sync 的 `todo!()` | 调用点审计；当前无生产调用 | C / Day2 |
| P1 | 空壳 fd 能力误报 | 全部降级为 `ENOSYS` | C / 已完成 |
| P1 | epoll EFAULT 消费 ET/ONESHOT | 失败注入计划已定义 | C / Day2-3 |
| P1 | epoll fd reuse identity 错误 | 生命周期计划已定义 | C / Day2-3 |
| P1 | epoll_pwait 忽略 signal mask | 明确为 D | C+A / Day2 |
| P1 | F_SETFL 不在共享 File | 共享图与测试已定义 | C / Day2 |
| P1 | msync/sync_file_range 假成功 | 明确为 D | C+B / Day2 |
| P1 | timerfd CANCEL_ON_SET/alarm 假支持 | 明确为受限边界 | C+A / Day2 |
| P1 | memfd HUGETLB 假支持 | 明确为受限边界 | C+B / Day2 |

## 7. 失败注入与最小回归计划

### 7.1 epoll

1. `ADD(target)` → close target → 创建新 fd 复用相同数字 → ADD/MOD/DEL，核对旧 open-file interest 不被新 fd 误操作。
2. ONESHOT ready 后把 events 指向非法页，第一次返回 EFAULT；修正地址后事件仍应可取。
3. ET ready → not-ready → ready，严格得到两次 edge。
4. `epoll_pwait` 使用正确/错误 sigsetsize，等待期间投递被临时 mask 的 signal，返回后检查原 mask 恢复。
5. waiter 注册、timeout、signal、target close 相邻发生，重复运行并检查无残留 waiter。

### 7.2 sync

1. 后端 `write_at` 注入 EIO/短写；fsync 必须返回错误，dirty page 保留。
2. page snapshot 写回期间再次 write；旧写回完成不得清新版本 dirty。
3. `sync_file_range` 分别测试 WAIT_BEFORE/WRITE/WAIT_AFTER 与组合；未支持的组合不得成功。
4. `msync(MS_SYNC)` 对 shared file mapping 修改后注入后端失败，必须可观察错误且 dirty 保留。
5. ext4 cache flush 注入失败；shutdown/superblock sync 不 panic、不假成功。

### 7.3 fd/open-file 生命周期

1. open → dup → `F_SETFL(O_APPEND/O_NONBLOCK)`，两个 fd 的 `F_GETFL` 与行为一致。
2. fork 后 child 修改 status flag，parent 观察一致；child 设置 CLOEXEC 不影响 parent/另一个 descriptor。
3. pread/pwrite 经 dup 和 fork 均不改变共享 offset；多 iovec、短 I/O、EFAULT、offset checked-add 溢出。
4. close 一个引用后对象仍可用；close 最后引用释放；随后复用 fd 数不继承旧对象状态。
5. pipe/eventfd/timerfd/memfd 分别覆盖 create → use/poll → dup/fork → exec → close/exit。

### 7.4 namei

1. rename 后端 EIO 前后分别注入，失败时 old 与 new 内容和 dentry identity 均不变。
2. `RENAME_NOREPLACE` 在目标存在时返回 EEXIST 且两边不变。
3. unlink 后立刻同名 create，lookup 得到新 inode，旧打开 fd 仍读到旧对象。
4. unlink 后端失败时 parent child、全局 dentry cache、page cache 全不变。

## 8. fd / open-file identity 与生命周期

```text
Task / Process
  └─ Arc<FdTable>
       ├─ fd 3 → FdEntry { descriptor flags: CLOEXEC }
       │           └─ Arc<FileOp A>  ← open-file identity
       └─ fd 7 → FdEntry { descriptor flags: 0 }       (dup)
                   └─ Arc<FileOp A>
                        ├─ shared offset
                        ├─ shared status: APPEND / NONBLOCK / access mode
                        └─ Path(mount, dentry) → Inode → PageCache
```

生命周期不变量：

```text
open/create
→ fd entry owns one Arc reference
→ dup/fork clones entry and Arc, CLOEXEC remains per entry
→ positioned I/O uses local offset only
→ close removes one entry, File remains while any Arc/open mapping exists
→ exec removes only CLOEXEC entries
→ last Arc release performs object cleanup
→ reused fd number has no authority over the old File identity
```

当前模型差异：`FdEntry.flags` 同时携带 descriptor flags 和 status flags，而 `FileInner.flags` 又保存创建时副本。Day2 需要以最小改动建立单一 status flag 真源。

## 9. 跨组接口

### 给 A 组：waiter 协议

- C 只调用 `prepare_current_task_blocked`、注册/注销 waiter、timeout finish 与 signal 检查，不直接改 scheduler。
- A 需保证 ready、timeout、signal、close 四个完成原因只提交一次；返回后 task 不残留在 ready/blocked/timeout/waiter 任一队列。
- waiter 注册必须支持“注册后立刻复查 ready”的无丢失唤醒流程；注销必须幂等。
- epoll 临时 signal mask 的原子替换/恢复由 A 提供现有 signal-state helper 或确认可用临界区。

### 给 B 组：文件 mmap / msync 协议

- B 提供按 `[addr, addr+len)` 枚举稳定 file-backed VMA 快照的接口，返回 File identity、file offset、shared/private、dirty 范围。
- C 提供 `FileOp` 范围写回接口或明确整文件 fsync fallback；不得在持 MemorySet 全局写锁时执行慢 I/O。
- `MS_SYNC` 在返回前传播写回错误；`MS_ASYNC` 若无后台 writeback 则明确拒绝；`MS_INVALIDATE` 若无法保证映射一致则明确拒绝。
- truncate 与映射页的失效/SIGBUS 边界由 B 管 VMA，C 管 inode size/page cache size，双方以同一提交顺序测试。

上述是 C 组提出的接口契约；需要 A/B owner 在集成前确认，Day1 未越界修改 scheduler 或 `MemorySet`。

## 10. Day1 闸门

- [x] FS/special-fd syscall 完成分级，网络 syscall 完成静态分级。
- [x] 文档红名单的全部空壳 fd 有明确结论并已降级。
- [x] 完成 10 份高风险状态影响审计。
- [x] 给出 P0/P1、处置结论和失败注入列表。
- [x] 给出 fd/open-file identity 与生命周期图。
- [x] 给出 A waiter 与 B 文件 mmap 接口契约。
- [ ] A/B owner 完成接口确认。
- [x] 双架构 debug build 通过：`make build-rv RV_MODE=debug`、`make build-la LA_MODE=debug`。
