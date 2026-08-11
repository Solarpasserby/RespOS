= 11. 项目参考、AI 使用与第三方代码
<11-项目参考ai-使用与第三方代码>
#quote(block: true)[
本章回答：RespOS 参考了哪些项目？AI 工具如何参与开发？第三方代码的来源和许可证是什么？
]

== 11.1 项目参考与相关对比
<111-项目参考与相关对比>
【待人工填写】

#quote(block: true)[
【本节目的】说明 RespOS 参考了哪些项目、与相关内核的设计差异。

【建议写什么】

- 基础项目：基于哪个 rCore-Tutorial 版本 / 哪个 chX 起点
- 参考的内核项目及借鉴内容：
  - 如参考了 Phoenix 的 VFS 设计、Titanix 的调度器思路等
  - 每一项都要说\"参考了什么\"和\"我们改了什么、为什么不同\"
- 与其他竞赛内核的性能/功能对比（如有数据）

【建议检查的代码】Git 最早 commit、`README.md` 中的参考声明

【建议准备的表】参考项目---参考内容---本队改进---差异说明表
]

== 11.2 AI 工具使用说明
<112-ai-工具使用说明>
【待人工填写】

#quote(block: true)[
【本节目的】完整披露 AI 工具的使用范围、方式和人工复核边界。

【建议写什么】

- 使用的 AI 工具清单（名称、版本/模型、使用时间段）
- 各工具的使用场景：
  - 代码生成 / 代码补全
  - Bug 定位与调试辅助
  - 文档撰写与整理
  - 方案讨论与技术调研
- 每个场景说明输入/输出类型和人工复核方式

【容易出现的问题】

- 不能把所有代码归因于 AI；也不能隐瞒 AI 参与的测试、Debug 或文档整理
- AI 输出的代码必须说明谁验证了生命周期、锁和失败路径
]

== 11.3 AI 产生内容与人工复核案例
<113-ai-产生内容与人工复核案例>
【待人工填写】

#quote(block: true)[
【本节目的】用具体案例说明 AI 建议如何被验证、修改或拒绝。

【建议写什么】

- 选 2-4 个典型案例
- 每个案例格式：AI 输出摘要 → 实际采用部分 → 人工修改内容 → 源码/测试证据 → 未采用原因
- 区分\"AI 建议日期\"与\"代码合入日期\"

【建议准备的表】建议→人工审查→代码变更→测试结果流程表
]

== 11.4 第三方代码与依赖
<114-第三方代码与依赖>
#figure(
  align(center)[#table(
    columns: 4,
    align: (auto,auto,auto,auto,),
    table.header([项目/资料], [使用位置], [使用方式], [本队修改],),
    table.hline(),
    [`lwext4_rust`], [`vendor/lwext4_rust/`], [ext4 后端，通过 C FFI 提供 ext4 读写能力], [封装为 `Ext4Inode`/`Ext4SuperBlock` 接入 VFS；实现内存分配函数对接内核堆；增加孤儿文件 rename-to-hidden 机制],
    [`smoltcp`], [`vendor/smoltcp/`、`os/src/net/`], [网络协议栈，提供 TCP/UDP/IPv4/IPv6 协议实现], [封装 `SocketFile` 接入 VFS；适配 loopback 设备和 virtio-net 驱动],
    [`riscv` crate], [`vendor/riscv/`、arch], [提供 RISC-V CSR 寄存器和汇编指令的 Rust 封装], [作为 HAL 层的底层依赖，未做大改动],
  )]
  , kind: table
  )

#quote(block: true)[
【建议检查的代码】`Cargo.toml`，`vendor/README.md`，vendor 中的 LICENSE 文件

【待人工确认】第三方库的版本/commit、License 和准确来源
]

== 11.5 资料引用与许可证说明
<115-资料引用与许可证说明>
【待人工填写】

#quote(block: true)[
【本节目的】为源码、文档、图和附录的引用提供统一格式。

【建议写什么】

- README、vendor license、初赛参考资料、QEMU/OpenSBI/比赛资料的引用方式
- 版本日期和获取地址
- 字体与图片的版权归属（如适用）
]
