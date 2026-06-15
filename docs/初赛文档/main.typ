// RespOS 初赛设计文档
// Typst 0.11.1

// ============================================================
// 字体配置
// ============================================================

// 字体配置：Dev Container 用文鼎字体，WSL 回退到 Windows 字体
#let font-hei  = ("AR PL UMing CN", "SimHei", "微软雅黑")
#let font-song = ("AR PL SungtiL GB", "SimSun", "宋体")
#let font-kai  = ("AR PL KaitiM GB", "KaiTi", "楷体")
#let font-body = font-song + ("Times New Roman",)
#let font-mono = ("DejaVu Sans Mono", "Cascadia Code", "Consolas", "Courier New")

// ============================================================
// 页面设置
// ============================================================

#set page(
  paper: "a4",
  margin: (top: 2.5cm, bottom: 2cm, left: 2.2cm, right: 2.2cm),
  header: context {
    if counter(page).get().first() > 3 {
      align(right, text(size: 9pt, font: font-hei, fill: gray)[RespOS 设计文档])
      line(length: 100%, stroke: 0.5pt + gray)
    }
  },
  footer: context {
    let n = counter(page).get().first()
    if n > 1 {
      align(center, text(size: 9pt, fill: gray, {
        if n <= 3 {
          numbering("I", n - 1)
        } else {
          str(n - 3)
        }
      }))
    }
  },
)

// ============================================================
// 样式设置
// ============================================================

// 正文
#set text(size: 11pt, font: font-body, lang: "zh")
#set par(justify: true, leading: 0.65em, first-line-indent: 2em)

// 标题编号和样式
#set heading(numbering: "1. 1.1 1.1.1")

#show heading.where(level: 1): it => {
  pagebreak()
  set align(center)
  set par(first-line-indent: 0em)
  set text(size: 16pt, font: font-hei, weight: "bold")
  block(spacing: 0.5em, it.body)
  v(0.4em)
}

#show heading.where(level: 2): it => {
  set par(first-line-indent: 0em)
  set text(size: 13pt, font: font-hei, weight: "bold")
  block(spacing: 0.4em, it.body)
  v(0.2em)
}

#show heading.where(level: 3): it => {
  set par(first-line-indent: 0em)
  set text(size: 11.5pt, font: font-hei, weight: "bold")
  block(spacing: 0.3em, it.body)
}

// 代码块
#show raw.where(block: true): it => {
  set text(size: 7.5pt, font: font-mono)
  set par(first-line-indent: 0em, leading: 0.5em)
  block(
    fill: rgb("#f2f3f5"),
    inset: (x: 10pt, y: 8pt),
    radius: 3pt,
    width: 100%,
    it
  )
}

// 表格
#show table: it => {
  set text(size: 10pt, font: font-body)
  set par(first-line-indent: 0em)
  align(center, it)
}

// 图片居中
#show figure.where(kind: image): it => {
  set align(center)
  set par(first-line-indent: 0em)
  it
}

// ============================================================
// 封面
// ============================================================

#set align(center)
#set par(first-line-indent: 0em)

#block[
  // 山大 logo + 校名
  #v(1.5em)
  #set align(center)
  #block(
    fill: rgb("#8B1A2B"),
    width: 18em,
    height: 3.8em,
    radius: 3pt,
  )[
    #v(0.7em)
    #image("sdu-logo.svg", width: 18em)
  ]
  #v(0.3em)
  #set text(size: 13pt, font: font-kai)
  山东大学（青岛）

  #v(4em)

  // 大标题
  #set text(size: 42pt, font: font-hei, weight: "bold")
  RespOS
  #v(0.3em)
  #set text(size: 20pt, font: font-hei)
  设计文档

  #v(5em)

  // 队伍信息
  #set text(size: 14pt, font: font-song)
  #table(
    columns: (auto, auto),
    align: (right + horizon, left + horizon),
    stroke: none,
    gutter: 0.8em,
    [参赛队名], [#text(font: font-hei)[比特工匠队]],
    [队伍成员], [李欣悦、肖安康、张俞睿],
    [指导教师], [颜廷坤、潘润宇],
    [日 期], [2026 年 六 月],
  )
]

// ============================================================
// 摘要
// ============================================================

#pagebreak()
#set align(left)
#set par(first-line-indent: 2em)

#heading(outlined: false, level: 1)[摘 要]

RespOS 是一个使用 Rust 语言开发、支持 RISC-V 64 和 LoongArch 64 硬件平台的宏内核操作系统。TODO: 补充项目简介。

截至 TODO 日期，RespOS 已经通过初赛的 TODO 测试点，TODO 排行榜情况。

#v(0.8em)
#heading(level: 2)[模块完成情况]

#figure(
  kind: table,
  supplement: [表],
  caption: [模块完成情况],
)[
  #table(
    columns: (auto, 1fr),
    align: (center + horizon, left + horizon),
    inset: 7pt,
    table.header(
      [模块], [完成情况],
    ),
    [进程管理], [TODO: 完成情况描述],
    [内存管理], [TODO: 完成情况描述],
    [文件系统], [TODO: 完成情况描述],
    [信号机制], [TODO: 完成情况描述],
    [进程间通信], [TODO: 完成情况描述],
    [时钟模块], [TODO: 完成情况描述],
    [网络模块], [TODO: 完成情况描述],
    [设备驱动], [TODO: 完成情况描述],
    [架构管理], [TODO: 完成情况描述],
  )
]

// ============================================================
// 目录
// ============================================================

#pagebreak()
#set heading(outlined: false)
#set par(first-line-indent: 0em)
#outline(
  title: [目 录],
  depth: 3,
)

// ============================================================
// 正文开始
// ============================================================

#set page(
  numbering: n => {
    let real = n - 3
    if real < 1 { none } else { str(real) }
  }
)

// ============================================================
// 第1章 概述
// ============================================================

#heading(level: 1)[概述]

#heading(level: 2)[RespOS 介绍]

RespOS 是一款 TODO: 项目介绍文字。

#heading(level: 2)[RespOS 整体架构]

项目结构如下：

```raw
.
├── doc                 // 文档相关
├── os                  // 内核源码
│   ├── Cargo.toml
│   └── src
│       ├── arch        // 架构相关（RISC-V 64 / LoongArch 64）
│       │   ├── rv64     // RISC-V 64 架构支持
│       │   └── loongarch64  // LoongArch 64 架构支持
│       ├── drivers     // 设备驱动（VirtIO 块设备）
│       ├── fs          // 文件系统
│       │   ├── vfs     // VFS 虚拟文件系统
│       │   ├── ext4    // Ext4 磁盘文件系统
│       │   ├── proc    // procfs 进程文件系统
│       │   ├── dev     // devfs 设备文件系统
│       │   └── ...
│       ├── mm          // 内存管理
│       ├── mutex       // 锁机制
│       ├── signal      // 信号处理
│       ├── syscall     // 系统调用
│       ├── task        // 任务管理
│       └── utils       // 工具函数
├── user                // 用户态程序
│   └── src/bin         // 用户程序二进制
├── scripts             // 构建脚本
├── Makefile            // 顶层 Makefile
└── CLAUDE.md           // 项目说明
```

// 图 1-1: 总体架构图
#figure(
  kind: image,
  supplement: [图],
  caption: [RespOS 总体架构],
)[
  #text(size: 10pt, fill: gray)[TODO: 插入架构图]
]

// ============================================================
// 第2章 进程管理
// ============================================================

#heading(level: 1)[进程管理]

#heading(level: 2)[概述]

TODO: 进程管理概述。

#heading(level: 2)[任务控制块（TaskControlBlock）设计]

TODO: TCB 设计说明。

// ============================================================
// 第3章 内存管理
// ============================================================

#heading(level: 1)[内存管理]

#heading(level: 2)[概述]

TODO: 内存管理概述。

// ============================================================
// 第4章 文件系统
// ============================================================

#heading(level: 1)[文件系统]

#heading(level: 2)[概述]

TODO: 文件系统概述。

// ============================================================
// 第5章 进程间通信
// ============================================================

#heading(level: 1)[进程间通信]

#heading(level: 2)[概述]

TODO: 进程间通信概述。

// ============================================================
// 第6章 时钟模块
// ============================================================

#heading(level: 1)[时钟模块]

#heading(level: 2)[概述]

TODO: 时钟模块概述。

// ============================================================
// 第7章 网络模块
// ============================================================

#heading(level: 1)[网络模块]

#heading(level: 2)[概述]

TODO: 网络模块概述。

// ============================================================
// 第8章 设备驱动
// ============================================================

#heading(level: 1)[设备驱动]

#heading(level: 2)[概述]

TODO: 设备驱动概述。

// ============================================================
// 第9章 支持 RISC-V 和 LoongArch 的硬件抽象层
// ============================================================

#heading(level: 1)[支持 RISC-V 和 LoongArch 的硬件抽象层]

#heading(level: 2)[概述]

TODO: 硬件抽象层概述。

// ============================================================
// 第10章 总结与展望
// ============================================================

#heading(level: 1)[总结与展望]

#heading(level: 2)[工作总结]

TODO: 工作总结。

#heading(level: 2)[经验总结]

TODO: 经验总结。

#heading(level: 2)[项目意义]

TODO: 项目意义。

#heading(level: 2)[未来计划]

TODO: 未来计划。
