// RespOS 初赛设计文档
// Typst 0.11.1

// ============================================================
// 全局样式设置
// ============================================================

// 中文字体配置，Windows 字体需通过 --font-path 指定
// Typst 会自动选择数组中第一个可用的字体
#let font-hei  = ("SimHei", "微软雅黑", "Noto Sans CJK SC")     // 黑体 - 标题
#let font-song = ("SimSun", "宋体", "Noto Serif CJK SC")      // 宋体 - 正文
#let font-kai  = ("KaiTi", "楷体")                             // 楷体
#let font-fang = ("FangSong", "仿宋")                          // 仿宋
#let font-body = font-song + ("Times New Roman",)              // 正文字体组合
#let font-mono = ("Cascadia Code", "Consolas", "Courier New")  // 等宽字体

// 页面设置：A4 纸
#set page(
  paper: "a4",
  margin: (top: 3cm, bottom: 2.5cm, left: 2.5cm, right: 2.5cm),
  // 正文页页眉
  header: context {
    if counter(page).get().first() > 3 {
      align(right, text(size: 9pt, font: font-hei)[RespOS 设计文档])
    }
  },
  // 页码
  footer: context {
    let n = counter(page).get().first()
    if n > 1 {
      align(center, text(size: 9pt, {
        if n <= 3 {
          numbering("I", n - 1)
        } else {
          str(n - 3)
        }
      }))
    }
  },
)

// 正文字体
#set text(
  size: 12pt,
  font: font-body,
  lang: "zh",
)

// 标题样式
#set heading(numbering: "1. 1.1 1.1.1")

#show heading.where(level: 1): it => {
  pagebreak()
  set align(center)
  set text(size: 17pt, font: font-hei, weight: "bold")
  block(
    spacing: 0.8em,
    it.body
  )
  v(0.5em)
}

#show heading.where(level: 2): it => {
  set text(size: 14pt, font: font-hei, weight: "bold")
  block(
    spacing: 0.6em,
    it.body
  )
}

#show heading.where(level: 3): it => {
  set text(size: 12pt, font: font-hei, weight: "bold")
  block(
    spacing: 0.4em,
    it.body
  )
}

// 段落间距
#set par(justify: true, leading: 0.8em, first-line-indent: 2em)

// 代码块样式
#show raw.where(block: true): it => {
  set text(size: 8pt, font: font-mono)
  block(
    fill: rgb("#f5f5f5"),
    inset: 10pt,
    radius: 4pt,
    width: 100%,
    it
  )
}

// 表格样式
#show table: it => {
  set text(size: 10pt)
  align(center, it)
}

// 图片样式
#show figure.where(kind: image): it => {
  set align(center)
  it
}

// ============================================================
// 封面
// ============================================================

#set align(center)
#block[
  #v(4em)

  #set text(size: 28pt, font: font-hei, weight: "bold")
  RespOS
  #v(1em)
  设计文档

  #v(6em)

  #set text(size: 14pt, font: font-hei)
  #table(
    columns: (auto, auto),
    align: (right + horizon, left + horizon),
    stroke: none,
    gutter: 1em,
    [参赛队名], [TODO: 队名],
    [队伍成员], [TODO: 成员1、成员2],
    [指导教师], [TODO: 指导老师],
    [日 期], [2026 年 TODO 月 TODO 日],
  )
]

// ============================================================
// 摘要
// ============================================================

#pagebreak()
#set par(first-line-indent: 2em)

#heading(outlined: false, level: 1)[摘 要]

RespOS 是一个使用 Rust 语言开发、支持 RISC-V 64 和 LoongArch 64 硬件平台的宏内核操作系统。TODO: 补充项目简介。

截至 TODO 日期，RespOS 已经通过初赛的 TODO 测试点，TODO 排行榜情况。

#v(1em)
#heading(level: 2)[模块完成情况]

#figure(
  kind: table,
  supplement: [表],
  caption: [模块完成情况],
)[
  #table(
    columns: (auto, 1fr),
    align: (center + horizon, left + horizon),
    stroke: (x, y) => if y == 1 { none } else { (bottom: 0.5pt + gray) },
    inset: 8pt,
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
#set align(left)
#set par(first-line-indent: 0em)
#outline(
  title: [目 录],
  depth: 3,
)

// ============================================================
// 正文开始（页码重置为 1）
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
│       │   ├── vfs     // 虚拟文件系统
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
