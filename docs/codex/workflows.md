# RespOS 开发与验证工作流

命令以当前仓库脚本为准。运行测试前先读 [current-status.md](./current-status.md) 和
[pitfalls.md](./pitfalls.md)。

## 环境与镜像

### 准备比赛镜像

- 状态：已确认
- 适用范围：首次运行或镜像恢复
- 最后验证：2026-08-01
- 证据：`scripts/get_img.sh`、顶层 `Makefile`
- 内容：

```bash
bash scripts/get_img.sh
ls -lh img/sdcard-rv.img img/sdcard-la.img
```

脚本优先复用 `img/*.img.xz`，解压时保留压缩包。运行中的内核会修改 ext4 镜像；需要干净
基线时，应从保留的压缩包重新解压，而不是假定上一次运行后的镜像未变化。

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
