# RespOS 四天内核重构：A/B/C 整合审查

## 1. 整合结果

整合目标为 `refactor/fs-consistency`，未修改 `main`。本次纳入：

- A：`origin/refactor/task-runtime`，通过 merge commit `2f736d4` 纳入；
- B：`origin/refactor/mm-contract`，通过 merge commit `e0d69fd` 纳入；
- C：文件系统与文件 ABI 提交 `cba8e24`；
- A/B/C 原始任务书、A 修复日志、B 重构汇报和 C Day1～Day4 记录。

合并顺序为 C → A → B。这个顺序便于先解决 runtime 与文件 ABI 的用户态封装重叠，再审查
MM 同 runtime/futex 和文件 mmap 的接口。

## 2. 冲突处理

| 冲突/重叠 | 处理结果 |
| --- | --- |
| `docs/RespOS一周内核语义训练项目.md` | 内容相同，仅文件 mode 不同；保留普通文档 mode |
| `docs/四天内核重构-C-文件系统.md` | A 分支携带的是未完成模板；保留 C 已验收版本 |
| `user/src/lib.rs` | 合并导出集合，同时保留 A 的 timer/resource/runtime 类型和 C 的 `IoVec`/文件 ABI |
| `user/src/syscall.rs` | Git 自动合并，静态复核 syscall number 与 wrapper 无覆盖 |
| `os/src/syscall/special_fd.rs` | A/C 修改可自动合并；保留 A 的 timer 生命周期与 C 的 epoll/eventfd/timerfd flags 语义 |
| `os/src/mm/mod.rs` | A/B 自动合并；保留 A 的 no-fault futex 读取接口与 B 的 copy/range/split 自检入口 |
| `os/src/syscall/net.rs` | B/C 自动合并；文件 status flags 与 MM 用户指针校验同时保留 |

## 3. 跨组审查结论

### A × B：task/futex 与 MM

B 汇报曾指出三个 A 侧风险，当前 A 分支均已闭环：

1. group exit 先统计共享 `MemorySet` 的组内 owner，只有地址空间确由该线程组独占时才回收；
2. `FUTEX_CMP_REQUEUE` 在 queue lock 外预触发用户页，锁内使用固定 4 字节 no-fault PTE 读取，
   不在全局 futex queue 临界区触发 lazy/COW 分配；
3. `wait4` 在 status/rusage copyout 全部成功后才提交 child ticks 和 zombie 删除，EFAULT 重试不会
   重复累计。

B 的 copyout-to-COW、相邻 VMA、范围溢出校验与 A 的失败注入探针能够同时编译运行。专项
`FUTEX_CMP_REQUEUE` 强制窗口在合并后的 RISC-V/LoongArch 上均通过。

### B × C：MM 与 file mmap

- B 的 VMA split 会同步调整 file backing offset/len，debug split self-test 在双架构启动时通过；
- C 的 `FileOp::mmap_allowed` 仍位于 B 的 `sys_mmap` 和 shared remap 检查链中；
- writable private 与 read-only shared 文件映射通过回归；
- writable `MAP_SHARED` 继续返回 `EOPNOTSUPP`，没有因 B 合并重新开放不安全能力；
- MM fault 读取 backing file 时仍可能持有 `MemorySet` 写锁；shared unmap 写回仍吞掉错误。
  因 writable shared 已被拒绝，这两个问题当前被隔离，但没有被宣称为实现完成；
- non-empty `msync` 继续返回 `EOPNOTSUPP`，truncate 与 read-only shared resident page 的完整
  失效契约仍待后续设计。

### A × C：runtime 与 fd/special-fd

- descriptor CLOEXEC 与 open-file status flags 的分层未被 A 的用户态扩展覆盖；
- A 的 POSIX timer owner-exit、clock domain 和 wait/futex 状态机与 C 的 epoll preview/commit、
  signal-mask 恢复及 fd reuse 回归可同时通过；
- B 汇报中的 signal/futex/wait4 风险不再是当前整合树的已知缺陷。

## 4. 验证矩阵

| 验证项 | RISC-V | LoongArch |
| --- | --- | --- |
| debug build | PASS | PASS |
| 启动时 MM split self-test / invariant | PASS | PASS |
| `fs_day4_freeze` | PASS | PASS |
| `fs_day3_regression` | PASS | PASS |
| `fs_day2_io_regression` | PASS | PASS |
| `task_a_atomic_probe` | PASS | PASS |
| `task_a_wait4_probe` | PASS | PASS |
| `task_a_clock_probe` | PASS | PASS |
| `task_a_futex_race_probe` | PASS | PASS |
| `task_a_futex_cmp_requeue_probe` 专项构建 | PASS | PASS |
| `pipetest` smoke | PASS | 未重复；A/B 原分支已有双架构证据 |

`task_a_futex_cmp_requeue_probe` 依赖内核以
`TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD=1` 构建；默认内核直接运行该探针不形成有效测试。专项验证后
已重新进行两架构默认 debug build，最终产物不带强制竞态窗口。

## 5. 审查结论与剩余风险

本次整合未发现阻止合并到个人工作分支的问题。A/B/C 的关键接口没有因自动合并丢失，双架构
构建和代表性 runtime/MM/FS 回归通过。

进入 `main` 前仍建议保留以下边界：

- 真正 SMP 下的 futex queue、页表替换和 scheduler contention 尚无本轮整合压力证据；
- `MAP_FIXED`/部分 `mremap` 的极端 ENOMEM 回滚仍缺完整失败注入；
- writable file `MAP_SHARED`、`msync/munmap` 错误传播和 truncate mapped-page 契约未完成；
- epoll 跨进程共享实例的最后 close 通知仍不是完整 Linux 语义；
- A 的性能数据表明正确性状态机使 thread create/join 和竞争 futex 有明显开销，后续优化不得
  回退 single-winner 与退出清理不变量；
- 合并前应在最终比赛镜像上再跑一次完整 LTP/目标 workload，而不仅是本次代表性回归。
