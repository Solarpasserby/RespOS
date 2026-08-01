# RespOS B 组四天内存管理重构汇报

## 一、总体结论

B 组四天任务的核心实现和双架构代表性验收已经完成，当前整体完成度约为 **85%～90%**。

本次工作没有重写 `MemorySet` 或页表框架，主要围绕用户地址检查、VMA 区间操作、COW、lazy allocation、mmap、page fault 和用户 copy 进行加固。

目前可以进入代码 review 和冻结阶段，但仍不能表述为“全部清单 100% 关闭”。尚未闭环的内容主要是跨组接口确认、极端失败原子性测试、完整性能计时、完整 LTP 和 Git 提交拆分。

## 二、第一天：MM/IPC ABI 审计

### 已完成

1. **shmat prepare/commit/rollback**
   - 地址、flags 和权限校验在共享状态修改前完成。
   - 映射成功后才更新 `atime/lpid`。
   - 映射失败时回滚 attach owner。
   - 支持并校验 `SHM_RDONLY/SHM_RND/SHM_REMAP/SHM_EXEC`。
   - 修复 LoongArch `SHM_RND` 错误使用 64 KiB 对齐的问题，统一为项目实际使用的 4 KiB 基页。

2. **madvise 分类处理**
   - 已实现 `DONTFORK/DOFORK`。
   - 已支持现有 `WIPEONFORK/KEEPONFORK/DONTNEED` 行为。
   - `POPULATE_READ/POPULATE_WRITE/GUARD_INSTALL/GUARD_REMOVE` 未实现时明确失败，不再假成功。
   - 纯性能 advice 在完整地址校验后采用受限 no-op。

3. **mlock 累计记账**
   - 使用“当前 locked bytes + 本次新增 locked bytes”检查 `RLIMIT_MEMLOCK`。
   - 重复锁同一范围不会重复计费。
   - `MAP_LOCKED` 也纳入相同累计限制。

4. **get_mempolicy 输出边界**
   - 明确为单节点受限实现。
   - 在任何 copyout 前限制 `maxnode`，避免用户控制的无界循环写入。

5. **mmap flags 和权限**
   - 普通 mmap 未知 flags 返回 `EINVAL`。
   - `MAP_SHARED_VALIDATE` 遇到未知 flags 返回 `EOPNOTSUPP`。
   - flags/prot 高位不再因转换成 `u32` 被静默截断。
   - 文件共享可写映射继续经过文件权限检查。

### 尚未闭环

- 尚未与 A/C 负责人实际确认共享 mm、futex key 和文件 backing 接口。
- `MAP_FIXED mmap` 与部分 `mremap` 极端分配失败路径尚未完成完整快照回滚测试。
- 审计表和 madvise 支持矩阵未单独写入仓库。

## 三、第二天：范围校验与 VMA 不变量

### 已完成

1. **统一地址范围 helper**
   - 覆盖 `munmap/mprotect/mremap/mlock/munlock/madvise`。
   - 检查 `addr + len`、页向上取整、地址对齐、空范围、固定地址及用户空间上界。

2. **VMA/PTE/Frame debug invariant**
   - VMA 有序、非空且互不重叠。
   - file backing offset/len 不发生明显溢出。
   - `data_frames` 中的 VPN 必须属于对应 VMA。
   - frame 必须对应有效 PTE。
   - 用户 PTE 必须带 USER。
   - VMA 与 PTE 的读写执行权限基本一致。
   - COW 不得同时带 WRITE，shared mapping 不得出现私有 COW。

3. **VMA split 启动自检**
   - 覆盖切头、切尾、中间切分和连续切分。
   - 覆盖匿名映射、非零文件 offset、短 backing。
   - 验证 `shared/locked/wipe_on_fork/dontfork/map_perm` 等字段不丢失。
   - 因内核为 `no_std`，测试改为 debug 内核启动自检，不依赖标准 Rust test harness。

4. **实际发现并修复 VMA 无序**
   - 原因是高地址 trampoline 先插入，低地址 ELF LOAD 段后插入。
   - 当前主要插入路径均保持 VMA 排序。

### 尚未闭环

- 尚未按“范围校验/invariant/split 修复”拆分 Git commit。
- VMA/PTE/Frame 关系图未单独保存到仓库。

## 四、第三天：COW、page fault 与用户指针

### 已完成

1. **COW 生命周期**
   - 私有可写页 fork 后父子共享 frame，并清 WRITE、设置 COW。
   - 只读代码页不添加 COW。
   - shared mapping 不走私有 COW。
   - 单引用 COW 恢复写权限。
   - 多引用 COW 分配新 frame 并复制数据。
   - 两架构增加一致的 `replace_pte`。

2. **COW 失败原子性**
   - 原实现先 unmap 旧页，再安装新页，失败可能留下地址空洞。
   - 当前改为先分配、复制，再原子替换已有叶 PTE，最后更新 `data_frames`。
   - ENOMEM 不再先破坏旧映射。

3. **page fault 分类**
   - null、用户空间上界外、VMA 不存在或权限不符均返回错误。
   - 合法 COW write 进入恢复写或复制路径。
   - 合法 lazy/file page 进行分配和 backing 填充。
   - 已有 PTE 仍异常时返回错误，不按普通 lazy fault 处理。

4. **用户 copy**
   - 支持跨页和跨相邻 VMA。
   - copyin 只要求读权限，copyout 要求写权限。
   - copyout 可以触发 lazy/COW fault。
   - 检查 null、末端溢出、类型长度乘法、页向上取整和 `isize::MAX`。
   - 修复字符串指针递增溢出。
   - 补充 iovec 总长度限制。

### 尚未闭环

- 尚未逐项运行父写子读、子写父读、多次 fork 同一 COW 页、copyout 到 COW 页等全部定制测试。
- 文件非零 offset fault 和用户 buffer 尾地址溢出缺少独立运行用例。

## 五、第四天：交叉审查、性能与冻结

### 双架构验证

以下组合全部编译通过：

- RISC-V debug
- RISC-V release
- LoongArch debug
- LoongArch release

两个架构均能启动到用户 shell，debug split 自检和 VMA invariant 均通过。

### 代表性 LTP

RISC-V 和 LoongArch 均通过：

| 用例 | 结果 |
|---|---|
| fork01 | 2 passed |
| mmap01 | passed |
| mprotect05 | 1 passed |
| mremap05 | 7 passed |
| madvise01 | 16 passed，4 个明确 unsupported |
| mlock01 | 4 passed |
| shmat01 | 4 passed |

### 固定内存回归

在 128 MiB 内存下：

- 初始 `free_kb=185336`
- 12 次 fork/exit 后 `free_kb=184816`
- 下降约 520 KiB，接近设计中的 128 页页表延迟回收隔离上限
- tasks 最终恢复为 3
- 多次 `pipetest` 均通过

没有发现任务数量持续增加或明显的无限 frame 泄漏。

### 交叉审查发现的 A 组风险

1. **signal group exit 可能提前回收共享地址空间**
   - 普通 group exit 会在 `MemorySet` 唯一持有时才回收。
   - signal group exit 当前无条件执行 `recycle_data_pages()`。
   - 跨进程 `CLONE_VM` 时可能影响仍在使用同一地址空间的任务。

2. **futex queue lock 内进入用户 copy/MM fault**
   - futex wait 持有全局 queue lock 后调用 `copy_from_user`。
   - copy 可能获取 MemorySet 写锁、处理 lazy/COW fault 或分配 frame。
   - 存在复杂锁序和长临界区风险。

3. **wait4 rusage copyout 失败原子性**
   - child 状态在 copyout 成功后才回收，这部分正确。
   - 但 child ticks 在 rusage copyout 前已经累计。
   - rusage copyout 失败后重试可能重复累计 ticks。

上述问题属于 `task/futex` 主体，按任务书边界应交由 A 负责人处理。

## 六、总体评价

本次工作可评价为 **8.5/10**：

### 做得较好的部分

- 没有重写核心 MemorySet 或页表框架。
- 不仅通过编译，还实际启动并测试了两个架构。
- invariant 实际发现并修复 ELF VMA 无序问题。
- LoongArch LTP 实际发现并修复 SHMLBA 对齐问题。
- COW 失败路径改为原子替换 PTE。
- 用户 copy 的跨页、lazy/COW、权限和溢出边界较完整。
- 两架构代表性 MM/IPC LTP 行为一致。

### 仍需完成

- A/C 跨组接口确认和问题闭环。
- `MAP_FIXED/mremap` 极端失败回滚验证。
- 完整定制 COW 和非法用户指针测试。
- fork+COW、首次 fault、大 buffer copy 的精确时间数据。
- 完整 LTP。
- 按语义拆分 Git commit。

## 七、汇报话术

> B 组四天内存管理任务的核心改造已经完成，当前整体完成度约 85%～90%。
>
> 第一天完成了 MM/IPC 高风险审计和主要修复，包括 shmat prepare/commit/rollback、madvise 状态型 advice 分类、mlock 和 MAP_LOCKED 累计记账、get_mempolicy 输出上限以及 mmap 未知 flags 检查。
>
> 第二天完成了统一用户页范围 helper、debug VMA/PTE/Frame invariant 和启动时 VMA split 自检。Invariant 实际发现并修复了 ELF trampoline 导致 VMA 无序的问题。
>
> 第三天完成了 COW、lazy fault 和用户 copy 边界加固。多引用 COW 改为先分配复制，再原子替换 PTE，ENOMEM 不会破坏旧映射；copyin/copyout 支持跨页、相邻 VMA、lazy/COW，并检查主要溢出。
>
> 第四天完成了 RISC-V 和 LoongArch 双架构 debug/release 构建、启动验证和代表性 MM/IPC LTP。fork、mmap、mprotect、mremap、madvise、mlock、shmat 在两架构均通过。
>
> 交叉审查发现三个需要 A 组处理的问题：signal group exit 可能无条件回收共享 MemorySet；futex queue lock 内进入 copy_from_user/MM fault；wait4 的 rusage copyout 失败可能重复累计 child ticks。
>
> 当前剩余工作是 A/C 接口确认、极端失败回滚验证、完整性能计时、完整 LTP 和提交整理。核心代码已经进入冻结和 review 阶段。
