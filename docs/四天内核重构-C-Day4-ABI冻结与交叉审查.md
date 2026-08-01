# C 组 Day4：ABI 冻结与交叉审查

## 1. 冻结结论

Day4 完成了文件 ABI、epoll 生命周期和 B 组文件 mmap 接口的交叉审查，并按实际能力冻结：

- 不支持的 open/fcntl/fallocate/epoll flags 不再被静默接受；
- 普通文件不再通过 truncate 或空操作伪装 fallocate；
- `TIOCNOTTY` 不再假成功，RTC ioctl 只接受精确 request；
- epoll interest 使用 `(fd number, open-file identity)`，可区分 close 后复用的 fd；
- EPOLLET/EPOLLONESHOT 状态只在 events copyout 成功后提交；
- `epoll_pwait` 校验并临时替换 signal mask，所有返回路径恢复原 mask；
- 在共享写回错误能够可靠传播前，可写文件 `MAP_SHARED` 明确返回 `EOPNOTSUPP`。

这轮没有改写 B 组 `MemorySet`/VMA 主体。审查发现的问题被作为冻结边界记录，不以 C 组私有
补丁掩盖。

## 2. B 组文件 mmap 交叉审查

| 检查项 | 结论 | 冻结边界 |
| --- | --- | --- |
| VMA 切分后的 backing offset | 通过静态审查 | `meta_for_subrange` 按子区间同步调整文件 offset/len |
| `MAP_PRIVATE` / `MAP_SHARED` | 子集明确 | writable private 与 read-only shared 可用；writable shared 返回 `EOPNOTSUPP` |
| fault 慢 I/O 的锁顺序 | 未收口 | `map_one` 读取 backing file 时调用方仍持有 `MemorySet` 写锁 |
| `munmap` / `msync` dirty 责任 | 未收口并已隔离 | 现有 shared writeback 在 MM 锁内执行且忽略错误；因此禁止 writable shared；非空 `msync` 返回 `EOPNOTSUPP` |
| truncate 与映射页 | 部分明确 | private 映射保持私有快照；read-only shared 与 truncate 的缓存失效尚无完整契约 |

后续若要开放 writable shared，B 组至少需要提供 prepare/writeback/commit 形式的锁外写回接口，
让错误能够返回 `munmap/msync`，并定义 truncate 对 resident mapped page 的失效或 SIGBUS 规则。

## 3. 本轮 ABI 支持分级

定义：A 为核心语义有回归；B 为明确子集；C 为显式不支持；D 为假成功。

| 家族 | 等级 | 当前承诺 |
| --- | --- | --- |
| `pread/pwrite/preadv/pwritev` | A | 使用独立 positioned offset，不改变 dup/fork 共享 offset；覆盖多 iovec 与部分成功 |
| `fsync/fdatasync` | A | page cache 写回并同步 superblock；`fdatasync` 使用更强的完整 fsync fallback |
| `sync_file_range` | B | 支持的 wait/write 子集执行完整 fsync fallback；无效 flags 拒绝；不承诺 Linux 范围写回优化 |
| `fcntl` | B | descriptor CLOEXEC 与 open-file status flags 分离；支持命令/flag 子集；`O_DIRECT/O_ASYNC` 显式拒绝 |
| `renameat2` | B | flags 0 与 `RENAME_NOREPLACE` 有生命周期回归；其他模式显式不支持 |
| `fallocate` | B/C | memfd 支持 `PUNCH_HOLE|KEEP_SIZE`；普通文件预分配、KEEP_SIZE 和 punch hole 返回 `EOPNOTSUPP` |
| `chmod/chown` | B | 仅承诺现有 Unix mode/owner 子集，不承诺 ACL、capability 或完整权限模型 |
| `stat/statfs` | B | 基础 inode/size/type 和文件系统容量字段；不承诺 Linux 全部扩展字段 |
| 文件相关 `ioctl` | B/C | 保留 winsize、FIONREAD、精确 RTC/device 分支；未知 request 返回 `ENOTTY`，`TIOCNOTTY` 不假成功 |
| file `mmap` | B/C | writable private、read-only shared；writable shared 和非空 `msync` 返回 `EOPNOTSUPP` |
| epoll | A/B | IN/OUT、ET、ONESHOT、pwait mask、EFAULT 提交和当前进程 close/fd reuse 有回归；其他 event bits 拒绝 |
| Day1 空壳 special-fd | C | 未具备最小生命周期的接口返回 `ENOSYS` |

本轮触及范围内 D 级条目已清零。这里的“清零”只表示已审计范围内没有已知假成功，不能外推为
整个 syscall 表完整实现。

## 4. epoll 状态与生命周期

ready 扫描现在分为 preview 与 commit：扫描只形成待 copyout 事件；copyout 成功后才更新 ET 的
`last_ready` 并消费 ONESHOT。interest 带 generation，MOD 与并发提交不会把旧 preview 写回新状态。

close/reuse 的 key 不再只是 fd 数字。扫描会基于当前任务的 open-file identity 清理 stale interest，
因此同一进程中 `ADD → close → fd reuse → ADD/DEL/wait` 不会操作旧对象。跨进程通过 fork 保留的
epoll/file alias 尚未建立全局 close 通知，所以该结论不扩展到完整 Linux epoll 跨进程生命周期。

`epoll_pwait` 对非空 mask 要求 `sigsetsize == sizeof(SigSet)`，移除不可屏蔽的 SIGKILL/SIGSTOP，
等待前替换 mask，并在成功、EFAULT、EINTR、timeout 等所有返回路径恢复。

## 5. 双架构验证

回归程序：`user/src/bin/fs_day4_freeze.rs`。

| 架构 | debug build | Day4 freeze | Day3 防回退 | Day2 防回退 |
| --- | --- | --- | --- | --- |
| RISC-V | PASS | PASS | PASS | PASS |
| LoongArch | PASS | PASS | PASS | PASS |

Day4 覆盖 open/fcntl/fallocate/ioctl/mmap 的拒绝与子集语义，以及 epoll EFAULT、ONESHOT、ET、
signal-mask size、close/fd reuse。Day3/Day2 完整回跑用于验证 ABI 收紧没有破坏路径、cache、sync、
positioned I/O 和 fd flag 语义。

## 6. 冻结后的未完成项

- B：fault 仍可能在 MM 全局写锁下读文件；shared dirty、msync/munmap 错误传播和 truncate
  mapped-page 契约未完成；
- A：pipe/poll waiter 与 scheduler 的 blocked/signal/close 竞争尚未取得 owner 契约；
- C：普通共享-offset read/write 仍持 `FileInner` 跨慢 I/O，以保持 offset/append 原子性；
- epoll：跨进程别名的最后 close 通知和 waiter/timeout/signal 全竞争矩阵仍需系统级压力测试；
- 当前工作树未按语义创建提交，因此“可独立回退”仍是交付流程项，不宣称完成。

