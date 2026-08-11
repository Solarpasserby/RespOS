# RespOS 决赛文档 Typst 工作流

## 目录职责

```text
docs/
├── assets/             # 两份文档共用的字体、校徽和图表
└── 决赛文档/
    ├── markdown/       # Markdown 编辑源，逐章维护
    ├── main.typ       # 总模板：字体、页面、封面、目录、章节 include
    ├── generate.sh     # Markdown → 每章 Typst，并生成 chapters.typ
    ├── chapters.typ    # 自动生成的 include 清单
    ├── chapters/       # 自动生成的中间产物，不直接编辑
    ├── build.sh        # 生成章节并编译最终 PDF
    └── 决赛文档.pdf    # 编译产物
```

Markdown 是唯一的正文来源。`chapters/` 和 `chapters.typ` 可以随时删除，再由脚本完整重建；版式调整只改 `main.typ`，内容调整只改 `markdown/*.md`。字体和图片统一放在上级 `../assets/`，不再依赖初赛文档目录。

## 编译

需要 Typst 0.11.1 和 Pandoc 3.x。执行：

```bash
bash docs/决赛文档/build.sh
```

日常编辑流程是：编辑 `markdown/` 中的 Markdown → 执行 `build.sh` → 在 VS Code 中预览 `决赛文档.pdf`。Pandoc 当前覆盖标题、段落、引用、表格、行内代码和 fenced code；新增图片时，统一放在 `../assets/figures/`，然后检查生成的 Typst 路径是否正确。

不要手改 `chapters/*.typ`；它们是可审查但不可维护的生成文件。
