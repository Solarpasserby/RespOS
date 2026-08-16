// RespOS 决赛设计文档总入口。
// 正文来自 markdown/*.md，请通过 build.sh 重建 chapters/。

#let font-hei = ("Noto Sans CJK SC", "Noto Sans CJK HK", "Liberation Sans")
#let font-song = ("Noto Serif CJK SC", "Noto Serif CJK HK", "Liberation Serif")
#let font-mono = ("Liberation Mono", "DejaVu Sans Mono")
#let font-title = ("DejaVu Sans", "AR PL SungtiL GB")
#let brand-red = rgb("#8B1A2B")
#let ink = rgb("#202124")
#let muted = rgb("#667085")

#set page(
  paper: "a4",
  margin: (top: 2.5cm, bottom: 2cm, left: 2.2cm, right: 2.2cm),
  header: context {
    if counter(page).get().first() > 3 {
      align(right, text(size: 9pt, font: font-hei, fill: gray)[RespOS 决赛设计文档])
      line(length: 100%, stroke: 0.5pt + gray)
    }
  },
  footer: context {
    let n = counter(page).get().first()
    // 封面不显示页号，目录从 1 开始编号。
    if n > 1 { align(center, text(size: 9pt, font: font-song, fill: gray)[#(n - 1)]) }
  },
)

#set text(size: 12pt, font: font-song, lang: "zh")
#set par(justify: true, leading: 1.05em, first-line-indent: 2em)
#set heading(numbering: none)

#show heading.where(level: 1): it => {
  pagebreak()
  set align(center)
  set par(first-line-indent: 0em)
  text(size: 21pt, font: font-hei, weight: "bold", fill: ink)[#it.body]
  v(0.45em)
  line(length: 3.4cm, stroke: 1.15pt + brand-red)
  v(0.9em)
}

#show heading.where(level: 2): it => {
  set par(first-line-indent: 0em)
  v(0.75em)
  set text(size: 16pt, font: font-hei, weight: "bold", fill: ink)
  block(spacing: 0.5em)[#it.body]
  v(0.45em)
}

#show heading.where(level: 3): it => {
  set par(first-line-indent: 0em)
  v(0.55em)
  set text(size: 13.5pt, font: font-hei, weight: "bold", fill: ink)
  block(spacing: 0.35em)[#it.body]
  v(0.3em)
}

#show raw.where(block: false): it => {
  // 直接取 raw 的文本，避免 Pandoc/Typst 默认 raw 样式再次缩放行内内容。
  text(size: 12pt, font: font-song)[#it.text]
}

#show raw.where(block: true): it => {
  set text(size: 9.5pt, font: font-mono)
  set par(first-line-indent: 0em, leading: 0.65em)
  block(fill: rgb("#f2f3f5"), inset: (x: 10pt, y: 8pt), radius: 3pt, width: 100%, it)
}

#show table: it => {
  set text(size: 10.5pt, font: font-song)
  set par(first-line-indent: 0em)
  align(center, it)
}

#show image: it => align(center, it)

// 封面
#set align(center)
#set par(first-line-indent: 0em)
#v(1em)
#block(width: 100%)[
  #align(center)[
    #block(
      fill: brand-red,
      width: 24em,
      inset: (x: 16pt, y: 9pt),
      radius: 2pt,
    )[
      #image("../assets/figures/sdu-wordmark-final.svg", width: 21em)
    ]
  ]
]
#v(0.65em)
#text(size: 12pt, font: font-hei, fill: muted)[山东大学（青岛） · 计算机科学与技术学院]
#v(4em)
#text(size: 15pt, fill: muted)[全国大学生计算机系统能力大赛]
#v(0.5em)
#text(size: 42pt, font: font-title, weight: "bold", fill: ink)[RespOS]
#v(0.4em)
#line(length: 4.4cm, stroke: 1.4pt + brand-red)
#v(0.9em)
#text(size: 24pt, font: font-hei, weight: "bold", fill: ink)[操作系统内核设计文档]
#v(0.8em)
#text(size: 12pt, fill: muted)[Rust · RISC-V 64 · LoongArch 64 · Linux ABI Compatibility]
#v(4em)
#text(size: 12.5pt, font: font-song)[比特工匠队]
#v(0.6em)
#text(size: 11pt, font: font-hei, fill: muted)[决赛设计文档 · 2026 年]

#pagebreak()
#set align(center)
#text(size: 19pt, font: font-hei, weight: "bold", fill: ink)[目 录]
#v(1.2em)
#outline(title: none, indent: auto, depth: 3)

#pagebreak()
#set align(left)
#set par(first-line-indent: 2em)
#include "chapters.typ"
