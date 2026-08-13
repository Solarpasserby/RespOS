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

当前 no-feature guest 可运行 `task_phase5_probe`。修复前预期打印三个
`TASK_PHASE5_EXPECTED_FAIL` 和 `TASK_PHASE5 CURRENT DIFFERENCES CONFIRMED`，并以非零状态退出；这些
marker 只证明探针捕获了已确认差异，不是通过。完成 task 生命周期修复后，退出门槛是
`TASK_PHASE5 ALL PASS`，并且不得出现 expected-fail marker。探针分别覆盖 leader 原始 `SYS_exit`
后 worker 的 `exit_group`、worker 的原始 `SYS_exit`，以及非 leader `execve` 后
`getpid() == gettid()` 的 identity 接管。

Phase 5 mmap EOF/SIGBUS Linux 对照：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/mmap_phase5_probe_linux.c \
  -o /tmp/mmap_phase5_probe_linux
/tmp/mmap_phase5_probe_linux
```

当前 no-feature guest 可运行 `mmap_phase5_probe`。修复前预期打印 shared/private 的
`MMAP_PHASE5_EXPECTED_FAIL` 和 `MMAP_PHASE5 CURRENT DIFFERENCES CONFIRMED`，并以非零状态退出；它们
覆盖初始 EOF 整页 SIGBUS、truncate 后 resident PTE 失效、未 COW EOF 部分页清零、已 COW private
部分页保留匿名字节，以及 mmap 后动态扩容。完成 MM 修复后必须同时出现 `MMAP_PHASE5 shared PASS`、
`MMAP_PHASE5 private PASS`、`MMAP_PHASE5 private_cow_truncate PASS` 与
`MMAP_PHASE5 ALL PASS`，并复跑 `buildstorm_file_probe` 的 mmap 扩容/写回竞态。

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

Phase 5 AF_UNIX、pipe 与 poll 对照：

```bash
cc -std=c11 -Wall -Wextra -Werror -O2 scripts/socket_phase5_probe_linux.c \
  -o /tmp/socket_phase5_probe_linux
/tmp/socket_phase5_probe_linux
```

RV64 no-feature release、`-m 16G -smp 8 -snapshot` guest 中运行 `socket_phase5_probe`，以
`SOCKET_PHASE5 ALL PASS` 为通过标志。它覆盖 pathname accept/connect、非阻塞 accept、EOF/EPIPE、
AF_UNIX shutdown、阻塞 ppoll 数据唤醒、pipe 的无条件 HUP/ERR、epoll HUP 和 accept EINTR。修改
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

stat 输出需核对 size/inode/mode/uid/gid；最终还需覆盖 symlink size、uid/gid 高位、超过 4 GiB size 和
LTP stat/fstatat。固定窗口比较必须同时报告阶段进度，不能因优化版在同一时间内执行了更多调用而只比较
累计 ticks。

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
