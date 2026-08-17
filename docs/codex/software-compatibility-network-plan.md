# RespOS 软件兼容性与真实网络推进计划

## 文档状态

- 决策日期：2026-08-16。
- 代码基线：健壮线 `ae2f38cee619d7c03c936125f4c3de1745d02439`。
- 状态：已采用；具体软件和网络结果在取得当前提交、镜像与命令对应的日志前一律为 `待验证`。
- 目的：确认现场赛准备的最新工作目标、两条并行主线、共享边界和验收口径。

本计划接替“继续按接口清单无限扩展 Phase 5”作为近期主入口。已有 Phase 5/POSIX 计划继续保存语义
现状和回归矩阵，但新增实现由真实软件的第一个稳定失败驱动。线上评分提交仍按既有决策固定在
`44f93dbb6bfa6e615cc489a0c4e75309e5d56b94`；本文针对现场赛健壮线，不混用两条分支的测试证据。

## 为什么调整目标

官方现场赛正在从单项 syscall 测试转向完整软件链：

- [2023 现场赛](https://github.com/oscomp/testsuits-for-oskernel/blob/on-site-final-2023/README.md)
  已包含 `sshd`/syzkaller 等系统级工作负载；
- [2024 现场赛](https://github.com/oscomp/testsuits-for-oskernel/blob/on-site-final-2024/README.md)
  已测试 Git 的本地、HTTP、SSH 和 HTTPS 操作；
- [2025 现场赛](https://github.com/oscomp/testsuits-for-oskernel/blob/on-site-final-2025/README.md)
  明确测试 Git、Vim、GCC、rustc，并要求网络流量经过 virtio-net 或物理网卡，而非 loopback；软件版本见
  [soft-info.txt](https://github.com/oscomp/testsuits-for-oskernel/blob/on-site-final-2025/soft-info.txt)。

因此近期收益最高的目标不是宣称“POSIX 覆盖率更高”，而是让真实工具在可复现环境里完成端到端任务，
再把暴露出的 ABI 缺口沉淀成 Linux 对照、专项 probe 和双架构回归。

## 总目标与非目标

### 总目标

1. 软件兼容性线：优先跑通官方 2025 的 Git、Vim、GCC、rustc，再用 `make -j`、Python、SQLite、
   `tar/gzip/xz` 扩展复杂工作负载覆盖。
2. 网络线：从当前 loopback-only 内核推进到真实 virtio-net，尽快跑通 Git HTTP/HTTPS 与 Git SSH，
   而不是把内核 HTTP server 当成交付目标。
3. 汇合目标：在真实网卡路径上完成可重复的 Git `clone/fetch/pull`；具备可写测试远端时再完成
   `push`。HTTP(S) 与 SSH 两种 transport 分别留存证据。
4. 所有修复保留 RV64/LA64 构建、相关专项和既有回归证据；通过声明必须绑定 commit、镜像、架构、
   libc、QEMU 参数与日志。

### 本阶段非目标

- 不追求 POSIX 官方认证，也不按 syscall 数量盲目补齐全部可选接口。
- 不把简单 HTTP server 作为产品目标；它只可作为 RX/TX、listen/accept 的阶段性诊断工具。
- 不在第一阶段承诺物理开发板网卡。先闭合 QEMU RV64 virtio-mmio 与 LA64 virtio-pci；板级驱动另行立项。
- 不把 loopback iperf、socket probe 或“QEMU 已挂载 virtio-net 设备”当作真实网卡已支持。
- 不在仓库、镜像或日志中保存私钥、token、账号密码或长期有效的测试凭据。

## 分工

| 主线 | 负责人 | 主要交付 | 近期不负责 |
| --- | --- | --- | --- |
| A：软件兼容性 | 当前 Codex/主线维护者 | 真实软件矩阵、首失败定位、TTY/PTY、进程/libc、FS/MM/proc/dev 等必要语义修复 | virtio-net 驱动和链路 bring-up |
| B：真实网络 | 队友 | virtio-net、smoltcp Ethernet 接入、地址配置/DNS、Git HTTP(S) 与 SSH 网络路径 | 泛化补齐非网络 POSIX 清单 |
| 集成 | 双方 | Git 本地基线 → HTTP(S) → SSH 的端到端门禁；双架构与既有专项回归 | 未协调地同时修改共享入口 |

这里的归属是当前任务分工，不是永久的目录所有权。某个失败跨域时，由最先稳定复现的一方提交最小
证据，双方先确定状态所有者和接口，再决定由谁修改。

## 主线 A：真实软件兼容性

### A0：建立可复现软件基线

- 固定官方或等价镜像，记录 SHA-256、软件版本、动态链接器/libc、启动参数和磁盘写入策略。
- 为 Git、Vim、GCC、rustc 建立最小成功任务和超时/退出 marker；先在 Linux 上确认任务本身有效。
- 每个程序只报告已经实际运行的能力；“内核已有相关 syscall”不能替代程序通过证据。

### A1：先跑通 2025 软件矩阵

| 软件 | 第一验收任务 | 第二验收任务 | 主要观察面 |
| --- | --- | --- | --- |
| Git | `init/add/commit/status/log` 的本地仓库闭环 | 与网络线汇合做 clone/fetch/pull/push | fork/exec、文件锁、rename/fsync、时间、管道、网络子进程 |
| Vim | 非交互 `--version`/批处理编辑保存 | PTY 中打开、编辑、保存、退出 | termios、TTY/PTY、signal/job-control、mmap、配置文件 |
| GCC | 编译并运行 C hello world | 多文件编译、汇编与链接 | exec、pipe、临时文件、mmap、rusage、进程回收 |
| rustc | 编译并运行 Rust hello world | 小型多模块 crate；必要时再试 Cargo | 大地址空间、线程/futex、文件映射、进程和 I/O 压力 |

执行顺序为 Git 本地 → Vim → GCC → rustc。某项失败后先固定第一个稳定失败，不同时追逐后续连锁错误。

### A2：由失败驱动补齐高价值语义

优先级按真实软件阻断动态调整，默认顺序如下：

1. TTY/PTY：canonical/raw、echo、控制字符、`VMIN/VTIME`、窗口大小/`SIGWINCH`、controlling tty、
   foreground process group、hangup；这是交互 Vim 和可能的 SSH 交互路径的共同依赖。
2. libc 组合路径：`pthread_*`、TLS、futex、robust list、`posix_spawn()` file actions/失败回报。
3. 文件一致性：`fcntl` record lock/`flock`、`fsync/fdatasync`、rename、unlink-open、目录同步和失败原子性；
   用 Git 与 SQLite 验证，而不是只看 syscall 返回 0。
4. 运行时伪文件与设备：按程序实际读取补 `/proc/self/*`、`/dev/tty`、`/dev/urandom` 和必要 sysctl。
5. 事件与资源：按阻断补 `poll/epoll` 边缘行为、resource limit、pipe/socket backpressure 和 signal restart。

default/`KEEP_SIZE` unwritten extent、AIO、POSIX message queue、namespace/cgroup、ptrace 等保留在 backlog；
除非真实 workload、LTP 或答辩目标给出证据，否则不抢占上述任务。

### A3：扩展复杂软件与自举入口

官方矩阵稳定后，依次尝试 `make -j`、Python、SQLite、`tar/gzip/xz`。这些程序用于扩大线程、进程、
文件一致性和事件组合覆盖，并帮助发现“单测都过、组合仍失败”的问题。

自举仍是有价值的长期展示目标，但近期先把 GCC/rustc 的编译、汇编、链接和多进程路径跑稳。只有在
工具链、头文件、构建系统、磁盘空间和长时稳定性都有证据后，才把“内核或用户态自举”设为正式门禁。

## 主线 B：virtio-net 与 Git 远端操作

### 当前边界

- 2026-08-16 更新：RV64 virtio-mmio 与 LA64 virtio-pci net 已合入，loopback/Ethernet 共享 SocketSet；
  QEMU 静态 IPv4、默认路由、UDP DNS、HTTP 与 Git HTTPS `ls-remote`/clone 已双架构通过。
- 内核 HTTP 已降为 `kernel_http` diagnostic feature，不进入普通 software/final/submission 路径。
- 下一阻断转为 Git SSH 用户态注入和门禁，以及 DHCP/中断、断网/重连等硬化；详细证据见
  [current-status.md](./current-status.md)。以下 B0--B4 保留为设计与尚未完成部分的检查表。

### B0：设备与接口契约

- 定义通用 net device 所有权、RX/TX buffer 生命周期、DMA 映射、MAC/MTU 和 poll/interrupt 边界。
- smoltcp interface 必须根据 link device 选择，不再让所有 inet socket 隐式绑定 IP-medium loopback。
- 第一版允许 polling，但要有明确唤醒/定时驱动，不能在 syscall 中无界 busy loop。

### B1：RV64 真实链路

- 枚举并初始化 virtio-mmio net，完成 TX、RX、MAC 和错误恢复。
- 先用 QEMU user networking 与静态地址跑通 ARP、IPv4、ICMP（若实现）和 TCP；随后补 DHCP 或明确的
  可配置地址入口。
- 验收日志必须显示非 loopback interface/MAC/IP，并由宿主或外部端点确认收发，不能只连接
  `127.0.0.1`。

### B2：Git HTTP/HTTPS

- 补齐 DNS、路由、TCP connect/error、poll/epoll、并发 socket 与合理超时。
- “Git HTTP”验收同时覆盖普通 HTTP 和实际常见的 HTTPS/redirect 路径。TLS、CA bundle、系统时间、
  `/dev/urandom`、代理变量与 Git/curl 用户态依赖需要单独盘点；内核网络连通不等于 HTTPS 已完成。
- 依次验证小仓库 `clone`、已有仓库 `fetch/pull`，最后做中断、DNS 失败、拒绝连接和超时错误路径。

### B3：Git SSH

- 优先采用非交互、一次性测试密钥，验证 DNS/IP、TCP、随机数、时间、权限、pipe、signal 和 SSH 子进程。
- 先完成 `BatchMode=yes` 的 clone/fetch；有可写临时远端后再验证 push。host key 和私钥通过运行时注入，
  不提交仓库。
- 交互式口令、agent forwarding、PTY shell 和 sshd server 不是第一验收门槛；若软件兼容线已完成 PTY，
  可作为后续展示扩展。

### B4：LA64 与硬化

- 将已在 RV64 通过的设备/协议门禁迁移到 LA64 virtio-pci，先统一 PCI 设备发现/BAR 所有权。
- correctness 稳定后再引入设备中断、NAPI 类预算轮询、DHCP、吞吐优化与更长断线重连测试。
- 物理网卡、板级中断/IOMMU 与真实交换网络另列清单，不能由 QEMU virtio-net 结果外推。

## 共享文件与并行修改规则

| 区域 | 默认主责 | 说明 |
| --- | --- | --- |
| `os/src/task/**`、`signal/**`、`mm/**`、TTY/PTY、非网络 `fs/syscall` | A | 保持 Phase 5 的 identity、exec、signal、mmap 不变量 |
| `os/src/drivers/**` 的 net 新增、`os/src/net/**`、`os/src/syscall/net.rs` | B | block 驱动公共 HAL 改动需先说明 DMA/性能影响 |
| `os/src/arch/*/pci*`、设备中断 | B 主责，架构改动共同审查 | LA block/net 必须共享确定性的枚举和 BAR 协议 |
| `main.rs`、`drivers/mod.rs`、`net/mod.rs`、`syscall/mod.rs`、`os/Cargo.toml`、Makefile | 单写入者 | 开工前约定接口和合并顺序，避免双方同时改入口 |
| `user/`、镜像脚本、测试清单、`docs/codex/current-status.md` | 按测试归属 | 同名 probe/日志 marker 先登记；镜像不得混用未记录状态 |

集成采用小而完整的提交：驱动、协议/ABI 修复、probe 和文档证据尽量同一提交闭环；不要把两条线的大量
未验证修改一次性揉在一起。共享接口发生变化时，另一条线先 rebase/merge 到已验证提交再继续。

## 汇合顺序与验收门禁

```text
A0 可复现镜像 ──> A1 Git 本地闭环 ───────────────┐
                                                   ├─> Git HTTP(S) ─> Git SSH ─> 双架构展示
B0 设备契约 ──> B1 RV virtio-net ─> DNS/TCP ─────┘                  │
                         └──────────────────────────> B4 LA64 ──────┘
```

每个里程碑至少保留：

1. 当前提交的 RV64/LA64 release 构建；架构相关实现必须有对应 QEMU 运行证据。
2. 新增 Linux oracle 或可解释的上游规范依据，以及 RespOS 专项 probe；错误路径不能只测 happy path。
3. 受影响的 Phase 5、socket、mmap、signal/job-control、BuildStorm file/minibuild 等相邻回归。
4. 应用级命令、退出码、关键输出、超时和产物校验；Git 记录 remote scheme、是否真实网卡、传输字节或
   抓包摘要，但对 URL 和日志中的凭据脱敏。
5. 日志元数据包含 commit、镜像 SHA-256、软件版本、架构/libc、QEMU 版本、内存/hart 数和日期。

完整 BuildStorm、宽 LTP 和长时 soak 按风险与集成节点运行，不要求每个小提交都付出完整轮次；但进入
现场赛演示候选前，必须完成固定资源下的双架构相关专项和至少一轮代表性综合回归。

## 对外可展示成果

近期答辩建议形成一条连贯演示，而不是孤立 syscall 数量：

1. 在 RespOS 中用 Vim 修改一份小型源码；
2. 用 GCC 或 rustc 编译并运行；
3. 用 Git 本地提交；
4. 通过真实 virtio-net 分别完成 HTTP(S) 和 SSH 的 clone/fetch，条件允许时 push；
5. 展示同一任务在 RV64/LA64 的日志，以及一个由真实软件失败反推并修正的 POSIX/Linux ABI 语义案例。

这条链同时证明交互终端、进程/线程、文件系统、内存映射、网络与用户态工具链的组合兼容性，比“能启动
一个 HTTP server”或“实现了若干 syscall”更贴近现场赛价值。
