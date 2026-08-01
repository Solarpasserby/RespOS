# RespOS 项目上下文

本文给未来维护者提供最短接手路径。稳定架构事实以当前源码为准，快速变化的状态统一放在
[current-status.md](./current-status.md)，设计理由见 [decisions.md](./decisions.md)。

## 项目定位

### 教学与竞赛型 Linux ABI 兼容内核

- 状态：已确认
- 适用范围：整个仓库
- 最后验证：2026-08-01
- 证据：`README.md`、`Makefile`、`user/src/bin/testrunner.rs`
- 内容：RespOS 是 Rust 编写的教学与竞赛型操作系统内核，面向全国大学生操作系统比赛的
  用户态工作负载，以 Linux ABI 兼容、双架构运行和比赛镜像测例为主要工程目标。
- 后续影响：判断实现是否正确时，不应只看“系统调用存在”，还要检查 errno、失败原子性、
  生命周期以及 musl/glibc 可观察行为。

### 支持 RISC-V 64 与 LoongArch 64

- 状态：已确认
- 适用范围：构建、启动、HAL、页表、trap、signal context
- 最后验证：2026-08-01
- 证据：`Makefile`、`os/src/arch/mod.rs`、`os/src/arch/rv64/`、
  `os/src/arch/loongarch64/`；`make rv`、`make la`
- 内容：公共内核代码同时构建到 `riscv64gc-unknown-none-elf` 与
  `loongarch64-unknown-none`。架构差异集中在 `os/src/arch/`、linker script、少量
  `#[cfg(target_arch)]` 分支和对应 Cargo 配置。
- 后续影响：涉及页表、trap frame、信号、时钟或启动的修改至少要完成双架构构建；高风险
  语义还要在两套 QEMU 镜像上运行。

## 仓库模块地图

| 路径 | 职责 | 接手提示 |
| --- | --- | --- |
| `os/src/main.rs` | 启动和初始化顺序 | trap → MM → net → initproc → timer → scheduler |
| `os/src/arch/` | RV64/LA64 启动、页表、trap、上下文切换、时钟 | 公共 API 由 `arch/mod.rs` 重导出 |
| `os/src/mm/` | 地址、frame/heap、`MemorySet`、VMA、COW/lazy/file mmap、用户拷贝 | 地址空间语义应集中在这里 |
| `os/src/task/` | TCB、线程组、scheduler、futex、退出回收 | 状态转换和 single-winner 语义是重点 |
| `os/src/fs/` | VFS、ext4、mount、namei、fd、page cache、pipe、proc/dev | 区分 fd、open file、path、dentry、inode |
| `os/src/syscall/` | Linux ABI 参数解析与各领域入口 | 保持薄层，避免在 syscall 中复制领域状态机 |
| `os/src/signal/` | signal state、handler、siginfo、alt stack | trap context 有架构差异 |
| `os/src/net/` | smoltcp socket、TCP/UDP、loopback、listen table | 当前实测主要覆盖 loopback benchmark |
| `os/src/drivers/` | virtio block/net 和设备抽象 | RV 使用 MMIO，LA QEMU 使用 PCI 设备形式 |
| `user/` | no_std 用户库、系统调用封装、工具、probe、testrunner | `user/build.rs` 生成 LTP 清单 |
| `img/` | 比赛测试镜像 | 运行会修改镜像内容，必要时从 `.xz` 恢复 |
| `judge/` | LTP 日志解析、Linux baseline 对比 | 不要用 QEMU 退出码替代日志分析 |
| `scripts/` | 镜像下载、LTP 报告等辅助流程 | `scripts/get_img.sh` 保留下载压缩包 |
| `docs/` | 设计、重构、比赛和调试记录 | 本目录只做经核验的 Codex 接手摘要 |

## 当前开发基线与目标

### A/B/C 重构已整合到 `dev`

- 状态：已确认
- 适用范围：当前开发分支
- 最后验证：2026-08-01
- 证据：Git `44430df`、`50f040b`、`e0d69fd`、`2f736d4`、`cba8e24`；
  `docs/四天内核重构-ABC-整合审查.md`
- 内容：task/runtime（A）、MM（B）、FS/file ABI（C）已经合并，当前 `dev` 指向
  `44430df`。这是继续修复回归的开发基线，不是已经通过完整验收的发布基线。
- 后续影响：后续修改应尽量保留已建立的不变量和 ABI 诚实性，同时恢复原有有效测例；不能
  为提高用例数量重新引入假成功或破坏失败原子性。

### 优先恢复有效历史测例，再继续优化

- 状态：暂定
- 适用范围：`dev` 后续工作
- 最后验证：2026-08-01
- 证据：当前开发方向；当前 `make rv`/`make la` 日志显示 LTP 被 mmap 策略阻断
- 内容：优化内核的同时维持原有具有语义价值的测例。依赖明显取巧行为的旧用例不自动构成
  兼容性要求，但排除前必须能解释其为何不代表目标 ABI。
- 后续影响：先修复 writable file `MAP_SHARED` 的安全协议，使 LTP 真正执行；之后才依据真实
  LTP 结果安排下一轮 task/MM/FS 重构。

## 证据优先级

1. 当前源码、构建脚本和当前版本的双架构运行日志。
2. 能定位到当前提交的 Git 历史和整合审查记录。
3. 较早的项目文档与 Codex 记忆，只作为历史线索，必须重新核验。
4. README 中的历史成绩不能覆盖当前回归结果。
