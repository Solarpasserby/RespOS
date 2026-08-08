# 9. 架构支持、SMP 与驱动

## 9.1 双架构公共层与差异收敛

【本节目的】说明 RV64 与 LA64 如何共享公共内核逻辑，差异被限制在哪里。

【建议写什么】填写目标 triple、入口、linker、高半区、分页、trap、timer、context switch、设备形式；说明公共 API 由 `arch/mod.rs` 重导出。

【建议检查的 RespOS 代码】`os/src/arch/mod.rs`；`arch/rv64/`；`arch/loongarch64/`；`linker_*.ld`；Cargo 配置和 `Makefile`。

【建议查看的 Git 历史】`main` 到 `dev` 的 arch 相关 commits；`f326ac8`、`dc793c4`；使用 `git log -- os/src/arch`。

【建议准备的图 / 表】公共 HAL—架构实现图；RV64/LA64 差异矩阵。

【建议准备的测试 / 数据】`make rv`、`make la`、两架构 boot/shutdown 和关键专项结果。

【容易出现的问题】RV64 SMP 结论不能外推到 LA64；两个架构构建共享活动 Cargo config，不能并行覆盖。

## 9.2 trap、特权级与 context return

【本节目的】解释 trap entry、kernel/user 返回、寄存器上下文和架构安全边界。

【建议写什么】描述 trap frame、`stvec`/`sscratch`/`sstatus`、`sret`、LA64 对应路径；突出 RV64 在 `__restore` 中先清 SIE、保持 kernel vector、最后切 user vector 的顺序。

【建议检查的 RespOS 代码】`os/src/arch/rv64/trap/{context.rs,trap.S,mod.rs}`；LA64 对应 `trap/`；`os/src/signal/`。

【建议查看的 Git 历史】`f326ac8`、`b785262`；对照 `/tmp/respos-rustc-pc-sample*` 与 postfix 快照。

【建议准备的图 / 表】user→trap→kernel→restore→sret 流程；TrapContext 字段和架构差异表。

【建议准备的测试 / 数据】非法指令、page fault、timer、signal return、GDB CSR/PC 快照；记录未出现递归 StorePageFault 的证据。

【容易出现的问题】不能原样信任来自 exec/signal 的 SIE；trap 修复后的 BuildStorm 静默边界仍可能是独立 sleep/wakeup 问题。

## 9.3 RV64 SMP、per-CPU 与 IPI

【本节目的】说明决赛新增的多核启动、每核状态、调度唤醒和硬件/固件依赖。

【建议写什么】覆盖 hart 启动、per-CPU processor/idle/timer、SBI HSM/IPI、owner handoff、affinity、boot barrier、LA64 当前单核边界。

【建议检查的 RespOS 代码】`os/src/arch/rv64/{smp.rs,sbi.rs,trap/mod.rs,entry/}`；`os/src/task/processor.rs`；`os/src/fs/proc/cpuinfo.rs`。

【建议查看的 Git 历史】`dc793c4`、`17dcd4e`、`f326ac8`；`docs/codex/buildstorm-smp-plan.md` 的 phase 记录。

【建议准备的图 / 表】boot hart/secondary hart 生命周期；per-CPU 状态表；IPI 目标选择流程。

【建议准备的测试 / 数据】`-smp 2/4/8`、nproc/procfs、Phase 3、affinity smoke、SMP 退出压力；按 release/debug 分列。

【容易出现的问题】当前完整 BuildStorm 未通过，不能写“题二已完成”；OpenSBI RFENCE completion 不能外推到未知 firmware。

## 9.4 MemorySet active mask 与 TLB shootdown

【本节目的】解释共享地址空间在多核上何时可修改/回收页表。

【建议写什么】写 active hart bit 发布/清除时机、MemorySet 读写锁、local `sfence.vma`、remote SBI RFENCE、exec/clone 临时 activate 的撤销。

【建议检查的 RespOS 代码】`os/src/mm/memory_set.rs`；`os/src/task/processor.rs`；`os/src/task/task.rs`；`os/src/arch/rv64/sbi.rs`。

【建议查看的 Git 历史】`17dcd4e`、`f326ac8`；关联 active-mask/shared-MM 日志。

【建议准备的图 / 表】页表修改—fence—remote ack—frame 回收时序；active mask 状态转移表。

【建议准备的测试 / 数据】`smp_shared_mm_probe` 2/8 核、remap 1000 轮、Phase 3 30/30；记录 firmware 版本。

【容易出现的问题】不能在旧 satp 仍运行时提前清 bit；不能把 `Arc` 引用数当 TLB 安全证明。

## 9.5 virtio、块/网卡与设备文件

【本节目的】说明设备抽象、RV MMIO/LA PCI 差异和 I/O 后端如何接入 VFS/net。

【建议写什么】覆盖 `Device`/disk、virtio block/net、driver config、devfs 设备节点、block flush 和 loopback；区分代码支持与当前实测覆盖。

【建议检查的 RespOS 代码】`os/src/drivers/{device.rs,disk.rs,virtio/}`；`arch/*/config/driver.rs`；`os/src/fs/dev/`；`os/src/net/loopback.rs`。

【建议查看的 Git 历史】`269a94a`、`5f77068`、`40d745a`；`vendor/` 依赖变更。

【建议准备的图 / 表】用户 I/O→VFS/net→driver→QEMU device 图；RV MMIO/LA PCI 表。

【建议准备的测试 / 数据】文件读写/fsync、网络 loopback、设备节点、正常卸载和 flush 证据。

【容易出现的问题】驱动能启动不等于所有错误/flush 语义已验证；镜像持久化必须和 guest 正常关机流程一起记录。

