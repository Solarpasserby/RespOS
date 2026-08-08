# 14. 第三方代码、参考项目与资料来源

## 15.1 第三方代码与依赖清单

【本节目的】列出进入构建、运行或设计的第三方代码，并区分直接依赖、vendor 和参考实现。

【建议写什么】使用下表逐项填写，不确定的基础仓库身份先标“待人工确认”。

| 项目/资料 | 来源 | 版本/commit | License | 使用位置 | 使用方式 | 本队修改 | 证据 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `lwext4_rust` | [待人工确认] | `vendor/` 当前版本 | [待补] | `vendor/lwext4_rust/` | ext4 后端 | [待补] | Cargo/目录/Git |
| `smoltcp` | [待人工确认] | `vendor/` 当前版本 | [待补] | `vendor/smoltcp/`、`os/src/net/` | 网络协议栈 | [待补] | Cargo/目录/Git |
| `riscv` | [待人工确认] | `vendor/` 当前版本 | [待补] | `vendor/riscv/`、arch | CSR/架构支持 | [待补] | Cargo/目录/Git |

【建议检查的 RespOS 代码】`Cargo.toml`；各 crate `Cargo.toml`；`vendor/README.md`；vendor README/许可证文件。

【建议查看的 Git 历史】`git log -- vendor Cargo.toml`；查找 vendor 引入和本队修改的提交。

【建议准备的图 / 表】最终第三方清单表；依赖→RespOS 模块→许可证表。

【建议准备的测试 / 数据】依赖版本、构建锁定信息和修改 diff；许可证文件路径。

【容易出现的问题】不能凭 crate 名猜 license 或来源；vendor 中本队修改必须与上游代码区分。

## 15.2 基础项目与参考实现来源

【本节目的】说明 RespOS 最初基于什么项目/版本，以及哪些设计只作为参考。

【建议写什么】从 Git 历史、README、Cargo、初赛文档和早期提交确定基础仓库；将“代码继承”“设计参考”“测试参考”分开记录。

【建议检查的 RespOS 代码】完整 Git refs；早期 `main`；`README.md`；`docs/初赛文档/main.typ`；Cargo package metadata。

【建议查看的 Git 历史】`git log --all --reverse`；最早可定位的根提交；必要时由团队补外部仓库链接和版本。

【建议准备的图 / 表】基础项目—RespOS 初始树—本队修改范围图；来源分类表。

【建议准备的测试 / 数据】基础版本 checkout 后的最小 build/boot（若可复现）；否则说明无法复现原因。

【容易出现的问题】不要把 rCore/RocketOS/其他竞赛项目的常见设计写成 RespOS 基础来源，除非 Git/许可证能证明。

## 15.3 资料引用与许可证说明

【本节目的】为源码、文档、图和 PDF 的引用提供统一格式。

【建议写什么】填写代码注释、README、vendor license、初赛参考资料、QEMU/OpenSBI/比赛资料的引用方式和版本日期。

【建议检查的 RespOS 代码】`README.md`；`vendor/*/README*`；`docs/初赛文档/`；构建脚本下载地址。

【建议查看的 Git 历史】资料首次引入 commit、后续版本升级 commit。

【建议准备的图 / 表】资料来源—正文引用位置—许可证/授权状态表。

【建议准备的测试 / 数据】生成 PDF 时检查字体、图片和外部链接；保存最终引用版本。

【容易出现的问题】初赛 PDF 中的外部图不能无来源复用；不要把“开源”当作没有许可证义务。
