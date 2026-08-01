# RespOS 四天内核重构：B 组任务书（内存管理）

> 负责人：B  
> 工作分支建议：`refactor/mm-contract`  
> 总体方案：[四天内核重构总控与验收方案](./四天内核重构总控与验收方案.md)

## 1. 任务目标

B 负责用户地址检查、VMA 区间操作、COW、lazy allocation、mmap 和 page fault。

四天内不重写整个 `MemorySet`，不设计新的页表框架。目标是：

1. 统一 mmap 系列 syscall 的地址范围校验；
2. 为 VMA 和物理页关系增加可执行不变量；
3. 保证区间切分不丢失文件映射等元数据；
4. 审查 fork/COW/lazy fault 的权限与生命周期；
5. 确保用户指针错误不会导致内核 panic；
6. 在不牺牲正确性的前提下减少重复翻译和无意义分配。

## 2. 文件边界

### 主要负责

- `os/src/mm/`
- `os/src/syscall/mm.rs`
- `os/src/arch/rv64/mm/page_table.rs`
- `os/src/arch/loongarch64/mm/page_table.rs`
- 与用户 copy 直接相关的 syscall helper

### 不主动修改

- `os/src/task/task.rs` 的进程生命周期；
- `os/src/task/futex/` 的 waiter 状态机；
- `os/src/fs/` 的 page cache 和 VFS；
- syscall 总分发表；
- 驱动和网络。

文件 mmap 需要新的 FS 接口时，先与 C 负责人确定语义，不直接修改 ext4/page cache。

## 3. 必须保持的不变量

### 3.1 VMA

- 所有区间采用 `[start, end)`；
- `start < end`；
- `areas` 有序且互不重叠；
- 用户映射不越过用户地址上界；
- 区间计算不能发生加法或向上取整溢出；
- split 后 `shared/locked/wipe_on_fork/map_perm/map_type` 不丢失；
- file backing 的 offset 和 len 随子区间正确变化；
- VMA 修改失败不能留下半完成状态。

### 3.2 VMA、PTE 与 Frame

```text
VMA：声明地址合法性和逻辑权限
PTE：CPU 实际使用的映射和权限
Frame：物理页生命周期
```

- present PTE 必须属于合法 VMA；
- `data_frames` 中记录的 vpn 必须属于对应 VMA；
- lazy VMA 可以没有 present PTE；
- COW 页不可同时作为普通私有可写页；
- 修改 PTE 权限后必须按架构要求刷新 TLB；
- unmap 后不能保留可访问的旧 TLB 映射；
- 物理页引用计数必须覆盖所有共享者。

### 3.3 用户指针

- null、溢出、越界、权限错误返回 `EFAULT` 或规定错误；
- 允许跨页；
- 允许跨相邻合法 VMA；
- copyin 只要求读权限；
- copyout 要求写权限，并能触发 lazy/COW fault；
- 内核不能直接解引用尚未映射的用户 VA；
- 长度为零的 ABI 行为应在 syscall 层明确处理。

## 4. 第一天：内存与 IPC ABI 审计

### 4.1 负责范围

B 对下面的 syscall 家族建立分级表和状态影响审计：

- brk、mmap、munmap、mremap、mprotect；
- mlock、munlock、madvise、get_mempolicy；
- copyin/copyout 使用方式；
- SysV shm；
- 文件映射与 MM 直接相关的部分。

重点不是枚举参数，而是确定每个接口修改 VMA、PTE、Frame、共享段表还是 task-local 状态，以及失败发生在哪个提交点。

### 4.2 首批红名单

- [ ] `shmat` 是否在地址和 flags 校验完成前修改 atime/lpid/attach 表；
- [ ] shmat 映射失败后 attach id、时间和 owner 状态是否完整回滚；
- [ ] `madvise` 对 DONTFORK/DOFORK/POPULATE/GUARD 等状态型 advice 是否假成功；
- [ ] `mlock` 是否只检查单次长度而没有累计 locked bytes；
- [ ] `get_mempolicy` 是否按用户 maxnode 无界 copyout；
- [ ] 单节点受限语义是否被错误宣传为完整 NUMA；
- [ ] mmap 未知 flags 是否因截断而被静默接受；
- [ ] mmap 文件权限与 MAP_SHARED/PRIVATE、PROT_WRITE 的组合；
- [ ] mremap/mprotect 失败是否留下部分 VMA/PTE 修改；
- [ ] 用户 copy 失败是否发生在共享状态提交之后。

### 4.3 advice 分级

`madvise` 必须按单个 advice 分类，不能把“多数是 hint”当作统一 no-op 理由：

```text
允许受限 no-op：
NORMAL/RANDOM/SEQUENTIAL 等纯性能建议

必须实现或拒绝：
DONTFORK/DOFORK
POPULATE_READ/POPULATE_WRITE
GUARD_INSTALL/GUARD_REMOVE
任何改变 fork、fault 或可访问性的 advice
```

对允许 no-op 的 advice 仍需验证完整地址范围。对改变可观察行为的 advice，返回成功后必须真的影响后续 fork/fault。

### 4.4 失败原子性测试

- [ ] shmat 地址非法后 attach 表和 segment 元数据不变；
- [ ] shmat 映射失败后 frame/attach id 不泄漏；
- [ ] mmap/mremap 失败后 VMA 集合快照不变；
- [ ] mprotect 覆盖部分非法范围时不产生半修改；
- [ ] mlock 多次调用的累计 limit；
- [ ] get_mempolicy 超大 maxnode 不造成长时间循环或越界；
- [ ] copyout 跨页失败后共享状态符合明确约定。

### 4.5 第一天交付

- [ ] MM/IPC syscall 分级表；
- [ ] 5～10 份高风险状态影响审计；
- [ ] 每个 madvise advice 的支持结论；
- [ ] shmat 的 prepare/commit/rollback 设计；
- [ ] mlock 累计记账方案或明确受限结论；
- [ ] 与 A/C 确认共享 mm、futex key 和文件 backing 接口。

## 5. 第二天：范围校验与 VMA 不变量

### 5.1 统一地址范围转换

为 mmap 系列提供统一 helper，覆盖：

- `addr + len`；
- `len` 页向上取整；
- addr 对齐要求；
- 用户空间上下界；
- 空范围；
- 固定映射特殊要求。

建议接口形式：

```rust
enum AddressAlignment {
    PageAligned,
    Any,
}

fn checked_user_page_range(
    addr: usize,
    len: usize,
    alignment: AddressAlignment,
) -> SysResult<VPNRange>
```

应用到：

- [ ] `munmap`
- [ ] `mprotect`
- [ ] `mlock`
- [ ] `munlock`
- [ ] `madvise`
- [ ] `mremap` 中适用的范围

`mmap` 参数组合更复杂，第二天只复用基础范围检查，不强制完全改写。

错误码必须按接口分别处理，不能为了复用 helper 把所有错误都改成同一个 errno。

### 5.2 MemorySet invariant

实现 debug-only 检查，至少包括：

- [ ] VMA 有序；
- [ ] VMA 不重叠；
- [ ] 每个 VMA 非空；
- [ ] file backing offset/len 无明显溢出；
- [ ] `data_frames` 的 vpn 位于 VMA 内；
- [ ] 用户区 VMA 带 USER；
- [ ] 页表中已记录的用户页与 VMA 权限基本一致；
- [ ] COW 与 WRITE 组合合法。

调用位置建议：

- mmap 成功后；
- munmap 成功后；
- mprotect 成功后；
- mremap 成功后；
- fork/COW 构造后；
- debug page fault 处理后。

如果遍历页表成本高，只在专门 debug feature 或关键操作后开启。

### 5.3 VMA split 测试

针对 `split_by_overlap` 和 file metadata：

- [ ] 完整覆盖；
- [ ] 切头；
- [ ] 切尾；
- [ ] 从中间切出一段；
- [ ] 连续两次切分；
- [ ] 匿名映射；
- [ ] 文件映射；
- [ ] 非零文件 offset；
- [ ] backing len 小于 VMA 长度；
- [ ] locked/wipe/shared 等字段保留。

建议优先把纯区间逻辑做成宿主机可测模型，避免每个边界都启动 QEMU。

### 5.4 第二天交付

- [ ] 统一范围 helper；
- [ ] VMA invariant；
- [ ] VMA split 测试；
- [ ] 一张 VMA/PTE/Frame 关系图；
- [ ] 提交按“范围校验”“invariant”“split 修复”拆分。

## 6. 第三天：COW、lazy fault 与用户指针

### 6.1 fork/COW 审查

沿着下面调用链检查：

```text
clone/fork
  → MemorySet::from_existed_user
  → 父子共享 frame
  → 私有可写页设置 COW 并清 WRITE
  → TLB 刷新
  → write fault
  → 单引用恢复写 / 多引用复制
```

检查清单：

- [ ] 父进程 PTE 被正确改成只读 COW；
- [ ] 子进程 PTE 与父进程一致；
- [ ] 只读代码页不会错误添加 COW；
- [ ] shared mapping 不走私有 COW；
- [ ] 单引用 COW 恢复写权限；
- [ ] 多引用 COW 分配新 frame 并复制数据；
- [ ] frame 分配失败不破坏旧映射；
- [ ] fork 中途失败时父进程仍可继续运行；
- [ ] 两架构 COW/PTE 标志语义一致；
- [ ] 权限变化后执行正确 TLB 刷新。

不要仅使用 `Arc::strong_count` 推断所有语义；需要确认引用来源是否只代表真实映射。

### 6.2 page fault 分类

将 fault 的决策路径整理为：

```text
地址不在用户 VMA            → fault/error
访问类型违反 VMA 权限       → fault/error
合法 lazy page              → 分配并映射
合法 COW write              → 恢复写或复制
合法文件映射缺页            → 从 backing 填充
已映射但仍触发异常          → 权限/架构错误
```

要求：

- [ ] 不使用 panic 处理普通用户 fault；
- [ ] 不把所有错误统一成一个无信息返回；
- [ ] 先验证 VMA 权限，再补页；
- [ ] 文件 fault 不在持有不必要的全局 MM 锁时做慢 I/O；
- [ ] 失败时不安装半初始化 PTE。

### 6.3 用户 copy

覆盖：

- [ ] `copy_from_user`
- [ ] `copy_to_user`
- [ ] `copy_cstr_from_user`
- [ ] 用户结构体数组；
- [ ] iovec 长度乘法；
- [ ] 跨页字符串；
- [ ] 跨相邻 VMA；
- [ ] lazy 页；
- [ ] COW 页；
- [ ] 读写权限不匹配；
- [ ] 地址空间末端溢出。

安全目标：

```text
任何用户可控指针
  → 最多导致 syscall 返回错误或用户任务收到 fault
  → 不能导致内核越界、unwrap 或 panic
```

### 6.4 与 A、C 的接口协作

与 A：

- futex 值读取必须是可靠的 32 位用户读取；
- futex key 对 shared mapping 必须稳定；
- task exit 不能提前回收仍共享的 `MemorySet`；
- wait/signal copyout 失败必须保持对象状态。

与 C：

- 文件映射 split 后 offset 保持正确；
- private file mapping 与 shared mapping 行为区分；
- page fault 读取 backing 的锁顺序明确；
- msync/munmap 与 page cache 的责任边界明确。

### 6.5 第三天测试

- [ ] fork 后父写、子读；
- [ ] fork 后子写、父读；
- [ ] 父子先后写同一页；
- [ ] 多次 fork 共享同一页；
- [ ] lazy 匿名页首次读写；
- [ ] mprotect 去掉写权限后写入；
- [ ] partial munmap 后访问保留区与删除区；
- [ ] 文件映射非零 offset；
- [ ] copyout 到 COW 页；
- [ ] 用户 buffer 跨页；
- [ ] 用户 buffer 尾地址溢出；
- [ ] RV/LA 相同行为。

### 6.6 第三天交付

- [ ] COW 生命周期得到回归保护；
- [ ] fault 分类逻辑清晰；
- [ ] 用户指针边界统一；
- [ ] 与 A/C 完成接口核对；
- [ ] 不新增直接用户指针解引用。

## 7. 第四天：交叉审查、性能与冻结

### 7.1 审查 A 的进程/同步修改

重点回答：

- [ ] 进程退出是否可能回收仍共享的地址空间？
- [ ] robust futex 是否在 mm 可访问时处理？
- [ ] futex 是否持有 queue lock 进入可能复杂的 MM fault？
- [ ] futex shared key 在 fork/shared mmap 后是否一致？
- [ ] wait/signal 的 copyout 失败是否改变进程状态？

### 7.2 内存性能

仅测量后优化：

- fork+COW 时间；
- 首次 page fault 时间；
- 大 buffer copyin/copyout；
- mmap/munmap 大量小区间；
- 测试前后 free frame 数。

可考虑的低风险优化：

- 一次获得 MemorySet 锁后按页批量翻译；
- 避免同一页重复权限检查；
- 避免零长度操作进入完整页表流程；
- invariant 仅在 debug 启用；
- 对连续页减少重复元数据查找。

不允许：

- 绕过用户权限检查；
- 缓存裸 PTE/裸指针跨越页表修改；
- 为性能关闭必要 TLB 刷新；
- 依赖测试恰好不触发的地址范围。

### 7.3 第四天下午冻结

仅允许：

- P0/P1 修复；
- 编译修复；
- 小测试修复；
- 注释和 TODO 分类；
- 删除已确认死代码。

不再进行：

- VMA 容器整体替换；
- 页表 trait 重写；
- 新内存回收算法；
- 大规模文件拆分；
- 新 mmap flag 支持。

## 8. Codex 任务拆分建议

拆成独立任务：

1. 生成 MM/IPC syscall 分级表和高风险状态影响表；
2. 审查 shmat 的 prepare/commit、copyout 和失败回滚；
3. 按 advice 审查 madvise，禁止用一个成功返回代表全部语义；
4. 审查 mlock 累计额度与 get_mempolicy 输出边界；
5. 盘点 mmap 系 syscall 的地址范围校验差异；
6. 实现统一 checked range helper；
7. 为 MemorySet 增加 debug invariant；
8. 为 `split_by_overlap` 编写表驱动测试；
9. 审查 fork/COW 权限和回滚；
10. 审查 page fault 分类；
11. 审查 copyin/copyout 跨页、溢出和 COW；
12. 比较 RV/LA PTE 行为。

每个 Codex 任务必须禁止修改 task/fs 主体，避免“顺手修复”越界。

## 9. 止损条件

出现以下情况停止当前改造：

- 新增随机 page fault；
- 只有关闭 COW 才能运行；
- 父子进程互相看到私有写入；
- frame 数持续下降且无法解释；
- 必须取消 TLB flush 才能通过；
- mmap 失败后地址空间处于半修改状态；
- RV 能运行但 LA 出现权限异常；
- 第四天下午仍在改 VMA 数据结构。

## 10. 最终验收清单

- [ ] mmap 系列地址计算无 unchecked overflow；
- [ ] MM/IPC syscall 已分级并记录共享状态；
- [ ] shmat 的失败路径不污染 attach 表和 segment 元数据；
- [ ] 状态型 madvise advice 已实现或明确拒绝；
- [ ] mlock 累计资源限制有明确语义；
- [ ] get_mempolicy 不接受无界输出规模；
- [ ] VMA 有序、无重叠；
- [ ] split 不丢元数据；
- [ ] 父子 COW 权限和 frame 生命周期正确；
- [ ] lazy fault 与非法 fault 明确区分；
- [ ] copyin/copyout 跨页和 COW 正确；
- [ ] 用户非法地址不会 panic 内核；
- [ ] 文件映射 offset 在切分后正确；
- [ ] RV/LA 双架构通过；
- [ ] 固定内存回归连续运行；
- [ ] 修改前后性能和 free frame 数据已记录；
- [ ] B 的提交可按语义独立回退。
