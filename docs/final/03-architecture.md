# 3. RespOS 总体架构

## 3.1 启动、初始化与用户态进入

【本节目的】解释从架构入口到第一个用户任务的真实调用链及初始化依赖。

【建议写什么】按“入口/linker → `rust_main`/高半区 → trap → MM → net → initproc → timer → scheduler”逐步填写；标出 LA64 早期分页和 RV64 FDT 内存识别差异。

【建议检查的 RespOS 代码】`os/src/main.rs`；`os/src/arch/*/entry/`；`os/src/linker_*.ld`；`os/src/mm/mod.rs`；`os/src/net/mod.rs`。

【建议查看的 Git 历史】`dc793c4`、`f326ac8`；`git blame os/src/main.rs`；核对初始化顺序变更。

【建议准备的图 / 表】启动流程图；子系统初始化前置条件表；RV/LA 入口差异表。

【建议准备的测试 / 数据】两架构 build、boot、进入 shell/testrunner、正常 quit 日志。

【容易出现的问题】不能交换依赖 allocator、页表、timer 的初始化；不能把某一次 QEMU 启动日志写成两架构共同事实。

## 3.2 内核分层与模块边界

【本节目的】说明 syscall、task、MM、FS、net、arch 之间谁拥有状态机。

【建议写什么】围绕“syscall 解析 ABI，领域模块维护状态”的边界展开；列出用户态、公共内核、架构/HAL、驱动、文件后端和测试工具的职责。

【建议检查的 RespOS 代码】`os/src/syscall/`；`os/src/task/`；`os/src/mm/`；`os/src/fs/`；`os/src/net/`；`os/src/arch/mod.rs`。

【建议查看的 Git 历史】`15fe1a5`、`3aa1fb5`、`cba8e24`；关联 `docs/codex/architecture.md` 的边界表。

【建议准备的图 / 表】模块依赖图；边界—所有者—禁止下沉逻辑表。

【建议准备的测试 / 数据】为至少 3 个跨层路径列调用链：`mmap`、`exec`、`open/read` 或 socket。

【容易出现的问题】不要在每章重复完整分层图；syscall 文件中的入口函数不等于底层对象的所有者。

## 3.3 跨模块不变量与失败原子性

【本节目的】给后续技术章节提供统一的并发、生命周期、错误传播和资源提交标准。

【建议写什么】整理 VMA/PTE、Task owner、fd/open-file、锁外 I/O、prepare/copyout/commit、deferred cleanup 等不变量；每条标明适用范围。

【建议检查的 RespOS 代码】`docs/codex/architecture.md`、`decisions.md`、`pitfalls.md`；对应 `memory_set.rs`、`task.rs`、`fdtable.rs`。

【建议查看的 Git 历史】`3aa1fb5`、`15fe1a5`、`cba8e24`、`b785262`。

【建议准备的图 / 表】不变量—破坏症状—检查点—验证命令表。

【建议准备的测试 / 数据】debug invariant、专项 probe、失败 errno 和日志中的关键 trace。

【容易出现的问题】不能把历史修复后的规则倒推成原始实现；不能把待验证的 firmware 行为外推为所有平台行为。

