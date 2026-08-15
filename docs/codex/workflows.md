# RespOS 开发与验证工作流

命令以当前仓库脚本为准。运行测试前先读 [current-status.md](./current-status.md) 和
[pitfalls.md](./pitfalls.md)。

## 队友接手入口

题目一当前阶段的执行计划、三人责任边界和每日审查格式见
[`docs/cagent/day1.md`](../cagent/day1.md)。队友应先恢复干净 pub 镜像，再按该计划使用
`make run-rv-pub` 做 RV64 单核基线；不要直接用 `make rv`/`make la` 代替 pub 入口。

CAgent 参考源码不随 RespOS 提交。若本地没有 `testsuit/cagent-test/`，从官方
[`final-2026` 分支](https://github.com/oscomp/testsuits-for-oskernel/tree/final-2026) 获取：

```bash
git clone --branch final-2026 --depth 1 \
  https://github.com/oscomp/testsuits-for-oskernel.git /tmp/testsuits-for-oskernel
```

源码位置为 `/tmp/testsuits-for-oskernel/cagent-test/`；官方启动脚本和评分器分别位于
`scripts/cagent_testcode.sh` 与 `judge/judge_cagent-glibc.py`。

## 环境与镜像

### 可选 `/respos` 辅助镜像

顶层 `Makefile` 的 `make all` 从 `respos/` 生成 16 MiB ext4 `disk.img` 和 `disk-la.img`；
提交产物名称和 profile 不接受命令行覆盖。文件存在时，RV64 将其连接到 `virtio-mmio-bus.1`，LoongArch
将其添加为第二个 `virtio-blk-pci`。文件不存在时不添加 x1。
`build-disks` 每次都完整重建两个小镜像，因此 `respos/` 中辅助文件的新增、修改和删除都会反映到产物，
不会因 Make 只观察 `profile` 而沿用陈旧内容。

辅助镜像必须是 ext4，其根目录将在 guest 中显示为 `/respos`。最小 profile 例子：

```text
mode=auto
```

线上提交使用 `mode=auto`，由 launcher 检查官方根盘中的测试脚本自动适配初赛复测或决赛；本地
强制初赛/决赛分别使用 `mode=preliminary`/`mode=final`。本地显式诊断可使用
`respos-diagnostic/profile` 的 `mode=diagnostic` 进入内嵌 `user_shell`；该目录不参与默认
`make all` 生成的提交镜像。profile 最多读取 512 bytes，忽略空行和以 `#` 开头的行；缺失、空白
或无法识别时也进入自动检测。自动检测先检查 CAgent/BuildStorm 决赛脚本，再检查 musl/glibc basic
初赛脚本；决赛标志优先，两类标志都没有时告警并回退 preliminary。final launcher
固定串行运行当前官方 pub 镜像的 `/glibc/cagent_testcode.sh` 和
`/glibc/buildstorm_testcode.sh`，不扫描宿主机或在 Makefile 中推断测例。

验证时应使用 `-snapshot`，并分别覆盖：不提供 x1、提供合法 ext4 x1、提供
非 ext4 x1。还应验证 auto 对初赛/决赛根盘的识别、两种显式 mode 的强制行为，以及决赛脚本严格
`fork → execve → waitpid` 串行并在全部完成后关机。

### 准备比赛镜像

- 状态：已确认
- 适用范围：首次运行或镜像恢复
- 最后验证：2026-08-11
- 证据：`scripts/get_img.sh`、顶层 `Makefile`
- 内容：

```bash
bash scripts/get_img.sh
ls -lh img/sdcard-rv.img img/sdcard-la.img \
  img/sdcard-rv-pub.img img/sdcard-la-pub.img
```

脚本从上游 `pre-20250615` Release 获取 `sdcard-rv.img`、`sdcard-la.img`，并从 RespOS 的
`contest-images-2026` Release 获取 `sdcard-rv-pub.img`、`sdcard-la-pub.img`。脚本优先复用
`img/` 中已有的 `.xz`/`.gz` 压缩包；LA pub 的 `.gz` 在 Release 中分为两个 `.part`，脚本会
下载并合并后再解压。解压时保留压缩包及分卷。运行中的内核会修改 ext4 镜像；需要干净基线
时，应从保留的压缩包重新解压，而不是假定上一次运行后的镜像未变化。

- 后续影响：对比两次测试前记录镜像来源；不要把本地大镜像提交到 Git。

## 构建

### 课程平台 Rust 兼容基线

课程平台在 2026-08-13 的日志表明其内核编译器为
`rustc 1.86.0-nightly (2025-01-17)`。提交前，若本地安装了对应 toolchain，应使用：

```bash
RUSTUP_TOOLCHAIN=nightly-2025-01-18 make all
```

`os/Cargo.toml` 和 `user/Cargo.toml` 声明 `rust-version = "1.85"`。不要使用在该平台
仍不稳定的 `let` chains、`usize::is_multiple_of` 或其他需要 `#![feature(...)]` 的语言/标准库
能力；以实际课程日志为准，不能只按本机较新的 nightly 判断可提交性。

### 决赛设计文档

- 状态：已实现
- 适用范围：`docs/决赛文档/markdown/*.md` 的 PDF 发布
- 证据：`docs/决赛文档/build.sh`、`generate.sh`、`main.typ`；Pandoc Typst writer
- 内容：Markdown 是逐章编辑源；`generate.sh` 使用 Pandoc 3.x 将每个 Markdown 转为
  `docs/决赛文档/chapters/*.typ`，并生成 include 清单；`main.typ` 负责封面、目录、
  页面样式和章节整合，`build.sh` 最后调用 Typst 生成 PDF。

```bash
bash docs/决赛文档/build.sh
```

- 后续影响：不要直接编辑 `chapters/` 下的生成文件；如果新增 Markdown 章节，按两位数字前缀
  命名，脚本会按文件名顺序整合。

### 顶层双架构入口

- 状态：已确认
- 适用范围：提交前构建
- 最后验证：2026-08-01
- 证据：`Makefile`
- 内容：

```bash
make all                  # 线上评测入口：生成 kernel-rv/kernel-la/disk.img/disk-la.img
make build-rv             # 只构建 RV release
make build-la             # 只构建 LA release
make MODE=debug all       # 双架构 debug
make MODE=release-debug all
make check-submit         # 构建并检查四个提交产物和 auto profile
```

顶层构建会复制架构对应的 Cargo config 到 `os/.cargo/config.toml` 和
`user/.cargo/config.toml`，先构建用户程序，再通过 `os/build.rs` 嵌入内核。
Makefile 使用 `.NOTPARALLEL`，因为两个架构共享可变 Cargo config。`make all` 固定读取
`respos/profile`，并在构建前验证第一个有效配置项为 `mode=auto`；命令行不能把线上提交入口
静默改成 preliminary/diagnostic。平台提供大型官方根镜像，仓库只生成第二块小型 ext4 辅助盘。

- 后续影响：不要并行执行 RV/LA 构建命令；共享 config 文件可能相互覆盖。

### 子目录快速检查

- 状态：已确认
- 适用范围：单架构开发循环
- 最后验证：2026-08-01
- 证据：`os/Makefile`、`user/Makefile`
- 内容：

```bash
make -C os build ARCH=riscv64 MODE=debug
make -C os build ARCH=loongarch64 MODE=debug
make -C user build ARCH=riscv64 MODE=debug
```

- 后续影响：最终验收仍回到顶层 Makefile，因为其 QEMU 参数和产物形态更接近比赛流程。

## QEMU 运行

### 启动后检查宿主调度优先级

- 状态：已确认
- 适用范围：所有由 Codex 启动的 QEMU 正确性、压力和性能运行
- 最后验证：2026-08-09
- 证据：`.devcontainer/{devcontainer.json,Dockerfile,codex-priority}`、宿主 `ps` 输出
- 内容：QEMU 启动后、开始 guest 测试或计时前，在另一终端检查实际 QEMU PID；不能只依据容器的
  `SYS_NICE`/ulimit 配置推断进程优先级：

```bash
qemu_pid="$(pgrep -n -f '[q]emu-system-(riscv64|loongarch64)')"
ps -o pid,ppid,ni,cls,stat,etime,cmd -p "$qemu_pid"
```

从可能继承 `SCHED_IDLE`/正 nice 值的 Codex 终端启动性能轮时，可通过 Make 的 QEMU 命令覆盖使用绝对
优先级包装器，例如 `QEMU_LA='scripts/run_performance_command.sh qemu-system-loongarch64'`。不要用
`nice -n -10` 代替：它是相对调整，若父进程 nice=16，子进程仍会是 nice=6。包装器需要允许执行
`chrt`/`renice` 的宿主权限；启动后仍须用上述 `ps` 命令核验 `NI=-10, CLS=TS`。
如需覆盖启动后很快结束的 CAgent，在同一 Make 命令设置 `RESPOS_PERF_SERIAL_LOG` 和
`RESPOS_PERF_TIMELINE_DIR`；包装器会在 exec QEMU 时按同一 PID 自动启动采样，避免人工取得 PID 的
时间窗。`RESPOS_PERF_INTERVAL` 可选，默认 1 秒。

重建当前 devcontainer 后，由 Codex 启动的 QEMU 期望为 `NI=-10`、`CLS=TS`（Linux
`SCHED_OTHER`）。若看到 `CLS=IDL`，或仍继承此前的 `NI=16`，应先停止该轮、修复 Codex 启动包装器
或改从正常优先级终端启动，再重新运行；这类结果可用于定位功能问题，但墙钟时间和超时不应作为
性能、活性或缩放结论。长时间 BuildStorm 还应把这行 `ps` 输出随串口日志一起保存。

- 后续影响：每次容器/扩展重启后的首个 QEMU 运行必须检查一次；正式计时和 jobs/CPU 缩放矩阵每轮
  都要记录实际 `NI/CLS`。

### 完整双架构运行

- 状态：已确认
- 适用范围：比赛镜像完整回归
- 最后验证：2026-08-01
- 证据：`Makefile`、当前运行日志
- 内容：

```bash
make prepare-pre-images   # 从保留的 .xz 恢复 4 GiB 初赛全量镜像
make run-rv-pre           # 初赛 RV64，进入内嵌 testrunner
make run-la-pre           # 初赛 LA64，进入内嵌 testrunner
make run-rv-final         # 决赛 RV64，官方 CAgent + BuildStorm
make run-la-final         # 决赛 LA64，官方 CAgent + BuildStorm
```

常用覆盖参数：

```bash
make run-rv-pre PRE_MEM=4G PRE_SMP=1 RV_PRE_OUTPUT=/tmp/respos-rv-pre.log
make run-la-pre PRE_MEM=4G PRE_SMP=1 LA_PRE_OUTPUT=/tmp/respos-la-pre.log
make run-rv-final RV_FINAL_MEM=16G RV_FINAL_SMP=8
make run-la-final LA_FINAL_MEM=12G LA_FINAL_SMP=12  # 本机内存不足时的功能配置
```

- 初赛串口日志默认分别写入仓库根目录的 `rv-output.txt` 和 `la-output.txt`，决赛日志分别写入新建的
  `rv-final-output.txt` 和 `la-final-output.txt`；四个文件均由 `tee` 同步输出到终端，并已在
  `.gitignore` 中排除。可通过对应的 `*_OUTPUT` 变量临时覆盖路径。
- 内容补充：本地目标分别在 `/tmp` 重建独立辅助盘，不会覆盖 `make all` 的提交产物。
  初赛 profile 为 `mode=preliminary`，决赛 profile 为 `mode=final`，线上 profile 为 `mode=auto`。
  LA 会通过 QEMU-virt
  IOCSR mailbox/IPI 启动最多 12 个 hart，并在进入用户态前输出
  online mask；`0xfff` 表示 12 个 hart 均已上线。本地 `rv`/`la` 目标默认使用 QEMU
  `-snapshot`，guest 在本轮仍可正常写盘，但不会因 Ctrl-C/超时把官方原始镜像留在
  journal/元数据不一致状态。RV64 使用网站给出的 virtio-mmio bus.0/1；LoongArch
  保留原 Makefile 已使用的 `-machine virt` 和 `virtio-blk-pci` 自动 PCI 总线分配。网站文本中的
  `virtio-blk-pci,...,bus=virtio-mmio-bus.0` 在本机 QEMU 10.0.2 会直接报
  `Bus 'virtio-mmio-bus.0' not found`，因此不能照抄。
- 后续影响：建议顺序运行；端口转发和共享构建配置使并行运行收益有限且更难诊断。

iozone 专项可用编译时诊断开关：

```bash
IOZONE_ONLY=1 make run-rv-pre
```

该开关只使 `testrunner` 依次运行 glibc/musl iozone 并关机，默认构建不受影响。当前
RV64 SMP=1、4 GiB 的干净镜像实测两组合计约 168 秒，不应用 60/120 秒外层超时
判定为卡死。中断前先检查日志中的 `Run began`、子项标题和 `iozone test complete.`
是否持续增长。

### 决赛镜像与交互式诊断

- 状态：已验证（RV64 启动到交互式 shell）
- 适用范围：决赛 pub 镜像的第一阶段检查
- 最后验证：2026-08-02
- 证据：顶层 `Makefile`、`user/src/bin/initproc.rs`、QEMU 直接启动日志
- 内容：

```bash
make run-rv-final       # RV pub 根盘，final profile
make run-la-final       # LA pub 根盘，final profile
make run-rv-diagnostic  # RV pub 根盘，diagnostic profile，进入 shell
make run-la-diagnostic  # LA pub 根盘，diagnostic profile，进入 shell
```

这些目标通过 virtio block 设备加载 ext4 镜像，不执行宿主机挂载。当前启动阶段由
`/respos/profile` 的 `mode=auto|preliminary|final|diagnostic` 决定，不再由 `eval` feature 直接选择
shell 或 runner。auto 根据根盘脚本识别阶段；下列本地目标仍使用显式 profile，便于隔离测试。
旧的 `run-rv-pub`/`run-la-pub` 只是 final 入口兼容别名。诊断目标才进入 shell；final 目标直接
运行 `/glibc/cagent_testcode.sh` 和 `/glibc/buildstorm_testcode.sh`。

交互式 RV64 SMP 专项构建后，可在 shell 直接运行内嵌 probe：

```text
smp_phase3_probe       # 30 轮 fork/exec/wait + pipe + loopback socket
smp_shared_mm_probe    # 至少 2 CPU；100 轮跨 CPU 固定 VA remap/read
```

QEMU 必须附 `-snapshot`，并保存完整串口日志。`task_a_futex_cmp_requeue_probe` 需先以
`TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD=1` 构建内核；专项结束后再执行一次不带该环境变量的默认构建。

- 2026-08-02 实测 `make run-rv-pub` 已完成用户程序、lwext4 和内核构建，QEMU 以 1 个
  HART、256M 内存加载 `img/sdcard-rv-pub.img`，并显示 `Rust user shell` 的 `/>` 提示符；
  未进入 `testrunner`。LA 入口已做同样的配置检查，但尚未完成本轮启动验证。

### 文件元数据 Linux/POSIX 对照

Linux 参考程序不依赖 RespOS 私有输出：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/fs_metadata_probe_linux.c \
  -o /tmp/fs_metadata_probe_linux
/tmp/fs_metadata_probe_linux normal
/tmp/fs_metadata_probe_linux prepare
/tmp/fs_metadata_probe_linux verify
/tmp/fs_metadata_probe_linux cleanup
```

RespOS 无 feature 交互内核中运行：

```text
fs_metadata_probe normal
fs_metadata_probe prepare
# 使用同一可写镜像重新启动 guest
fs_metadata_probe verify
fs_metadata_probe cleanup
```

跨启动检查必须使用原始 raw 镜像的临时 qcow2 backing overlay，不得直接写 pub 镜像。输出中的
`FS_METADATA_EXPECTED_FAIL` 表示探针成功捕获尚未修复的差异，不表示该语义通过；只有对应的
`FS_METADATA_*_PASS` 且无该项 expected failure 才能用于关闭缺陷。

`UTIME_NOW/UTIME_OMIT` 特殊值单项门禁：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/utimens_special_probe_linux.c \
  -o /tmp/utimens_special_probe_linux
/tmp/utimens_special_probe_linux

TASK_A_UTIMENS_SPECIAL_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-utimens-special.log
TASK_A_UTIMENS_SPECIAL_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-utimens-special.log
```

Linux 必须输出 `UTIMENS_SPECIAL_LINUX PASS omit=pass now=pass invalid_nsec=pass
missing_double_omit=pass permission=pass|skip`，guest 必须输出对应无 `_LINUX`、且
`permission=pass` 的 marker 与 runner PASS。本机只有以 root 运行时才能降权到 uid/gid 65534 执行权限
向量；非 root 的 `permission=skip` 只表示特殊值基线通过，不能记作 Linux 权限向量实测。probe 要求
双 OMIT 保持 atime/mtime/ctime 且不存在 pathname 也成功，单 OMIT 只保持指定字段，NOW 忽略 `tv_sec`
并落在按文件系统精度容许的 realtime 窗口内；任一普通 `tv_nsec` 为 `1000000000` 或 `-1` 时，两个字段及 ctime 均
保持不变并返回 `EINVAL`。降权向量对 pathname 和继承 fd 验证：双 OMIT 始终成功；双 NOW 在 mode
`0666` 时成功、在 `0000` 时返回 `EACCES`；显式时间和 `NOW+OMIT` 均返回 `EPERM`。该门禁不用于宣称
ext4 已持久化纳秒或负秒；两架构仍须顺序运行。

namespace identity/rename/unlink 对照：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/fs_namespace_probe_linux.c \
  -o /tmp/fs_namespace_probe_linux
/tmp/fs_namespace_probe_linux
```

RespOS 无 feature、至少 2 核的 snapshot guest 中运行 `fs_namespace_probe`。它覆盖 hardlink identity、
跨目录与覆盖 rename、打开后 unlink、打开目录被覆盖、目录 nlink/后代路径和 fork rename/open 竞态；
以 `FS_NAMESPACE_PROBE_PASS race_observations=N` 为通过标志。

Phase 4 namei/权限/fd Linux 对照：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/fs_phase4_probe_linux.c \
  -o /tmp/fs_phase4_probe_linux
/tmp/fs_phase4_probe_linux
```

RV64 no-feature release、`-snapshot` guest 中运行 `fs_phase4_probe`，以
`FS_PHASE4_PROBE_PASS` 为通过标志。它覆盖 final symlink、trailing slash errno、`O_PATH`、dup/fcntl
descriptor/open-file 分层、`AT_EMPTY_PATH`、umask、fsuid/fsgid、supplementary groups、setgid/sticky、
`O_NOATIME` 和只读 bind mount。权限 probe 会临时修改当前进程的 fsuid/fsgid/groups，但不会修改
shell 进程；失败后直接重启 snapshot guest，避免残留测试目录影响二次判读。

Phase 5 task leader/exec Linux 对照：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/task_phase5_probe_linux.c \
  -o /tmp/task_phase5_probe_linux
/tmp/task_phase5_probe_linux
```

当前 no-feature guest 可运行 `task_phase5_probe`。修复前预期打印四个
`TASK_PHASE5_EXPECTED_FAIL` 和 `TASK_PHASE5 CURRENT DIFFERENCES CONFIRMED`，并以非零状态退出；这些
marker 只证明探针捕获了已确认差异，不是通过。完成 task 生命周期修复后，退出门槛是
`TASK_PHASE5 ALL PASS`，并且不得出现 expected-fail marker。探针分别覆盖 leader 原始 `SYS_exit`
后 worker 的 `exit_group`、worker 的原始 `SYS_exit`，以及非 leader `execve` 后
`getpid() == gettid()` 的 identity 接管。新增稳定身份项还覆盖 leader 原始退出后 `WNOHANG` 不得提前
收尸、TGID 仍可被 `kill(pid)` 查询、process-directed SIGUSR1 递送给 worker，以及 worker 的
`getpid()==TGID`/`gettid()!=TGID`。

当前双架构 2 hart 自动入口：

```bash
TASK_A_TASK_PHASE5_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-task-phase5-identity-gate.log
TASK_A_TASK_PHASE5_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-task-phase5-identity-gate.log
```

修复前入口会在 probe 非零后打印 status 并安全 poweroff；判断必须看 guest marker，不能因宿主 QEMU/make
返回 0 就误判通过。修复后两份日志都必须出现 `TASK_PHASE5 ALL PASS`，且不得出现
`TASK_PHASE5_EXPECTED_FAIL` 或 `CURRENT DIFFERENCES CONFIRMED`。

Phase 5 `CLONE_VFORK|CLONE_VM` 共享可见性：

```bash
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=clone05 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-clone05-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=clone05 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-clone05-phase5.log
```

两架构必须顺序运行。musl/glibc 都应出现 `child_exited passed` 和
`SUMMARY: 1 passed, 0 failed`；该 case 同时要求 parent 等到 child 退出和 child 对共享用户变量的
写入可见，但不覆盖 child exec 后的 MM handle 脱离。修改 `share_user_vm()`、exec MM handle 或 vfork
wakeup 后，还要至少跑一个真实的 vfork/exec command workload；短门禁可用 final CAgent，并明确区分
CAgent 完成与随后 BuildStorm 是否被诊断 timeout。

Phase 5 SysV SHM attach/detach 小簇：

```bash
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=shmat01,shmdt02 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-sysv-shm-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=shmat01,shmdt02 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-sysv-shm-phase5.log
```

两架构必须顺序运行。当前 RV64 musl/glibc 与 LA64 musl 应各为 `2 passed, 0 failed`；LA64 glibc 2.38
的 `shmdt02` 通过，但 `shmat01` 会因编译期 64 KiB `SHMLBA` 与当前 Linux/RespOS 4 KiB ABI 不一致而
失败。不得通过按调用者特判或全局改成 64 KiB 消除该差异。该小簇不覆盖同一 segment 的重复/跨进程
attach 共享数据、futex、`IPC_RMID` 后最后 detach 回收或并发 attach/detach。

Phase 5 SysV SHM 跨 attach futex identity：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/sysv_shm_futex_probe_linux.c \
  -o /tmp/sysv_shm_futex_probe_linux
/tmp/sysv_shm_futex_probe_linux

TASK_A_SYSV_SHM_FUTEX_PROBE=1 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-sysv-shm-futex.log
TASK_A_SYSV_SHM_FUTEX_PROBE=1 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-sysv-shm-futex.log
```

Linux 必须输出 `SYSV_SHM_FUTEX_LINUX PASS`。修复前 guest 会输出
`SYSV_SHM_FUTEX_EXPECTED_FAIL wake=0`：第二 attach 已读到 sentinel，但 attach-specific futex key 使
child timeout；该 marker 是反证，不是通过。修复后两架构都必须输出 `SYSV_SHM_FUTEX PASS`，且 runner
输出 `SysV SHM futex probe PASS`。修改 shared futex key 后还要以相同 4 GiB/2 hart 配置顺序运行：

```bash
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=futex_wait01,futex_wake03 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-sysv-shm-futex-regression.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=futex_wait01,futex_wake03 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-sysv-shm-futex-regression.log
```

musl/glibc 都必须为 `SUMMARY: 2 passed, 0 failed`。两架构仍须顺序运行。

Phase 5 SysV SHM `IPC_RMID` 地址空间生命周期：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/sysv_shm_lifecycle_probe_linux.c \
  -o /tmp/sysv_shm_lifecycle_probe_linux
/tmp/sysv_shm_lifecycle_probe_linux

TASK_A_SYSV_SHM_LIFECYCLE_PROBE=1 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-sysv-shm-lifecycle.log
TASK_A_SYSV_SHM_LIFECYCLE_PROBE=1 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-sysv-shm-lifecycle.log
```

Linux 必须输出 `SYSV_SHM_LIFECYCLE_LINUX PASS`；guest 必须依次输出 explicit detach、exit cleanup、
exec cleanup、inherited attach、signal-exit cleanup 五项 PASS，最终输出 `SYSV_SHM_LIFECYCLE PASS`
和 runner PASS。修复前
`SYSV_SHM_LIFECYCLE_EXPECTED_FAIL exit_stale=true exec_stale=true` 是反证。修改该提交点后还须复跑
跨 attach futex probe 和 `clone05,shmat01,shmdt02`；LA64 glibc `shmat01` 的旧 64 KiB `SHMLBA` 差异
仍按既有 runtime 边界判读，不能为了 lifecycle 回归改内核 rounding。

Phase 5 SysV SHM `shm_nattch` MM identity：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 -pthread \
  scripts/sysv_shm_nattch_probe_linux.c -o /tmp/sysv_shm_nattch_probe_linux
/tmp/sysv_shm_nattch_probe_linux

TASK_A_SYSV_SHM_NATTCH_PROBE=1 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-sysv-shm-nattch.log
TASK_A_SYSV_SHM_NATTCH_PROBE=1 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-sysv-shm-nattch.log
```

Linux 必须输出 `SYSV_SHM_NATTCH_LINUX PASS`；guest 必须输出 `SYSV_SHM_NATTCH PASS` 与 runner PASS。
修复前 `SYSV_SHM_NATTCH_EXPECTED_FAIL thread_count=4` 表示两个 attachment 被两个共享 MM 线程重复计数，
不是通过。probe 同时要求同一 MM 重复 attach 为 2、fork 后为 4、child exit 后回到 2、逐次 detach 为
1/0。修改统计后还须双架构复跑 lifecycle probe 和 `shmctl03,shmctl07,shmctl08`；不得仅以 thread
退出后的最终 2 代替其存活窗口验证。

Phase 5 SysV SHM `shmat` 与最后 detach/`IPC_RMID` 线性化：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 \
  scripts/sysv_shm_attach_race_probe_linux.c -o /tmp/sysv_shm_attach_race_probe_linux
/tmp/sysv_shm_attach_race_probe_linux

TASK_A_SYSV_SHM_ATTACH_RACE_PROBE=1 TASK_A_SYSV_SHM_ATTACH_TEST_YIELD=1 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-sysv-shm-attach-race-forced.log
TASK_A_SYSV_SHM_ATTACH_RACE_PROBE=1 TASK_A_SYSV_SHM_ATTACH_TEST_YIELD=1 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-sysv-shm-attach-race-forced.log

TASK_A_SYSV_SHM_ATTACH_RACE_PROBE=1 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-sysv-shm-attach-race-default.log
TASK_A_SYSV_SHM_ATTACH_RACE_PROBE=1 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-sysv-shm-attach-race-default.log
```

Linux 必须输出 `SYSV_SHM_ATTACH_RACE_LINUX PASS shmmax=... shmmni=... pressure=128 attempts=64 ...`。
probe 先确认 `SHMMIN=1`、size 0/`SHMMAX+1` 新建、existing-key 的 size 0/精确/过大查询，以及
`IPC_CREAT|IPC_EXCL` 的 `EEXIST` 优先级；随后以 `IPC_INFO/SHM_INFO` 读取当前配额和用量，补满
`SHMMNI` 后要求额外 `shmget` 返回 `ENOSPC`，删除一个 segment 后要求 replacement 创建成功，最后
用量恢复到测试前。默认 Linux/RespOS 配额为 4096。Linux 运行会短暂占满当前 IPC namespace，必须在
隔离或无其他 SysV SHM workload 的环境中顺序执行。guest 随后把 `/proc/sys/kernel/shmall` 临时设为
2，验证两个单页和一个双页的额度边界、`ENOSPC`、删除后复用，再恢复原值并由 `IPC_INFO` 复核；任何
新增失败出口也必须先恢复该全局值。随后 guest 在已有双页 segment 时把 `SHMALL` 降为 1，并在已有
两个 segment 时把 `SHMMNI` 降为 1：已有对象必须继续由 `IPC_STAT/SHM_INFO` 可见，新建必须阻塞到
用量低于新阈值，两个 sysctl 都须在断言前恢复并复核。最后以共享原子 ready/go 屏障同时释放两个
child，在固定 `SHMALL=1` 和 `SHMMNI=1` 下分别要求恰好一成一败、失败为 `ENOSPC`、全局计数为
1 个 ID/1 页；parent 清理成功对象，恢复并复核 sysctl。强制让出
构建用于稳定扩大 table reservation 与 VMA commit 之间的窗口；修复前
`SYSV_SHM_ATTACH_RACE_EXPECTED_FAIL orphan=64` 是反证，修复后两架构必须输出
`SYSV_SHM_ATTACH_RACE PASS shmmax=... shmmni=... dynamic_limits=pass concurrent_limits=pass
pressure=128 attempts=64 ...` 与 runner PASS。当前门禁
再做 128 轮顺序单页创建/删除/最后 detach 回收复用，再做 32 轮、每轮两个 child 同时 attach；
`invalid`/`attached` 比例不固定，但总和必须为 64 且不得出现 orphan。probe 还要求已占用的非空
`shmaddr` 返回 `EINVAL`，并验证失败 attach 的 reservation 回滚。强制构建后必须不带
`TASK_A_SYSV_SHM_ATTACH_TEST_YIELD` 依次重建和运行两架构，恢复默认 kernel；随后复跑 lifecycle、
nattch 及 `shmat01,shmdt02,shmctl03`。本门禁不覆盖更宽 N 路并发、并发 sysctl/create、IPC namespace、
物理内存/ID 溢出等其他资源边界或 `SHM_REMAP` 并发覆盖。

Phase 5 SysV SHM 核心 metadata（不进入多子进程 teardown）：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 \
  scripts/sysv_shm_metadata_probe_linux.c -o /tmp/sysv_shm_metadata_probe_linux
/tmp/sysv_shm_metadata_probe_linux

TASK_A_SYSV_SHM_METADATA_PROBE=1 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-sysv-shm-lock.log
TASK_A_SYSV_SHM_METADATA_PROBE=1 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-sysv-shm-lock.log
```

Linux 必须输出 `SYSV_SHM_METADATA_LINUX PASS ... mode_access=pass lock=pass`；guest 必须输出
`SYSV_SHM_METADATA PASS ... permission=pass lock=pass` 与 runner PASS。probe 使用 4113-byte segment 同时验证
byte size 与 2-page accounting，覆盖 initial/attach/
detach/`IPC_SET`/marked-removed/最后回收状态，以及 `SHM_STAT` index、`SHM_STAT_ANY` 和 `SHM_INFO`。
Linux mode `0000` 向量要求以无 `CAP_IPC_OWNER` 的普通用户运行；若实际 euid 为 root，probe 会 fork
一个 UID/GID 65534 child 做完整 ownership denial。guest 同样只 fork 一个降权 child 并立即 wait，用于
区分 metadata/权限回归和 LA64 `shmctl01` 的 20-child signal/reap teardown 阻断。owner lock/unlock 只
验证 `SHM_LOCKED` flag，运行 Linux oracle 前应确认 segment 大小不超过当前 `RLIMIT_MEMLOCK`。不得用
本结果宣称 namespace capability、真实 page pinning/memlock accounting 或绝对 realtime timestamp 已通过。

Phase 5 session/`getsid` Linux 对照：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/session_phase5_probe_linux.c \
  -o /tmp/session_phase5_probe_linux
/tmp/session_phase5_probe_linux
```

双架构 no-feature、2 hart 自动 guest 专项：

```bash
TASK_A_SESSION_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-session-phase5.log
TASK_A_SESSION_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-session-phase5.log
```

以 `SESSION_PHASE5_RESPOS ALL PASS` 为通过标志。该 probe 覆盖 query/error、子进程新建 session 后的
父进程查询和 pgrp leader 的 `EPERM`；不覆盖 controlling tty/job control。

Phase 5 socket timeout Linux 对照及 guest 专项：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/socket_timeout_probe_linux.c \
  -o /tmp/socket_timeout_probe_linux
/tmp/socket_timeout_probe_linux
TASK_A_SOCKET_TIMEOUT_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2
TASK_A_SOCKET_TIMEOUT_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2
```

除 `SOCKET_TIMEOUT_RESPOS ALL PASS` 外，还必须检查 50 ms timeout 没有明显早醒或晚醒。2026-08-14
LA64 2 hart 存在约 1 秒晚醒，故当前命令预期暴露未闭合阻断；不能只跑单核或固定 hart0 关闭任务。

Phase 5 socket message flags Linux 对照及 guest 专项：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/socket_flags_probe_linux.c \
  -o /tmp/socket_flags_probe_linux
/tmp/socket_flags_probe_linux
TASK_A_SOCKET_FLAGS_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-socket-flags.log
TASK_A_SOCKET_FLAGS_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-socket-flags.log
```

以 `SOCKET_FLAGS ALL PASS` 为通过标志；probe 覆盖 PEEK 不消费、WAITALL 跨分片等待、timeout/EOF
短读，以及 NOSIGNAL 对 SIGPIPE 的精确抑制。修改 AF_UNIX 等待或 socket flag 解析后，还要以相同 2 hart
配置复跑 `TASK_A_SOCKET_PHASE5_PROBE=1`，不能只看新 probe。

Phase 5 nonblocking connect/`SO_ERROR` Linux 对照及 guest 专项：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/socket_connect_probe_linux.c \
  -o /tmp/socket_connect_probe_linux
/tmp/socket_connect_probe_linux
TASK_A_SOCKET_CONNECT_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-socket-connect.log
TASK_A_SOCKET_CONNECT_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-socket-connect.log
```

以 `SOCKET_CONNECT ALL PASS` 为通过标志。必须同时看到 success、refused、blocking refused 与
`retry_after_refused`；异步失败路径要求 `POLLOUT|POLLERR`、首次 `SO_ERROR=ECONNREFUSED`、第二次
`SO_ERROR=0`，同 fd 重连路径要求随后观察 `ECONNABORTED -> EINPROGRESS -> success` 并实际传输数据。
该 loopback probe 不替代真实 unreachable/SYN timeout/reset 和 iperf 回归。

Phase 5 TCP half-close Linux 对照及 guest 专项：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/tcp_half_close_probe_linux.c \
  -o /tmp/tcp_half_close_probe_linux
/tmp/tcp_half_close_probe_linux
TASK_A_TCP_HALF_CLOSE_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-tcp-half-close.log
TASK_A_TCP_HALF_CLOSE_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-tcp-half-close.log
```

Linux 必须输出 `TCP_HALF_CLOSE_LINUX PASS ... poll_eof=pass rdhup=pass`，guest 输出对应无 `_LINUX` marker 与
runner PASS。向量覆盖未连接/非法 how 的错误、排队数据先于 FIN/EOF、`SHUT_WR` 后反向数据流、dup
共享 shutdown 状态、peer FIN 的 read readiness，以及数据尚未消费时 poll/epoll 同时返回
`IN|RDHUP`。修改 FileOp/poll/epoll readiness 后须同配置复跑 `TASK_A_SOCKET_PHASE5_PROBE=1`。该门禁
不覆盖 AF_UNIX 独立 RDHUP、跨线程阻塞 send/recv、reset/linger 或非 loopback 网络。

Phase 5 `getpeername` 错误优先级 Linux 对照、guest 专项与聚焦 LTP：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/getpeername_probe_linux.c \
  -o /tmp/getpeername_probe_linux
/tmp/getpeername_probe_linux
TASK_A_GETPEERNAME_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-getpeername-named-phase5.log
TASK_A_GETPEERNAME_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-getpeername-named-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=getpeername01,getsockname01 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-unix-address-ltp-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=getpeername01,getsockname01 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-unix-address-ltp-phase5.log
```

专项以 `GETPEERNAME ALL PASS` 为标志；除原错误向量和 unnamed socketpair 外，还必须覆盖双方都绑定的
pathname/含非 UTF-8 字节 abstract 地址、accept/client/accepted 的双向 local/peer 和小 buffer 截断。
LTP 的 musl/glibc 都必须出现 `SUMMARY: 2 passed, 0 failed`。`getpeername01` 七个向量覆盖
`EBADF/ENOTSOCK/ENOTCONN`、connected socketpair
上的负长度 `EINVAL` 以及坏 sockaddr、空/坏 addrlen 指针的 `EFAULT`；自有 probe 还确认未连接 inet
带非法长度仍优先 `ENOTCONN`。修改地址快照或 writer 后还要双架构复跑 2 hart
`TASK_A_SOCKET_PHASE5_PROBE=1`。

Phase 5 AF_UNIX `SO_PEERCRED` Linux 对照、guest 专项与聚焦 LTP：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/socket_peercred_probe_linux.c \
  -o /tmp/socket_peercred_probe_linux
/tmp/socket_peercred_probe_linux
TASK_A_SOCKET_PEERCRED_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-socket-peercred-phase5.log
TASK_A_SOCKET_PEERCRED_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-socket-peercred-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=getsockopt02 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-getsockopt02-ltp.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=getsockopt02 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-getsockopt02-ltp.log
```

专项以 `SOCKET_PEERCRED ALL PASS` 为标志，必须同时验证 socketpair 两端、client 看到 listener 凭据、
accept 端看到 connector 子进程凭据；LTP 的 musl/glibc 都必须出现
`SUMMARY: 1 passed, 0 failed`。修改 AF_UNIX 建链/accept 所有权后还要复跑双架构 2 hart
`TASK_A_SOCKET_PHASE5_PROBE=1`。该流程不验证 `SCM_CREDENTIALS/SO_PASSCRED`。

Phase 5 AF_UNIX→pipe `splice` Linux 对照、guest 专项与 LTP 簇：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/splice_socket_probe_linux.c \
  -o /tmp/splice_socket_probe_linux
/tmp/splice_socket_probe_linux
TASK_A_SPLICE_SOCKET_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-splice-socket-phase5.log
TASK_A_SPLICE_SOCKET_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-splice-socket-phase5.log
TASK_A_LTP_ONLY=1 \
  LTP_CASE_FILTER=splice01,splice02,splice03,splice04,splice05,splice06,splice07 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-splice-cluster-phase5.log
TASK_A_LTP_ONLY=1 \
  LTP_CASE_FILTER=splice01,splice02,splice03,splice04,splice05,splice06,splice07 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-splice-cluster-phase5.log
```

专项以 `SPLICE_SOCKET ALL PASS` 为标志，必须同时看到 unconnected AF_UNIX=`EINVAL`、unconnected
inet=`ENOTCONN`、错误 pipe 方向=`EBADF` 和 connected AF_UNIX 实际传输；LTP 的 musl/glibc 都必须
出现 `SUMMARY: 7 passed, 0 failed`，且 `splice07` 内部为 `passed 159, failed 0`。修改 FileOp splice
预检或 AF_UNIX read 状态后还要复跑双架构 2 hart `TASK_A_SOCKET_PHASE5_PROBE=1`。

Phase 5 musl `pathconf()` 调用链镜像审计：

```bash
debugfs -R 'dump /musl/lib/libc.so /tmp/respos-rv-musl-libc.so' img/sdcard-rv-pre.img
readelf -Ws /tmp/respos-rv-musl-libc.so | rg ' (f?pathconf)$'
rust-objdump --disassemble-symbols=pathconf,fpathconf /tmp/respos-rv-musl-libc.so

debugfs -R 'dump /musl/lib/libc.so /tmp/respos-la-musl-libc.so' img/sdcard-la-pre.img
readelf -Ws /tmp/respos-la-musl-libc.so | rg ' (f?pathconf)$'
rust-objdump --disassemble-symbols=pathconf,fpathconf /tmp/respos-la-musl-libc.so
```

该流程只读审计实际 guest libc，不修改 raw image。当前两架构都应看到 8-byte
`pathconf` 将第一参数改为 `-1` 并跳转 `fpathconf`，而 `fpathconf` 只读常量表；这是
musl `pathconf02` 的已知差异定位流程，不是通过门禁。只有经协商替换 musl runtime
后，才应以下列双架构命令验收，并继续跑完整 musl workload：

```bash
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=pathconf01,pathconf02 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=pathconf01,pathconf02 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2
```

LA64 musl `readlink*()` 零长度差异可复用上述导出的 libc 继续审计：

```bash
strings -a /tmp/respos-la-musl-libc.so | rg '^1\.[0-9]+\.[0-9]+$'
rust-objdump --disassemble-symbols=readlink,readlinkat /tmp/respos-la-musl-libc.so
strings -a /tmp/respos-rv-musl-libc.so | rg '^1\.[0-9]+\.[0-9]+$'
rust-objdump --disassemble-symbols=readlink,readlinkat /tmp/respos-rv-musl-libc.so
```

当前 LA64 musl 1.2.5 应显示零长度分支把 buffer 换成栈地址、size 改为 1；RV64
musl 1.2.0 则直接发起 `readlinkat` syscall。内核 `sys_readlinkat()` 已对实际传入的 size 0
返回 `EINVAL`，不得为让 LA64 musl `readlink03/readlinkat02` 通过而拒绝所有 size 1
的合法截断读。

RV64 musl `epoll_create()` invalid-size 差异使用同一份镜像导出物审计：

```bash
rust-objdump --disassemble-symbols=epoll_create /tmp/respos-rv-musl-libc.so
rust-objdump --disassemble-symbols=epoll_create /tmp/respos-la-musl-libc.so

TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=epoll_create02 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-epoll-create-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=epoll_create02 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-epoll-create-phase5.log
```

RV64 musl 1.2.0 应显示清零 `a0` 后直接跳到 `epoll_create1`，其 `epoll_create02` libc
variant 失败；LA64 musl 1.2.5 应先判断 `size <= 0` 并返回 `EINVAL`，与两架构 glibc 一样通过。
raw syscall variant 在两架构均因没有 legacy `__NR_epoll_create` 而 `TCONF`。不得修改内核
`sys_epoll_create1()` 去拒绝 flags 0；该值是现代 ABI 的合法无 flag 调用。

musl `recvmmsg()` bad-vector 调用链审计与错误矩阵：

```bash
rust-objdump --disassemble-symbols=recvmmsg /tmp/respos-rv-musl-libc.so
rust-objdump --disassemble-symbols=recvmmsg /tmp/respos-la-musl-libc.so

TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=recvmmsg01 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-recvmmsg-errors-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=recvmmsg01 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-recvmmsg-errors-phase5.log
```

两份 musl 反汇编都应显示 syscall 前以 64-byte stride 遍历 `msgvec`，并向 offset 28/44 写零，
即规范化 `msg_iovlen/msg_controllen` 的高 32 位；因此 LTP bad-vector 项在 wrapper 内 SIGSEGV，
只能记为 libc/LTP 阻断。两架构 glibc 应各自让
libc-time 与 old-kernel-time 两种 variant 共 10 项全部通过，确认真正进入内核的
`EBADF/EFAULT/EINVAL` 错误矩阵。该命令不验证阻塞 deadline、partial result 或 LA64 跨 hart timeout。

Phase 5 `pwrite()` + `O_APPEND` Linux 对照与 pwrite/pwritev 簇：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/pwrite_append_probe_linux.c \
  -o /tmp/pwrite_append_probe_linux
/tmp/pwrite_append_probe_linux

TASK_A_LTP_ONLY=1 \
  LTP_CASE_FILTER=pwrite01,pwrite01_64,pwrite02,pwrite02_64,pwrite03,pwrite03_64,pwrite04,pwrite04_64,pwritev01,pwritev01_64,pwritev02,pwritev02_64,pwritev201,pwritev201_64,pwritev202,pwritev202_64 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-pwrite-phase5.log
TASK_A_LTP_ONLY=1 \
  LTP_CASE_FILTER=pwrite01,pwrite01_64,pwrite02,pwrite02_64,pwrite03,pwrite03_64,pwrite04,pwrite04_64,pwritev01,pwritev01_64,pwritev02,pwritev02_64,pwritev201,pwritev201_64,pwritev202,pwritev202_64 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-pwrite-phase5.log
```

宿主 probe 必须证明 append 写到旧 EOF、显式 offset 未覆盖原文件、payload 位于追加区，
且 open-file offset 不变；清除 `O_APPEND` 后仍要保持普通定位覆盖。两份 guest 日志中
musl/glibc 都必须出现 `SUMMARY: 16 passed, 0 failed`。LTP `pwrite04` 测的是 Linux
对 POSIX 文本的已知偏离，覆盖矩阵应显式标记为 Linux ABI 兼容，不宣称为纯 POSIX
行为。两架构必须顺序运行，因为 build target 会改写共享 Cargo config。

Phase 5 `O_APPEND pwrite` 整 syscall 并发原子性：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 \
  scripts/pwrite_append_atomic_probe_linux.c -o /tmp/pwrite_append_atomic_probe_linux
/tmp/pwrite_append_atomic_probe_linux

TASK_A_PWRITE_APPEND_ATOMIC_PROBE=1 TASK_A_PWRITE_APPEND_TEST_YIELD=1 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-pwrite-append-atomic-forced.log
TASK_A_PWRITE_APPEND_ATOMIC_PROBE=1 TASK_A_PWRITE_APPEND_TEST_YIELD=1 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-pwrite-append-atomic-forced.log

TASK_A_PWRITE_APPEND_ATOMIC_PROBE=1 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-pwrite-append-atomic-default.log
TASK_A_PWRITE_APPEND_ATOMIC_PROBE=1 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-pwrite-append-atomic-default.log
```

Linux 32 轮必须输出 `PWRITE_APPEND_ATOMIC_LINUX PASS`。guest 每轮由共享 open-file description 的
parent/child 并发写两个 128 KiB record；最终只能是完整 A+B 或 B+A。修复前强制让出构建应输出
`PWRITE_APPEND_ATOMIC_EXPECTED_FAIL interleaved=16 rounds=16`，它是反证而非通过；默认构建是否自然
命中受调度影响。强制构建后必须不带 `TASK_A_PWRITE_APPEND_TEST_YIELD` 依次重建两架构以恢复默认
kernel。实现修复后，强制与默认四次运行都必须输出 `PWRITE_APPEND_ATOMIC PASS`，并复跑上方 16-case
pwrite/pwritev 簇；还需另补不同 open description、EFAULT/short-write 与 truncate 竞态，不能用持有
spin lock 跨 usercopy 的方式通过该 probe。

Phase 5 已删除目录 fd 的 `getdents64()` Linux 对照与 LTP：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/getdents_unlinked_probe_linux.c \
  -o /tmp/getdents_unlinked_probe_linux
/tmp/getdents_unlinked_probe_linux
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=getdents01,getdents02 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-getdents-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=getdents01,getdents02 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-getdents-phase5.log
```

宿主 probe 同时覆盖 rmdir 前未读目录流、先 getdents+lseek 建立过遍历状态的两种 fd；
删除后都必须返回 `ENOENT`，且未删除目录的 1-byte buffer 必须返回 `EINVAL`。两份
guest 日志中 musl/glibc 都必须出现 `SUMMARY: 2 passed, 0 failed`，`getdents02`
内部应显示 `EBADF/EINVAL/ENOTDIR/ENOENT` 通过。修改目录 cache 后不能只跑错误项；
`getdents01` 还要确认普通 `.`/`..` 和文件遍历未回归。

Phase 5 `chroot()` pathname/permission/privilege 错误优先级：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/chroot_permission_probe_linux.c \
  -o /tmp/chroot_permission_probe_linux
/tmp/chroot_permission_probe_linux

TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=chroot01,chroot02,chroot03,chroot04 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-chroot-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=chroot01,chroot02,chroot03,chroot04 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-chroot-phase5.log
```

宿主 probe 应输出 `CHROOT_PERMISSION_LINUX_PASS`，证明不可搜索目录和 missing path 分别返回
`EACCES/ENOENT`，只有可访问目录返回 `EPERM`。两份 guest 日志的 musl/glibc 均必须出现
`SUMMARY: 4 passed, 0 failed`，其中 `chroot04` 明确显示 `EACCES`。两架构必须顺序运行。

### mknod 特殊 inode 与 xattr 门禁

专项 probe 固定 character/block device payload、四类特殊 inode mode 和 `user.*` xattr 限制；随后用
同一筛选簇覆盖常规/错误路径。两架构必须顺序运行：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/mknod_dev_t_probe_linux.c \
  -o /tmp/mknod_dev_t_probe_linux
/tmp/mknod_dev_t_probe_linux

TASK_A_MKNOD_XATTR_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-mknod-xattr-phase5.log
TASK_A_MKNOD_XATTR_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-mknod-xattr-phase5.log

TASK_A_LTP_ONLY=1 \
  LTP_CASE_FILTER=mknod01,mknod02,mknod03,mknod04,mknod05,mknod06,mknod07,mknod08,mknod09,setxattr01,setxattr02,fsetxattr01,getxattr02 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-mknod-xattr-ltp-phase5.log
TASK_A_LTP_ONLY=1 \
  LTP_CASE_FILTER=mknod01,mknod02,mknod03,mknod04,mknod05,mknod06,mknod07,mknod08,mknod09,setxattr01,setxattr02,fsetxattr01,getxattr02 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-mknod-xattr-ltp-phase5.log

TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=mknod01,setxattr02,statx02,statx03 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-mknod-devt-ltp-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=mknod01,setxattr02,statx02,statx03 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-mknod-devt-ltp-phase5.log
```

宿主若没有 `CAP_MKNOD`，probe 会打印 runtime skip，但仍验证 libc `dev_t` 编码；真实节点的 high-minor
stat/statx 对照必须由双架构 guest 专项完成。专项日志必须包含 `MKNOD_XATTR_PROBE_PASS`；完整 LTP
日志的 musl/glibc 均须为
`SUMMARY: 13 passed, 0 failed`，且 `setxattr02` 的 regular/directory 成功、symlink `EEXIST`、
FIFO/character/block/socket `EPERM` 七项全部通过；device/statx 小簇必须为
`SUMMARY: 4 passed, 0 failed`。该门禁不验证字符/块设备驱动功能。

### fallocate 真实预分配门禁

宿主 probe 同时检查逻辑长度、物理块数、零读、原数据和 open-file offset，避免把 LTP 仅检查返回值的
`fallocate03` 误当成完整语义证明：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 \
  scripts/fallocate_prealloc_probe_linux.c \
  -o /tmp/fallocate_prealloc_probe_linux
/tmp/fallocate_prealloc_probe_linux

TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=fallocate03 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-fallocate-phase5.log
TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=fallocate03 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-fallocate-phase5.log
```

Phase 5 iperf→iozone 固定顺序门禁：

```bash
timeout 240s env TASK_A_NETWORK_ORDER_PROBE=1 \
  make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-network-order-phase5.log
timeout 240s env TASK_A_NETWORK_ORDER_PROBE=1 \
  make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-network-order-phase5.log
```

日志必须包含 musl/glibc 的 BASIC/PARALLEL/REVERSE UDP/TCP 共 12 个 `end: success`、三个测试组的
`GROUP END`，以及 `[testrunner] network order probe finished, powering off`。该入口保留 iperf daemon，
随后完整运行 glibc iozone，用于同时检查网络模式和 daemon 存活时的无关 timer/I/O 前进。2026-08-14
RV64 2 hart 与 LA64 1 hart 通过；LA64 2 hart 连续两轮停在 musl BASIC_UDP 后的 BASIC_TCP connect，
当前预期暴露未闭合阻断，不能以单核结果关闭 M1。

Phase 5 mmap EOF/SIGBUS Linux 对照：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/mmap_phase5_probe_linux.c \
  -o /tmp/mmap_phase5_probe_linux
/tmp/mmap_phase5_probe_linux

TASK_A_MMAP_PHASE5_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-mmap-phase5.log
TASK_A_MMAP_PHASE5_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-mmap-phase5.log
```

两架构命令必须顺序运行，因为 build target 会改写共享 Cargo config。当前 no-feature guest 可通过
上述 `testrunner` 入口运行 `mmap_phase5_probe`。修复前预期打印 shared/private 的
`MMAP_PHASE5_EXPECTED_FAIL` 和 `MMAP_PHASE5 CURRENT DIFFERENCES CONFIRMED`，并以非零状态退出；它们
覆盖初始 EOF 整页 SIGBUS、truncate 后 resident PTE 失效、未 COW EOF 部分页清零、已 COW private
部分页保留匿名字节，以及 mmap 后动态扩容。完成 MM 修复后必须同时出现 `MMAP_PHASE5 shared PASS`、
`MMAP_PHASE5 private PASS`、`MMAP_PHASE5 private_cow_truncate PASS` 与
`MMAP_PHASE5 ALL PASS`，并复跑 `buildstorm_file_probe` 的 mmap 扩容/写回竞态。宿主 `make`/QEMU
管线即使在 guest probe 非零后仍可能返回 0，因此必须以 guest marker 判定，不能只看宿主退出状态。

Phase 5 `mprotect()` 失败权限边界：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/mprotect_failure_probe_linux.c \
  -o /tmp/mprotect_failure_probe_linux
/tmp/mprotect_failure_probe_linux

TASK_A_MPROTECT_FAILURE_PROBE=1 make run-rv-pre PRE_MEM=4G PRE_SMP=2 \
  RV_PRE_OUTPUT=/tmp/respos-rv-mprotect-failure.log
TASK_A_MPROTECT_FAILURE_PROBE=1 make run-la-pre PRE_MEM=4G PRE_SMP=2 \
  LA_PRE_OUTPUT=/tmp/respos-la-mprotect-failure.log
```

Linux 必须输出 `MPROTECT_FAILURE_LINUX PASS einval_atomic=pass eacces_write=pass hole_enomem=pass`，
guest 必须输出不带 `_LINUX` 的同组 marker 与 runner PASS。probe 用 fork child 实际尝试写入，不以
`mprotect()` 返回值替代权限验证：未知 prot 与未对齐地址的 `EINVAL` 后，原只读/可写页面必须保持；
只读 fd 的 `MAP_SHARED` 升级写权限返回 `EACCES` 且仍以 `SIGSEGV` 拒绝写；跨 unmapped hole 返回
`ENOMEM`。POSIX 允许非 `EINVAL` 失败改变部分页面，因此 hole 向量只断言 errno，不添加整段回滚要求。
两架构命令仍须顺序运行。

Phase 5 signal ABI 对照：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/signal_phase5_probe_linux.c \
  -o /tmp/signal_phase5_probe_linux
/tmp/signal_phase5_probe_linux
```

无 feature guest 中运行 `signal_phase5_probe`，以 `SIGNAL_PHASE5 ALL PASS` 为通过标志。它覆盖
query-only `rt_sigprocmask`、`rt_sigaction` size/EFAULT、`rt_sigqueueinfo(sig=0)` 和 pending signal
跨 exec。修改 signal wait/timer 唤醒后还应复跑 `task_a_clock_probe` 与
`/glibc/busybox timeout 1 /glibc/busybox sleep 10`；只通过 ABI 探针不能替代阻塞路径门禁。

CPU clock/CPU timer 专项使用无 feature release；两架构必须顺序构建：

```bash
TASK_A_LTP_ONLY=1 \
LTP_CASE_FILTER=clock_getres01,clock_gettime01,clock_gettime02,timer_delete01,timer_settime01,timer_settime02 \
RUSTUP_TOOLCHAIN=nightly-2025-01-18 \
make run-rv-pre RV_PRE_OUTPUT=/tmp/respos-rv-cpu-clock-cluster.log

TASK_A_LTP_ONLY=1 \
LTP_CASE_FILTER=clock_getres01,clock_gettime01,clock_gettime02,timer_delete01,timer_settime01,timer_settime02 \
RUSTUP_TOOLCHAIN=nightly-2025-01-18 \
make run-la-pre LA_PRE_OUTPUT=/tmp/respos-la-cpu-clock-cluster.log

TASK_A_CLOCK_PROBE=1 RUSTUP_TOOLCHAIN=nightly-2025-01-18 \
make run-rv-pre PRE_SMP=2 RV_PRE_OUTPUT=/tmp/respos-rv-cpu-clock-probe-smp2.log
TASK_A_CLOCK_PROBE=1 RUSTUP_TOOLCHAIN=nightly-2025-01-18 \
make run-la-pre PRE_SMP=2 LA_PRE_OUTPUT=/tmp/respos-la-cpu-clock-probe-smp2.log
```

LTP 必须逐项检查五个目标的退出值；RV64 glibc 在冷缓存下运行的第一个动态程序可能先触发独立的
loader file-fault `SIGBUS`，不能把后续目标通过伪装成整组全绿。SMP probe 每轮必须同时出现
`process/thread CPU clocks PASS`、`process aggregation PASS`、`ALL PASS`，共 20 轮。

ext4 命名 FIFO 专项使用无 feature release，并把日志写到 `/tmp`，避免覆盖完整初赛日志：

```bash
TASK_A_LTP_ONLY=1 \
LTP_CASE_FILTER=fsync03,lseek02,open06,read03,write04 \
RUSTUP_TOOLCHAIN=nightly-2025-01-18 \
make run-rv-pre RV_PRE_OUTPUT=/tmp/respos-rv-fifo-ltp.log

TASK_A_LTP_ONLY=1 \
LTP_CASE_FILTER=fsync03,lseek02,open06,read03,write04 \
RUSTUP_TOOLCHAIN=nightly-2025-01-18 \
make run-la-pre LA_PRE_OUTPUT=/tmp/respos-la-fifo-ltp.log
```

两架构必须顺序运行，因为 build target 会改写共享 Cargo config。每个日志应同时包含 musl/glibc
`SUMMARY: 5 passed, 0 failed, 0 skipped, 5 selected` 和最终 poweroff；还要逐项确认 `open06=ENXIO`、
`read03/write04=EAGAIN`、`lseek02=ESPIPE`、`fsync03=EINVAL`，不能只看 case exit 0。

Phase 5 AF_UNIX、pipe 与 poll 对照：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/socket_phase5_probe_linux.c \
  -o /tmp/socket_phase5_probe_linux
/tmp/socket_phase5_probe_linux
```

RV64 no-feature release、`-m 16G -smp 8 -snapshot` guest 中运行 `socket_phase5_probe`，以
`SOCKET_PHASE5 ALL PASS` 为通过标志。它覆盖 pathname accept/connect、非阻塞 accept、EOF/EPIPE、
AF_UNIX buffered shutdown 的 poll/epoll `IN|RDHUP`、阻塞 ppoll 数据唤醒、pipe 的无条件 HUP/ERR、
epoll HUP 和 accept EINTR。Linux 还必须输出 `SOCKET_PHASE5_LINUX shutdown_poll_rdhup PASS`。修改
Unix buffer/waiter 后还必须复跑 `unix_socket_block_probe` 的 128 KiB 满缓冲区传输；宿主 QEMU
需要单独核对 `NI=-10/CLS=TS`。

题一 CAgent 的 guest 入口是 `/glibc/cagent_testcode.sh`，不是内置 `testrunner`。它会启动
`simple_llm_server`，并行运行 10 个 `agent_lite`，最终由上游
[`judge_cagent-glibc.py`](https://github.com/oscomp/testsuits-for-oskernel/blob/final-2026/judge/judge_cagent-glibc.py)
解析 `testcase cagent ...` 记录。当前先在 `SMP=1` 下验证该脚本；上游规则中明确的 `-smp 8`
属于题二 BuildStorm，不是题一的前置条件。

### CAgent 保留日志的单项诊断

- 状态：已验证（RV64）
- 适用范围：CAgent 固定命令、agent/popen/validation 分层诊断
- 最后验证：2026-08-03
- 证据：`scripts/cagent_debug.sh`；RV64 `kernel` 单项和全量 debug 运行
- 内容：runner 应位于 guest 的 `/glibc` 并从该目录运行。它接受测试名或 `all`，结果保存在
  `/tmp/cagent_debug_<run-id>/`，不会像官方脚本一样删除日志。若用 `debugfs` 把 runner 临时写入
  刚恢复的镜像，必须先回放 journal，否则启动时的 journal recovery 可能覆盖新 inode：

```bash
gzip -dkf img/sdcard-rv-pub.img.gz
e2fsck -pf img/sdcard-rv-pub.img || test $? -eq 1
debugfs -w -R \
  'write scripts/cagent_debug.sh /glibc/cagent_debug.sh' \
  img/sdcard-rv-pub.img
debugfs -w -R \
  'set_inode_field /glibc/cagent_debug.sh mode 0100755' \
  img/sdcard-rv-pub.img
```

guest 中运行：

```bash
cd /glibc
./cagent_debug.sh kernel
```

- 后续影响：正式评分回归仍使用镜像原有 `/glibc/cagent_testcode.sh`；debug runner 和日志不写入
  官方测例、不提交进镜像。

### GDB

- 状态：已确认
- 适用范围：早期启动、trap、页表和内核崩溃
- 最后验证：2026-08-01
- 证据：`os/Makefile`
- 内容：

```bash
make -C os gdbserver ARCH=riscv64 MODE=debug
make -C os gdbserver ARCH=loongarch64 MODE=debug
# 另一终端：
make -C os gdbclient ARCH=riscv64 MODE=debug
```

## 聚焦 LTP

### 从清单生成测试集合

- 状态：已确认
- 适用范围：LTP musl/glibc
- 最后验证：2026-08-01
- 证据：`user/oscomp_ltp_list.txt`、`user/build.rs`、`user/src/bin/testrunner.rs`
- 内容：`user/build.rs` 把清单按 phase 生成进 `OUT_DIR/ltp_cases.rs`；musl/glibc 共用选择逻辑。
  可用环境变量缩小构建进内核的集合：

```bash
LTP_CASE_FILTER=confstr01,mmap14 make rv
LTP_CASE_FILTER=confstr01,mmap14 make la
```

- 后续影响：修改清单或 filter 会触发 user build script；确认日志确实只运行目标 case。

### 解析与 baseline 对比

- 状态：已确认
- 适用范围：完整或聚焦 LTP 日志
- 最后验证：2026-08-01
- 证据：`judge/ltp_report.py`、`judge/ltp_compare.py`、`scripts/gen_ltp_csv.sh`
- 内容：

```bash
python3 judge/ltp_report.py rv-output.txt la-output.txt --format markdown
bash scripts/gen_ltp_csv.sh rv-output.txt la-output.txt
python3 judge/ltp_report.py rv-output.txt la-output.txt \
  --format csv -o /tmp/respos-ltp-report.csv
python3 judge/ltp_compare.py \
  --baseline judge/baseline/ltp-linux-baseline.csv \
  --respos /tmp/respos-ltp-report.csv \
  --case-list user/oscomp_ltp_list.txt
```

`scripts/gen_ltp_csv.sh` 会清理并重建 `judge/local-report/` 与 `judge/local-compare/` 下的 CSV，
运行前留意这些路径是否有需要保留的人工结果。

- 后续影响：优先看首个真实失败；大量后续 TBROK 可能是 harness 初始化失败的级联。

## 检查镜像内文件

### 不挂载读取脚本

- 状态：已确认
- 适用范围：测试脚本、loader、依赖文件诊断
- 最后验证：2026-08-01
- 证据：历史调试流程；镜像为 ext4
- 内容：

```bash
debugfs -R 'cat /musl/ltp_testcode.sh' img/sdcard-rv.img
debugfs -R 'ls -l /musl/ltp/testcases/bin' img/sdcard-rv.img
```

- 后续影响：只读诊断优先使用 `debugfs -R`；不要为查看一个脚本而写入镜像。

## 日志快速筛查

```bash
rg -n 'SUMMARY:|TBROK|Segmentation fault|Assert Fatal|end: fail|exited with code|panic' \
  rv-output.txt la-output.txt
tail -n 120 rv-output.txt
tail -n 120 la-output.txt
```

完整运行后还应执行：

```bash
git status --short
git diff --check
```

原因是架构配置会被复制，镜像会被写入，且本地未跟踪 `user/src/bin/*.rs` 也会被 wildcard 构建。

## 决赛 BuildStorm 分层验证

### BuildStorm 性能计数器

2026-08-09 起可构建带低量原子计数的诊断 kernel；正式成绩对照仍应再跑一轮无 feature 内核：

```bash
make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=perf_counters
# guest 中在目标计时区间开始前清零，结束后读取
/bin/busybox echo reset > /proc/respos_perf
cat /proc/respos_perf
```

LA64 对应使用 `LA_USER_FEATURES=` 和 `LA_KERNEL_FEATURES=perf_counters`。输出中的 `ticks` 使用同一行
`clock_hz` 换算；block 平均请求大小可由 bytes/requests 计算。读取 proc 文件本身会产生少量 task、
heap 和文件关闭活动，因此分析大工作负载时忽略最后一次读取造成的常数扰动。未带 feature 的 kernel
仍保留该 proc 路径，但只输出 `enabled=0`。

heap 分桶按 allocator 实际服务尺度 `max(Layout::size, Layout::align)` 选择，`upper=0` 表示
`>4096` 的无界末桶；`alloc_bytes/dealloc_bytes` 仍是调用者请求字节，不是 buddy 向上取整后的占用。
各 hart 独立更新桶和 heap 总时长，读取时求和；`heap_class_totals match_totals=1` 表示同一次只读
分桶快照生成的兼容总量闭合。运行中的并发写入不提供跨 hart 的瞬时原子快照，正式采样应在 timeout
子进程已经 wait 完成或 guest 即将退出的静止点读取。`heap_current_bytes/heap_peak_bytes` 来自 allocator
锁内的 live requested-byte 统计，reset 将 peak 基线设置为当时 current。calls/bytes 精确逐次累计；
heap 的 total/wait/core ticks 为各 hart 每 64 次操作抽取一次并乘采样率的估算值，必须同时报告
`heap_timing_sample_rate` 和 alloc/dealloc sample 数。max ticks 是被抽中样本的实际最大值，不乘 64，
不能视为全窗口精确极值。

任何新增高频计数器先做观测开销校准：同一 commit、冷 snapshot、QEMU 参数、宿主优先级和相同编译
marker 下，交替运行 `perf_counters` 与 no-feature；比较两个 dev 阶段、`BUILDSTORM_BEGIN` 到
`compile_core_begin`、完成 crate 数和宿主核秒。阶段时延中位数超过 3% 时不得用该计数版本指导 A/B，
应先分片、采样或降低更新频率。一次相邻样本只能筛选，最终结论至少需要交替多轮或完整无 feature
成绩；QEMU 未启动、串口为空或时间轴目录缺失的 timeout 样本直接作废。

LA 本地 12 GiB 校准先分别构建 feature/no-feature kernel，再各自从同一只读根盘和重建后的同内容
diagnostic/final 辅助盘启动。展开后的运行命令应显式固定 `-m 12G -smp 12 -snapshot`，外层使用：

```bash
timeout 130s env \
  RESPOS_PERF_SERIAL_LOG=/tmp/respos-la-calibration.log \
  RESPOS_PERF_TIMELINE_DIR=/tmp/respos-la-calibration-timeline \
  scripts/run_performance_command.sh qemu-system-loongarch64 QEMU_ARGS... \
  |& tee /tmp/respos-la-calibration.log
```

`QEMU_ARGS...` 必须用实际完整参数替换，不能原样执行；feature/no-feature 使用不同日志目录。若宿主无法
分配默认 36 GiB，必须显式降到记录在 metadata 中的 12 GiB，不能让 QEMU 失败后仍等待 timeout 并把
空文件当样本。

LA64 的可复现短窗口可先生成不参与提交的 diagnostic 辅助盘：

```bash
make build-disks AUX_FS_DIR=respos-diagnostic \
  RV_DISK_IMG=/tmp/respos-rv-diagnostic.img \
  LA_DISK_IMG=/tmp/respos-la-diagnostic.img
make la LA_FS_IMG=img/sdcard-la-pub.img \
  LA_DISK_IMG=/tmp/respos-la-diagnostic.img \
  LA_USER_FEATURES= LA_KERNEL_FEATURES=perf_counters
```

进入 shell 后将 reset、限时工作、读取计数和 quit 一次性预排，避免 shell stdin 空轮询污染：

```text
/> /glibc/busybox echo reset > /proc/respos_perf
/> /glibc/busybox timeout 30 /bin/bash /glibc/buildstorm_testcode.sh
/> /glibc/busybox cat /proc/respos_perf
/> quit
```

旧 pub 脚本的前置 toolchain/minibuild/`tg-xtask` 也包含在这个 30 秒窗口内，因此该窗口用于热点和
进展对比，不是正式 BuildStorm 计时。带 `perf_counters` 的正常关机还会输出 `[perf shutdown]` 快照；
正式无 feature 内核没有这项输出或原子计数开销。

修改 signal wait 或 timeout wakeup 后，先用短命令检查功能与调度计数；工作命令、proc read 和 quit
必须一次性预排，避免 user_shell stdin 空轮询混入窗口：

```text
/> /bin/busybox echo reset > /proc/respos_perf
/> /bin/busybox timeout 3 /bin/busybox sleep 60
/> cat /proc/respos_perf
/> quit
```

期望约 3 秒返回，`signal_time=0`、`scheduler_yields=0`，并出现少量 `blocking_switches`。真实 Cargo
固定窗口同样把 reset、`timeout N cargo ...`、proc read、quit 预排；旧实现每分钟约 196 万次
`signal_time` yield 可作为回归量级。专项还应补充非目标信号产生 `EINTR` 的用例；busybox timeout
只覆盖当前 BuildStorm 使用的主要路径。

定位 lwext4 读取放大时，计数内核还会输出 `block_read_sizes_*`、`page_cache_fill_*` 和
`inode_read_*`。使用相同冷 snapshot 和固定时长；计算：

```text
block amplification = block_read_bytes / inode_read_completed_bytes
```

若大多数请求在 `le4k` 且放大倍数远大于 1，应先检查 lwext4 metadata cache/path lookup，不要把所有
读取都归因于 PageCache miss。调整 `CONFIG_BLOCK_DEV_CACHE_SIZE` 时同时比较固定窗口内
`page_cache_fill_bytes`（进度代理）、ext4 acquisitions/hold 和 heap current/peak；容量更小产生的块
读取略少但完成工作也更少，并不代表更快。普通数据由 direct multi-block path读取，因此当前 4096 项
主要是元数据预算，约占 16 MiB kernel heap。

当 block read ticks 已远小于 ext4 lock hold 时，使用 `ext4_ops_*` 分桶比较 stat、lookup、readdir、
create、write 的 calls/ticks，并计算每次平均时间。当前旧镜像 tg-xtask 前段已验证的判读示例是：stat
和 lookup 合计占约 84% lock hold，说明应减少 pathname walk，而不是继续扩大 block cache。修改 raw
inode/dirent 快路径后至少运行：

```text
/> /bin/busybox stat /work/tgoskits/Cargo.toml
/> buildstorm_file_probe
/> buildstorm_private_map_probe
/> smp_shared_mm_probe
/> frame_reclaim_probe
```

`fs_namespace_probe` 还必须输出 `FS_NAMESPACE_DIRENT_TYPE_PASS`，其真实 `getdents64` 解析需确认
目录、普通文件和相对 symlink 分别为 `DT_DIR/DT_REG/DT_LNK`。计数内核同时检查
`ext4_readdir_dirent_type_known/unknown`：当前公开 RV/LA 镜像的 BuildStorm 短窗口应以 known 为主，
但 UNKNOWN 必须继续走 child pathname mode 回退，不能把 `unknown=0` 当作删除兼容路径的依据。
stat 输出需核对 size/inode/mode/uid/gid；最终还需覆盖 symlink size、uid/gid 高位、超过 4 GiB size 和
LTP stat/fstatat。固定窗口比较必须同时报告阶段进度；若进度不同，优先报告每次 readdir/lower call 的
归一化 ticks，并补一个操作计数相同的目录遍历 A/B，不能因优化版执行了更多或更少工作只比较累计值。

当 stat/lookup 降低后，继续查看 `ext4_lock_*_by_class`。class 只用于分析，所有 class 仍必须串行于
同一把 `EXT4_OP_LOCK`；不能据此拆成多把锁。若 attributes 占主要 hold，同时对照每类 acquisitions：
当前工作负载曾有约 5.8 万次 attributes 获取，而 namespace 仅约 375 次。检查一次 VFS 操作是否连续调用
多个 lwext4 pathname API；合并底层 inode transaction 后再用同镜像、同阶段的固定窗口比较 hold、block
write requests 与 PageCache fill。时间戳优化还需保留显式 utimens、unlink 后打开 fd 和跨 reopen 语义。

开启 inode raw-metadata 跨 syscall 缓存后，不能只用重复只读 stat 证明正确。在 `-snapshot` 的 ext4
目录中必须先 stat 填充缓存，再依次执行 append、truncate、chmod/chown、hardlink/unlink 和
rename，每步重新核对 size/mode/uid/gid/nlink/inode。同时读取 `/proc/respos_perf` 的
`stat_cache_hits/misses`；命中率只证明缓存生效，没有同阶段、同宿主负载旧版对照时不报加速比。
目录缓存还必须验证 mkdir/rmdir 的父目录 nlink、普通创建、跨目录 rename 和延迟 orphan
cleanup；它们必须通过 namespace generation 使旧快照失效。

调整 `DENTRY_CACHE_CAPACITY` 时，打开 `perf_counters` 并同时报告 `dentry_cache_hits/misses/evictions`、
`ext4_ops_lookup_calls/ticks`、stat miss、PageCache fill/registry 和 heap peak。dentry 持强引用会同时延长
inode 及 PageCache identity，所以 fill 下降可能是避免重读而非负载进展变少；仍需对照编译阶段。

分析 kernel heap 时使用 `heap_*_lock_wait_ticks` 与 `heap_*_core_ticks` 拆分总耗时；8 核累计 ticks
可以超过墙钟，不能当作单核 elapsed。修改仓库内 allocator 后先运行：

```bash
cargo test --manifest-path vendor/respos_buddy_allocator/Cargo.toml
make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=
make build-la LA_USER_FEATURES= LA_KERNEL_FEATURES=
```

然后在 RV64 1 GiB/8 核无 feature snapshot 同一轮运行 file/private-map/shared-MM/frame-reclaim probes，
最后再进真实 Cargo 固定窗口。`copy_from_user_*`/`copy_to_user_*` 用来判断用户复制是否值得重构；必须同时
报告 calls、bytes、ticks。仅观察到 syscall 使用 bounce buffer 不构成零拷贝依据，prepared user pages
还需覆盖 EFAULT-before-side-effect、lazy/COW、short I/O、共享 offset 和并发 munmap。

allocator magazine 实验使用独立 `heap_magazine` feature；当前 A2 完整 A/B 未过门槛，必须默认关闭。
诊断路径和生产 A/B
必须分开构建：

```bash
# 路径、命中和 cache 上限诊断
make build-la LA_USER_FEATURES= LA_KERNEL_FEATURES='heap_magazine perf_counters'

# 墙钟候选；不要带 perf_counters
make build-la LA_USER_FEATURES= LA_KERNEL_FEATURES=heap_magazine

# 相邻 baseline；当前默认即关闭 magazine
make build-la LA_USER_FEATURES= LA_KERNEL_FEATURES=
```

带 `heap_magazine perf_counters` 时可向 `/proc/respos_perf` 写入 `drain_heap_magazines`。它按固定
`magazine -> buddy` 锁序归还所有 hart 的 cached block，并把块数累加到 `reclaim_blocks`；命令返回后
当前 syscall 的析构可能再次缓存少量对象，所以验收看 `reclaim_blocks > 0`、cache 明显下降、后续跨核
probe 正常和最终 coalesce 单测，不要求 proc read 后 cached bytes 永远为零。magazine 模式的
`heap_peak_exact=0`：`heap_peak_bytes` 只能作非精确诊断；cached peak upper bound 是各 hart 峰值之和。
正式 A/B 一律关闭 `perf_counters`，并同时记录 marker、Cargo 自报阶段时间、宿主优先级与日志 hash。
完整对照还必须固定 guest memory、hart 数、根盘/辅助盘 hash 和 `-snapshot`；8 GiB 结果不能直接与
12 GiB 历史轮计算加速比。记录宿主 `MemAvailable`、`pswpin/pswpout`、major fault 和 QEMU 核秒；空串口、
启动包装器未 exec 到 QEMU 或持续宿主换页的样本作废。至少比较 Cargo release、axbuild、自报产物字节和
完整成功 marker，不能只看早期 dev 时间或 `Compiling` 行数。当前 8 GiB/12 hart 的严格完整参考为：
关闭 magazine `1281.89s`，旧三原子 A1 `1335.45s`，记账并入本地锁后的 A1 `1318.37s`；后两者均未过
`>=5%` 收益门槛。若继续 A3，在 owner-hart drain/OOM 协议闭合前不得移除最后的 magazine mutex。

修改 AF_UNIX wait/wakeup 后，先运行 `unix_socket_block_probe`，并在 perf kernel 下确认
`unix_yields=0 scheduler_yields=0` 且存在 `blocking_switches`；再跑真实 Cargo timeout 窗口确认 IPC 活性。
专项至少传输超过 `UNIX_SOCKET_BUFFER_LIMIT` 的数据，才能覆盖满写 backpressure，不能只测一条短消息。
容量/墙钟 A/B 期间若宿主启动其他重负载应用，应把该轮明确标记为污染并重跑，不使用其 tg-xtask 或
ext4 ticks 作容量选择。

历史进程/退出/pipe/timer/futex 详细串口输出由独立的 `debug_traces` feature 控制。它会显著扰动
QEMU wall time，不得用于正式成绩或性能计数对照：

```bash
# 只打开详细串口 trace
make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=debug_traces

# 必要时同时打开聚合计数和详细 trace；仅用于定位正确性/活性问题
make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES='perf_counters debug_traces'

# 只记录最终无法处理的 RV64 用户页故障；用于 rustc SIGSEGV 定位
make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=fault_trace

# 正式计时必须保持所有诊断 feature 都关闭
make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=
```

`fault_trace` 只在 `MemorySet::handle_page_fault()` 返回错误、即将向用户发送 SIGSEGV/SIGBUS 时打印
hart/tid/tgid、cause、sepc、stval、sp、ra 和 errno；它不会打印成功的 demand/COW fault。它适合
正确性复现，不是性能内核，结束诊断后仍须恢复无 feature kernel。

Phase 3 写回错误游标可用同一个 `debug_traces` 构建做确定性一次性故障验证。必须使用 `-snapshot`
客体；控制命令只影响下一批 PageCache lower write，触发后自动解除：

```text
/> fs_writeback_probe fault
FS_WRITEBACK_FAULT_PASS
```

probe 通过 `/proc/respos_perf` 写入 `fail_writeback`，覆盖旧 observer 重试、独立 open 和 dup 共享 cursor。
release 内核不接受该命令；正常语义门禁在无 feature 客体运行 `fs_writeback_probe normal`。

Phase 3 完整语义门禁使用无 feature release 客体：

```text
/> fs_writeback_probe phase3
FS_PHASE3_PROBE_PASS
```

该模式覆盖 close 后 dirty-owner 强生命周期、`sync_file_range`、`syncfs`、全局 `sync`、
`MS_SYNC|MS_INVALIDATE`、132 个短文件触发的受控批量写回和 tmpfs 脏文件 unmount。带
`perf_counters debug_traces` 运行后再读 `/proc/respos_perf`，结束态必须为
`page_cache_dirty_pages=0`、`dirty_owners=0`，且 pages/LRU/registry 不随累计文件数增长。
跨启动持久化不能使用 `-snapshot`；应从只读 raw 镜像创建临时 qcow2 overlay，第一轮运行
`fs_writeback_probe persist-prepare` 并正常退出，第二轮挂同一 overlay 运行
`fs_writeback_probe persist-verify`。不得直接修改仓库中的基准镜像。

### BuildStorm CPU/jobs 缩放矩阵

正式 judge 只接受最终平台规定的 RV64 8 核、LA64 12 核配置；下列矩阵只用于选择优化方向，日志必须
明确标为 diagnostic，不得混入成绩。每个数据点从同一只读 snapshot/冷启动开始，固定内核、镜像、
QEMU、宿主负载和磁盘后端，记录 `CARGO_BUILD_JOBS`、kernel features 与 host/guest 两套时间。

先固定官方 vCPU 数，只改变 Cargo 并发：

```sh
# guest /bin/sh 中；1、2、4、8 分别独立冷启动运行
export CARGO_BUILD_JOBS=1
/glibc/buildstorm_testcode.sh
```

再让 vCPU 与 jobs 同时变化：RV64 `1/1、2/2、4/4、8/8`，LA64 使用
`1/1、3/3、6/6、12/12`。LA 每个数据点都要检查启动日志的 online mask；正式命令之外的
`-smp` 只用于计算：

```text
speedup(n) = timed_seconds(1) / timed_seconds(n)
efficiency(n) = speedup(n) / n
```

完整构建太慢时，先用同一镜像中的单个历史 rustc 命令或 2 路不同输出文件回放筛选；短回放只能证明
局部趋势，最终选择仍以官方 timed command 为准。不要并发执行会写同一输出文件的命令。历史 Codex
会话曾把 QEMU 置于 `nice=16/SCHED_IDLE`，使六条带完整 release link 的命令并发 30 分钟仍无法作为
短测收口；重建 devcontainer 或显式修正启动调度后，必须按本文件前述方法确认实际 `NI/CLS`，不能仅
凭配置推断。即使调度优先级正常，固定 4/8 路完整 rustc 仍不作为每次修改后的强制门禁；退出回收优先
使用下述专用 probe。

进程退出后的物理页回收使用嵌入式 `frame_reclaim_probe` 做分钟级门禁。RV64 1 GiB/8 核、`-snapshot`
进入 `user_shell` 后，先通过 `/bin/sh` 读取 `/proc/respos_health`，退出 shell，再连续运行探针并重新
读取 `free_kb`。每轮触碰 64 MiB 并并发退出 7 个 worker；修复版不应出现约 64 MiB/轮的线性下降：

```text
/> /bin/sh
cat /proc/respos_health
exit
/> frame_reclaim_probe
/> frame_reclaim_probe
/> /bin/sh
cat /proc/respos_health
```

该 probe 针对 exit/reap 生命周期，不替代共享 MM/TLB 的 `smp_shared_mm_probe` 或文件一致性的
`buildstorm_file_probe`。

大型只读 `MAP_PRIVATE` 的重复 fault/copy 使用嵌入式 `buildstorm_private_map_probe`。RV64 1 GiB/8 核、
`perf_counters`、`-snapshot` 进入 `user_shell` 后直接运行；探针先准备并写回 64 MiB 文件、验证
`mprotect(PROT_WRITE)` 的 private-copy 语义，再自行清零计数器，让 4 个进程同步逐页读取。为避免
probe 返回后交互 shell 的 stdin 空轮询污染计数，必须把 probe、读取和退出命令一次性预排入串口：

```text
/> buildstorm_private_map_probe
BUILDSTORM_PRIVATE_MAP_PROBE_PASS file_mb=64 workers=4
/> /bin/sh
cat /proc/respos_perf
exit
/> quit
```

重点比较 `private_file_faults`、PageCache hit/miss/eviction、free frames、heap alloc/dealloc ticks、
context switch/IPI 和 ext4 lock 时间。该探针复用已缓存输入，主要测 fault/copy/回收路径，不替代冷缓存
块 I/O 或官方 timed build；每个样本仍须从冷启动 snapshot 开始。若分开发送命令，提示符空窗中的
`Stdin::read()` 会产生大量 `scheduler_yields`，不得归因于 probe 负载。

每个样本至少保存：

- commit、`git diff --stat`、镜像 hash、QEMU 版本/参数、kernel features、jobs；
- guest `BUILDSTORM_BEGIN/COMPILE` 与 `/proc/uptime` timed 秒数；
- host wall time、QEMU `%CPU/RSS`、available memory/swap，长测约每五分钟采样；
- `/proc/respos_perf` 的 running/idle ticks、context switch、IPI、local/remote fence、page fault、
  PageCache、block request、ext4 lock 和 heap 数据；
- 成功 marker、产物大小、首次 kernel/rustc 错误，而不是只记录 Cargo 最后一行。

### BuildStorm 统一耗时评价时间轴

只看宿主 QEMU `%CPU` 或 guest 累计 counter 都无法定位低利用率。长窗口和完整 final 启动 QEMU、
核验 `CLS=TS/NI=-10` 后，用独立 shell 启动无侵入采样器：

```bash
scripts/monitor_qemu_timeline.sh QEMU_PID /tmp/respos-la-final.log \
  /tmp/respos-la-timeline 1
```

输出包括：

- `host-samples.csv`：每秒 QEMU 总 `%CPU`、RSS、线程数、状态与最近 CPU；
- `host-threads.csv`：每个 QEMU 线程的 `%CPU`、状态与最近 CPU，用来区分 vCPU、主循环和 I/O；
- `host-system.csv`：宿主可用内存、空闲 swap、swap-in/out、major fault 与 load average；
- `serial-events.csv`：为 group、BuildStorm begin/compile、core/app compile 和 Cargo finish marker 记录
  采样时的宿主 monotonic 秒；
- `metadata.txt`：命令、宿主核数、采样间隔和起止 UTC。

运行结束后生成宿主摘要：

```bash
scripts/summarize_qemu_timeline.sh /tmp/respos-la-timeline \
  | tee /tmp/respos-la-timeline/summary.txt
```

摘要给出测量窗口、QEMU 核秒、平均/峰值 CPU、低于 400%/800% 的时间占比、峰值 RSS、宿主最低可用
内存与 swap 活动，以及按线程名聚合的核秒与峰值。原始 CSV 仍是证据；摘要不能替代阶段 marker 和
guest perf 快照。

CPU 百分比由相邻 `/proc/PID[/task/TID]/stat` 的 `utime+stime` 差值除以宿主 `CLK_TCK` 和实际
单调时间间隔得到；因此 `300%` 表示该区间约消耗 3 个宿主核，首个无前序样本记为 0。串口 marker
只在采样周期内对齐，1 秒间隔下误差不超过约一个采样周期。

带 `perf_counters` 的 guest 在每个 hart 的 timer tick 无锁采样全局 running hart 数与 ready queue
深度，并在 `/proc/respos_perf` 输出 `running_harts_{0,1,2_3,4_7,8_plus}` 和
`scheduler_ready_{0,1,2_3,4_7,8_plus}`。这些桶是 tick 样本数而不是墙钟秒数；各 hart tick 频率一致
时桶占比可近似时间占比。更新 ready 状态发生在已经持有 scheduler 锁的队列变更路径，timer 侧只做
原子 load/add，不额外获取 runqueue 锁。诊断结束必须同时保存 `concurrency_samples`，避免比较不同
采样总量的绝对桶值。

LA TLB 诊断同时读取 `local_sfence_ticks/max_ticks` 与
`tlb_flush_calls/fresh_map_flushes/cow_flushes/retired_batches/retired_frames`。ticks 只覆盖本地 fence 与
INVTLB 指令，不包含随后因 ASID-wide 驱逐产生的 refill 成本；fresh-map 是“页错误确认 PTE 无效并成功
map 后执行的 flush”次数，不代表可以直接删除失效。LA refill 会填入 invalid TLB pair，故删除前必须
另有可靠的定点 invalidation 或改变 refill 协议；当前 op=5 已被完整 final 否决，不能凭该比例重启。

评价按四层同时报告：

1. **正确性与进度**：group marker、退出码、`ok=true`、产物大小和相同编译 crate；
2. **墙钟阶段**：前置、timed 准备、core/std、依赖 crate、应用与链接/转换；
3. **宿主供给**：QEMU 总 CPU、各线程 CPU、RSS、宿主可用内存/swap 和调度类；
4. **guest 原因**：running/idle 核时、runnable 分布、blocking switch、锁 wait/hold、fault/clear、
   PageCache/block I/O、TLB shootdown completion。

低利用率的判读必须联合四层：若只有约 300% 且 guest idle 高、ready 低，是 workload 串行或等待；
ready 持续高但 idle 也高才指向 scheduler/wakeup；ready 低而 ext4/heap wait 增长指向锁阻塞；guest
running 高但宿主 `%CPU` 低则优先检查宿主调度、线程阻塞或采样口径。后续 guest runnable 指标必须
使用无锁原子状态采样，禁止在 timer interrupt 中为诊断强取 scheduler lock。

子系统 `*_ticks` 是跨 hart 累计核时，且调用链可能嵌套（例如 inode read 包含 PageCache miss，后者又
包含 block read；ext4 hold 又可能覆盖 lower call），所以禁止把各项直接相加当作墙钟分解。评价时应
分别使用：`wait_ticks/acquisitions` 判断排队，`hold_ticks/acquisitions` 判断临界区，
`operation_ticks/calls` 判断路径单次成本，bytes/requests/faults 判断工作量放大；再与同阶段
`task_running_ticks`、host core-seconds 做 A/B。未被这些互不排他的指标可靠解释的部分标为
`unattributed gap`，继续通过阶段缩小或新增专用计数器验证。

判读顺序：

1. runnable 充足而 hart idle，或 wake-to-run 延迟高：先查 scheduler/IPI/runqueue；
2. QEMU CPU 低且 ext4 lock 时间高：先查 FS 锁域和锁内 I/O；
3. private-file faults、RSS 和 eviction 随 jobs 急升：先查 private mmap/PageCache；
4. remote RFENCE 随共享线程增加：再查 active-mask/ASID/shootdown；
5. guest 计数相近但只在 host swap 压力下骤慢/失败：先稳定宿主环境，不把它误判成内核回归。

一次只实现一个由上述数据支持的主要改动，然后重跑对应短门禁、缩放矩阵的数据点和无 feature 正式
配置。完整三层级路线、进入/退出门槛和候选优化见
[buildstorm-smp-plan.md](./buildstorm-smp-plan.md#buildstorm-三层级优化总路线2026-08-09)。

2026-08-08 比赛官方群公告的新决赛参数为：RV64 `-m 16G -smp 8`，LA64
`-m 36G -smp 12`，整轮超时 6250 秒。评测宿主是 128 GiB、40 线程的 VMware Guest。2026-08-09
核对官方 `testsuits-for-oskernel` `final-2026` 分支提交 `3c80dc1` / PR #60 后，正式 timed build 的
Linux baseline 为 RV64 1616.09 秒、LA64 1985.21 秒；此前 4655.23 / 6223.0 秒包含或对应了不同
计时范围，已被上游明确替换。资源参数来自群公告，仓库 README 仍写 RV64 8 GiB，两者冲突处必须以
最终平台启动命令为准；本地取得更新后的镜像后仍须核对镜像 hash、脚本和实际启动命令。

BuildStorm 使用 release kernel、无 `eval` user feature。诊断必须带 `-snapshot`。下列 16 GiB 命令是
正式目标配置；2026-08-11 已在 QEMU 10.0.2/OpenSBI 1.5.1 验证 FDT
`0x47fe00000` 可达、完整内存识别与 BuildStorm 最终 marker：

```bash
make build-rv RV_USER_FEATURES=
timeout 6250s nice -n -10 qemu-system-riscv64 \
  -machine virt -kernel kernel-rv -m 16G -nographic -smp 8 \
  -bios default \
  -drive file=img/sdcard-rv-pub.img,if=none,format=raw,id=x0 \
  -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
  -no-reboot -snapshot
```

LA64 最终资源参数固定为 `-m 36G -smp 12`；具体 QEMU machine、BIOS 与磁盘参数必须以更新后的
官方启动脚本为准，不能从 RV64 命令类推。

进入 guest shell 后执行 `/glibc/buildstorm_testcode.sh`。验收依次检查
`BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok`、最终
`BUILDSTORM_COMPILE mode=multi ok=true ... cores=8 bytes>=500000`；前一标记通过不能替代后一标记。
新镜像将补回预编译 `tg-xtask`；官方计分只覆盖测试用例自身的编译时间，不包含前置依赖构建，也不
包含编译完成后的运行验证。旧镜像仍应保留 minibuild/tg-xtask 自举作为兼容性诊断，但其耗时不能
与新基线成绩直接比较。
脚本的 minibuild 会重定向 cargo 输出，若长时间静默，先单独在 `/bin/sh` 中设置脚本同款
PATH/HOME/RUSTUP/CARGO 环境并运行 `cargo new`、`cargo build -vv`，再决定是否加入临时 kernel trace。
