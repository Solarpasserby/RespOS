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

### 准备比赛镜像

- 状态：已确认
- 适用范围：首次运行或镜像恢复
- 最后验证：2026-08-01
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

### 顶层双架构入口

- 状态：已确认
- 适用范围：提交前构建
- 最后验证：2026-08-01
- 证据：`Makefile`
- 内容：

```bash
make all                  # RV/LA release，生成 kernel-rv、kernel-la
make build-rv             # 只构建 RV release
make build-la             # 只构建 LA release
make MODE=debug all       # 双架构 debug
make MODE=release-debug all
make check-submit         # 构建并检查两个 ELF 产物类型
```

顶层构建会复制架构对应的 Cargo config 到 `os/.cargo/config.toml` 和
`user/.cargo/config.toml`，先构建用户程序，再通过 `os/build.rs` 嵌入内核。

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

### 完整双架构运行

- 状态：已确认
- 适用范围：比赛镜像完整回归
- 最后验证：2026-08-01
- 证据：`Makefile`、当前运行日志
- 内容：

```bash
make rv                   # 输出同时写入 rv-output.txt
make la                   # 输出同时写入 la-output.txt
```

常用覆盖参数：

```bash
make rv MEM=128M SMP=1 RV_OUTPUT=/tmp/respos-rv.log
make la MEM=128M SMP=1 LA_OUTPUT=/tmp/respos-la.log
```

- 后续影响：建议顺序运行；端口转发和共享构建配置使并行运行收益有限且更难诊断。

### pub 镜像交互式启动

- 状态：已验证（RV64 启动到交互式 shell）
- 适用范围：决赛 pub 镜像的第一阶段检查
- 最后验证：2026-08-02
- 证据：顶层 `Makefile`、`user/src/bin/initproc.rs`、QEMU 直接启动日志
- 内容：

```bash
make run-rv-pub       # RV pub 镜像，默认 256M、单核、串口 user_shell
make run-la-pub       # LA pub 镜像，默认 256M、单核、串口 user_shell
```

这两个目标通过 virtio block 设备加载 ext4 镜像，不执行宿主机挂载。它们将用户程序的
`eval` feature 清空，因此 `initproc` 启动 `user_shell`，不会自动运行初赛 `testrunner`。
`make rv` 和 `make la` 仍保留 `FEATURES=eval` 及原初赛镜像，不能用来检查 pub 镜像。
8 vCPU/8G 不是当前交互式入口的默认值；待 SMP 和决赛 launcher 明确后再单独增加决赛资源配置。

- 2026-08-02 实测 `make run-rv-pub` 已完成用户程序、lwext4 和内核构建，QEMU 以 1 个
  HART、256M 内存加载 `img/sdcard-rv-pub.img`，并显示 `Rust user shell` 的 `/>` 提示符；
  未进入 `testrunner`。LA 入口已做同样的配置检查，但尚未完成本轮启动验证。

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
