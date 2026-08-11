# RespOS 当前状态

本文件是快速变化的状态页。更新测试结论时必须同时更新日期、提交和命令。

## 2026-08-11 RV64 16 GiB 启动与完整 BuildStorm（基于 `9bde322`）

- **early/direct map**：RV64 early Sv39 root page table 的 identity 和高半区 direct-map 窗口
  各从 8 个扩为 16 个 1 GiB leaf，覆盖 QEMU virt RAM
  `0x80000000..0x480000000`；FDT 解析后的物理末址上限同步扩为 `0x480000000`。
  实际 frame allocator 上限仍由 FDT 决定，不会因 early 窗口扩大而分配未安装 RAM。
- **16 GiB 启动证据**：QEMU 10.0.2/OpenSBI 1.5.1、`-m 16G -smp 8 -snapshot`
  把 FDT 传入 `0x47fe00000`，当前内核能进入 8 核用户 shell；`/proc/meminfo` 报告
  `MemTotal: 16775168 kB`，`nproc` 为 8。QEMU 直接在宿主启动，实测为
  `NI=-10/CLS=TS`。
- **专项门禁**：同一 16 GiB/8 核 release 客体通过 `fs_writeback_probe normal`、
  `fs_metadata_probe normal`、`fs_namespace_probe`、`unix_socket_block_probe`、
  `buildstorm_file_probe`、`buildstorm_private_map_probe`、`smp_shared_mm_probe` 和
  `frame_reclaim_probe`，各进程均退出 0。RV64/LA64 无 feature release 构建通过。
- **完整 BuildStorm**：当前本地 pub 镜像 SHA-256
  `ccf4844bfa9a1f1284724a2d0a6b3d497017a71b1f66f78d7e38dd76419c1168`，客体执行
  `/glibc/buildstorm_testcode.sh`，toolchain/minibuild 均 PASS，最终输出
  `BUILDSTORM_COMPILE mode=multi ok=true cores=8 bytes=1681000 arch=riscv64`，脚本退出 0。
  Cargo 报告 `19m25s`，axbuild 报告 `1178.08s`；本地旧脚本仍输出无效的
  `elapsed_s=0.00`，因此本轮首先是 16 GiB 正确性验收，不代替新官方镜像/平台计时。
  结束时 QEMU RSS 约 2.8 GiB，宿主 swap 约 2 MiB，未见 panic、fault、OOM 或 ext4 错误。
- **小内存边界**：`-m 512M -smp 1` 仍可进入 shell，并报告
  `MemTotal: 522240 kB`。当前 256 MiB 静态 kernel heap 使 `ekernel` 物理末址约为
  `0x90bce000`；`-m 256M` 在 QEMU 放置 DTB 前即报
  `No enough memory to place DTB after kernel/initrd`。这是当前内核产物大小的独立旧边界，
  不是 16 GiB early map 回归。

## 2026-08-11 Linux/POSIX Phase 3 写回与持久化语义完成（基于 `50fb93a` 的本轮工作树）

- **dirty owner 与 close**：新增 inode-wide dirty-owner registry，强持有 PageCache、inode、filesystem
  和 lower I/O identity，直到脏数据与待提交 data mtime/ctime 都成功清理。`File::drop()` 不再写回；
  128 owners 或单 cache/全局 256 dirty pages 达到阈值后，每个 syscall safe point 最多处理 8 个 owner。
  后台失败保留 dirty/error 并停止无休止重试，新 mutation 会重新允许受控提交。truncate 清掉最后脏页
  时显式回收 clean owner，避免低于阈值的强引用泄漏。
- **同步边界**：新增 RV64/LA64 generic syscall `sync(81)` 与 `syncfs(267)` 及 user wrapper；fsync/
  fdatasync、syncfs、全局 sync、unmount 子树和 shutdown 均先遍历对应 dirty owners，再执行 superblock
  barrier。`sync` 按 Linux void 语义不返回异步错误；syncfs/unmount 和 open-file fsync/fdatasync 返回
  错误。lwext4 只有统一 durability barrier，因此 fdatasync 保守地比最小保证更强。
- **范围、时间与并发**：`sync_file_range` 只提交相交 PageCache pages，不再退化为全文件 fsync；
  `MS_SYNC` 按映射对应文件范围等待写回，`MS_INVALIDATE` 因 buffered/MAP_SHARED 已共享同一 frame 而
  无需第二份 cache invalidation。Ext4Inode 共享发布 data mtime/ctime，并在数据写回后按 generation
  提交；lower truncate 与 PageCache writeback 由 per-inode writeback lock 串行，消除 truncate lower
  完成后旧写回重新扩展 EOF 的窗口。
- **专项与资源闭环**：无 feature RV64 1 GiB/8 核、`-snapshot` 通过
  `fs_writeback_probe normal/phase3`、`fs_metadata_probe`、`buildstorm_file_probe`、
  `buildstorm_private_map_probe`、`smp_shared_mm_probe` 和 `frame_reclaim_probe`。Phase 3 probe 覆盖
  close owner、range/sync/syncfs/msync、132 个短文件及脏 tmpfs unmount；`buildstorm_file_probe`
  新增 32 轮 fork 并发 mmap+pwrite+truncate。计数构建结束为
  `page_cache_pages=0 page_cache_dirty_pages=0 page_cache_lru_entries=0 dirty_owners=0`，dirty peak 128。
  `debug_traces` 的一次性 EIO cursor probe 继续通过。
- **持久化**：从 `img/sdcard-rv.img` 创建临时 qcow2 overlay，不使用 snapshot；第一轮
  `fs_writeback_probe persist-prepare` 经 syncfs 写入并退出，第二次启动同一 overlay 的
  `persist-verify` 读到完整 22-byte payload 后清理，两个 marker 均 PASS。
- **构建与完整压力**：RV64/LA64 debug 构建与最终 RV64 no-feature release 构建通过。QEMU 10.0.2、
  `img/sdcard-rv-pub.img`（SHA-256 `ccf4844bfa9a1f1284724a2d0a6b3d497017a71b1f66f78d7e38dd76419c1168`）、
  `-m 16G -smp 8 -snapshot`、宿主 `NI=-10/CLS=TS` 先通过最新版 Phase 3 probe，再运行原始
  `/glibc/buildstorm_testcode.sh`；toolchain/minibuild/final marker 全部 PASS，最终
  `BUILDSTORM_COMPILE mode=multi ok=true cores=8 bytes=1681000 arch=riscv64`。Cargo `20m05s`、
  axbuild `1217.26s`，脚本退出 0；结束时 QEMU RSS 约 3.0 GiB，未见 panic、fault、OOM 或 ext4 错误。
- **保留边界**：当前仍无硬件 PTE dirty bit，resident writable MAP_SHARED page 采用保守范围写回；
  truncate 后访问已越过新 EOF 的映射尚未实现精确 Linux SIGBUS。这两项属于后续 MM 精化，不影响
  Phase 3 的 page identity、错误可见性和持久化退出门槛。
## 2026-08-11 题一 CAgent 当前并发结果与性能定位（`ab893b0`）

- **状态**：RV64、`SMP=1` 的受控 10 路 CAgent 负载全部通过，2026-08-03 记录的 20--60 秒
  并发超时未在当前提交复现；这不是官方脚本经 judge 解析后的可申报成绩。当前运行使用仓库
  `scripts/cagent_debug.sh`，设置 `SKIP_COMMAND_PROBE=1`，保留与官方脚本相同的 10 个 prompt、
  timeout、`simple_llm_server`、`agent_lite` 和并发启动形态，只省略每个 agent 前额外执行一次固定
  命令的诊断探针。官方脚本依赖工作目录为 `/glibc`，本地 `user_shell` 无法可靠预排 `cd`，因此
  尚未完成原始 `/glibc/cagent_testcode.sh` + `judge_cagent-glibc.py` 的最终确认。
- **配置与命令**：提交 `ab893b0`；构建命令为
  `make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES='perf_counters debug_traces'`，执行成功。随后直接以
  `kernel-rv`、pub 镜像、RV64 `-smp 1 -m 512M` 启动 QEMU，在 guest
  中清零 `/proc/respos_perf` 后执行
  `/bin/busybox env SKIP_COMMAND_PROBE=1 /glibc/cagent_debug.sh all`，立即读取计数并正常退出。
  诊断 kernel 在 `256M` 下因 QEMU 无空间放置 DTB 而未启动，故本轮提高到 `512M`；详细 trace 会扰动
  墙钟，结果只用于活性和瓶颈定位。日志为 `/tmp/cagent-all-trace.log`；单项对照为
  `/tmp/cagent-kernel-trace.log`。
- **10 项结果**：`cpu 667 ms`、`factorial 701 ms`、`fs-usage 619 ms`、`kernel 742 ms`、
  `fs-create 655 ms`、`network 769 ms`、`date 894 ms`、`fs-search 860 ms`、
  `fs-readwrite 853 ms`、`fs-directory 856 ms`，均为 `pass`。独立 `kernel` agent 全链路为
  `132 ms`。因此当前没有证据支持继续把题一阻断归因于单项 timeout、`uname`、FS 固定命令或
  TCP listener 无法承载 10 个并发连接。
- **调度与等待证据**：10 路窗口内 `context_switches=504`、`blocking_switches=264`、
  `scheduler_yields=20`、`timer_preemptions=127`。20 次 yield 全部属于 `tcp_connect`；FS、stdio、
  pipe、futex、process、`signal_time` 和其他网络等待分桶均为 0。clone/exec/wait/exit trace 完整，
  未观察到 waiter 丢唤醒、子进程无法回收、server 清理卡死或 timeout 风暴。一次宿主延迟 100 秒后
  才读取计数的错误运行产生 7,182,645 次 `stdio` yield，已确认是测试窗口污染，不是 CAgent 负载。
- **当前性能热点**：窗口内 `private_file_faults=11645`、`cow_faults=3766`、
  `anonymous_faults=1666`，同时有 `page_cache_hits=11858`、`page_cache_misses=206`；这表明 glibc shell
  和工具的重复 exec、动态 ELF/file-backed mmap 与私有/COW 装页是当前最显著的可优化资源路径。
  但全部测例仍在 0.9 秒内完成，所以它是性能候选而非正确性阻断。后续若优化，应先做 1/2/4/10
  并发的 exec/page-fault 计数与无 feature 墙钟 A/B，不能仅凭 fault 次数改共享/COW 语义。
- **已排除的首要瓶颈**：ext4 `lock_wait_ticks=2644`，相对 `lock_hold_ticks=2731019` 很低；hold 主要
  分布在 read、namespace、attributes、lookup 和 write 的正常工作中，没有长期锁等待。kernel heap
  峰值约 5.9 MiB，也不是内存压力。当前不应优先修改 timeout、wait4、signal、pipe、TCP waiter、
  ext4 锁模型或扩大 heap；除非官方原始脚本复跑给出与本轮不同的证据。
## 2026-08-10 Linux/POSIX 语义与模型重构路线（`7cdae1e` 后）

- 已建立 [linux-posix-refactor-plan.md](./linux-posix-refactor-plan.md)，后续以保持 BuildStorm 不回退、
  修复 Linux/POSIX 可观察差异和收敛内核状态所有权为共同目标。
- 第一阶段先建立 `fs_metadata_probe`，固定目录 chmod 持久化、chmod/chown 失败原子性、hardlink alias
  和时间戳行为；取得 Linux 对照后再修改属性模型。
- 当时已知优先缺口包括目录 chmod 未持久化、属性 override 先于底层成功发布、时间纳秒/realtime
  不完整；ext4 C 库线程安全未证明前继续保留唯一全局锁。

### Phase 3 首轮：PageCache 写回状态与 open-file error cursor（已提交 `9bde322`）

本节保留首轮当时的历史边界；其中“未完成”项已由上方 2026-08-11 Phase 3 完成记录闭合。

- **状态机**：PageCache page 在原有 dirty/write-version 之外记录当前 writeback batch id 与最近失败；
  锁外 I/O 完成后只有 batch id 仍匹配的完成者能结束该页 writeback，且只有内容 version 未变化时才能
  清 dirty。短写、lower write 和 truncate 恢复失败均保留 dirty，并发布到 inode 共享 PageCache 的
  error sequence。
- **错误可见性**：每个新 `File` 在 open 时采样 PageCache error sequence；dup/fork 因共享同一个
  `FileInner` 而共享 cursor，独立 open 各自推进。`fsync`/当前较强实现的 `fdatasync` 在重试数据写回后
  消费一次旧错误；新 open 不继承 open 前已发生的错误。close 仍不返回后台错误，但若同 inode 还有旧
  open-file description 存活，其后续同步接口可观察该错误。
- **受控失败 probe**：`fs_writeback_probe normal` 验证只读 fd 可同步 inode 共享 PageCache、
  `fdatasync`/`fsync` 正常返回及 pipe `EINVAL`。`debug_traces` 内核额外接受
  `/proc/respos_perf` 的一次性 `fail_writeback` 命令；`fs_writeback_probe fault` 已验证 observer 首次
  `fsync -> EIO`、重试成功、新 open 不继承旧错、另一旧 writer 独立收到一次 `EIO`。release 路径既不
  导出也不接受该故障控制命令。
- **门禁**：RV64/LA64 无 feature release 与 RV64 `debug_traces` 构建通过；RV64 1 GiB/8 核、
  `-snapshot`、QEMU `NI=-10/CLS=TS` 同轮通过 writeback normal、metadata、namespace、xattr、Unix
  socket、file、private-map、shared-MM 与 frame-reclaim，debug 客体通过 writeback fault。格式与
  `git diff --check` 通过。
- **完整 BuildStorm**：当前无 feature RV64 工作树、旧 pub 镜像、8 GiB/8 核、`-snapshot`、
  `NI=-10/CLS=TS` 输出 toolchain/minibuild PASS，并最终输出
  `BUILDSTORM_COMPILE mode=multi ok=true cores=8 bytes=1681000 arch=riscv64`，脚本退出 0；Cargo 为
  `31m11s`、axbuild 1896.55 秒。宿主同时运行基线 `debug_traces` BuildStorm，因此墙钟受诊断并行负载
  污染，只作当前改动的完整正确性回归，不作性能比较。
- **交接 trace 复核与当时的 16 GiB 边界**：先在 `ab893b0` 构建 `debug_traces`。宿主能以
  `NI=-10/CLS=TS` 创建 16 GiB QEMU，但 OpenSBI 把 FDT 放在 `0x47fe00000`，超过当时 8 GiB
  early/direct-map 上界 `0x280000000`，因此无内核输出；该地址空间扩展未混入本轮。改用 8 GiB/8 核后
  trace 通过 toolchain/minibuild 并持续进入正式编译，3 小时诊断截止时到达 `irq-framework`，QEMU 仍约
  524% CPU、RSS 3.0 GiB，宿主 swap 约 2 MiB，未见 panic、fault、OOM 或 ext4 错误。该 trace 没有最终
  `BUILDSTORM_COMPILE`，只证明先前长静默不是 inode/ext4 死锁；完整通过结论来自上一条无 feature 轮次。
  16 GiB 阻断已于 2026-08-11 修复并完整回归，见本页首节。
- **保留边界**：当前 `fdatasync` 仍比 Linux 最小保证更强，等价走完整 `fsync`；系统调用 `sync`/
  `syncfs`、unmount 前 inode-wide dirty flush、后台 writeback 与硬件 dirty-bit 精确 mmap 写回仍未实现。
  本轮不取消 close 数据提交，也不声称已完成 Phase 3。

### inode-number 下沉与最后 VFS 引用回收（已提交 `ab893b0`）

- **状态收敛**：Ext4Inode 不再保存 pathname aliases、hidden orphan path、mode/owner/nlink override 或
  内存 xattr；数据、属性、readdir、readlink 和 xattr 均通过真实后端 inode number 操作。metadata raw
  snapshot、纳秒时间补充与 generation 合并到单一 metadata lock。
- **生命周期修正**：首版 inode-number 改造仍用 `open_files` 决定 nlink=0 inode 的 truncate/free，遗漏
  cwd 与 Path/Dentry 引用。当前删除该计数：最后 unlink/rename 覆盖只提交 nlink=0；最后一个
  Ext4Inode Arc Drop 无阻塞入队，syscall 前后及 shutdown 安全点持统一 ext4 锁回收。回收失败保留队列
  重试；正常运行时避免 inode-number ABA。
- **Linux/POSIX 对照**：namespace probe 新增“子进程 cwd 被父进程 rmdir”场景。Linux `/dev/shm`
  输出 `FS_NAMESPACE_CWD_UNLINK_PASS` 与 `FS_NAMESPACE_PROBE_PASS race_observations=1077`；RV64
  1 GiB/8 核输出同一 cwd PASS 与 `race_observations=1200`，旧 cwd 通过 `.` 打开时保持原 inode 且
  nlink=0，新目录在旧引用存活期间未复用 inode number。
- **门禁**：2026-08-10 RV64/LA64 无 feature release 构建通过；RV64 同轮通过 namespace、metadata、
  xattr、Unix socket、file、private-map、shared-MM、frame-reclaim。全新 qcow2 overlay 两次冷启动通过
  metadata 目录 mode `0711` 与 xattr 持久化。8 GiB/8 核 snapshot BuildStorm 短门禁输出
  `BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok` 并进入正式 build-std；随后主动停止，未冒充完整
  timed build。格式检查和 `git diff --check` 通过。
- **第二轮完整 BuildStorm 未完成（交接状态）**：2026-08-10 在提交 `6636cfe` 的当前未提交工作树上，
  重新执行 `make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=` 通过；随后直接在宿主运行 RV64
  `-m 8G -smp 8 -snapshot`，guest 执行原始 `/glibc/buildstorm_testcode.sh`。本轮再次输出
  `BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok`，untimed tg-xtask 为 13.40 秒，并进入正式
  build-std；可见进度先后到达 `ax-posix-api`、标准库和 ArceOS 平台/驱动/MM 依赖。约 49 分钟后按
  用户要求主动终止，未出现最终 `BUILDSTORM_COMPILE mode=multi ok=true`，因此第二轮不能视为完成或
  通过。终止前宿主 QEMU 仍约占用 233% CPU，RSS 约 2.3 GiB，未见 panic、fault、OOM 或明确 ext4
  错误；但 QEMU 实际继承 `CLS=IDL`，且宿主 swap 曾增长到约 3.4 GiB，墙钟和长静默均不能作为性能
  数据。当前只能把 inode/ext4 活性问题列为 `待验证`，不能仅凭本轮长静默断言已定位 inode 死锁。
  QEMU 使用 `-snapshot`，原始 pub 镜像未被本轮修改；进程已终止，无残留 QEMU。
- **未关闭边界**：lwext4 vendor 当前没有完整 orphan-list mount recovery；异常断电恰好发生在 nlink=0
  提交与安全点回收之间时，可能遗留泄漏 inode。当前不以提前 free 换取表面上的即时回收，也不宣称
  已达到 Linux ext4 崩溃恢复。`ab893b0` 的高量 trace 在三小时截止前未完成，但已证明持续编译而非
  inode/ext4 死锁；后续 Phase 3 无 feature 工作树的完整 BuildStorm 已通过，见本页首节。

### Phase 0 语义回归框架（已提交 `4dc52ef`，Phase 1 已校正基线）

- **实现**：新增可独立运行的 `fs_metadata_probe normal|prepare|verify|cleanup`，记录 mode/uid/gid/
  nlink/times、hardlink alias、close/reopen、跨启动目录属性及打开后 unlink 的 `fchmod`；新增同场景 Linux
  C 对照。已知差异使用 `FS_METADATA_EXPECTED_FAIL`，探针继续执行且不会打印对应 PASS。
- **Linux 对照**：2026-08-10 使用
  `cc -std=c11 -Wall -Wextra -Werror -O2 scripts/fs_metadata_probe_linux.c -o /tmp/fs_metadata_probe_linux`
  构建；`normal`、`prepare`、`verify` 均通过，包括 hardlink identity、unlink 后 fd chmod 和目录 mode
  `0711` 跨进程保持。
- **基线校正**：Phase 0 用户库 `stat()` 错把 Linux syscall 79 当作二参数旧式 stat；该编号在
  RV64/LA64 实际是 `newfstatat(dirfd,path,buf,flags)`。因此当时 hardlink 和目录查询的 `-ENOENT` 是
  探针 ABI 假阴性，不是内核 namespace 缺陷。Phase 1 已改为 `AT_FDCWD` 四参数调用并重新验证。unlink
  后 fd 的 `fchmod` 失败则是真实路径依赖，已由 Phase 1 修复。
- **门禁状态**：RV64/LA64 无 feature release 构建通过；RV64 实机运行上述 probe。Linux 对照以
  `-Wall -Wextra -Werror` 重编译复跑通过，`cargo fmt` 与 `git diff --check` 通过。与属性/namei 直接
  相关的最小 LTP 清单已写入重构方案；现有完整 LTP 仍受 writable `MAP_SHARED` 测试框架阻断，未
  声称通过。

### Phase 1 inode 属性事务与 fd 生命周期（已提交 `ea7aaa2`）

- **事务模型**：vendor lwext4 新增 `ext4_setattr()`，在一次 pathname lookup、inode ref 和 transaction
  内按 mask 更新 mode、uid/gid、atime/mtime/ctime。目录不再绕过底层 `chmod`；`chown` 与 suid/sgid
  清除不再拆成两个可部分成功的提交。Rust inode 只在底层成功后失效 raw metadata 并发布缓存，删除
  原先吞掉 `ENOENT` 后仍发布覆盖值的兼容路径。
- **打开后 unlink**：fd 级 `fchmod/fchown/futimens` 使用 ext4 orphan storage path，而不是失效的可见
  旧路径。RV64 probe 中三项均返回 0，随后 `fstat` 观察到 mode `0600`、当前 uid/gid、atime 3 与
  mtime 4；hardlink 两个别名的 inode/nlink/属性一致，normal 场景无 expected failure。
- **目录持久化**：全新 qcow2 overlay 第一次启动 `prepare` 后 fd 与 pathname 均观察 mode `0711`；
  第二次启动同一 overlay 的 `verify` 输出 `FS_METADATA_DIRECTORY_PERSISTENCE_PASS mode=711`。
- **回归**：RV64/LA64 无 feature release 构建通过；RV64 1 GiB/8 核通过 Unix socket、file、
  private-map、shared-MM、frame-reclaim 五项 probe。Linux C 对照同步覆盖 unlink 后三种 fd 属性操作。
  RV64 8 GiB/8 核完整 BuildStorm 输出 `ok=true`、1,681,000 B、脚本退出 0；axbuild timed build
  `1232.95s`。宿主本轮 swap 增长约 1 GiB，故只作为正确性回归，不作性能提升结论。
- **保留边界**：ext4 vendor 仍只持久化 32-bit 秒，wall clock 尚未从平台 RTC 初始化；纳秒、负时间和
  真实 `CLOCK_REALTIME` 是明确未关闭项，不能据本阶段结果宣称完整 POSIX 时间模型。

### Phase 2 inode identity 与 namespace 一致性（首轮已提交 `6636cfe`）

- **身份与缓存**：删除新建文件使用的 synthetic inode，所有 ext4 dentry 以真实后端 inode number
  进入 weak inode cache；hardlink、rename、reopen 与打开 fd 因而共享同一 inode/PageCache。目录 raw
  metadata 从全局 generation 改为每 inode generation，成功 mutation 只失效实际源/目标父目录。
- **路径兼容层**：lwext4 尚无可直接用于所有数据操作的 inode handle，因此 Ext4Inode 暂存全部存活
  alias；rename 同时迁移被缓存目录的后代前缀，unlink 注销单个 alias，最后目录项或 rename 覆盖目标
  进入 orphan path。普通文件与空目录的最后 fd 关闭后再清理 orphan；清理失败保留状态以便后续重试。
- **可观察语义**：删除 `File::get_stat()` 的 ENOENT fake stat 和 `Stat` 转换中的 `nlink.max(1)`；打开
  后 unlink/覆盖 rename 的 fd 现在观察真实 inode 且 `st_nlink=0`。新增 Linux/RespOS namespace probe，
  覆盖 inode 稳定性、hardlink、跨目录/覆盖 rename、目录 nlink、打开后 unlink、打开目录被覆盖及
  fork rename/open 竞态；Linux `/dev/shm` 为 1069 次、RespOS 8 核为 1200 次有效竞态观测，均 PASS。
- **门禁**：2026-08-10 RV64/LA64 无 feature release 构建通过；RV64 1 GiB/8 核 snapshot 同轮通过
  metadata、Unix socket、file、private-map、shared-MM、frame-reclaim。8 GiB/8 核 snapshot 完整
  BuildStorm 在 `NI=-10/CLS=TS` 下输出 `ok=true`、1,681,000 B、脚本退出 0；axbuild 1459.33 秒，
  Cargo 24m07s。该数据是本机正确性回归，不代替评测平台结果。
- **保留边界**：alias 集仍是 lwext4 pathname API 的受控适配，不是真正 path-independent inode handle；
  mutation writer 由 NAMEI 锁串行，lookup 与 mutation 的完整 seqlock/RCU 可见性协议仍待后续 VFS 演进。

## 2026-08-10 ext4 时间戳合并更新（已提交 `7cdae1e`）

- **热点拆分**：在 `perf_counters` 下把唯一的 `EXT4_OP_LOCK` 按 stat/lookup/read/write/readdir/
  namespace/attributes/superblock 分类；这只是诊断分类，不拆锁。旧 RV64 pub image、8 GiB/8 核、
  `-snapshot`、窗口外预构建 tg-xtask 后固定 120 秒 Cargo 窗口中，总 hold 约 68.82 CPU 秒；其中
  attributes 57,751 次获取、hold 54.60 秒（79.3%），namespace 仅 375 次、0.82 秒，确认剩余热点
  主要是时间戳/属性而非目录增删改名。
- **根因与实现**：`Ext4Inode::set_times()` 原按 atime/mtime/ctime 分别调用三个 lwext4 API；每个 API
  都重新执行 pathname walk、inode ref 获取与提交。仓库内 vendor lwext4 新增 `ext4_times_set()`，在
  一次 inode lookup/ref 生命周期内按 mask 更新所选字段并一次提交。Rust 层仍做原有秒值范围过滤、
  ENOENT 打开后 unlink 兼容和内存 time override；全局 lwext4 锁、只读挂载检查及错误返回均保留。
- **同口径 A/B**：组合更新后的 120 秒窗口总 ext4 hold `68.82 -> 44.62s`（-35.2%），attributes hold
  `54.60 -> 30.46s`（-44.2%），block write requests `573,253 -> 295,585`（-48.4%）。两轮均推进至
  `ax-posix-api`；优化轮 PageCache fill `255.8 -> 273.5 MB`，没有以减少完成工作换取低计数，也未见
  panic、fault 或文件系统错误。该窗口仍由 timeout 结束，不代表完整题二通过。
- **自动 atime 语义修复**：进一步计数发现 58,692 次 set-times 中 58,682 次是 read/readdir 自动
  atime，而 mtime 只有 10 次。旧路径把自动 atime 当作显式 utimens，同时刷新 ctime；relatime 随后
  总满足 `atime <= ctime`，导致每次读取重复落盘。自动 atime 现只更新 atime，显式时间修改仍更新
  ctime。最终 120 秒窗口 atime updates `58,682 -> 1,185`、attributes acquisitions
  `59,062 -> 1,565`、attributes hold `28.56 -> 0.95s`，总 ext4 hold `42.75 -> 14.43s`，block write
  requests `293,980 -> 9,663`；PageCache fill 同时 `276.7 -> 321.0 MB`。
- **完整 BuildStorm**：最终无 feature RV64 release、旧 pub image SHA-256
  `9d163855dbb67da561925c74666d0e4fc1856e118640cb4889e88dcaf5f8e25f`、QEMU
  10.0.2、8 GiB/8 核、`-snapshot` 运行 `/glibc/buildstorm_testcode.sh`。依次输出
  `BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok` 和
  `BUILDSTORM_COMPILE mode=multi ok=true cores=8 bytes=1681000 arch=riscv64`，脚本退出 0。
  axbuild 报告 timed build `1348.08s`（Cargo 为 `22m16s`）；guest 时间戳 5 -> 1399 秒包含前置检查和
  未计时 tg-xtask。旧镜像脚本打印 `elapsed_s=0.00`，不作为计时依据。后续实测宿主能创建 16 GiB
  QEMU，但当时 RV64 early map 无法访问其 FDT；因此这是完整正确性回归与本机 8 GiB 数据，不冒充
  正式平台成绩。
- **门禁**：无 feature RV64/LA64 release 构建通过；RV64 1 GiB/8 核无 feature snapshot 同轮通过
  Unix socket、file、private-map、shared-MM 与 frame-reclaim 五项 probe。`cargo fmt` 和
  `git diff --check` 通过。另用 ext4 临时文件验证普通读取为 `atime 190 -> 204`、ctime 保持 190；随后
  显式 `touch -a` 使 atime/ctime 同步变为 206。后续仍应补 chmod/chown 与跨重新挂载持久化专项。

## 2026-08-10 目录 metadata generation 与 16K dentry cache（已提交 `edc0623`）

- **环境**：宿主 available memory 9.7 GiB，load average 约 `0.50/1.79/3.26`；旧 RV64 pub
  image SHA-256 为 `9d163855dbb67da561925c74666d0e4fc1856e118640cb4889e88dcaf5f8e25f`，
  QEMU 10.0.2。该历史轮基于宿主 15 GiB RAM 余量继续使用可复现的 RV64 8 GiB/8 核、`-snapshot`、
  `perf_counters`；窗口外预构建 tg-xtask，reset 后执行 120 秒 arceos build。
- **inode 缓存基线**：`da957ea` 收紧版 tg-xtask 19.29 秒，继续编译至 `ax-posix-api`；
  139,908 次 stat 中 91,509 次命中、48,399 次回源，命中率 65.4%，stat 平均约
  0.131 ms。ext4 lock wait/hold 约 90.7/78.8 CPU 秒，说明主要限制已转向 ext4 串行域。
- **目录缓存**：48,043 次 stat miss 中 24,781 次来自原本不缓存的目录。现为 ext4
  namespace 维护全局 generation；成功 create/link/symlink/unlink/rename/orphan cleanup 后递增，
  目录 raw metadata 只在 generation 匹配时命中，可能更新 atime 的 readdir 后也做 inode 局部失效。
  专项验证 mkdir/rmdir 后父目录
  `nlink 2 -> 3 -> 2`、普通创建和跨目录 rename；385 次 stat 中 367 命中、18 回源、
  11 次有效失效，五项 1 GiB/8 核 probe 通过。
- **收益**：相对不缓存目录的收紧版，stat miss `48,043 -> 30,113` (-37.3%)、
  命中率 `65.1% -> 77.9%`、stat CPU `20.05 -> 17.50s` (-12.7%)、ext4 acquisitions
  `170,405 -> 158,992` (-6.7%)、hold `81.84 -> 78.65s` (-3.9%)。优化轮 PageCache fill 约
  914 MB，高于对照的 348 MB 且达到同一可见阶段，不是完成工作更少。
- **16K dentry cache**：原 1024 项在 Cargo 树中反复任意淘汰叶 dentry。增至 8192 后，
  lookup calls/ticks `30,616/14.03s -> 6,007/3.40s`，stat miss `30,113 -> 8,666`，
  ext4 acquisitions `158,992 -> 102,815`，heap peak `45.6 -> 62.2 MiB`。16K 轮进一步将
  lookup 降至 4,019，eviction `5,426 -> 0`，heap peak 仅增至 65.0 MiB；lookup ticks 已基本
  持平，因此保留 16K 而不继续扩容。最终无 feature RV64/LA64 release 构建通过，
  RV64 1 GiB/8 核同轮通过 Unix/file/private-map/shared-MM/frame-reclaim 五项 probe。性能窗口仍在
  120 秒 timeout，不是完整题二通过。

## 2026-08-10 ext4 inode 原始元数据快照缓存（已提交 `da957ea`）

- **实现与边界**：非 synthetic 普通文件/符号链接 `Ext4Inode` 保存一份
  `ext4_raw_inode_fill` 结果，重复
  stat 不再进入 lwext4 路径解析和全局 ext4 锁。size 仍与 PageCache 长度合并，mode/owner/
  times/nlink 仍在每次 stat 动态应用内存 override。write、truncate、chmod、chown、link、
  unlink、rename、orphan/restore 和可能更新 atime 的 read 成功后均失效 raw 快照；修改路径保持
  先释放 ext4 锁再失效，不引入反向锁序。目录会被 create/unlink/跨目录 rename 间接修改，
  在完整父 inode 失效协议建立前不跨 syscall 缓存目录 raw metadata。
- **计数证据**：提交前审查收紧目录/read-atime 边界之前的 RV64 1 GiB/8 核
  `perf_counters` snapshot 交互轮次记录
  `stat_calls=1314`、`stat_cache_hits=1293`、`stat_cache_misses=21`、`stat_ticks=21066`
  (`clock_hz=10MHz`)。该轮次仅证明原型命中和回源数量；当前收紧版命中率待干净窗口
  重测，且没有同负载旧实现对照，不宣称加速比。
- **语义验证**：在 ext4 根目录创建文件后，依次验证 append `7 -> 12 B`、truncate
  `12 -> 2 B`、chmod `0644 -> 0600`、hardlink/unlink `1 -> 2 -> 1`、rename 后 inode/size/mode
  保持，及现有 `/bin/busybox` symlink size=14。测试暴露并修复了原有 `nlink_override`
  成功 unlink 后不递减的问题。
- **门禁**：RV64 `perf_counters` 构建和 LA64 无 feature release 构建通过；RV64 1 GiB/8 核
  snapshot 同一轮通过 Unix socket、file、private-map、shared-MM 和 frame-reclaim 五项 probe。尚缺
  high uid/gid、>4 GiB 文件及干净 BuildStorm 固定窗口回归。

## 2026-08-10 vendor allocator 长窗口、128 MiB PageCache 与 Unix socket 阻塞（已提交 `da957ea`）

- **allocator 600 秒 soak**：RV64 8 GiB/8 核、旧 pub image、冷 snapshot、`perf_counters` 下，vendor
  allocator 共承受 `20,781,711 alloc/20,634,417 dealloc`，无 assertion、OOM、panic 或数据损坏；
  dealloc total/core 为约 `13.51/8.39s`，相对旧 allocator 同口径 `369.2s` total 大幅下降。tg-xtask
  `29.64s`，600 秒从旧窗口的 axklib 继续推进到 axplat-dyn/ax-hal；该旧镜像窗口仍 timeout，不是题二
  完整通过。PageCache fill/eviction 为约 3.44 GiB/538,698，显示 allocator 闭环后的热点转向缓存 churn。
- **128 MiB PageCache**：两架构 `PAGE_CACHE_GLOBAL_MAX_PAGES` 从 16K 增至 32K；数据仍为 frame-backed，
  不占 256 MiB kernel heap。首轮 120 秒达到 26,744 页且 eviction=0；重复轮达到 32,765 页并发生
  28,958 次 eviction。两轮期间用户确认宿主同时启动其他应用，tg-xtask 为 42.64/39.63 秒，故这些墙钟
  和累计吞吐明确标记为受外部负载污染，不用于 64/128 MiB 速度结论。保留 128 MiB 是基于 600 秒 64 MiB
  cache 已持续满载和 1 GiB guest 仍有足够 frame；干净容量 A/B 仍待复核。
- **Unix socket event wait**：AF_UNIX read-empty、write-full 与 accept-empty 不再 yield polling；waiter 在
  对应 buffer/pending lock 内登记并发布 Blocked，数据、空间、connect、peer close 或 signal 负责唤醒，
  发布后重查 interrupt 关闭 lost-wakeup 窗口。新增 `unix_socket_block_probe` 用 128 KiB 流量覆盖空读、
  64 KiB 满缓冲区写和 peer close，打印 `UNIX_SOCKET_BLOCK_PROBE_PASS bytes=131072`；专项计数为
  `unix_yields=0 scheduler_yields=0 blocking_switches=5`。受宿主负载污染的 120 秒真实 Cargo 窗口仍可作
  活性/语义证据：timeout 正常返回，`net=0 unix=0`，但不能作速度 A/B。
- **最终门禁**：os/user fmt、vendor allocator tests、无 feature RV64/LA64 release 构建通过；RV64
  1 GiB/8 核无 feature snapshot 同一轮通过 Unix socket、file、private-map、shared-MM、frame-reclaim
  五项 probe。尚缺 AF_UNIX pathname accept/connect、非阻塞 EAGAIN 和 signal EINTR 的独立专项组合，
  以及 128 MiB 配置下不受宿主干扰的长 BuildStorm；当前不宣称完整验收。

## 2026-08-10 raw lookup 后的 600 秒 BuildStorm 窗口与 allocator 根因（未提交，继续优化）

- **长窗口进展**：当前未提交 raw stat/lookup、4096 项 lwext4 metadata cache、signal blocking 和
  private frame 共享工作树；RV64 8 GiB/8 核、旧 pub image、`perf_counters`、冷 `-snapshot`，在窗口
  外预排环境和命令后执行
  `timeout 600 cargo xtask arceos build -p arceos-helloworld --arch riscv64`。debug tg-xtask 用时
  `33.65s`，随后进入 build-std 并推进到 `Compiling axklib`；600 秒由 timeout 返回 124，未见 fault、
  panic 或 OOM，因此本轮是性能窗口而不是完整通过。
- **调度与剩余串行域**：`task_running_ticks=25113288899`、`idle_ticks=21845338591`，按 8 个 vCPU 的
  累计观测约 52.3% 时间在运行 task；旧 signal 风暴仍保持 `signal_time=0`。剩余 69,194 次 yield 全部
  来自 Unix socket。ext4 lock wait/hold 分别约 66.4/216.1 CPU 秒；物理 block read/write 分别约
  18.1/25.6 CPU 秒，说明磁盘请求已经不是唯一或最大成本。
- **缓存与分配压力**：64 MiB PageCache 达到 16,384 页上限，600 秒窗口内有 360,749 次 eviction、
  2,375,720,237 B fill；heap 共 17,954,099 alloc 和 17,838,374
  dealloc，peak 约 41.8 MiB。原总计时为 alloc 94.7、dealloc 369.2 CPU 秒，但该数包含全局 heap lock
  等待，不能直接断言都是 allocator 算法。
- **allocator 拆分证据**：为避免误判，heap wrapper 新增 lock-wait/core 分桶；同配置冷 snapshot 的
  120 秒窗口进入 build-std，dealloc 总计 `298181587 ticks`（29.82 CPU 秒），其中 lock wait
  `29022698`（2.90 秒）、buddy core `262602329`（26.26 秒），core 占约 88%。依赖
  `buddy_system_allocator 0.10.0` 的 dealloc 会逐项扫描对应 free list 查找 buddy，确认主要根因是
  O(n) 合并查找而非多核锁等待。该轮 tg-xtask 35.45 秒，并推进到 ax-driver/somehal，阶段与此前接近。
- **实现边界与下一步**：不能修改本机 `.cargo-home/registry` 作为交付；若替换 allocator，源码必须放入
  仓库 `vendor/` 并由 `os/Cargo.toml` 使用 path dependency。新实现必须保持 Layout size/alignment、
  split/coalesce、OOM、统计和 IRQ-safe 全局锁语义，增加随机 alloc/free 不重叠与完全回收测试，再通过
  1 GiB SMP 内存/退出回收门禁。`copy_to_user/copy_from_user` 目前也在读写 syscall 的 kernel bounce
  buffer 两侧形成额外复制，已加 calls/bytes/ticks 计数；在取得占比前不移除用户范围检查、lazy/COW
  fault 处理或部分读写语义。
- **vendor allocator A/B**：新增仓库内 `vendor/respos_buddy_allocator`，`os/Cargo.toml` 使用显式 path
  dependency；没有修改或依赖本机 registry。它保留 8 B 最小 class 和原 buddy split/coalesce/accounting，
  对至少 16 B 的 free block 使用 intrusive doubly-linked node 加 membership bitmap，8 B class 因容不下
  两个指针继续兼容线性查找。120 秒同口径窗口中 tg-xtask `30.56s`，dealloc total/core 从
  `29.82/26.26s` 降至 `4.42/2.88s`（约 -85%/-89%）；PageCache fill 从约 589 MB 增至 943 MB，窗口已
  推进到 ax-driver/somehal。alloc total 为约 5.00 秒，未出现 OOM、allocator assertion 或 kernel panic。
- **用户复制结论**：同一优化后窗口共 `copy_from_user=62,434 calls/19,319,240 B/0.129s`、
  `copy_to_user=62,197 calls/86,632,228 B/0.295s`，合计约 0.424 CPU 秒。当前 bounce buffer 确有额外
  copy 和 allocation，但复制本身不是主要热点；不为这点收益破坏预先 EFAULT 检查、lazy/COW fault、
  file offset、short I/O 或锁顺序。后续只有在热点迁移且有 prepared user-page/scatter-gather 设计及专项
  ABI 回归时才做零拷贝。
- **稳定性门禁**：vendor crate 两组宿主测试覆盖 mixed size/alignment、不重叠、乱序释放、统计归零和
  全释放后大块重新分配；无 feature RV64 与 LA64 release 均构建通过。最终 RV64 1 GiB/8 核无 feature
  snapshot 同一轮依次通过 `BUILDSTORM_FILE_PROBE_PASS`、
  `BUILDSTORM_PRIVATE_MAP_PROBE_PASS file_mb=64 workers=4`、
  `SMP_SHARED_MM_PROBE_PASS rounds=100`、`FRAME_RECLAIM_PROBE_EXIT resident_mb=64 threads=7`。
  这些门禁证明当前 allocator 在短时并发和回收路径稳定，但尚不能替代更长 BuildStorm/allocator soak。

## 2026-08-10 ext4 stat/lookup 消除重复完整路径遍历（未提交，固定窗口 A/B 通过）

- **分桶证据**：4096 项 metadata cache 下的 RV64 8 GiB/8 核旧 pub snapshot，干净 120 秒
  `timeout cargo xtask ...` 窗口中，block read 与完整 `Ext4Inode::read_at` 分别只占约 2.55/2.76 秒，
  但 `EXT4_OP_LOCK` hold 为约 103.2 秒。新增操作分桶后，`stat=26958 calls/39.82s`、
  `lookup=6763 calls/46.87s`，两者合计约占锁内时间 84%；readdir/create/write 均不是该阶段主因。
- **根因与修复**：旧 `stat()` 为同一路径先 open/close 取 size，再分别调用 mode、owner、atime、mtime、
  ctime 五个 lwext4 API，每个 API 都重新完整遍历路径。现在一次 `ext4_raw_inode_fill()` 取得 packed inode，
  按 ext4 little-endian 字段生成 size/mode/uid/gid/nlink/times，并继续应用现有 PageCache size、owner/mode/
  time override。旧 lookup 先从 Rust 逐项调用 FFI 扫描父目录，再按 child path 查 mode；中间版复用 dirent
  type 后仍受逐项 FFI/线性遍历限制。最终版直接对 child path 调一次 `ext4_raw_inode_fill()`，复用 lwext4
  内部 `ext4_generic_open2` 的按名查找/目录索引，同时取得 inode number 和 mode type。
- **同口径进展**：优化前 120 秒窗口 debug tg-xtask 尚未完成，操作分桶为上述 26,958 stat/6,763
  lookup；优化后 debug tg-xtask 在 `1m34s` 完成并进入 ArceOS/build-std 前置，而此前 signal fix 后的
  300 秒样本约 `2m15--2m19s` 才完成该阶段，缩短约 30%。优化后同一 120 秒已处理 59,371 stat 和
  14,282 lookup；stat 总计 10.76 秒、lookup 60.82 秒。按调用数归一，stat 约
  `1.477ms -> 0.181ms`（-87.7%），lookup 约 `6.93ms -> 4.26ms`（-38.5%），且实际完成工作量显著增加。
- **最终 raw lookup A/B**：相对上述中间版，同口径 120 秒的 debug tg-xtask 再从 `1m34s` 降至
  `41.73s`；lookup 由 14,282 calls/60.82s 变为 17,095 calls/4.26s，单次约
  `4.26ms -> 0.249ms`（-94.2%）。窗口内 PageCache fill 从约 269 MB 增至 829 MB，ext4 lock hold
  从约 98.5 秒降至 65.7 秒；优化版在相同时长已执行更多 stat/lookup 并进入 build-std，不能只比较累计
  调用数。
- **正确性与门禁**：`busybox stat /work/tgoskits/Cargo.toml` 返回 size 16825、inode 306533、mode 0644、
  uid/gid 0；另创建 symlink `abcdef`，busybox stat 返回 type=symlink、size=6、mode 0777，readlink 内容
  一致。无 feature RV64/LA64 release 均构建通过。最终无 feature RV64 1 GiB/8 核 snapshot 同一轮通过
  file/private-map/shared-MM/frame-reclaim 四项 probe。尚未运行 high uid/gid/大于 4 GiB file 的专用
  stat ABI probe；当前 raw inode 读取已显式处理 size/uid/gid 高位，但完整 BuildStorm 与更广 libc/LTP
  stat 回归仍是验收要求。

## 2026-08-10 lwext4 元数据 cache 从 16 项扩至 4096 项（未提交，固定窗口 A/B 通过）

- **稳定热点与来源分解**：signal wait 修复后的 RV64 8 GiB/8 核旧 pub snapshot 中，重复的干净
  300 秒窗口有两轮 `net=0`，因此此前单轮约 8.3 万次 net yield 不足以支持 socket 重构。相反，180 秒
  `timeout cargo xtask ...` 基线稳定记录 `ext4_lock_hold_ticks=1633031097`（约 163.3 秒）、
  `block_read_requests=1178126`、`block_read_bytes=4970704896`；其中 1,172,282 个请求位于 513 B--4 KiB
  档。PageCache/Ext4Inode 实际只有 5,211 次、172,554,667 B file-data fill，底层块读取是用户文件内容的
  约 28.8 倍，确认主要是路径查找/inode/extent 元数据放大，而不是继续扩大文件 PageCache 就能解决。
- **根因与改动**：`vendor/lwext4_rust/c/lwext4/CMakeLists.txt` 原把
  `CONFIG_BLOCK_DEV_CACHE_SIZE` 固定为 16；在 4 KiB ext4 上只有 64 KiB 元数据工作集。文件内容读取走
  `ext4_blocks_get_direct()`，不会占用该 cache，因此将两个构建分支统一增至 4096 项（约 16 MiB）以
  保留 Cargo 深目录树的目录块、inode table 和 extent metadata。
- **同口径 4096 项 A/B**：180 秒窗口降至 `block_read_requests=25048`（-97.9%）和
  `block_read_bytes=295823872`（-94.0%）；PageCache fill 已推进到 221,722,602 B，较基线同窗口多约
  28.5%。累计 heap alloc bytes 从 6,382,606,173 降至 1,374,643,521；heap current/peak 为约
  24.2/25.4 MiB，较基线约增加 16 MiB 常驻容量。ext4 hold 仍约 154.5 秒，只下降约 5.4%，说明同步
  lwext4 CPU/锁域仍是后续热点，但已消除绝大多数实际 VirtIO 读和 allocator churn。
- **容量选择**：另测 1024 项（约 4 MiB）：请求/读取已降至 26,966/255,742,464 B，但同窗口 file-data
  fill 只有 172,748,123 B，接近 16 项基线而明显低于 4096 项；ext4 acquisitions 104,273，也高于
  4096 项的 90,605。基于吞吐进度而非只看读取字节，最终保留 4096 项。
- **门禁与边界**：无 feature RV64/LA64 release 均构建通过；RV64 1 GiB/8 核 snapshot 同一轮依次通过
  `BUILDSTORM_FILE_PROBE_PASS`、`BUILDSTORM_PRIVATE_MAP_PROBE_PASS`、
  `SMP_SHARED_MM_PROBE_PASS rounds=100` 和 `FRAME_RECLAIM_PROBE_EXIT resident_mb=64 threads=7`。
  尚未用该 cache 容量完成无 feature 全量 BuildStorm；16 MiB cache 属于内核堆固定开销，后续完整
  运行必须继续观察 heap peak/OOM，并处理 ext4 lock 内剩余 CPU 时间。

## 2026-08-10 signal wait 忙轮询改为阻塞唤醒（未提交，专项与真实 Cargo 窗口通过）

- **定位证据**：旧 `perf_counters` 内核在 RV64 8 GiB/8 核、旧 pub snapshot 的固定 600 秒
  `timeout 600 cargo xtask arceos build ...` 窗口中记录
  `scheduler_yields=19618748`、`signal_time=19618691`、`context_switches=19658036` 和
  `scheduler_ipis=19658737`。FS/stdio/futex/net 分桶均不是该轮主因；源码确认
  `rt_sigtimedwait` 与 `rt_sigsuspend` 每次检查失败后直接 `yield_current_task()`，使外层
  `timeout` 在整个编译期间占用调度器。
- **实现**：两条 signal wait 路径现在把任务发布到 blocked queue，并在发布后重查 pending/interrupted/
  deadline 以关闭 lost-wakeup 窗口。`rt_sigtimedwait` 另在 TCB 发布 wanted mask：进程级信号优先选择
  真正的 waiter，即使该信号按 POSIX 用法已被用户掩码阻塞；目标信号只唤醒并由 sigtimedwait 消费，
  其他可投递信号仍设置 interrupted 并返回 `EINTR`。有限等待复用 timer timeout registry 到期唤醒。
- **专项验证**：RV64 1 GiB/8 核、`perf_counters`、snapshot 中预排
  `busybox timeout 3 busybox sleep 60`、读取计数和 quit，约 3.2 秒返回，sleep 进程由 SIGTERM 结束；
  窗口为 `signal_time=0 scheduler_yields=0 context_switches=18 blocking_switches=9`，证明超时和目标信号
  唤醒均不再轮询。
- **真实 Cargo 窗口**：同一旧镜像、RV64 8 GiB/8 核冷 snapshot，先在窗口外构建 tg-xtask，再将
  reset、`timeout 300 cargo xtask arceos build -p arceos-helloworld --arch riscv64`、proc read 和 quit
  一次性预排。窗口内 debug tg-xtask 用时 `2m15s`（修复前 600 秒样本为 `2m55s`），随后进入
  build-std 并编译到 `ax-alloc`；计数为 `signal_time=0 stdio=0`、`scheduler_yields=83911`（其中
  `net=83099`）、`context_switches=106956`、`scheduler_ipis=107932`、`task_running_ticks=3191284229`、
  `idle_ticks=20802628171`。它与旧 600 秒样本时长不同，不能直接比较累计 ticks，但已把每分钟约
  196 万次 signal yield 降为零。另一次未预排 proc read 的 600 秒样本被 shell stdin 轮询污染，明确
  不用于 running/idle 对比。
- **门禁与边界**：os 格式检查、无 feature RV64/LA64 release 构建通过；第一次 RV64 构建曾在 lwext4
  CMake 重建目录缺少 dependency file，原命令重跑通过，未涉及本次 Rust 代码。尚未重跑无 feature
  完整 BuildStorm，不能据 300 秒窗口宣称题二达标。signal 风暴消除后干净窗口的最大 yield 分桶转为
  net，ext4 hold 约 262.5 CPU 秒；下一轮应先精确解释 net wait 与 ext4 串行占比，再决定后续改动。

## 2026-08-10 private frame 共享后的完整 BuildStorm 运行（主动停止，吞吐仍不足）

- **配置**：`4d41e26` 加当前未提交 private PageCache frame 共享和诊断工作树；无 kernel/user feature
  release，旧 RV64 pub 镜像 SHA-256
  `9d163855dbb67da561925c74666d0e4fc1856e118640cb4889e88dcaf5f8e25f`，QEMU
  `-snapshot -m 8G -smp 8`，执行原始 `/glibc/buildstorm_testcode.sh`。这仍不是公告的 16 GiB 新镜像
  正式环境。
- **阶段结果**：`BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok`；旧镜像 untimed `tg-xtask`
  预构建 `3m15s`，正式窗口内 debug `tg-xtask` 构建 `4m20s` 后成功进入 ArceOS/build-std。最新输出已
  编译到 `ax-task`、`ax-runtime`、`ax-posix-api`、`alloc`、`panic_abort`、`unwind` 和
  `rustc-std-workspace-*`，全程未见 SIGSEGV、ENOMEM、kernel panic 或 heap allocation failure。
- **停止边界**：QEMU 总运行约 2128 秒（35 分 28 秒）时，正式主构建仍处于上述库编译阶段，未生成
  `BUILDSTORM_COMPILE` 或计分产物。宿主侧 QEMU user+sys CPU 约 7548 秒，折合总运行期间平均约
  3.55 个 CPU；available memory 约 6.9 GiB，swap 使用约 2.2 GiB。基于剩余 build-std/ArceOS 工作量
  与此前 6250 秒超时边界，继续运行大概率只能再次得到 timeout，故通过 QEMU `Ctrl-A x` 主动停止。
  该轮不是脚本失败，也不能记为通过或精确正式耗时。
- **结论与下一步**：private frame 共享已显著改善 minibuild 和真实 cargo/rustc 短测，但仍不足以让
  完整 BuildStorm 达标。下一轮不再立即重复完整构建；应使用 `perf_counters` 对 build-std 的固定
  10--15 分钟窗口采样，重点比较 ext4 hold、block 请求、task running/idle、PageCache eviction 和
  allocator，再只处理最高占比热点。当前约 3.55/8 的平均 host CPU 利用提示仍有较大串行/等待比例，
  但在取得正式窗口计数前不直接归因于 ext4、scheduler 或宿主 swap。

## 2026-08-10 BuildStorm 私有只读映射共享 PageCache（未提交，专项 A/B 通过）

- **目的与配置**：基线 `4d41e26` 加未提交的 `buildstorm_private_map_probe`；RV64 1 GiB/8 核、
  `perf_counters` release kernel、旧 pub 镜像 SHA-256
  `9d163855dbb67da561925c74666d0e4fc1856e118640cb4889e88dcaf5f8e25f`、`-snapshot`。探针先创建并
  写回 64 MiB 普通文件，再清零 `/proc/respos_perf`，让 4 个独立进程同步开始逐页读取各自的只读
  `MAP_PRIVATE` 映射；文件准备不计入窗口。
- **修复前基线**：探针打印 `BUILDSTORM_PRIVATE_MAP_PROBE_PASS file_mb=64 workers=4`。干净窗口记录
  `private_file_faults=65812`，接近 4×16384 个数据页加各进程少量装载 fault；PageCache 为
  `hits=65786 misses=36`、最终 16384 页，说明输入已在缓存中，但旧 private fault 仍为每个进程分配并
  复制独立 frame。窗口内 ext4 lock wait/hold 仅约 0.00016/0.40 秒，不支持先拆 ext4 全局锁。
- **测量污染纠正**：最初在 probe 返回 shell 提示符后才交互发送 `cat /proc/respos_perf`，空窗中的
  `Stdin::read()` 轮询累计约 46--113 万次 yield/context switch。将 probe、读取计数和 `quit` 一次性
  预排入串口后，修复前干净窗口只有 `context_switches=369 scheduler_yields=0 timer_preemptions=360`。
  因此历史“private fault 引发调度风暴”结论已撤销；stdio 空闲轮询是真实但独立的问题，不能混入本
  probe 或 BuildStorm 判断。
- **实现与语义边界**：无写权限的普通文件 `MAP_PRIVATE` fault 现在直接引用 PageCache frame，保留只读
  PTE；原生可写 private mapping 继续逐页私有分配。若后续 `mprotect(PROT_WRITE)`，先为所有 resident
  页分配和复制 private frame，再授予写权限，避免修改 PageCache 或其他进程映射。probe 在计数窗口前
  覆盖“只读 private mmap → mprotect 可写 → 写入 → backing file 保持原值”。
- **专项 A/B**：同一 RV64 1 GiB/8 核 snapshot 中，预排命令后的宿主观察墙钟约 `7.1s -> 3.7s`，
  `task_running_ticks 204344690 -> 65740621`，`heap_alloc_bytes 28295957 -> 12900635`；PageCache
  hit/miss 与 `private_file_faults=65812` 基本不变，说明收益来自消除重复 frame/copy，而非隐藏磁盘 I/O。
  修复后 `context_switches=299 scheduler_yields=0`，未引入调度副作用。
- **真实 Cargo A/B**：旧官方镜像、RV64 8 GiB/8 核、`perf_counters`、每轮冷启动 snapshot；先
  `cargo new`，随后 reset 计数并编译同一空 binary crate。优化版 Cargo 自报 `1m16s`，临时仅关闭
  private frame 共享的对照版为 `2m42s`，完成时间缩短约 53%。优化版/对照版的
  `task_running_ticks=844161965/1966678543`、`ext4_lock_acquisitions=74708/186873`、
  `ext4_lock_hold_ticks=680276909/1452468515`、`heap_alloc_bytes=8056249042/10259796996`；两轮均
  `MINIBUILD_RC=0`，无 fault/panic。旧镜像该启动方式没有 `/proc/uptime` 节点，因此不伪造 guest timed
  秒数，A/B 时间采用 Cargo 自身输出；它证明真实 cargo/rustc 路径收益，但仍不替代正式 BuildStorm。
- **门禁**：最终工作树通过 os/user `cargo fmt --check`、`git diff --check`、
  `make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=` 和
  `make build-la LA_USER_FEATURES= LA_KERNEL_FEATURES=`。无 feature RV64 1 GiB/8 核同一轮依次通过
  `BUILDSTORM_PRIVATE_MAP_PROBE_PASS`、`BUILDSTORM_FILE_PROBE_PASS`、
  `SMP_SHARED_MM_PROBE_PASS rounds=100` 与 `FRAME_RECLAIM_PROBE_EXIT resident_mb=64 threads=7`。
  尚未重跑完整 BuildStorm，本节不等同于正式题二成绩。

## 2026-08-09 正常宿主调度优先级下的完整 BuildStorm 复跑（正式阶段超时）

- **基线与配置**：`dev` HEAD `7cb282a` 加当前未提交的 PageCache、退出页回收、诊断和
  devcontainer 工作树；RV64 pub 镜像 SHA-256 为
  `9d163855dbb67da561925c74666d0e4fc1856e118640cb4889e88dcaf5f8e25f`。使用无 kernel feature、
  无 user `eval` feature 的 release kernel，以旧 pub 镜像、`-snapshot -m 8G -smp 8` 运行
  `/glibc/buildstorm_testcode.sh`。该轮使用 8 GiB，且当前内核后来确认无法访问 16 GiB guest 顶部的
  FDT，因此不是公告要求的 RV64 16 GiB 正式资源配置，也不能作为最终比赛成绩。
- **宿主调度前提**：QEMU 启动后实测为 `NI=-10`、`CLS=TS`，不再继承历史 Codex 会话的
  `nice=16/SCHED_IDLE`。运行中 QEMU RSS 主要约 1.2--2.0 GiB，宿主 swap 使用量从约 2.0 GiB
  增至约 4.7 GiB，但 available memory 仍约 7 GiB；本轮超时不能继续归因于 QEMU 被
  `SCHED_IDLE` 饿死。
- **阶段结果**：`BUILDSTORM_TOOLCHAIN ok` 与 `BUILDSTORM_MINIBUILD ok` 均通过；旧镜像特有的
  untimed `tg-xtask` 预构建约 3 分 32 秒，正式区间开始前的 debug `tg-xtask` 构建约 3 分 14 秒。
  正式 `arceos-helloworld` 构建最终打印
  `BUILDSTORM_COMPILE mode=multi ok=false rc=124 elapsed_s=0.00 cores=8 bytes=0 arch=riscv64`，即在
  脚本 6250 秒上限触发 timeout，未生成计分产物。
- **正确性边界**：本轮未出现此前的用户 fault `ENOMEM`、并行 rustc SIGSEGV、kernel panic 或
  heap allocation failure，说明进程组退出页回收修复已经让完整工作负载越过旧正确性阻断；但
  “不再崩溃”不等于 BuildStorm 通过。超时时仍在自定义 RISC-V target 的 build-std/ArceOS 编译中，
  tail 包含 `alloc`、`panic_abort`、`unwind`、`std_detect` 与 `hashbrown`。
- **性能观察与下一步**：构建前段常只有约 2--4 个 vCPU 活跃，后段曾达到 8 个 vCPU 各约
  86%--94%，说明宿主优先级修复有效，但整体吞吐仍远低于 Linux baseline。后续不再用完整
  BuildStorm 定位单点问题；按三层级路线先用 `perf_counters` 和分钟级
  `buildstorm_file_probe`/专项小测量化 ext4 lock、block I/O、PageCache、scheduler idle 与 fault，
  每次只修改一个最高热点，短门禁通过后才重跑无 feature 完整配置。

## 2026-08-09 BuildStorm 三层级优化路线与物理页泄漏修复（进行中）

- **统一路线**：`buildstorm-smp-plan.md` 已把历史 SMP Phase 0--5 作为正确性前置，并将性能工作统一为
  三层：低风险热路径/资源闭环、共享瓶颈/有效多核扩展、深层 I/O/MM/双架构扩展。第一、二轮现有
  优化均归入第一层；在并行 rustc SIGSEGV 闭环前不叠加 ext4 拆锁、per-CPU runqueue、private mmap
  共享或异步 VirtIO 重构。可复现 CPU/jobs 双矩阵与判读规则见 `workflows.md`。
- **独立故障 trace**：新增 kernel feature `fault_trace`，只在 RV64 用户页故障无法由
  `MemorySet::handle_page_fault()` 处理、即将发送 SIGSEGV/SIGBUS 时打印 hart/tid/tgid、cause、
  sepc/stval/sp/ra 和 errno；它不打开历史 `debug_traces` 的高频输出。正式计时仍要求无诊断 feature。
- **短回放证据**：旧 pub 镜像、`-snapshot` 下，单个历史失败 `riscv` rustc 精确命令成功；`riscv`
  与 `rgb` 两路并发均成功；七个历史失败 crate 的不同输出文件并发回放中六个成功，`rust_decimal`
  只因脱离 Cargo 后缺少 `OUT_DIR` 返回普通编译错误，全程无 `fault_trace`。另有 8 路并发读取约
  300 MiB `librustc_driver.so` 的 SHA-256 一致。以上排除固定 crate 输入、普通并发读和单次七路并发
  的确定性崩溃，但不能排除完整构建累计后的 PageCache/MM/资源状态。
- **完整 fault-only 证据**：RV64 8 GiB/8 核旧 pub 镜像的正式阶段仍失败，首次不可处理的 instruction
  与 store fault 均返回 `ENOMEM`；Cargo 结束后 guest 只剩 4 个基础任务，但 `/proc/respos_health`
  仍为约 `free_kb=60004 cached_kb=52464 heap_kb=187807`，十秒后不恢复。约 7.7 GiB 物理页既不属于
  PageCache，也不属于 kernel heap，确认是退出后的用户 frame 泄漏。宿主 QEMU RSS+swap 约 9.5 GiB，
  swap 压力解释了本轮极慢，但不是 guest frame 未回收的根因。
- **根因与修复**：`exit_process_group()` 原用 `MemorySet` 的原始 `Arc::strong_count` 与当前
  `thread_group` 成员数判断地址空间是否仅由本组拥有。同组 worker 已从 thread group 移除、但 TCB
  仍在退出 handoff/延迟 drop 中时会抬高引用数，导致 leader 跳过 `recycle_data_pages()`，随后 zombie
  leader 长期固定全部 resident frame。现在与 fd-table 归属规则一致：只把 `TASK_MANAGER` 中仍存活、
  不同 tgid 且共享同一 `MemorySet` 的任务视为外部 `CLONE_VM` owner。
- **短周期 A/B**：新增 RV64 `frame_reclaim_probe`，每轮实际触碰 64 MiB，并让 7 个 worker 与 leader
  近同时退出。在 1 GiB/8 核、`-snapshot` 中，旧判定 6 轮使 `free_kb` 从 `774156` 降至 `375648`
  （约 389 MiB，符合 6×64 MiB 线性泄漏）；修复版 10 轮从 `773996` 到 `768672`，640 MiB 累计压力后
  仅约 5.2 MiB 初始化/缓存抖动，`tasks=4 deferred=0`。这构成同一探针、同一资源配置的因果回归证据。
- **修复后门禁**：`cargo fmt --check`（os/user）、`git diff --check` 和无 feature
  `make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=` 均通过；该正式内核在 RV64 1 GiB/8 核
  `-snapshot` 下依次打印 `FRAME_RECLAIM_PROBE_EXIT resident_mb=64 threads=7`、
  `SMP_SHARED_MM_PROBE_PASS rounds=100` 与 `BUILDSTORM_FILE_PROBE_PASS`。最终退出路径把 MemorySet/FdTable
  的外部 owner 判断合并为一次 live-task snapshot；重构后再次运行 10 轮 frame probe，`free_kb`
  从 `774208` 到 `768912`，仍只有约 5.17 MiB 非线性抖动，`tasks=4 deferred=0`。
- **六路 rustc 回放边界**：无 feature 修复内核另以 RV64 4 GiB/8 核并发回放失败日志中的
  `riscv/rgb/radium/rdif_serial/funty/rdrive` 六条完整 release rustc 命令。QEMU 持续使用约 4--6 核、
  RSS 在约 1.4--1.9 GiB 间且无串口 fault，但受当前宿主 `nice=16/SCHED_IDLE` 放大，约 32 分钟仍未全部
  返回，已终止 snapshot。该轮没有最终退出码/`free_kb`，明确不计为通过；它说明六条完整 link 命令
  不适合作为本环境的日常短门禁，不能推翻专用 probe 的同配置 A/B 因果证据。
- **双架构构建**：同一修复已通过 LA64 无 feature
  `make build-la LA_USER_FEATURES= LA_KERNEL_FEATURES=`，新增 RV64 probe 在 LA64 构建中明确跳过运行。
- **下一执行顺序**：该退出回收缺陷按专用 A/B、三项 RV64 门禁和双架构 release 构建视为闭环；下一步
  才重跑完整 BuildStorm，并进入固定 8-vCPU jobs=1/2/4/8 与 CPU=jobs 1/2/4/8 缩放矩阵。完整运行用于
  集成验收，不再承担单点根因定位。

## 2026-08-09 BuildStorm 第二轮：扩大 heap、PageCache 工作集与顺序预读（未提交，完整运行失败）

- **基线/范围**：提交 `7cb282a` 加当前 PageCache/共享 mmap 重构工作树。RV64/LA64 kernel heap
  从 128 MiB 扩大到 256 MiB；frame-backed PageCache 全局上限从 512 页（2 MiB）提高到 16384 页
  （64 MiB）；缓存 read miss 最多预读 16 页（64 KiB）。未拆分 `EXT4_OP_LOCK`，未引入异步或多队列
  VirtIO。
- **预读语义**：顺序 run 在 PageCache 锁外通过一次 inode/lwext4 read 填充，再逐页复制到
  `FrameTracker`。写路径的 read-modify-write 只加载目标页；插入前检查 `size_version`，若期间发生
  truncate/extend 则丢弃旧快照并重试，避免把截断前数据重新放回缓存。
- **专项计数**：RV64 1 GiB/8 核、`perf_counters`、`-snapshot` 的同一
  `buildstorm_file_probe` 继续通过。相对统一页帧但尚无本轮优化的 probe，block read requests
  `2494 -> 12`、block write requests `17522 -> 1247`、ext4 lock acquisitions `3568 -> 503`、
  累计 heap allocation `124455223 -> 37737157` 字节、heap peak `6971508 -> 6630659` 字节；
  `shared_file_page_entries=0`，最终无脏页。该 probe 的收益不能直接外推为完整 BuildStorm 成绩。
- **门禁**：最终无 feature RV64/LA64 release 构建通过；RV64 1 GiB/8 核无 feature 专项 probe
  打印 `BUILDSTORM_FILE_PROBE_PASS`。RV64 8 GiB/8 核、旧 pub 镜像、无 feature 运行到
  `BUILDSTORM_TOOLCHAIN ok` 与 `BUILDSTORM_MINIBUILD ok` 后按计划主动停止，日志为
  `/tmp/respos-buildstorm-round2-minibuild.log`。256 MiB BSS 的 RV64 PT_LOAD `MemSize=268968108`，
  已在 1 GiB 与 8 GiB guest 正常启动。
- **完整运行结果**：同一无 feature kernel 以 RV64 8 GiB/8 核、旧 pub 镜像、`-snapshot` 完整执行
  `/glibc/buildstorm_testcode.sh`，日志为 `/tmp/respos-buildstorm-round2-full.log`。工具链和 minibuild
  均通过，正式阶段最终打印
  `BUILDSTORM_COMPILE mode=multi ok=false rc=1 elapsed_s=0.00 cores=8 bytes=0 arch=riscv64`；宿主侧
  `time` 为 `real 8067.83`、`user 23789.22`、`sys 17788.38` 秒。运行越过先前 4515.91 秒的堆耗尽点，
  全程未见 heap allocation failure、kernel panic 或 OOM，但 `riscv`、`rgb`、`radium`、
  `rdif-serial`、`funty`、`rust_decimal`、`rdrive` 七个不同 crate 的并行 `rustc` 均收到 SIGSEGV。
- **采样与下一阻塞**：约每五分钟采样时 QEMU RSS 主要在 2.5--3.2 GiB 间波动，并非单调增长；后半程
  宿主 swap 接近耗尽，瞬时 QEMU CPU 一度降至约单核。该环境压力能解释运行极慢，不能单独解释多个
  guest `rustc` 的 SIGSEGV。当前应把高并发用户映射/PageCache 一致性或回收问题列为 `待验证`，先用
  可缩短复现时间的并发 rustc/文件映射专项测试定位；本轮不能宣称通过或达到 `<3000s`。公告要求的
  新镜像与 RV64 16 GiB 正式资源仍待取得。

## 2026-08-09 PageCache/共享 mmap 统一页帧（未提交，BuildStorm 仍受 buddy 堆膨胀阻塞）

- **基线/范围**：提交 `7cb282a` 加当前工作树。`PageCache::Page` 不再为每页持有
  `Vec<u8>`，而是持有 `Arc<FrameTracker>`；普通文件 `MAP_SHARED` 直接复用该帧，不再另分配 mmap
  帧或向 `SHARED_FILE_PAGES` 插入常驻弱引用键。无 PageCache 的特殊文件仍使用原弱表兼容路径，
  插入时会清除失效项。
- **回收与一致性**：PageCache 全局 LRU 在 cache drop 时删除残留条目；受 mmap pin 的页采用有界
  轮询并重新排队，避免忙循环且能在映射释放后的下一轮压力中回收。truncate 会先清零仍可能被映射
  持有的完整 victim frame；普通缓存文件 read/write 不再进入旧的全局 overlay/update 锁路径。
- **专项验证**：`make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=` 与对应 `build-la` 均通过；RV64
  `-m 1G -smp 8 -snapshot` 的 `buildstorm_file_probe` 通过，覆盖 mmap 后扩容、truncate/regrow、
  mmap+pwrite 同页一致性和稀疏洞。带 `perf_counters` 的同一 probe 显示
  `shared_file_page_entries=0`、`heap_peak_bytes=6971508`；重构前同类 probe 峰值约 8.82 MiB。
- **完整 BuildStorm 结果**：统一页帧版本（随后只把正常缓存文件移出冗余 fallback 全局锁，最终工作树
  未重跑完整用例）以无 feature RV64、8 GiB/8 核、旧 pub 镜像、`-snapshot` 执行
  `/glibc/buildstorm_testcode.sh`，日志 `/tmp/respos-buildstorm-shared-pagecache.log`。已通过
  `BUILDSTORM_TOOLCHAIN ok` 和 `BUILDSTORM_MINIBUILD ok`，但在宿主 `real 4515.91s`、标准库构建阶段
  申请 60000 字节失败，未产生最终 `BUILDSTORM_COMPILE`。失败时
  `user_bytes=98093955`、`actual_bytes=134159544`、`total_bytes=134217728`。
- **结论/下一阻塞**：该版本越过原实现约 2202 秒以及仅完成 PageCache 帧化版本约 4269 秒的 OOM
  窗口，并把失败时仍存活的用户请求量降到约 93.5 MiB；普通文件共享页的重复物理帧和弱表增长已被
  消除。当前 buddy allocator 将每次分配上取整到 2 的幂，约 34.4 MiB 消耗在请求量与实际占用差额，
  因此 128 MiB 堆仍被内部膨胀耗尽。尚未达到 `<3000s`，下一轮应评估低内部碎片堆分配器或分离
  大块临时缓冲，而不是继续扩大 PageCache 数据结构。

## 2026-08-09 BuildStorm 第一轮性能优化与可选计数器（已提交 `7cb282a`）

- **基线/范围**：提交 `7cb282a`（父提交 `999bd8e`）；只调整普通 close 写回边界、lwext4 到
  VirtIO 的连续块提交、scheduler handoff IPI 以及可选诊断计数器，未拆分 `EXT4_OP_LOCK`，也未
  改变显式 `fsync`、`sync` 或正常卸载的持久化屏障。
- **close 语义**：`File::drop()` 不再把每次普通 close 提升为 `fsync()` 和
  `ext4_cache_flush("/")`。为兼容当前 weak inode cache 生命周期，最后 open-file description
  消失时仍将该文件的脏 PageCache 写入 lwext4；只有显式同步和 shutdown 才继续执行全文件系统/
  设备 flush。后续 inode/page-cache 生命周期重构完成前，不能直接移除 close 时的数据写回。
- **连续块 I/O**：`Disk` 对齐的连续读写现在一次提交完整 multi-block buffer，头尾非对齐部分仍走
  原有 read-modify-write。RV64 8 核 `buildstorm_file_probe` 通过；该轮计数为约 11.7 MiB block
  write / 930 requests（平均约 12 KiB/request），`filesystem_flushes=0`，并打印
  `BUILDSTORM_FILE_PROBE_PASS`。
- **调度 IPI**：handoff 任务先在 scheduler 锁下入队但不 kick，待旧 CPU owner 释放后只发送一次
  IPI，保留原 owner-release 竞态保护并消除原先 add-task 与 handoff 的重复 kick。
- **诊断接口**：kernel feature `perf_counters` 启用原子计数；`/proc/respos_perf` 汇总 close/fsync、
  block I/O、page cache、fault、TLB/RFENCE/IPI、task/idle ticks、ext4 lock 和 heap allocator 数据，
  写入 `reset` 可建立新的测量窗口。未启用 feature 时接口只返回 `enabled=0`，热路径更新编译为空操作。
- **串口 trace 边界**：历史 `proctrace`、`quiescetrace`、`pipelifetrace`、RV64 `ldtrace`，以及原有
  futex/timer lifecycle/LA pthread 调试输出统一要求 kernel feature `debug_traces`。默认和仅
  `perf_counters` 的正式/计数内核不会编译这些格式化与 timer 采样路径；panic、错误和启动信息保留。
- **减法重构**：在保持上述行为的前提下，删除 `Disk` 中已被连续块路径覆盖的整块单请求分支和两个
  全仓库无调用的 offset I/O 接口，仅保留 batched span 与 partial head/tail 两条路径；close 与 fsync
  复用缓存数据同步函数；性能计数器移除中间 `Counter` 枚举/匹配层，高频串口调用统一走
  `debug_trace!`。`perf.rs` 由 377 行降为 308 行，`disk.rs` 由 216 行降为 176 行。
- **验证**：带 `perf_counters` 的 RV64/LA64 release、无 eval user feature 均构建通过；RV64
  `-m 1G -smp 8 -snapshot` 验证 proc reset/read、创建文件后 close/reopen 内容一致及
  `buildstorm_file_probe`。新增 trace 门控后，无 feature RV64/LA64、RV64
  `perf_counters debug_traces` 组合和 LA64 `debug_traces` 构建通过；RV64 8 核无 feature 烟测中四类
  trace 均消失且 file probe 继续通过，启用 `debug_traces` 后 clone/exec/wait/exit trace 恢复。
  减法重构后重新验证 RV64/LA64 无 feature 及两 feature 同开 release 构建；RV64 1 GiB/8 核
  `-snapshot` 下无 feature 返回 `enabled=0` 且 probe 通过，仅 `perf_counters` 时返回 `enabled=1`、
  `filesystem_flushes=0`，约 10.3 MiB block write 合并为 701 次请求，probe 继续通过。
  后续 8 GiB 完整 BuildStorm 的失败结果与 `<3000s` 状态见上一节。

## 2026-08-08 决赛镜像/计时口径更新（官方群公告，待新镜像验证）

- 比赛官方群同步：CAgent 已修正 waitpid 测例问题并补回缺失的 `ss`；决赛 QEMU 参数调整为
  RV64 `-m 16G -smp 8`、LA64 `-m 36G -smp 12`，整轮超时 6250 秒。评测平台为 128 GiB、
  40 线程 VMware Guest。2026-08-09 核对官方 `testsuits-for-oskernel` 的 `final-2026` 分支提交
  `3c80dc1` / PR #60 后，计分窗口 Linux baseline 已修正为 RV64 `1616.09s`、LA64 `1985.21s`；
  原 `4655.23s` / `6223.0s` 与正式 timed 窗口不一致，不能继续用于成绩比较。
- 当前两个旧镜像缺少预编译 `tg-xtask`。官方计划在更新镜像中补回它；前置依赖构建不计入内核
  编译成绩，计时基线仅覆盖测试用例自身编译，也不包含编译后的运行验证。
- PR #60 的 Linux 原始日志显示 RV64/LA64 untimed `tg-xtask` 预构建分别为 58 分 27 秒和
  64 分 45 秒，而计分的 `cargo xtask arceos build ...` 分别为 1616.09 秒和 1985.21 秒。这是计时
  口径纠正，不代表任意本地环境排除 `tg-xtask` 都会固定节省约 3000 秒；缓存和宿主性能会显著改变
  untimed 阶段。
- **对当前结论的影响**：旧 pub 镜像上的 `tg-xtask` 自举 SIGSEGV 当时是真实兼容性缺陷，现已按
  下一节所述修复；但旧镜像结果仍不能直接证明新计分区间通过，此前 8 GiB 运行也只能作为历史功能
  证据，不能作为新资源配置下的最终结果。
- **待验证**：本地尚未取得公告所述新镜像。拿到后需记录镜像 hash、核对脚本实际 marker/计时边界，
  并分别按 RV64 16 GiB/8 核和 LA64 36 GiB/12 核重跑；公告调整期的平台成绩波动不能作为内核回归。

## 2026-08-08 tg-xtask 启动与链接产物损坏已修复（未提交，新镜像仍待验证）

- **基线/配置**：`dev` HEAD `0881216` 加当前未提交工作树。由于宿主仅约 15 GiB 内存，本地功能验证
  使用 RV64 `-m 8G -smp 8`；这不是公告中的 16 GiB 正式资源配置，也不能作为最终成绩。
- **tg-xtask SIGSEGV 根因**：原 512 KiB 用户栈被大程序启动数据耗尽，向下越过 guard 后落入动态
  解释器的只读可执行 VMA，表现为在解释器地址附近发生 store page fault。RV64/LA64 用户栈扩大为
  8 MiB 并保持 lazy VMA，仅为 argv/envp/auxv 实际写入页建页；旧镜像中的
  `target/debug/tg-xtask --help` 已完整退出成功。
- **装载与内存门槛**：filesystem 动态解释器的 PT_LOAD 与主 ELF 一样改为按页 file-backed fault，
  仅预读并校验最多 1 MiB ELF 元数据；嵌入式 fallback 保持 eager。并发工具链仍会耗尽 64 MiB
  kernel heap，RV64/LA64 均调整为 128 MiB；64 MiB 失败时已记录到精确 393216-byte 分配及后续小分配。
- **损坏 ELF 的最终根因**：lwext4 `ext4_fread()` 把未分配的完整块和尾部洞的 `fblock == 0` 当作
  物理块 0 读取。新建稀疏 lld 输出在用户写入前便出现稳定的磁盘块 0 字节，导致 ELF
  `.symtab[0]` 非零、`llvm-objcopy` 拒绝产物。完整洞块和尾部洞现显式填零；Rust ext4 `read_at()`
  也先清零 buffer 作为防御。修复后专项 probe 在 RV64 `-m 1G -smp 8 -snapshot` 打印
  `BUILDSTORM_FILE_PROBE_PASS`。
- **共享映射一致性**：普通文件以 backend inode 作为稳定身份共享 resident file pages；pwrite/write
  会更新已驻留的共享页，read 会叠加共享页内容，truncate shrink 会清零新 EOF 之后的驻留字节。
  probe 同时覆盖同页 mmap+pwrite、truncate/regrow 零洞以及新建约 1.7 MiB 稀疏文件立即
  `MAP_SHARED` 读取。
- **工具链证据**：在最终修复内核上直接执行与 xtask 相同的 release cargo 构建，7 分 15 秒完成；
  新 ELF 的 `.symtab[0]` 为合法的全零 undefined entry，`llvm-objcopy` 返回 0，生成 763000-byte
  binary（ELF 1696472 bytes）。随后旧镜像完整执行
  `cargo xtask arceos build -p arceos-helloworld --arch riscv64`，全量 release cargo 阶段用时
  6584.09 秒，wrapper 的 `llvm-objcopy --strip-all -O binary` 和顶层命令均返回 0。该结果证明旧镜像
  全调用链兼容，但因包含失效缓存后的前置重编译、使用本地 8 GiB 配置且超过 6250 秒，不能作为新
  计时口径下的正式成绩。
- **门禁状态**：最终修复后的 `make build-rv RV_USER_FEATURES=`、`make build-la LA_USER_FEATURES=`、
  os/user `cargo fmt --check` 与 `git diff --check` 均通过。补回预编译 `tg-xtask` 的新官方镜像仍为
  `待验证`。

## 2026-08-08 BuildStorm minibuild 已通过，最终 tg-xtask 执行仍失败（未提交）

- **基线/配置**：`dev` HEAD `0881216` 加当前未提交工作树；执行
  `make build-rv RV_USER_FEATURES=`，随后按 `docs/codex/workflows.md` 的 8 GiB、8 核、pub 镜像、
  `-snapshot` 命令启动，guest 运行 `/glibc/buildstorm_testcode.sh`。
- **本轮确认并修复**：新建普通文件现在以真实 ext4 inode 为键复用 synthetic inode 的
  `PageCache`；lwext4 `ext4_generic_open2()` 的失败清理不再重复释放旧 inode ref；
  `ext4_ftruncate()` 扩容会按稀疏文件语义更新 inode/open-file size；离散脏页写回在 seek 前先扩展
  lower file；可写 `MAP_SHARED` 文件映射保留完整映射作为写回窗口，最终仍按当前 EOF 裁剪。
- **专项回归**：新增 `buildstorm_file_probe`，同时覆盖“短文件 mmap 后扩容并写入尾页”和
  “4 MiB 级稀疏 pwrite + hardlink + unlink-open-source + close/reopen”两条路径。RV64 release、
  `-m 8G -smp 8 -snapshot` 打印 `BUILDSTORM_FILE_PROBE_PASS`。
- **官方结果**：同一内核依次打印 `BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok`；此前
  minibuild ELF 被截为 `0x395d30`、而节表要求到 `0x40cd30` 的问题已越过。最终预构建 cargo
  在 5 分 01 秒完成，timed cargo 在 5 分 54 秒生成并执行
  `/work/tgoskits/target/debug/tg-xtask`，但该程序以 SIGSEGV 退出，最终标记为
  `BUILDSTORM_COMPILE mode=multi ok=false rc=139 elapsed_s=0.00 cores=8 bytes=0 arch=riscv64`。
- **失败边界**：失败现场中 `tg-xtask` 实际大小为 `277827240`，ELF 节表起点为
  `277824552`、42 个 64-byte section header，`readelf -S` 可正常解析；因此这一次不能再归因于
  文件尾截断。下一步应对未处理的 RV64 user page fault 临时记录 `sepc/stval/cause/tid/tgid`，
  复现 tg-xtask 启动 SIGSEGV 后再判断是大 ELF 装载、动态链接器还是多线程启动路径。
- **当前门禁**：`make build-rv RV_USER_FEATURES=` 通过（仅既有 target-feature warning），
  `make build-la LA_USER_FEATURES=`、os/user `cargo fmt --check` 与 `git diff --check` 通过；完整题二
  仍未通过，不能将 minibuild 成功等同于最终成绩。

## 2026-08-08 BuildStorm 已越过 ld/cargo 管道阻塞（未提交，minibuild 执行仍失败）

- **基线/配置**：`dev` HEAD `b785262`；`make build-rv RV_USER_FEATURES=`，随后以
  `qemu-system-riscv64 -machine virt -kernel kernel-rv -m 8G -nographic -smp 8 -bios default
  -drive file=img/sdcard-rv-pub.img,if=none,format=raw,id=x0
  -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -snapshot` 启动，guest 执行
  `/glibc/buildstorm_testcode.sh`。原有两份未跟踪文档未修改。
- **本轮已确认修复**：futex compare/requeue 在持队列锁前预取用户值，锁内使用 nofault 读取；
  `CLONE_VM` task 共享内层地址空间但各自持有可替换 handle，exec 不再覆盖 vfork/clone parent 的
  `MemorySet`；用户非法指令改为投递 `SIGILL`；所有 lwext4 入口（含 superblock sync/statfs/shutdown）
  统一串行化；UNIX socket 端点共享 close 状态，peer close 后 read 在排空队列后返回 EOF、write 返回
  `EPIPE`，poll readiness 与之保持一致。
- **运行证据**：官方脚本稳定打印 `BUILDSTORM_TOOLCHAIN ok`；`ld.bfd` 已能完成，collect2/gcc/rustc/
  cargo 均退出，说明先前 lwext4 并发断言循环和 UNIX socket `recvfrom` 永久等待均已越过。随后生成
  `/tmp/minibuild/target/debug/minibuild`，但首次执行在 `sepc=0x4293f0`、`stval=0x9` 发生 load fault，
  脚本打印 `BUILDSTORM_MINIBUILD fail`，因此不能宣称题目通过。
- **文件一致性证据**：针对 linker tgid 83 的临时 syscall trace（已移除）确认输出 fd 4 在
  `0x40c370` 连续写入 `0x800 + 0x1c0 = 0x9c0` 字节，恰为 ELF 的 39 个 64-byte section headers，
  所有 write 均返回完整长度且最终 close 成功；但重新打开时该区间为零。根因已缩小到“新建/重命名
  普通文件的 synthetic inode 与后续 lookup 所见页缓存不一致，或最终写回生命周期”一层，仍标记
  **待验证**。强制 close 同步和直接改用真实 inode 都造成 linker 长时间同步/写入，已回退，不属于
  当前工作树。
- **当前门禁**：`cargo fmt --manifest-path os/Cargo.toml -- --check` 与 `git diff --check` 通过；
  `make build-rv RV_USER_FEATURES=` 通过（仅既有 target-feature warning）。最后一次无高频 syscall trace
  的真实-inode实验在 `ld.bfd` 阶段超过 3 分钟无输出后主动终止并回退，不能计为验证结果。当前无
  QEMU 残留；下一步应设计“真实 inode 号到共享 PageCache”的独立映射或专项 create/rename/reopen
  probe，避免在 close 路径同步整份文件。

## 2026-08-07 补充：quiescence 协议与诊断 trace（未提交，minibuild 仍待下一轮运行）

- **基线/配置**：`dev` HEAD `17dcd4e`，基于上一节 pipe/wait/ARG_MAX 修复后的工作树。
  原有两份未跟踪文档未修改。
- **quiescence 协议（stop/ack）**：`close_other_threads_for_exec()` 和 `exit_process_group()`
  现采用四步协作式终止协议，消除"远端 sibling 仍在旧 `MemorySet` 上执行时主线程已开始
  释放共享资源"的窗口。
  - 新增 `TaskControlBlock::terminate_requested: AtomicBool` 字段，`request_termination()`
    同时设该标记和 `set_exited()`。
  - `can_be_claimed_on_cpu()` 与 `try_claim_running_on_cpu()` 拒绝已标记终止的 task。
  - `publish_saved_handoff()` 对 `terminate_requested` 的 task 不再重新发布到 ready queue，
    改为静默丢弃——远端 CPU 切回 idle 后该 task 自然消失。
  - 四步流程：① `request_termination()` + `remove_task()` 标记并摘除所有 sibling；
    ② spin-wait `has_cpu_owner()` 等待各远端 CPU 在 `__switch` 后释放 owner；
    ③ 确认全部 ack 后打印 `[quiescetrace] exec remote-ack` / `group-exit remote-ack`；
    ④ 执行 `cleanup_exiting_thread()` 等回收操作。
  - 机制不依赖 IPI——远端 CPU 在下一次 timer preempt 进入调度路径时因 `terminate_requested`
    而无法被 claim，随后切入 idle 并释放 owner。最坏等待不超过一个调度 quantum。
- **诊断 trace 基础设施**：新增五条受控 trace channel，均使用可 grep 的固定前缀：
  - `[quiescetrace]`：wait-block/resume、child exit 通知、exec sibling quiescence、group-exit
    全流程，覆盖 `task.rs` 和 `syscall/process.rs`。
  - `[proctrace]`：clone（含 parent/child tgid 和 flags）与 exec（含 path 和 argv[0]），
    位于 `syscall/process.rs`。
  - `[pipelifetrace]`：pipe create（`make_pipe()`）、drop（`Pipe::drop()`，含 buffer 地址、
    读写端状态和 Arc 强引用计数）、poll-notify（events 和被唤醒 tids），位于 `fs/pipe.rs`
    和 `fs/poll.rs`。
  - `[ldtrace]`：timer 中断时对 `tgid==20` 采样 `sepc`，每 hart 每秒至多一次（`AtomicUsize`
    CAS 限速），位于 `arch/rv64/trap/mod.rs`。用于定位 BuildStorm"静默阻塞"时 cargo
    进程停在什么内核路径。
  - `[illegaltrace]`：IllegalInstruction trap 现在打印 hart/tid/tgid/sepc/指令字/stval，
    替代原来的纯英文描述，位于同文件。
  - 所有 trace 均为临时诊断用途，后续在 BuildStorm 通过后应移除。
- **当前门禁**：`cargo fmt --manifest-path {os,user}/Cargo.toml -- --check`、`git diff --check`、
  `make build-rv RV_USER_FEATURES=`、`make build-la LA_USER_FEATURES=` 均通过。
- **仍待完成**：上述 quiescence 协议 + trace 尚未在实际 BuildStorm 运行中验证；下一轮应在
  verbose cargo 运行同时收集 quiescetrace/ldtrace/pipelifetrace，确认"静默边界"是否由远端
  sibling 未正确停止、pipe 引用泄漏或其他独立原因导致。

## 2026-08-07 BuildStorm 越过 pipe/wait/ARG_MAX（未提交，minibuild 仍待完成）

- **基线/配置**：`dev` HEAD `17dcd4e`；`make build-rv RV_USER_FEATURES=` 的 release kernel，
  `img/sdcard-rv-pub.img` 以 `-snapshot -smp 8 -m 8G` 启动，guest 执行
  `/glibc/buildstorm_testcode.sh`。原有两份未跟踪文档未修改。
- **pipe/exec 根因**：受控 trace 确认 rustup/cargo shim 在多线程状态 exec；被
  `close_other_threads_for_exec` 摘除的 blocked TCB 通过遗留 kernel stack 持有旧
  `FdTable`，使 stdout/stderr pipe 写端不到 EOF。exec 现在 clone 新表后，仅在
  `TASK_MANAGER` 中无其他 live sharer 时显式清空旧表。修复后日志
  `/tmp/respos-buildstorm-waiter-wake-trace.log` 中对应 pipe 强引用由异常的 3 恢复为 2，
  cargo parent 可继续运行。
- **thread-group wait**：子进程 69 退出时，实际 `wait4` 等待者是同组 tid 68，
  旧路径只唤醒 parent leader tid 45。TCB 现显式标记 child waiter，退出事件唤醒
  该线程组的真正 waiter；同一日志确认 tid 68 连续回收 child 69/70，不再永久睡眠。
- **exec 顺序**：旧代码先替换共享 `MemorySet`，再执行旧线程 robust-list /
  `clear_child_tid` 清理，会使旧用户地址写入新程序映像。现改为在旧地址空间仍安装时
  先清理 sibling threads，再替换映像。远端 running thread 的完全协作式终止仍是后续边界。
- **实际 RAM**：RV64 原固定 `MEMORY_END=0x90000000`，QEMU `-m 8G` 时仍只管理约
  256 MiB；`/tmp/respos-buildstorm-user-fault-trace.log` 确认 cargo 指令缺页和 shell 读缺页均
  最终返回 `ENOMEM`。现在 boot 大页覆盖最多 8 GiB QEMU RAM，从 OpenSBI FDT 取实际上限，
  frame allocator/procfs/sysinfo 使用动态值，首 GiB 之后用 Sv39 1 GiB direct-map leaf。
  8 GiB guest 实测 `MemTotal: 8386560 kB`、`MemFree: 8312232 kB`；同一 trace-free RV
  release 以 `-m 256M -smp 1` 回归，`MemTotal: 260096 kB`、shell `quit` 使 QEMU exit 0，
  日志 `/tmp/respos-dynamic-memory-256m.log`。
- **ARG_MAX**：官方 minibuild 原先稳定失败；保留 stderr 的临时镜像日志
  `/tmp/respos-minibuild-pipe-output.log` 给出 rustc `never executed` / `Argument list too long (os error 7)`。
  根因是 argv 和 envp 各自被限制为 32 项。现改为每组 4096 项且每组字符串总量
  1 MiB，防止无界 kernel allocation；修复后 rustc 已真正进入 `Compiling minibuild`。
- **RV64 trap 恢复根因**：对上述长时间运行附加 GDB 后，连续快照
  `/tmp/respos-rustc-pc-sample{1,2,3,4,5}.txt` 发现 CPU0 重复停在
  `__trap_from_user`，其中 `sepc=__trap_from_user+8`、`scause=15`，对应
  `sd sp, 8(t0)` 的递归 StorePageFault。旧 `__restore` 过早把 `stvec` 切到 user trap
  入口，又原样恢复可能带 `SIE=1` 的 `sstatus`；timer 可在 `sret` 前、`sscratch`/寄存器
  仍处于过渡状态时重入。现在 `TrapContext::init_app_context()` 清 `SIE`，汇编在写
  `sstatus` 前再次统一掩码，并把 user `stvec` 切换延后到最终返回窗口。
- **修复后证据**：RV64 release 重新构建后，`-m 256M -smp 8` 的 `nproc`、`/bin/true`、
  `quit` smoke 通过。新的 8 GiB/8 核 BuildStorm 已越过 `BUILDSTORM_TOOLCHAIN ok`；运行中
  快照 `/tmp/respos-rustc-pc-postfix-sample{1,2,3,4,5}.txt` 显示早期工作核在 ext4、用户缺页、
  `mprotect` 与调度路径推进，且所有快照均不再出现上述递归特征。
- **新的静默边界**：同一官方运行到约 15 分 51 秒时，较晚两次快照显示所有 guest CPU
  均回到 scheduler idle/SBI timer 路径，但脚本仍未打印 `BUILDSTORM_MINIBUILD ok|fail`；
  这说明 trap 修复后仍存在独立的 sleep/wakeup 或资源生命周期阻塞，不能解释为 rustc
  单纯执行缓慢。该 QEMU 已主动终止；后续 verbose 复现已确认 `cargo new` 成功并进入
  `cargo build -vv`，但因本轮收口未继续等待到新的阻塞点。
- **环境与门禁**：当前执行环境把 QEMU 置于 Linux `SCHED_IDLE`（且无权提升），所以墙钟
  耗时不能作为 RespOS 性能结论。修复后
  `cargo fmt --manifest-path {os,user}/Cargo.toml -- --check`、`git diff --check`、
  `make build-rv RV_USER_FEATURES=`、`make build-la LA_USER_FEATURES=` 均通过；当前无 QEMU
  残留。BuildStorm minibuild/full compile 仍未通过，下一轮应在 verbose cargo 运行同时
  读取任务/等待关系，并继续审计远端 exec thread teardown 的 stop/ack 协议。

## 2026-08-06 BuildStorm minibuild 续查（未提交，blocker 仍存在）

- **实际基线**：队友的多核推进已提交为 `17dcd4e`（`fix: 推进多核工作`），不再是下节所写的
  `dc793c4 + 未提交工作树`。本轮保留的新增改动位于 wait4、task manager、per-CPU idle 回收和
  process-group fd-table 归属判断；两份原有未跟踪文档未修改。
- **复现**：RV64 release、无 `eval` user feature、8 核/8 GiB、pub 镜像 `-snapshot`，运行
  `/glibc/buildstorm_testcode.sh`。`/tmp/respos-buildstorm-wait4-fix.log` 等多轮日志均稳定打印
  `BUILDSTORM_TOOLCHAIN ok`，但至少再等待 120 秒仍无 `BUILDSTORM_MINIBUILD` 标记；所有失败态均由
  QEMU monitor 终止，仓库镜像未写入。
- **已确认的两个独立窗口**：`wait4` 在扫描 child 后、发布 Blocked 前存在 child-exit lost wakeup，
  现于 blocked 发布后复查 `exited_children`；退出 TCB 原只在后续 task 恢复等少数路径清理，所有 CPU
  idle 时 `DEAD_TASKS` 可永久保留旧 fd table，现于 context 已切回 per-CPU idle 栈后清理。这两项均为
  代码证据明确的生命周期问题，但加入后 BuildStorm minibuild 仍未通过，不能把它们写成最终根因。
- **fd-table 归属**：退出路径原用 `Arc::strong_count <= 当前 thread_group 成员数` 判断能否清表；已退出
  但仍延迟回收的同组 TCB 会使该值漂移。当前改为从 `TASK_MANAGER` 的 live-task snapshot 判断是否有
  不同 tgid 共享同一张表，保留真正的跨进程 `CLONE_FILES`，忽略同组延迟引用。该边界尚待专项
  `CLONE_FILES + exit` probe 验证。
- **已撤回的实验**：曾尝试让 pipe EOF 由显式 descriptor 槽位计数和 live-task fd 扫描驱动；它未让
  minibuild 前进，并在普通 `close()` 路径触发 TCB 外层 fd-table 锁递归。GDB 证据为
  `/tmp/respos-buildstorm-fd-lifetime-bt.txt`（失败态其余 CPU 均在 idle；实验性扫描版本另见当轮终端
  记录）。该实验已完全移除，不属于当前工作树。
- **当前门禁**：保留改动后 `cargo fmt --manifest-path {os,user}/Cargo.toml -- --check`、
  `git diff --check`、`make build-rv RV_USER_FEATURES=`、`make build-la LA_USER_FEATURES=` 均通过。
  尚未完成专项 wait4/pipe/CLONE_FILES guest probe，也未得到 `BUILDSTORM_MINIBUILD ok`；下一步应对
  cargo parent 的 wait4/poll 状态和 rustfmt child 的退出通知做同一轮低量 trace，不能继续只以
  `Pipe::drop()` 是否出现判断阻塞点。

## 2026-08-06 RV64 8 核 BuildStorm 首轮推进（未提交，minibuild 仍待验证）

- **基线/命令**：`dev` HEAD 为 `dc793c4`，使用 release、无 `eval` user feature 的 `kernel-rv`，
  `img/sdcard-rv-pub.img` 只读 snapshot 启动：`qemu-system-riscv64 -machine virt -kernel kernel-rv
  -m 8G -nographic -smp 8 -bios default ... -snapshot`，guest 执行
  `/glibc/buildstorm_testcode.sh`。镜像内脚本与 final-2026 规则一致。
- **已越过的阻塞**：epoll 现在接受合法 `EPOLLRDHUP` interest；并发 `exit_group` 的非 owner
  不再以 Running 状态返回；并发 lazy fault 在拿到 MM 锁后若发现 PTE 已由另一核补齐，会本地
  `sfence` 并重试。以上修复后，8 核 BuildStorm 可稳定打印 `rustc 1.98.0-nightly`、
  `cargo 1.98.0-nightly` 和 `BUILDSTORM_TOOLCHAIN ok`。
- **大 ELF exec**：文件系统 ELF 不再通过 `File::read_all()` 把完整文件放入固定 64 MiB kernel
  heap。新路径只读取 ELF64 header、program headers 和 PT_INTERP 字符串；主程序 PT_LOAD 作为
  private file backing 记录，缺页时逐页读取。45,559,552 字节 cargo 已可执行；此前日志
  `/tmp/respos-buildstorm-rv8-latched-fault-fix.log` 的 ENOMEM 上限不再出现。
- **Rust 子进程 ABI**：实现 `ioctl(FIONBIO)` 对 open-file status 的 `O_NONBLOCK` 切换，修复
  Rust std 同时捕获 stdout/stderr 时 `read_output` 因 ENOTTY panic。exec 在应用 CLOEXEC 前显式
  解除可能的 `CLONE_FILES` 共享。lazy framed VMA 回收只遍历 `data_frames` 的 resident 页，
  不再按可能极大的虚拟跨度逐页查询 PTE。
- **SMP vfork 丢唤醒**：受控 trace 确认 cargo 启动 rustfmt 使用 `clone(0x4111)`。原 `sys_clone`
  先 `add_task(child)`、后阻塞父任务；8 核下子任务可先 exec 并发出一次性 vfork wake，父任务
  尚未进入 blocked 表导致 wake 丢失。当前改为先 `prepare_current_task_blocked()`，再发布 child，
  最后 `switch_to_next_task()`，关闭该窗口。该结论由 `/tmp/respos-buildstorm-cloneflags.log`、
  `/tmp/respos-buildstorm-exittrace.log` 支撑；临时 trace 已全部移除。
- **最新分层证据/边界**：`/tmp/respos-buildstorm-pipetrace.log` 在同一 8 核 release 配置中确认：
  vfork parent 已在 rustfmt child exec 后恢复，rustfmt（tid/tgid 13）随后完成 process-group exit，
  `fd_table_owned_by_group=true` 且 fd table 已执行 `clear()`；但对应 `Pipe::drop()` 未出现，
  `cargo new` 仍未返回。QEMU 随后由宿主终止，不能计为完整通过。当前 blocker 已缩小为
  child-side pipe open-file 引用/父进程输出收集生命周期，根因仍为 `待验证`；不能再归因于 vfork
  wake 丢失，也不能宣称 `BUILDSTORM_MINIBUILD` 或 `BUILDSTORM_COMPILE` 通过。临时 trace 已全部移除。
  所有测试均为 `-snapshot`，repo pub 镜像未写入。
- **收口审查门禁**：移除全部 `vforktrace`/`pipetrace` 后，文件式 ELF loader 又补充 1 MiB
  metadata 上限、ELF64 program-header size 与 PT_LOAD 文件末尾校验。随后
  `cargo fmt --manifest-path {os,user}/Cargo.toml -- --check`、`git diff --check`、
  `make build-rv RV_USER_FEATURES=`、`make build-la LA_USER_FEATURES=` 均通过。该防御性校验后未重跑
  BuildStorm runtime；已通过的 runtime 边界仍以前述日志为准。审查结束时无 QEMU 进程遗留。

## 2026-08-06 RV64 SMP Phase 3 并发回归与 Phase 4 active-mask（未提交）

- **基线与实现**：`dev` HEAD 仍为 `dc793c4`，所有改动未提交。新增
  `smp_phase3_probe`，每轮并发执行两组 wait4/fork/exec、pipe 和 TCP/UDP loopback，固定运行
  30 轮；loopback 端口按 probe 父 PID 分槽，避免并发测试自冲突。用户库补充
  `sched_{set,get}affinity` 封装。
- **Phase 3 结果**：RV64 debug、pub 镜像、`-snapshot -m 256M` 下，2/4/8 核各一次
  30/30 PASS，随后 `nproc` 分别为 2/4/8，guest `quit` 均使 QEMU exit 0。日志为
  `/tmp/respos-phase3-smp{2,4,8}.log`。active-mask 改动后 8 核同一 30 轮再次通过，日志
  `/tmp/respos-active-mask-phase3-smp8.log`。
- **active-mask / shootdown 协议**：`MemorySet` 的历史 residency 集合已改为真实
  `active_hart_mask`。恢复 task 前在 `MemorySet` 读锁内发布 bit；`__switch` 已恢复
  per-CPU idle/kernel `satp` 后才清除旧 bit；`exec` 和 clone 临时页表切换显式转移/撤销 bit。
  页表写入持写锁，先完成 PTE 写入和本地 `sfence.vma`，再对 active remote hart 调用 SBI
  RFENCE。当前 QEMU OpenSBI 1.5.1 的 `sbi_tlb_request → sbi_ipi_send_many` 使用同步计数等待
  远端处理完成，故在该目标平台上 RFENCE 返回构成 request/ack；此结论不外推到未知 firmware。
- **共享 MM 验证**：新增 `smp_shared_mm_probe`，把同一 `CLONE_VM` 地址空间的 writer/reader
  固定到两个 CPU；writer 在固定 VA 上反复 `munmap + MAP_FIXED mmap`，reader 在每次握手后
  从另一 CPU 读取新页。RV64 debug 8 核连续 10/10 PASS，共 1000 轮 remap；2 核再通过
  100 轮，随后 shell health 与 QEMU exit 0。日志为
  `/tmp/respos-active-mask-shared-mm-smp{2,8}.log`。
- **futex 结果与边界**：8 核默认 build 的 `task_a_futex_exit_probe`、
  `task_a_futex_race_probe` 各通过一次。`cmp_requeue` 默认 build 的结果无效，因为该 probe 已知
  依赖 `TASK_A_FUTEX_CMP_REQUEUE_TEST_YIELD=1`；专项 build 首次出现一次 waiter timeout/不收敛，
  随后相同 8 核配置连续 20/20 PASS，日志
  `/tmp/respos-active-mask-futex-cmp{-gdb,}-smp8.log`。该首个样本保留为 `待验证`，不能宣称
  cmp-requeue 压力已完全稳定。
- **构建/release 门禁**：默认 RV64/LA64 debug 与 release build、两个 fmt check、
  `git diff --check` 均通过；LA probe 明确 skip，因为 LA64 当前仍为单核路径。RV64 release
  `-snapshot -smp 8 -m 256M` 下 Phase 3 30/30、共享-MM 100 轮、`nproc=8` 和 QEMU exit 0
  均通过，日志 `/tmp/respos-release-active-mask-smp8.log`。
- **单核 CAgent 门禁**：把 pub 镜像复制到 `/tmp/respos-cagent-active.xOhODX.img`，仅在临时副本
  注入 `scripts/cagent_debug.sh`；RV64 debug `-snapshot -smp 1 -m 256M` 的 kernel 单项输出
  `testcase cagent kernel pass 7752`，随后 `true` exit 0、QEMU 正常退出，日志
  `/tmp/respos-active-mask-cagent-kernel-smp1.log`。CAgent 分数受 agent 输出影响，不能与历史
  8829 直接作性能比较；这里只作为当前 task/MM 改动未破坏单核执行链的门禁。
- **仍未完成**：BuildStorm 的最新进展见上一节；完整 CAgent、mprotect/COW 双线程压力或 LA64 SMP
  尚未运行，
  因此不能把本节等同于题二完成。

## 2026-08-06 RV64 SMP 退出风暴的两处中断重入死锁已修复（未提交）

- **基线与范围**：`dev` HEAD 仍为 `dc793c4`；本轮修复位于 `os/src/mm/heap_allocator.rs`、
  `os/src/arch/rv64/trap/mod.rs` 和 scheduler debug invariant，并包含下一节的 affinity 改动，均未提交。
- **根因 1（已用全 CPU GDB 栈确认）**：CPU 0 在
  `MapArea::unmap_one → BTreeMap::remove → __rust_dealloc → LockedHeap::dealloc` 持有全局 heap
  内部锁时被 kernel timer 中断；`check_nanosleep_timeouts()` 在中断中 `collect::<Vec<_>>()`
  再次分配，同 CPU 永久等待自己持有的 heap 锁。CPU 6 同时在另一退出路径等该锁。
  证据为 `/tmp/respos-smp8-gdb-bt1.txt`。当前 frame allocator 是
  `StackFrameAllocator + spin::Mutex`，不是 buddy；buddy 是内核 heap allocator。
- **根因 2（低扰动动态 GDB 确认）**：2 核失败态中，boot hart 在 syscall 内
  `TaskControlBlock::check_real_timer()` 持有 `ACTIVE_ITIMER_TASKS` 普通 `SpinLock` 时被 timer 中断；
  `kernel_trap_handler → check_active_itimers()` 重入同一把锁。另一 hart 已 WFI，因而整机无进展。
  QEMU 正常启动时只开 monitor，确认卡住后才动态启动 gdbserver，避免 `-gdb` 改变竞态；
  证据为 `/tmp/respos-smp2-dynamic-bt.txt`。
- **修复协议**：全局 heap 现由 `IrqSafeHeap` 包装，alloc/dealloc/init/stats 均先保存并关闭
  本地中断，再由 `LockedHeap` 处理跨 CPU 互斥，且先释放 heap 锁再恢复中断。RV64
  kernel-mode timer trap 只在 boot hart 的 per-CPU idle context（`current_task == None`）运行
  `check_all_task_timers()`；中断普通 syscall/exit/scheduler 临界区时只重编程下一 tick。从用户态直接
  进入的 timer trap 仍是无内核锁的 timer-work 安全点。
- **debug scheduler 检查**：不再分配 `BTreeSet`，也不再对 `task_index` 每一项反查
  140 个优先级队列。现在在一次线性队列遍历中验证 status、queue kind、index mapping 和
  ready/blocked 互斥；`add()` 保留重复 tid 断言，队列总数与 index 基数保持相等。
- **退出压力矩阵**：RV64 debug pub、`-snapshot -m 256M`，2/4/8 核各连续 3 轮并发运行
  4 路 `busybox timeout 3 busybox sleep 60` + `wait`。9 轮全部输出 `SMP_EXIT_STORM_DONE`，
  guest 压力段为 3655–5159 ms；每轮随后的 `/proc/respos_health` 均可读，guest `quit`
  使 QEMU exit 0，未见 panic。日志为 `/tmp/respos-safe-smp{2,4,8}-r{1,2,3}.log`。
- **其他门禁**：RV64/LA64 debug build、两个 `cargo fmt --check`、`git diff --check` 通过。
  RV64 `-snapshot -smp 1` 的 `true; sleep 1; true` 三项 exit 0；8 核 affinity 掩码
  `0x80..0x01` 的 8 个进程全部 exit 0，`nproc=8`。日志为 `/tmp/respos-irqsafe-smp1.log`、
  `/tmp/respos-irqsafe-affinity-smp8.log`。RV64 release 首次构建命中已知 lwext4 共享 CMake 目录
  `mkdir: File exists`；确认无遗留构建进程后，按 `pitfalls.md` 顺序执行
  `make musl-generic ARCH=riscv64 -C vendor/lwext4_rust/c/lwext4` 再重试，release build 通过。
  该 release 产物在 `-snapshot -smp 8 -m 256M` 同一四路退出压力中完成标记、health
  和 QEMU exit 0 均通过，日志 `/tmp/respos-release-smp8-exit.log`。
- **当时仍未完成**：该阶段尚未运行完整 fork/exec/wait、futex、pipe/socket 或共享地址空间
  压力；这些项目的后续结果见上一节。BuildStorm 与完整 CAgent 仍未运行。

## 2026-08-06 RV64 affinity-aware dispatch 与退出压力复验（未提交）

- **当前基线**：`dev` HEAD 为 `dc793c4`（`feat: 初步完成 RV64 smp 启动和调度`）；本轮
  affinity/scheduler 及本文档改动尚未提交。工作树原有两份未跟踪中文文档未修改。
- **实现**：全局 ready queue 现在为当前 CPU 选取最高优先级的首个 affinity-compatible
  任务，不移动不兼容任务，保留同级 FIFO 顺序；候选任务还必须已释放 context owner。
  ready 发布、stopped/wakeup、requeue 和 idle handoff owner 释放均只从该任务 affinity
  允许的 online idle hart 中选择 IPI 目标，避免只允许高编号 CPU 的任务无人唤醒。
- **调度器锁内分配**：debug invariant 不再在持 scheduler 锁时构造 `BTreeSet`；改用
  ready 数量、现有 `task_index` 及队列反查保留 bitmap、重复 tid、队列错位和
  ready/blocked 重叠检查。这只移除一个已知的 heap/scheduler 锁交叉放大因素，不是退出风暴修复。
- **验证通过**：`make RV_MODE=debug RV_USER_FEATURES= build-rv`、
  `make LA_MODE=debug LA_USER_FEATURES= build-la`、两个 `cargo fmt --check` 和 `git diff --check`。
  RV64 debug pub 以 `-snapshot -smp 8 -m 256M` 启动（OpenSBI 本轮 boot hart 为 5），依次执行
  `/glibc/busybox taskset 80|40|20|10|8|4|2|1 ...`；包括只允许 CPU 7 的 `sleep 1`
  在内的 8 个进程均 exit 0，随后 `nproc` 输出 8，guest `quit` 使 QEMU exit 0。日志为
  `/tmp/respos-affinity-smp8.log`。
- **单核回归**：同一 RV64 debug 产物以 `-snapshot -smp 1 -m 256M` 运行
  `true; sleep 1; true`，三个子进程均 exit 0，guest `quit` 使 QEMU exit 0；日志为
  `/tmp/respos-affinity-smp1.log`。本轮未注入 debug runner，因此这不替代 CAgent 单项回归。
- **修复前阻塞证据**：以单参数 `busybox sh -c` 确实启动四路
  `busybox timeout 3 busybox sleep 60` 后，debug、`-snapshot -smp 8 -m 256M` 在 50 秒内仍未输出
  `SMP_EXIT_STORM_DONE`，宿主 timeout 124；日志为 `/tmp/respos-smp8-exit-storm2.log`。未观察到
  panic。本证据已由上一节的动态 GDB 根因和修复后矩阵取代，保留用于说明回归对照。

## 2026-08-05 交接审查：RV64 SMP 未提交工作树

- **交接基线**：`dev` 的已提交 HEAD 为 `cf30f64`；下列 SMP 改动和
  `docs/codex/buildstorm-smp-plan.md` 均尚未提交。工作树还保留用户已有的未提交修改；本次没有
  reset、checkout、提交、推送，也没有改动 `testsuit/cagent-test/` 或官方测试逻辑。
- **本次审查范围**：已逐项检查 RV64 entry/HSM/SBI、per-CPU trap scratch 与 `tp`/TLS 保存、
  per-CPU processor/idle handoff、全局 ready queue 的 claim/owner 协议、`MemorySet` RFENCE、
  `/proc/cpuinfo` 与 affinity syscall 的 diff。`git diff --check` 当时通过。没有发现已复现的
  双运行 panic 或 ABI 寄存器破坏；但这只是代码审查和有限 guest 回归，不能替代并发压力验证。
- **已验证的最小边界**：RV64 debug build、LA64 debug build（后者在最后一次仅删除未使用方法前）、
  两个 rustfmt check 和 diff check 已通过。当前 owner 版本在 RV64 pub `-smp 1` 的
  `true; sleep 1; true`、以及 `-smp 8` 的 `nproc=8`、CPU 0--7 的 `/proc/cpuinfo`、四路后台
  `sleep 1` + `wait` 中均 exit 0；未见 owner assertion、`sepc=0` 或 panic。
- **不能作为通过的项目**：owner 栅栏加入后尚未重新运行 CAgent；文中更早的 CAgent kernel
  `pass 8829` 仅适用于 handoff 前的工作树，不代表当前版本。也未运行 BuildStorm、完整 fork/exec/wait、
  futex、pipe/socket 或共享地址空间压力。
- **审查结论 / 阻塞项**：四路 `busybox timeout 3 busybox sleep 60` + `wait` 在
  `-snapshot -smp 8 -m 256M` 的 debug 和 release 中均超过 60 秒，不可计为成功。暂停 GDB 时可见
  多个 CPU 在 `exit_process_group → recycle_data_pages → MapArea::unmap_one` 的页回收路径，争用
  全局 heap lock；另有 CPU 竞争 scheduler lock，debug invariant 还会在该锁内分配 `BTreeSet`。
  这证明严重迟滞/活性风险，**尚不足以断言永久死锁或 TLB 根因**。在专项审计完成前，不应提交为
  “BuildStorm 可用 SMP”，也不应以关闭 debug assertion 或未经审计的 per-CPU allocator 掩盖问题。
- **另一个明确的 ABI 缺口**：`sched_setaffinity` / `sched_getaffinity` 已按 online hart mask
  读写 TCB affinity，但 `fetch_task()` 尚未按该 mask 筛选候选 task。因此 setaffinity 可能成功返回
  却未实际约束运行 CPU；在实现筛选、空队列/唤醒策略和回归前，不应声称 affinity 语义完成。
- **实现上限与待审计边界**：early stack、`PerCpu`、online/idle mask 固定为 8 个 hart；入口汇编在
  Rust 范围检查前已经按 `a0` 计算栈，故只能用于预期 hart ID 小于 8 的 QEMU 配置。`tlb_hart_mask`
  是“曾运行过”集合而非 active mask；虽保守地发 RFENCE，但没有 request/ack 协议，不能作为共享
  `MemorySet`、frame 回收或多线程工具链并发安全的证明。

### 接手后的优先顺序

1. 固定镜像与 QEMU 命令（全程 `-snapshot`），为 2/4/8 核的短 `fork/exec/wait`、futex、pipe、
   socket 和退出压力建立每轮 serial log、guest exit code、wall time 表；先区分“慢”与“不收敛”。
2. 从 `MemorySet::recycle_data_pages`、frame/buddy allocator、`MapArea::unmap_one` 和
   `Scheduler::assert_invariants` 开始，记录每一把锁的获取顺序及锁内分配；以最小复现定位 timeout
   storm 的具体等待环或串行热点。未得到证据前不替换 allocator。
3. 实现并验证 affinity-aware dequeue（不得饿死仅允许某一 CPU 的 ready task）；随后重跑上述矩阵。
4. 完成 active-mask + shootdown request/ack，之后才允许共享地址空间并发、pthread/futex 和
   BuildStorm toolchain。每一项修复后均重跑 RV64 build/fmt/diff 及干净 pub 的 SMP=1 CAgent 单项。

## 2026-08-05 RV64 SMP owner 栅栏与退出清理压力（未提交，待设计决策）

- 为覆盖 Blocked→wakeup 在 `__switch` 前重新入 ready queue 的窗口，TCB 新增 atomic CPU owner。
  `fetch_task()` 只会 claim owner 为 `NO_CPU_OWNER` 的 Ready task；yield/preempt/blocked/stopped/exited
  的 current 都先切到本 CPU idle，idle 在 context 已保存后才释放 owner。该 owner 同时是 debug
  不变量：被恢复的 task 必须由当前 CPU claim。RV64/LA64 debug build、两个 fmt check、diff check
  通过；RV64 SMP=1 `true → sleep 1 → true` 均 exit 0，SMP=8 `nproc`、完整 `/proc/cpuinfo`、四个
  后台 `sleep 1` + `wait` 均 exit 0，未触发 owner/assert/panic。
- **压力失败/迟滞证据**：在 QEMU `-snapshot -smp 8 -m 256M` 中，四路
  `busybox timeout 3 busybox sleep 60` + `wait` 在 release 与 debug 均超过 60 秒未收敛；不是
  debug-only scheduler invariant。GDB 暂停时，多个 CPU 正在 `exit_process_group →
  MemorySet::recycle_data_pages → MapArea::unmap_one`，并发争用全局
  `HEAP_ALLOCATOR` spin mutex；其他 CPU 处于 scheduler lock 等待或 idle handoff 的 `add_task`。
  Debug build 还显示 `Scheduler::assert_invariants()` 在持 scheduler lock 时为 `BTreeSet` 分配，
  加剧 heap/scheduler 争用。未观测到 owner 断言、`sepc=0` 或新的 kernel panic。
- 判断边界：当前证据支持“并发退出/页回收与全局 allocator 严重串行化（并可能存在锁序活性风险）”，
  不足以证明一个永久死锁或将其归因于 TLB stale mapping。下一步若继续处理，需要专项审计
  heap/frame allocator、`MemorySet::recycle_data_pages` 与 scheduler debug invariant 的锁范围；这已
  超出早期 SMP bring-up 的局部修补。未完成前不能把 timeout-storm 作为通过，也不能运行 BuildStorm。

## 2026-08-05 RV64 scheduler context-handoff 修复与首次多核任务回归（未提交）

- 基线提交：`cf30f64`；本轮未提交 SMP 工作树。GDB 已确认此前 8 核 `/proc/cpuinfo` panic 的
  直接原因是 `yield_current_task()`/`preempt_current_task()` 在当前任务仍执行时先
  `set_ready()+add_task()`、再 `__switch()`。其他 hart 能在其 context 尚未保存时 claim 同一
  TCB，造成同一 kernel stack/trap context 并发访问，最终 CPU5 观测到
  `InstructionPageFault (scause=12), sepc=0, stval=0`。这不是 procfs/文件系统语义失败。
- 修复：每 CPU `Processor` 新增受本核锁保护的 handoff slot。yield/preempt 先把仍为
  `Running` 的 current 放入该 slot 并切到本 CPU idle context；仅 idle loop 在 `__switch`
  已保存旧 context 后，将 handoff task 标记 Ready 并调用统一 `add_task()` 发布/IPI kick。因而
  task 对其他 CPU 可见时已不再执行，保留全局 scheduler lock 的 claim 语义且不持锁跨用户态。
- 验证：构建 `make RV_MODE=debug RV_USER_FEATURES= build-rv` 与
  `make LA_MODE=debug LA_USER_FEATURES= build-la` 均通过；两个 `cargo fmt --check` 与
  `git diff --check` 通过。恢复 `img/sdcard-rv-pub.img.gz` 后 `e2fsck -pf` 正常完成。RV64
  SMP=1 的 `true → sleep 1 → true` 均 exit 0；SMP=8 的 `nproc` 输出 8，`cat /proc/cpuinfo`
  完整输出 CPU 0--7、exit 0；在 BusyBox sh 中运行四个后台 `sleep 1` 与 `wait` 后 sh exit 0，
  shell 随后仍可 `nproc=8`。此前同一复现点未再 panic。
- 单核题一回归：干净 pub 镜像临时注入仓库 `scripts/cagent_debug.sh`（不纳入 Git）后，执行
  `/glibc/cagent_debug.sh kernel` 得到 `testcase cagent kernel pass 8829`，shell exit 0，guest
  日志 `/tmp/cagent_debug_30_2`。这不是多核 CAgent/BuildStorm 成绩。
- 当前边界：本次只覆盖 context handoff、启动、procfs、短 sleep 压力。共享 `MemorySet`、完整
  affinity 选择、futex/pipe/socket 并发压力与 BuildStorm toolchain 仍未验证；在这些回归通过前
  不能宣称 SMP 或题二完成。

## 2026-08-05 RV64 SMP Phase 1/2 early bring-up 与 per-CPU timer idle（未提交）

- 基线提交：`cf30f64`；当前工作树新增 RV64 HSM early bring-up，未提交。范围只覆盖
  RV64，CAgent、pub 镜像和 `testsuit/cagent-test/` 均未修改。
- 已实现：每 hart 64 KiB early stack；SBI HSM `hart_start`；first-arriving boot-hart 原子认领
  （不能假设 hart 0）；Release/Acquire boot-ready barrier；次 hart 的共享内核页表和本地 trap
  vector 设置。QEMU `-smp 2` 与 `-smp 8` 都到达 Rust user shell；8 核中 7 个次 hart 均打印
  `online (per-cpu timer idle)`（控制台输出交错）。`-smp 8` 下 `/bin/busybox uname -r` 输出
  `6.10.0-dev`，随后 `true` exit 0。
- 已完成本阶段的 per-CPU 基础：`sscratch` 固定指向 per-CPU trap scratch，内核 `tp` 为
  `PerCpu` 指针而用户 `tp` 仍按 Linux TLS ABI 保存/恢复；`PROCESSOR` 与 bootstrap context
  已按 CPU 独立；次 hart已打开本地 supervisor timer 并以 WFI 空闲。内核 timer handler 在
  没有 current task 的 idle hart 只重设本地 tick，不并发扫描全局 task timer。
- 重要边界：次 hart尚不会进入 scheduler 或运行用户任务；全局 run queue 的原子 claim、调度器
  IPI 唤醒，以及共享地址空间的 TLB shootdown 均未实现。因此这仍不是 BuildStorm 可用状态。
- 验证命令：`make RV_MODE=debug RV_USER_FEATURES= build-rv`、
  `cargo fmt --manifest-path os/Cargo.toml -- --check`、
  `cargo fmt --manifest-path user/Cargo.toml -- --check`、`git diff --check`；均通过。
  启动命令和现象见 `docs/codex/buildstorm-smp-plan.md` 的执行记录。

## 2026-08-05 RV64 SMP Phase 2 trap/processor 基础（未提交）

- 已完成：`sscratch` per-CPU trap scratch、kernel/user `tp` 分离、每次切换前更新 task kernel
  context 与 scratch kernel stack、per-CPU `PROCESSOR` 和独立 bootstrap/idle context。RV64
  `-smp 1`、`-smp 2` pub 启动均通过，次 hart 进入 `per-cpu idle`；用户态
  `/bin/busybox uname -r` 与 `true` 均 exit 0。RV/LA debug build 均通过。
- 已修复：次 hart timer 初诊断中读到的低 `stvec` 并非高半区映射缺失。`__trap_from_kernel`
  原先只有 2 字节对齐，链接地址低两位为二进制 `10`；而 `stvec` 的低两位是 trap mode，保留值
  会被 QEMU WARL 改写为错误地址。为该入口增加 `.align 2` 后，guest 实测 `stvec` 为对齐的
  高半区 `0xffffffc08035de18`，`-smp 2` 首个 SBI TIME tick 会从 WFI 返回；清理探针后
  `-smp 8` 的七个次 hart 都可进入 per-CPU timer idle。此前关于“补 `KERNEL_BASE`”的判断已撤销。
- 多核 scheduler、调度器 IPI 唤醒、共享地址空间的 TLB shootdown 仍未实现，不能运行
  BuildStorm 或宣称有真实用户任务并行。

## 2026-08-05 RV64 SMP IPI trap 基础（未提交）

- 已实现 SBI v0.2 IPI `send_ipi` 封装、`sie.SSIE` 使能、`SupervisorSoft` trap 分支，以及
  `sip.SSIP` 清除。IPI handler 只做 pending ack 和 per-hart 原子计数，不获取 scheduler、驱动
  或文件系统锁；这保留了后续“先发布 ready task、再 kick idle CPU”的锁顺序。
- QEMU `-smp 8` 实测每个次 hart 在完成 per-CPU 初始化后向自身发送一次 IPI；7 条交错的上线
  日志均含 `ipi=1`，随后 boot hart 到达 Rust user shell。此前在 boot hart 向 HSM
  start-pending hart 发送 IPI 的试验得到 `ipi=0`，已撤销；不能把未 online 的 hart 当作可可靠
  唤醒的调度目标。
- 此项只证明 SBI IPI → SSIP → supervisor trap → clear pending 的本地链路。尚未把 IPI 接到
  scheduler，也没有运行次 hart 的用户 task；仍需先完成 scheduler 内的 ready-task claim 与
  idle CPU 目标选择，才可启用真正多核调度。
- scheduler 的第一项并发修复已完成：`fetch_task()` 现在在持有全局 scheduler lock 时同时
  从 ready queue 出队并将 task 标为 `Running`，随后才释放锁并做 context switch。此前的
  “出队后、设 Running 前”窗口会让 signal/wakeup 路径看到既不在队列又仍为 `Ready` 的 task。
  RV64 debug pub 镜像启动后 `/bin/busybox true`、`uname -r` 均 exit 0；这尚不是多核任务
  调度验证。
- idle/task 返回闭环已在单核验证：阻塞任务在无 runnable task 时保存自身 context 并恢复本 CPU
  的独立 idle context；idle 在无锁状态重新启用 supervisor interrupt 后 WFI。boot hart 是目前
  唯一的全局 timeout service CPU，因此即使它 idle，kernel timer 仍会扫描 timeout 并把 task
  入队；次 hart 的 tick 只重编程本地 deadline。干净 debug pub 中 `/bin/busybox sleep 1` 随后
  返回 shell，接着 `true` exit 0。此前第一次实现漏掉从 user syscall 返回 kernel 时被硬件清除的
  `SIE`，表现为 WFI 返回但不进 timer trap；已修复，临时诊断日志已移除。

## 2026-08-05 RV64 SMP MemorySet residency 与 RFENCE 基础（未提交）

- `MemorySet` 新增保守的 `tlb_hart_mask`：task 即将恢复到某 hart 前记录该地址空间曾在该 hart
  加载；该位在地址空间生命周期内不主动清除，因此 context switch 与页表修改交错时可能多刷但
  不会漏掉仍持有旧 TLB 的 hart。所有既有 `MemorySet::flush_tlb()` 在本地 `sfence.vma` 和内存
  barrier 后，对掩码中的远端 hart 调用 SBI RFENCE `remote_sfence_vma`；当前无 ASID 分配器，
  故使用全地址空间刷新。
- QEMU/OpenSBI `-smp 8` bring-up 中，临时让每个次 hart 对已 online 的 boot hart 发起一次
  RFENCE，七项均成功返回（`rfnc=1`）；探针已移除。secondary 初始化顺序也改为先建立 `PerCpu`
  `tp`/`sscratch`，再激活并标记共享 kernel page table，避免 residency 访问未初始化 CPU 身份。
- 这提供了 page-table shootdown 的基础，不等于共享 `MemorySet` 的并发压力已经验证；secondary
  尚未进入 scheduler，仍需实现 idle mask、enqueue-after-kick 与多核 task 压力回归。

## 2026-08-05 RV64 secondary scheduler 首次接入失败（未提交，暂停扩大范围）

- 已实现 per-CPU idle bit、ready task 发布后二次取队列、SBI IPI kick，以及让 secondary 进入
  `task::run_tasks()` 的首次尝试；2 核 pub 可启动，`uname -r` 与 `sleep 1` 返回。默认 affinity、
  `sched_{get,set}affinity` online mask 和 `/proc/cpuinfo` 已改为真实 online hart，不再硬编码
  `0b11`；8 核下 BusyBox `nproc` 实测输出 8。
- **失败证据与根因**：同一 8 核 pub 启动后，`/bin/busybox nproc` 正确输出 `8`；紧接着执行
  `/bin/busybox cat /proc/cpuinfo` 会触发 kernel panic。GDB 在 `core::panicking::panic_fmt` 停住：
  CPU5 的 `kernel_trap_handler` 收到 `InstructionPageFault`，`sepc=0`、`stval=0`、`scause=12`；其余
  CPU 同时处于 scheduler `fetch()`、user timer preempt 和 per-CPU idle loop。此前并发 lazy
  `IDLE_TASKS` 初始化确实存在，已由 boot hart 提前初始化消除，但复测仍失败，故不是根因。
- **已确认的调度竞态**：`preempt_current_task()`/`yield_current_task()` 先对仍在本 CPU 执行的
  current task 做 `set_ready()` + `add_task()`，再调用 `__switch()` 保存其 kernel context。另一 CPU
  可在保存前从全局队列 claim 同一 TCB 并恢复/修改同一 kernel stack/trap context，最终可使返回 PC
  变为零。现有“fetch 时在 scheduler lock 内设为 Running”只能关闭出队后的状态窗口，不能关闭这段
  **context 尚未保存却已公开 runnable** 的窗口。
- 结论：问题位于多核 context-switch/ready 发布协议，不是 procfs、文件系统或单一 trap 汇编错误。
  当前 secondary `run_tasks()` 接入不得用于 BuildStorm；继续实现前必须设计“保存 current context
  与将其发布到 ready queue”的原子交接（例如 scheduler handoff trampoline 或受控的 CPU ownership
  协议），并为 task 加 CPU owner/debug 断言。不能仅增加 scheduler lock、关闭某个 timer 或针对
  `/proc/cpuinfo` 特判。
- 单核题一回归：在当前 pub 镜像、`-smp 1` 直接运行 `/glibc/cagent_debug.sh kernel`，得到
  `testcase cagent kernel pass 3533`，shell process exit 0；guest 日志目录为
  `/tmp/cagent_debug_6_2`（随镜像后续恢复会丢失）。这只证明 kernel 单项链路未被本轮 ABI
  改造破坏，不是 CAgent 全量或 SMP 回归。

## 2026-08-05 CAgent `CLONE_VFORK` 语义修复与最新基线

- 当前基线：`dev` 已提交 `269a94a`（包含 ext4 durability、timer、wait/TCP 修复）；本轮工作树新增
  `CLONE_VFORK` 父任务唤醒和诊断 runner 选择 timeout/跳过 command probe 的开关，尚未提交。
  `testsuit/cagent-test/` 仍是本地忽略的上游参考，未修改、未加入 Git。
- 根因（已由源码与受控结果共同确认）：`sys_clone()` 对 `CLONE_VFORK` 调用
  `blocking_and_run_next()`，但原实现未在子任务成功 `execve()` 时唤醒父任务；只有子任务最终
  exit 时的普通 `SIGCHLD` 路径会偶然唤醒父任务。因此 glibc `popen/posix_spawn` 的父进程会错误地
  等到 `/bin/sh -c` 命令结束，表现为 `popen()` 4 路约 4.6--12.3 s 而 `pclose()` 仅约 2--44 ms。
- 修复：`TaskControlBlock` 为 vfork 建立一次性弱父引用；`sys_clone()` 仅在
  `CLONE_VFORK` 设置它；成功 exec 在新映像、fd 和 signal 状态完成后唤醒，未能 exec 的子任务在
  最终退出时唤醒。普通 fork/SIGCHLD 不受影响。RV64 `make RV_MODE=debug RV_USER_FEATURES= build-rv`、
  两个 `cargo fmt --check` 和 `git diff --check` 均通过。
- 干净 RV64 pub、SMP=1、256 MiB 受控回归（先从 `img/sdcard-rv-pub.img.gz` 恢复并
  `e2fsck -pf`，临时注入 runner）：4 路 `factorial,date,network,cpu` 在 BusyBox timeout 下全部
  pass，分别为 15547、17829、18843、18057 ms。10 路在同一条件下 6/10 pass：factorial 48457、
  kernel 61549、fs-create 45538、fs-readwrite 62149、fs-search 67115、fs-usage 61507 ms；date、
  network、cpu、fs-directory 的 agent exit 均为 143（SIGTERM），其中 fs-directory validation
  exit 为 0，不能归为文件系统语义失败。此轮不等同官方成绩：官方 runner 还使用动态
  `/usr/bin/timeout`，其 4 路复现出现随机 exit 127（`/usr/bin/timeout: not found`），是下一项独立待查。
- 下一目标：先对动态 coreutils/dash exec 失败和 10 路单核排队分别计数；在没有逐层日志前，不把
  exit 127/143 归为某个固定 syscall。随后直接从干净镜像运行 `/glibc/cagent_testcode.sh` 和上游 judge
  更新可申报分数。最新临时 guest 日志目录：`/tmp/cagent_debug_vfork_fix_4_busybox/`、
  `/tmp/cagent_debug_vfork_fix_10_busybox/`（随镜像恢复会丢失）。

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
