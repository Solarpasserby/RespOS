# 12. 初赛到决赛的开发过程与版本演进

## 13.1 初赛版本形成

【本节目的】复原初赛文档、初赛提交和当时功能边界。

【建议写什么】填写初赛目标、三人分工、模块完成顺序、最后提交版本和当时已知限制；只引用能定位的资料。

【建议检查的 RespOS 代码】`docs/初赛文档/main.typ`；`docs/dev-log.md`；初赛 tag/branch；README。

【建议查看的 Git 历史】`docs/pre`、`contest-images-2026`；`git log docs/pre`；`5737bd2`、`da2f977`。

【建议准备的图 / 表】初赛阶段时间线；初赛模块成熟度表。

【建议准备的测试 / 数据】初赛构建/测例命令和原始 PDF/PPT 中能复核的结果。

【容易出现的问题】初赛 PPT 的展示图不等于最终代码；历史分数必须带版本和环境。

## 13.2 决赛阶段时间线

【本节目的】将 A/B/C、CAgent、SMP、BuildStorm 等工作放到可审计的 commit 时间线上。

【建议写什么】按日期填写目标、关键变更、验证结果和遗留问题；用状态词区分已提交、未提交、待验证。

【建议检查的 RespOS 代码】`git log --stat`；`docs/codex/current-status.md`；`buildstorm-smp-plan.md`；`docs/cagent/`。

【建议查看的 Git 历史】`00c6822`→`15fe1a5`→`3aa1fb5`→`cba8e24`→`dc793c4`→`17dcd4e`→`f326ac8`→`b785262`。

【建议准备的图 / 表】commit 时间线；commit→模块→测试→状态表。

【建议准备的测试 / 数据】每个里程碑的构建门禁、关键 runtime 和日志路径。

【容易出现的问题】current-status 中旧条目可能写旧 HEAD；以实际 `git rev-parse HEAD` 和当前 diff 为准。

## 13.3 三人协作与交叉审查

【本节目的】说明如何并行开发又保持内核边界和证据一致。

【建议写什么】填写三人负责模块、接口协商、审查门禁、冲突处理、日志归档和最终合稿流程。

【建议检查的 RespOS 代码】`docs/cagent/day1.md`；`docs/git团队协作.md`；相关 commit author/path。

【建议查看的 Git 历史】`347414d`；各模块提交历史；不要依据作者名猜测贡献，需团队确认。

【建议准备的图 / 表】成员—文件—审阅者矩阵；接口交接清单。

【建议准备的测试 / 数据】每次合并前双架构 build、`fmt --check`、`git diff --check` 和指定 smoke。

【容易出现的问题】不要多人同时改同一个章节；不要把 AI/队友生成的未经复核文字直接并入正式正文。

## 13.4 最终版本冻结

【本节目的】定义交稿前的版本、文档、代码和测试冻结动作。

【建议写什么】记录最终 commit/tag、工作树状态、镜像 hash、生成 PDF 的源文件、最终测试清单和未解决问题。

【建议检查的 RespOS 代码】`git status`；`Makefile`；`docs/初赛文档工具链.md`；最终 `docs/final/`。

【建议查看的 Git 历史】最终提交前后 `git diff`；不在本章节自行创建 commit。

【建议准备的图 / 表】交稿 checklist；代码/文档/数据三者版本绑定表。

【建议准备的测试 / 数据】双架构构建和必要 runtime 的最终日志；Typst/PDF 生成命令和输出 hash。

【容易出现的问题】本地 dirty tree、临时 probe、诊断 trace、被修改的镜像都会影响可复现性。
