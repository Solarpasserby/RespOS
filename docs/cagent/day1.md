# CAgent 题目一：单日三人异步协作计划

## 目标与边界

### 当日唯一目标

在 `SMP=1` 下，让决赛题一当前公开版本的 10 个 CAgent 测试稳定通过，并完成 RV64
回归；如果时间允许，再补做 LA64 验证。

官方参考：

- [决赛说明](https://github.com/oscomp/testsuits-for-oskernel/blob/final-2026/README.md)
- [CAgent 测试脚本](https://github.com/oscomp/testsuits-for-oskernel/blob/final-2026/scripts/cagent_testcode.sh)
- [CAgent judge](https://github.com/oscomp/testsuits-for-oskernel/blob/final-2026/judge/judge_cagent-glibc.py)

### 队友获取测例源码

本仓库中的 `testsuit/cagent-test` 只是本地参考副本，并且被 `.gitignore` 忽略；队友同步
RespOS 后不一定能看到它。官方源码在
[`oscomp/testsuits-for-oskernel` 的 `final-2026` 分支](https://github.com/oscomp/testsuits-for-oskernel/tree/final-2026)：

```sh
git clone --branch final-2026 --depth 1 \
  https://github.com/oscomp/testsuits-for-oskernel.git /tmp/testsuits-for-oskernel

cd /tmp/testsuits-for-oskernel
ls cagent-test/
```

本轮定位主要阅读以下文件：

- `cagent-test/simple_llm_server.c`：固定 agent 命令的生成逻辑；
- `cagent-test/agent_lite.c`：`popen`、输出检查和 agent 执行逻辑；
- `scripts/cagent_testcode.sh`：官方 10 项测试的启动与并发方式；
- `judge/judge_cagent-glibc.py`：测试结果和分数解析。

若需要让现有命令直接使用本地路径，可以把上游 `cagent-test/` 复制到 RespOS 的
`testsuit/cagent-test/`；该目录仍只作为未提交的参考源码，不要提交编译产物、镜像或临时日志。

### 当天不做

- 不实现 SMP；
- 不启用 `-smp 8`；
- 不做 BuildStorm；
- 不扩展随机 agent 命令；
- 不为单个测试路径写硬编码成功分支；
- 不修改官方测试脚本和评分器。

### 当前基线

动态链接问题已经通过恢复干净 pub 镜像解决。当前 RV64 单核结果：

通过：`factorial`、`date`、`cpu`、`fs-usage`、`fs-search`。

失败：`network`、`kernel`、`fs-create`、`fs-readwrite`、`fs-directory`。

固定命令来自：

- `testsuit/cagent-test/simple_llm_server.c`
- `testsuit/cagent-test/agent_lite.c`

注意：`testsuit/` 当前被 `.gitignore` 忽略；缺少该目录不表示缺少测例，直接从上游
`final-2026` 分支获取即可。它只作为本地参考源码，不应把编译产物或镜像加入提交。

## 协作规则

三名开发者各自负责一个模块，使用独立分支或独立提交。集成人每天只做一次集中审查和合并。

每个提交必须包含：

1. 失败现象和最小复现命令；
2. 涉及的内核路径和 Linux ABI 语义；
3. 修改后的单项测试结果；
4. 是否验证了已有通过项；
5. `git diff --check` 结果。

不要在共享镜像上长时间反复测试。每轮正式回归前，从 `img/sdcard-rv-pub.img.gz` 恢复干净
`img/sdcard-rv-pub.img`，因为 QEMU 运行会修改 ext4 journal 和测试文件。

## 三人分工

### 开发者 A：调试链路、进程并发、kernel 测试

#### 责任范围

- CAgent 单项 debug runner；
- 保存 agent 原始输出、实际命令、stdout、stderr、exit code；
- `popen`/shell 执行链路；
- `execve`、fork/clone、wait/wait4、timeout、signal；
- `kernel` 测试；
- 完整 CAgent 并发回归。

#### 固定命令

```sh
uname -r
```

#### 当日交付

- 一个不删除 `/tmp/cagent_*` 结果的 debug runner，或者等价的外部日志保存方案；
- `kernel` reject 的根因和最小修复；
- 完整测试连续运行 3 次不出现随机失败；
- 不破坏已有 5 项通过测试。

#### 提前完成后的备用任务

- 检查 `date`、`nproc`、`df/awk` 的 shell 管道返回值；
- 检查子进程回收和 `pclose` 状态；
- 为 B/C 提供每条 agent 命令的原始输出。

### 开发者 B：文件系统三项

#### 责任范围

- `fs-create`；
- `fs-readwrite`；
- `fs-directory`。

#### 固定命令

```sh
printf 'Hello OS\n' > test_file.txt

printf '1\n2\n3\n4\n5\n' > test_input.txt && \\
  awk '{sum += $1} END {print sum}' test_input.txt

mkdir -p test_dir && \\
  touch test_dir/file1 test_dir/file2 test_dir/file3 && \\
  ls test_dir | wc -l
```

#### 优先检查

- `open/openat`、`O_CREAT/O_TRUNC`；
- `read/write/close` 和文件 offset；
- `mkdir/mkdirat`；
- `getdents64`；
- 相对路径和当前工作目录；
- `unlink/rmdir` 和测试清理；
- shell 重定向和管道中的退出状态。

#### 当日交付

- 三个 FS 测试全部通过；
- 至少一个最小 FS 回归命令或 probe；
- 确认没有使用 `/glibc` 或测试文件名特判绕过 ABI。

#### 提前完成后的备用任务

- 检查空目录、多文件目录和重复 `mkdir -p`；
- 运行已有 FS/LTP 相关回归；
- 验证 FS 修复不影响 `fs-usage` 和 `fs-search`。

### 开发者 C：网络、PATH、procfs

#### 责任范围

- `network`；
- `ss` 命令发现和 PATH；
- `/proc/net/tcp`；
- loopback TCP；
- 必要时的最小 netlink ABI。

#### 固定命令

```sh
ss -tan | grep ESTAB | wc -l
```

#### 第一优先级：先区分两类问题

1. `ss` 是否能通过 PATH 找到。当前镜像中已知文件是 `/glibc/ss`，但命令没有使用绝对路径；
2. 找到 `ss` 后，是否因为 `AF_NETLINK` 或 `NETLINK_SOCK_DIAG` 未实现而失败。

#### 当日交付

- 明确记录 PATH、`ss` 执行、procfs 和 socket 的实际失败点；
- 如果只是 PATH，修复通用 guest 环境，不改测试命令；
- 如果是内核能力，补真实 Linux ABI，不返回固定伪造数字；
- `network` 测试通过，并验证 server/agent 的 loopback TCP 不回归。

#### 提前完成后的备用任务

- `ss -tln | grep LISTEN | wc -l`；
- `/proc/net/udp`；
- 多进程 connect/close 后的连接状态清理；
- RV64/LA64 网络行为对比。

## 单日时间表

### 统一开始阶段：第 0～1 小时

所有人先确认：

```sh
make run-rv-pub
```

必须看到：

```text
SMP=1
FEATURES=
Rust user shell
```

然后从干净镜像运行一次完整 CAgent，保存 baseline，不立即修改内核。

### 定位阶段：第 1～3 小时

- A：完成 debug runner 和 `kernel` 单项复现；
- B：分别复现三个 FS 固定命令；
- C：分别复现 PATH、`ss`、`/proc/net/tcp` 和 loopback。

第 3 小时结束时，每个人必须提交一份短报告：

```text
失败命令：
实际输出：
返回值/errno：
内核路径：
拟修复方案：
```

### 修复阶段：第 3～7 小时

- B 优先完成 FS 三项；
- C 完成 network 根因和最小修复；
- A 完成 debug 链路、kernel 和公共进程问题。

每个修复独立提交，不把多个模块重构合并成一个提交。

### 第一次集中审查：第 7 小时

集成人检查：

- 是否有最小复现；
- 是否符合 Linux ABI；
- 是否存在测试特判；
- 是否影响 mmap、LTP、已有 FS/网络功能；
- 是否能在干净镜像上复现结果。

只合并证据完整的提交。

### 集成阶段：第 7～10 小时

合并后依次执行：

```sh
make run-rv-pub
make run-la-pub
```

优先完成 RV64 全部 10 项，再补 LA64。若 LA64 来不及，不要牺牲 RV64 的回归质量。

### 收尾阶段：第 10～12 小时

最低验收标准：

- RV64 10/10 pass；
- 至少连续运行 3 次无随机失败；
- LA64 已完成启动和尽可能多的单项验证；
- `git diff --check` 通过；
- 没有将测试镜像、测试二进制或临时日志加入 Git；
- 未启用 SMP。

## 集成审查格式

每位开发者在当天结束前提交以下格式：

```text
负责人：
提交：
修改模块：
最小复现：
修复说明：
RV64 结果：
LA64 结果：
已有回归：
未解决问题：
```

当天目标是固定命令全部通过。随机 agent、更多 shell 命令、SMP 和 BuildStorm 放到下一阶段。
