# RespOS 项目上下文

本文给未来维护者提供最短接手路径。稳定架构事实以当前源码为准，快速变化的状态统一放在
[current-status.md](./current-status.md)，设计理由见 [decisions.md](./decisions.md)。

## 队友接手入口

队友同步仓库后，建议按以下顺序阅读：

1. [current-status.md](./current-status.md)：当前提交、已验证结果和正在进行的阻断；
2. 当前现场赛分工见 [software-compatibility-network-plan.md](./software-compatibility-network-plan.md)：
   当前主线负责真实软件兼容性，队友负责 virtio-net 与 Git HTTP(S)/SSH；既有 Phase 5/POSIX 语义矩阵见
   [posix-semantics-execution-plan.md](./posix-semantics-execution-plan.md)；
3. [workflows.md](./workflows.md)：构建、镜像恢复和 QEMU 运行命令；
4. [architecture.md](./architecture.md)：修改内核前需要遵守的调用链和不变量；
5. [pitfalls.md](./pitfalls.md)：已知失败模式和排查顺序；
6. [../cagent/day1.md](../cagent/day1.md)：题目一历史协作材料。

当前现场赛健壮线基线为 `ae2f38ce`。Linux/POSIX Phase 0--4 主体和 Phase 5 多个核心语义已经闭合；
2026-08-16 起近期开发改为两条面向真实软件的并行线：当前主线继续 Git/Vim/GCC/rustc 软件兼容性，
队友推进 QEMU virtio-net 以及 Git HTTP(S)/SSH。线上评分提交仍固定为既有决策中的 `44f93dbb`，不得
混用两条分支的测试证据。实际软件、真实网卡和平台结果在产生对应日志前统一标记 `待验证`。

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
| `os/src/platform/` | QEMU RV64、JH7110、QEMU LA64、LS2K1000 的板级实现 | 唯一的 `board_*` feature 选择点；拥有入口、MMIO、设备、启动/关机策略 |
| `os/src/mm/` | 地址、frame/heap、`MemorySet`、VMA、COW/lazy/file mmap、用户拷贝 | 地址空间语义应集中在这里 |
| `os/src/task/` | TCB、线程组、scheduler、futex、退出回收 | 状态转换和 single-winner 语义是重点 |
| `os/src/fs/` | VFS、ext4、mount、namei、fd、page cache、pipe、proc/dev | 区分 fd、open file、path、dentry、inode |
| `os/src/syscall/` | Linux ABI 参数解析与各领域入口 | 保持薄层，避免在 syscall 中复制领域状态机 |
| `os/src/signal/` | signal state、handler、siginfo、alt stack | trap context 有架构差异 |
| `os/src/net/` | smoltcp socket、TCP/UDP、loopback/Ethernet、listen table | loopback 与 virtio-net 共享 SocketSet；当前真实接口是 QEMU 静态 IPv4 |
| `os/src/drivers/` | 通用设备/DMA 抽象与 virtio 实现 | 平台通过统一块设备工厂选择 virtio、SD 或 AHCI；真实网卡当前采用 10ms polling |
| `user/` | no_std 用户库、系统调用封装、工具、probe、testrunner | `user/build.rs` 生成 LTP 清单 |
| `img/` | 比赛测试镜像 | 运行会修改镜像内容，必要时从 `.xz` 恢复 |
| `judge/` | LTP 日志解析、Linux baseline 对比 | 不要用 QEMU 退出码替代日志分析 |
| `scripts/` | 镜像下载、LTP 报告等辅助流程 | `scripts/get_img.sh` 保留下载压缩包 |
| `auxfs/` | 辅助盘的运行 profile 与 software/bootstrap payload | 源码布局与 guest `/respos` 挂载点分离；统一由制盘脚本组装 |
| `docs/codex/` | Codex 接手摘要、状态和验证工作流 | 只保留经核验的项目知识 |
| `docs/cagent/` | CAgent 题目执行计划和协作材料 | 队友按当前阶段文档执行，不放镜像或产物 |

## 当前开发基线与目标

### 2026-08-16 软件兼容性与真实网络双线

- 状态：当前
- 适用范围：现场赛健壮线 `ae2f38ce` 后续工作
- 最后验证：2026-08-16
- 证据：[software-compatibility-network-plan.md](./software-compatibility-network-plan.md)、当前源码的
  loopback net 与 block-only virtio driver、官方 2023--2025 现场赛 README
- 内容：Phase 5 后续改为真实 workload 驱动。Git/Vim/GCC/rustc 本地矩阵已建立；virtio-net 已合入并
  双架构通过用户态 DNS/HTTP/Git HTTPS clone。后续继续由真实失败补 TTY/PTY、进程/libc、文件一致性、
  proc/dev 等语义，并推进 Git SSH。HTTP server 只作可选诊断，不是交付目标。
- 后续影响：两条线在 Git 本地 → HTTP(S) → SSH 处汇合，共享入口实行单写入者；真实网卡证据必须
  排除 loopback，所有应用通过声明必须绑定当前 commit、镜像和双架构日志。

### 2026-08-13 双线开发基线

- 状态：历史基线；2026-08-16 起由软件兼容性/真实网络双线接替
- 适用范围：`1788fa2` 至路线切换前的工作
- 最后验证：2026-08-13
- 证据：当前 Git HEAD、[current-status.md](./current-status.md) 顶部状态收口、双架构兼容构建记录
- 内容：架构线负责 `os/src/arch/**` 和 LA SMP/TLB/ASID 的底层协议与性能验证；Phase 线负责
  Linux/POSIX Phase 5，先推进与架构低耦合的 IPC/network 和 task/signal，再在底层 shootdown 接口
  稳定后合入 mmap EOF/truncate/SIGBUS。`MemorySet`、scheduler/processor/task、trap context 和公共
  arch API 是共享集成面，修改前必须先约定接口与验证责任。
- 性能线新增负责 BuildStorm ext4/PageCache 关键路径优化，但这是按不变量协作而非永久文件归属：
  性能线可以跨 ext4、PageCache、VFS/file/namei 完成完整调用链；Phase 线保有 Linux/POSIX 可观察
  契约。inode identity/generation、dirty-owner/writeback、truncate 和 mmap 是共享协议，同一时段
  单写入者，方案与门禁见 [buildstorm-smp-plan.md](./buildstorm-smp-plan.md)。
- 后续影响：平台不可用期间以本地构建、Linux 对照、专项 probe、SMP 压力和固定窗口作为开发证据，
  但不申报平台成绩；平台恢复后先复评当前 HEAD，再按新增改动补正式镜像门禁。

### A/B/C 重构历史基线

- 状态：历史基线，已被后续提交覆盖
- 适用范围：理解早期 task/MM/FS 所有权来源
- 最后验证：2026-08-01
- 证据：Git `44430df`、`50f040b`、`e0d69fd`、`2f736d4`、`cba8e24`；
  `docs/四天内核重构-ABC-整合审查.md`
- 内容：task/runtime（A）、MM（B）、FS/file ABI（C）曾在 `dev@44430df` 完成整合。该提交解释
  当前若干所有权不变量，但不再是当前开发 HEAD 或测试基线。
- 后续影响：后续修改应尽量保留已建立的不变量和 ABI 诚实性，同时恢复原有有效测例；不能
  为提高用例数量重新引入假成功或破坏失败原子性。

### 保持有效历史测例，再继续优化

- 状态：持续原则；原 writable `MAP_SHARED` 阻断已解除
- 适用范围：当前两条推进线
- 最后验证：2026-08-13 状态收口
- 证据：[current-status.md](./current-status.md) 的 Phase 3/4、RV64/LA64 运行记录
- 内容：优化内核的同时维持原有具有语义价值的测例。依赖明显取巧行为的旧用例不自动构成
  兼容性要求，但排除前必须能解释其为何不代表目标 ABI。
- 后续影响：writable file `MAP_SHARED` 已采用 PageCache 统一页帧和锁外写回协议，不再是当前入口。
  当前根据 Linux 对照 probe 推进 Phase 5，并继续用真实日志判断 LTP/比赛 workload；不得把早期
  600 余项历史分数当作当前结果。

## 证据优先级

1. 当前源码、构建脚本和当前版本的双架构运行日志。
2. 能定位到当前提交的 Git 历史和整合审查记录。
3. 较早的项目文档与 Codex 记忆，只作为历史线索，必须重新核验。
4. README 中的历史成绩不能覆盖当前回归结果。
