# RespOS 四天内核重构：C 组任务书（文件系统与文件 ABI）

> 负责人：C  
> 工作分支建议：`refactor/fs-consistency`  
> 总体方案：[四天内核重构总控与验收方案](./四天内核重构总控与验收方案.md)

## 1. 任务目标

C 负责 fd、打开文件对象、VFS/path、dentry/inode、page cache、ext4 后端及文件类 syscall。

四天内不重写 VFS，不替换 ext4，不追求完整权限/ACL。目标是：

1. 修复 positioned I/O 临时修改共享 offset 的明显错误；
2. 明确 fd flag 与 open-file status flag 的归属；
3. 消除核心 sync 路径的可达 panic 和假成功；
4. 收紧 create/unlink/rename 后的 dentry、inode 和 page cache 一致性；
5. 减少持有 spin lock 执行慢 I/O 的情况；
6. 用实测数据优化 read/write 和 page cache 热点。

## 2. 文件边界

### 主要负责

- `os/src/fs/`
- `os/src/syscall/fs.rs`
- 与文件描述符有关的 special fd 小范围代码
- ext4/lwext4 适配层
- 文件系统用户态回归

### 不主动修改

- `os/src/task/scheduler.rs`
- `os/src/task/futex/`
- `os/src/mm/memory_set.rs` 的 VMA/COW 主体；
- syscall 总分发表；
- 网络 socket 主体；
- 驱动层大重构。

文件 mmap 需要修改 MM 时，与 B 负责人定义接口；pipe/poll 阻塞需要修改 scheduler 时，与 A 负责人定义接口。

## 3. 必须保持的对象关系

```text
Process
  → FdTable
      → FdEntry
          → Arc<dyn FileOp>
              → File/open-file description
                  → Path(mount, dentry)
                  → Inode
                  → PageCache
```

### 3.1 fd 与 File

- fd 是表下标；
- CLOEXEC 等 descriptor flags 属于 `FdEntry`；
- offset、O_APPEND、O_NONBLOCK 等 status flags 属于共享打开文件对象；
- dup 复制 fd entry，但共享 File；
- fork 复制或共享 fd table 时，打开文件对象仍按 Linux 语义共享；
- close 只移除一个 fd 引用；
- positioned I/O 不修改 File 的共享 offset。

### 3.2 path、dentry 与 inode

- Path 必须包含 mount 和 dentry；
- rename/unlink 不应让已经打开的 File 突然失效；
- 同一路径连续 lookup 不应得到互相冲突的 inode 身份；
- unlink 后重新 create 同名文件不能复用错误旧 dentry；
- mount crossing 和 `..` 不得被 syscall 层绕开；
- syscall 层不得包含 LTP 路径前缀特判。

### 3.3 page cache

- cache page 的 dirty/version 状态一致；
- 慢 I/O 不长期持有全局 spin lock；
- 写回期间页面再次被修改时不能错误清 dirty；
- fsync 成功必须完成当前内核承诺范围内的写回；
- truncate 后 cache size 和后端 size 一致；
- read/write/mmap 的责任边界清楚。

## 4. 第一天：文件与 fd 型 ABI 审计

### 4.1 负责范围

C 对下面的 syscall 家族建立分级表和状态影响审计：

- 普通文件、目录、路径和 metadata syscall；
- fd table、fcntl、dup、close；
- pipe、poll、epoll；
- eventfd、timerfd、signalfd 等 special fd；
- mount 和新 mount API；
- 网络 syscall 只做静态分级，除 P0 外不纳入本轮重构。

任何返回 fd 的接口都要审查完整生命周期，而不是只检查创建成功。

### 4.2 空壳 fd 红名单

逐项确认以下接口是否只是分配通用 `SpecialFd`：

- [ ] `inotify_init1`
- [ ] `signalfd4`
- [ ] `pidfd_open`
- [ ] `fanotify_init`
- [ ] `userfaultfd`
- [ ] `perf_event_open`
- [ ] `io_uring_setup`
- [ ] `bpf`
- [ ] `fsopen`
- [ ] `fspick`
- [ ] `open_tree`
- [ ] `memfd_secret`

默认动作是返回 `ENOSYS`。只有同时具备 create、use、poll/ioctl、dup/fork、exec、close/exit 语义的接口才保留。

不能因为 LTP 只检查“返回了 fd”就评为实现。

### 4.3 epoll 和 special-fd 红名单

- [ ] `epoll_pwait` 是否忽略 signal mask 和 sigset size；
- [ ] 临时 signal mask 是否在等待期间原子替换并恢复；
- [ ] edge/oneshot 状态是否在 events copyout 失败前被消费；
- [ ] epoll interest 是 fd 数字、打开文件对象还是二者组合；
- [ ] close 后 fd 复用是否会错误 DEL/MOD 旧 interest；
- [ ] epoll 持锁注册底层 waiter 的锁顺序；
- [ ] timerfd 接受 `CANCEL_ON_SET` 后是否真的取消；
- [ ] alarm clock 是否具备对应语义；
- [ ] memfd_create 接受 HUGETLB flags 后是否真实支持；
- [ ] eventfd/epoll/timerfd 的 owner exit、close 和最后引用释放。

### 4.4 文件 ABI 红名单

- [ ] pread/pwrite/preadv/pwritev 是否临时修改共享 offset；
- [ ] splice 的显式 offset 是否也通过 seek/restore 模拟；
- [ ] fsync/fdatasync/sync_file_range/msync 是否返回假成功；
- [ ] F_SETFL 修改的是 fd entry 还是共享 open-file status；
- [ ] rename/unlink 失败是否已经污染 dentry/cache；
- [ ] mount/new mount API 是否只返回空壳对象；
- [ ] flags 是否因截断或默认分支被静默接受；
- [ ] 对错误 FileOp 类型是否返回正确 errno。

### 4.5 fd 型生命周期测试

每个保留的 fd 型 syscall 至少覆盖相关阶段：

```text
create
→ read/write/ioctl/mmap/poll
→ dup
→ fork
→ exec/CLOEXEC
→ close 一个引用
→ close 最后引用
→ owner exit
→ fd number reuse
```

epoll 额外测试：

- [ ] ADD → close target → fd number reuse；
- [ ] ONESHOT + events copyout EFAULT；
- [ ] ET 从 ready 到 not-ready 再 ready；
- [ ] pwait 有/无 signal mask；
- [ ] waiter、timeout、signal 竞争。

### 4.6 第一天交付

- [ ] FS/special-fd syscall 分级表；
- [ ] 空壳 fd 的批量降级清单；
- [ ] 5～10 份高风险状态影响审计；
- [ ] epoll 和 sync 的失败注入计划；
- [ ] fd/open-file identity 与生命周期图；
- [ ] 与 A/B 确认 waiter 和文件 mmap 接口。

## 5. 第二天：positioned I/O、sync 和 fd 语义

### 5.1 修复 pread/pwrite 系列

当前 `preadv/pwritev` 使用：

```text
保存共享 offset
→ seek 到指定 offset
→ 调用普通 read/write
→ 恢复 offset
```

并发共享同一 File 时，其他线程可能观察到临时 offset。

`File` 已有：

- `read_at_offset`
- `write_at_offset`

任务：

- [ ] `pread64` 使用 `read_at_offset`；
- [ ] `pwrite64` 使用 `write_at_offset`；
- [ ] `preadv` 使用局部 positioned offset；
- [ ] `pwritev` 使用局部 positioned offset；
- [ ] 不调用 `seek`；
- [ ] 不读取/恢复共享 offset；
- [ ] 每段后 checked-add 局部 offset；
- [ ] 保留短读、短写和部分成功语义；
- [ ] `preadv2/pwritev2` 的 `offset == -1` 仍走普通共享 offset；
- [ ] 不支持的 flags 返回明确错误。

必须测试：

- dup 两个 fd，共享 offset；
- fd1 执行 pread，fd2 offset 不变；
- fork 后 child pread，parent offset 不变；
- 多 iovec；
- 中途短读；
- 非法 fd、不可读/不可写 fd；
- offset 溢出。

### 5.2 处理 sync 的可达 panic

检查：

- `Ext4SuperBlock::sync`
- `File::fsync`
- `fdatasync`
- page cache flush
- shutdown 时 ext4 sync

禁止保留可达 `todo!()`。

处理策略按优先级：

1. 正确实现已支持 page cache 和 ext4 flush；
2. 若只能同步部分对象，明确实现边界；
3. 无法实现的对象返回错误；
4. 不得返回成功但不执行任何可观察同步。

### 5.3 fd flag 归属

制作表格并对照代码：

| 状态 | 应归属 | dup 后 |
| --- | --- | --- |
| CLOEXEC | FdEntry | 独立 |
| offset | File/open description | 共享 |
| O_APPEND | File/open description | 共享 |
| O_NONBLOCK | File/open description | 共享 |
| access mode | File/open description | 共享 |

重点检查：

- `FdTable::from_existed_user`
- `sys_dup/sys_dup3`
- `sys_fcntl`
- `close_on_exec`
- `FileInner.flags`

第二天至少修复会造成可观察错误的 flag，不必为了完美模型重写所有结构。

### 5.4 第二天交付

- [ ] pread/pwrite 不改变共享 offset；
- [ ] sync 路径不再 panic；
- [ ] fd/open-file flag 表完成；
- [ ] 6 个以上文件回归；
- [ ] 提交按“positioned I/O”“sync”“fd flag”拆分。

## 6. 第三天：锁、缓存和路径一致性

### 6.1 盘点持锁慢操作

重点搜索：

```text
lock()
  → inode.read_at/write_at
  → page cache flush
  → ext4
  → block device
```

对每处记录：

| 锁 | 锁内操作 | 是否可能 I/O/阻塞 | 修改方式 |
| --- | --- | --- | --- |
| FileInner | inode/page cache read | 是 | 快照后锁外执行 |
| Page | writeback | 是 | version + 锁外写回 |
| NAMEI mutation | create/rename/unlink | 可能 | 区分准备与后端提交 |

读路径优先采用：

```text
锁内快照 path/cache/flags
→ 释放锁
→ 执行 read/I/O
→ 必要时锁内提交 atime
```

写路径需要额外保证：

- append 的 offset/size 原子语义；
- truncate 与 write 不破坏 size；
- rename 后后端路径仍有效；
- dirty/version 不丢失。

不要为了缩短锁范围破坏这些语义。

### 6.2 dentry/inode 生命周期

固定测试序列：

1. create → lookup → open；
2. create → unlink → create same name；
3. rename old new → open new → open old；
4. open → unlink → 使用原 fd 读写；
5. create → close → reopen；
6. hard link → unlink 其中一个名字；
7. symlink follow/no-follow；
8. rename 覆盖已有目标；
9. 目录 rename 后 getcwd/path；
10. mount point 附近的 `..`。

检查点：

- dentry cache 何时失效；
- synthetic inode 是否泄漏到后续 lookup；
- inode number 是否稳定；
- page cache 是否绑错新文件；
- unlink/rename 后旧 Path 是否仍能支持打开 fd；
- 失败操作是否错误修改缓存树。

### 6.3 page cache 和 writeback

检查：

- [ ] dirty page 写回后只有版本未变化才清 dirty；
- [ ] writeback 失败保留 dirty；
- [ ] fsync 覆盖所有目标 dirty page；
- [ ] fdatasync 不承诺未实现的完整元数据语义；
- [ ] truncate 缩小时清理尾部 cache；
- [ ] 扩展文件时新区域读取为零；
- [ ] close 不错误丢弃仍共享的 cache；
- [ ] mmap 与 read/write 至少不会返回互相冲突的数据。

四天内不实现后台 writeback 守护线程，也不追求崩溃一致性。

### 6.4 read/write 性能

先基准，再决定是否处理 syscall 中转缓冲区。

记录：

- 4 KiB、64 KiB、1 MiB 顺序读写；
- 第一次读取和 cache hit；
- 重复 reopen；
- fsync 前后；
- iozone 选定项目。

低风险优化候选：

- 小块固定分片，减少反复堆分配；
- iovec 复用中转 buffer；
- 连续用户页批量 copy；
- page cache hit 避免不必要后端 stat/read；
- 缩短全局锁临界区。

禁止：

- 直接长期借用用户内存；
- 绕过 copyin/copyout 权限检查；
- fsync 返回前不完成承诺写回；
- 通过扩大全局锁掩盖竞态。

### 6.5 与 A、B 协作

与 A：

- pipe/poll 的 waiter 登记和 blocked 顺序；
- close pipe 端时唤醒读写者；
- signal 打断阻塞 I/O；
- O_NONBLOCK 与 EAGAIN；
- task exit 关闭 fd 时不能持有 scheduler 锁做 I/O。

与 B：

- 文件映射 backing offset；
- private/shared mmap；
- mmap fault 读取 page cache 的锁顺序；
- msync/munmap 的写回责任；
- truncate 与已映射页面。

### 6.6 第三天交付

- [ ] 至少修复一处明确的持 spin lock 慢 I/O；
- [ ] dentry 生命周期测试；
- [ ] page cache dirty/writeback 测试；
- [ ] 修改前后文件性能数据；
- [ ] 与 A/B 完成接口核对。

## 7. 第四天：交叉审查、ABI 收口与冻结

### 7.1 审查 B 的内存修改

重点检查：

- [ ] 文件映射切分后 backing offset；
- [ ] private/shared 语义；
- [ ] page fault 是否在持 MM 全局锁时做慢 I/O；
- [ ] munmap/msync 是否可能丢 dirty page；
- [ ] truncate 与已映射页面是否至少行为明确。

### 7.2 文件 ABI 诚实化

将本轮触及的 syscall 分为：

```text
A：核心语义有测试
B：仅支持明确子集
C：显式不支持
D：假成功，必须消除
```

优先整理：

- pread/pwrite/preadv/pwritev；
- fsync/fdatasync/sync_file_range；
- fcntl；
- renameat2；
- fallocate；
- chmod/chown；
- stat/statfs；
- ioctl 的文件相关分支。

规则：

- 不支持的 flags 不能静默忽略；
- 没有效果的操作不能轻易返回成功；
- 底层 ext4 特例不能放到 syscall 层；
- LTP 路径或程序名不能影响 syscall 结果。

### 7.3 第四天下午冻结

仅允许：

- P0/P1 修复；
- 编译和测试修复；
- helper 收敛；
- 错误注释修正；
- TODO 分类；
- 删除确定死代码。

禁止：

- VFS trait 全面改造；
- ext4 替换；
- dentry 树重写；
- page cache 新架构；
- syscall/fs 大规模拆文件；
- 新文件系统功能。

## 8. Codex 任务拆分建议

建议拆成：

1. 生成 FS/special-fd syscall 分级表和高风险状态影响表；
2. 批量识别并降级返回空对象的 fd 型伪实现；
3. 审查 epoll_pwait signal mask、epoll EFAULT 和 fd 复用语义；
4. 审查 timerfd/memfd 的 unsupported flags 和对象生命周期；
5. 将 pread/pwrite 系列改为真正 positioned I/O；
6. 审查并消除 ext4 sync 的 panic；
7. 核对 fd flags 与 File flags；
8. 盘点 File/page cache 持锁 I/O；
9. 修复一个明确的锁内 I/O 热点；
10. 审查 create/unlink/rename 的 cache 变更；
11. 编写 page cache writeback 竞争测试；
12. 建立文件 ABI 支持表。

每项明确禁止修改 scheduler、MemorySet 主体和 syscall 总分发表。

## 9. 止损条件

出现以下情况立即停止当前改造：

- 文件读写出现随机数据差异；
- fsync 后重启读取不到已承诺数据；
- unlink/rename 后出现旧内容污染新文件；
- 为减少锁而引入 append/truncate 错误；
- page cache dirty 状态无法解释；
- 文件测试必须增加延时才能通过；
- 性能优化绕过用户地址检查；
- 第四天下午仍在重写 VFS trait。

## 10. 最终验收清单

- [ ] positioned I/O 不修改共享 offset；
- [ ] FS/special-fd syscall 已完成分级；
- [ ] 空壳 fd 型 syscall 已降级或具备完整最小生命周期；
- [ ] epoll_pwait signal mask 不再被静默忽略；
- [ ] epoll EFAULT、close 和 fd 复用行为有回归；
- [ ] timerfd/memfd flags 与实际能力一致；
- [ ] dup/fork 后 fd 与 File 共享关系正确；
- [ ] sync 核心路径无可达 `todo!()`；
- [ ] 不支持的同步能力不假成功；
- [ ] create/unlink/rename 无明显旧缓存污染；
- [ ] 打开后 unlink 的 fd 生命周期合理；
- [ ] page cache dirty/version 语义稳定；
- [ ] 至少一处锁内 I/O 热点得到改善；
- [ ] 文件性能无明显回退；
- [ ] pipe/poll 与调度器协议核对完成；
- [ ] 文件 mmap 与 B 组核对完成；
- [ ] RV/LA 双架构通过；
- [ ] C 的提交可按语义独立回退。
