# C 组 Day2：positioned I/O、sync 与 fd 语义

> 基线：`3839e4e5b7fd`，包含 C 组 Day1 修改  
> 实现范围：`os/src/fs/`、`os/src/syscall/fs.rs`、special fd 小范围代码及文件 ABI 回归

## 1. 完成结论

Day2 的三个主要目标已经完成：

1. `pread64`、`pwrite64`、`preadv`、`pwritev` 不再调用 seek，也不再读取或恢复共享 offset；`splice` 的显式 offset 同步改为局部 positioned offset。
2. ext4 superblock 的 `todo!()` 已替换为可传播 EIO 的真实 cache flush；普通文件 fsync 在 page-cache 写回后继续 flush superblock；`sync_file_range` 使用整文件同步作为较强 fallback；无法安全实现的非空 `msync` 明确返回 `EOPNOTSUPP`。
3. `F_GETFL/F_SETFL` 改为读取和更新共享 FileOp status flags；CLOEXEC 继续只由 FdEntry 管理。dup 和 fork 共享 APPEND/NONBLOCK，descriptor CLOEXEC 保持独立。

此外，timerfd alarm clock、`CANCEL_ON_SET` 和 memfd HUGETLB 不再被静默接受。

## 2. positioned I/O

### 2.1 FileOp 契约

`FileOp` 新增：

```text
read_at_offset(offset, buffer)
write_at_offset(offset, buffer)
```

默认实现返回 `ESPIPE`。普通 `File` 复用已有 page-cache/inode positioned I/O；memfd 直接按 backing Vec 下标读写。pipe、socket、eventfd 等不可 seek 对象不会退化为 seek/restore。

### 2.2 syscall 行为

| syscall | Day2 行为 |
| --- | --- |
| `pread64` | 使用局部 offset 分块读取；共享 offset 不变 |
| `pwrite64` | 使用局部 offset 分块写入；共享 offset 不变 |
| `preadv` | 每段以 checked-add 推进局部 offset，保留短读与部分成功 |
| `pwritev` | 每段以 checked-add 推进局部 offset，保留短写与部分成功 |
| `preadv2/pwritev2` | flags 非 0 返回 `EOPNOTSUPP`；offset=-1 仍走共享 offset 的 readv/writev |
| `splice` | 非 pipe 一侧的显式 offset 使用 positioned I/O，并在成功后 copyout 新 offset |

所有用户 iovec 和 buffer 在 I/O 前完成基本可读/可写检查；offset 推进使用 checked-add。

## 3. sync 支持边界

| 接口 | Day2 等级 | 承诺范围 |
| --- | --- | --- |
| `File::fsync` | B | 写回当前 File page cache、同步缓存时间并执行挂载 superblock flush |
| `fdatasync` | B | 当前等价于更强的 fsync，不承诺与 fsync 区分 metadata |
| `Ext4SuperBlock::sync` | B | 调用 `ext4_cache_flush("/")`，失败返回 EIO，不 panic |
| ext4 shutdown | B | flush 成功后释放 wrapper；flush 失败时不关机并返回错误 |
| `sync_file_range` | B | 任意有效非零 flags 使用整文件 fsync，强于请求范围；flags=0 无操作 |
| `msync` | C | len=0 成功；非空已映射范围返回 `EOPNOTSUPP` |

`PageCache::sync` 原有 write-version 检查保持不变：写回快照完成后，只有版本未变化的 dirty page 才会被清理。

## 4. fd flag 归属

| 状态 | 当前真源 | dup/fork 后 | exec |
| --- | --- | --- | --- |
| CLOEXEC | `FdEntry.flags` | 每个 descriptor 独立 | 对应 entry 被关闭 |
| offset | File/open-file object | 共享 | 随 File 引用存活 |
| O_APPEND | FileOp status flags | 共享 | 随 File 引用存活 |
| O_NONBLOCK | FileOp status flags | 共享 | 随 File 引用存活 |
| O_DIRECT | FileOp status flags | 共享；具体直接 I/O 语义仍受限 | 随 File 引用存活 |
| access mode | FileOp 创建 flags | 共享且不可由 F_SETFL 修改 | 随 File 引用存活 |

本轮实现 `set_status_flags` 的对象：普通 File、memfd、pipe/FIFO、eventfd、timerfd、socket。`F_GETFL` 从 FileOp 读取并排除 CLOEXEC；`F_GETFD/F_SETFD` 仍只操作 FdEntry。

## 5. special-fd flags 收紧

- `timerfd_create(CLOCK_REALTIME_ALARM/CLOCK_BOOTTIME_ALARM)`：`EOPNOTSUPP`。
- `timerfd_settime(TFD_TIMER_CANCEL_ON_SET)`：`EOPNOTSUPP`。
- `memfd_create(MFD_HUGETLB)`：`EOPNOTSUPP`。
- 未同时提供 HUGETLB 的 huge-size selector：`EINVAL`。
- Day1 降级的 12 个空壳 fd syscall 继续固定返回 `ENOSYS`。

## 6. 回归

新增用户态回归 `fs_day2_io_regression`，覆盖：

1. dup 后 pread 不修改共享 offset；
2. dup 后 pwrite 不修改共享 offset；
3. fork 后 child pread 不修改 parent 共享 offset；
4. preadv 多 iovec 与 offset 保持；
5. pwritev 多 iovec 与 offset 保持；
6. positioned I/O 数据内容；
7. 负 offset 返回 EINVAL；
8. F_SETFL(O_APPEND) 经 dup 可见并影响写行为；
9. F_SETFD(CLOEXEC) 在两个 descriptor 间独立；
10. fsync、fdatasync 与 sync_file_range fallback；
11. alarm timerfd 和 HUGETLB memfd 明确拒绝；
12. close 一个/最后引用及文件清理。

实测结果：

| 架构 | build | QEMU 回归 |
| --- | --- | --- |
| RISC-V 64 | PASS | 全部检查 PASS，进程退出码 0 |
| LoongArch 64 | PASS | 全部检查 PASS，进程退出码 0 |

当前执行环境禁止交叉 C 编译器启动子进程，完整 make 首次在 lwext4 冗余重建处得到 `Bad system call`。验证时复用仓库已有、未修改的 `liblwext4-{arch}.a`，Rust 内核与用户程序仍完成双架构重新编译、链接和 QEMU 实机运行。正常开发环境无需该绕过。

## 7. 剩余边界

- `msync` 要提升为 B/A，需要 B 组提供不持 MemorySet 全局锁的 file-backed VMA 快照/写回接口。
- O_DIRECT 的对齐、绕过 cache 和 pipe packet mode 尚未成为完整承诺，后续 ABI 诚实化需继续收紧。
- epoll signal mask、copyout commit 和 fd reuse 属于 Day3 前的 P1 项，不在本次 positioned I/O 修改中混入。
- 本轮没有创建 git commit；代码可按 positioned I/O、sync、fd flags 三组变更拆分提交。
