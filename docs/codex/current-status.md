# RespOS 当前状态

本文件是快速变化的状态页。更新测试结论时必须同时更新日期、提交和命令。

## 2026-08-03 提交前交接：题一现状、目标与建议

- 当前基线：`dev` 的已提交基线为 `8169793`；工作树含 ext4 durability、RV64 kernel timer、
  wait/TCP 可中断阻塞以及 CAgent 诊断补丁，尚未提交。`testsuit/cagent-test/` 是本地上游参考，
  保持忽略且未被修改或加入 Git。
- 已验证能力（RV64、SMP=1、256 MiB glibc pub）：单项 CAgent `kernel` pass；2 路
  `factorial,date` pass；4 路 `factorial,date,network,cpu` pass；`fs-create`、
  `fs-readwrite`、`fs-directory` 的固定命令和 agent 链路已通过。TCP listener 并发连接、
  `wait4` 的 SIGALRM/EINTR、server 在 accept/read 阻塞时的 SIGTERM 清理均有专项验证。
  最后提交前单项回归：干净 pub 镜像上 `kernel` pass，4409 ms。
- 当前完整题一结论：最近一次 10 路诊断 runner 正常收敛但只有 `factorial`、`fs-create`、
  `fs-readwrite`、`fs-usage` pass，且都没有时间奖励；这相当于 RV64 的**诊断性估计**
  73.5/200，而不是可申报官方成绩。必须在干净 pub 镜像上直接运行官方
  `/glibc/cagent_testcode.sh` 并把输出交给 `judge/judge_cagent-glibc.py`，才能确认当前分数。
- 已排除的主因：server 的阻塞 `read(2)` 能被 SIGTERM 中断；HTTP 本身仅约 10--245 ms；
  静态 fork/exec/wait 在 4 路下仅约 0.3--0.5 s。不能再把当前超时归结为 TCP 死锁、
  `waitpid` 卡死或单个固定命令的 syscall 语义失败。
- 当前最强性能线索：glibc `popen()` 的 `/bin/sh -c` 动态 dash 路径单路约 0.93 s、4 路约
  2.7--3.3 s；CAgent 的 `popen()` 返回前在 4 路下为 4.6--12.3 s。动态 ELF/loader/mmap
  路径比 HTTP 和静态 exec 更值得优先优化。
- 下一开发目标（按顺序）：
  1. 为 `sys_execve`、`read_dynamic_linker()`、ELF LOAD 段复制、private file-mmap page fault
     加**可关闭的通用计数/耗时探针**，先量化各段，不修改上游测试。
  2. 若证据确认重复文件拷贝/缺页读取主导，设计可回收、权限正确的只读 executable/page cache
     或共享 file frame；私有可写/COW、`mprotect`、`munmap`、`exec` 退出必须保持 Linux ABI，
     不可针对 dash、CAgent 或测试名特判。
  3. 每次性能修改后，除构建/fmt/diff 检查外，回归 writable `MAP_SHARED`、ext4 正常关机
     `e2fsck`、TCP 并发 listener、CAgent 单项/4 路；最后从干净镜像跑官方 10 路和 judge。
- 提交建议：可将“ext4 durability”、“wait/TCP/timer 语义”、“CAgent diagnostics/docs”分成
  独立提交，便于回滚和审查；提交前勿纳入 `img/`、`testsuit/`、`/tmp` 产物或本地学习资料。

## 2026-08-03 CAgent 受控并发矩阵（进行中）

- 状态：1/2/4 并发已通过；10 并发存在普遍的并发超时/启动延迟，正在区分定时器与 fork/exec 排队
- 适用范围：`dev` 提交 `8169793` 加当前未提交补丁；RV64、SMP=1、256 MiB glibc pub 镜像
- 证据：镜像内 `/tmp/cagent_debug_matrix_{1,2,4,10_fixed}/`、
  `/tmp/cagent-matrix-10-fixed-recovery-e2fsck.log`；`scripts/cagent_debug.sh`
- 结果：`kernel` 单项 pass（5492 ms）；2 并发 `factorial`/`date` pass（8823/8457 ms）；4 并发
  `factorial`、`date`、`network`、`cpu` 的 fixed command、agent、validation exit 均为 0。
  10 并发的全部 fixed command 完成；`kernel` 和四个 FS agent/validation exit 为 0，
  `factorial`、`date`、`network`、`cpu` 以 143 结束。所有已启动 worker 都写出了 duration，
  但顶层 runner 未回到 shell。随后不含 agent 的 `server-interrupt` probe 对阻塞在
  `accept(2)` 的 server 发送 SIGTERM 并 `wait`，约 813 ms 正常完成，排除了该清理路径。
  不经网络的 10 路 `busybox timeout 3 busybox sleep 60` 也分别耗时约 13--18 秒且都以
  143 结束；因此超时膨胀并非 simple_llm_server 的串行请求处理所致，但尚须用 1/2/4/10
  的纯 timeout 基线确定是 timer wakeup 还是高并发 fork/exec 启动排队。
  修复 RV64 kernel-mode timer rearm 后，标准 QEMU stdio 控制台上的单项 `kernel` 仍 pass（4992 ms）；
  但 2 路纯 timeout 在 90 秒内未回到 shell，因诊断被中止而没有可采信的每项 duration。
  这反驳了“仅缺 kernel-mode timer rearm 即可恢复”的假设，下一步需审计 `wait4` 子进程退出唤醒和
  ready queue 状态转换。
- 诊断修正：最初 runner 从 `/` 启动，导致 `fs-search` 误执行 `find /`。runner 现强制
  `cd /glibc`，修正后的 10 并发中 `fs-search` command 和 agent 均成功；这不是内核 FS 缺陷。

## 2026-08-03 RV64 内核态 timer interrupt 重新编程

- 状态：已修复并完成构建检查；CAgent 并发回归待使用可靠控制台重跑
- 适用范围：RV64 `trap_from_kernel` 收到 `SupervisorTimer` 时；提交 `8169793` 加当前未提交补丁
- 证据：`os/src/arch/rv64/trap/mod.rs`；
  `make RV_MODE=debug RV_USER_FEATURES= build-rv`、两个 `cargo fmt --check`、`git diff --check`
- 内容：用户态 timer trap 原本会调用 `set_next_ti_trigger()`，而内核态同类 trap 只记录日志。
  已改为重设下一 tick 并执行 `check_all_task_timers()`；不在嵌套 kernel trap 中调用调度器。
  这是避免长 syscall 跨 tick 后遗留已到期 timer interrupt 的必要语义修复。它是 CAgent
  并发超时的候选根因之一，尚无修复后受控 matrix 数据，不能据此宣称 CAgent 已恢复。

## 2026-08-03 CAgent server 等待路径进一步定位

- 状态：TCP 忙轮询和信号发布竞态均已修复并在 guest 验证；10 路容量限制仍是 server 串行服务
- 适用范围：RV64、SMP=1、256 MiB pub 镜像；提交 `8169793` 加当前未提交补丁
- 证据：`os/src/net/tcp.rs::TcpSocket::block_on`、`os/src/syscall/process.rs::wait_block_current`、
  `testsuit/cagent-test/simple_llm_server.c`；`make RV_MODE=debug RV_USER_FEATURES= build-rv`；
  `/tmp/respos-rv-pub-output.txt`
- 结果：单个 `sleep 3` 正常完成；`timeout 3 sleep 60` 在 15 秒采样窗口内以 signal 15 结束，
  支持 wait4 可中断修复生效。相反，启动 server 后的 runner 在其前置 `sleep 1` 即停滞，
  日志目录仅有空 `server.log`，没有任何 timeout worker 文件。`TcpSocket::block_on` 原先对
  `EAGAIN` 仅 `yield_current_task()`；当没有其它 ready task 时会在 accept syscall 内忙轮询，
  不能可靠驱动其它等待任务。现改为登记短 deadline、置 blocked、切换后再 poll，避免该饥饿。
- Linux ABI 验证：改良的 `server-read-interrupt` probe 通过 FIFO 建立但不发送 HTTP 数据的连接，
  使 server 阻塞于 `handle_client()` 的 `read(2)`；随后 SIGTERM server 并 wait。RV64 guest
  正常回到 shell、runner exit 0。这证明该 read 可被信号中断，先前 probe 卡住是其
  `sleep | nc` 后台管道的回收错误，不是内核 read/EINTR 失败。

### 后续验证（当前未提交 TCP 补丁）

- 2026-08-03 已完成：RV64 build、两个 `cargo fmt --check` 与 `git diff --check` 均通过。
  在干净 pub 镜像、SMP=1、256 MiB 上，server 在场的 2 路 `timeout 3 sleep 60` 正常返回；
  两个目标均被 SIGTERM 终止、runner exit 0。这验证 TCP EAGAIN 忙轮询造成的 sleep/watchdog
  饥饿已解除。
- CAgent 小矩阵：2 路 `factorial`/`date` pass（8264/8862 ms）；4 路 `factorial`、`cpu`、
  `date`、`network` 全部 pass（16329、19693、20083、20078 ms），runner 均 exit 0。
- 10 路：runner 正常收敛、exit 0，但仅 `fs-create`（46715 ms）、`factorial`（58140 ms）、
  `fs-usage`（66900 ms）、`fs-readwrite`（74272 ms）pass；`kernel`、`cpu`、`date`、`network`、
  `fs-search`、`fs-directory` reject。后六项 agent exit=143；其中 fixed command exit=0，
  说明 reject 层是 agent watchdog，非对应 command syscall。server log 记录串行处理 HTTP 请求，
  各 agent 需要两轮请求；在 SMP=1 下总耗时 46--78 秒，超过官方各项 20--35 秒 watchdog。
  因此 TCP/调度死锁已解除，但官方单线程 server 的应用层串行容量仍是完整 10 路无法通过的
  直接原因；不能把这些 reject 归为 FS、uname、nproc 或 TCP ABI 失败。
- 串行/并行对照：同一镜像、同一内核下，4 项 `factorial,date,network,cpu` 串行均 pass，
  单项为 3971、4420、5536、4367 ms（合计约 18.3 s）；相同 4 项并行墙钟约 20.1 s。
  10 项串行时每项均 pass，单项 4203--7080 ms、合计约 50 s。这证明单线程 server 的
  串行服务成本本身已超过“10 项各自 20--35 s、同时启动”的可行总窗口；并发仅令请求排队，
  不增加 server 吞吐。
- 结论边界：这不能证明 10 路超时完全与内核性能无关；fork/exec、TCP、FS 和调度开销仍会影响
  每个 4--7 秒服务槽的时长。但在当前实测下，server 单线程的串行总服务时间已足以独立超过
  10 个同时启动 worker 的 20--35 秒 watchdog，SMP=1 改为多核也不能让该 server 并行处理。

## 2026-08-03 CAgent `popen`/动态 exec 分层耗时（已验证）

- 状态：HTTP 不是当前主要耗时；动态 `/bin/sh` 路径在并发下明显放大，尚未实施优化
- 适用范围：RV64、SMP=1、256 MiB pub 镜像；提交 `8169793` 加当前未提交补丁
- 证据：host `/tmp/cagent-profile.oC8qkH/{single.log,four-*.log,fp*.log,dash*.log}`；诊断二进制仅
  创建于 `/tmp` 并临时注入镜像，未替换官方 `agent_lite` 或修改上游 CAgent 源码。
- 结果：带单调时钟标记的单 agent `factorial` 为 4374 ms，其中两次 HTTP `connect/send/recv`
  往返各约 10--20 ms，`popen()` 返回前约 758 ms，`pclose()` 约 2 ms。4 路
  `factorial,date,network,cpu` 时 HTTP 往返仍为约 20--245 ms，而 `popen()` 返回前分别为
  4585、8331、8970、12322 ms，`pclose()` 约 2--44 ms。
- fork/exec 微探针：单路和四路静态 self-exec + wait 均约 0.13 s 和 0.3--0.5 s；单路动态
  `/bin/sh`（镜像中为 glibc `dash`）再 exec 静态程序约 0.93 s，四路为约 2.7--3.3 s。
  因此 `popen` 的主要放大点是动态 shell/动态装载及其后续命令，而不是 TCP、`waitpid` 或
  静态 `execve`。
- 源码线索：`sys_execve` 读取整个可执行文件；`MemorySet::try_from_elf_data()` 为 LOAD 段
  逐页分配/复制，并每次重新 `read_dynamic_linker()`；动态 loader 对 libc 等私有 file mmap
  还会走逐页缺页读入。Linux 的共享页缓存/只读映射与此不同。下一步需先以内核级计数或更细
  探针量化 ELF 拷贝、动态 linker read 和 mmap page fault 各自占比，再决定是否做安全的
  只读可执行页缓存；不能以 CAgent 名称特判。

## 2026-08-03 ext4 正常关机持久化屏障

- 状态：已验证（RV64、SMP=1、256 MiB pub 镜像）
- 适用范围：`dev` 提交 `8169793` 加当前未提交补丁；`sys_reboot` 的 ext4 卸载路径
- 证据：`vendor/lwext4_rust/src/blockdev.rs`、`os/src/fs/ext4/super_block.rs`；
  `/tmp/ext4-flush-pre-e2fsck.log`；以当前 `kernel-rv` 启动 pub 镜像、guest 执行 `quit` 后的
  `e2fsck -fn img/sdcard-rv-pub.img`
- 内容：lwext4 的 cache flush/journal stop/umount 原先没有调用已有的
  `KernelDevOp::flush()`，因此没有把 virtio-blk FLUSH 作为关机完成条件。现在卸载后、注销
  block device 前调用该屏障；失败仍注销设备并向 reboot 返回 `EIO`，避免遗留静态 lwext4
  设备指针。wrapper Drop 不再 `unwrap()` 卸载失败。
- 结果：QEMU 经 guest `quit` 正常退出，离线 `e2fsck -fn` 完成五个检查阶段且未报告错误。
  此结果只覆盖正常关机；QEMU 被 `SIGKILL`、或 QEMU 持有镜像时使用 `debugfs`/`e2fsck` 的
  并发宿主写入仍不安全，必须先停止 QEMU。

## 2026-08-03 A/B/C 队友提交整合验收

- 状态：源码与项目文档已整合；RV64/LA64 debug 构建通过；完整 RV64 CAgent 并行回归仍待定位
- 适用范围：`dev`，提交 `8169793`；包含队友网络并发修复 `5f77068`、文件系统修复
  `40d745a` 和本地任务 B 的 `8169793`，以及此前已合入的 writable file `MAP_SHARED` 修复
  `8a53c43`
- 最后验证：2026-08-03
- 证据：`git log --graph --oneline`、`git merge-base --is-ancestor`、
  `make RV_MODE=debug RV_USER_FEATURES= build-rv`、
  `make LA_MODE=debug LA_USER_FEATURES= build-la`、`cargo fmt --check`、`git diff --check`
- 结果：rebase 冲突已解决；A/B/C 相关提交均位于当前 `dev` 历史中。RV pub 镜像仍能启动到
  `Rust user shell`，LA64 debug 内核也能构建完成；使用已生成的 `kernel-la` 直接配合
  `img/sdcard-la-pub.img`、`-smp 1` 启动也进入了 `Rust user shell`。本轮 `make run-la-pub`
  的 release 构建另遇到 lwext4 CMake 生成目录的 `getcwd: No such file or directory`，属于
  构建目录问题，不能当作 LA 内核运行失败。
- CAgent 观察：使用当前 `kernel-rv`、`img/sdcard-rv-pub.img`、`-smp 1` 直接执行官方
  `/glibc/cagent_testcode.sh` 时，server 收到 10 个请求，但记录为 10/10 reject；各项耗时约
  43–60 秒，超过官方 20–35 秒的单项 timeout。日志中未见启动失败，参考
  `/tmp/respos-integrated-rv-cagent.log`。这轮结果只能说明当前“10 个并行 agent + 单核调度”
  回归未通过，不能据此判定 A/B/C 的 syscall 语义全部失败；单项固定命令和 B 的三个 agent
  链路此前已通过。进一步的当前提交单 agent 复现中，`kernel` agent 得到
  `6.10.0-dev`，agent exit `0`、最终回答成功；这支持“基础 TCP/`uname` 链路可用”。下一步
  应保留每个 agent 的 exit/validation 日志，区分调度超时、server 并发处理和具体内核接口。

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
## 2026-08-02 题一任务 B 文件系统三项修复

- 状态：已验证（RV64、SMP=1；三项固定命令和对应 agent 链路均通过）
- 适用范围：`fs-create`、`fs-readwrite`、`fs-directory`；glibc pub 镜像
- 证据：`os/src/fs/file.rs`、`os/src/fs/ext4/inode.rs`；`/tmp/respos-task-b-command-exact-fixed.log`、
  `/tmp/respos-task-b-fs-create-agent-fixed.log`、`/tmp/respos-task-b-fs-agents-fixed2.log`；命令为
  `make RV_MODE=debug RV_USER_FEATURES= build-rv` 后以 `kernel-rv`、`img/sdcard-rv-pub.img`、
  `-smp 1` 启动 QEMU
- 内容：glibc `touch` 会携带 Linux 合法的 `O_NOCTTY`（`1<<8`），此前
  `validate_open_flags()` 将其误判为未知标志并返回 `EINVAL`；现在作为无副作用的合法 open
  flag 接受。新建目录不再安装 synthetic inode，而是在 `mkdir` 后回读并绑定真实 ext4 inode，
  避免随后创建 `dir/file` 时把 synthetic inode 作为父目录传入 lwext4。
- 结果：固定命令 `mkdir -p test_dir && touch test_dir/file1 test_dir/file2 test_dir/file3 &&
  ls test_dir | wc -l` 输出 `3`；fs-readwrite agent 得到 `15`，目录 agent 得到 `3`；fs-create
  agent 已输出 `Hello OS` 并成功完成任务。
- 边界：这不是完整 10 项 CAgent 回归；此前并发隔离运行还暴露了
  `simple_llm_server`/runner 的并发连接问题，需要任务 A 单独定位。当前工作副本镜像已被
  QEMU 和 `e2fsck` 修改，正式评分前仍须从保留压缩包恢复并记录镜像状态。

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
