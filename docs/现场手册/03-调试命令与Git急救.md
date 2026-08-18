# 调试命令与 Git 急救

## 版本和差异

```bash
git status --short
git rev-parse --short HEAD
git log -10 --oneline --decorate
git diff --stat
git diff --check
```

看到不认识的未提交修改时先停下确认归属，不要用 reset/checkout 覆盖队友工作。

## 构建与提测门禁

```bash
make help
make build-qemu-rv64
make build-qemu-loongarch64
make build-jh7110
make build-ls2k1000

make check-submit
make preflight
```

`make all` 是线上固定入口，只构建两个 QEMU release 内核和两份自动识别辅助盘。`make preflight` 会
实际重新构建。`make verify-clean-tree` 和 `make package-submit` 要求当前修改已经提交且工作区干净，
不要在仍需保留的现场修改上强行执行。

## QEMU 运行

```bash
make run-rv-pre
make run-la-pre
make run-rv-final
make run-la-final
make run-rv-diagnostic
make run-la-diagnostic
```

初赛默认日志是 `rv-output.txt`、`la-output.txt`，决赛默认日志是 `rv-final-output.txt`、
`la-final-output.txt`。本地入口使用快照；若自己拼 QEMU 命令，显式检查是否会写原始镜像。

## 日志先看什么

```bash
rg -n 'SUMMARY:|TBROK|Segmentation fault|SIGBUS|Assert Fatal|end: fail|exited with code|panic' \
  rv-output.txt la-output.txt rv-final-output.txt la-final-output.txt
tail -n 120 rv-output.txt
tail -n 120 la-output.txt
```

先定位第一个框架错误或内核异常，再看后续失败。保留未经裁剪的原始日志，并把筛选结果另存为摘要。

LTP 报告：

```bash
python3 judge/ltp_report.py rv-output.txt la-output.txt --format markdown
python3 judge/ltp_compare.py \
  --baseline judge/baseline/ltp-linux-baseline.csv \
  --respos /tmp/respos-ltp-report.csv \
  --case-list user/oscomp_ltp_list.txt
```

生成 `/tmp/respos-ltp-report.csv` 的完整步骤见
[`docs/codex/workflows.md`](../codex/workflows.md#解析与-baseline-对比)。

## GDB

一个终端启动：

```bash
make -C os gdbserver ARCH=riscv64 MODE=debug
# 或
make -C os gdbserver ARCH=loongarch64 MODE=debug
```

另一个终端连接：

```bash
make -C os gdbclient ARCH=riscv64 MODE=debug
# 或
make -C os gdbclient ARCH=loongarch64 MODE=debug
```

适合早期启动、trap、页表和 panic。卡死时先中断并查看所有 hart 的 PC/栈；不要只盯当前 hart。

## 只读检查镜像

```bash
debugfs -R 'cat /musl/ltp_testcode.sh' img/sdcard-rv.img
debugfs -R 'ls -l /musl/ltp/testcases/bin' img/sdcard-rv.img
```

优先用 `debugfs -R` 读取。需要修改镜像做实验时先复制到 `/tmp`，记录源文件 SHA-256，不在官方原件上
直接写入。

## 聚焦源码

```bash
rg -n 'SYSCALL_[A-Z0-9_]+' os/src/syscall/mod.rs
rg -n 'handle_page_fault|wakeup_task|fetch_task|copy_(to|from)_user' os/src
rg -n 'TODO|FIXME|panic!|unwrap\(' os/src
```

修改前用 `git log -S'<symbol>' --all --oneline` 和 `git blame <file>` 找历史原因；它们是线索，不替代
当前代码和复现结果。

## Git 急救

### 保存当前现场

```bash
git status --short
git diff > /tmp/respos-working.patch
git diff --cached > /tmp/respos-index.patch
git switch -c rescue/现场-日期
```

分支名中的“现场-日期”改成有意义且不冲突的名字。未跟踪文件不会进入 `git diff`，用
`git status --short` 单独清点；包含镜像、密钥或大日志时不要随意 `git add -A`。

### 找回提交或比较版本

```bash
git reflog --date=iso
git show --stat <commit>
git diff <good-commit>..<bad-commit> -- os/src
git log --oneline <good-commit>..<bad-commit>
```

### 撤销一个已提交改动

共享分支优先新增反向提交：

```bash
git switch -c rescue/before-revert
git revert <commit>
```

### 移植单个修复

```bash
git switch -c rescue/before-cherry-pick
git cherry-pick <commit>
```

冲突时先看 `git status`，逐个解决并测试；不确定时可执行 `git cherry-pick --abort` 回到开始前。

不要在未确认目标和备份前使用 `git reset --hard`、`git clean -fdx`、强推或删除分支。这些操作可能
永久丢失未提交源码、忽略的镜像和队友工作。
