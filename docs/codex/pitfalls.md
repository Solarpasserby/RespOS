# RespOS 已确认易错点

## lwext4 的 superblock 与 inode 操作必须使用同一把全局锁

- 状态：已确认并修复当前 BuildStorm linker 阻塞
- 适用范围：SMP 下所有 lwext4 C API，包括文件/目录操作、sync、statfs 和 shutdown
- 最后验证：2026-08-08；RV64 8 核官方 BuildStorm
- 证据：`os/src/fs/ext4/{inode.rs,super_block.rs}`；修复前 PC 长期位于
  `ext4_dir_write_entry` 的断言路径，修复后 `ld.bfd` 完成且 collect2/gcc/rustc/cargo 正常退出
- 内容：lwext4 的 mount、block cache 与目录遍历状态位于共享 C 对象中。只串行 inode 操作而让
  superblock flush/statfs/shutdown 并发进入，会破坏其内部状态；增加另一把锁也不能建立正确互斥。
- 后续影响：任何新增 lwext4 入口必须复用 `EXT4_OP_LOCK`，不能按 Rust 模块各建一把锁。

## UNIX stream socket 必须显式传播 peer close

- 状态：已确认并修复当前 cargo/rustc 退出阻塞
- 适用范围：`socketpair(AF_UNIX, SOCK_STREAM)`、connect/accept、read/write/poll
- 最后验证：2026-08-08；RV64 8 核官方 BuildStorm
- 证据：`os/src/net/socket.rs`；修复前 cargo 永久等 `recvfrom`，修复后 linker 及完整编译子进程退出
- 内容：仅靠接收队列是否为空无法区分“暂时无数据”和“对端已经关闭”。端点必须共享 close 状态；
  peer close 后 read 先排空已有数据再返回 0，write 返回 `EPIPE`，poll 同时报告相应 readiness。
- 后续影响：修改 UNIX socket clone/drop/connect/pair 时需一起审查端点引用和 close 状态，不能只修
  阻塞 read 分支，否则 poll 或 write 会继续产生不一致行为。

## RV64 `sret` 前不能提前恢复 SIE 或 user trap vector

- 状态：已确认并修复
- 适用范围：RV64 trap return、exec 初始上下文、signal frame 恢复、SMP timer interrupt
- 最后验证：2026-08-07；8 核 BuildStorm 运行中 GDB CSR 快照
- 证据：`os/src/arch/rv64/trap/{context.rs,trap.S}`；修复前
  `/tmp/respos-rustc-pc-sample{1,2,3,4,5}.txt`，修复后
  `/tmp/respos-rustc-pc-postfix-sample{1,2,3,4,5}.txt`
- 内容：若 `__restore` 先把 `stvec` 指向 `__trap_from_user`，再原样写回带 `SIE=1` 的
  `sstatus`，timer 可在 `sret` 前进入 user trap 汇编。此时 `sscratch` 和 GPR 尚未形成
  user-entry 契约，`csrrw` 后的保存指令会递归 StorePageFault；表面上多个 vCPU 仍活跃，
  实际任务已不能前进。
- 后续影响：恢复汇编必须在写 `sstatus` 前无条件清 `SIE`，保持 kernel `stvec` 直到所有
  可能 fault 的恢复完成，并由 `sret`/`SPIE` 完成最终中断状态转换。不能只依赖某一种
  TrapContext 构造路径清位，因为 signal return 等路径也可能提供保存上下文。

## exec 必须在旧地址空间中清理 sibling thread 的用户地址

- 状态：顺序已修正，远端 sibling 的协作式终止协议已实现
- 适用范围：多线程 exec、robust futex、`clear_child_tid`、共享 `MemorySet`
- 最后验证：2026-08-07；RV64 8 核 BuildStorm 受控 trace
- 证据：`os/src/task/task.rs::close_other_threads_for_exec()` 与 `exit_process_group()`；
  `os/src/task/processor.rs::publish_saved_handoff()`；`/tmp/respos-buildstorm-user-fault-trace.log`
- 内容：线程保存的 robust-list 和 clear-child-tid 是旧用户映像中的地址。若先替换
  共享 `MemorySet` 的内容，再清理 sibling，内核会把旧地址当作新程序地址写零或写
  `FUTEX_OWNER_DIED`，可破坏刚装载的映像。
- 后续影响：必须在替换 MM 前处理依赖旧用户地址的线程私有状态。当前实现采用四步
  quiescence 协议：`request_termination()` 标记 sibling 不可再被 claim → `remove_task()`
  从调度器摘除 → spin-wait `has_cpu_owner()` 等待远端 CPU 切回 idle → 安全清理。
  `publish_saved_handoff()` 对已标记 `terminate_requested` 的 task 不再重新发布到 ready
  queue，而是静默丢弃。`can_be_claimed_on_cpu()` 和 `try_claim_running_on_cpu()` 均拒绝
  已标记终止的 task。

## exec argv/envp 不能用很小的固定项数上限

- 状态：已确认并修复当前 BuildStorm E2BIG
- 适用范围：`execve`/`execveat`、动态工具链、大环境表
- 最后验证：2026-08-07；RV64 8 核 cargo/rustc
- 证据：`os/src/mm/mod.rs::extract_cstrings_from_user()`；
  `/tmp/respos-minibuild-pipe-output.log`
- 内容：argv 和 envp 共用的 32 项限制使 cargo 启动 rustc 时返回 `E2BIG`，cargo 只能报
  child `never executed`。项数放宽必须同时限制累计字节，否则会把用户指针数转化为无界
  kernel heap allocation。
- 后续影响：调试工具链 exec 必须保留 cargo 捕获的 child stderr；仅看顶层
  `BUILDSTORM_MINIBUILD fail` 会丢失具体 errno。

## 跨 CPU 自旋锁不能防止同 CPU 中断重入

- 状态：已确认并修复当前 heap/timer 路径
- 适用范围：全局 heap、kernel timer trap、任何同时被 syscall 和中断访问的状态
- 最后验证：2026-08-06；RV64 debug、2/4/8 核
- 证据：`/tmp/respos-smp8-gdb-bt1.txt`、`/tmp/respos-smp2-dynamic-bt.txt`；
  `os/src/mm/heap_allocator.rs`、`os/src/arch/rv64/trap/mod.rs`
- 内容：一个 CPU 持有普通 spin lock 时若仍可被使用同一把锁的中断打断，中断会永久等待
  被它自己暂停的临界区。实测的两种形态是 heap dealloc 被 timer 中断后再 alloc，以及
  `ACTIVE_ITIMER_TASKS.remove()` 被 timer 中断后再 lock。其他 CPU 的锁等待只是级联现象。
- 后续影响：GDB 审查要保留“中断栈下方的被打断栈”，不能只看顶层等锁 CPU。修复时要么
  让底层锁关本地中断，要么把中断工作延迟到无锁安全点；不能仅增加跨 CPU 锁或换 allocator。

## `CLONE_VFORK` 必须先登记父进程 blocked 再发布子进程

- 状态：已确认并在当前工作树修复
- 适用范围：glibc `vfork`/`posix_spawn`/`popen`，以及所有带 `CLONE_VFORK` 的 clone 调用
- 最后验证：2026-08-06；RV64、SMP=1 的原语义回归与 SMP=8 BuildStorm 受控 trace
- 证据：`os/src/syscall/process.rs::sys_clone()`、`os/src/task/task.rs::execve()` 与
  `exit_process_group()`；修复后的 4 路 CAgent pass 日志
  `/tmp/cagent_debug_vfork_fix_4_busybox/`
- 内容：Linux 语义要求 vfork 父任务一直阻塞到子任务成功 exec 或 exit。仅调用
  `blocking_and_run_next()` 而不建立子到父的唤醒边，会使父任务错误地等到子命令退出；普通 exit
  的 SIGCHLD 唤醒会掩盖这个错误，造成 `popen` 很慢而 `pclose` 几乎立即返回。SMP 下还有更窄的
  窗口：若先发布 child、后把 parent 加入 blocked 表，child 可先 exec 并把一次性 wake 丢掉。
  cargo/rustfmt 曾表现为捕获输出不返回，但最新 trace 已确认顺序修复后 parent 正常恢复且 rustfmt
  完成退出，仍有独立的 pipe 引用未释放问题；两者不能混作同一个根因。
- 后续影响：vfork 同步必须是一次性且只限该 clone 关系；exec 仅在新映像状态完整后释放，退出路径
  也必须释放以覆盖 exec 失败。父 waiter 必须先登记，child 才能进入 ready queue；不要以普通
  SIGCHLD、yield 或单核通过代替该协议。

## lazy VMA 销毁不能按虚拟跨度逐页扫描

- 状态：回收复杂度已修复，完整 BuildStorm 仍待验证
- 适用范围：进程退出、munmap 辅助路径、稀疏匿名/文件 VMA
- 最后验证：2026-08-06；RV64 8 核 cargo/rustfmt exit trace
- 证据：`os/src/mm/memory_set.rs::MapArea::unmap()`；
  `/tmp/respos-buildstorm-exittrace.log`
- 内容：lazy VMA 可能保留很大的虚拟区间但只有少量 resident frame。销毁时遍历整个 VPNRange 会让
  exit 成本与地址预留大小成正比。Framed area 应只遍历 `data_frames`；Direct area 才按完整
  区间解除既有恒等映射。最新 rustfmt trace 已越过进程退出，但 pipe open-file 仍有额外引用，
  因此 sparse unmap 优化不能作为 EOF 问题已解决的证据。
- 后续影响：对 lazy 结构做回收、fork、mprotect 或统计时优先按 resident metadata 工作；需要改变
  整个 VMA 属性时操作区间元数据，不能隐式扫描每个潜在页。

## pub 镜像不能在 QEMU 运行时由宿主修改

- 状态：已确认
- 适用范围：`img/sdcard-*-pub.img`、QEMU、`debugfs`、`e2fsck`
- 最后验证：2026-08-03
- 证据：RV64 pub 镜像的正常 guest `quit` 后 `e2fsck -fn`；lwext4 卸载路径和
  `/tmp/ext4-flush-pre-e2fsck.log`
- 内容：guest 正常 reboot 会依次停止 journal、卸载 ext4 并等待 virtio-blk FLUSH；只有看到
  QEMU 退出后，宿主才可运行 `e2fsck` 或 `debugfs -w`。强制终止 QEMU、在其持有 raw image
  时修改镜像、或在 gzip 解压尚未完成时运行 fsck，都会造成 journal recovery、短读或元数据
  不一致，不能归因于单个 CAgent syscall。
- 后续影响：每轮写入诊断 runner 前先恢复完整镜像并 `e2fsck -pf`；测试后先正常 guest
  `quit`、确认 QEMU 退出，再离线提取或修改文件。不可恢复的强制停止后应从压缩基线恢复镜像。

## 多核诊断应使用 QEMU `-snapshot`

- 状态：已确认
- 适用范围：RV64 SMP 压力、超时、GDB 暂停和所有不需要持久保存 guest 写入的诊断
- 最后验证：2026-08-05；RV64 `-smp 8 -m 256M` timeout/exit 压力
- 内容：SMP 压力常需超时后暂停 QEMU 或接入 GDB，不能保证 guest 有机会正常卸载 ext4。直接对
  raw pub 镜像运行会把临时 `/tmp`、journal 及非正常退出混入后续结果。
- 后续影响：诊断启动命令在 raw drive 之外加入 `-snapshot`，例如
  `qemu-system-riscv64 ... -snapshot -smp 8 -m 256M`。只有确实需要保留 guest 文件时才退出 snapshot
  模式，并遵守上一节的正常 guest `quit`/离线 fsck 流程。

## `make rv/la` 返回 0 不等于测试通过

- 状态：已确认
- 适用范围：完整 QEMU 测试
- 最后验证：2026-08-01
- 证据：当前 `make rv`/`make la` 均退出 0，但 LTP 各有 600 余项失败
- 内容：顶层命令通过 pipefail 运行 QEMU 并 tee 日志；testrunner 即使记录用例失败仍会跑完并主动
  关机，因此外层成功主要表示 QEMU 正常结束。
- 后续影响：验收必须解析 summary、TBROK、segfault、wrapper exit code 和分组 fail。

## writable file `MAP_SHARED` 拒绝造成全局级联

- 状态：已确认
- 适用范围：basic、lmbench、LTP 以及依赖共享控制页的用户程序
- 最后验证：2026-08-01
- 证据：`os/src/fs/file.rs::mmap_allowed`、`os/src/syscall/mm.rs`、当前双架构日志
- 内容：当前策略返回 `EOPNOTSUPP`。LTP 框架自身使用 fd-backed writable shared page，所以看似
  无关的 getpid/fcntl/fs 等用例在初始化时一起 TBROK。
- 后续影响：不要逐个修数百个 LTP case；先修 shared file mapping 协议。也不要直接删除检查，
  因为旧实现仍有锁内 I/O 和写回错误传播风险。

## MM 锁内后端 I/O 是长期锁序风险

- 状态：已确认
- 适用范围：file fault、shared unmap/writeback
- 最后验证：2026-08-01
- 证据：`os/src/mm/memory_set.rs`、`docs/四天内核重构-ABC-整合审查.md`
- 内容：部分 file backing fault/writeback 仍可能持有 `MemorySet` 写锁访问文件后端；munmap 写回
  错误也没有完整传播协议。
- 后续影响：设计 writable shared 时把文件读写移到地址空间锁外，并在重新加锁后校验映射版本
  或身份，再 commit。

## LTP 的首个框架错误会污染后续结论

- 状态：已确认
- 适用范围：LTP harness 调试
- 最后验证：2026-08-01
- 证据：当前 mmap 初始化级联；`user/src/bin/testrunner.rs`；历史 `pipe2_02` helper 级联记录
- 内容：测试可能在 `tst_test.c` 资源准备、mount、helper exec 或共享控制页阶段失败，尚未进入目标
  syscall assertion。后续大量相同错误不是大量独立内核缺陷。
- 后续影响：先定位第一个不同的失败点，并从 testsuit/image 脚本确认它属于 setup 还是测试主体。

## 未跟踪用户程序仍会进入本地构建

- 状态：已确认
- 适用范围：`user/src/bin/` 与内核内嵌应用
- 最后验证：2026-08-01
- 证据：`user/Makefile` 的 `$(wildcard src/bin/*.rs)`；当前构建日志编译了未跟踪 FS probe
- 内容：Git 未跟踪不等于构建不可见。任何放在 `user/src/bin` 的 `.rs` 都会被构建并可能嵌入内核，
  影响大小、编译时间或应用清单。
- 后续影响：报告可复现结果时附 `git status --short`；干净 CI 与本地 dirty tree 可能产物不同。

## futex cmp-requeue probe 默认构建不是有效竞态测试

- 状态：已确认
- 适用范围：`task_a_futex_cmp_requeue_probe`
- 最后验证：2026-08-06
- 证据：`os/src/task/futex/wait.rs` 的 `TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD`；
  `docs/四天内核重构-ABC-整合审查.md`
- 内容：probe 期待 changer 必定在线性化点前改值；只有用
  `TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD=1` 编译内核才会强制形成该窗口。默认内核直接运行时，
  cmp-requeue 合法地先完成并返回 affected count，probe 会产生伪失败。
- 后续影响：专项测试后必须恢复默认 build。即使专项 build 一次通过，也要记录重复轮数和任何
  timeout；不能删除非收敛样本。

## RV/LA 构建共享活动 Cargo config

- 状态：已确认
- 适用范围：顶层和子目录构建
- 最后验证：2026-08-01
- 证据：`Makefile`、`os/Makefile`、`user/Makefile`
- 内容：构建目标把架构模板复制到同一个 `.cargo/config.toml`。并行运行两个架构构建可能在
  config 被覆盖时使用错误 target/linker 设置。
- 后续影响：双架构顺序构建；若未来需要 `make -j`，先改成互不共享的配置/target dir 协议。

## 比赛镜像是可变测试状态

- 状态：已确认
- 适用范围：mount、文件、LTP、benchmark
- 最后验证：2026-08-01
- 证据：顶层 QEMU 以 raw 可写方式挂载 `img/sdcard-*.img`；`scripts/get_img.sh`
- 内容：测试会创建 `/tmp`、`/etc`、symlink、benchmark 文件和 mount backing。前一轮异常退出可能
  改变后一轮失败形态。
- 后续影响：难以解释的非确定性首先用保留的 `.xz` 恢复镜像，再比较；记录镜像版本和 hash。

## glibc 工具会暴露未覆盖的合法 open flag

- 状态：已确认
- 适用范围：glibc pub 镜像中的 coreutils/CAgent 文件命令
- 最后验证：2026-08-02
- 证据：`os/src/fs/file.rs::OpenFlags`、`os/src/syscall/fs.rs::validate_open_flags()`、
  glibc `/usr/bin/touch` 的 `EINVAL` 复现
- 内容：BusyBox 和 glibc 工具的等价操作不一定使用相同的 open flag。此次 glibc `touch`
  携带 Linux ABI 规定的 `O_NOCTTY`，旧实现因未声明该位在进入 namei 前返回 `EINVAL`。
- 后续影响：遇到用户态工具特定失败时，先保存实际 syscall flags 并对照 Linux ABI；对纯兼容、
  不改变普通文件状态的合法 flag 可接受为 no-op，不能按测试名添加特判。

## 网络 `Connection refused` 可能是 ready 时序而非 ABI

- 状态：已确认并修复（当前工作树）
- 适用范围：多个 client 同时连接同一个 smoltcp TCP listener
- 最后验证：2026-08-03
- 证据：CAgent debug 的 agent exit 255 与 `Connection failed to 127.0.0.1:8080`；
  `os/src/net/listen.rs`；修复后 RV64 三轮官方 CAgent 日志
- 内容：服务端已经 listen 不代表单个 smoltcp listener 能承接同一轮 poll 中的全部 SYN。旧实现等
  userspace `accept` 时才补 listener，第一个握手占用 listener 后，其余并发 SYN 会被 reset。该错误
  会随机落到任意 agent，因而可能伪装成 `kernel`、`cpu` 或 FS 命令失败。
- 后续影响：先保存 agent 原始日志和退出码；若是 connect 失败，不要修改 `uname`、`nproc` 等固定
  命令。监听实现必须按 backlog 提前提供容量，而不是在客户端硬编码重试。

## CAgent 全量 reject 可能来自单线程 LLM server 排队

- 状态：已初步定位，仍需并行/串行对照确认
- 适用范围：题一 `/glibc/cagent_testcode.sh`，`SMP=1` 的 10 个并行 agent
- 最后验证：2026-08-03
- 证据：`testsuit/cagent-test/simple_llm_server.c` 的 `listen(server_fd, 10)`、主循环中的
  `accept` 后直接 `handle_client(client_fd)`；`/tmp/respos-integrated-rv-cagent.log`；当前整合
  提交的单 agent `kernel` 复现
- 内容：官方 runner 同时 fork 10 个 `agent_lite`，但固定 server 不为每个连接创建线程或子进程，
  请求处理是串行的。当前全量运行虽收到请求，却让各项耗时达到约 43–60 秒并超过官方
  20–35 秒 timeout；单 agent `kernel` 能正常获得 `6.10.0-dev` 并以 exit 0 完成。
- 后续影响：不能把这轮 10/10 reject 直接归因于 `uname`、文件系统或 TCP ABI。先用保留日志的
  单项/串行 runner 复现，再单独测 server 并发；A 的 listener backlog 修复仍需保留，因为它
  解决的是连接承载能力，不能消除 server 应用层串行排队。

## 在待恢复 journal 的镜像上写入 debug 文件会丢失

- 状态：已确认
- 适用范围：从 `.gz` 恢复 pub ext4 镜像后使用 `debugfs -w`
- 最后验证：2026-08-03
- 证据：RV64 镜像注入的 `/glibc/cagent_debug.sh` host/image SHA-256 一致，但首次启动 journal
  recovery 后 guest 中变为 0 字节；先运行 `e2fsck -pf` 后重新注入可正常执行
- 内容：压缩包中的 ext4 可能带 `needs journal recovery`。在回放旧 journal 前直接写新 inode，启动
  时的恢复会用旧元数据覆盖该写入。
- 后续影响：只读查看不受影响；临时写镜像前先恢复 journal 并再次校验内容。正式干净回归不要
  注入 debug 文件。

## lwext4 的 CMake 生成目录也跨架构共享

- 状态：已确认
- 适用范围：RV64/LA64 快速切换构建
- 最后验证：2026-08-03
- 证据：`vendor/lwext4_rust/build.rs`、`c/lwext4/Makefile`；失败日志中 LA C compiler 配合 RV
  `ar/ranlib`，手动顺序执行目标架构 `make musl-generic ARCH=...` 后重试成功
- 内容：除了活动 Cargo config，lwext4 两架构还共用 `c/lwext4/build_musl-generic`。切换架构时
  可能出现 C compiler 已切换但 archive 工具仍来自上一架构的混合配置。
- 后续影响：不要并行构建两架构。若顶层构建在 archive 阶段出现交叉工具混用，先顺序执行
  `make musl-generic ARCH=riscv64|loongarch64 -C vendor/lwext4_rust/c/lwext4` 再重试；长期应把
  CMake build dir 按架构拆分。

## libctest wrapper 的 256 需要解码

- 状态：待验证
- 适用范围：musl libctest static/dynamic
- 最后验证：2026-08-01
- 证据：RV/LA 日志均显示 `run-static.sh`、`run-dynamic.sh exited with code 256`
- 内容：可见子用例大量显示 `Pass!`，但 runner 打印的是 wait status/封装值，尚未确认是脚本真实
  exit 1、状态解码问题还是隐藏子用例失败。
- 后续影响：检查 waitpid ABI 和脚本最终命令，不能把“屏幕上都是 Pass”当成 wrapper 成功。

## 高半区模型与 LoongArch 2 MiB 边界历史问题

- 状态：待验证
- 适用范围：LoongArch mmap、TLS、pthread、页表边界
- 最后验证：2026-08-01
- 证据：当前高半区源码已确认；2 MiB TLS 故障仅来自 2026-06 历史 Codex 记忆，当前源码未重现
- 内容：历史调试曾观察到小匿名 mmap 跨 2 MiB 页表边界与 pthread/TLS 故障相关，并采用 LA
  特定 mmap placement 缓解。当前是否仍需要该缓解尚未专项复验。
- 后续影响：若 LA-only pthread fault 重新出现，优先记录 fault VA、TP/TLS、PMD 边界和 mmap
  placement；在复现前不要把旧解释写成当前缺陷。

## rename/unlink 仍受 path-based ext4 后端限制

- 状态：已确认
- 适用范围：多硬链接、rename 覆盖、unlink 后打开文件
- 最后验证：2026-08-01
- 证据：`os/src/fs/ext4/inode.rs`、`os/src/fs/namei.rs`、整合审查
- 内容：open-file 计数已替代 `Arc::strong_count` 猜测，但后端仍通过 renamed/orphan path 兼容
  path API；多别名的完整 inode-handle/事务语义尚未建立。
- 后续影响：不要用更多路径特判宣称完整 POSIX 语义；复杂 rename 需要统一 inode identity/backup
  设计和失败注入。

## 历史测试成绩会快速过期

- 状态：已确认
- 适用范围：README、汇报和分支比较
- 最后验证：2026-08-01
- 证据：README 历史“600 余 LTP”与当前 LTP 初始化失败并存
- 内容：同一仓库的不同 commit、镜像、内存配置、libc 和 runner 清单会产生完全不同的结果。
- 后续影响：任何成绩必须携带 commit、日期、架构、镜像、命令和 summary；旧记忆只用于寻找线索。
