# RespOS 当前状态

本文件是快速变化的状态页。更新测试结论时必须同时更新日期、提交和命令。

## 2026-08-03 CAgent 开发者 A：并发 TCP 与 kernel 链路

- 状态：RV64 A 责任项已验证；完整 CAgent 仍被两个 FS 项阻断；LA64 待继续定位
- 适用范围：基线提交 `347414d` 加当前未提交补丁，RV64/LA64 pub 镜像，`SMP=1`、256 MiB
- 最后验证：2026-08-03
- 证据：`scripts/cagent_debug.sh`、`os/src/net/listen.rs`、`os/src/net/mod.rs`、
  `/tmp/cagent-a-rv-run{1,2,3}.log`、`/tmp/cagent-a-la-run1.log`
- 内容：debug runner 分别保留固定命令、命令 stdout/stderr/exit、agent 原始日志、agent exit、
  validation exit 和 server 日志。`kernel` 单项的固定命令为 `uname -r`，实测输出
  `6.10.0-dev`，stderr 为空，命令/agent/validation 退出码均为 0；因此此前的 `kernel reject`
  不是 `sys_uname` 缺陷。
- 根因：旧 listen table 只有一个 smoltcp listener。一次 interface poll 同时处理多个 SYN 时，
  第一个握手会占用该 listener，其余连接在 userspace `accept` 补 listener 前收到 reset，表现为
  agent 退出 255 和 `Connection failed to 127.0.0.1:8080`。当前补丁把 syscall 的 backlog 传入
  TCP 层，按受限 backlog 预建 listener 池，并在网络 poll/accept 后把已连接 handle 转入 accept
  queue、补充 listener。
- RV64 验证：`make build-rv` 通过；每轮从 `img/sdcard-rv-pub.img.gz` 恢复镜像，再以
  `make run-rv-pub LA_PUB_FS_IMG=img/sdcard-la.img RV_OUTPUT=/tmp/cagent-a-rv-runN.log`
  启动并执行 `/glibc/cagent_testcode.sh`（当时 LA pub 尚在下载，override 只满足入口的双镜像
  可读检查，RV QEMU 未使用该 LA 镜像）。连续三轮结果完全一致：8/10 pass；
  `factorial`、`date`、`network`、`cpu`、`kernel`、`fs-readwrite`、`fs-search`、`fs-usage` 通过，
  `fs-create`、`fs-directory` reject。修复后的全量 debug 日志不再出现 `Connection failed`，
  三轮官方结果中 A 的 kernel/并发项和此前五个通过项稳定。
- 剩余阻断：`fs-create` 的验证读取 `test_file.txt` 报 `Invalid argument`；`fs-directory` 的
  `touch test_dir/file{1,2,3}` 报 `Invalid argument`，属于文件系统路径。LA64 release 构建和
  `make run-la-pub` 启动到 `Rust user shell` 均通过，`./busybox uname -r` 输出 `6.10.0-dev`；
  但官方 CAgent 脚本在输出测试组记录前退出 139，尚未证明 LA64 完整 CAgent 可运行。

## 2026-08-02 决赛题一 CAgent 初步基线

- 状态：题目结构已确认；单核 RV64 官方脚本已完整执行
- 适用范围：RV64/LA64 pub 镜像，glibc CAgent
- 最后验证：2026-08-02
- 证据：`img/sdcard-rv-pub.img`、`Makefile`、上游 `final-2026` 的
  `scripts/cagent_testcode.sh` 与 `judge/judge_cagent-glibc.py`；RV64 `/tmp/respos-rv-pub-output.txt`
- 内容：题目一不是内核内置 `testrunner`，而是镜像 `/glibc/cagent_testcode.sh`。脚本启动
  `/glibc/simple_llm_server`，并行启动 10 个 `agent_lite` 任务，分别覆盖 factorial、date、
  network、cpu、kernel、文件创建/读写/目录、文件搜索和磁盘使用；每项输出
  `testcase cagent <name> pass|reject <duration>`，外部 judge 只解析这些记录。
- 单核实测：第一次直接执行脚本时出现 glibc loader 错误，随后确认原因是此前带 `eval` 的旧
  `testrunner` 修改了镜像中的 `/usr/lib/ld-linux-riscv64-lp64d.so.1` 链接。用保留的
  `sdcard-rv-pub.img.gz` 恢复后，`make run-rv-pub` 成功执行 `/glibc/cagent_testcode.sh`，
  动态链接链路正常；当前 RV64 单核基线为 5 pass、5 reject：通过 factorial、date、cpu、
  fs-usage、fs-search；reject network、kernel、fs-create、fs-readwrite、fs-directory。
- 当前 reject 的初步证据：`/proc/net/tcp` 返回 `ENOENT`，`/glibc/ss -tn` 报
  `Address family not supported by protocol`；其余文件/内核项还需保存 agent 输出后逐项复现。
- 决策：题一先保持 `SMP=1`。脚本虽然并行启动 10 个测试，但单核已经能覆盖进程创建、等待、
  信号/超时、socket、文件和 glibc 动态加载等并发交互；上游规则明确要求 `-smp 8 -m 8G` 的是
  题二 BuildStorm。题一功能闭环后再做 `SMP=2/8` 烟测。

### CAgent 源码带来的精确命令映射

- 状态：已确认
- 适用范围：`testsuit/cagent-test`、当前 CAgent reject 定位
- 最后验证：2026-08-02
- 证据：`testsuit/cagent-test/simple_llm_server.c`、`testsuit/cagent-test/agent_lite.c`
- 内容：`simple_llm_server` 对文件任务直接生成固定命令：
  `printf 'Hello OS\n' > test_file.txt`；写读任务使用 `printf ... > test_input.txt && awk ...`；
  目录任务使用 `mkdir -p test_dir && touch ... && ls test_dir | wc -l`；搜索任务使用
  `find . -name '*.sh' | wc -l`。网络任务固定为 `ss -tan | grep ESTAB | wc -l`，CPU 为 `nproc`，
  磁盘为 `df -h / | awk ...`，内核版本为 `uname -r`，日期为 `date -d ...`。
- `agent_lite` 的 `tool_bash` 通过 `popen(command, "r")` 交给 shell 执行，并检查 `pclose`
  状态；因此后续应分别检查 shell、PATH、命令可执行文件、管道返回值和底层 syscall，不能只按
  测例名称猜测单个内核接口。
- 当前线索：镜像中的 `ss` 位于 `/glibc/ss`，而测试命令使用未限定路径的 `ss`；需要在官方
  CAgent 环境变量下确认这是 PATH 问题还是 `ss` 所需 netlink/procfs 能力缺失。
- 维护注意：当前仓库 `.gitignore` 的 `testsuit/` 规则会忽略这批源码；它目前是本地参考资料，
  队友缺少该目录时应从上游 [`testsuits-for-oskernel`](https://github.com/oscomp/testsuits-for-oskernel/tree/final-2026)
  的 `final-2026` 分支获取，重点查看 `cagent-test/`、`scripts/cagent_testcode.sh` 和
  `judge/judge_cagent-glibc.py`。如需随项目提交，必须单独调整忽略规则并审查第三方许可证与文件范围。
- 当前协作计划：见 [`docs/cagent/day1.md`](../cagent/day1.md)。计划按调试/进程、文件系统、
  网络/PATH/procfs 三个模块分工，目标是在 `SMP=1` 下先完成 10 个固定命令；随机 agent、SMP
  和 BuildStorm 暂不纳入本轮范围。
- 文档分层：`docs/cagent/` 保存队友执行用的阶段计划；`docs/codex/` 保存当前状态、架构、
  工作流和陷阱，供 Codex 在同步仓库后快速接手。

## 2026-08-02 pub 镜像交互式启动配置（当前工作树，未提交）

- 状态：已验证（RV64 启动到交互式 shell）
- 适用范围：`sdcard-rv-pub.img`、`sdcard-la-pub.img` 的第一阶段介入
- 最后验证：2026-08-02
- 证据：`Makefile`、`user/src/bin/initproc.rs`、`user/Makefile`、dry-run 输出及 QEMU 日志
- 内容：顶层 Makefile 新增 `make run-rv-pub` 和 `make run-la-pub`。这两个入口加载 pub
  镜像，默认使用 `256M`、`SMP=1`，并将 `RV_USER_FEATURES`/`LA_USER_FEATURES` 置空，
  使 initproc 启动内置 `user_shell` 而不是初赛 `testrunner`。原 `make rv` / `make la`
  仍默认使用初赛镜像、`FEATURES=eval` 和单核配置。
- 直接使用现有 `kernel-rv`、`-smp 8` 启动 pub 镜像已证明 QEMU 能加载并挂载该磁盘，
  但由于该内核仍内嵌初赛 `testrunner`，随后因 pub 镜像没有 `/musl/basic` 等初赛文件而
  报 `ENOENT` 并关机；这不是 pub 镜像挂载失败。
- 实测 `make run-rv-pub` 已完成构建并启动 QEMU，日志显示 `Platform HART Count: 1` 和
  `Rust user shell` 的 `/>` 提示符；本次未运行 `testrunner`。容器内的
  `/opt/riscv64-linux-musl-cross/bin/riscv64-linux-musl-gcc` 确实存在，且本轮直接构建
  lwext4 与完整 RV kernel 均成功，因此当前没有足够证据要求更新 Dockerfile。此前出现的
  `SIGSYS`/`Bad system call` 暂未复现，保留为待定位的环境异常，而不是当前构建阻断。
- 后续影响：8 vCPU/8G 不是当前内核的启动要求，也不是本阶段默认配置；在 SMP 未验证前，
  不应把它加入交互式 pub 入口。

## 基线

### 当前开发提交

- 状态：已确认
- 适用范围：本地 `dev`
- 最后验证：2026-08-01
- 证据：`git log -1`、`git status --short --branch`
- 内容：`dev` 指向 `44430df`（`fix(integration): tighten mmap and unlink lifecycle`），包含
  A/B/C 整合。tracked 工作树在本轮文档工作前无业务源码修改。
- 后续影响：后续结果必须注明是否仍基于该提交；新增 Codex 文档尚未提交。

## 2026-08-02 writable file `MAP_SHARED` 修复（当前工作树，未提交）

### 实现范围

- 状态：已验证（受限 ABI 子集）
- 适用范围：文件 `MAP_SHARED`、`msync`、`munmap`、`mremap`、`mprotect`、进程退出；RV64/LA64
- 最后验证：2026-08-02
- 证据：`os/src/mm/memory_set.rs`、`os/src/syscall/mm.rs`、`os/src/syscall/fs.rs`、
  `os/src/fs/file.rs`、`os/src/task/task.rs`；基线 `94a2598`
- 内容：共享文件映射的 resident frame 在 `MemorySet` 锁内只做快照，文件读写在锁外执行；
  共享文件页在建立 PTE 前锁外预取；`MS_ASYNC` 写入文件页缓存后返回，`MS_SYNC` 额外执行
  `fsync`；munmap、固定映射替换、mremap 覆盖/收缩、mprotect 和进程退出均先处理共享写回。
  当前没有硬件 dirty bit，因此 resident writable shared file page 采用保守写回。
- 后续影响：`MS_INVALIDATE` 仍返回 `EOPNOTSUPP`，因为全局共享文件 frame 缓存尚无安全的
  inode-wide 失效协议；文件截断后的访问也尚未实现 Linux `SIGBUS` 边界。

### 针对性回归

- 状态：已确认
- 适用范围：LTP mmap/munmap 子集、两种 libc、两架构
- 最后验证：2026-08-02
- 证据：`/tmp/respos-rv-ltp-mmap-all.log`、`/tmp/respos-la-ltp-mmap.log`；命令分别为
  `TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=... make rv RV_MODE=debug` 和
  `TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=... make la LA_MODE=debug`
- 内容：RV64 musl/glibc 各 `20 passed, 2 failed`，失败为已有的 `mmap13` SIGBUS 和
  `mmap18` 栈边界语义；其余目标 mmap/munmap 测例通过。LA64 musl/glibc 各 `15 passed,
  0 failed`。RV64/LA64 的 `mmap001`（1000 页映射、触碰、同步、解除映射）均通过，且不再
  在 LTP harness 初始化阶段因 writable shared mmap 返回 `EOPNOTSUPP`。
- 后续影响：这证明阻断已解除，但不代表完整 LTP 或文件截断/SIGBUS 语义全部完成。

## 2026-08-01 双架构完整运行

### release 构建、启动和关机

- 状态：已确认
- 适用范围：RV64/LA64，单核，默认 256 MiB 比赛镜像
- 最后验证：2026-08-01
- 证据：`make rv` → `rv-output.txt`；`make la` → `la-output.txt`
- 内容：两条命令都成功构建 user/kernel、启动 QEMU、运行到 testrunner 结束并主动关机；外层
  make 退出码均为 0。未观察到 kernel panic 或整机死锁。
- 后续影响：这只证明运行流程完成，不代表测试通过；结果必须从日志内的分组和 summary 判定。

### LTP 当前被 writable file `MAP_SHARED` 阻断

- 状态：已确认
- 适用范围：musl/glibc LTP，RV64/LA64
- 最后验证：2026-08-01
- 证据：`os/src/fs/file.rs::FileOp::mmap_allowed`；两份运行日志中的
  `mmap(... PROT_READ | PROT_WRITE, MAP_SHARED, fd=3 ...) failed: EOPNOTSUPP`
- 内容：LTP 框架建立控制页时失败，绝大多数用例在测试主体之前 `TBROK`：

| 架构 | libc | passed | failed | skipped | selected |
| --- | --- | ---: | ---: | ---: | ---: |
| RV64 | musl | 18 | 664 | 17 | 699 |
| RV64 | glibc | 18 | 667 | 14 | 699 |
| LA64 | musl | 18 | 665 | 16 | 699 |
| LA64 | glibc | 18 | 668 | 13 | 699 |

- 后续影响：不能用这轮数字评价几百个 syscall 的真实语义；首要任务是实现安全的 writable
  shared file mapping/writeback 协议，再重跑完整 LTP。

### basic 与 lmbench 的 mmap 回归

- 状态：已确认
- 适用范围：两架构、两种 libc
- 最后验证：2026-08-01
- 证据：`rv-output.txt`、`la-output.txt`
- 内容：basic 的文件 mmap/munmap 失败，munmap 返回 `-EINVAL`；RV mmap 测试出现用户态
  segmentation fault。lmbench musl/glibc 均报告 mmap/msync 不支持。
- 后续影响：这是跨架构策略回归，不是单架构偶发故障。

### 其余工作负载可运行但尚非全绿

- 状态：已确认
- 适用范围：当前完整日志
- 最后验证：2026-08-01
- 证据：`rv-output.txt`、`la-output.txt`
- 内容：BusyBox、libcbench、Lua 和 iozone 均运行到分组结束，日志未显示内核崩溃；但
  libctest 的 static/dynamic wrapper 在两架构均报告退出码 256。网络存在非确定性建连失败：
  RV 的 iperf musl parallel TCP 与 netperf musl UDP_STREAM 各失败一次，LA 的 iperf glibc
  parallel TCP 失败一次。
- 后续影响：libctest 返回值要单独定位；网络项应增加服务端 ready/retry 诊断后再判断内核缺陷。

## A/B/C 整合时的专项验证

### 代表性 runtime/MM/FS probe 曾通过双架构

- 状态：已确认（历史集成证据）
- 适用范围：提交 `44430df` 形成前后的整合验证
- 最后验证：2026-08-01
- 证据：`docs/四天内核重构-ABC-整合审查.md`、Git `2f736d4`/`e0d69fd`/`cba8e24`
- 内容：文档记录双架构 debug/release build、MM split/invariant、task A probe、FS Day2～Day4
  回归通过；futex cmp-requeue 使用专项强制竞态构建验证，之后恢复默认构建。
- 后续影响：这些结果证明合并接口未明显破坏专项语义，但不能替代今天被阻断的完整 LTP。

## 历史信息与待验证项

### README 的“LTP 600 余项”是历史状态

- 状态：已过期
- 适用范围：README 所描述的早期稳定版本
- 最后验证：2026-08-01
- 证据：`README.md`；当前完整 RV/LA 日志与其不一致
- 内容：项目历史上曾记录本地 LTP 可通过 600 余项、评测稳定版本约 2350 分。这些数字不能
  代表当前 `dev`。
- 后续影响：发布说明或汇报引用成绩时必须注明对应 commit、镜像和测试日期。

### 尚未完成的高风险验证

- 状态：待验证
- 适用范围：进入 main 前的回归门禁
- 最后验证：2026-08-01
- 证据：`docs/四天内核重构-ABC-整合审查.md`
- 内容：真实 SMP；MAP_FIXED/mremap 极端 ENOMEM 回滚；truncate 与 resident mapped page；
  rename+多硬链接事务；epoll 跨进程最后关闭；pipe/poll/epoll 与 close/signal/timeout 联合竞争。
- 后续影响：这些问题不能因当前 QEMU 完成整轮运行而视为关闭。

## 工作区注意事项

### 本地存在刻意未提交的资料和回归程序

- 状态：已确认
- 适用范围：当前容器工作区
- 最后验证：2026-08-01
- 证据：`git status --short`
- 内容：两份 `docs/RespOS*.md` 与 `user/src/bin/fs_day{2,3,4}_*.rs` 未跟踪；此前约定不纳入
  提交。本目录新增文件也将保持未提交，直到维护者主动审查。
- 后续影响：`user/Makefile` 会通过 wildcard 构建所有 `user/src/bin/*.rs`，所以未跟踪测试仍会
  影响本地构建产物；复现实验时要记录它们是否存在。
