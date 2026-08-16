---
name: kernel-doc-writing
description: Write and revise judge-facing operating-system kernel documentation. Use when creating, restructuring, polishing, or source-checking Chinese technical chapters that explain kernel architecture, module design, supported functionality, implementation mechanisms, engineering trade-offs, code structures, or cross-module interactions.
---

# Kernel Doc Writing

Use this skill to present an operating-system kernel as a designed system rather than as a source-code inventory. Optimize for a competition judge who needs to understand what the team built, why the design was chosen, how the implementation works, and which engineering details demonstrate correctness or maturity.

## Workflow

### 1. Establish evidence

Before changing prose:

- Read repository instructions and relevant project-context, architecture, status, decision, workflow, and pitfall notes.
- Inspect the current chapter before editing it.
- Locate the owning source files, data structures, call chains, and tests or logs that support substantive claims.
- Treat current source and reproducible results as stronger evidence than historical prose or memory.
- Use `pdftotext -layout` or equivalent tools when comparing with reference PDF documents.

Do not copy historical claims merely because they sound persuasive. Mark uncertain facts for later verification or omit them.

### 2. Build the chapter narrative

Organize a typical module chapter as:

```text
概述与设计目标
  → 基础概念和核心数据结构
  → 核心调用链或算法流程
  → 本项目的工程取舍与特色设计
  → 与其他模块的协作
  → 功能与设计成果总结
```

Use the sequence “是什么 → 如何工作 → 为什么这样设计 → 产生什么效果”. Adjust the number of sections to the module; do not force empty subsections into a chapter.

### 3. Write for judges

- State the design choice in the first sentence of a paragraph.
- Explain the problem the design solves before describing implementation details.
- Connect every major mechanism to a user-visible capability, correctness property, performance benefit, or architecture requirement.
- Prefer “采用……设计，因为……” and “与……不同，RespOS……” over disconnected API descriptions.
- Introduce Linux or other kernels as reference models, then state what this project retained, changed, or extended.
- Present engineering details as evidence of design maturity: lifecycle management, failure atomicity, lock boundaries, cross-architecture abstraction, cache behavior, or concurrency handling.
- Keep routine flags, standard constants, and ordinary system calls brief unless they change object identity, sharing, synchronization, or user-visible behavior.

Do not turn the conclusion into a defect report. Mention a limitation only when it clarifies a design boundary, an implemented semantic choice, or the interpretation of a feature. Prefer a “功能与设计总结” table over a standalone list of shortcomings.

### 4. Explain foundations before refinements

Define the module’s basic objects before discussing optimizations or ABI details. For each important object, explain:

1. what concept it represents;
2. which state it owns;
3. who creates, shares, updates, and releases it;
4. which other objects it references;
5. what user-visible behavior depends on it.

For example, a filesystem chapter should distinguish fd, open-file description, path, dentry, inode, and superblock before discussing `CLOEXEC`, page-cache writeback, or rename rollback.

### 5. Use code, diagrams, and tables deliberately

- Number code blocks as `代码片段 X-X  描述`.
- Explain each code block immediately afterward; explain design meaning, not merely syntax.
- Number diagrams and tables as `图 X-X  描述` and `表 X-X  描述`.
- Refer to every diagram or table in surrounding prose.
- Use a flow diagram for a multi-stage call chain, a table for object responsibilities or feature mappings, and a short code block for a representative structure or invariant.
- Do not paste large routine implementations. Show the smallest excerpt that proves the design.

### 6. Control depth

Expand substantially when the material is:

- an original data structure or abstraction;
- a cross-architecture difference;
- a concurrency, lifecycle, or failure-atomicity protocol;
- a performance optimization;
- a difficult engineering problem with a verified solution.

Summarize briefly when the material is:

- a standard Linux interface with no project-specific behavior;
- a third-party library whose internals are not modified;
- a long list of flags or routine syscall wrappers;
- an implementation detail that does not affect design understanding.

### 7. Review the result

Before handing off:

- Remove drafting prompts such as “建议写什么”, “建议检查代码”, and “待人工填写”.
- Check that the chapter opening states its central question and the ending summarizes achieved design and functionality.
- Check that all code paths and symbol names still exist in the source tree.
- Check terminology consistency across neighboring chapters.
- Check figure, table, and code numbering and in-text references.
- Run `git diff --check` and inspect the final diff.
