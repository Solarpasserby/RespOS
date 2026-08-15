# RespOS 当前状态

## 2026-08-15 Phase 5 SysV SHM 核心 metadata 独立门禁（基于 `60deddb`）

- **独立契约**：新增 Linux/guest `sysv_shm_metadata_probe`，不创建工作 child，避免把 `shmctl`
  metadata 与 LA64 多子进程 signal/reap 活性混在一起。4113-byte segment 必须保留原始 `shm_segsz`，
  同时在 `SHM_INFO.shm_tot` 中计为 2 页；初始 owner/group、`0640` mode、creator pid、last-op pid、
  attach 数和零 atime/dtime 必须符合 Linux。
- **状态转换**：probe 通过 `SHM_STAT` index 和 `SHM_STAT_ANY` 找回同一 shmid；attach/detach 验证
  `shm_nattch=1->0`、`shm_lpid`、atime/dtime 的相对更新，`IPC_SET` 验证 mode 与 ctime，随后再次
  attach、`IPC_RMID`，要求最后 detach 前 `SHM_DEST` 可见且 `SHM_INFO` 仍计入 segment/页数，最后
  detach 后 shmid 返回 `EINVAL` 且全局计数恢复。
- **验证结果**：Linux 以 `-std=c11 -Wall -Wextra -Werror -O2` 编译并输出
  `SYSV_SHM_METADATA_LINUX PASS index=<namespace-dependent> size=4113 pages=2`。release 初赛 snapshot、
  4 GiB/2 hart 的
  RV64/LA64 均输出 `SYSV_SHM_METADATA PASS index=0 size=4113 pages=2` 与 runner PASS，日志为
  `/tmp/respos-{rv,la}-sysv-shm-metadata.log`；本轮无需修改内核。
- **边界**：该结果独立确认核心 `IPC_STAT/IPC_SET/SHM_STAT/SHM_STAT_ANY/SHM_INFO/SHM_DEST` 状态，不
  代表非 root permission denial、`SHM_STAT_ANY` 权限绕过、`SHM_LOCK/SHM_UNLOCK` capability 或时间戳
  的绝对 realtime 基准已闭合。LA64 `shmctl01` 的 20-child signal/reap teardown 阻断也仍存在，但不能
  再归因于本 probe 已覆盖的 metadata 返回值。

## 2026-08-15 Phase 5 SysV SHM size 与 `SHMMNI/SHMALL` 配额回收（基于 `9548ded`）

- **size/lookup 契约**：Linux 要求新建 segment 的 size 0 和 `SHMMAX+1` 返回 `EINVAL`。已有 keyed
  segment 可用 size 0 或原大小查询；超过原大小返回 `EINVAL`，而同时指定 `IPC_CREAT|IPC_EXCL` 时
  `EEXIST` 优先。segment 删除后无创建标志的查询返回 `ENOENT`，以 size 0 重新创建仍返回 `EINVAL`。
  Linux/RV64/LA64 的 `SHMMIN=1`、`SHMMAX=18446744073692774399` 及上述 errno 矩阵一致。
- **`SHMMNI` 契约**：扩展 `sysv_shm_attach_race_probe_linux.c`，通过 `IPC_INFO/SHM_INFO` 读取当前
  `shmmni` 和已用 segment 数；补满可用槽位后，下一次 `shmget(IPC_PRIVATE)` 必须返回 `ENOSPC`。
  删除一个未 attach segment 后必须能立即创建 replacement，全部 `IPC_RMID` 后 `used_ids` 必须恢复
  到测试前数值。当前 Linux namespace 在 `shmmni=4096` 下通过，并继续通过原双 attacher/回收循环。
- **`SHMALL` 契约**：[Linux `shmget(2)`](https://man7.org/linux/man-pages/man2/shmget.2.html) 规定申请
  后总页数超过 `SHMALL` 返回 `ENOSPC`；[Linux sysctl 文档](https://www.kernel.org/doc/html/latest/admin-guide/sysctl/kernel.html#shmall)
  明确额度按 IPC namespace 分别计数。本地容器的 `/proc/sys` 只读，未强行修改宿主；guest 在一次性
  QEMU 实例内把 `/proc/sys/kernel/shmall` 临时降为 2，验证两个单页/一个双页恰好成功，额外一页
  `ENOSPC`，删除单页后 replacement 成功，清理后恢复原值并以 `IPC_INFO` 复核。
- **RespOS 验证**：guest probe 使用固定 4096 项栈上 ID 表，避免其小型用户堆无法一次分配 32 KiB
  动态数组。release 初赛 snapshot、4 GiB/2 hart 的 RV64/LA64 均输出
  `SYSV_SHM_ATTACH_RACE PASS shmmax=18446744073692774399 shmmni=4096 shmall=4503599627370495
  pressure=128 attempts=64 invalid=0 attached=64` 与 runner PASS；最新日志为
  `/tmp/respos-{rv,la}-sysv-shm-shmall-final.log`。
- **实现结论与边界**：现有 `sys_shmget()` 已按 existing-key、创建必要性、size 范围的顺序返回上述
  errno；`ShmTable::alloc_id()` 在达到运行时 `shmmni` 时返回 `ENOSPC`，页数总和按运行时 `shmall`
  返回 `ENOSPC`，`IPC_RMID` 会归还两种额度，本轮不需修改内核。当前关闭默认 size、`SHMMNI=4096`
  顺序耗尽，以及 clean-table 下调 `SHMALL=2` 的页计数/恢复；RespOS 尚无 IPC namespace，已有对象时
  动态下调、并发创建、物理 `ENOMEM` 和单调 segment/attach ID 溢出仍未覆盖。

## 2026-08-15 Phase 5 SysV SHM 双 attacher 与回收循环门禁（基于 `1fc3915`）

- **扩展契约**：Linux/guest `sysv_shm_attach_race_probe` 从单 child 竞态扩展为每轮两个 child 同时
  `shmat()` 已标记 `IPC_RMID` 的同一 segment，共 32 轮、64 次 attach。每次只允许
  `EINVAL/EIDRM`，或成功读取原值且 `IPC_STAT` 仍有效、`shm_nattch >= 1`；成功后不可出现已脱离
  table 的孤儿映射。另增加 128 轮单页 `shmget -> shmat -> IPC_RMID -> shmdt` 回收复用，要求旧
  shmid 在最后 detach 后稳定返回 `EINVAL`。该循环验证反复分配/回收，不假定底层 frame 必然复用。
- **验证结果**：Linux 以 `-std=c11 -Wall -Wextra -Werror -O2` 编译运行通过，输出
  `SYSV_SHM_ATTACH_RACE_LINUX PASS pressure=128 attempts=64 invalid=0 attached=64`。release 初赛
  snapshot、4 GiB/2 hart 的 RV64/LA64 在强制扩大 reservation/commit 窗口和恢复默认构建后均输出
  `SYSV_SHM_ATTACH_RACE PASS pressure=128 attempts=64 invalid=0 attached=64`；日志为
  `/tmp/respos-{rv,la}-sysv-shm-multi-attach-{forced,default}.log`。
- **回归与边界**：恢复默认构建后，两架构 lifecycle 五向量和 `shm_nattch` 专项继续通过，日志为
  `/tmp/respos-{rv,la}-sysv-shm-multi-{lifecycle,nattch}-regression.log`。本轮只强化 probe，没有修改
  内核；它关闭的是两个同时在途 attacher 和 128 轮顺序回收循环，不代表任意 N 路并发、`SHMMNI`/
  `SHMALL`/attach-id 等资源上限耗尽或 `SHM_REMAP` 并发覆盖已经闭合。LA64 `shmctl01` 的多子进程
  signal/reap teardown 阻断也仍独立存在。

## 2026-08-14 Phase 5 `O_APPEND pwrite` 整 syscall 原子性基线（基于 `8617f87`）

- **Linux 契约**：新增 `scripts/pwrite_append_atomic_probe_linux.c`，每轮让 parent/child 通过同一
  `O_APPEND` open-file description 并发提交两个 128 KiB `pwrite(fd, record, 0)`，要求最终文件只能是
  完整 A-record 后接完整 B-record，或相反，不能在 64 KiB 内部分块边界交错。Linux 32 轮通过并输出
  `PWRITE_APPEND_ATOMIC_LINUX PASS`。
- **RespOS 反证**：新增 guest `pwrite_append_atomic_probe`。release 初赛 snapshot、4 GiB/2 hart 的
  默认 RV64 在 16 轮中交错 15 轮，输出 `PWRITE_APPEND_ATOMIC_EXPECTED_FAIL interleaved=15 rounds=16`；
  默认 LA64 本次 16 轮未自然命中。以 `TASK_A_PWRITE_APPEND_TEST_YIELD=1` 在每个 64 KiB chunk 后强制
  让出时，RV64/LA64 均稳定交错 16/16。日志为
  `/tmp/respos-rv-pwrite-append-atomic-baseline-v2.log`、
  `/tmp/respos-la-pwrite-append-atomic-baseline.log` 及对应 `default.log`；强制验证后已不带钩子重建
  两架构，默认 kernel 已恢复。
- **根因与边界**：`sys_pwrite64()` 为限制 kernel buffer 以 `IO_CHUNK_SIZE=64 KiB` 循环复制/写入；
  每个 chunk 单独调用 `File::pwrite_at_offset()`，只在该 chunk 内持有 open-file inner lock、重新选择
  EOF，故另一个 writer 可插入整块。现有锁证明 chunk 级 append 原子，不证明一次大 syscall 原子。
- **待协商实现**：不能简单让现有 spin lock 跨 `copy_from_user()` 或调度让出持有，否则单核 contention
  可死锁，lazy/COW fault 也会进入锁内。候选实现需在 inode/open-file 层建立可睡眠的 syscall 级序列化，
  或设计 append-range prepare/commit/rollback，并明确 EFAULT、short write、并发 truncate、不同 open
  description 和文件长度可见性。该修改跨 syscall/usercopy/File/PageCache 提交边界，实施前需与用户
  协商；当前提交只保留可复现门禁，不宣称修复。

## 2026-08-14 Phase 5 `shmctl01` 恢复审计与 LA64 teardown 阻断（基于 `b78f082`）

- **审计方法**：仅临时取消 `user/oscomp_ltp_list.txt` 中 `shmctl01` 的注释并用
  `TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=shmctl01` 定向运行；审计后已恢复注释，未改变正式 695-case 清单。
  该 LTP case 覆盖 `IPC_STAT/SHM_STAT` 基本 metadata、20 child 独立 attach/detach，以及 parent attach
  后 20 child fork 继承/各自 detach 的 `shm_nattch=21→1→0`。
- **RV64 结果**：release 初赛 snapshot、4 GiB/2 hart 的 musl/glibc 均完整输出 12 项 TPASS、summary
  `1 passed, 0 failed`，单组约 116--117 ms；日志为 `/tmp/respos-rv-shmctl01-phase5-audit.log`。此前
  “两架构都会卡死”的旧注释不再准确，但尚不足以直接恢复正式清单。
- **LA64 阻断**：4 GiB/2 hart 的 musl 已输出全部 12 项 TPASS，包括继承组最后
  `after parent shmdt() shm_nattch=0`，随后在 `signal_children(); reap_children();` 阶段超过 case 自身
  30 秒 timeout，未产生 summary；日志为 `/tmp/respos-la-shmctl01-phase5-audit.log`。4 GiB/1 hart 复核
  排除了 secondary 上线竞态：它在第一组 20 child 已 detach、`shm_nattch=0` 后同样卡在最终
  signal/reap，日志为 `/tmp/respos-la-shmctl01-phase5-smp1.log`。两次 QEMU 均由宿主定向终止。
- **边界与下一步**：当前证据支持 SysV metadata/计数断言正确，不支持把 case 记为通过；阻断点属于
  LA64 多子进程 signal delivery、exit 或 wait/reap 活性，而不是 `shmctl` 返回值。正式清单继续注释
  `shmctl01`。若继续修复，需要带 child id/kill/wait 进度的最小 probe，再审计 LA64 task/signal/退出
  路径；该范围与既有 process-identity/task-lifecycle 高风险项重叠，实施前需与用户协商。

## 2026-08-14 Linux/POSIX Phase 5 SysV SHM attach/回收线性化（基于 `e831255` 的当前工作树）

- **Linux 契约与门禁**：新增 `scripts/sysv_shm_attach_race_probe_linux.c` 和 guest
  `sysv_shm_attach_race_probe`。64 轮中 child 的并发 `shmat(old_shmid)` 只允许两种结果：返回
  `EINVAL/EIDRM`，或成功且随后的 `IPC_STAT` 仍有效、`shm_nattch` 为 1--2；不允许 attach 成功后旧
  shmid 已被最后 detach 回收。另以已占用的非空 `shmaddr` 强制 attach 失败，确认返回 `EINVAL`，且
  失败预留撤销后 survivor 最后 detach 仍能删除 segment。Linux 以
  `-std=c11 -Wall -Wextra -Werror -O2` 编译并输出 `SYSV_SHM_ATTACH_RACE_LINUX PASS`。
- **修复前反证与根因**：在 `sys_shmat()` 发布 table owner 后、安装 VMA 前强制让出 64 次，RV64/LA64
  release 初赛 snapshot、4 GiB/2 hart 均稳定输出
  `SYSV_SHM_ATTACH_RACE_EXPECTED_FAIL orphan=64 invalid=0 attached=0`，日志为
  `/tmp/respos-{rv,la}-sysv-shm-attach-race-baseline.log`。最后 detach 只观察已安装 VMA，因而可在
  in-flight attach 尚未进入 `shm_attach_count()` 时删除 segment；随后 attach 虽成功，却已没有可查询的
  shmid。审计同时发现通用 mmap hint 路径会把被占用的非空 `shmaddr` 静默搬到其他地址，偏离 Linux
  `shmat` 的精确地址契约。
- **实现与线性化点**：`ShmSegment::pending_attaches` 在持有 `SHM_TABLE` 时先预留，VMA 安装成功或失败后
  再在同一 table 下撤销；`IPC_RMID`、显式/隐式 detach 只有在 pending 与已提交 attachment 都归零时
  才释放 segment。成功路径在撤销预留前已安装 VMA，失败路径同时删除 attach owner，并再次执行延迟
  删除判定。非空 `shmaddr` 且未指定 `SHM_REMAP` 时改为精确、不可覆盖映射，内部地址冲突规范化为
  `EINVAL`；`SHM_REMAP` 仍走显式覆盖路径。
- **双架构验证**：强制让出构建在 RV64/LA64 均为 `invalid=0 attached=64`；日志为
  `/tmp/respos-{rv,la}-sysv-shm-attach-race-final-forced-v2.log`。恢复默认构建后 RV64 为
  `invalid=1 attached=63`，LA64 为 `invalid=0 attached=64`，日志为对应
  `final-default-v3`/`final-default-v2`。lifecycle 五向量和 `shm_nattch` 专项在两架构继续通过；聚焦
  `shmat01,shmdt02,shmctl03` 时 RV64 musl/glibc 与 LA64 musl 均为 `3 passed, 0 failed`，LA64 glibc
  只有既有 64 KiB `SHMLBA` rounding 差异。
- **剩余边界**：本轮关闭的是单 segment 的 `shmat` 与最后 detach/`IPC_RMID` 发布竞态及失败回滚；
  多个并发 attacher、`SHM_REMAP` 与并发 unmap、资源上限/attach-id 耗尽、多轮 frame 复用压力和其余
  `IPC_STAT/SHM_STAT` metadata 仍需独立门禁。`shmctl01` 的 metadata/计数断言已通过，但 LA64
  teardown 活性阻断仍使其不能恢复，见上节。

## 2026-08-14 Linux/POSIX Phase 5 SysV SHM `shm_nattch` MM identity（基于 `4e2e6c7` 的当前工作树）

- **Linux 契约与门禁**：新增 `scripts/sysv_shm_nattch_probe_linux.c` 和 guest
  `sysv_shm_nattch_probe`，覆盖初始 0、同一 MM 两次 attach 计 2、共享该 MM 的 pthread 存活期间仍为
  2、fork 复制为两个独立 MM 后计 4、child exit 回到 2，以及逐次 detach 的 1/0。Linux 以
  `-std=c11 -Wall -Wextra -Werror -O2 -pthread` 编译并输出 `SYSV_SHM_NATTCH_LINUX PASS`。
- **修复前反证与根因**：RV64/LA64 release 初赛 snapshot、4 GiB/2 hart 都在 worker thread 存活时把
  两个 attachment 报为 4，输出 `SYSV_SHM_NATTCH_EXPECTED_FAIL thread_count=4`；其余重复 attach、fork
  与 detach 向量正确。日志为 `/tmp/respos-{rv,la}-sysv-shm-nattch-baseline.log`。根因是
  `shm_attach_count()` 按 `TASK_MANAGER` 中每个 TCB 扫描，同一 `Arc<RwLock<MemorySet>>` 被线程组重复
  累计。
- **实现与验证**：统计先按 `Arc<RwLock<MemorySet>>` identity 去重，再对每个独立 MM 中匹配 segment
  frames 的 attach id 计数；因此线程共享不增加，fork 生成的新 MM 仍独立累计。修复后 RV64/LA64
  2-hart 均输出 `SYSV_SHM_NATTCH PASS` 与 runner PASS，日志为
  `/tmp/respos-{rv,la}-sysv-shm-nattch-fix.log`。上一轮五向量 lifecycle probe 双架构回归通过，日志为
  `/tmp/respos-{rv,la}-sysv-shm-nattch-lifecycle-regression.log`；`shmctl03,shmctl07,shmctl08` 在两架构
  musl/glibc 均为 `3 passed, 0 failed`，日志为 `/tmp/respos-{rv,la}-sysv-shm-nattch-ltp.log`。
- **剩余边界**：本轮固定的是 observable `IPC_STAT.shm_nattch` 与生命周期零值判断；并发 `shmat`
  发布/失败回滚与最后 detach/`IPC_RMID` 竞争已由上节闭合。被正式清单注释、历史会时序性卡死的
  `shmctl01` 未恢复，多 attacher 与多轮资源压力仍需独立 probe。

## 2026-08-14 Linux/POSIX Phase 5 SysV SHM `IPC_RMID` 地址空间回收（基于 `f22b3fd` 的当前工作树）

- **Linux 契约与门禁**：新增 `scripts/sysv_shm_lifecycle_probe_linux.c` 和 guest
  `sysv_shm_lifecycle_probe`。五组向量确认：`IPC_RMID` 立即释放 key namespace、旧 mapping 活到最后
  detach；显式最后 `shmdt` 后旧 shmid 返回 `EINVAL`；进程不调用 `shmdt` 而 exit/成功 exec 时也必须
  隐式 detach；signal group-exit 遵守同一回收；fork 继承 mapping 的 child 退出后，parent 仍能访问并
  再次 attach，直到 parent 最后 detach 才删除 segment。Linux 以
  `-std=c11 -Wall -Wextra -Werror -O2` 编译并输出
  `SYSV_SHM_LIFECYCLE_LINUX PASS`。
- **修复前反证与根因**：release 初赛 snapshot、4 GiB/2 hart 的 RV64/LA64 都只有显式 detach 通过，
  exit/exec 后旧 shmid 仍可 `shmat`，输出
  `SYSV_SHM_LIFECYCLE_EXPECTED_FAIL exit_stale=true exec_stale=true`；日志为
  `/tmp/respos-{rv,la}-sysv-shm-lifecycle-baseline.log`。根因是 `MemorySet` 回收了 PTE/frame 引用，但
  segment 及 `attach_owners` 由独立 `SHM_TABLE` 持有，清理只从 `sys_shmdt()` 触发。
- **实现与所有权**：`MemorySet` 暴露当前 attach id 快照；成功 exec 在安装新 MM 后、group exit 在旧
  MM 完成 recycle 后，把隐式 detach 提交给 `SHM_TABLE`。提交前扫描 live MM：若 fork 或
  `CLONE_VM` peer 仍持有同一 attach id，只更新时间而不删除 owner/segment；只有已标记删除且全局
  attachment 归零时才释放 segment。显式 `shmdt` 复用同一提交函数，避免 fork child 先 detach 就过早
  丢失共享 attach owner。
- **双架构验证**：RV64/LA64 2-hart 五组向量均输出 `SYSV_SHM_LIFECYCLE PASS` 与 runner PASS，日志为
  `/tmp/respos-{rv,la}-sysv-shm-lifecycle-final.log`。既有跨 attach futex probe 同配置通过，日志为
  `/tmp/respos-{rv,la}-sysv-shm-lifecycle-futex-regression.log`。聚焦 `clone05,shmat01,shmdt02` 时 RV64
  双 libc、LA64 musl 均为 `3 passed, 0 failed`；LA64 glibc 的 clone/shmdt 通过，只有已知旧 64 KiB
  `SHMLBA` 的 shmat rounding 失败，日志为 `/tmp/respos-{rv,la}-sysv-shm-lifecycle-ltp.log`。
- **剩余边界**：`shm_nattch` 的共享 MM/多线程去重已由上节闭合；本轮仍不宣称关闭 `shmat` 发布与
  最后 detach 的真实并发竞态、多轮资源压力或 `SHM_STAT/IPC_STAT` 其余完整元数据。

## 2026-08-14 Linux/POSIX Phase 5 SysV SHM 跨 attach futex 闭合（基于 `806eb5a` 的当前工作树）

- **新增门禁**：`scripts/sysv_shm_futex_probe_linux.c` 与 guest `sysv_shm_futex_probe` 建立同一 SysV
  segment 的两个不同 attach 地址；child 从第二地址读 parent sentinel，随后在该地址执行 shared
  `FUTEX_WAIT`，parent 从第一地址执行 `FUTEX_WAKE`。Linux oracle 以
  `-std=c11 -Wall -Wextra -Werror -O2` 编译运行并输出 `SYSV_SHM_FUTEX_LINUX PASS`。
- **修复前反证**：release 初赛 snapshot、4 GiB/2 hart 下，RV64/LA64 都确认第二 attach 能读到
  sentinel，但 parent wake 始终返回 0，child 两秒后返回 `ETIMEDOUT`，最终输出
  `SYSV_SHM_FUTEX_EXPECTED_FAIL wake=0 child_status=256`。日志为
  `/tmp/respos-{rv,la}-sysv-shm-futex-baseline.log`。
- **根因与实现**：`ShmSegment` 已持有稳定的共享 `Arc<FrameTracker>`，所以不是数据页复制问题；
  `MemorySet::shared_futex_key()` 却以每次 `shmat` 新分配的 `attach_id` 作为 owner，同一 segment 的两个
  地址必然进入不同 futex key。修复后 `attach_id` 仍只标识一次 attach、供 `shmdt` 成组拆映射；shared
  futex key 改用该页 resident shared frame 的 PPN，页内 offset 继续区分不同 futex，因此不同虚拟地址
  attach 的同一物理页进入同一队列，不需要新增全局 segment 索引。
- **双架构验证**：修复后 RV64/LA64 同配置均输出 `SYSV_SHM_FUTEX PASS`，runner 输出
  `SysV SHM futex probe PASS`，日志为 `/tmp/respos-{rv,la}-sysv-shm-futex-fix.log`。普通
  `futex_wait01,futex_wake03` 回归在两架构 musl/glibc 均为 `SUMMARY: 2 passed, 0 failed`，日志为
  `/tmp/respos-{rv,la}-sysv-shm-futex-regression.log`。LA64 首组 musl wait 仍受已知 secondary hart
  冷启动窗口影响约 0.9 秒，但 case 通过，本修复不宣称关闭该时间问题。
- **剩余边界**：当前 probe 覆盖一个 segment 的单页、不同 attach 地址、跨进程数据可见性与
  wait/wake；`IPC_RMID` 显式/exit/exec/fork-inherited 生命周期已由上节闭合，并发 attach/detach、
  多页/复用压力仍需独立验证，不能由本项外推为 SysV SHM 全部完成。

## 2026-08-14 Linux/POSIX Phase 5 SysV SHM 清账与 LA64 glibc 2.38 `SHMLBA` 差异（基于 `7622572`）

- **当前结果**：release 初赛 snapshot、4 GiB/2 hart 聚焦 `shmat01,shmdt02`；RV64 musl/glibc 与
  LA64 musl 均为 `SUMMARY: 2 passed, 0 failed`，`shmat01` 内 NULL、aligned、`SHM_RND` 和
  `SHM_RDONLY` 四项全部通过；LA64 glibc 的 `shmdt02` 两个 `EINVAL` 向量通过，但 `shmat01` 的
  `SHM_RND` 一项失败，整个组为 `1 passed, 1 failed`。日志为
  `/tmp/respos-{rv,la}-sysv-shm-phase5.log`。旧完整日志中 setup 的固定 key `EEXIST` 当前不复现，
  不能继续记为 `IPC_RMID` 泄漏。
- **ABI 归因**：LA64 glibc 测试把 `SHMLBA` 编译为 64 KiB，期望把输入向下舍入到
  `0x2000200000`；RespOS 按 4 KiB `PAGE_SIZE` 舍入得到 `0x200020f000`。镜像实际是 glibc 2.38，
  其 LoongArch 专用 header 定义 `SHMLBA=0x10000`；同镜像 musl header 定义 4096。Linux
  [d23b779](https://github.com/torvalds/linux/commit/d23b77953f5a4fbf94c05157b186aac2a247ae32)
  已在 2024 年把 LoongArch `SHMLBA` 改为 `PAGE_SIZE`，glibc
  [cae3c9e](https://github.com/bminor/glibc/commit/cae3c9e3a117fd240fbf5fd4b403ef4e5304c4a6)
  随后删除专用 64 KiB header，恢复 generic page-size 定义。
- **决策与剩余边界**：内核保持当前 Linux 的 page-size ABI，不按调用二进制猜测 4/64 KiB，也不为
  旧 glibc 特判地址尾数；该单项记为 glibc 2.38/LTP header `已知差异`，纳入统一 runtime 更新。
  本轮没有为旧 runtime 修改源码。现有 `shmat01` 只验证单次 attach、rounding、readonly 与基本计数；
  segment 的跨 attach 数据/futex identity 与 `IPC_RMID` 基本生命周期已由上方专项闭合，但并发发布/
  回收与完整 metadata 仍不能由三环境通过外推为完成。

## 2026-08-14 Linux/POSIX Phase 5 `CLONE_VFORK|CLONE_VM` 可见性（基于 `e39cdd9` 的当前工作树）

- **失败契约与根因**：LTP 20240524 `clone05` 要求 vfork child 在共享地址空间把全局
  `child_exited` 置 1，且 parent 必须等 child 退出后才从 `clone()` 返回。修复前 RV64/LA64 的
  musl/glibc 都正确等待约 0.16 秒（LA64 每次冷启动的首组 musl 另受 secondary 上线影响约 1 秒），
  但都读回 0。`CloneFlags::share_user_vm()` 仍保留 2026-06 的临时规避：非线程 vfork 即使带
  `CLONE_VM` 也复制地址空间，因此 child 写入对 parent 不可见。
- **实现**：删除该过期例外，所有 `CLONE_VM` 都共享同一内层 `MemorySet`。2026-08-08 已落地的
  per-task 可替换 MM handle 使这一恢复安全：child exec 只替换自己的 handle，parent 继续持有旧 MM；
  child exit 的回收路径也会检测线程组外的共享 owner，不提前回收旧地址空间。既有 vfork parent
  blocked-before-publish 与 exec/exit 一次性唤醒协议保持不变。
- **双架构门禁**：release 初赛 snapshot、4 GiB/2 hart 的 `clone05` 在 RV64/LA64、musl/glibc
  四组均从 `0 passed, 1 failed` 变为 `1 passed, 0 failed`；基线日志为
  `/tmp/respos-{rv,la}-clone05-phase5.log`，修复日志为
  `/tmp/respos-{rv,la}-clone05-vfork-mm-fix.log`。临时纳入但未保留在正式 695 清单的
  `vfork01/vfork02` 也在四组环境各为 `2 passed, 0 failed`，日志
  `/tmp/respos-{rv,la}-vfork-extra-phase5.log`。
- **exec 回归边界**：RV64 final snapshot、4 GiB/2 hart 的 CAgent 十项全通过；随后
  `BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok`，说明 command/process 创建路径未观察到 parent
  MM 被 child 新映像覆盖的回归；精确隔离边界仍以 per-task handle 源码所有权为证据。90 秒诊断窗口在
  正式 timed BuildStorm 编译期间主动结束，故不记录 BuildStorm 完成或性能结论；日志
  `/tmp/respos-rv-vfork-cagent-regression.log`。

## 2026-08-14 Linux/POSIX Phase 5 mmap EOF/truncate/SIGBUS 基线（基于 `fcd68d5` 的当前工作树）

- **可复现入口**：`testrunner` 新增 `TASK_A_MMAP_PHASE5_PROBE=1`，会单独执行已有的
  `mmap_phase5_probe`、回报子进程状态并安全关机。宿主 Linux oracle 以
  `-std=c11 -Wall -Wextra -Werror -O2` 编译运行，shared、private、private COW/truncate 三组均
  `PASS`，最终输出 `MMAP_PHASE5_LINUX ALL PASS`。
- **双架构当前基线**：release 初赛 snapshot、4 GiB/2 hart 下，RV64 与 LA64 都稳定报告同一七项
  `MMAP_PHASE5_EXPECTED_FAIL`：shared/private 的初始整页越过 EOF 与 truncate 后 resident 整页越过
  EOF 未触发 `SIGBUS`；private 的 truncate 部分页尾未清零、映射后文件增长不可见；private COW
  所在整页被 truncate 后未触发 `SIGBUS`。日志为
  `/tmp/respos-{rv,la}-mmap-phase5-baseline.log`。
- **边界判断**：两架构失败集合完全一致，当前归类为统一的 file-backed resident provenance、动态 EOF
  与 truncate invalidation 缺口，而非架构页表特例。该项会改变 VMA/PTE/frame 生命周期、truncate 后
  shootdown 和 fault 分类；本轮只固定诊断入口与证据，未修改内核语义，按用户确认后再实施 M3 方案。

## 2026-08-14 Linux/POSIX Phase 5 futex bitset/wake 清账与 LA64 冷启动阻断（基于 `b7cb356`）

- **已关闭项**：release 初赛 snapshot、4 GiB/2 hart 聚焦
  `futex_wait_bitset01,futex_wake03`；RV64 的 musl/glibc 两 case 均通过，其中 absolute
  monotonic/realtime wait 约 100 ms，wake 依次精确唤醒 1--10 个 child 后返回 0。LA64 的
  `futex_wake03` 两套 libc 也全部 11 项通过。日志为 `/tmp/respos-{rv,la}-futex-ltp-phase5.log`。
- **LA64 稳定差异**：LA64 musl 是每次启动的第一组。其首个 absolute monotonic wait 两轮分别耗时
  约 859 ms 和 872 ms，超过 LTP 的 200 ms 上限，且两轮都在等待期间打印 secondary hart online；
  同一 case 随后的 realtime wait 约 100 ms。接着运行的 glibc monotonic/realtime 均约 100 ms。
  第二轮日志为 `/tmp/respos-la-futex-wait-bitset-rerun-phase5.log`。
- **边界判断**：当前证据说明 futex bitset 的 absolute timeout 解析、`ETIMEDOUT` 与正常 steady-state
  路径可用，旧完整日志中的 RV realtime 早醒和 RV wake 首项未 reap 也未在当前 HEAD 复现；但不能把
  LA64 musl 首组失败归咎于 libc，因为 syscall 确实阻塞约 0.87 s。现象与 secondary 启动窗口强相关，
  归入待协商的 LA64 跨 hart 时间/启动调度审计；本轮不放宽测试、不伪造 deadline，也不修改架构时钟。

## 2026-08-14 Linux/POSIX Phase 5 `getcwd04` rename 竞态清账（基于 `8e2336a`）

- **历史结果纠正**：`rv-output.txt`/`la-output.txt` 中两架构 musl/glibc 的 `getcwd04` 都以
  status 32 结束，但正文是 `TCONF: Test needs at least 2 CPUs online`；这四项没有执行 rename/getcwd
  竞态，不能记为内核语义失败，也不能记为通过。
- **当前证据**：release 初赛 snapshot、4 GiB/2 hart 聚焦 `getcwd04`；RV64/LA64 的 musl/glibc
  四组各运行约 5 秒，均输出 `TPASS: Bug is not reproduced!` 与
  `SUMMARY: 1 passed, 0 failed, 0 skipped`。日志为 `/tmp/respos-{rv,la}-getcwd04-phase5.log`。
- **适用边界**：该 LTP 只验证另一个 task 持续 rename cwd 内普通文件时，当前 task 的 cwd 字符串
  不得退化或改变；它不覆盖 cwd 自身/祖先被 rename、跨 mount rename、已删除 cwd 的 `ENOENT`、
  rename 与 chroot 并发，也不能替代 namei/rename 的更广泛 SMP 原子性测试。本轮没有源码修改。

## 2026-08-14 Linux/POSIX Phase 5 LA64 `PROT_NONE`（基于 `7619764` 的当前工作树）

- **失败边界**：release 初赛 snapshot、4 GiB/2 hart 聚焦 `mmap05`；修复前 RV64 的
  musl/glibc 均收到预期 `SIGSEGV`，LA64 的 musl/glibc 却都能从 `PROT_NONE` 文件映射读到原字节，
  因而分别为 `SUMMARY: 1 passed, 0 failed` 与 `SUMMARY: 0 passed, 1 failed`。基线日志为
  `/tmp/respos-{rv,la}-mmap05-phase5.log`。
- **根因与表示**：LA64 PTE 的 `NR=61`、`NX=62` 位号正确；本地 QEMU 10.0.2 的软件 refill
  `helper_ldpte()` 却把读出的整个 PTE 与 `TARGET_PHYS_MASK` 相与，导致高位 inhibit 标记在进入 TLB
  前丢失。当前只对用户 `PROT_NONE` 叶子清硬件 `V`，同时保留 bit 10 `PROTNONE` 与 bit 7 software
  present；`PageTableEntry::is_valid()` 表示 resident/software-present，因此 `mprotect()`、`munmap()`、
  `fork()` 与 debug invariant 仍能找到该页，而硬件访问稳定触发 page-invalid fault。普通 R/W/X 页、
  页表目录项和 RV64 后端不变。
- **验证证据**：修复后 LA64 聚焦 `mmap05` 的 musl/glibc 均为
  `SUMMARY: 1 passed, 0 failed`，日志 `/tmp/respos-la-mmap05-protnone-fix.log`；临时仅把 LTP
  `mprotect04` 加入构建过滤后，两套 libc 的 `RW -> PROT_NONE` SIGSEGV 与后续 `PROT_EXEC` 恢复两项
  均 `TPASS`，日志 `/tmp/respos-la-mprotect04-protnone-fix.log`，默认 LTP 清单随后恢复原样。既有
  `mprotect05` 在 LA64 双 libc 通过；RV64 回归 `mmap05,mprotect05` 双 libc 各
  `SUMMARY: 2 passed, 0 failed`，日志 `/tmp/respos-rv-protnone-regression.log`。
- **剩余边界**：本轮只关闭 `PROT_NONE` 的 mmap/mprotect 可观察权限及恢复路径；mmap EOF/truncate/
  `SIGBUS`、mprotect 失败原子性与并发 user-copy、LA64 QEMU 10.0.2 上单独依赖 NX/NR 的 execute-only/
  write-only 最小权限仍需各自验证，不能由本轮结果外推为 MM Phase 5 全闭合。

## 2026-08-14 Linux/POSIX Phase 5 musl `recvmmsg()` bad-vector wrapper 阻断（基于 `659eeb9`）

- **当前差异**：release 初赛镜像、4 GiB/2 hart 聚焦 `recvmmsg01`；RV64/LA64 musl 都在第一项
  `EBADF` 通过后，于 bad message-vector 项被 `SIGSEGV` 终止，case 为
  `SUMMARY: 0 passed, 1 failed`。两架构 glibc 的 libc-time 与 old-kernel-time 两种 variant 共
  10 个 `EBADF/EFAULT/EINVAL` 断言全部通过，case 为 `SUMMARY: 1 passed, 0 failed`。日志为
  `/tmp/respos-{rv,la}-recvmmsg-errors-phase5.log`。
- **libc 调用链**：实际 RV64 musl 1.2.0 与 LA64 musl 1.2.5 的 `recvmmsg` wrapper 都在发起
  syscall 前按 `vlen` 遍历 `msgvec`，向每个 64-byte `mmsghdr` 的 offset 28/44 写零；
  这两个位置是 libc 64-bit `msg_iovlen/msg_controllen` 的高 32 位，用于适配 kernel ABI。LTP 的
  第二项刻意传 guard/bad address，因此 fault 发生在用户态预写，尚未进入内核 `sys_recvmmsg()`。
- **内核边界**：同一内核下 glibc 对 bad message vector 精确得到 `EFAULT`，并且 timeout 的负秒、
  溢出纳秒与 bad address 在两种 ABI 下均通过，证明当前错误路径可以处理真正到达 syscall 的输入。
  内核无法拦截 libc syscall 前的用户态 store，也不得用 signal handler 或测试二进制特判掩盖。
- **当前状态**：该项记为 musl/LTP wrapper `已知差异`，纳入待协商的统一 musl runtime 处理；本轮
  不改 runtime，也不从这份错误矩阵推导 `recvmmsg` 的阻塞 deadline、partial result 或 LA64 跨 hart
  timeout 已闭合。

## 2026-08-14 Linux/POSIX Phase 5 RV64 musl `epoll_create()` wrapper 差异（基于 `2958fa5`）

- **当前差异**：release 初赛镜像、4 GiB/2 hart 聚焦 `epoll_create02`；RV64 musl 的 libc variant
  对 `size=0/-1` 都错误返回新 fd，case 为 `SUMMARY: 0 passed, 1 failed`；RV64 glibc 与 LA64
  musl/glibc 均为 `SUMMARY: 1 passed, 0 failed`。两架构的 raw `__NR_epoll_create` variant 因没有
  legacy syscall 编号而 `TCONF`，不是内核返回值失败。日志为
  `/tmp/respos-{rv,la}-epoll-create-phase5.log`。
- **libc 调用链**：实际镜像中 RV64 musl 1.2.0 的 `epoll_create` 无条件执行 `a0=0` 后跳到
  `epoll_create1`，完全丢弃原 `size`；LA64 musl 1.2.5 则先判断 `size <= 0` 并在用户态返回
  `EINVAL`，正数才调用 `epoll_create1(0)`。两份 libc 已导出并以符号反汇编核对。
- **内核边界**：现代 RV64/LA64 ABI 只暴露 `epoll_create1(flags)`，其中 `flags=0` 是合法请求；
  `sys_epoll_create1()` 无法判断这个 0 来自合法直接调用，还是旧 musl 丢弃后的 invalid size。
  因此不得在内核拒绝 `epoll_create1(0)` 或按调用者二进制特判。
- **当前状态**：该项记为 RV64 musl runtime `已知差异`，与现有 `pathconf/readlink` 一起等待协商
  可复现的 libc 更新与镜像替换；本轮不修改 runtime，也不把 LA64/两组 glibc 的通过扩大成双架构闭合。

## 2026-08-14 Linux/POSIX Phase 5 `chroot()` 权限错误优先级（基于 `cd765fe` 的当前工作树）

- **Linux 契约**：新增 `scripts/chroot_permission_probe_linux.c`，以非特权进程确认 pathname
  lookup 与目录 search permission 先于 `CAP_SYS_CHROOT` 检查：不可搜索目录返回 `EACCES`、
  不存在路径返回 `ENOENT`，只有可访问目录才返回 `EPERM`。宿主 probe 以
  `-Wall -Wextra -Werror` 编译并输出 `CHROOT_PERMISSION_LINUX_PASS`。
- **根因与修复**：原 `sys_chroot()` 在复制用户路径前就检查 `euid`，因此把 `EFAULT`、路径解析错误、
  `ENOTDIR` 和 `EACCES` 全部遮蔽成 `EPERM`。当前顺序改为 copy path → lookup → directory/type 与
  search permission → privilege → commit root；所有检查通过前不修改 task root。
- **双架构证据**：修复前 RV64/LA64、musl/glibc 聚焦 `chroot01`--`chroot04` 均为
  `SUMMARY: 3 passed, 1 failed`，`chroot04` 期望 `EACCES` 而得到 `EPERM`。修复后同一 release、
  初赛 snapshot、4 GiB/2 hart 配置四组均为 `SUMMARY: 4 passed, 0 failed`，日志为
  `/tmp/respos-{rv,la}-chroot-phase5.log`；基线日志为
  `/tmp/respos-{rv,la}-chroot-phase5-baseline.log`。
- **剩余边界**：本轮只关闭现有单 task pathname/权限与失败原子性；mount namespace、并发 root/cwd
  可见性、capability/user namespace 仍未建模，不能由本轮 euid=0 门槛推导为已支持。

## 2026-08-14 Linux/POSIX Phase 5 ext4 特殊 inode、device 编码与 xattr（基于 `087af08` 的当前工作树）

- **失败边界与根因**：修复前双架构 musl/glibc 的 LTP `setxattr02` 仅通过 regular、directory、
  symlink 与 FIFO 四项；character、block、socket 三类节点错误接受 `user.*` xattr。`sys_mknodat()`
  丢弃 `dev`，ext4 又用普通文件 `O_CREAT` 代替这三类 lower inode，导致后续 lookup/stat 将其稳定识别为
  regular，而不是单纯的 xattr errno 映射错误。
- **实现边界**：VFS/namei 增加携带 device payload 的 `create_special()` 路径；ext4 对 FIFO、character、
  block、socket 统一调用 lwext4 `ext4_mknod()`，并从 raw inode device slot 恢复 `KStat.rdev`。普通
  `create()` 不再允许缺少 device payload 的 character/block placeholder。该变更只保证 namespace inode
  类型、rdev 与适用的 xattr 限制，不代表 RespOS 已有对应字符/块设备驱动或完整 open/read/write 语义。
- **device 编码修复**：首轮实现虽由 `stat()` 保存 ext4 的完整 32-bit device encoding，`statx`
  却只取 minor 的 legacy 低 8 位。当前按 Linux libc `dev_t` 布局统一拆分 major/minor，保留 20-bit
  minor；`mknod_xattr_probe` 使用 character major `0xabc`、minor `0x54321`，同时验证 raw
  `st_rdev=0x543abc21` 与 `statx.stx_rdev_*`。宿主 C probe 以 `-Wall -Wextra -Werror` 编译并通过编码
  检查；当前容器缺少 `CAP_MKNOD`，真实节点检查明确 skip，不把它记作宿主 runtime 通过。
- **专项与回归证据**：双架构 release、初赛 snapshot、4 GiB/2 hart 的专项均输出
  `MKNOD_XATTR_PROBE_PASS`，并验证 block `rdev=0x700`、四类特殊 inode mode 及 `user.*` xattr 失败。
  首轮同配置聚焦 `mknod01`--`mknod09`、`setxattr01/02`、`fsetxattr01`、
  `getxattr02` 共 13 case；RV64/LA64 的 musl/glibc 四组均为
  `SUMMARY: 13 passed, 0 failed`；device 编码修复后再聚焦
  `mknod01,setxattr02,statx02,statx03`，四环境均为 `SUMMARY: 4 passed, 0 failed`。日志为
  `/tmp/respos-{rv,la}-mknod-{xattr,devt}{,-ltp}-phase5.log`。
- **剩余边界**：当前关闭 Linux kernel/ext4 的 12-bit major、20-bit minor 编码和 stat/statx 回报；
  这不代表 RespOS 已有对应字符/块设备驱动或完整 open/read/write 语义，也不承诺超出 kernel
  32-bit device encoding 的 libc-only 大 major。

## 2026-08-14 Linux/POSIX Phase 5 ext4 `fallocate()` 预分配审计（基于 `80e8c5a`）

- **契约门禁**：新增 `scripts/fallocate_prealloc_probe_linux.c`，不只复刻 LTP 的返回值检查，还固定
  `FALLOC_FL_KEEP_SIZE` 必须增加 `st_blocks` 且不改变逻辑长度、默认模式必须把 EOF 扩展到 range end、
  两种模式均不得推进 open-file offset，并验证预分配范围零读及既有数据不变。宿主 Linux 以
  `cc -std=c11 -Wall -Wextra -Werror -O2` 编译并通过。
- **当前双架构差异**：release 初赛镜像、4 GiB/2 hart、筛选 `fallocate03`；RV64/LA64 的 musl 与
  glibc 均为 `SUMMARY: 0 passed, 1 failed`，每组八个 default/`KEEP_SIZE` 调用全部返回
  `EOPNOTSUPP`。日志为 `/tmp/respos-{rv,la}-fallocate-phase5-baseline.log`。该结果确认当前 HEAD 的
  普通 ext4 `File` 没有实现 `allocate_range()`，不是沿用历史 LTP 结果。
- **实现边界**：LTP 20240524 `fallocate03` 只断言 syscall 成功，不核对块预留；不得用稀疏
  `truncate`、临时写零再缩回或无操作成功换取通过。vendored lwext4 已能读取/写入 unwritten extent，
  但没有公开“在不改变 inode size 时建立 unwritten extent”的接口。真实修复需要给 extent allocator
  增加预分配入口，并由 ext4 `File` 在 PageCache writeback exclusion 下事务化调用；该项涉及 on-disk
  extent 元数据，实施前需取得用户确认。

## 2026-08-14 Linux/POSIX Phase 5 已删除目录 fd 的 `getdents64()`（当前工作树）

- **Linux 契约**：`scripts/getdents_unlinked_probe_linux.c` 确认，空目录被 `rmdir()` 从
  namespace 移除后，无论其 open fd 是否已经读取过目录流，后续 `getdents64()` 都返回
  `ENOENT`；未删除目录的 1-byte 结果 buffer 仍返回 `EINVAL`。宿主 probe 以
  `-Wall -Wextra -Werror` 通过。
- **根因与修复**：ext4 deferred unlink 已在 inode 上保留 `unlinked` 原子状态，但目录 fd 的
  `readdir_cached()` 未读取它，仍可从 lower inode 或 open-file `dirent_cache` 返回 `.`/`..`。
  当前 `File` 在生成或返回缓存目录项之前检查 ext4 inode 的 `is_unlinked()`，脱离
  namespace 后精确返回 `ENOENT`；不改变延迟回收和 open fd 对 inode 的 Arc 生命周期。
- **双架构证据**：RV64/LA64 release、初赛 snapshot、4 GiB/2 hart 聚焦
  `getdents01,getdents02`；两架构 musl/glibc 四组均为 `SUMMARY: 2 passed, 0 failed`，
  `getdents02` 内部的 `EBADF/EINVAL/ENOTDIR/ENOENT` 均通过。日志为
  `/tmp/respos-{rv,la}-getdents-phase5.log`。
- **剩余边界**：该检查当前由 ext4 的 deferred-unlink 状态驱动；`/dev/shm`、proc/devfs 等
  自定义目录若日后支持打开后 unlink/rmdir，需把同一语义下沉为通用 inode 状态，不能
  从本轮 ext4 通过推导它们已支持。

## 2026-08-14 Linux/POSIX Phase 5 `pwrite()` + `O_APPEND`（当前工作树）

- **目标契约**：Linux 在 open-file status 含 `O_APPEND` 时让 `pwrite()` 忽略显式 offset 并写到
  EOF，但仍不修改共享 open-file offset；这是 Linux 对 POSIX “`pwrite` 不受 `O_APPEND`
  影响”的已知偏离。`scripts/pwrite_append_probe_linux.c` 固定了 append 位置、payload、文件长度
  和 open-file offset 不变四条边界，宿主以 `-Wall -Wextra -Werror` 通过。
- **状态所有权**：`FileOp` 增加只供 pwrite syscall 使用的 `pwrite_at_offset()`。普通
  `File` 在同一 open-file inner lock 下读取 `O_APPEND`、选择当前 page-cache/lower-inode
  EOF 并完成写入，不推进 `inner.offset`。普通 `write()` 复用同一 locked writer 并仍推进
  offset；mmap/splice 等内核定位写继续使用 `write_at_offset()`，不被 `O_APPEND` 意外改写。
- **双架构证据**：RV64/LA64 release、初赛 snapshot、4 GiB/2 hart 聚焦运行
  `pwrite01`--`pwrite04`、对应 `_64` 变体及 `pwritev01/02/201/202` 的 `_64`/非 `_64`
  共 16 case；两架构 musl/glibc 四组均为 `SUMMARY: 16 passed, 0 failed`，日志为
  `/tmp/respos-{rv,la}-pwrite-phase5.log`。
- **剩余边界**：本轮关闭 LTP 覆盖的单调用选位语义并保持既有 pwritev 簇通过；大于
  `IO_CHUNK_SIZE` 的单 syscall 与并发 append writer 之间会在内部 chunk 边界交错，已由本文件顶部
  专项稳定反证。完整 Linux write-call atomicity 仍待更大的 I/O 提交模型，本轮不宣称已关闭。

## 2026-08-14 Linux/POSIX Phase 5 LA64 musl `readlink*()` wrapper 差异（基于 `de7c880`）

- **失败边界**：完整初赛日志中只有 LA64 musl 的 `readlink03/readlinkat02` 零长度项失败，
  这两项分别调用 `readlink(symlink, buf, 0)` 与 `readlinkat(dirfd, symlink, buf, 0)`；RV64
  musl 与两架构 glibc 均得到 LTP 要求的 `EINVAL`。其他 pathname/type/fd/fault 错误项已通过。
- **libc 调用链**：当前 LA64 `/musl/lib/libc.so` 是 musl 1.2.5；`readlink` 和
  `readlinkat` 在 `bufsiz==0` 时都把用户 buffer 替换为 1-byte 栈缓冲区，把 size 改为 1 再执行
  syscall，并在 symlink 非空时把成功短读归一成 0。RV64 镜像是 musl 1.2.0，其 wrapper
  直接传递零长度，因而没有该差异。
- **内核边界**：RespOS `sys_readlinkat()` 在路径解析与 copyout 之前已对真实
  `bufsize==0` 返回 `EINVAL`；当 LA64 musl 传入有效栈地址和 size 1 时，内核无法区分这是
  wrapper 的零长度代理还是应用真正请求的 1-byte 截断读。修改内核会破坏合法短读。
- **当前状态**：该项记为 libc/LTP `已知差异`，不记为内核失败。若要让 LA64 musl
  这两个 LTP 项通过，需要打补丁或替换 musl runtime 后跑完整 musl 回归，与
  `pathconf()` 一样属于待用户确认的较大改动。

## 2026-08-14 Linux/POSIX Phase 5 musl `pathconf()` libc 阻断（基于 `26852ff`）

- **当前现象**：RV64/LA64 完整初赛日志 `rv-output.txt`/`la-output.txt` 中，musl
  `pathconf02` 的 `ENOTDIR/ENOENT/ENAMETOOLONG/EACCES/ELOOP` 五项都返回常量 8 且
  `errno=0`，只有非法 `name` 的 `EINVAL` 通过；同一镜像的 glibc 六项全部通过。
- **所有权证据**：从当前 RV64/LA64 初赛镜像的 `/musl/lib/libc.so` 直接导出并反汇编，
  两架构的 `pathconf` 都是 8-byte wrapper：将第一个参数改成 `-1` 后跳转
  `fpathconf`；`fpathconf` 只检查 `name` 范围并读常量表，没有路径解析或 syscall。对应镜像
  SHA-256 为 RV64 `95973543db6b84a9a5e70f30da466ce292867aff5b689fb14c88dc9406e378b8`、
  LA64 `1aa79d03cf41e2a80ae4ed43771101c1e67ec8db41c3c20b77792fe6b1b85b50`。
- **内核边界**：RespOS `sys_statfs()` 已通过 `copy_cstr_from_user()` 和 `filename_lookup()` 解析
  path，但当前 musl `pathconf()` 根本不调用它，所以内核无法补回已被 libc 丢弃的
  pathname，也不应为 LTP 增加特判。
- **决策与剩余交付**：该项状态为 `已知差异`。修复需要建立可复现的 musl 构建并替换
  两套镜像的 runtime，然后重跑完整 musl 回归；这会影响所有 musl workload，属于待用户确认
  的较大改动，本轮不修改 libc 或镜像。

## 2026-08-14 Linux/POSIX Phase 5 AF_UNIX `splice` 错误语义（当前工作树）

- **Linux 契约与修复边界**：`scripts/splice_socket_probe_linux.c` 确认未连接 AF_UNIX stream→pipe 写端
  返回 `EINVAL`，未连接 inet 的同一操作仍返回 `ENOTCONN`，目标为 pipe 读端时则由方向错误优先返回
  `EBADF`；connected AF_UNIX→pipe 必须成功传输。RespOS 给 `FileOp` 增加只读
  `validate_splice_read()` 预检，在通用 fd/pipe/方向检查完成后、消费输入前由 AF_UNIX socket 拒绝未连接
  splice，因此不再让普通 `read()` 的 `ENOTCONN` 泄漏成 splice ABI。
- **双架构证据**：宿主 probe 以 `cc -std=c11 -Wall -Wextra -Werror -O2` 通过。RV64/LA64 release、
  初赛 snapshot、4 GiB/2 hart 的 `TASK_A_SPLICE_SOCKET_PROBE=1` 均输出
  `SPLICE_SOCKET ALL PASS`，日志为 `/tmp/respos-{rv,la}-splice-socket-phase5.log`。同配置聚焦
  `splice07` 后，两架构 musl/glibc 均为 `passed 159, failed 0`；扩展到 `splice01`--`splice07` 后四组
  均为 `SUMMARY: 7 passed, 0 failed`，日志为 `/tmp/respos-{rv,la}-splice-cluster-phase5.log`。既有
  `TASK_A_SOCKET_PHASE5_PROBE=1` 双架构 2 hart 回归也全通过，日志为
  `/tmp/respos-{rv,la}-socket-phase5-after-splice.log`。
- **剩余边界**：当前 splice 仍通过有界 kernel buffer 复制，不宣称具备 Linux pipe-buffer page steal/
  zero-copy 性能；本轮只关闭 AF_UNIX 未连接输入的错误映射和已连接传输，不扩展 datagram/seqpacket、
  socket 输出方向或 `SPLICE_F_MOVE/MORE` 的性能含义。

## 2026-08-14 Linux/POSIX Phase 5 AF_UNIX `SO_PEERCRED`（当前工作树）

- **Linux 契约与所有权**：`scripts/socket_peercred_probe_linux.c` 确认 AF_UNIX socketpair 两端持有建链
  进程的 PID/UID/GID；pathname connect 建链后，client 观察 listener 凭据，accept 端观察 connector
  凭据。RespOS 现在在 socketpair、listen/connect 提交点快照 `UnixPeerCredentials` 到连接端点，
  `getsockopt(SO_PEERCRED)` 只读取快照，不保存 live task 引用，也不在查询时用当前调用者身份伪造。
- **双架构证据**：宿主 probe 以 `cc -std=c11 -Wall -Wextra -Werror -O2` 通过。RV64/LA64 release、
  初赛 snapshot、4 GiB/2 hart 的 `TASK_A_SOCKET_PEERCRED_PROBE=1` 均输出
  `SOCKET_PEERCRED ALL PASS`，日志为 `/tmp/respos-{rv,la}-socket-peercred-phase5.log`。同配置聚焦
  `LTP_CASE_FILTER=getsockopt02` 后，musl/glibc 在两架构均为 `SUMMARY: 1 passed, 0 failed`，日志为
  `/tmp/respos-{rv,la}-getsockopt02-ltp.log`；既有 `TASK_A_SOCKET_PHASE5_PROBE=1` 双架构 2 hart 回归
  也全通过，日志为 `/tmp/respos-{rv,la}-socket-phase5-after-peercred.log`。
- **剩余边界**：本轮使用现有 TGID 与 real UID/GID 生成快照；leader exit/non-leader exec 后的稳定
  process identity 仍由独立 Phase 5 task 方案负责。`SCM_CREDENTIALS/SO_PASSCRED`、user namespace 与
  credential change 后的完整 Linux 矩阵未实现，不能由 `SO_PEERCRED` 通过推导。

## 2026-08-14 Linux/POSIX Phase 5 AF_UNIX 地址回报与 `getpeername` 错误优先级（基于 `90bb5a5` 的当前工作树）

- **Linux 契约与修复边界**：`scripts/getpeername_probe_linux.c` 和 LTP `getpeername01` 固定七类错误
  向量，并额外确认未连接 inet socket 即使带非法 `addrlen` 也先返回 `ENOTCONN`。RespOS 现在先解析 fd
  并确认 socket，再按地址族确认连接态；AF_UNIX socketpair 不再被误判为未连接，而是回报长度为
  `sizeof(sa_family_t)` 的未命名 peer 地址。连接成立后，地址 writer 校验 `addrlen` 指针、负值/过大
  长度和实际写入范围，因此 LTP 的 connected socketpair 分支得到 `EINVAL/EFAULT`，且校验失败不产生
  部分写回。
- **named 地址所有权**：扩展后的 Linux probe 确认，pathname 和 abstract listener/client 都绑定时，
  client 的 peer 与 accepted socket 的 local 地址是 listener 地址，`accept()` 输出和 accepted socket
  的 peer 地址是 connector 地址；4-byte buffer 只截断 copy，`addrlen` 仍回报完整长度。abstract 名称
  是含前导 NUL 的任意字节串，不要求 UTF-8，pathname 输出则包含结尾 NUL。修复前 RV64 把应为
  25-byte 的 pathname peer 回报成 2-byte unnamed；当前 `UnixSocket` 在 connect 提交时分别快照 local/
  peer raw address，accept 只转移快照，地址查询不再从全局 listener registry 反推。
- **双架构证据**：宿主 probe 以 `cc -std=c11 -Wall -Wextra -Werror -O2` 通过。RV64/LA64 release、
  初赛 snapshot、4 GiB/2 hart 的 `TASK_A_GETPEERNAME_PROBE=1` 均输出
  `GETPEERNAME ALL PASS`，日志为 `/tmp/respos-{rv,la}-getpeername-named-phase5.log`。同配置聚焦
  `LTP_CASE_FILTER=getpeername01,getsockname01` 后，musl/glibc 在两架构均为
  `SUMMARY: 2 passed, 0 failed`，日志为 `/tmp/respos-{rv,la}-unix-address-ltp-phase5.log`；既有
  `TASK_A_SOCKET_PHASE5_PROBE=1` 双架构 2 hart 回归也全通过，日志为
  `/tmp/respos-{rv,la}-socket-phase5-after-unix-address.log`。
- **剩余边界**：本轮关闭未命名、pathname/abstract AF_UNIX stream 的双方地址和截断长度，不代表
  所有 socket address 契约完成；已关闭/半关闭 inet socket、datagram/seqpacket 的 disconnected 与
  autobind 地址，以及 pathname socket 经 rename/symlink alias 后的 inode-identity connect 仍需单独
  Linux 对照。

## 2026-08-14 Linux/POSIX Phase 5 iperf→iozone 固定顺序 SMP 门禁（当前工作树）

- **可复现入口**：testrunner 新增 `TASK_A_NETWORK_ORDER_PROBE=1`，只按官方初赛脚本顺序运行
  musl iperf、glibc iperf 和 glibc iozone 后关机；默认完整 runner 顺序不变。两组 iperf 各覆盖
  BASIC/PARALLEL/REVERSE 的 UDP/TCP 六种模式，iozone 完整运行镜像脚本而不是只检查首个 writer。
- **通过证据**：基于 `8550a13` 加当前测试入口，RV64 release、4 GiB/2 hart 的 12 个 iperf 项全部
  `success`，其后 iozone 输出完整 group end 并正常 poweroff；LA64 release、4 GiB/1 hart 得到相同结果。
  日志为 `/tmp/respos-rv-network-order-phase5.log` 与
  `/tmp/respos-la-network-order-phase5-smp1.log`。
- **LA64 SMP 反证**：LA64 release、4 GiB/2 hart 连续两轮都在 musl BASIC_UDP success 后停在
  BASIC_TCP 的 `Connecting to host 127.0.0.1`，未打印本地 endpoint；分别观察 139 秒和由外部
  100 秒 watchdog 终止。日志为 `/tmp/respos-la-network-order-phase5.log` 与
  `/tmp/respos-la-network-order-phase5-smp2-r2.log`。该结果不是通过，也不能由 LA64 单核替代。
- **当前边界**：既有 loopback connect probe 在 LA64 2 hart 通过，真实 iperf 固定顺序却稳定失败；
  目前只能确定差异与 SMP/连续 workload 相关，尚不能把它直接归因于 per-hart 时钟、TCP listener
  补位或 scheduler/wakeup。M1 的双架构 iperf 退出门槛保持阻断，根因标记 `待验证`。

## 2026-08-14 Linux/POSIX Phase 5 task 生命周期修复前门禁与方案（当前工作树）

- **修复前门禁**：把既有 `task_phase5_probe` 接入 `TASK_A_TASK_PHASE5_PROBE=1` 自动入口；失败时仍会
  记录 guest status 并安全 poweroff，便于在修复前/后使用同一命令。宿主 Linux probe 三项全通过。
- **双架构当前反证**：RV64/LA64 release、初赛 snapshot、4 GiB/2 hart 都稳定复现四个差异：leader
  原始 `SYS_exit(42)` 错误结束全组并返回 `42 << 8`，worker 未存活；non-leader exec 返回 `EINVAL`
  并由失败路径返回 `111 << 8`。新增稳定身份项还直接确认父进程过早以 `WNOHANG` 收到 status 42；因
  worker 已被杀死，TGID 后续 `kill(pid)`、process-directed signal 和 worker PID/TID 不变量均无法进入。
  最新日志为 `/tmp/respos-{rv,la}-task-phase5-identity-gate.log`，均打印
  `TASK_PHASE5 CURRENT DIFFERENCES CONFIRMED`；这不是通过结果。
- **拟定方案**：新增 [process-identity-phase5-design.md](./process-identity-phase5-design.md)，规定独立
  `ProcessState/ProcessTable`、最后线程 Zombie、wait copyout 后 Reaped，以及 non-leader exec 的
  sibling quiescence 和 TID 接管顺序。方案状态为 `待确认`；未采用保留 exited leader TCB 的 tombstone
  兼容方案，也尚未修改 task 生命周期实现。
- **协商点**：该方案会同时改变 task/process owner、wait、process signal、session 查询与 exec 提交，
  应按设计文档五个可回滚步骤逐项实现并逐项提交。获得确认前不以局部特判消除 expected-fail marker。

## 2026-08-14 Linux/POSIX Phase 5 nonblocking connect、`SO_ERROR` 与失败后重连（当前工作树）

- **Linux 契约**：`scripts/socket_connect_probe_linux.c` 确认 loopback TCP 成功和 refused 两条异步
  connect 路径。首次 nonblocking connect 返回 `EINPROGRESS`；完成由 poll 的 `POLLOUT` 表示，失败还带
  `POLLERR`；`SO_ERROR` 返回正 errno 并原子消费，第二次读取为 0。对同一 fd，消费
  `ECONNREFUSED` 后首次重连返回 `ECONNABORTED` 并完成旧传输状态复位，下一次重连重新返回
  `EINPROGRESS`，随后可以成功连接、accept 并传输数据。
- **状态所有权**：`TcpSocket` 增加显式 `FAILED` 状态和 pending-error 原子槽。协议 poll 把
  `CONNECTING` 提交为 `CONNECTED` 或 `FAILED`，poll/epoll 只观察状态；`getsockopt(SO_ERROR)` 是错误
  的唯一消费点。失败 socket 在错误消费后的首次重连返回 `ECONNABORTED` 并原子替换 smoltcp handle，
  socket 回到可再次 connect 的 CLOSED 状态；后续尝试复用正常异步 connect 提交点。
- **双架构证据**：宿主 probe 以 `cc -std=c11 -Wall -Wextra -Werror -O2` 通过。RV64/LA64 release、
  初赛 snapshot、4 GiB/2 hart 的 `TASK_A_SOCKET_CONNECT_PROBE=1` 均输出
  `SOCKET_CONNECT ALL PASS`；基础日志为 `/tmp/respos-{rv,la}-socket-connect-v3.log`，失败后完整重连
  日志为 `/tmp/respos-{rv,la}-socket-connect-retry.log`。成功项和重连项都完成一字节双端传输；异步
  失败项确认消费 `SO_ERROR` 后旧 `POLLERR` 不再出现；阻塞 refused 项确认 errno 已由 `connect` 同步
  返回，之后 `SO_ERROR` 为 0。扩大 probe 后既有 `socket_phase5_probe` 双架构 2 hart 回归仍通过，日志为
  `/tmp/respos-{rv,la}-socket-phase5-after-retry.log`。
- **剩余边界**：当前网络栈以 loopback/smoltcp 为主，本轮只验证 success/refused；真实 route
  unreachable、SYN timeout、reset 细分 errno 和 iperf 固定顺序回归尚未验证，不能据此宣布 M1 全部
  退出。

## 2026-08-14 Linux/POSIX Phase 5 socket message flags（当前工作树）

- **Linux 契约**：新增 `scripts/socket_flags_probe_linux.c`，用宿主 Linux 固定四条边界：
  `MSG_PEEK` 不消费数据；阻塞流 `MSG_WAITALL` 跨分片等待；timeout/EOF/信号发生在已有部分数据之后时
  返回短读；断链 send 返回 `EPIPE`，且仅 `MSG_NOSIGNAL` 抑制同步 `SIGPIPE`。
- **实现边界**：`sendto/sendmsg/sendmmsg` 统一解析 `MSG_DONTWAIT|MSG_NOSIGNAL` 并在未抑制的 `EPIPE`
  上投递 `SIGPIPE`；`recvfrom/recvmsg/recvmmsg` 接入 `MSG_PEEK|MSG_WAITALL`。AF_UNIX、TCP 的 WAITALL
  使用一次 syscall 的固定绝对 deadline，错误或 EOF 后保留已收部分；AF_UNIX/TCP/UDP 的 PEEK 均不推进
  接收队列。数据报上的 WAITALL 按 Linux 语义不跨数据报聚合。本轮不实现 `MSG_OOB/ERRQUEUE`。
- **验证证据**：宿主 probe 以 `cc -std=c11 -Wall -Wextra -Werror -O2` 通过。RV64/LA64 release、
  初赛 snapshot、4 GiB/2 hart 的 `TASK_A_SOCKET_FLAGS_PROBE=1` 均输出 `SOCKET_FLAGS ALL PASS`，日志为
  `/tmp/respos-{rv,la}-socket-flags-v2.log`。既有 `TASK_A_SOCKET_PHASE5_PROBE=1` 同配置双架构回归全通过，
  日志为 `/tmp/respos-{rv,la}-socket-phase5-after-flags.log`。
- **剩余边界**：LA64 2 hart 的 timeout 时长仍受未归一化 `rdtime.d` 阻断，本专项只证明 timeout 后短读
  返回值正确，不能据此关闭下一节的 SMP 时钟问题。完整初赛/LTP 尚未在本轮改动后复跑。

## 2026-08-14 Linux/POSIX Phase 5 `getsid` 与初始 PID 语义（当前工作树）

- **Linux 契约与实现**：新增 `scripts/session_phase5_probe_linux.c`，确认 `getsid(0)`/按 pid 查询、
  不存在或负 pid 的 `ESRCH`、子进程 `setsid()` 后父进程跨 session 查询，以及 process-group leader
  调用 `setsid()` 的 `EPERM`。RespOS 增加 syscall 156 和对称 `session_phase5_probe`；syscall 只查询
  现有 task/session 状态，不在查询路径引入 controlling-tty 假模型。
- **初始身份修正**：probe 首轮发现 init/testrunner 的 PID、PGID、SID 均为 0。当前将首个用户 TID/PID
  改为 1，使 PID 0 只保留为 syscall 的“当前进程/当前进程组”等选择值；fork 子进程继承合法 session/
  pgrp，之后才能按 Linux 契约成功 `setsid()`。non-leader exec/de-thread 仍是独立 Phase 5 边界，本轮
  没有改变 leader TCB 所有权模型。
- **双架构证据**：宿主 Linux probe 以 `cc -std=c11 -Wall -Wextra -Werror -O2` 全通过。RV64/LA64
  release、初赛 snapshot、4 GiB/2 hart 的 `TASK_A_SESSION_PROBE=1` 均输出
  `SESSION_PHASE5_RESPOS ALL PASS`；日志为 `/tmp/respos-rv-session-phase5-v2.log` 与
  `/tmp/respos-la-session-phase5.log`。PID 起点变化后的 `TASK_A_WAIT4_PROBE=1`、
  `TASK_A_SIGNAL_PHASE5_PROBE=1` 和 `TASK_A_SOCKET_PHASE5_PROBE=1` 也在两架构 2 hart 全通过，
  对应日志为 `/tmp/respos-{rv,la}-pid1-{wait4,signal-phase5}.log` 与
  `/tmp/respos-{rv,la}-socket-phase5-regression.log`。
- **边界**：完整初赛/LTP 尚未复跑；本轮只关闭 `getsid` 与 PID 0 基础模型，不能据此宣称 termios/
  controlling tty/job control 已完成。

## 2026-08-14 Linux/POSIX Phase 5 socket timeout 验收阻断（基于 `75216ff` 加当前 probe）

- **已完成部分**：`scripts/socket_timeout_probe_linux.c` 在宿主通过，覆盖 `SO_RCVTIMEO/SO_SNDTIMEO`
  的 timeval round-trip、零 timeout、`MSG_DONTWAIT`、recv timeout 和 send 满缓冲 timeout。新增对称
  `socket_timeout_probe` 与 `TASK_A_SOCKET_TIMEOUT_PROBE` 自动入口。RV64 release、4 GiB/2 hart 和
  LA64 release、4 GiB/1 hart 均输出三项 PASS 与 `SOCKET_TIMEOUT_RESPOS ALL PASS`；日志为
  `/tmp/respos-rv-socket-timeout.log` 和 `/tmp/respos-la-socket-timeout-smp1.log`。
- **LA64 SMP 反证**：LA64 4 GiB/2 hart 连续两轮都在 50 ms recv timeout 上约 979--981 ms 才返回，
  日志为 `/tmp/respos-la-socket-timeout.log` 与 `/tmp/respos-la-socket-timeout-smp2-r2.log`。临时 affinity
  A/B 中固定 hart0 全通过，固定 hart1 稳定复现约 979 ms；日志为
  `/tmp/respos-la-socket-timeout-hart{0,1}.log`，临时 affinity 代码已撤销。
- **当前判断与边界**：A/B 表明跨 hart 使用未归一化 `rdtime.d` 绝对 deadline 是首要根因候选；当前
  timer-service hart 直接消费其他 hart 生成的 deadline，约 1 秒的时间域偏移会变成同量级晚醒。精确
  raw-counter offset 与归一化实现仍为 `待验证`，在完成 secondary boot 校准/统一单调时间域并重跑
  双架构专项前，socket timeout 状态保持“RV64/LA64 单核已验证，LA64 SMP 未通过”，不得关闭 M1.1。

## 2026-08-14 初赛 ext4 命名 FIFO 修复（当前工作树）

- **根因与修复边界**：`mknod/mkfifo` 请求的 FIFO 在 ext4 lower create 中误走普通文件
  `O_CREAT`，落盘 inode type 因而成为 regular；`stat`、`open_named_fifo()` 以及已有 pipe 的
  `ENXIO/EAGAIN/ESPIPE/EINVAL` 语义都无法到达。当前仅把 FIFO lower create 改为 lwext4
  `ext4_mknod(..., EXT4_DE_FIFO, 0)`，普通文件/目录路径不变。字符/块设备的 `dev` 参数尚未贯穿
  `filename_create/InodeOp::create`，本轮没有借机扩大该接口。
- **双架构专项门禁**：初赛 snapshot、4 GiB/1 hart，以
  `TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=fsync03,lseek02,open06,read03,write04` 分别运行
  `make run-rv-pre` 与 `make run-la-pre`；RV64/LA64 的 musl/glibc 四组均为
  `5 passed, 0 failed, 0 skipped`。日志为 `/tmp/respos-{rv,la}-fifo-ltp.log`。两架构 release
  构建均通过，日志中正常执行到 testrunner poweroff。
- **影响范围**：这五项是 2026-08-14 完整初赛四环境共同失败，因此预计下一次完整复跑四组各减少
  5 个失败；专项结果不能代替完整 summary，最终增量仍以用户下一轮 `rv-output.txt`/
  `la-output.txt` 为准。

## 2026-08-14 初赛 CPU clock 簇实现（当前工作树）

- **实现边界**：scheduler 在每次真实 task `__switch` 前后记录硬件计数器；thread clock 保存本线程
  累计运行时间，process clock 由线程组共享并以 per-hart running slot 汇总可并行运行的线程。
  `CLOCK_PROCESS_CPUTIME_ID`/`CLOCK_THREAD_CPUTIME_ID` 已接入 `clock_gettime/getres` 和 POSIX
  `timer_create/settime/gettime/delete`。CPU timer 保存脱离 TCB/address space 的 clock handle，线程
  退出后时钟停止但 timer 不会延长完整 task 生命周期。`times/getrusage` 仍未拆分 user/system，
  只是 total 已从 wall-clock 近似切换为 scheduler runtime。
- **LTP 专项**：初赛 snapshot、4 GiB/1 hart，以 `clock_getres01` 作为动态装载预热，再筛选
  `clock_gettime01,clock_gettime02,timer_delete01,timer_settime01,timer_settime02`。RV64/LA64 的
  musl/glibc 五个目标均返回 0；LA64 连同预热项为两组 `6 passed, 0 failed`。日志为
  `/tmp/respos-{rv,la}-cpu-clock-cluster.log`。RV64 glibc 的冷启动预热项有独立 loader `SIGBUS`
  边界，见 `pitfalls.md`，不属于 CPU clock syscall 失败。
- **SMP probe**：`TASK_A_CLOCK_PROBE=1 PRE_SMP=2` 在 RV64/LA64 各运行 20 轮，全部打印
  `process/thread CPU clocks PASS`、`process aggregation PASS` 与 `ALL PASS`。probe 创建同一进程
  worker thread，并验证 process 增量至少覆盖 main/worker 两条 thread clock 增量。日志为
  `/tmp/respos-{rv,la}-cpu-clock-probe-smp2.log`。
- **完整复跑**：基于 `bf780542774b9cc9428f2935446609c17035c97a` 加当前 CPU clock 工作树，
  2026-08-14 03:00 UTC 的 `rv-output.txt` 与 02:57 UTC 的 `la-output.txt` 均正常结束全部 19 组并
  poweroff。RV64 LTP musl/glibc 为 `634/37/24`、`643/31/21`，LA64 为 `638/34/23`、
  `646/29/20`（passed/failed/skipped，均选择 695 项）；CPU clock/timer 六个 case 在四环境均退出 0。
  未见 kernel panic、用户 fault 或 OOM，末段 health 为 `tasks=2 ready=0 blocked=1`。RV64 冷启动
  glibc loader EIO 仍应作为 MM/ext4 独立任务推进。

## 2026-08-13 初赛亚 10 ms 精确 deadline 修复（当前工作树）

- **根因**：`poll02`、`pselect01`/`pselect01_64`、`select02`、`epoll_wait02`、`futex_wait05`、
  `nanosleep01`、`clock_nanosleep02` 在 RV64/LA64、musl/glibc 都失败。它们覆盖 1/2/5/10/25/100/
  1000 ms timeout；旧实现只依赖 100 Hz 周期扫描，1 ms 请求实测约 10.2 ms，属于统一的 tick 量化，
  不是八个独立 syscall 错误。
- **实现边界**：保留 100 Hz 调度 tick；nanosleep、poll/pselect/epoll task timeout 与 futex waiter
  额外发布微秒级最早 deadline。timer-service hart 直接缩短硬件 compare，其他 hart 只经 IPI 通知其
  读取原子提示。timer 扫描先清空提示，再从权威 waiter 注册表重建下一 deadline；过期/撤销提示最多
  造成一次额外中断，不决定 timeout 语义。
- **QEMU 注入补偿**：RV64/LA64 QEMU 的 one-shot timer 注入有数百微秒延迟。当前提前 800 us 设置
  compare，并只在这段有界窗口内等待软件权威 deadline，避免提前唤醒和第二次注入延迟。该常量是
  当前 QEMU 正确性门禁的一部分，不是实机性能结论。
- **双架构专项门禁**：初赛 snapshot、4 GiB/1 hart，使用
  `TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=poll02,pselect01,pselect01_64,select02,epoll_wait02,futex_wait05,nanosleep01,clock_nanosleep02`
  分别运行 `make run-rv-pre` 与 `make run-la-pre`；两架构 musl/glibc 均为
  `8 passed, 0 failed, 0 skipped`。日志为 `/tmp/respos-rv-exact-deadline-ltp-v2.log` 与
  `/tmp/respos-la-exact-deadline-ltp.log`。
- **SMP/时钟门禁**：RV64/LA64 均以 `PRE_SMP=2` 验证 `nanosleep01,futex_wait05`，两组 libc 均
  `2 passed, 0 failed`；日志为 `/tmp/respos-{rv,la}-exact-deadline-smp2.log`。两架构
  `TASK_A_CLOCK_PROBE=1` 各 20 轮全部 `ALL PASS`，日志为
  `/tmp/respos-{rv,la}-clock-probe-exact-deadline.log`。Rust 1.86 release 双架构构建通过。
- **剩余门禁**：本专项消除了上一轮 40 个四环境共同失败中的 8 个；完整初赛尚未在精确 deadline
  改动后重新跑完，因此本节不能替代下一次完整 summary，也不能据专项结果推算当前总分。

## 2026-08-13 初赛完整复跑与 epoll/memfd 专项修复（当前工作树）

- **完整初赛复跑**：基于 `adf874b8b8f6c2e06594a0b90b43300d97116c43` 加当前未提交工作树，
  2026-08-13 20:28 UTC 更新的 `rv-output.txt` 与 `la-output.txt` 均正常关闭全部 19 个测试组，未见
  kernel panic/page fault，末段 health 仍为 `tasks=2 ready=0 blocked=1`。RV64 的 LTP musl/glibc
  汇总分别为 `626/45/24`、`635/39/21`，LA64 分别为 `626/46/23`、`634/41/20`（顺序为
  passed/failed/skipped，均选择 695 个 case）。相对修复前基线分别改善 `+3/-3/0`、`+2/-2/0`、
  `+3/-3/0`、`+4/-4/0`；最后一组多出的一项改善不能仅凭本轮日志归因给三个目标修复。
- **筛选边界**：`user/oscomp_ltp_list.txt` 当前注释了 `linkat02`、`rename11`、`waitpid11`、
  `shmctl01` 四个已确认会长时间运行或卡死的 case，因此 695 是当前筛选集，不是未删减的 LTP 集合。
- **epoll 修复**：`epoll_ctl()` 接受 `EPOLLPRI` 作为合法监听位；就绪扫描暂不凭空合成异常带外事件。
  此前合法事件组合被事件掩码误判为 `EOPNOTSUPP`，导致 `epoll_ctl03` 失败。
- **memfd 修复**：`memfd_create()` 先拒绝未知 flag，再把已识别但尚不支持的 `MFD_HUGETLB` 返回
  `EOPNOTSUPP`，从而恢复 Linux 的 `EINVAL`/`EOPNOTSUPP` 错误优先级；内存型 `SpecialFd` 实现
  `fallocate(mode=0)` 的扩容语义，并遵守 `F_SEAL_GROW`。普通不支持预分配的后端仍返回
  `EOPNOTSUPP`，没有伪造成功。
- **双架构专项门禁**：使用
  `TASK_A_LTP_ONLY=1 LTP_CASE_FILTER=epoll_ctl03,memfd_create01,memfd_create02` 分别运行
  `make run-rv-pre` 与 `make run-la-pre`，RV64/LA64 的 musl/glibc 均为
  `3 passed, 0 failed, 0 skipped`。其中 `memfd_create01` 每组 157 项、`epoll_ctl03` 每组 256 项均
  完整通过；日志为 `/tmp/respos-{rv,la}-epoll-memfd-ltp.log`。完整复跑再次确认三个 case 在四组
  环境中都返回 0。Rust 1.86 release 双架构构建通过。
- **剩余失败结构**：解析四组 case 后有 40 个失败/损坏 case 为 RV64/LA64 与 musl/glibc 共同项，
  说明下一阶段应优先处理跨架构共性语义，而不是先追逐单环境波动。四组失败集合并集为 54 个；其余
  差异项需用筛选回归确认可重复性后再归因。
- **后续进展**：上述 8 个亚 10 ms deadline 共性失败已由同日精确 deadline 子轮修复并通过双架构
  单核/SMP 专项；完整初赛仍需复跑后才能更新总 summary。

## 2026-08-13 初赛 `wait4` SA_RESTART 与 ext4 `/tmp` 修复（当前工作树）

- **完整日志归因**：用户复跑的 `rv-output.txt`/`la-output.txt` 都完整结束了双架构两组 iozone，随后
  分别在 `ltp-musl` 第 602/462 个 case 处被外部终止；两份日志均无 kernel panic/page fault。已完成
  case 中 `waitpid(...)=EINTR` 分别出现 259/211 次，且 task health 从基线 2 增长到 282/256，说明
  LTP newlib 父进程被意外中断后留下大量后代，是当前最大框架级放大器。
- **SA_RESTART 修复**：RV64/LA64 syscall trap 在 `wait4` 返回 `EINTR` 时保留原始 arg0；实际送达的
  用户 handler 若带 `SA_RESTART`，signal frame 改为保存 syscall 指令 PC 和原始 arg0，handler 经
  `sigreturn` 后重新执行 `wait4`。无 `SA_RESTART` 时仍向用户返回 `EINTR`。当前只把 Linux 可重启的
  `wait4/waitpid` 纳入该机制，其他阻塞 syscall 仍需逐项审计后扩展。
- **`/tmp` 后端修复**：当前官方初赛压缩基线展开后的根镜像本身就把 `/tmp` 链到 `/dev/shm`，
  libcbench 的准备逻辑只是重建同一链接。shm owner 修复后临时文件可创建，但其轻量 inode 仍不支持
  LTP 需要的完整 timestamp/symlink/hardlink/FIFO/xattr 语义。testrunner 现在保留 libcbench 的 shm
  环境，之后把 `/tmp` 重建为根盘上的 `01777` ext4 目录；LTP-only 入口也执行相同准备。
- **双架构验证**：Rust 1.86 release 双架构构建通过。`TASK_A_WAIT4_PROBE=1` 在 RV64/LA64 均验证
  无 restart handler 返回 `EINTR`、带 `SA_RESTART` handler 执行后自动重启并回收子进程，日志为
  `/tmp/respos-{rv,la}-wait4-restart-probe.log`。初赛 snapshot、4 GiB/1 hart 筛选
  `confstr01,openat02,lstat01,statfs02,fstat02,dup05,flistxattr02`，两架构 musl/glibc 均为
  `7 passed, 0 failed`，结束时 tasks 保持 2；日志为 `/tmp/respos-{rv,la}-restart-tmp-ltp.log`。
- **剩余门禁**：完整双架构初赛尚需复跑；该专项不证明其他 692 个 LTP case 或 libctest 全部通过。
  LA 专项宿主 QEMU 为 `CLS=TS/NI=0`，结果只用于正确性，不用于性能结论。

## 2026-08-13 初赛 RV64 内核栈边界与 `/dev/shm`/`/tmp` 创建修复（当前工作树）

- **触发与根因**：`adf874b` 上的完整 RV64 初赛在 `ltp-glibc/posix_fadvise03_64` 创建下一任务时
  发生 kernel StorePageFault。坏地址 `0xffffffffbfffede0` 精确等于 kernel-stack slot 16384 的
  top 减去 RV64 `TrapContext` 大小 `0x220`。用户页表创建时按值复制内核高半区根 PTE；动态内核栈
  第一次跨过 1 GiB 根页表边界后，`KERNEL_SPACE` 新建的根 PTE 不会出现在已有用户 root 中。
- **内核映射修复**：RV64/LA64 在首个用户地址空间创建前，为尚为空的内核高半区根 PTE 建立共享的
  下级页表分支；物理栈页和更低层页表仍按需分配。这样用户 root 的根拓扑保持不变，后续内核栈映射
  只修改已共享的下级表。临时把 allocator 起始 slot 设为 16383 的 RV64 release 诊断中，initproc
  后首个 fork 强制跨到 slot 16384，musl/glibc `exit01` 均通过并正常关机；临时改动已撤销，日志为
  `/tmp/respos-rv-kstack-slot16384.log`。
- **shm 元数据修复**：`ShmFileInode` 与 `ShmDirInode` 现在保存并报告 uid/gid，并实现
  `set_owner()`/`set_owner_and_mode()`。此前 namei 的 create 协议在 lower create 后提交 mode/owner，
  shm inode 缺少 owner setter，默认返回 `EINVAL` 并回滚文件，因而所有 LTP newlib 框架文件都在
  `/dev/shm/ltp_*` 创建阶段 TBROK。
- **`/tmp` 归因与 libctest**：官方初赛根镜像本身已把 `/tmp` 链到 `/dev/shm`，testrunner 在
  libcbench 前会重建同一链接；故此前 `mkstemp()`、`tmpfile()` 和 `mkdtemp(/tmp/LTP_*)` 的 `EINVAL`
  与上项是同一故障链，不是 ext4 `/tmp` 的独立问题。RV64 临时 libctest-only release 运行中 static/dynamic 均无
  `FAIL`，原先的 `fdopen/fscanf/fwscanf/pthread_cancel_points/ungetc/utime/fflush_exit/`
  `ftello_unflushed_append/lseek_large` 创建级联全部消失；wrapper 仍打印既有的 raw status `256`，
  日志为 `/tmp/respos-rv-libctest-only-after-shm.log`。
- **LTP 专项**：双架构 release、4 GiB/1 hart、初赛 snapshot 上筛选 `confstr01,openat02`；两架构
  musl/glibc 的 `openat02` 六项均通过，RV64 musl `confstr01` 32 项通过。其余 `confstr01` 运行已越过
  `/dev/shm` open，但随后暴露 `waitpid(...)=EINTR` 的独立 LTP blocker，不能计作 shm 修复失败或整体
  LTP 通过。日志为 `/tmp/respos-{rv,la}-shm-ltp.log`；完整初赛仍待用户复跑。

## 2026-08-14 P0：ext4 readdir 复用目录项类型（双架构专项通过，完整 final 待验证）

- **实现与语义边界**：`Ext4Inode::readdir()` 现在直接使用 lwext4 已解析的 ext4 directory entry
  file type 生成 `d_type`；只有后端返回 `EXT4_DE_UNKNOWN` 时才回退到原有 child pathname
  `ext4_mode_get`。`.`/`..` 的 UNKNOWN 行为保持原样，所有 lwext4 C 入口仍持同一
  `EXT4_OP_LOCK`，没有目录内容缓存、BuildStorm 路径特判或锁域拆分。该回退覆盖未启用 ext4
  `FILETYPE` feature 的合法文件系统，不能为了命中率删除。
- **可观测性与专项门禁**：`perf_counters` 新增 `ext4_readdir_dirent_type_known/unknown`；
  `fs_namespace_probe` 通过真实 `getdents64` 解析验证目录、普通文件和相对符号链接分别返回
  `DT_DIR/DT_REG/DT_LNK`，输出 `FS_NAMESPACE_DIRENT_TYPE_PASS`。RV64 无 feature 还通过 namespace
  race 1200 轮、metadata、Phase4、BuildStorm file/private-map、shared-MM 100 轮和 frame-reclaim；
  LA64 通过 d_type、namespace、metadata 与 BuildStorm file。日志
  `/tmp/respos-rv-readdir-p0-probes.log`（SHA-256 `dbabb92d...87b0`）和
  `/tmp/respos-la-readdir-p0-probes.log`（`dc94e71a...0078`）。Rust 1.86 的 RV64/LA64、
  `perf_counters`/无 feature release 组合均构建通过；按线上准确入口 `make all` 收尾重建的正式无
  feature `kernel-rv/kernel-la` SHA-256 分别为 `759b418b...53a3`/`ccd368be...1424`。
- **同工作量 RV A/B**：同一 4 GiB/8 hart、相同 root/x1 snapshot 对 `/work/tgoskits` 执行 BusyBox
  `find -type f`。两轮都因镜像中两个不可表示路径报告 status 1，但 FS/block 操作计数完全相同，故可
  比较共同完成的目录遍历。旧内核到候选的 wall 为 `36.75 -> 35.77s`（`-2.67%`），lower readdir
  calls `47294 -> 31672`（`-33.03%`）、readdir ticks `60875932 -> 11461818`（`-81.17%`）、
  ext4 readdir hold `60807713 -> 11386903`（`-81.27%`）；heap alloc calls/bytes 分别下降
  `5.03%/9.38%`。候选返回 `22042` 个 known、`0` 个 unknown；lower calls 精确等于
  `known + 3 * readdir_calls`，剩余三次是每轮 open/end/close。日志
  `/tmp/respos-rv-readdir-p0-micro-{baseline,candidate}.log`，SHA-256 分别为
  `ded8cd5f...97cd`/`62ca53a4...6cb`。
- **BuildStorm 短窗口**：RV64 16 GiB/8 hart 的单次 readdir 平均从约 `3.833ms` 降至 `0.199ms`
  （`-94.8%`），LA64 12 GiB/12 hart 从约 `1.758ms` 降至 `0.277ms`（`-84.2%`）；候选分别记录
  `9501/9509` 个 known 和 `0/0` 个 unknown。日志
  `/tmp/respos-rv-readdir-p0-120.log`（SHA-256 `c1a4ab0c...ad33`）和
  `/tmp/respos-la-readdir-p0-12h-120.log`（`264ce543...2b3`）。两组候选在 120 秒内的编译进度都少于
  各自历史基线，宿主状态没有形成相邻、同进度 A/B，因此这些归一化 ticks 只能证明 P0 热点消除，
  不能宣称完整 BuildStorm wall 加速。完整无 feature final 与平台时限结果仍标记 `待验证`。

## 2026-08-14 RV64 final SIGBUS：VirtIO 跨非连续物理页 DMA（已修复并完整通过）

- **平台症状**：用户提供的正式 RV64 输出在 CAgent 后读取 BuildStorm/Rust 工具链时出现文件内容损坏，
  `rustc` 最终以 signal 7/SIGBUS 退出。相同 16 GiB/8 hart、双 virtio-mmio 磁盘和 Rust 1.86 构建在
  本地可稳定复现 `/glibc/buildstorm_testcode.sh: cannot execute binary file`；这不是单纯超时。
- **已排除层次**：脚本 inode、extent 到磁盘 sector 的解析与 backing raw image 内容一致；VirtIO used
  ring 返回 `status=OK`，失败后 overlay 的目标 sector 仍正确。临时强制单 sector I/O、修改 ext4 路径
  缓存、增加 TLB shootdown 均不能修复，相关试验代码均已撤销。
- **根因证据**：失败请求的 `BlkReq` 位于虚拟地址页尾 `...dff8`，描述符长度 16 B。前 8 B
  `type/reserved` 合法地为 0，后 8 B `sector=6320152` 位于下一虚拟页；HAL 旧实现只翻译首地址并假设
  整段物理连续，设备因而从首物理页后的相邻物理页读到 0，把请求当成 sector 0。随后返回的 boot/code
  数据被上层解释为脚本或动态库，解释了 `cannot execute binary file`、随机命令和最终 SIGBUS。该错误
  由栈布局决定，Rust 版本、LTO 或 `fault_trace` 只会改变是否跨页，不是根因。
- **修复边界**：`VirtIoHalImpl::share()` 现在验证整个虚拟范围的物理连续性。单页、direct map 和真实
  连续范围继续零拷贝；只有跨到非相邻物理页时才分配 direct-map heap bounce buffer，按
  `BufferDirection` 拷入，并在 `unshare()` 中对设备输出拷回。已释放的同尺寸 bounce 最多缓存 64 个，
  且空闲缓存总计不超过 1 MiB，避免固定栈布局下每个块请求重复分配或由大请求长期占住内核堆；实现
  覆盖请求头、状态、数据和 indirect descriptor，不依赖对 `BlkReq` 做特殊对齐。
- **针对性验证**：`nightly-2025-01-18`、`fault_trace`、官方 16 GiB/8 hart 双盘参数和 `-snapshot`
  下，CAgent 10/10 pass，BuildStorm 脚本可执行，toolchain/minibuild 通过，untimed prebuild 31.13 s，
  timed `arceos-helloworld` 已进入正式 release cargo build；确认越过原稳定故障点后手工终止，不能记为
  完整 BuildStorm 通过。日志 `/tmp/respos-rv-virtio-bounce.log`。另一轮临时 trace 直接记录 CAgent 与
  BuildStorm 的 16 B 请求在 `...dff8` 进入 bounce 后继续通过前置门禁，日志
  `/tmp/respos-rv-virtio-bounce-diag.log`；该临时打印已撤销。无效 shootdown 对照日志
  `/tmp/respos-rv-kstack-shootdown.log` 仍在同一点失败。RV64/LA64 无 feature release 均用同一
  Rust 1.86 工具链构建通过。移除诊断后的正式无 feature `kernel-rv`（SHA-256
  `196ce666...345b36`）再次以相同平台参数运行，CAgent 10/10、BuildStorm toolchain/minibuild 通过后
  在 untimed prebuild 手工终止；日志 `/tmp/respos-rv-virtio-bounce-final-smoke.log`。
- **完整 final 验证**：增加 64 项/1 MiB 空闲缓存双预算后的正式无 feature `kernel-rv` SHA-256 为
  `4606ed12...28b468`，对应 LA 构建产物为 `64dd94aa...70e3f6`；双架构 Rust 1.86 release 均通过。
  该 RV 候选按官方 16 GiB/8 hart 双盘参数和 `-snapshot` 完整运行：CAgent 10/10、BuildStorm
  toolchain/minibuild 通过，Cargo release `34m53s`，axbuild `2114.04s`，最终
  `ok=true cores=8 bytes=1681000`，脚本 exit 0 并正常关机。日志
  `/tmp/respos-rv-virtio-bounce-full.log`，SHA-256 `0a7c39f0...827a3f`；按日志时间戳整轮约
  `2200.53s`。全程没有 SIGBUS、脚本损坏、I/O error、kernel panic 或 OOM。
- **性能边界**：2026-08-11 同为 RV 16 GiB/8 hart、无 feature 的历史 axbuild 为 `1310.01s`，本轮
  慢 `804.03s`（约 61.38%）；用户近期另一轮体感约 1800s，本轮也慢约 314s。两者都不是同一当前
  源码/相邻宿主状态的 A/B，因此不能把差值直接归因于修复。随后用当前源码、16 GiB/8 hart、
  `perf_counters` 和 120 秒 BuildStorm timeout 做短窗口：抓取时累计 30,000 次块读、57,706 次块写，
  只有 35 个 16 B 范围进入 bounce，共 560 B；首次分配 1 次、缓存命中 34 次、同时活跃峰值 1。
  已计时的 bounce 分配/查找/复制/回收慢路径 `share_ticks + unshare_ticks = 4,966`，在
  `clock_hz=10,000,000` 下约 `0.497 ms`，只占该窗口块读写累计 ticks 的约 `0.0048%`。诊断 shell
  拒绝了测试前的 procfs 重定向清零，因此这些数字还包含启动 I/O 和 timeout 后约 5 秒抓取尾部；这会
  高估而非低估 bounce 次数/慢路径成本。该计时不包含仍走零拷贝路径的连续性判断以及 direct
  `unshare` 空表查询，故只能排除“实际 bounce 分配/复制导致数百秒回归”，不能单独证明全部 HAL
  入口的精确墙钟差。计数内核 SHA-256 `d4924646...c26144`，日志
  `/tmp/respos-rv-virtio-bounce-120.log`，SHA-256 `6db05abf...001919`。完整运行偏慢更符合宿主/整机
  吞吐波动，但仍需当前源码无 perf 的相邻 A/B 才能定量归因。不得为追回时间恢复首地址直译，也不得
  仅靠关闭 LTO 再次掩盖跨页问题。

## 2026-08-14 LA 4 KiB Global kernel mapping（稳定宿主复测为正，仍默认关闭）

- **实验与保守边界**：实验性 `la_global_kernel` feature 只在 final kernel root 第一次激活前遍历共享
  kernel half。仅当同一 LoongArch TLB pair 的偶/奇 4 KiB leaf 都有效时才同时设置两个 G 位；单边
  leaf 明确清 G。高端 RAM 的 2 MiB huge direct-map leaf 和初始化后新增的 kernel-stack leaf 仍为
  ASID-scoped。普通 PTE writer、op=4 本地/远端同步失效、residency mask、retired-frame completion
  均未绕过；新用户 root 仍复制同一 kernel root entry，因而引用相同的共享下级页表。
- **被否决的 huge-global 编码**：实验曾按未经实机验证的 `PMD_HGLOBAL=bit 12` 标记 2 MiB leaf，
  启动后首次高端 RAM 写立即触发 `PageInvalidStore`，ERA `0xffffffc0002f7094`、badaddr
  `0xffffffc090400e70`。日志 `/tmp/respos-la-global-probe.log`，SHA-256 `c633dccd...6000`。该编码已
  删除，`map_huge_2m()` 现在显式拒绝 `PTEFlags::GLOBAL`；不得把 4 KiB pair 规则外推到 huge leaf。
- **正确性门禁**：LA `la_global_kernel + perf_counters` 和无计数 release 均构建通过。4 GiB/12 hart、
  独立 qcow2 overlay 下连续两次 `smp_shared_mm_probe` 各 100 轮以及 `smp_phase3_probe` 30 轮通过，
  约 108 万 syscall、850 次真实 remote shootdown、1377 个 retired batch，无 panic/内存破坏；日志
  `/tmp/respos-la-global4k-probe.log`，SHA-256 `61be9c...c8d1`。随后 RV64 `perf_counters` 已用
  `nightly-2025-01-18` 成功构建并完成 16 GiB/8 hart 的 120 秒 BuildStorm 计数窗口；RV64/LA64
  无 feature release 也再次构建通过。双架构构建门禁已经补齐。
- **计数与短 A/B**：12 hart、120 秒计数窗口的本地 op=4 ticks 从 `259911669` 降到
  `246472973`（约 `-5.2%`），工作进度仍为 23 个 marker；ext4/heap 波动反向放大，因此不把这一个
  计数样本解释为 wall 收益。相邻无 perf 冷 overlay 的 off/on 120 秒窗口都到 23 个 marker，两个
  dev 阶段由 `13.58/5.02s` 降到 `10.29/4.09s`（约 `-18%--24%`）。日志为
  `/tmp/respos-la-global4k-12h-120.log`、`/tmp/respos-la-global-ab-{off,on}-120.log`，后两者 SHA-256
  `dbecf5...4015`/`78e07c...c5b`。on 轮一次 Cargo last-use `disk I/O error` 只影响 cache 记账并继续
  编译，不能把该警告当作内核通过项或性能收益来源。
- **完整 final 结果**：QEMU 10.0.2、12 GiB/12 hart、无 perf，kernel SHA-256
  `5d09b91f...62558`，公开 LA root 与临时 final x1 均通过独立 qcow2 overlay 挂载。CAgent 脚本退出
  0；并发串口把两个逐项输出交织，故不从该日志宣称精确 pass 数。BuildStorm toolchain/minibuild
  通过，正式 release 为 `25m50s`，axbuild `1560.36s`，产物 `1714568` B、`ok=true cores=12`，脚本
  退出 0 并正常关机。日志 `/tmp/respos-la-global4k-full-final.log`，SHA-256
  `bb0106e4...0708`；按日志创建/结束时间计算整轮约 `1603.89s`。相对相同 12 GiB/12 hart、无 perf
  的 E1 完整结果 `1743.70s`，axbuild 减少 `183.34s`（约 `10.51%`），超过原定 5% 保留门槛；但
  两轮并非同一时刻的完整相邻 A/B，精确加速比仍会受宿主波动影响。
- **相邻完整 off 反证**：随后以同一当前源码、Rust 1.86、root/x1 backing、12 GiB/12 hart 和无 perf
  构建关闭 feature 的内核（SHA-256 `a3475d61...5b2301`），新 qcow2 overlay 下仍完整通过；CAgent 与
  BuildStorm 脚本均退出 0，产物相同。off 的 release/axbuild 反而只有 `23m19s`/`1410.58s`，比上述
  on 快 `149.78s`（以 off 为基准约 `10.62%`）。日志
  `/tmp/respos-la-global4k-ab-off-full-nohostfwd.log`，SHA-256 `fa889962...cc408`，整轮约 `1452.46s`。
  本轮只因 sandbox 禁止绑定 UDP host port 而移除了不被客体脚本使用的 hostfwd，virtio-net/NAT 与
  guest 测试路径未变。更关键的未控变量是 host backing-file page cache：先运行者可能承担冷读，后运行者
  复用热缓存，因此现有 on/off 顺序不能给出精确因果比例。
- **第一组 on--off--on 的历史判定**：紧邻 on2 使用新 Rust 1.86 on 产物（SHA-256
  `0b4d4daa...c398`）和同样无
  hostfwd 配置，CAgent 10 项 pass、BuildStorm `ok=true`，但 release/axbuild 为
  `27m04s`/`1634.28s`，比中间 off 慢 `223.70s`（以 off 为基准约 `15.86%`）；整轮约
  `1673.65s`。日志 `/tmp/respos-la-global4k-ab-on2-full-nohostfwd.log`，SHA-256
  `1f36e356...13c4d`。on--off--on 为 `1560.36/1410.58/1634.28s`，两次 on 均未胜过 off；样本量和
  host cache/负载不足以证明 feature 必然造成固定幅度退化，当时据此判为性能 No-Go 并回退实验；下述
  更稳定宿主复测已重新打开候选，但不把旧数字删除或改写。
- **更稳定宿主的 off→on 复测**：固定 Rust 1.86、QEMU 10.0.2、12 GiB/12 hart、同一 root/x1
  backing、独立 qcow2 overlay、无 perf/hostfwd，并以 5 秒间隔记录 QEMU/宿主时间线。R1/off 内核
  SHA-256 `f0c790b5...60fe1c`，CAgent 10/10、BuildStorm `ok=true`，release/axbuild 为
  `25m20s`/`1530.37s`，整轮约 `1571.67s`；日志 `/tmp/respos-la-clean-r1-off.log`，SHA-256
  `4cbb0b73...6e8b7`。R2/on 内核 SHA-256 `ce372edc...4cd041`，同样完整通过，release/axbuild 为
  `23m29s`/`1418.31s`，整轮约 `1455.74s`；日志 `/tmp/respos-la-clean-r2-on.log`，SHA-256
  `21afaae2...34b6`。on 的 axbuild 减少 `112.06s`（约 `7.32%`），整轮减少约 `115.93s`（约
  `7.38%`），超过候选门槛。
- **复测资源边界与当前结论**：R1/off 时间线平均/峰值 QEMU CPU 为 `702.1%/1190.5%`，最低可用内存
  约 `7.42 GiB`，swap-in/out 分别约 `64/217 MiB`，major fault 增量 `13111`；R2/on 为
  `657.8%/1191.1%`、最低约 `7.01 GiB`、swap-in/out 约 `25/0.5 MiB`、major fault `7939`。第二轮
  宿主换页明显更轻，故不能把全部 `7.32%` 归因于 Global；用户决定不跑包围它的 R3/off，本轮只有一对
  样本。当前判定从 No-Go 调整为 **正向但待复现**：恢复 default-off 的 `la_global_kernel` feature、
  4 KiB paired-leaf 遍历和启用点，仍不进入默认提交路径。通用 TLB 计数与 `map_huge_2m()` 对未经验证
  Global 编码的拒绝继续保留；feature 开启时也不跳过 op=4，不改变 shootdown/frame completion，
  huge/runtime mapping 不纳入 Global 域。

## 2026-08-14 LA `1/3/6/12` hart BuildStorm 缩放诊断（提交 `277ceaa8`）

- **口径与证据**：LA release kernel 只开启 `perf_counters`，kernel SHA-256
  `e3963e18...16b78`；公开决赛 root image SHA-256 `450682fd...e9ca`，diagnostic x1
  SHA-256 `4328ad97...d1ea`。QEMU 10.0.2、`-snapshot -m 12G`，分别使用
  `-smp 1/3/6/12` 与 `CARGO_BUILD_JOBS=1/3/6/12`，每轮从冷 snapshot reset 后运行 120 秒。
  online mask 依次为 `0x1/0x7/0x3f/0xfff`。串口日志为
  `/tmp/respos-la-scale-{1h-j1,3h-j3,6h-j6,12h-j12}-120.log`，SHA-256 依次为
  `9a9e97ab...bfdc`、`4e0a67ea...6590`、`1d2b41c5...401d`、`178e9702...2848`；宿主时间线位于
  对应 `-timeline` 目录（3 hart 的有效目录后缀为 `-run-timeline`）。1 hart 的宿主时间线包含首次命令
  包装失败后的 shell 等待，故该点只使用 reset 后 guest 计数，不把整段 host wall/CPU 与其余点比较。
- **同进度边界**：1 hart 只输出 2 个 `Compiling` marker，停在 `core`，不能和其余点直接比较；3/6/12
  hart 都输出 23 个 marker，最后均为 `ax-posix-api`。后三点 heap alloc 约
  `16.425/16.437/16.433 M`、ext4 acquisitions `85187/85470/85526`、PageCache miss
  `14541/14679/14633`、block read requests `28597/28736/28694`，工作量高度接近，允许比较累计等待和
  并发结构，但 marker 不能显示正在编译 crate 的内部进度，仍不能外推完整 BuildStorm 加速比。
- **利用率与调度判读**：由 `task_running_ticks/(running+idle)` 折算，3/6/12 hart 平均实际运行约
  `1.25/1.34/1.43` 个 hart；`running_harts_1` 占 concurrency samples 约
  `81.5%/83.6%/82.7%`，`scheduler_ready_0` 占 `98.3%/98.4%/99.7%`。对应宿主 QEMU 全程平均约
  `127.7%/137.3%/147.2%` CPU。额外 hart 没有形成持续 runnable backlog，因此当前数据否决优先重构
  runqueue/wakeup；低利用率主要表现为 Cargo/crate 依赖链或阻塞阶段只有约一个任务可运行，而不是 ready
  任务未被调度。
- **锁、I/O 与 MM 归因**：3/6/12 hart 的 ext4 wait 为 `1.672/2.755/3.118` 累计 CPU 秒，hold 为
  `19.714/18.767/18.147` 秒，最大单次 wait 为 `201.0/240.5/221.7 ms`；wait 随 hart 增长且存在长尾，
  允许进入 E2 C 层共享状态审计，但约 18 秒串行 lower 工作和约 3 秒累计等待不足以解释约 120 秒内的
  大量 idle，不能直接据此拆 `EXT4_OP_LOCK`。heap lock wait 为 `2.529/6.761/9.530` 秒，说明高 hart
  contention 存在，但 A1 完整 A/B 已 No-Go，不能重复扩大 magazine。frame alloc lock wait 仍远低于
  clear/core，PageCache 三点 eviction 约 `77.4--77.8 K`、raced pages 仅 `254/1150/761`，block 工作量
  也近似恒定，均不支持优先重写 frame allocator 或继续扩大 E1 缓存策略。
- **TLB 后续测量入口**：同阶段 3/6/12 hart 的 local sfence 约 `614/617/614 K`，remote-empty request 约
  `603/603/598 K`，而真实 remote RFENCE 仅 `10.5/13.8/16.1 K`。remote wait 为
  `0.806/1.925/2.549` 秒。为补齐原口径缺失，本轮新增静态门控的 local invalidation timing 以及
  fresh-map/COW/retired-frame 分类；unmap、COW、mprotect、ASID residency 和 frame completion 行为
  尚未改变。
- **local INVTLB 结果**：新 kernel SHA-256 `90aeb4ca...4bc3`，使用两个显式 `/tmp` qcow2 overlay
  保护原始 root/x1，在 `-m 12G -smp 12`、jobs=12 下运行同一 120 秒窗口；仍到 23 个 marker 和
  `ax-posix-api`。日志 `/tmp/respos-la-tlb-local-12h-120.log`，SHA-256 `b33b807a...a11c`。
  `608476` 次 flush 的本地 op=4 累计 `259911669` ticks（100 MHz 下约 `2.599s`），平均约 `4.27us`、
  最大约 `0.590ms`；其中 fresh-map `555029` 次（`91.2%`）、COW `32539` 次，只有 `6291` 批含 retired
  frame。显式 invtlb 成本已可量化，ASID-wide 失效造成的 TLB 热项丢失仍未直接计时。
- **不能直接跳过 fresh-map flush**：LA software refill 在缺失 PGD/PMD 时会构造两个 invalid TLBELO
  并执行 `tlbfill`，所以无效 PTE→有效 PTE 后当前 faulting hart 确实留有 negative TLB entry；直接跳过
  本地失效会重复 fault。op=5 又已被完整 final 的内存破坏单变量 A/B 否决。因此本轮只保留测量，不恢复
  op=5、不省略同步。该证据随后导向上节 4 KiB Global kernel mapping 实验；实验中 op=4 仍完整执行，
  只让成对、共享且启动后不修改的叶项跨 ASID 保留。稳定宿主的一对 off/on 为正，但因换页差异和缺少
  R3 仍保持 default-off；huge/runtime mapping 从未纳入 Global 域。

## 2026-08-13 allocator A1 per-hart 小对象 magazine（当前工作树，默认关闭）

- **实现与边界**：新增独立 `heap_magazine` feature；每 hart 为 8/16/32/64/128/256 B 六类保留固定
  64 项 intrusive LIFO，miss 最多批量 refill 16 项。普通命中/释放只持本 hart magazine 锁并关闭本地
  中断；大于 256 B、异常 alignment、buddy split/coalesce、失败与 OOM 仍由原 bitmap-assisted buddy
  负责。跨 hart free 进入执行释放的 hart，不依赖原分配 hart。全局回收严格使用
  `magazine -> buddy` 锁序；OOM 会先释放所有 hart cache，再重试一次，不能关闭 coalesce 或改变失败语义。
- **统计语义**：live requested bytes、buddy reserved bytes 与 cached bytes 分开记账；raw-order API 不修改
  user bytes。live/cached/peak 分片已并入同一个本地 magazine 锁保护的普通字段，避免每次 hit/free 额外
  执行两个 LA 原子；跨 hart free 允许单 hart 的 dealloc 大于 alloc，因此读取时必须先分别汇总全局
  alloc/dealloc，再做饱和相减，不能逐 hart 相减。统计快照先逐 hart 取值并释放锁、再读 buddy，保持
  `magazine -> buddy` 锁序；统计、peak reset 与显式 drain 自身也持有 IRQ guard，避免 syscall 上下文
  被中断侧 allocator 重入同 hart magazine。开启 magazine 时 `heap_peak_exact=0`；`heap_current_bytes` 仍由 fallback
  buddy live bytes 加各 hart alloc/dealloc 全局差额得到，
  `heap_magazine_cached_peak_upper_bound_bytes` 是各 hart 局部峰值之和，不是同一时刻的精确全局峰值。
  诊断组合 `heap_magazine perf_counters` 额外接受 `drain_heap_magazines`，仅用于安全回收测试。
- **host 与双架构门禁**：allocator 无计数 9/9、带计数 10/10 单测通过，覆盖全部小类、容量溢出、
  模拟跨 hart 转移、混合 alignment、部分 refill/OOM/drain/retry、两万次随机操作及最终大块 coalesce。
  比赛同版默认 `nightly-2025-01-18`（Rust 1.86）下，RV/LA 的 feature 开/关 release 均构建通过。
  记账降本后 RV64 4 GiB/8 hart 运行 shared-MM 100 轮、64 MiB frame reclaim、显式 drain 293 块、再运行
  shared-MM 100 轮均通过；两次 `match_totals=1`，日志
  `/tmp/respos-rv-a1-accounting-regression.log`。LA 无 perf 反汇编确认普通 hit 只剩 magazine mutex 的一次
  原子获取，原先 cached/live 两次原子更新已经消失。补齐辅助入口 IRQ guard 后，RV64 proc read、显式
  drain 127 块及 shared-MM 100 轮再次通过，日志 `/tmp/respos-rv-a1-irq-stats-regression.log`。
- **LA 计数证据**：12 GiB/12 hart、30 秒窗口共 7,430,314 次 alloc；magazine hit/miss 为
  7,309,569/17,495，eligible 分配命中率约 99.76%，refill 262,425 块、overflow return 111,326 块，
  cache 当前 171,232 B、各 hart peak 上界 291,480 B，低于理论硬上限 387,072 B。heap 分桶
  `match_totals=1`，free frames 约 301 万，无 panic；日志 `/tmp/respos-la-a1-magazine-profile.log`，
  SHA-256 `1cb5c8d8...968d0`。该 feature 组合只证明路径和数量级，不用于小幅墙钟结论。
- **LA 无 perf A/B**：12 GiB/12 hart、`TS/NI=-10`、冷 snapshot 的两对相邻 70 秒 off/on 均到达
  23 个 `Compiling` marker。off 两轮 dev 阶段为 `13.37/5.06s`、`13.04/5.12s`，on 为
  `12.22/4.71s`、`11.39/4.47s`；两类中位数约改善 10.6%/9.8%。另一个相邻 120 秒样本仍为相同
  23 个 marker，off/on 是 `11.61/4.52s` 对 `10.89/4.27s`，约改善 6.2%/5.5%；日志
  `/tmp/respos-la-a1-120s-{off,on}.log`，SHA-256 为 `bc9f031f...ba60`/`aaa41e7a...472d`。
  固定窗口只确认前段 Cargo 收益，主 release 编译未产生更细进度差，不能外推完整成绩。
- **完整 LA 隔离**：早先 12 GiB/12 hart、无 perf 的旧 A1 在 2366.80 秒主动终止且仅到 78 个 marker；
  但随后 8 GiB/12 hart 的 `heap_magazine + perf_counters` 完整运行以 Cargo `23m24s`、axbuild
  `1414.71s` 成功，约 7493 万次 heap alloc，fault/block/PageCache 等工作量与旧完整 perf 基线同量级，
  排除了跳过工作、稳定活锁或必现内存破坏。再以同一 8 GiB/12 hart、同镜像、无 perf 相邻运行旧 A1
  与关闭 magazine：两者均成功且产物同为 1714568 B，axbuild 分别为 `1335.45s` 与 `1281.89s`，旧 A1
  反而慢 `53.56s`（`4.18%`）。因此 2366 秒轮保留为原因 `待验证` 的异常放大样本，不能作为稳定退化
  的单独因果证据；严格 No-Go 依据改为上述同配置完整 A/B。日志分别为
  `/tmp/respos-la-a1-{1000s-mag-perf,isolate-8g-noperf,isolate-8g-off}.log`。
- **记账降本结果**：把 live/cached 记账并入 magazine 锁后，8 GiB/12 hart 无 perf 中窗口到 34/35 个
  marker 约为 `496/737s`，相邻 baseline 约 `763/824s`，证明删除两次原子的阶段收益真实存在；但新的
  完整轮 Cargo `21m48s`、axbuild `1318.37s`，相对同配置 baseline `21m11s`、`1281.89s` 仍慢
  `36.48s`（`2.85%`）。新完整轮成功、产物 1714568 B，宿主只换入 866 页且无换出；日志
  `/tmp/respos-la-a1-local-accounting-full.log`（SHA-256 `ebd35fd7...ed5148`），kernel-la SHA-256
  `975293de...e3cd`，时间线 `/tmp/respos-la-a1-local-accounting-full-timeline/`。
- **当前结论**：A1 所有权、OOM 回收和统计实现可以保留为默认关闭的实验代码，但完整收益仍未达到
  `>=5%` 门槛，性能验收 **No-Go**；不得默认启用，也不得扩大到 512/1024 B。剩余普通 hit 上仍有一次
  per-hart mutex 原子获取。若推进 A3，必须设计由 owner hart 在 IPI/安全点自行 drain 的同步协议后才可
  消除该锁；在没有同步和 OOM 回收证明前，不得把 per-hart state 改成无锁 `UnsafeCell`。

## 2026-08-13 BuildStorm M0 测量闭环与 allocator A0 入口（提交 `c6d13766`）

- **实现范围**：`perf_counters` 新增 heap effective-size 分桶（`max(size, align)`，上界
  16/32/64/128/256/512/1024/2048/4096/>4096）、LA/RV user trap 分类、RV IPI，以及两架构 remote
  shootdown/RFENCE 的目标 hart 数、空请求、完成等待和最大等待。heap calls/bytes/wait/core/total/max
  均按 hart 分片，读取时汇总；heap 当前值/峰值在已经持有 allocator 锁的内部统计中维护，避免诊断
  本身在每次 alloc/dealloc 上写同一全局 cache line。第二轮校准仍未过 3% 后，calls/bytes 保持精确，
  硬件时钟 total/wait/core 改为每 hart 1/64 抽样估算并显式输出样本数；max 是抽中样本的真实最大值。
  无 `perf_counters` 时峰值字段和更新分支静态消除。
- **正确性验证**：`respos_buddy_allocator` 无 feature 2/2、带 feature 3/3 单测通过；LA/RV feature 开/关 release
  构建均通过；比赛同版 `nightly-2025-01-18`（Rust 1.86）LA/RV feature release 也均通过。RV64
  4 GiB/8 hart diagnostic 中 `smp_shared_mm_probe` 100 轮通过；统计记录约 300 次
  remote RFENCE、300 个目标 hart，heap 分桶汇总 `match_totals=1`。reset 后 `heap_peak_bytes` 等于当前
  占用，后续退出阶段峰值可继续增长，证明峰值复位与读取语义闭合。日志为
  `/tmp/respos-rv-m0-sharded-smoke.log`。
- **LA 30 秒 size-class 证据**：LA64 12 GiB/12 hart、旧 pub 镜像、diagnostic、从冷 snapshot 运行
  `busybox timeout 30 /bin/bash /glibc/buildstorm_testcode.sh`，日志
  `/tmp/respos-la-m0-heap-profile.log`。窗口完成 toolchain/minibuild，停止在 untimed tg-xtask，故只用于
  热点结构而不是正式 BuildStorm 成绩。共 5,588,558 次 alloc、5,415,110 次 dealloc；alloc 调用中
  `<=16/32/64/128/256 B` 分别约占 `36.06/19.12/11.09/17.23/14.75%`，累计 `98.25%`；这五类占
  alloc core ticks 约 `96.7%`。`>4096 B` 仅约 `1.13%` 调用，却占请求字节约 `63.7%`。因此下一实验
  应是现有 buddy 上的有界 per-hart 小对象 cache，而不是替换整个 allocator；大块、异常对齐、split/
  coalesce 和 OOM 路径继续由现有 bitmap-assisted buddy 负责。
- **开销门禁**：第一版全局 heap 分桶在 LA 130 秒窗口中，相对相邻 no-feature 样本的两个 dev 阶段
  和 core 开始时间慢约 17--22%，未达到预设 `<=3%`，因此已否决并改为上述按 hart 汇总。重构后的
  RV 功能证据有效。随后 LA 12 GiB/12 hart、`TS/NI=-10` 的相邻 70 秒 sharded-perf/no-feature
  样本均到达相同 23 个 `Compiling` marker，但两个 dev 阶段仍为 `12.89/5.42s` 对
  `12.24/4.81s`，单样本约慢 5.3%/12.7%，仍未过门槛；日志为
  `/tmp/respos-la-m0-sharded-cal-{perf,nofeature}.log`。因此进一步引入 1/64 timing sampling；其开销
  重校准 sampled-perf 的两个 dev 阶段为 `14.22/5.35s`，相邻 no-feature 为 `12.24/4.81s`；两者
  均到达 23 个 crate，仍高于 3%。该结果只能说明完整 `perf_counters` feature 不适合精细墙钟 A/B，
  不能把差额全归因于 heap，因为 ext4/fault/scheduler/TLB 等仍逐事件计数，且单次宿主样本有波动。
  sampled-perf 日志 SHA-256 为 `2258e536...693da`，no-feature 为 `903cb94e...2adbf`。不得用旧全局或
  未采样 sharded 样本的绝对 heap wait 数评价后续 A/B，也不得把 30 秒分布样本当作性能收益。
- **Go/No-Go**：A0 设计 **Go**（调用分布证据充分）；A1 仅允许作为默认关闭的独立实验推进，生产
  路径收益必须用 **关闭 `perf_counters`** 的 before/after A/B 决定。合入/成绩声明仍 **No-Go**，直到
  专项 allocator 压测、双架构门禁和无 feature 完整 final 通过。具体边界见
  `buildstorm-smp-plan.md` 的 allocator A0--A2。

## 2026-08-13 LA64 BuildStorm 完整时间线诊断

- **范围与证据**：代码基线 `66853fe` 加当前未提交的性能计数/时间线补丁；LA64 release，
  `perf_counters`，12 GiB/12 hart，`-snapshot`，QEMU 10.0.2。QEMU 从启动起核验为
  `NI=-10, CLS=TS`，online mask `0xfff`。串口日志为
  `/tmp/respos-la-eval-20260813.log`（SHA-256
  `d2bb6cb00ce628fddfddd73aba448c30053624d8af40d35e55131a2f299bd5c2`），kernel-la 为
  `11a88ef6f8376e7580bddcc735fa800da406aa9e1364430ce339ef78dc09e5b9`，宿主时间线位于
  `/tmp/respos-la-eval-20260813-timeline/`。该轮是本机 12 GiB 诊断，不是平台 36 GiB 正式成绩。
- **正确性与耗时**：CAgent 10 项 pass、脚本 exit 0；BuildStorm toolchain/minibuild 均通过，
  `BUILDSTORM_COMPILE mode=multi ok=true cores=12 bytes=1714568`，脚本 exit 0。axbuild 报告
  `1715.30s`，Cargo release 为 `28m24s`。`BUILDSTORM_COMPILE elapsed_s=0.00` 是当前脚本/guest
  计时字段异常，不能替代 axbuild/Cargo/宿主单调时间。
- **宿主/guest 交叉校验**：宿主记录 1758.45 秒、10580.95 核秒，平均 CPU `601.7%`，峰值
  `1202.8%`；低于 `400%/800%` 的时间分别占 `34.9%/68.6%`。guest `task_running_ticks` 折合
  10443.95 核秒、idle 10192.17 核秒，有效运行率 `50.61%`；宿主与 guest running 核秒相差约
  1.3%，采样口径相互支持。12 个主要 vCPU 线程均取得约 627--1109 核秒，不存在单个 hart 长期
  未运行的证据。
- **调度器结论**：running hart 样本 `0/1/2-3/4-7/8+` 占比为
  `0.12/20.53/20.16/29.12/30.07%`；ready queue 对应为
  `95.27/3.61/1.08/0.04/0.00%`。scheduler 锁 3430646 次获取、累计等待 2.97 核秒，平均约
  0.87 微秒、最大约 1.46 毫秒。当前证据不支持把全局 scheduler 锁或 runnable 未及时派发列为
  P0；低 CPU 更接近编译任务阶段性并行度不足与其他阻塞/串行路径。
- **已量化内核成本**：heap alloc/dealloc 共约 1.217e3 核秒，其中锁等待约 919.18 核秒、allocator
  core 约 290.30 核秒，占 guest running 核时约 11.7%/8.8%/2.8%；7533 万次 alloc 与 7507 万次
  dealloc 使单全局 heap 锁成为当前最强的已量化候选。frame alloc 149.39 核秒，其中清零 129.33
  核秒、锁等待（含 dealloc）5.91 核秒；连续帧/批量清零值得做健全性和次级优化，但不是本轮 P0。
  copy user 约 112.08 核秒。
- **文件系统候选**：ext4 锁等待/持有约 95.69/160.19 核秒，lower call 约 152.37 核秒；有优化价值，
  但量级低于 heap。PageCache 最终停在 32762 页（约 128 MiB 上限），全程 587381 次淘汰、约
  2.18 GiB fill 和 2.40 GiB block read；这支持检查固定容量导致的重复读取，但 hit/miss 为
  2594099/83916，且 block read/write 计时仅约 27.53/19.27 核秒，扩大容量的墙钟收益仍需 A/B，
  不能仅凭淘汰数断言。
- **TLB 待补计时**：remote RFENCE 515482、ASID invalidation 7015484，range shootdown 475191 次、
  累计范围页数约 49.18 亿；计数仍高，但当前没有 RFENCE 发起到完成的 ticks，不能从次数直接排序
  到 heap 之前。下一轮应补完成延迟/等待核时，而不是继续只增加事件计数。
- **测量边界**：上述 `*_ticks` 可能嵌套，禁止直接相加成墙钟分解；该轮使用的是后来因观测扰动而
  淘汰的全局 heap 原子计数版本，绝对 heap wait/core 数只用于发现候选，正式 A/B 必须用上节的
  sharded 版本重校准并以无 feature 成绩收口。该 LA 时间线版本尚未逐秒记录宿主
  swap；启动/结束观察到 swap 使用约从 4.2 GiB 到 5.2 GiB，但不能据此归因。采样器随后已加入
  `host-system.csv` 的 MemAvailable/swap/major-fault 时间序列和退出后的串口 marker drain，供 RV
  及下一次 LA 使用。

## 2026-08-13 RV64 BuildStorm 完整时间线诊断与 LA 对照

- **范围与证据**：与上一节相同代码/feature，RV64 release、16 GiB/8 hart、`-snapshot`，QEMU 从
  启动起为 `NI=-10, CLS=TS`。串口日志 `/tmp/respos-rv-eval-20260813.log`（SHA-256
  `a11da56bc411c2ee0f563227e9ab58d33939dbabd0fe2c96c4bd59c248c2b88f`），kernel-rv 为
  `b1fb9cffe42edca9b3d3a0e142129652644a353a6410cdcb3960b12c27528432`，时间线目录为
  `/tmp/respos-rv-eval-20260813-timeline/`。
- **正确性与耗时**：CAgent 10 项 pass、exit 0；BuildStorm toolchain/minibuild 通过，最终
  `ok=true cores=8 bytes=1681000`、exit 0。axbuild `1245.38s`，Cargo release `20m35s`。
  宿主记录 1289.32 秒、5559.26 核秒，平均/峰值 CPU `431.2%/804.7%`；guest running/idle 为
  5600.67/4652.42 核秒，有效运行率 `54.62%`。宿主与 guest running 核秒误差小于 1%。
- **宿主压力边界**：最低 MemAvailable 约 3.70 GiB、最低 SwapFree 约 9.03 GiB；全程 swap-in
  27542 页（约 108 MiB）、swap-out 205639 页（约 803 MiB）、major fault 增量 29573，且几乎全部
  发生在 core-to-app 主编译阶段。没有内存耗尽，但存在可测的宿主换页污染；该轮可用于内核热点和
  跨指标量级，不应把 1245.38 秒当作无宿主干扰的正式成绩。
- **调度器结论**：running hart `0/1/2-3/4-7/8` 占比
  `0.19/30.60/25.84/17.61/25.76%`，ready queue `0/1/2-3/4-7/8+` 占比
  `97.68/2.00/0.30/0.03/0.00%`。scheduler 锁累计等待 1.26 核秒、平均约 0.74 微秒；8 个主要
  vCPU 线程均运行约 517--1015 核秒。与 LA 一致，不支持优先重构 scheduler；主编译平均约
  4.52 个宿主核忙，低利用率主要对应编译依赖图/阻塞路径没有持续提供满核工作。
- **allocator 对照**：RV heap alloc/dealloc 约 7610/7583 万次，总计 177.45 核秒，锁等待 87.08
  核秒，占 running 核时约 3.17%/1.55%；LA 次数近似，却为 1216.88/919.18 核秒，占
  11.65%/8.80%。更重要的是累计请求字节 RV 约 10.52 GiB（平均 alloc 148.5 B），LA 约 51.14
  GiB（平均 728.9 B）。因此全局 heap 是共同高频路径，但 LA 的 P0 同时包含锁竞争和请求尺寸/工作量
  放大；改 slab 前必须补 size-class 次数、字节和 wait/core ticks，不能把全部差异归因于 buddy 算法。
- **文件/内存对照**：RV ext4 wait/hold 77.46/111.89 核秒，LA 为 95.69/160.19；两者均有价值但
  低于 LA heap。两架构 PageCache 都结束在约 32768 页上限；RV 淘汰 433195、fill 1.58 GiB、block
  read 1.83 GiB，LA 为 587381/2.18/2.40 GiB，固定 128 MiB 容量抖动是共同候选。RV frame alloc
  94.94 核秒中 clear 93.00，锁等待（含 dealloc）1.36；LA 为 149.39/129.33/5.91，继续支持“批量/
  连续页和清零优化是次级项”。RV copy-user 约 135.50 核秒，也应列入 RV 专项候选。
- **RV TLB 当轮计数缺口**：该轮输出 `remote_rfences=227068`，但 shootdown 分类、IPI received 与
  user trap 分类全为 0，因此这份历史日志不能直接与 LA 对比。上节 M0 已在当前工作树补齐 RV
  trap/IPI/shootdown 分类和两架构发起到完成等待；新字段已由 8-hart shared-MM 探针验证，但尚无
  当前版本的 RV/LA 完整 BuildStorm 对照。

## 2026-08-13 ext4/PageCache E0 当前 HEAD 基线（`51bed0e1`）

- **可复现口径**：代码为 `51bed0e1c598c33eab4bc2da7703534525149ac4`，使用
  `RUSTUP_TOOLCHAIN=nightly-2025-01-18`（Rust 1.86）构建仅带 `perf_counters` 的 LA release
  kernel，kernel SHA-256 为 `36828ea9b7b74800d38b2dd4dd1b05f1ecf6300faa7126e87e7a8214de4b9fc1`；
  pub x0 SHA-256 为 `450682fd547c43a19379ff0cf46f211eb7ba0b22463165dc88443701bd9ee9ca`。
  QEMU 10.0.2 以 `-snapshot -m 12G -smp 12` 运行，online mask `0xfff`；启动后核验宿主
  QEMU 为 `CLS=TS / NI=-10`。30/120 秒日志分别为
  `/tmp/respos-la-e0-rust186-12h-30s.log` 和 `/tmp/respos-la-e0-rust186-12h-120s.log`，SHA-256
  分别为 `aa31df0c37a40f1eb7dd81076be4755349d743dc1ce74f69af5090cc68006236`、
  `090a0a303479a782428b9725b38eb2e63c8ae89e0373e8ea68e84924bad72ac3`。
- **30 秒窗口**：toolchain/minibuild 通过，untimed `tg-xtask` 13.81 秒后被 timeout；ext4
  acquisition `43357`，wait/hold `2106964/921225232` ticks，即约 `0.021/9.212` CPU 秒。
  hold 以 lookup/read/namespace/attributes 为主，约 `2.630/2.681/1.745/1.470` 秒；PageCache
  hit/miss/eviction 为 `108747/8207/24856`，fill `337866624` bytes，inode read 约 `2.683` 秒。
  该阶段 wait 近零，仍否决直接拆 `EXT4_OP_LOCK`。
- **120 秒窗口**：停止于 timed `arceos-helloworld` 编译，完成的 workload 进度与另一 120 秒窗口
  可直接对照。ext4 acquisition `85136`，wait/hold 升至 `456195698/2253604957` ticks，即约
  `4.562/22.536` CPU 秒；lookup/read wait 约 `2.434/1.080` 秒，证明并发编译阶段全局锁竞争会
  出现，但尚未通过 `1/3/6/12` 缩放证明拆锁收益。PageCache hit/miss/eviction 为
  `324575/14267/80604`，fill `688721334` bytes，inode read 约 `7.382` 秒；固定 32768 页下存在
  明显 eviction/refill 压力。
- **allocator 交叉证据**：120 秒内 kernel heap alloc/dealloc 共约 `22.523` CPU 秒，其中 lock wait
  共约 `10.318` 秒；它与 ext4 并发阶段同时放大，不能把全部墙钟损失归因于 ext4。下一步先补锁内
  prepare/C-call/publish 分段计数，并做 `1/3/6/12` 同进度缩放；E1 优先移出可证明的锁内 Rust
  分配/转换和重复 lookup，保留 lwext4 C 全局串行。allocator 改造仍作为独立实验，不混入 E1。
- **排除样本**：同日先误用 Rust 1.89、默认宿主调度跑出的两轮只用于发现阶段变化，不属于正式
  E0；一次请求 `nice -10` 但实际继承为 `CLS=IDL` 的空载启动也已立即退出，均不得用于 A/B。
- **E0 分段计数烟测（未提交工作树）**：新增计数不改变 allocator 或 ext4 行为，并在 Rust 1.86
  LA/RV64 `perf_counters`、LA 无 feature release 构建通过。LA 12-hart/12 GiB、`CLS=TS/NI=-10`
  的 30 秒日志为 `/tmp/respos-la-e0-segmented-12h-30s.log`，toolchain/minibuild 通过并进入 timed
  构建。frame alloc `179022` 次、失败 0，总计约 `2.093` CPU 秒，其中清零约 `1.949` 秒、allocator
  core 约 `0.138` 秒、锁等待约 `0.006` 秒；frame dealloc 约 `0.017` 秒。因此当前帧 allocator
  元数据和锁不是 BuildStorm 一级性能瓶颈，连续/批量接口仍是 VirtIO DMA 健全性任务，不能据此
  预期普遍加速单页 fault。
- **ext4 分段结果与覆盖边界**：本轮只对 stat/lookup/read/readdir 的明确 lwext4 调用或调用序列计时，
  未覆盖 write/namespace/attributes/superblock，故 `profiled_lower` 不能与全部 ext4 hold 直接相减。
  已覆盖类别的 lower/hold 分别约为 stat `0.066/0.073`、lookup `2.563/2.605`、read
  `1.949/2.209`、readdir `0.417/0.431` CPU 秒。lookup/stat/readdir 的锁内成本几乎都在 lower C/I/O；
  read 尚有约 `0.260` CPU 秒锁内非 lower 工作。E1 应先审计 read buffer 准备和可合并 lower read，
  不能把移动 CString 或结果转换描述成主要收益；namespace/attributes 仍需补同口径分段后再动实现。
- **E0 全类别分段闭合**：后续 12-hart 30 秒窗口 `/tmp/respos-la-e0-full-segments-12h-30s.log`
  已覆盖 write/namespace/attributes/superblock。ext4 总 hold 约 `8.054` CPU 秒，明确 lower C/I/O
  约 `7.729` 秒；namespace lower/hold `1.374/1.377`、attributes `1.309/1.309`、write
  `0.107/0.107`、superblock `0.033/0.033` 秒。全部类别锁内非 lower 工作合计仅约 `0.325` 秒，
  因而 E1 的主目标改为减少/截断无效 lower 调用，而不是大规模搬移 CString/结果转换。E0 尚余
  `1/3/6/12` 同进度缩放；短 timeout 受宿主速度影响明显，不能直接按总计数比较。
- **PageCache 重叠预读证据**：新增候选/发布/竞态页计数后，未优化行为的 30 秒样本
  `/tmp/respos-la-e0-page-publish-12h-30s.log` 在尚未完成 untimed `tg-xtask` 时已有候选 `81304`、
  发布 `54475`、发布时已存在 `26829` 页，约 33% 候选页对应重复 fill/清零。该轮宿主整体较慢，
  绝对 wall time 和 CPU ticks 不用于 A/B，但同一 guest 内候选与发布差值可用于定位工作放大。
- **E1 预读边界优化**：`PageCache::get_or_load` 在规划顺序预读时检查前方已发布页面，并在首个
  cached page 前截断 speculative run；请求页、lower 错误、size-version 重试和发布协议不变，也未
  增加等待或新锁。优化后 `/tmp/respos-la-e1-readahead-boundary-12h-30s.log` 已进入 timed 构建，
  候选/发布/竞态为 `56129/56129/0`，fill bytes `221070975`、block read bytes `259740160`；相邻
  未优化样本分别约 `330.7/359.4 MiB`，但因进度与宿主速度不同，只确认“重叠发布浪费归零”，
  完整 wall-time 收益仍待无 feature final 验证。
- **否决实验与正确性门禁**：per-inode miss-fill gate 将同一大文件不同页区间错误串行，样本
  `coalesced_misses=0` 且 30 秒无法完成 untimed 阶段，已完整回退，不在当前 diff 中。当前保留的
  cached-boundary 实现通过 Rust 1.86 LA/RV64 无 feature release 构建；LA 4 GiB/12-hart 无 feature
  客体通过 `buildstorm_file_probe` 与 `fs_writeback_probe normal`，日志
  `/tmp/respos-la-e1-pagecache-probes.log`；LA 专项明确跳过 RV64-only private-map probe。
- **E1 120 秒同进度 A/B**：最终计数窗口 `/tmp/respos-la-e1-final-12h-120s.log` 与上方当前 HEAD
  Rust 1.86 基线均停止在 timed `arceos-helloworld` 的相同编译阶段，`file_closes=12853`，stat
  `221528`（基线 `221527`）、create `412` 完全对齐。PageCache fill bytes 从 `688721334` 降至
  `441110664`（约 -36.0%），block read bytes 从 `732134912` 降至 `484924416`（约 -33.8%），
  eviction `80604 -> 77753`；候选/发布/竞态页为 `110400/109632/768`，竞态浪费仅 0.70%。ext4
  wait/hold 从约 `4.562/22.536` 降至 `3.549/19.256` CPU 秒，read hold `6.609 -> 5.069`
  秒。该结果支持保留 cached-boundary E1；它证明工作量和累计 CPU 成本下降，不等同于完整 final
  wall-time 收益，后者仍以无 feature 完整运行验收。
- **E1 锁外准备收口**：lookup/link/symlink/rename/remove 的 immutable path/CString 在取得
  `EXT4_OP_LOCK` 前构造；构造失败仍早于任何 lower mutation。read 的 sparse-hole 预清零仍保留在
  open/seek 成功后和锁内，避免为性能改变 lower 失败时 buffer 状态；readdir 也保留单次 iterator
  快照，不拆成可能观察不同目录 generation 的多轮调用。create 后 lookup 在本窗口仅 412 次，未为
  消除低频调用扩展 vendor mutation API。
- **E1 完整无 feature final**：Rust 1.86 no-LTO 内核 SHA-256
  `cdde3c22caa04476ad86bbd12c6edc49c36c8b5cefe8a4c0922871874e60516c`，LA 12 GiB/12 hart、
  `CLS=TS/NI=-10`、pub x0 与临时 final x1 下完整运行。CAgent 脚本退出 0，原始逐项为 9 个
  `pass`、`cpu` 1 个 `reject`，不记作 10/10；BuildStorm 输出 `ok=true cores=12 bytes=1714568`，
  脚本退出 0并正常关机。Cargo release `28m53s`，axbuild `1743.70s`；相邻旧 op=4 完整基线
  `1773.01s`，改善 `29.31s`，约 1.65%。日志 `/tmp/respos-la-e1-full-final.log`，SHA-256
  `dc325a471c1714ef3b83ed774f42a4aae426342c81d13ca77d0e7a92e35af31f`。完整收益低于 5% 门槛，
  因此保留已证明降低读取放大的低风险 E1，但不据此扩大 ext4/PageCache 高风险重构；下一性能主题
  由缩放和 allocator 数据重新选择。

## 2026-08-13 LA op=5 完整门禁失败并回退 range 至 op=4（当前工作树）

- **当前实现与健全性边界**：单页 op=5 已从本地与远端执行路径删除；address-space 和所有 range
  均执行一次 op=4，root 激活、ASID retired 批量复用的 all 请求和非法请求仍执行 op=0。范围传播、
  同步 request/ack、residency mask 与 frame completion 均保留，且不在 IPI handler 中逐页循环。
- **构建与正确性门禁**：LA/RV64 无 feature release 顺序构建通过，LA 汇编器接受 op=5 寄存器形式。
  LA `-m 4G -smp 12 -snapshot` 上 2400 次 exec 与连续两次 shared-MM 各 100 轮通过；全新 snapshot
  中 Phase3 30 轮通过，结束 `free_kb=3903436 dirty_kb=0 heap_kb=4752`。首轮 Phase3 曾有一个
  `net_loopback_smoke` 读到 EOF 后未结束，故该轮未记为通过；全新 guest 复跑不再复现。
- **op=5 实验计数**：清零后 2400 次 exec 返回 0，range 请求 `3015` 个且全部为单页，invalid 为
  0；当时执行计数 full/ASID/page 分别为 `2460/21662/294531`。这些短门禁只能证明路径被执行，
  已被下述完整 final 内存破坏否决。
- **30 秒 BuildStorm 窗口**：通过 toolchain/minibuild，untimed `tg-xtask` 12.02 秒，随后 timed
  arceos 外层准备 4.64 秒并进入实际构建。shutdown 快照记录 range `6052`，其中单页/2--16/
  17--256/>256 为 `6009/8/18/17`；full/ASID/page 执行为 `1660/8706/209948`，invalid 为 0。
  相比上一协议阶段 range 回退 op=0 的 full `23114`，完整失效已降回低位；短窗口进度受 I/O 与
  fault 波动影响，仍不作为正式 wall-time 加速比。
- **完整 final 门禁失败**：同一无计数器内核以官方 LA pub x0、final x1、`-snapshot -m 12G
  -smp 12` 运行。CAgent 10/10、退出 0；BuildStorm minibuild 随后出现
  `stack smashing detected`，正式 arceos 构建在编译 `std/core/libc/compiler_builtins` 时报告
  `free(): chunks in smallbin corrupted`，脚本因 SIGABRT 退出 134。日志为
  `/tmp/respos-la-op5-final-local.log`。因此单页 op=5 当前不能作为可提交实现。
- **op=4 单变量 A/B**：只把单页执行回退为 op=4 后，比赛 nightly 双架构 release 构建通过；LA
  2400 exec、双 shared-MM 与 Phase3 30 轮通过。相同 final 配置下 CAgent 退出 0、minibuild 从 fail
  恢复为 ok，并越过原先在 `compiler_builtins/core/libc/std` 处的 allocator corruption。完整构建最终
  输出 `BUILDSTORM_COMPILE mode=multi ok=true cores=12 bytes=1714568 arch=loongarch64`，BuildStorm
  脚本退出 0 并正常关机；Cargo release 为 `29m 22s`，axbuild 报 `1773.01s`。脚本自身
  `elapsed_s=0.00` 是镜像计时字段异常，不作为耗时。完整日志
  `/tmp/respos-la-op4-range-final-ab.log`。该单变量 A/B 支持保留 range 协议但禁用 op=5。

## 2026-08-13 LA 叶 PTE 失效范围传播（当前工作树）

- **实现与边界**：LA `PageTable` 的 map/unmap/permission/COW/replace 入口在成功修改叶 PTE 后，
  累积半开 VPN 最小包络；`MemorySet::flush_tlb()` 与 retired-data-frame 批次一起冻结该范围，并向
  residency 目标发布带 ASID 的 `range` 请求。包络可包含稀疏修改之间的未变页，但不会漏掉修改页。
  无实际 PTE 变化的 lazy mmap/brk flush 回退为 address-space；完整 root activate 会清除已由 op=0
  覆盖的构建期包络。本阶段 range handler 仍按既定保守边界执行 op=0，尚未启用 op=5。
- **正确性门禁**：LA/RV64 无 feature release 顺序构建通过。LA `-m 4G -smp 12 -snapshot` 上
  2400 次 exec、shared-MM 连续两次各 100 轮和 Phase3 30 轮通过；结束状态
  `free_kb=3821452 heap_kb=17151 tasks=3`。`mmap_phase5_probe` 仍返回仓库既有的 7 项
  `EXPECTED_FAIL` 并输出 `CURRENT DIFFERENCES CONFIRMED`，没有新增崩溃，但不记为 COW PASS。
- **30 秒范围分布**：同 P0 口径通过 toolchain/minibuild，untimed `tg-xtask` 12.93 秒并进入 timed
  arceos build。5900 个 range 请求累计 31020 页，最大 10938 页；单页/2--16/17--256/>256 页
  分别为 `5856/9/18/17`，即约 99.25% 是单页。另有 1032 个无范围 address-space 请求，invalid 为 0。
- **性能边界**：因本阶段 range handler 有意回退 op=0，full invalidations 从上一阶段 1543 临时升至
  23114；这是协议验证成本，不是性能改进。下一阶段应给 op=5 设置小范围阈值，大范围继续 op=4，
  不能按最大 10938 页包络逐页执行 INVTLB。

## 2026-08-13 LA 按 ASID 执行 INVTLB op=4（当前工作树）

- **实现与安全边界**：LA 新增带 10-bit 边界检查的 `sfence_asid()`，普通 `MemorySet` PTE writer
  的本地失效以及远端 `address-space` handler 使用 `invtlb op=4`。ASID 批量回收的 `all`、root
  激活、非法请求和 `range` 请求仍执行 `op=0`；运行期页表未设置 Global PTE，故 op=4 会覆盖目标
  ASID 的用户项和共享高半区非 Global 项。本阶段没有启用 op=5，也没有改变 frame 释放屏障。
- **正确性门禁**：LA/RV64 无 feature release 顺序构建通过；LA `-m 4G -smp 12 -snapshot`
  串行 exec 2400 次后，shared-MM 连续两次各 100 轮、Phase3 30 轮全部通过。结束状态
  `free_kb=3821976 heap_kb=16993 tasks=3`，未见 stale translation、卡死或回收异常。
- **执行分类**：带计数 rollover 窗口返回 0、PID 达 2407，请求分类
  `all/address-space/range/invalid=4/3649/0/0`；执行计数为 full 2459、ASID 316801。full 主要包含
  2400 次保守 root 激活及 4 次 all 请求的远端执行，证明 rollover 没有被错误降级成 op=4。
- **30 秒对照**：同 P0 口径通过 toolchain/minibuild、untimed `tg-xtask` 13.45 秒完成并进入 timed
  arceos 构建。相邻协议阶段为 full 184406；本阶段为 full 1543、ASID 184271，remote RFENCE
  6819 且全部为 address-space、invalid 为 0。该窗口说明失效类型替换生效且进度未回退；由于 I/O、
  fault 数和宿主调度仍有波动，不作为最终 wall-time 加速比。

## 2026-08-13 LA shootdown 请求语义显式化（当前工作树）

- **协议边界**：LA 每目标 hart 的 generation 槽现在同时发布 `all`、`address-space` 或 `range`
  请求及 ASID/页对齐区间；发送端校验并分类计数，接收端在 ack 前重建和校验同一描述。ASID
  retired 批次回收使用 `all`，普通 `MemorySet` PTE 刷新使用自身 ASID 的 `address-space`。
  该协议阶段当时为保持可回退且不把协议验证冒充硬件语义验证，handler 仍统一执行保守的
  `invtlb op=0`；当时 `range` 仅有协议表示且尚无调用方。
- **正确性门禁**：LA/RV64 无 feature release 顺序构建通过。LA `-m 4G -smp 12 -snapshot`
  上 shared-MM 两次各 100 轮、Phase3 30 轮通过。另由 diagnostic Bash 脚本串行 exec 2400 次，
  返回 0、PID 达 2407；窗口记录 `all/address-space/range/invalid=4/3476/0/0`，随后 shared-MM
  100 轮通过，健康状态为 `free_kb=3835452 heap_kb=14960 tasks=3`。临时脚本已从工作树删除。
- **30 秒活性/分类**：同 P0 配置通过 toolchain/minibuild，untimed `tg-xtask` 在 13.49 秒完成。
  计数为 remote RFENCE 6814、full invalidations 184406，请求分类
  `all/address-space/range/invalid=0/6814/0/0`。该结果证明普通路径的语义分类和协议传输一致；
  因执行端仍为 op=0，本阶段不宣称 TLB 性能提升。

## 2026-08-13 页表页按 root-switch completion 退役（当前工作树）

- **实现**：删除 RV64/LA64 共同的全局 128 页 `PAGE_TABLE_FRAME_QUARANTINE`。
  `recycle_data_pages()` 将根和中间页表 frame 移入所属 `PageTable` 的退役槽；如果地址空间
  从未 active 则立即释放，否则由调度路径在 `__switch` 已恢复 per-CPU idle/kernel root 后清除
  active bit，最后一个 bit 的清除者释放页表页。这将释放条件从“已有 128 页更新的退役页”
  改为“已证明无 hart 仍使用该 root”。本阶段仍未修改 LA `invtlb op=0` 或 shootdown 范围。
- **构建门禁**：LA/RV64 无 feature release 顺序构建通过；os/user fmt 与 `git diff --check`
  通过。
- **立即复用压力**：LA `-m 4G -smp 12 -snapshot`上 BusyBox xargs 串行 exec 2400 次返回 0，
  PID 达 2405，覆盖两轮以上 10-bit ASID 空间且页表页不再受旧 quarantine 保护。随后
  `smp_shared_mm_probe` 连续两次各 100 轮、Phase3 30 轮全部通过；结束为
  `free_kb=3821344 heap_kb=17094 tasks=4`，未见 stale translation、卡死或页表页线性泄漏。
- **30 秒活性复测**：LA `-m 12G -smp 12` perf 窗口继续通过 toolchain/minibuild，并在
  timeout 前输出 untimed `tg-xtask` `Finished dev profile ... in 13.80s`，进度比前两轮更远。本轮
  page faults/full invalidations/remote RFENCE 为 `110674/180248/6794`；由于实际进度和 COW
  faults 不同，不用总计数宣称性能加速。

## 2026-08-13 LA 数据 frame 按地址空间退役（当前工作树）

- **所有权修复**：删除混合所有 LA `MemorySet` 的全局 retired-data-frame 队列。每个 LA
  `PageTable` 现在保留自己 PTE 被撤销/替换后的 `Arc<FrameTracker>`；`flush_tlb()` 在本地
  失效前冻结该批次，对同一地址空间的 residency mask 同步 shootdown，所有目标 ack 后
  才 drop 批次。这使 frame 释放屏障与唯一 ASID/地址空间绑定，不再由一个无法证明归属的
  全局批次强制全 online hart 失效。本阶段没有修改 `invtlb op=0`、ASID 编号复用规则或
  request/ack 协议。RV64 保持原 immediate-release 路径。
- **正确性门禁**：LA/RV64 无 feature release 顺序构建通过，fmt 与 `git diff --check` 通过。
  LA `-m 12G -smp 12 -snapshot` 上 `smp_shared_mm_probe` 连续两次各 100 轮通过，Phase3
  30 轮通过，结束时 `free_kb=12288936 tasks=4`。LA `-m 4G -smp 12` 上 BusyBox xargs 串行
  exec 1200 次返回 0，PID 达 1205；ASID rollover 后 shared-MM 100 轮和 Phase3 30 轮再次通过。
  `smp_shared_mm_probe` 在两 hart 间反复执行固定 VA `munmap + MAP_FIXED mmap + read`，是本轮
  stale-TLB/frame-reuse 的直接专项，不是仅观察 BuildStorm 不崩溃。
- **30 秒同口径 A/B**：变更前/后均通过 toolchain/minibuild 并停在 untimed `tg-xtask`。变更后
  user traps/page faults 为 `203559/106329`（变更前 `196826/106178`），负载量级相当且
  `file_closes=4065` 高于前轮 `3270`。remote RFENCE `7656 -> 6788`（约 -11.3%），full TLB
  invalidation `183563 -> 175784`（约 -4.2%），IPI received `38795 -> 29211`。该结果符合
  “只消除跨地址空间的误伤目标”边界，不作为精确 ASID/VA 失效或 wall-time 加速证明。
- **后续边界**：数据 frame 退役所有权已闭合；页表页的容量型 quarantine 也已由上方
  root-switch/active completion 取代。按 ASID/VA 精确失效仍是独立的下一阶段，不能由
  本轮生命周期门禁直接推导为已安全。

## 2026-08-13 LA BuildStorm P0 30 秒性能基线（`bba2ee3`）

- **口径**：当前干净工作树 `bba2ee3a72eb372ecca909a93cd6c6fd3ee86ab0`，LA release kernel
  仅开启 `perf_counters`；QEMU LA virt 使用公开决赛 x0、临时 `mode=diagnostic` x1、
  `-snapshot -m 12G -smp 12`，online mask `0xfff`。在 shell 中一次性预排 perf reset、
  `busybox timeout 30 /bin/bash /glibc/buildstorm_testcode.sh`、读取 `/proc/respos_perf` 和 `quit`，
  避免 stdin 空轮询污染。该旧 pub 镜像的 30 秒窗口包含 toolchain、minibuild 和 untimed
  `tg-xtask`，因此是诊断基线，不是正式 timed BuildStorm 成绩。
- **进度与活性**：`BUILDSTORM_TOOLCHAIN ok` 和 `BUILDSTORM_MINIBUILD ok` 均出现；窗口在
  `----- pre-build tg-xtask (untimed) -----` 中由 timeout 结束，shell 报该进程 exit code 15，
  随后 perf proc 读取成功且 launcher 正常关机。`scheduler_yields=2`，两次均属于 process
  路径；stdio/fs/futex/net/signal-time 的 yield 均为 0。
- **MM/TLB 计数**：user traps `196826`，其中 page fault `106178`；private-file/anonymous/COW
  fault 分别为 `82855/51667/9377`。local sfence `148381`、remote RFENCE `7656`、full TLB
  invalidation `183563`。extension eager save `196152`，说明 Rust 工具链进程激活扩展状态后，
  首次使用门控无法避免绝大多数后续 trap 的 eager save。
- **FS/PageCache 计数**：128 MiB PageCache 到达 `32768` pages，hit/miss/eviction 为
  `108486/7521/24135`，fill `336661498` bytes。block read 为 `18837` requests / `373223424`
  bytes，block write 为 `45726` requests / `178389504` bytes。ext4 lock acquisition `37373`，
  wait/hold 为 `1696486/921703055` ticks（100 MHz 下约 `0.017/9.217` 累计 CPU 秒）；hold
  主要来自 read `2.976s`、lookup `2.542s`、namespace `1.718s`、attributes `1.293s` 和
  readdir `0.475s`。dentry cache `69208/9800/0` hits/misses/evictions，本窗口不支持优先
  继续扩容 dentry cache。
- **scheduler/heap 边界**：context switch `4406`，scheduler ready peak `3`，lock wait
  `2203636` ticks（约 `0.022s`），不支持优先重构 per-CPU runqueue。heap peak 约
  `34.8 MB`；alloc/dealloc core 约 `1.413/0.919s`，lock wait 合计约 `0.390s`，明显低于
  ext4 hold，不支持优先更换 allocator 或扩大 kernel heap。
- **P0 判读**：下一轮优先用同口径验证“按 MemorySet 拆 retired batch / 按 ASID 精确
  shootdown”对 full invalidation 与进度的影响；与此同时，PageCache 满容量淘汰和 ext4
  read/lookup/namespace 锁内工作是后续最强的两个非架构候选。本轮只建立基线，
  没有根据单个短窗口修改实现或宣称 wall-time 加速比。

## 2026-08-13 线上提交与本地比赛入口收敛（当前工作树）

- **线上入口**：顶层 `make all` 固定等价于 `submit`，顺序生成 `kernel-rv`、`kernel-la`、
  `disk.img`、`disk-la.img`。提交辅助盘固定从 `respos/` 构建，并在构建前验证 profile 为
  `mode=auto`；launcher 根据平台根盘自动识别阶段，决赛 CAgent/BuildStorm 脚本优先，随后检查初赛
  musl/glibc basic 脚本，未知镜像回退 preliminary。该构建路径不访问本地大镜像、不运行 QEMU。
  Makefile 全局 `.NOTPARALLEL`，避免 RV/LA
  共用 `os/.cargo/config.toml` 与 `user/.cargo/config.toml` 时发生交叉嵌入。
- **本地入口**：新增 `prepare-pre-images`、`run-rv-pre`、`run-la-pre`、`run-rv-final`、
  `run-la-final`、`run-rv-diagnostic`、`run-la-diagnostic`。初赛镜像从保留的 `.xz` 恢复为
  `sdcard-*-pre.img` 并检查 `basic_testcode.sh`；决赛镜像检查 CAgent/BuildStorm 两个官方脚本；
  三种 mode 使用 `/tmp` 下不同的 x1 辅助盘且都以 `-snapshot` 启动。
- **testrunner 边界**：`testrunner` 只负责初赛全量组、LTP 筛选及专项诊断，并输出初赛 judge group
  marker；决赛 final 由 `contest_launcher` 直接顺序运行 `/glibc/cagent_testcode.sh` 和
  `/glibc/buildstorm_testcode.sh`。旧 `eval` feature 不再选择启动路径，默认 user feature 已清空。
- **验证**：`RUSTUP_TOOLCHAIN=nightly-2025-01-18 make check-submit` 通过，确认平台 Rust 1.86
  兼容的 RV64/LA64 release 构建和四个提交产物，两个辅助盘均含 `mode=auto`。同一提交辅助盘分别
  搭配 RV64/LA64 初赛与决赛官方根盘启动：两架构初赛都自动进入 `testrunner` 并越过 basic 组，
  两架构决赛都自动进入 CAgent、10/10 pass，并继续通过 BuildStorm toolchain/minibuild 前置门禁；
  RV64 决赛不挂载 x1 时也从 profile 缺失进入自动检测。日志为
  `/tmp/respos-{rv,la}-auto-{pre,final}.log` 与 `/tmp/respos-rv-auto-no-profile.log`。这些限时烟测
  只验证阶段分派，不代替正式资源下的完整 BuildStorm。显式 Make 入口烟测中，
  LA preliminary 正确进入 `contest_launcher → testrunner` 并跑完 basic-musl/basic-glibc；RV final
  正确进入官方两个脚本。平台 score/rank 仍为 `待验证`。
- **Rust 1.86 LTO 历史 A/B（旧归因已被 2026-08-14 DMA 结论取代）**：平台同版 rustc 的 RV64
  `lto="thin"` 无 feature 内核可编译、启动，
  但两个冷 `-snapshot` 运行均使 `simple_llm_server`、mount、rustc/cargo 稳定 SIGSEGV，CAgent 0/10，
  BuildStorm toolchain/minibuild 失败；同源码 Rust 1.89 thin-LTO 内核则 CAgent 10/10。Rust 1.86 关闭
  LTO 后，RV64 CAgent 10/10 且 `BUILDSTORM_TOOLCHAIN/MINIBUILD ok`；LA64 12 hart 同样 CAgent 10/10
  且两个 BuildStorm 前置门禁通过。当时因此把双架构 release config 固定为 `lto=false`；后续已确认
  成败来自 LTO/feature 改变栈布局后是否触发 VirtIO 跨非连续物理页 DMA，并非已证明的 Rust 编译器
  缺陷。`lto=false` 暂保留为提交保守项，但不再视为根因修复；完整 BuildStorm 和平台正式资源/计时
  仍待验证，不能由短回归外推。

## 2026-08-13 状态收口与双线推进基线（当前工作树）

- **代码与工作树**：当前分支为 `main`，代码基线为 `1788fa2`（已包含学长 `0c21575` 和自动比赛
  镜像识别）。本次收口开始时
  `git status --short` 为空；最新代码已按课程平台实际使用的 Rust 1.86 nightly 兼容基线完成
  RV64/LA64 顺序构建。下文更早日期的“当前工作树”“下一步”和“仍阻塞”均是历史执行记录；若与
  本节冲突，以本节及其后更新为准。
- **外部评测阻塞**：课程评测平台当前暂不可用，因此当前基线的 score/rank、正式镜像
  与宿主耗时都标记为 `待验证`。平台不可用不计为代码失败，也不能用本地结果替代平台通过；恢复后
  首个动作是保存平台日志、工具链版本、镜像/命令口径和结果，再决定是否产生新的代码任务。
- **已闭合主线**：RV64 16 GiB/8 核本地 final 路径已完成 CAgent 10/10 和完整 BuildStorm；LA64
  12 GiB/12 hart 已完成完整 CAgent/BuildStorm，并完成同步 shootdown、ASID、FP/LSX first-use、
  TLB residency、按地址空间 frame 退役、页表页 root-switch completion 和 range 传播。op=5 在完整
  final 中导致内存破坏，已通过单变量 A/B 回退为安全的 op=4。Linux/POSIX Phase 0--3 已闭合，
  Phase 4 主体已由 Linux 对照 probe、
  双架构构建和 RV64 BuildStorm 覆盖。上述结论只适用于各节记录的 commit、镜像、QEMU 参数和日期。
- **当前未闭合主线**：一是 LA 架构与 BuildStorm 性能/正式 36 GiB 验证；二是 Linux/POSIX Phase 5
  的 MM、task/signal、IPC/network 语义。Phase 6 的大规模调度器、allocator、异步 I/O 重构仍由数据
  触发，不因平台暂不可用而提前展开。

### 2026-08-13 Phase 5 iperf daemon 后 iozone timer 停滞修复

- **根因**：glibc iozone throughput 打印 initial writers 的 `Min xfer` 后先执行 `sync()`，随后
  `sleep(2)`。诊断确认 `sync()` 已返回，nanosleep deadline 也已登记；真正阻塞者是遗留的
  `iperf3 -s -D`。daemon 在 inet socket 上调用 `poll()`，而 TCP/UDP `FileOp` 尚不支持事件式
  poll waiter，`block_for_poll()` 因而在同一次 kernel syscall 内持续 yield。普通 kernel timer trap
  只重编程 tick，boot hart 又一直带着 current task，原有 user/idle 两类高层 timer 安全点均不会运行，
  iozone 的 nanosleep 永远无法到期。此前关于 `wait4`、SIGCHLD、daemon reparent 的联合根因假设已否定。
- **修复边界**：新增 timer-service-hart 专用的显式 syscall 安全点，按 monotonic millisecond 至多扫描
  一次 futex/nanosleep/timerfd/itimer/POSIX timer registry；inet `ppoll/pselect` 非事件 fallback 以及
  TCP/UDP blocking retry 都在不持有 socket/task/signal/timer 锁的位置消费延迟 timer work。没有修改
  比赛 runner、测试顺序或 daemon 生命周期。
- **回归探针**：`net_timer_progress_probe` 同时保留阻塞 TCP accept 和模拟 iperf 的 UDP socket
  infinite `ppoll`，父进程的 100 ms nanosleep 必须打印 `NET_TIMER_PROGRESS_WAKE` 与
  `NET_TIMER_PROGRESS_PROBE_PASS`。RV64/LA64 release、4 GiB、1 hart 均通过；两架构的
  `net_loopback_smoke`、`socket_phase5_probe`、`task_a_clock_probe` 也通过。
- **比赛顺序验证**：RV64 release、`img/sdcard-rv-pre.img`、4 GiB/1 hart、snapshot 下按
  `/musl/iperf_testcode.sh → /glibc/iperf_testcode.sh → /glibc/iozone_testcode.sh` 运行，两组 iperf
  六项均 success，iozone 越过原卡点并输出 `#### OS COMP TEST GROUP END iozone-glibc ####`。
  LA64 同配置用 `iperf3 -s -D → iozone -t 4 -i 0 -i 1 -r 1k -s 1m` 完成 initial writers、
  rewriters、readers、re-readers。正式完整 preliminary runner 与平台结果仍 `待验证`。
- **构建证据**：`RUSTUP_TOOLCHAIN=nightly-2025-01-18 make build-rv RV_MODE=release` 与
  `RUSTUP_TOOLCHAIN=nightly-2025-01-18 make build-la LA_MODE=release` 顺序通过。适用代码基线为
  `51bed0e` 加本节工作树；未提交的 Makefile/log 文档和 PPT 资料保持原样。

### 双线分工与共享边界

| 推进线 | 当前负责人 | 当前任务包 | 本地退出证据 | 平台恢复后的补验 |
| --- | --- | --- | --- | --- |
| 架构/性能线 | 学长 | LA Global mapping 保持 default-off 正向候选；保留 range+op=4 安全边界；转入 BuildStorm ext4 E2 审计；36 GiB 启动 | 双架构顺序构建，LA 12-hart shared-MM/Phase3/ASID churn、FS/写回专项、固定窗口 A/B 与完整 BuildStorm | LA `-m 36G -smp 12` 正式镜像和时限；必要时 RV64 正式回归 |
| Phase 线 | 当前维护者 | 继续 Phase 5，并独立维护 POSIX 语义覆盖任务；先收敛与架构代码低耦合的 IPC/network，再做 task/signal、基础 POSIX 缺口；待 TLB/MM 接口稳定后实现 mmap EOF/truncate/SIGBUS | POSIX 覆盖矩阵、Linux 对照 probe、RV64 专项与 SMP 回归、LA/RV 顺序构建；高风险修改补 shared-MM/资源闭环 | 正式镜像的完整 workload、LTP/比赛 runner 与平台计时 |

默认文件边界如下：架构线拥有 `os/src/arch/**` 以及 LA SMP/TLB/ASID 的底层协议；Phase 线拥有
`os/src/net/**`、`os/src/signal/**`、相应 syscall/用户 probe 和 Linux 对照。`os/src/mm/memory_set.rs`、
`os/src/task/{task,processor,scheduler}.rs`、trap context、公共 arch API 和本状态页是共享集成面；两条线
改动这些文件前先约定接口和验证责任，同一时段只保留一个写入者，另一方以可审查 patch 接入。

### 双线并行新增任务：BuildStorm ext4/PageCache 关键路径

- **负责人和目标**：架构/性能线负责测量并降低 BuildStorm 的 ext4 read/lookup 与 PageCache
  refill/eviction 关键路径；完整方案和门禁见 [buildstorm-smp-plan.md](./buildstorm-smp-plan.md)。该任务
  可以跨 `fs/ext4`、PageCache、VFS/file/namei 和 perf 计数完成闭环，不以文件隔离牺牲实现完整性。
- **当前证据边界**：历史 LA 30 秒窗口 ext4 hold 约 9.217 CPU 秒但 wait 仅约 0.017 秒，同时
  PageCache 满 32768 页并 eviction 24135 次。故第一步是当前 HEAD 的同口径基线和锁内阶段细分，
  不是直接拆全局锁；lwext4 C 层线程安全未证明前保留唯一 `EXT4_OP_LOCK`。
- **给 Phase 线保留的空间**：Phase 线可独立继续 IPC/network 与 task/signal。涉及权限、namespace、
  metadata/time、writeback error、fsync/syncfs、mmap EOF/truncate/SIGBUS 的可观察语义仍由 Phase 线
  定义；性能线触及相同状态机前先共享接口说明和 baseline。PageCache、inode generation/identity、
  dirty-owner 和 truncate/writeback 是共享协议，同一时段单写入者、双方共同门禁。
- **当前退出标准**：先完成 E0 测量；E1 只做锁外准备/发布、可证明的 generation fast path 与连续
  read/fill 合并。只有缩放数据证明 wait 随 hart 显著增长才进入 E2 锁域并行。每项需双架构构建、
  FS/写回/资源专项和固定窗口 A/B；完整收益低于 5% 时不扩大高风险重构。

### POSIX 语义覆盖任务（待推进，Phase 线）

该任务独立于“增加 Linux syscall 数量”：以 POSIX.1-2024 源级接口和 musl/glibc 可观察行为为范围，
维护“已支持 / syscall 存在但语义未闭合 / libc 组合路径待验证 / 可选扩展 / 不支持”覆盖矩阵。

1. 基础接口先补明确缺项 `getsid()`，并为 termios/job control、线程组 exit/exec、signal restart、
   mmap EOF/truncate/SIGBUS、socket flags/timeout/`SO_ERROR` 建立 Linux 对照 probe；已列入 Phase 5 的
   子系统工作直接复用，不建立第二套实现任务。
2. `pthread_*`、`sem_open()`、`shm_open()`、`aio_*`、`posix_spawn()` 先验证 libc 组合路径，不能因没有
   同名 syscall 就判定缺失，也不能因程序能启动就判定完整支持。
3. POSIX message queue、`mlockall()`/`munlockall()` 和 XSI SysV message/semaphore 单列为可选扩展，
   由 LTP、比赛 workload 或明确需求触发优先级，不抢占基础语义闭合。
4. 每个条目的本地退出证据至少包括规范契约、Linux baseline、RespOS expected-fail/通过 probe 和
   RV64/LA64 顺序构建；涉及阻塞、signal、task 或 MM 时追加对应 SMP/资源闭环专项。
5. 课程评测平台恢复前可以更新本地覆盖状态，但正式镜像、LTP 总体结果和平台通过保持 `待验证`。

### 平台不可用期间的执行顺序

1. 架构线先以当前本地镜像做 LA 12-hart 无 feature 完整运行；资源不足时使用同镜像、同窗口的
   `1/3/6/12` 缩放与 `perf_counters` 定位，不能把短窗口写成正式成绩。
2. Phase 线先补 Linux 对照和 RespOS probe，再依次推进 socket timeout/nonblocking connect/MSG flags/
   poll、遗留 daemon 与 wait/signal 生命周期；每个主题独立提交和验证。
3. task leader exit/non-leader exec、`wait4` 之外的 `SA_RESTART`/process-pending 等跨 task/signal
   项单独设计；不得和网络状态机或 scheduler 性能重构混在同一修改中。
4. mmap EOF/truncate/SIGBUS 先完成契约、probe 和 VMA/inode identity 设计；底层 shootdown/frame
   completion 已稳定，但实现前仍须和架构线约定 `MemorySet` 接口与验证责任。
5. 平台恢复后先复评未经平台确认的当前 HEAD，再分别补两条线的正式镜像门禁；平台结果只更新
   `current-status.md`，稳定的新不变量/决策再同步到 `architecture.md`/`decisions.md`。

## 2026-08-13 课程平台 Rust 编译器兼容性（当前工作树）

- **平台证据**：课程评测于 2026-08-13 的 `make all` 在 RV64 内核阶段使用
  `rustc 1.86.0-nightly (2025-01-17)`；日志中的“缺少 score/rank”是平台对编译中断的
  外层提示，实际结果为 `Compile Error`、`score: 0`。编译器不支持 `let_chains` 和
  `unsigned_is_multiple_of`，因此拒绝了 13 处新语法/API。
- **兼容基线**：`os` 与 `user_lib` 的 `rust-version` 固定为 1.85；内核代码不得引入
  Rust 1.86 nightly 尚未稳定的标准库 API 或需要 feature gate 的语法。相关条件均改写为
  嵌套 `if` / `match` 和取模对齐检查，保持原控制流与错误语义。
- **本地复现环境**：`nightly-2025-01-18` 实际报告同一版本字符串
  `rustc 1.86.0-nightly (6067b3631 2025-01-17)`，并安装 RV64、LA64 bare-metal targets。
  在受限环境外顺序执行 `RUSTUP_TOOLCHAIN=nightly-2025-01-18 make build-rv` 和
  `RUSTUP_TOOLCHAIN=nightly-2025-01-18 make build-la` 均成功（仅保留既有 target-feature
  warnings），已生成 `kernel-rv` 与 `kernel-la`。首次沙箱内尝试曾被宿主 seccomp 在 lwext4 的
  `riscv64-linux-musl-gcc` 调用处中止（`Bad system call`），该限制不影响受限环境外的验证结论。

## 2026-08-12 LA SMP 阶段 1E：按 TLB residency 缩小远端失效目标（当前工作树）

- **实现**：LA `MemorySet` 新增独立 `tlb_hart_mask`。scheduler 激活地址空间时同时记录 active 与
  residency；普通 PTE writer 向自上次同步失效后加载过该 ASID 的 residency 集合发请求，完成后在
  MemorySet 写锁保护下把 residency 收缩回 active。RV64 的 active-mask/SBI RFENCE 路径未改变。
- **回收安全性**：LA retired data frame 仍为全局批次，可能混有多个 MemorySet。只要本轮冻结到非空
  批次，就继续对全部 online hart 完成全量失效后才释放；只有不释放旧 frame 的 PTE 更新才缩小目标。
  不能直接使用当前 active mask，因为已切到 idle 的 hart 仍可能缓存同 ASID 的旧项。
- **正确性门禁**：LA `-m 12G -smp 12` 双 `smp_shared_mm_probe` 各 100 轮和 Phase3 30 轮通过；
  LA `-m 4G -smp 12` 以 BusyBox xargs 串行 exec 1200 个短进程并打印 `ASID_CHURN_STAGE_E_PASS`，pid
  达到 1204。默认 LA/RV release 顺序构建通过；RV `-m 4G -smp 8` shared-MM 100 轮通过。
- **性能计数**：相同 12-hart、30 秒 BuildStorm perf 窗口中，Stage C 基线 remote RFENCE 93800、
  full invalidations 1127163；本阶段为 7424 和 169422，分别约下降 92.1% 与 85.0%。两轮 user trap
  约 19.5 万/18.0 万、context switch 4155/4389，进度量级接近；该对比证明目标集合缩小，不等同于
  BuildStorm wall-time 加速比。本轮窗口含 3304 次 COW fault，未出现 frame 回收故障。
- **剩余边界**：本地和远端 handler 仍执行全量 `invtlb op=0`；按 ASID/VA 精确失效、Global kernel
  映射与全局 retired batch 的按地址空间拆分仍需分别验证，不能由本阶段结果外推。

## 2026-08-12 LA SMP 阶段 1D：FP/LSX 首次使用门控（当前工作树）

- **实现**：LA 用户 trap frame 从 800 字节扩为 816 字节，新增扩展状态激活标记和对齐槽。exec 初始
  关闭用户 EUEN.FPE/SXE；首次 FloatingPointUnavailable/SimdUnavailable 激活任务但不推进 ERA，
  返回路径恢复零状态并重试。未激活任务的 trap 不执行向量/浮点保存，激活后仍使用原 eager 隔离。
- **正确性边界**：这是 per-task first-use gating，不是跨 hart lazy-owner。trap entry 在读到 EUEN 未
  启用时必须先跳过所有 FP/LSX 指令；进入 Rust 内核前重新开启扩展。fork 复制整个 trap frame，exec
  重置激活标记。内核 trap 的 272 字节汇编帧未改变；signal mcontext 的 FP/LSX ABI 仍待补齐。
- **LA 12-hart 门禁**：QEMU 10.0.2、`-m 12G -smp 12 -snapshot`、LA pub x0 与临时 diagnostic x1，
  online mask `0xfff`。官方动态 BusyBox 执行成功；`smp_shared_mm_probe` 100 轮、`smp_phase3_probe`
  30 轮通过；从 `/glibc` 启动原始 CAgent 脚本，10 项全部 pass、group end、退出码 0。
- **计数结果**：同配置 `perf_counters` 窗口在 reset 后运行 BusyBox true/echo/cat，记录 user traps 535、
  extension eager saves 62；旧实现每次 user trap 均保存，故该窗口减少 473 次（约 88.4%）扩展保存。
  这是小型功能窗口的操作次数，不代表 BuildStorm wall-time 加速比。实际使用扩展的任务仍承担一次
  unavailable trap 和后续 eager 保存成本。
- **RV 边界**：变更仅位于 `arch/loongarch64`。RV64 release 顺序构建通过，并在 `-m 4G -smp 8`
  diagnostic 启动后通过 `smp_shared_mm_probe` 100 轮。LA/RV 构建仍须顺序执行，避免共享 Cargo 配置污染。

## 2026-08-12 LA SMP 阶段 1C：ASID 与免逐切换全失效（当前工作树）

- **实现**：ASID 0 保留给 kernel/idle，用户 MemorySet 使用 1--1023。软件 MMU token 携带 root 与
  低 10 位 ASID；`__switch` 保存时屏蔽 CSR.ASID 的只读 ASIDBITS 字段，恢复 PGDL/PGDH/ASID 后
  不再执行逐切换 `invtlb op=0`。ELF loader、activate、fork/clone 均传递完整 MemorySet token。
- **生命周期**：退出路径确认地址空间没有外部 CLONE_VM owner、回收数据页并完成全在线 TLB 屏障后，
  立即幂等退役 ASID，不依赖 zombie/deferred TCB 的最终 Drop。编号耗尽时冻结 retired 位图，完成
  本地和全在线 TLB 失效后才批量复用；构造失败或 exec 旧空间仍由 Drop fallback 退役。
- **复用门禁**：LA `-m 4G -smp 12` 下，以 BusyBox xargs 串行 exec 1200 个短进程，命令退出 0 并
  打印 `ASID_CHURN_PASS`，进程号达到 1204。随后双 `smp_shared_mm_probe` 各 100 轮通过，
  `smp_phase3_probe` 30 轮通过，证明 rollover 后共享 MM、fork/exec/wait 和网络/pipe 仍工作。
- **BuildStorm/计数**：LA 12 GiB/12 hart 无 feature 短测通过 toolchain/minibuild 并完成 untimed
  Cargo（正式 final 为 20.57 秒）并进入 timed arceos build。perf 30 秒窗口记录 context switches
  4155、local sfences 93861、remote rfences 93800、full TLB invalidations 1127163；ASID 消除了逐切换
  失效，但 PTE fault/update 的全在线 shootdown 仍占绝对多数，不能宣称总体大幅提速。
- **容量边界**：当前只复用已退役编号；若同时存在超过 1023 个仍可运行的独立 MemorySet，会返回
  `ENOMEM`。解除该限制需要带 active-ASID 保留的 generation rollover，不能直接覆盖在用编号。
- **双架构边界**：ASID 分配、CSR 与切换汇编均为 LA 条件路径；RV64 保持 SATP/OpenSBI RFENCE。
  双架构 release 构建通过；RV64 8-hart shared-MM 100 轮通过，LA final CAgent 10/10 后进入 BuildStorm。

## 2026-08-12 LA SMP 阶段 1B：同步远端 TLB shootdown（当前工作树）

- **实现**：LA IOCSR IPI vector 1 专用于 TLB shootdown。每个目标 hart 有独立 request/ack
  generation 槽；并发发起者按 hart id 递增顺序认领目标槽，发布页表修改后发送 IPI，并等待所有
  目标执行本地 `invtlb op=0` 后确认。槽在确认前不复用，因此 IPI vector 合并不会丢失请求。
- **等待进展**：LA 内核通常保持 `CRMD.IE=0`。MemorySet RwLock 与项目普通 SpinMutex 的竞争等待
  会轮询 pending IPI，使持有页表写锁并等待 shootdown 的 hart 不会与等待该锁的远端 hart 构成
  环；RV64 路径保持原锁行为和 OpenSBI RFENCE。
- **失效语义拆分与页帧寿命**：地址空间 `activate()` 只执行当前 hart 的 root-switch flush；只有实际
  修改 PTE 的 `flush_tlb()` 才同步失效远端，避免启动互等和冗余 RV RFENCE。LA 在清除/替换 PTE 后
  先把旧数据页放入全局退役批次，等所有在线 hart 确认全量失效后才归还分配器；因此当前 LA 使用
  online mask，而不是尚未配合 ASID 生命周期验证的 active-hart mask。RV64 保持原回收与 RFENCE 路径。
- **专项验证**：QEMU 10.0.2、LA `-m 4G -smp 12` online mask `0xfff`；
  `smp_shared_mm_probe` 单实例 100 轮通过，两个实例并发各 100 轮通过；`smp_phase3_probe` 30 轮通过。
  RV64 `-m 4G -smp 8` 的 `smp_shared_mm_probe` 100 轮通过；双架构 release 构建通过。
- **BuildStorm 短测**：LA 12 GiB/12 hart diagnostic 单跑通过 toolchain/minibuild、完成 untimed
  `tg-xtask`，并进入 timed arceos 编译。修复 SpinMutex 的 IPI polling 前负载停在 toolchain 启动；
  修复后继续输出多项 crate 编译。该结果是活性短测，不是完整成绩。

## 2026-08-12 LA SMP 阶段 1A 收口：稳定次核高半区启动（当前工作树）

- **实现**：`jump_to_high_half()` 不再在未声明 clobber 的 `$t0` 中装载 `KERNEL_BASE`，而是让
  编译器分别分配 high-half target 与 base 输入寄存器，避免代码布局变化时 target 被覆盖。LA
  high-RAM huge direct map 暂不设置 Global；当前所有地址空间仍使用 ASID 0 且切换时完整
  `invtlb op=0`，Global/ASID 必须等同步远端 shootdown 完成后整体引入。
- **启动矩阵**：QEMU 10.0.2、LA pub x0、临时 diagnostic x1、`-snapshot -m 4G`；
  `-smp 1/3/6/12` 分别报告 online mask `0x1/0x7/0x3f/0xfff`，3-hart
  `/proc/cpuinfo` 列出 processor 0--2。RV64 `-m 4G -smp 8` 同样进入 diagnostic shell，
  `/proc/cpuinfo` 列出 0--7。
- **短程决赛回归**：LA pub x0、正式 final x1、`-snapshot -m 12G -smp 12` 的 40 秒宿主限时运行中，
  CAgent 10/10 pass，BuildStorm toolchain/minibuild 通过，untimed `tg-xtask` 完成并进入
  `arceos-helloworld` timed build；这是活性短测，不是完整 BuildStorm 成绩。
- **边界**：未完成的 LA 远端 TLB shootdown/MemorySet 锁轮询实验已从本阶段撤下，避免把尚未通过
  多核启动门禁的协议混入稳定基线。下一阶段仍需独立实现 request/ack，并通过 shared-MM 专项后才能
  宣称通用 LA SMP 页表修改安全。

## 2026-08-12 LA SMP 阶段 1A：动态内存与 2 MiB direct map（当前工作树）

- **实现**：LA boot hart 在关闭 DMW0 前通过 QEMU virt `fw_cfg` MMIO 的
  `FW_CFG_RAM_SIZE` 读取实际 guest RAM，按 256 MiB low RAM + high RAM 计算物理末址；解析失败时
  保留旧 12 GiB 上限，支持上限钳制为比赛 36 GiB。该路径完全位于 LoongArch 条件编译代码，RV64
  仍使用 OpenSBI FDT。
- **页表启动成本**：LA 正式 high-RAM direct map 从逐个 4 KiB PTE 改为 PMD 2 MiB huge leaf，
  huge entry 使用 bit 6；软件 TLB refill 遇到 huge leaf 时不再执行仅适用于
  table pointer 的 `-1`。这使 12 GiB/12 hart 无根盘启动在约 6 秒内到达预期 root-device panic，
  避免按 36 GiB 建立约九百万个 4 KiB 映射。
- **动态容量验证**：QEMU 10.0.2、LA pub/preliminary x0、diagnostic x1、`-snapshot -smp 12`；
  `-m 4G` 进入 shell 后 `/proc/meminfo` 报 `MemTotal: 4194304 kB`、online mask `0xfff`；
  `-m 12G` 同样以 online mask `0xfff` 进入 shell。4 GiB 结果证明不再固定使用 12 GiB 上限。
- **短 BuildStorm**：LA pub x0、12 GiB/12 hart 的 30 秒 guest `timeout` 窗口通过 toolchain 与
  minibuild，进入 `arceos-helloworld` timed build；timeout 只结束父 shell 后仍有子进程输出，属于
  已知进程组/timeout 语义缺口，因此该窗口仅作活性回归，不作计时成绩。
- **双架构门禁**：LA/RV64 no-feature release 均构建通过；RV64 `-m 4G -smp 8`、diagnostic x1
  启动到内嵌 shell。完整 36 GiB 启动受本地主机内存限制仍待评测平台验证；下一阶段仍是 LA
  远端 TLB shootdown/ack。

## 2026-08-12 LA SMP 阶段 0 性能观测基线（当前工作树）

- **诊断边界**：`perf_counters` 新增 scheduler lock 获取/等待/ready peak、IPI 接收、完整 TLB
  失效、LA user trap 分类和 eager FP/LSX 保存恢复对数；无 feature 时调用静态消除。正常关机时仅
  对 perf kernel 输出一次完整快照。`contest_launcher` 新增显式 `mode=diagnostic` 进入内嵌
  `user_shell`，默认 `respos/profile` 仍为 `mode=final`，`make all` 的提交行为不变。
- **12-hart 专项**：QEMU 10.0.2、LA pub x0、无 x1、`-snapshot -m 12G -smp 12`，已有
  `TASK_A_CLOCK_PROBE` 连续 20 轮全部通过并正常关机。该短程记录 643 次 user trap/扩展状态 eager save、
  52 次 context switch、172 次完整 TLB 失效；scheduler lock 657 次获取，累计等待 760054 ticks
  （100 MHz 下约 7.60 ms），ready peak 为 1。
- **30 秒 BuildStorm 窗口**：使用 diagnostic shell 预排 reset、
  `/glibc/busybox timeout 30 /bin/bash /glibc/buildstorm_testcode.sh`、读取 proc 和 quit。toolchain 与
  minibuild 通过，窗口在旧 pub 镜像的 untimed `tg-xtask` 中由 timeout 结束，不能作为正式成绩。
  快照为 user traps `194682`（syscall `84881`、page fault `106528`、timer `3268`），eager
  extension eager saves `194682`；context switches `4141`，完整 TLB invalidations `155203`；scheduler
  lock `83925` 次、累计等待 `1292393` ticks（约 12.9 ms）、ready peak 3。同期 ext4 全局锁 hold
  `818411275` ticks（累计约 8.18 CPU 秒），heap alloc/dealloc lock wait 合计约 0.30 CPU 秒。
- **当前判定**：该窗口不支持优先重构公共 scheduler；LA 完整本地 TLB 失效、eager FP/LSX 和 ext4
  串行工作明显更值得继续测量。正确性顺序仍是先完成 LA 远端 shootdown，再考虑 ASID/精确失效；
  不因性能数据跳过共享 MM 门禁。
- **构建门禁**：RV64/LA64 无 feature release 与两架构 `perf_counters` release 均构建通过；LA
  12-hart clock probe 正常关机。一次尝试直接运行缺失的 `/musl/task_a_perf` 返回 `ENOENT`，不计为
  probe 结果；一次错误使用不存在的 `/bin/busybox` 被 shell 轮询污染，同样从基线排除。
- **正式路径短回归**：最终源码的无 feature 内核分别以 RV64 `-m 4G -smp 8` 和 LA64
  `-m 12G -smp 12`、final x1、`-snapshot` 限时 35 秒运行；两架构 CAgent 均 10/10、BuildStorm
  toolchain/minibuild 均通过并进入后续构建，随后由宿主 timeout 停止。该结果只证明阶段 0 没有改变
  默认 final 启动与短程活性，不是完整 BuildStorm 回归。

## 2026-08-12 LoongArch 重复 TLB 失效去除与限时回归（当前工作树）

- **实现**：LA `switch.S` 与 `register::mmu::flush_tlb()` 删除了紧随
  `invtlb op=0` 的 `op=3`。LoongArch Volume 1 明确定义 op=0 清除全部 TLB 项、op=3 只清除
  `G=0` 项，因此第二条严格冗余；本轮没有启用 ASID，也没有保留跨 root 的 global TLB 项。
- **被否决的实验**：曾分别尝试把共享 kernel half 标为 global 并在 switch 只清非 global 项、以及
  ready queue 没有 eligible competitor 时跳过 LA timer handoff。前一组合的两次冷启动在正式编译
  阶段分别出现 SIGSEGV 与 `double free or corruption`；仅保留跳过 handoff 后仍出现
  `free(): invalid pointer`。这些实验已全部回退。故障与这两个实验的独立因果仍为 `待验证`，不得
  把它们重新作为无门禁优化合入。
- **LA 限时回归**：QEMU 10.0.2，公开决赛 x0、final-profile x1，`-snapshot -m 12G -smp 12`。
  最终单 `invtlb op=0` 版本 online mask 为 `0xfff`，CAgent 10/10，通过 BuildStorm toolchain、
  minibuild 和 `tg-xtask` 预构建；正式编译越过此前实验的 allocator 崩溃点并推进到
  `ax-posix-api`，240 秒外层 timeout 时仍在运行。最终精确源码日志：
  `/tmp/respos-la-single-invtlb-final-smoke.log`。相邻版本（只多一个当前 task→idle 路径不会命中的
  same-root 比较，随后因无收益删除）420 秒推进到 `rustc-std-workspace-core/alloc`，日志为
  `/tmp/respos-la-single-invtlb-pub-smoke.log`。这些结果是正确性/活性限时门禁，不是完整
  BuildStorm 通过，也尚未量化相对性能提升。
- **构建门禁**：`make build-la LA_USER_FEATURES= LA_KERNEL_FEATURES=`、
  `make build-rv RV_USER_FEATURES=eval RV_KERNEL_FEATURES=`、kernel `cargo fmt --check` 与
  `git diff --check` 通过。

## 2026-08-11 LoongArch 12 GiB / 12-hart 完整决赛功能回归（当前工作树）

- **实现**：新增 QEMU-virt LoongArch IOCSR mailbox/IPI secondary 启动路径和 12 份 early/idle
  context；scheduler、processor、affinity、timer 与 `/proc/cpuinfo` 接入实际 CPUID/online mask。
  boot hart 在进入用户态前有界等待 secondary，并输出 online mask；本地 `make la` 和
  `make run-la-pub` 均默认 `LA_MEM=12G`、`LA_SMP=12`。
- **关键根因**：旧 LA `__switch` 连续用 `$t0` 写 PGDL/PGDH，但 `csrwr` 会把 CSR 旧值回写
  源寄存器，造成低/高半区 split-root。单核残留 TLB 会暂时掩盖它，SMP BuildStorm 则表现为
  内核高半区 fault、scheduler owner hart 消失和全局锁自旋。当前先复制 root 到 `$t1` 再分别写
  PGDL/PGDH；idle context 还会恢复已发布的 kernel root，并同时核对两者。
- **12-hart 证据**：QEMU 10.0.2，官方 `img/sdcard-la-pub.img` 为 x0、final-profile
  `disk-la.img` 为 x1，`-snapshot -m 12G -smp 12`。guest online mask 达到 12 bit；QEMU 的 12
  条 vCPU 线程均累计实际 CPU 时间，采样时最多 6 条受当前宿主并行度限制同时运行，QEMU 总体
  CPU 随构建阶段升至约 300%--900%。BuildStorm 最终报告 `cores=12`。
- **功能结果**：CAgent 10/10 pass、exit code 0；BuildStorm toolchain/minibuild 通过，正式编译
  输出 `BUILDSTORM_COMPILE mode=multi ok=true ... cores=12 ... arch=loongarch64`，group 正常结束，
  launcher 主动关机。完整日志：`/tmp/respos-la-smp12-clean.log`。
- **性能结论**：功能通过不等于满足评分时限。本地 Cargo 报 `147m 20s`，axbuild 报
  `8851.87s`，高于当前文档记录的平台整轮 6250 秒上限；后续仍需单独做 LA BuildStorm 性能优化。
  Cargo 的 global-cache last-use `out of range integral type conversion attempted` 警告仍出现，但
  未令本轮构建失败。
- **剩余正确性边界**：LA 页表切换会本地全 TLB flush，但尚无带完成确认的远端 TLB shootdown；
  本轮 BuildStorm 覆盖大量进程/pthread，只能证明该工作负载。`smp_shared_mm_probe` 已具备 LA
  clone 汇编入口，但跨核并发 `munmap/mprotect/MAP_FIXED` 专项仍标记 `待验证`。
- **构建门禁**：清理全部临时 lock/panic/trace 探针后，`make build-la ...` 与
  `make build-rv ...` release 均通过；完整运行后的最终源码仅再移除并发 secondary 单独打印、
  加强 PGDH root 核对，未重跑 147 分钟 BuildStorm。

## 2026-08-11 `make all` 产物 RV64 正式资源完整决赛回归（当前工作树）

- **平台口径**：当前比赛公告参数为 RV64 `-m 16G -smp 8`、LoongArch
  `-m 36G -smp 12`，整轮上限 6250 秒；LA 不是 16 GiB。本轮只验收 RV64。
- **构建与启动**：仓库根目录直接执行 `make all` 成功生成 `kernel-rv`、`kernel-la`、
  final-profile `disk.img`/`disk-la.img`。随后以 QEMU 10.0.2、`kernel-rv`、本地
  `img/sdcard-rv-pub.img` 为 x0、生成的 `disk.img` 为 x1，使用 `-m 16G -smp 8 -snapshot`
  及比赛 virtio-mmio/net/RTC 参数启动；8 个 hart 全部上线，launcher 读取 x1 profile 后进入
  final mode。
- **CAgent**：原始 `/glibc/cagent_testcode.sh` 输出 group start/end，factorial、fs-create、kernel、
  fs-usage、cpu、fs-readwrite、date、network、fs-search、fs-directory 共 10 项全部 pass；脚本
  exit code 0。单项约 352--674 ms。
- **BuildStorm**：launcher 在 CAgent 完成后才串行启动原始 `/glibc/buildstorm_testcode.sh`；输出
  `BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok`，正式阶段最终为
  `BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=0.00 cores=8 bytes=1681000 arch=riscv64`。
  Cargo 报告 `21m 38s`，axbuild 报告 `1310.01s`；本地脚本的 `elapsed_s=0.00` 仍不可用于评分计时。
  脚本 exit code 0，launcher 输出 `all final scripts finished; powering off`，QEMU 正常退出。
- **结论与范围**：该轮 heap/双盘基线能够通过“平台执行 `make all` 后启动 RV64”这一完整
  本地模拟。结果对应当前本地 pub 镜像，不替代评测方最终镜像哈希确认；该轮记录的 LA 单 hart
  限制已由上方 2026-08-11 LA SMP 记录取代，12 GiB 分段硬编码与正式 36 GiB 动态布局问题仍保留。

## 2026-08-11 LoongArch FP/LSX 用户上下文修复（历史基线，已由阶段 1D 取代）

- **根因**：LA 初始化时已开启 `EUEN.FPE/SXE`，官方 Rust 工具链也会执行 LSX 指令，但原
  `TrapContext` 和 user trap 汇编只保存通用寄存器、PRMD 与 ERA。timer、syscall 或调度发生后，
  用户 FP/LSX/FCSR/FCC 状态没有任务级所有权，BuildStorm 的 `rustc`/`cargo` 因坏指针 SIGSEGV。
- **修复**：LA user trap frame 从 272 字节扩为 800 字节，eager 保存/恢复 32 个 128-bit
  `vr`（同时覆盖标量 FP）、FCSR0 与 FCC0..7；exec 初始化为零，fork/clone 随完整
  `TrapContext` 复制。Rust 结构对汇编使用的所有关键偏移带编译期断言。
- **因果回归**：QEMU 10.0.2，`-m 12G -smp 12 -snapshot`，官方 LA pub x0 与 final-profile x1，
  `LA_KERNEL_FEATURES=fault_trace`。CAgent 10/10 pass；BuildStorm 的 `rustc --version`、
  `cargo --version`、`BUILDSTORM_TOOLCHAIN`、`BUILDSTORM_MINIBUILD` 均通过，不再出现此前稳定的
  SIGSEGV，并进入正式 arceos 构建的 `compiler_builtins`/`core` 编译。QEMU 持续约 100% 单核 CPU；
  运行 10 分 23 秒后按用户要求由宿主停止，因此没有 BuildStorm group end，不能宣称完整通过。
  日志：`/tmp/respos-la-lsx-context.log`。
- **当时的边界与代价**：该基线为全量 eager 策略，每次 user trap 固定搬运约 528 字节扩展状态；现已
  由上方阶段 1D 的首次使用门控取代。LoongArch 用户信号 mcontext 仍只公开/恢复 GPR，signal handler
  的完整 FP/LSX ABI 另行补齐；这不影响该历史记录的因果结论。

## 2026-08-11 kernel heap 移出静态 BSS（当前工作树）

- **实现**：删除 RV64/LoongArch 的静态 `HEAP_SPACE` 和 `HEAP_BITMAP`。RV64 在页对齐的
  `ekernel` 后预留 bitmap/heap；LoongArch 12 GiB QEMU RAM 按实测布局拆为 256 MiB low RAM 与
  11.75 GiB high RAM，bitmap/256 MiB heap 放在 `0x80000000` 起始的 high RAM。frame allocator
  支持多个不连续区间，并排除内核与 heap 预留区。现有 IRQ-safe buddy 热路径不变。
- **ELF**：release `kernel-rv` 约 8.0 MiB、最后一个 BSS PT_LOAD `MemSize=532828`；
  `kernel-la` 约 6.3 MiB，静态 heap 不再扩大 `ebss/ekernel` 或触发整段 `clear_bss()`。
- **验证**：`make build-rv`、`make build-la`、kernel `cargo fmt --check` 和 `git diff --check`
  通过。RV64 QEMU 10.0.2、`-m 512M -smp 1 -snapshot` 从 preliminary launcher 运行完
  basic-musl/basic-glibc 并进入 libcbench；LoongArch 同内存/SMP 配置进入首个 `basic-musl`。
  两轮由宿主主动停止，只证明启动、堆分配和早期用户态路径，不代表完整测例通过。
- **256 MiB 边界**：同一 `kernel-rv` 在 `-m 256M` 下已由 QEMU 成功装载并进入内核，随后按设计
  报告 `reserved_end=0x90c0b000, memory_end=0x90000000` 后停止；这证明旧 ELF/DTB 装载问题已经
  消失，剩余限制是固定 256 MiB heap 加 bitmap 和内核本体无法同时容纳在 256 MiB RAM 中。
- **LoongArch 决赛诊断**：正式 `-m 36G -smp 12` 在当前宿主由 QEMU 创建 RAM 时即报
  `Cannot allocate memory`，未进入 guest。原 BusyBox `AddressError era=badv=0x4c0000c128f300c6`
  已确认由 TLB refill 的 `construct_invalid` 分支错把目录缺失构造成有效映射导致：GOT 页没有进入
  Rust page-fault handler，而是别名到 ELF 首页。改为向 `TLBRELO0/1` 明确写零后，文件页懒加载
  恢复；清理全部临时探针后，QEMU 10.0.2 `-m 12G -smp 12`、官方 LA pub 根盘加
  `disk-la.img` 已进入 `/glibc/cagent_testcode.sh` 并输出
  `#### OS COMP TEST GROUP START cagent-glibc ####`。原卡死根因是所有用户任务 blocked 后，LA
  调度器以 `IE=0` 执行 `spin_loop()`：timer 已在 `ESTAT.IS[11]` pending，却无法进入 trap 唤醒
  nanosleep/网络 timeout。当前 idle 路径临时开启中断并执行 `idle 0`，kernel idle timer trap
  处理 task timer，醒来后恢复内核 `IE=0` 约束。QEMU 10.0.2、`make la
  LA_FS_IMG=img/sdcard-la-pub.img LA_MEM=12G LA_SMP=12` 在约 18 秒内退出；CAgent 10 项全部 pass。
  随后的 BuildStorm 输出 group end，但 `rustc --version`、`cargo new` 和正式编译均 SIGSEGV
  （rc=139），属于剩余动态程序兼容缺陷。日志：`/tmp/respos-la-full-after-idle-fix.log`。
- **LoongArch 当时的资源限制（历史，已部分取代）**：该轮基线 task processor 使用
  `MAX_CPUS=1` 且没有 secondary 启动路径；现已由上方 LA 12-hart 实现取代。RAM high-end 仍按
  本地 12 GiB QEMU 布局硬编码，尚不支持公告的 36 GiB 动态布局。
- **保留边界**：这不是动态扩容 heap；预留容量在启动时仍固定且不能归还 frame allocator。
  LoongArch 12 GiB 上界和 RAM 分段当前按 QEMU 10.0.2 布局配置，尚未从固件/ACPI 动态发现；
  使用不同 `-m` 容量前必须补充探测或匹配配置。

## 2026-08-11 官方根盘 + 可选辅助 ext4 迁移（当前工作树）

- **实现**：RV64 virtio-mmio bus.0/1 和 LoongArch virtio PCI 均支持按 block-device index
  创建。x0 继续作为 `/`；x1 合法 ext4 时以独立 lwext4 device/mountpoint、
  `Ext4SuperBlock` 和 `(fs_id, ino)` cache identity 挂载到 `/respos`。无 x1 时不在官方
  x0 上创建 `/respos`。关机时辅助与根 superblock 都会尝试完成 journal/cache/virtio flush；
  一个设备失败不会阻止另一个设备的 shutdown 尝试。
- **guest launcher**：新增内嵌 `contest_launcher`，读取 `/respos/profile` 的
  `mode=preliminary|final`。profile 缺失/无效或 preliminary 时保持原内嵌 `testrunner`；final
  时用官方根镜像 `/bin/bash` 在 `/glibc` 中依次运行固定的 `cagent_testcode.sh` 和
  `buildstorm_testcode.sh`，每项 `waitpid` 完成后才启动下一项，最后主动关机。顶层
  `make all` 同时从 `respos/` 完整重建 16 MiB ext4 `disk.img` 和 `disk-la.img`。
- **RV64 运行验证**：QEMU 10.0.2、`kernel-rv`、`img/sdcard-rv.img`、`-m 1G
  -smp 1 -snapshot`。仅 x0 可进入原 `testrunner`；64 MiB 临时 ext4 x1 内含
  `profile -> /respos/final-runner` 和 RV64 `hello_world` ELF 时，串口输出
  `[initproc] trying entry /respos/final-runner` 及 `Hello, world!`；64 MiB 全零非 ext4 x1
  时无 panic 并进入原 `testrunner`。所有运行使用 snapshot，官方/初赛镜像未被该轮写入。
- **阶段分派验证**：`mode=preliminary` 的生成镜像在 RV64 初赛 x0 上输出 dispatcher 日志后进入原
  `testrunner`。临时切换为 `mode=final`、挂载 RV64 pub x0 后，CAgent 脚本完整输出 10 项 pass
  和 group end，launcher 观察到 exit code 0 后才启动 BuildStorm；BuildStorm 已输出 group start、
  rustc/cargo版本及 `BUILDSTORM_TOOLCHAIN ok`。该轮随后由宿主终止，不代表完整 BuildStorm 通过。
- **构建验证**：`make build-rv` 和 `make build-la` release 均通过。
- **LoongArch 历史启动诊断**：此前无串口输出发生在 `clear_bss()`：256 MiB 静态
  `KERNEL_HEAP_SIZE` 使 BSS 结束于约 `0x10aa7000`，超过 128 MiB early map 和 256 MiB
  板级低内存窗口。QEMU `-d in_asm,guest_errors` 显示清零循环最终跳到地址 0。将 LA 专用静态堆
  恢复为 64 MiB 后，QEMU 10.0.2、`-m 4G -smp 1`、x0+x1 已输出
  `contest_launcher` preliminary 日志、`[testrunner] start` 并进入首个 `basic-musl` 测例。
  该历史修复先降为 64 MiB；当前 heap storage 已进一步移出 BSS，并恢复为 256 MiB。

本文件是快速变化的状态页。更新测试结论时必须同时更新日期、提交和命令。

## 2026-08-11 Phase 5 前置 BuildStorm debug-traces 完整回归（基于 `76f7c61` 的本轮工作树）

- **配置与命令**：以
  `make build-rv RV_USER_FEATURES= RV_KERNEL_FEATURES=debug_traces` 构建 release kernel，QEMU 10.0.2
  直接在宿主以 `-m 16G -smp 8 -snapshot` 和 pub 镜像运行，实测 QEMU 为 `NI=-10/CLS=TS`；guest
  执行 `/glibc/buildstorm_testcode.sh`。完整串口日志为
  `/tmp/respos-buildstorm-phase5-debug-traces.log`（688459 bytes）。这是正确性/活性诊断，不是正式性能成绩。
- **结果**：依次观察到 `BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok` 与
  `BUILDSTORM_COMPILE mode=multi ok=true elapsed_s=0.00 cores=8 bytes=1681000 arch=riscv64`；脚本经最终
  `sync` 后退出 0。Cargo 报告 `20m 42s`，axbuild 报告 `1254.30s`；旧脚本的 `elapsed_s=0.00` 继续
  不作为计时依据。
- **trace 审查**：16 次带 sibling 列表的 exec 与 16 次 `exec remote-ack` 的 TID 多重集合完全一致；
  pipe poll/HUP、child exit 与 wait resume 在完整运行中持续推进。并发串口输出会交织并打碎部分
  group-exit 行，不能用其简单行数做严格配对，但全日志未出现 kernel panic、SIGSEGV、OOM、allocation
  failure、assertion failure 或 illegal instruction。此前第二轮的并行 rustc SIGSEGV 本轮未复现。
- **宿主边界**：采样中 QEMU RSS 约 1.5--2.9 GiB，宿主 available memory 约 9.0--9.4 GiB，swap 维持
  约 3.5 MiB，没有历史失败轮次的宿主内存压力。脚本结束后已退出 QEMU；诊断完成后须恢复无 feature
  kernel 再做正式门禁。

## 2026-08-11 Linux/POSIX Phase 5 mmap EOF/SIGBUS 审计（基于 `76f7c61` 的本轮工作树）

- **Linux 对照**：新增 `scripts/mmap_phase5_probe_linux.c`，分别覆盖 MAP_SHARED/MAP_PRIVATE：初始
  EOF 所在部分页尾部补零、下一完整页触发 SIGBUS；truncate 后已驻留但未 COW 的部分页尾部清零、
  完整越界页失效并触发 SIGBUS；映射后文件扩容且尚未 fault 的新页按当前 EOF 读取数据。补充对照还
  确认 MAP_PRIVATE 已 COW 的 EOF 部分页保留其匿名尾部字节，但新 EOF 之后的完整 COW 页仍失效并
  SIGBUS。宿主以 `cc -std=c11 -O2 -Wall -Wextra -Werror` 编译运行，全部 PASS。
- **当前 RespOS 差异**：新增 `mmap_phase5_probe`。RV64 no-feature release、QEMU 10.0.2、
  `-m 16G -smp 8 -snapshot`、宿主 `NI=-10/CLS=TS` 中，shared/private 的初始完整越界页和 truncate
  后已驻留完整越界页都继续可读并正常退出，没有 SIGBUS；private truncate 后未 COW 的同一部分页仍
  暴露旧字节，映射后扩容的新页也保持 mmap-time EOF 的零页；已 COW 的完整越界页同样未失效。探针
  打印七项 `MMAP_PHASE5_EXPECTED_FAIL` 和
  `MMAP_PHASE5 CURRENT DIFFERENCES CONFIRMED`，以非零退出，不能作为通过标记。
- **根因边界**：MAP_SHARED 当前在 mmap 时为整个窗口预建 frame，越过 EOF 的完整页因此已有有效 PTE；
  MAP_PRIVATE 的 `FileBacking.len` 固化 mmap-time EOF。truncate 会裁剪 PageCache/writeback，却没有按
  inode identity 扫描 live MemorySet、撤销越界 PTE 与私有 resident frame；当前 page-fault 路径也没有
  在映射页 offset 对照当前 inode size 后返回 SIGBUS fault 类型。
- **待协商方案**：建议 fault 时动态读取当前 EOF；mmap 时在 backing 缓存 `(dev, ino)`，truncate 成功
  并释放 File 锁后按 live task/MemorySet 去重扫描同 inode VMA，清除完整越界页并复用现有跨 hart TLB
  shootdown。先接受 O(live tasks × VMAs) 的低频扫描，不新增常驻 inode→VMA 反向索引。EOF 部分页
  还必须区分未 COW 文件页与已 COW 匿名页；当前 writable MAP_PRIVATE 首次 fault 即直接映射可写私有
  frame、没有 COW/dirty 状态，不能无条件清尾。该点需在实施前与用户确认取舍。

## 2026-08-11 Linux/POSIX Phase 5 AF_UNIX 与 poll 语义（基于 `76f7c61` 的本轮工作树）

- **pathname 与关闭语义**：AF_UNIX pathname `bind/listen/connect/accept` 已用 Linux 对照闭合；空队列
  非阻塞 accept 返回 `EAGAIN`，连接关闭后 read 返回 EOF、write 返回 `EPIPE`。connect 现在先验证
  pathname namespace entry；节点不存在返回 `ENOENT`，残留 socket 节点没有 listener 返回
  `ECONNREFUSED`。AF_UNIX `shutdown(SHUT_RD/WR/RDWR)` 不再是空操作，读写半边分别发布 EOF/EPIPE，
  清理相应 buffer 并唤醒已阻塞的 reader/writer。
- **poll/epoll**：Unix buffer、listener pending queue 复用 `PollWaiters` 做事件驱动登记，数据、空间、
  connect、shutdown 与 endpoint close 在释放 data lock 后唤醒；AF_UNIX 的 ppoll 不再退化为 yield
  polling。`FileOp` 显式发布 HUP/error 状态，ppoll/epoll 即使用户未请求也返回 `POLLHUP/POLLERR`；
  pipe 的 writer-close/read-end HUP 与 reader-close/write-end error 同步纳入该路径。
- **专项验证**：新增 `scripts/socket_phase5_probe_linux.c` 与 `socket_phase5_probe`。Linux 对照以
  `-Wall -Wextra -Werror` 通过；RV64 QEMU 10.0.2、`-m 16G -smp 8 -snapshot`、宿主
  `NI=-10/CLS=TS` 输出 `SOCKET_PHASE5 ALL PASS`，覆盖 pathname/nonblock、EOF/EPIPE、shutdown、
  阻塞 ppoll 数据唤醒、pipe HUP/ERR、epoll HUP 和 accept EINTR。既有 128 KiB
  `unix_socket_block_probe` 同轮继续通过；RV64/LA64 no-feature release 构建通过。
- **保留边界**：pathname 在 listener 存活期间 unlink 后用同名节点 rebind 的 registry identity、
  AF_UNIX getsockname/getpeername 完整地址回报和 named FIFO blocking-open/multi-open 仍需后续专项；
  TCP/UDP poll 仍沿用协议栈轮询，本轮只收敛有现成条件队列的 AF_UNIX。

## 2026-08-11 Linux/POSIX Phase 5 signal ABI 首轮（基于 `76f7c61` 的本轮工作树）

- **查询与校验语义**：`rt_sigprocmask(set=NULL)` 现在忽略 `how`，仍按固定 8-byte kernel sigset
  校验 `sigsetsize` 并可返回旧 mask；`rt_sigpending` 只报告被 mask 阻塞的 pending signals。
  `rt_sigaction` 开始校验第四个 `sigsetsize` 参数，并先准备新 action、验证 old-action copyout，最后
  提交 handler，避免非法输入指针先改写 old buffer 或 handler table。
- **空信号与 exec**：`rt_sigqueueinfo(..., sig=0)` 只做 target、权限和 info 指针校验，不再把
  `Sig(0)` 送入一基 signal bitmap；对其他进程补齐基本 uid/suid/euid 权限与非负 `si_code` 限制。
  exec 继续保留调用线程的 signal mask，并不再错误清空 pending set；自定义 handler 重置、SIG_IGN
  保留和 alt stack 重置维持原有规则。
- **专项验证**：新增 `scripts/signal_phase5_probe_linux.c` 与 `signal_phase5_probe`，覆盖 query-only
  sigprocmask、sigaction size/EFAULT 顺序、sigqueueinfo signal 0，以及 blocked SIGUSR1 跨 exec
  保留。Linux 对照以 `-Wall -Wextra -Werror` 通过；RV64 QEMU 10.0.2、
  `-m 16G -smp 8 -snapshot`、宿主 `NI=-10/CLS=TS` 输出 `SIGNAL_PHASE5 ALL PASS`。同一内核继续通过
  `task_a_clock_probe` 和 BusyBox `timeout 1 sleep 10`；后者由 SIGTERM 结束 sleep。RV64/LA64
  no-feature release 构建通过。
- **保留边界**：2026-08-13 后 `wait4/waitpid` 已有窄化的 `SA_RESTART` 路径，其他 syscall restart
  class 仍未实现通用 restart block；`SA_NOCLDWAIT`、精确 process-pending queue 和完整 job-control
  signal 仍待后续子轮，不以本专项通过宣称 signal 子系统已完成。

## 2026-08-11 Linux/POSIX Phase 5 task 生命周期审计（基于 `76f7c61` 的本轮工作树）

- **Linux 对照**：新增 `scripts/task_phase5_probe_linux.c`，宿主以
  `cc -std=c11 -Wall -Wextra -Werror -O2` 编译运行通过。对照确认：线程组 leader 调用原始
  `SYS_exit` 只结束自身，worker 后续无论调用 `exit_group(7)` 还是原始 `SYS_exit(7)`，父进程最终都
  观察到进程退出状态 7；非 leader 调用 `execve` 会结束 sibling、接管 TGID 并成功安装新映像，
  新映像中 `getpid() == gettid()`，而不是返回 `EINVAL`。
- **当前 RespOS 差异**：新增 `task_phase5_probe` 与独立 exec target。RV64 no-feature release、QEMU
  10.0.2、`-m 16G -smp 8 -snapshot`、宿主 `NI=-10/CLS=TS` 中，两种 leader `SYS_exit(42)` 用例都
  错误地立即结束整个线程组：父进程观察到 status `10752`（`42 << 8`）且 worker 未继续运行；非
  leader `execve` 返回 `-EINVAL`，失败路径的 `exit_group(111)` 使父进程观察到 status `28416`。
  探针打印三个 `TASK_PHASE5_EXPECTED_FAIL` 和 `TASK_PHASE5 CURRENT DIFFERENCES CONFIRMED`，并以非零
  状态退出；这些 marker 不能作为通过标记。
- **根因边界**：`task_exit()` 显式把 process leader 的单线程退出转为 group exit；`execve_file()` /
  `execve()` 显式拒绝非 leader。精确修复不能只删除两个条件：当前 `TASK_MANAGER`、父进程 children
  表、`ThreadGroup` 和多处 signal/process 查询都以 `tid == tgid` 的 TCB 作为 leader identity，非
  leader exec 需要安全的 de-thread/TID 接管；leader 单独退出还要保留进程可寻址性，并只在最后线程
  退出时向父进程发布 waitable zombie。
- **构建门禁**：加入探针后的 RV64/LA64 no-feature release 构建均通过。尚未修改 task 生命周期；
  mmap EOF/SIGBUS 与 task leader/de-thread 都属于本阶段待协商的跨模块设计点。

## 2026-08-11 Linux/POSIX Phase 4 namei、权限与文件系统 ABI（基于 `4d7f86d` 的本轮工作树）

- **final component 与 trailing slash**：namei 保留原始路径的 trailing-slash 目录约束；普通 lookup
  会按 Linux 规则在需要时跟随最终 symlink 并要求目录，rename 则定位旧目录项自身。`link()` 默认
  硬链接 symlink inode，`linkat(AT_SYMLINK_FOLLOW)` 才跟随目标；rename/unlink 不再误操作 symlink
  目标。open/create/link/unlink/rename 的 regular/symlink/directory 组合已由 Linux 对照 probe 固定
  `ENOENT/EISDIR/ENOTDIR/ELOOP/EEXIST`。
- **open-file 与权限**：`O_PATH` 忽略目标 read/write 权限及 `O_TRUNC/O_CREAT` 等无关状态位，
  `O_PATH|O_NOFOLLOW` 可持有 symlink 自身；read/fchmod/F_SETFL/fsync/fdatasync/getdents 等不允许的 fd
  操作返回 `EBADF`。dup 后 CLOEXEC 仍为 descriptor-local，O_APPEND/O_NONBLOCK 由共享 open-file
  description 统一观察。fsuid/fsgid、supplementary groups、umask、setgid directory、sticky bit、
  O_NOATIME 由专项 probe 覆盖；regular file 的 setgid 保留条件改为调用者属于继承 group。
- **mutation 与只读挂载**：create 在 lower inode 的 mode/uid/gid 成功提交后才发布 dentry，失败会回滚
  lower namespace entry；symlink owner 同样在发布前提交。只读 mount 对 open-for-write、buffered/
  positioned write、truncate、chmod/chown/utimens 和 xattr mutation 返回 `EROFS`，读取不触发 atime
  写入。`AT_EMPTY_PATH` 已覆盖 fstatat 与 O_TMPFILE link；当前 O_TMPFILE 仍以 materialize/copy 建立
  可见 inode，因此 link 前后 inode-number identity 不是 Linux 精确匿名 inode，实现边界保留为后续扩展。
- **验证**：宿主 Linux `scripts/fs_phase4_probe_linux.c` 以 `-Wall -Wextra -Werror` 编译运行通过；
  RV64/LA64 no-feature release 构建通过。本轮 RV64 QEMU 10.0.2、`-m 16G -smp 8 -snapshot`、宿主
  `NI=-10/CLS=TS` 依次通过 `fs_phase4_probe`、namespace、metadata、xattr、writeback normal 与
  `buildstorm_file_probe`；最终审查补丁重编译后又复跑 Phase 4/xattr probe。主体改动的完整 BuildStorm
  同配置输出 toolchain/minibuild PASS 和
  `BUILDSTORM_COMPILE mode=multi ok=true cores=8 bytes=1681000 arch=riscv64`，Cargo `20m41s`、
  axbuild 1256.83 秒、脚本退出 0；旧脚本的 `elapsed_s=0.00` 仍不作为计时依据。

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
- **历史小内存边界**：`-m 512M -smp 1` 可进入 shell，并报告
  `MemTotal: 522240 kB`。当时 256 MiB 静态 kernel heap 使 `ekernel` 物理末址约为
  `0x90bce000`；`-m 256M` 在 QEMU 放置 DTB 前即报
  `No enough memory to place DTB after kernel/initrd`。当前工作树已把 heap 移出 ELF/BSS；
  该记录只保留为旧故障证据，不再是当前 ELF 大小边界。

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

## 2026-08-11 RV64 iperf 控制通道卡死定位与修复

- 状态：根因已确认，内核修复已通过 RV64 实机运行与 RV64/LA 构建；`testrunner`
  无改动。
- 适用范围：当前工作树，RV64 SMP=1、4 GiB，由 `img/sdcard-rv.img.xz` 解压的
  干净镜像；临时仅调整镜像内脚本顺序以提前运行 iperf。
- 根因：`269a94a` 引入的 TCP 1 ms 阻塞避免了空闲 listener 忙轮询，但没有在
  smoltcp 状态前进时唤醒对端。iperf3 UDP 先建立 TCP 控制通道；客户端写入 JSON
  后服务端仍睡眠在接收长度上。临时回退到 yield-poll 后六项全部通过，因果链成立。
- 修复：`poll_interfaces()` 唤醒全局 TCP waiter；`TcpSocket::block_on` 在阻塞前登记 waiter、
  poll 并二次检查，防止丢失唤醒，同时保留 1 ms task-timeout 兜底。
- 验证：两轮 RV64 中 iperf musl BASIC/PARALLEL/REVERSE UDP/TCP 六项均输出
  `success`；`make build-rv`、`make build-la`、`cargo fmt --manifest-path os/Cargo.toml -- --check`
  通过。人为将 iperf 放到 basic 之前的临时顺序中，其后 glibc `test_sleep` 未在
  60/90 s 全局窗口内完成；这不是官方正常顺序，原因待验证，不得据此宣称
  sleep 回归通过。

## 2026-08-11 RV64 iozone “卡死”诊断

- 状态：干净镜像上未复现内核死锁；已修复本地镜像污染工作流
- 适用范围：RV64 SMP=1、4 GiB，release，`IOZONE_ONLY=1`，QEMU `-snapshot`
- 证据：`/tmp/respos-iozone-only.log`；从 `img/sdcard-rv.img.xz` 恢复的干净 x0；
  `e2fsck -fn img/sdcard-rv.img`
- 结果：glibc 和 musl 均完成 automatic 及所有 throughput 子项，两组均输出
  group end，guest 主动关机，总时间约 168 秒。先前 120 秒样本截止时 musl 仍在持续
  产生 random-write 结果，属于慢而非无进展。
- 镜像问题：原 `img/sdcard-rv.img` 的目录 checksum、inode/block bitmap、reference/free
  count 存在错误，已保留为 `img/sdcard-rv.img.corrupt-20260811`，并从仓库 `.xz`
  恢复原路径；恢复后 `e2fsck -fn` exit 0。
- 修复：顶层 `make rv`/`make la` 默认增加 `-snapshot`；`IOZONE_ONLY=1 make rv`
  提供可选专项诊断，默认 testrunner 清单不变。本轮没有为 iozone 修改内核
  FS/MM 语义。
- 后续定位：完整 runner 在 glibc iozone throughput 的 initial writers 后可稳定
  停滞；干净镜像上的最小 `iperf-musl → iperf-glibc → iozone-glibc` 顺序复现
  同一卡点。两个 iperf 脚本都使用 `iperf3 -s -D` 且不停止 daemon；在诊断
  路径中只杀掉遗留 `iperf3` 后，iozone 立即越过 rewriters/readers。将 TCP timeout
  兜底从 1 ms 改为 10 ms/1000 ms 均不能解除，已撤销这些无效实验。
- 2026-08-13 后续结论：本节当时确认的触发器成立，但 wait/kill/process-group 假设已由后续
  syscall/timer 观测否定。实际停在 initial-writer 后的 `sleep(2)`；iperf daemon 的 inet poll
  fallback 长期留在 kernel yield 循环，使 nanosleep deadline 没有高层 timer 安全点可消费。
  修复和双架构证据见本文件顶部同日 Phase 5 更新；runner 仍未增加 kill daemon 绕过。

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
