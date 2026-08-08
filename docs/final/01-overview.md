# 1. 项目概述

> 本章建立读者对 RespOS 和决赛文档的最小共同背景；不在这里详细证明各模块实现。

## 1.1 项目定位与目标

【本节目的】说明 RespOS 面向的用户态工作负载、比赛赛道和工程目标。

【建议写什么】填写 Rust 教学/竞赛型内核、Linux ABI 兼容、RV64/LA64 双架构、QEMU 与比赛镜像边界；区分“项目目标”和“已完成能力”。

【建议检查的 RespOS 代码】`README.md`；`Makefile`；`os/src/main.rs`；`user/src/bin/testrunner.rs`。

【建议查看的 Git 历史】`3839e4e`、`9fa0110`、`bb917d3`；核对 README 中历史叙述是否仍适用于最终 commit。

【建议准备的图 / 表】项目目标—实现模块—验证方式三列映射表；不画泛化的“操作系统五层图”。

【建议准备的测试 / 数据】最终 commit、支持架构、镜像版本、构建命令、实际运行入口和当前已确认测例集合。

【容易出现的问题】不要把 README 的历史 LTP/分数描述写成当前成绩；不要把“支持某 syscall”写成“完整支持其 Linux 语义”。

## 1.2 比赛环境与用户态运行链

【本节目的】让读者知道内核如何从启动进入 initproc、testrunner 和比赛程序。

【建议写什么】只描述启动入口、镜像挂载、用户程序打包/扫描、日志输出和关机判定；详细调用链放到第 3 章，测试协议放到第 10 章。

【建议检查的 RespOS 代码】`os/src/main.rs`；`os/src/loader.rs`；`user/build.rs`；`user/src/bin/testrunner.rs`；顶层 `Makefile`。

【建议查看的 Git 历史】`8169793`、`269a94a`、`347414d`；确认测例入口和镜像脚本的变化。

【建议准备的图 / 表】“QEMU → kernel → initproc → testrunner → test script”流程图；架构/镜像/feature 矩阵。

【建议准备的测试 / 数据】RV/LA 各一份从构建到正常 guest quit 的 serial log，记录镜像 hash 和 QEMU 参数。

【容易出现的问题】`make rv/la` 返回 0 不代表用例通过；QEMU 运行时不能用 host `debugfs` 修改 raw 镜像。

## 1.3 设计原则与阅读指南

【本节目的】给出贯穿全文的判断标准，突出本队增量而非教材式背景。

【建议写什么】围绕公共逻辑复用、syscall 薄层、领域对象拥有状态机、失败原子性、诚实 errno、生命周期可证明和双架构验证组织条目。

【建议检查的 RespOS 代码】`docs/codex/decisions.md`；`os/src/syscall/`；`os/src/mm/`；`os/src/task/`；`os/src/fs/`。

【建议查看的 Git 历史】`00c6822`、`15fe1a5`、`3aa1fb5`、`cba8e24`；把设计原则与实际重构提交对应起来。

【建议准备的图 / 表】原则—对应模块—可观察证据表。

【建议准备的测试 / 数据】至少为每条原则指定一个失败路径或专项 probe，而非只放成功截图。

【容易出现的问题】不要把“代码结构看起来合理”当作测试证据；不确定的基础项目来源在第 15 章确认前写“待人工确认”。
