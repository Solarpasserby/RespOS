# 10. 测试、兼容性与性能

## 11.1 测试环境与可复现配置

【本节目的】固定最终结果的环境，避免不同镜像、QEMU、编译器和工作树混在一起。

【建议写什么】记录 host、Rust/nightly、target、QEMU、OpenSBI、架构、SMP、内存、kernel mode、user features、镜像 hash、最终 commit 和日期。

【建议检查的 RespOS 代码】`Makefile`；`os/Makefile`；`user/Makefile`；`.cargo/config*`；`docs/codex/workflows.md`。

【建议查看的 Git 历史】`94a2598`、`347414d`；最终提交前执行 `git status --short`。

【建议准备的图 / 表】测试配置卡片；RV/LA、debug/release、SMP/内存矩阵。

【建议准备的测试 / 数据】每个结论保存完整命令、日志路径、镜像恢复步骤和 `git diff --check` 结果。

【容易出现的问题】RV/LA 构建共用活动 config，不能并行；未跟踪 `user/src/bin/*.rs` 也会进入本地构建。

## 11.2 功能测试与比赛测例

【本节目的】展示测试覆盖的层次和判定规则，而非只给通过截图。

【建议写什么】分 basic/busybox/libc/LTP/CAgent/自研 probe；说明官方测例、聚焦 filter、脚本 setup、目标 assertion、退出码和日志 summary 的区别。

【建议检查的 RespOS 代码】`user/build.rs`；`user/src/bin/testrunner.rs`；`user/oscomp_ltp_list.txt`；`judge/ltp_report.py`；`judge/ltp_compare.py`。

【建议查看的 Git 历史】`8169793`、`269a94a`、`347414d`；对照 `contest-images-2026`。

【建议准备的图 / 表】测试层级→内核模块→测例表；首个框架失败与级联失败对照表。

【建议准备的测试 / 数据】完整/聚焦 LTP、CAgent 单项/全量、basic smoke；报告 pass/fail/TBROK/segfault 和首个不同失败。

【容易出现的问题】LTP harness 初始化失败会污染后续结论；`make` 成功和 QEMU exit 0 不是测试全通过。

## 11.3 SMP 与并发压力矩阵

【本节目的】证明哪些多核不变量在什么强度下被验证。

【建议写什么】列 SMP=1/2/4/8、debug/release、256M/8G、轮数、snapshot、测试对象（调度、wait4、futex、pipe、TCP、shared MM、exec/exit）。

【建议检查的 RespOS 代码】`user/src/bin/smp_phase3_probe.rs`；`smp_shared_mm_probe.rs`；task/mm/arch 相关实现。

【建议查看的 Git 历史】`dc793c4`、`17dcd4e`、`f326ac8`、`b785262`。

【建议准备的图 / 表】SMP 验收矩阵；每个 case 的不变量和日志前缀表。

【建议准备的测试 / 数据】Phase 3 30/30、shared MM 1000 轮、futex 20/20（若最终复现）、退出压力；任何首失败样本保留并说明。

【容易出现的问题】LA64 当前不是已验证 SMP；专项 yield build 与默认 build 的结果不可混写。

## 11.4 性能数据记录模板

【本节目的】确保“优化提升”有可比较数据。

【建议写什么】为每项优化填写优化前 commit、优化后 commit、环境、方法、预热、次数、中位数/离散度、前值、后值、百分比和回归结果。

【建议检查的 RespOS 代码】`docs/ltp-performance-optimization.md`；scheduler/timer/MM/FS 热路径；测试脚本。

【建议查看的 Git 历史】对应优化提交；不要只引用 `main` 或 README 历史数字。

【建议准备的图 / 表】前后对比表和误差线图；性能指标—可能瓶颈—回归测例表。

【建议准备的测试 / 数据】同机同镜像同 QEMU，至少 3 次；使用 guest `/proc/uptime` timed 区间；记录 host 调度限制。

【容易出现的问题】当前环境可能将 QEMU 置于 `SCHED_IDLE`，墙钟差异不能直接当 RespOS 性能；没有 profile 和前后数据不写“显著提升”。

## 11.5 结果、限制与可复现附件

【本节目的】为正文和 PDF 之外的审阅者提供复现入口。

【建议写什么】列最终结果摘要、原始日志命名规则、镜像恢复方法、judge 命令、已知 blocker 和“未测/待验证”列表。

【建议检查的 RespOS 代码】`scripts/`；`judge/`；`docs/codex/workflows.md`；`README.md`。

【建议查看的 Git 历史】最终 commit；将所有日志绑定到 commit/日期。

【建议准备的图 / 表】结果摘要表：结论｜命令｜环境｜日志｜状态｜限制。

【建议准备的测试 / 数据】最终双架构构建、关键 runtime、比赛测例和性能结果；保留原始 stdout/stderr。

【容易出现的问题】不要删除失败样本；不能从历史结果推断最终版本；镜像被污染后需从压缩基线恢复。
