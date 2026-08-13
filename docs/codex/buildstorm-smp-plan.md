# BuildStorm SMP 启动与题二推进方案

## 2026-08-13 当前适用状态

RV64 8 核与 LA64 12 hart 的启动、调度、共享 MM 和 BuildStorm 功能链已经建立；下文 2026-08-05 至
2026-08-08 的 Phase 0--5、单 Processor、并行 rustc SIGSEGV 和 minibuild 阻塞均为历史实施记录，
不能继续当作当前 blocker。`0c21575` 又完成了 per-address-space frame 退役、页表页 root-switch
completion、ASID op=4 与失效范围传播，并以 LA 12 GiB/12 hart 完整 final 验证 op=4；当前架构线
只保留以下目标：

1. 以 `1/3/6/12` 缩放和 TLB/ext4/scheduler 计数决定下一优化，不预设 scheduler 或 ext4 是主因；
2. 独立评估 Global kernel mapping；范围请求暂以单次 op=4 覆盖，op=5 已因完整 final 内存破坏禁用；
3. 在资源允许或平台恢复后验证正式 LA `-m 36G -smp 12` 镜像与时限。

课程评测平台当前暂不可用；平台成绩与正式 36 GiB 结果标记 `待验证`。本地 12 GiB 功能或短窗口不能
替代平台结论。双线协作时，架构线修改 `MemorySet`、scheduler/processor、trap context 或公共 arch API
前，须与 Linux/POSIX Phase 线先约定接口；具体分工见 [current-status.md](./current-status.md) 首节。

## 2026-08-13 BuildStorm ext4/PageCache 关键路径优化包

### 选择依据与目标

该任务由架构/性能线推进，但按**不变量和接口**协作，不按文件机械隔离。现有 LA 30 秒窗口中，
`EXT4_OP_LOCK` 获取 37373 次，累计 wait/hold 为约 `0.017/9.217` CPU 秒；hold 主要分布在 read
`2.976s`、lookup `2.542s`、namespace `1.718s`、attributes `1.293s` 和 readdir `0.475s`。
同时 128 MiB PageCache 达到 32768 页，hit/miss/eviction 为 `108486/7521/24135`，block read 为
18837 次、373223424 bytes。该数据证明文件读取/查找和缓存淘汰位于关键路径，但极低的累计 wait
尚不支持直接拆 `EXT4_OP_LOCK`；“累计持锁工作”也不能等同于可直接扣除的墙钟时间。

本任务目标是降低固定 BuildStorm 进度下的 lower ext4 调用、锁内 CPU/I/O、PageCache refill/eviction
和小块读取，而不是以放松 Linux/POSIX 语义换分。第一里程碑以当前 HEAD、关闭 LTO、同一 pub 镜像
重新建立 LA 12 GiB/12-hart 30 秒计数基线；历史 `0c21575` 的 1773.01 秒完整结果仅作相邻参考，
不能替代当前提交入口的基线。阶段收益门槛为：短窗口至少一项关键工作量按相同进度下降且无反向
放大，最终无 feature 完整 final 保持 `ok=true`；若完整墙钟改善不足 5%，不继续扩大重构范围。

### 所有权与协作边界

- 性能线拥有本任务的测量、热点归因、ext4 lower-call 封装、只读 fast path、PageCache 读取/淘汰策略
  和 BuildStorm A/B。为形成完整实现，可以修改 `os/src/fs/ext4/**`、`os/src/fs/page_cache.rs`、
  VFS/file/namei 的相关调用链及 perf 计数；不因文件归属保留重复转换、重复 lookup 或半套缓存协议。
- Phase 线继续拥有返回值/errno、权限、namespace 原子性、metadata/time、writeback error、fsync/syncfs、
  mmap EOF/truncate/SIGBUS 等 Linux/POSIX 可观察契约。性能线若触及这些状态机，必须先提交接口说明和
  Linux baseline，双方共同审查，不得把语义变化隐藏为缓存或锁优化。
- `PageCache`、inode metadata generation、dirty-owner/writeback、truncate 串行化、VFS inode/dentry
  identity 是共享接口。两线可以跨文件完成闭环，但同一接口同一时段只保留一个写入者；另一线以
  probe、review 或后续可审查 patch 接入。队友的 IPC/network 与 task/signal 工作不需要等待本任务。
- 当前唯一 `EXT4_OP_LOCK` 保护 lwext4 的 mount、block cache 和目录遍历共享 C 状态，并跨 root/x1
  实例串行。没有上游线程安全证明、源码审计和双实例压力证据前，所有 lwext4 C 入口继续持该锁；
  per-inode Rust 锁、PageCache 锁或目录 generation 不能替代它。

### 分阶段方案

#### E0：当前 HEAD 基线与可归因测量

1. 固定镜像 hash、`mode=diagnostic`、LA 12 GiB/12 hart、`NI=-10/CLS=TS`，记录 30/120 秒窗口；
   同时补 `1/3/6/12` 短窗口，记录相同构建阶段而不是只比较 timeout 时的总计数。
2. 在现有 stat/lookup/read/write/readdir/namespace/attributes/superblock 分类上，进一步区分：进入
   lwext4 C 调用前的 Rust 准备、C 调用及同步 block I/O、返回后的 VFS/PageCache 发布。计数器只在
   `perf_counters` 启用；无 feature 路径静态消除。
3. 同时记录 inode read requested/completed、PageCache fill/hit/miss/eviction、block request size、
   ext4 acquisition/wait/hold/max、dentry hit/miss、guest timed 进度和宿主 CPU/RSS。若 wait 在缩放中
   仍近零，明确否决“先拆锁”；若 eviction/read amplification 主导，优先缓存/读取路径。

#### E1：不改变 lwext4 并发模型的完整热路径收缩

1. 审计每个高占比入口，把 CString/path 规范化、Rust buffer 分配、KStat/dirent 转换、用户 copy、
   PageCache 查找与发布等不访问 lwext4 共享状态的工作移到锁外；锁外准备失败不得留下 lower mutation。
2. 复用已有 inode/dentry/metadata generation 做可证明的只读 fast path，消除同一 namei/stat/read
   链上的重复 lower lookup/stat。cache key 必须含 filesystem id 和 inode identity；namespace、属性、
   truncate、unlink/rename 成功后按现有 generation 精确失效，失败不得提前发布。
3. 对连续只读 miss 合并 PageCache fill 与 lower read，调整预读只依据访问跨度和 eviction 数据；
   不用无界扩大 cache 掩盖生命周期问题，也不在持 `MemorySet`、PageCache page 或用户锁时进入 ext4。
4. namespace/attributes 首轮只移出纯准备和发布工作，不合并本应独立失败的 lower mutation，不改变
   rollback、只读挂载、权限、timestamp、nlink 或 dentry 可见性语义。

#### E2：按证据决定缓存策略或锁域并行

- 若固定进度下 PageCache eviction、refill bytes 或小块 block read 仍占主导，评估冷热分离、自适应
  预读和容量 A/B；容量修改必须同时检查 heap/frame 余量、dirty owner 与完整运行资源闭环。
- 只有 `1/3/6/12` 显示 ext4 wait/max-wait 随并发显著增长，且 E1 已排除锁内无关工作后，才研究
  per-superblock/对象锁。进入前必须审计 lwext4 mount/block-cache/journal/目录 iterator 的共享全局，
  为 root 与 x1 建立并发 probe；无法证明 C 层可并发时继续保留全局锁。
- 异步 VirtIO、多队列、PageCache 私有只读映射共享或 allocator 改造是独立实验，不与 E1/E2 同一
  提交混合，否则无法把 BuildStorm 收益归因。

### 正确性、性能与交付门禁

每个主题独立提交，至少通过 `git diff --check`、内核/用户格式检查、Rust 1.86 RV64/LA64 顺序 release
构建，以及 RV64/LA64 的 file/metadata/namespace/xattr/writeback probes。涉及 PageCache、truncate、
inode identity 或 mmap 时，追加 private-map、shared-MM、frame-reclaim、Phase3 writeback fault/persist
门禁；涉及 namespace 并发时保留 cwd-unlink、rename/link rollback 和双实例 mount。短窗口确认收益后
再跑 LA 12-hart CAgent 与完整 BuildStorm；正式结果必须无 feature、产物有效、脚本退出 0。

交付文档同时记录：commit、镜像 hash、QEMU/宿主调度、LTO/features、相同进度、计数前后差异、完整
wall time 与已运行 probe。性能线不得把“wait 下降”冒充语义正确，Phase 线也不得因文件边界复制一套
metadata/PageCache 状态；双方共同维护唯一的 inode identity、generation 和 writeback 协议。

## 目标与边界

最初目标是在不破坏 RV64 CAgent 回归的前提下，使内核真实使用 BuildStorm 所需的多个 CPU，并跑通
Debian glibc 镜像中的 Rust 工具链与 arceos-helloworld 全量构建；该功能目标现已在 RV64 8 核和
LA64 12 hart 本地完成，当前转入 LA 性能、Global mapping 评估和正式资源验收。

2026-08-08 比赛官方群更新的决赛参数为 RV64 `-smp 8 -m 16G`、LA64
`-smp 12 -m 36G`，整轮超时 6250 秒；脚本将 `nproc` 写入 `BUILDSTORM_COMPILE`。公告还说明新镜像
会补回预编译 `tg-xtask`，计时仅包含测试用例自身编译，不包含前置依赖和编译后的运行验证。当前
本地仍是旧 pub 镜像，以上口径在取得新镜像后必须以镜像 hash、脚本和实际命令再次验证。

历史第一阶段只承诺 SMP 正确性，不承诺时间分。后续仍不得伪造 `/proc/cpuinfo`、`nproc` 或
`/proc/uptime`，也不得只启动 QEMU 多 vCPU 而让次核停机。

历史实施顺序为先 RV64、后 LA64；两架构现均已接入真实 SMP。全局 run queue 和全局锁仍是允许的
正确性基线，per-CPU run queue、work stealing 和 cache 优化只有在计数证明为热点后才进入。

## BuildStorm 三层级优化总路线（2026-08-09）

历史 Phase 0--5 继续作为 SMP 启动、task owner、唤醒、共享 MM 与真实拓扑的**正确性前置**；本节把
原 Phase 6 的性能工作与已经完成的第一、二轮优化合并为后续唯一推进路线。正式评分看 RV64 8 核、
LA64 12 核的完整构建，但诊断必须先取得 1/2/4/8（LA64 为 1/3/6/12）缩放曲线，不能把
`nproc` 正确或 QEMU 启动了多个 vCPU 当作并行加速证据。

统一原则：

1. **先成功，再计时。** 任一 rustc SIGSEGV、产物缺失或内核错误都先按正确性缺陷处理；性能优化
   不能绕过失败门槛。
2. **先测量，再选择层级。** 每次只改变一个主要变量，保存 commit、镜像 hash、kernel features、
   QEMU 参数、Cargo jobs、guest timed 秒数、host wall time 和计数器窗口。
3. **诊断与正式配置分离。** `perf_counters`、`fault_trace`、`debug_traces` 只用于定位；正式成绩必须
   用无调试 feature 的 release kernel 重跑。详细串口 trace 不参与性能对照。
4. **整体吞吐优先。** BuildStorm 同时受 scheduler、ext4、PageCache/MM、allocator、VirtIO 和宿主
   资源影响；不能只因配置为 8/12 核就预设 scheduler 是主瓶颈。

### 第一层：低风险热路径与资源闭环

目标是在不改变核心并发模型的前提下降低确定性的串行 I/O、冗余写回、日志和内存元数据成本。

已完成主体：

- 普通 close 不再执行全文件系统/device flush，显式 `fsync`/`sync`/shutdown 语义保留；
- lwext4 连续块合并为 multi-block VirtIO 请求；scheduler handoff 去除重复 IPI；
- 性能计数和详细串口输出由 feature 静态隔离；
- PageCache 以 `Arc<FrameTracker>` 承载缓存页，普通文件 `MAP_SHARED` 与缓存共用 frame；
- PageCache 工作集提高到 64 MiB 并做最多 64 KiB 顺序预读；kernel heap 提高到 256 MiB。

该层已于 2026-08-11 闭环：RV64 无 feature 完整 BuildStorm 和 LA 12-hart 功能 BuildStorm 均成功，
历史并行 rustc SIGSEGV 不再是当前 blocker。后续若再次出现 SIGSEGV，必须按对应 commit/镜像重新
归因，不能直接复用旧结论。

第一层退出门槛：

- `BUILDSTORM_TOOLCHAIN ok`、`BUILDSTORM_MINIBUILD ok`、最终
  `BUILDSTORM_COMPILE ... ok=true` 且产物不少于 500 KiB；
- RV64/LA64 无 feature release 构建、RV64 8 核 file/shared-MM/进程门禁和题一关键回归通过；
- heap、PageCache page/LRU/registry、共享 file-page 表不随累计短命进程或文件流量无界增长；
- 保存第一轮提交 `7cb282a`、第二轮工作树与修复后的可比数据，不把旧计时窗口当成新基线。

### 第二层：消除共享瓶颈并取得有效多核扩展

目标是让固定的 RV64 8 核、LA64 12 核真正产生吞吐，而不是把额外并发转化成锁竞争、缺页复制、
迁移和内存压力。进入本层后先执行 `workflows.md` 的 CPU/jobs 双矩阵，再按证据只选择最高占比热点。

候选按当前证据优先级排列：

1. **ext4 锁域。** 测量 `EXT4_OP_LOCK` 等待/持有时间，区分 metadata、inode/file data 与 block I/O。
   只有确认 lwext4 对目标对象可并发后才拆锁；不能直接在全局锁外并发调用未知线程安全的 C 状态。
2. **私有只读文件映射。** 当前每个 rustc 的 `MAP_PRIVATE` fault 会分配独立 frame 并从 PageCache
   复制；约 300 MiB 的 `librustc_driver.so` 会放大多进程缺页和回收。评估让 clean、只读 private
   mapping 共享 PageCache frame，写权限映射保持 COW；必须覆盖 truncate、写入一致性、mprotect、
   fork/exec/exit 和回收 pin，不能把现有 `MAP_SHARED` 方案直接套用。
3. **per-CPU run queue。** 当前 RV64 已有 per-CPU Processor/idle 和 task owner，但 ready/blocked
   状态仍由全局 `SCHEDULER` 锁管理。仅当 runnable 积压、idle 比例或 scheduler lock 数据证明其为
   热点时，引入本地队列、last-CPU/wake affinity、idle pull/有限 work stealing 和 global fallback。
4. **PageCache 策略。** 根据 hit/miss/eviction 和访问跨度判断 64 MiB/16 页预读是否合适，再考虑
   自适应预读、冷热分离或工作集调整；不得只靠扩大缓存掩盖生命周期问题。
5. **退出/分配并发。** 若高并发退出再次集中在全局 heap/frame/task 回收锁，先缩短锁内析构与分配，
   再评估 per-CPU 小对象缓存；本阶段不替换整个 heap allocator。

调度器的第二层目标不是移植完整 CFS/EEVDF，而是消除单一全局 ready lock、保持缓存局部性并在空闲时
拉取工作。8/12 核规模可先用 O(N) `nr_running` 选择最忙队列；enqueue 仅在目标 CPU 从 idle 获得
runnable work 时发送/合并 IPI。Linux 的 per-CPU runqueue、wakeup placement、idle pull 和周期负载
均衡作为结构参考，RespOS 的 owner/handoff/affinity 不变量仍是实现边界。

第二层退出门槛：

- 固定 8-vCPU 的 jobs=1/2/4/8 曲线以及 CPU=jobs 的 1/2/4/8 曲线可复现；
- runnable 充足时无非预期 idle，且 8 路吞吐相对 1/2 路有明确提升；
- 每项修改都有 guest timed、host wall、CPU/RSS/swap 和对应锁/IO/fault 计数的前后对照；
- 单核、2/4/8 核以及 FS/MM/futex/pipe/exit 回归未因追求吞吐而退化为不安全语义。

### 第三层：深层并行 I/O、MM 与双架构扩展

只有第二层显示 CPU 仍被底层架构限制时才进入本层：

- VirtIO block 异步提交、完成中断与多队列，减少同步轮询和单队列串行；
- 强引用 inode/PageCache 生命周期与后台/批量 writeback，使普通 close 可完全脱离数据提交；
- per-CPU allocator cache 或按对象类型分配，前提是先证明全局 allocator 为热点并完成所有权审计；
- ASID、精确 active-mask 和按地址/地址空间 shootdown，降低共享 MM 的远端 RFENCE；
- LA64 已接入 12-hart PerCpu、runqueue、IPI、timer、同步 shootdown、ASID、按地址空间 frame 退役和
  range 元数据；range 当前安全执行 op=4，op=5 已由完整 final 失败否决。下一步是 Global kernel
  mapping、缩放诊断及正式 36 GiB 验证；
- 最后才考虑 NUMA/复杂 scheduler domain、完整公平调度策略或更激进的无锁结构。

第三层每项都是独立设计任务，必须有专项正确性测试和回退点；不得把多队列、ASID、allocator 与
scheduler 大改合在一次 BuildStorm 计时中。

### 数据到优化方向的判定

| 观测 | 优先方向 |
| --- | --- |
| ready/runnable 长期大于 CPU 数，但部分 hart idle 或 wake-to-run 很长 | scheduler 唤醒、IPI、runqueue 放置 |
| QEMU CPU 未满、ext4 lock wait/hold 占比高 | ext4 锁域、PageCache/I/O 锁外工作 |
| private-file faults、RSS、PageCache eviction 随 jobs 急升 | clean private 映射共享/COW、缓存策略 |
| remote RFENCE 随线程数急升且 MM 写操作集中 | active-mask、ASID、精确 shootdown |
| 所有 hart busy 但 block requests 小且同步等待多 | I/O 合并、异步完成、多队列 |
| guest 指标相近，只有 host available/swap 变化时出现骤慢或失败 | 宿主隔离与资源条件，不先修改内核 |

每轮只推进表中证据最强的一项。若计数器不能解释 wall time，先补测量，不以“优秀内核这样做”作为
直接合入理由。

## 交接状态与下一轮计划（2026-08-05，未提交工作树）

当前实现已经越过 early bring-up：RV64 次 hart 会进入 `task::run_tasks()`，全局 ready queue 有
CPU-owner/context-handoff 协议，`nproc=8`、完整 `/proc/cpuinfo` 和四路短 sleep 已通过。它**不是**
BuildStorm 就绪状态。以 QEMU `-snapshot -smp 8 -m 256M` 运行四路
`busybox timeout 3 busybox sleep 60` 并 `wait` 时，debug/release 均超过 60 秒未收敛；GDB 快照显示
并发 exit 的 `MemorySet` 页回收、全局 heap lock 与 scheduler lock 竞争。该现象应按活性/性能缺陷
调查，不能在没有等待环证据时叫作死锁，也不能归因于 TLB。

### 下一轮的可执行顺序

1. **先测准，再改代码。** 对每一条命令保存 host wall time、guest exit code、完整 serial log、
   QEMU 配置和镜像 hash；固定使用 `-snapshot`。最小矩阵为 `-smp 1/2/4/8`，每项至少 10 次：
   `true; sleep 1; true`、四路 timeout/exit、fork+exec+wait、pipe producer/consumer、futex wait/wake、
   TCP connect/close。超时后只抓 GDB/锁状态，不向 raw 镜像写诊断文件。
2. **专项审计退出路径。** 从 `TaskControlBlock::exit_process_group` 至
   `MemorySet::recycle_data_pages` / `MapArea::unmap_one`、frame allocator 和 `HEAP_ALLOCATOR`，列出
   每个锁、可能的内存分配和嵌套顺序；同时检查 `Scheduler::assert_invariants()` 是否在 scheduler lock
   内分配。用最小复现证明具体热点/等待环后才修复。
3. **补齐调度 ABI。** 当前 affinity syscall 只保存/报告 mask，`fetch_task()` 不筛选，必须实现
   affinity-aware dequeue 和对应 idle/IPI 行为，并回归 set/getaffinity、单核和多核非饥饿情形。
4. **补齐共享 MM。** `tlb_hart_mask` 是保守 residency 记录，不是 active-mask 和 request/ack
   shootdown。完成后才能开始 pthread/共享 `MemorySet` 压力；不要提前运行 BuildStorm cargo。
5. **最后才验收 BuildStorm。** 在固定官方 Debian 镜像与 RV64 `-smp 8 -m 16G`（LA64
   `-smp 12 -m 36G`）命令下依次跑 toolchain、
   minibuild、untimed build 和 timed build；题一 CAgent 保持 SMP=1，并在 owner/调度改动后重新做
   干净 pub 单项回归。

每次内核修改后的最低门槛仍是：

```bash
make RV_MODE=debug RV_USER_FEATURES= build-rv
cargo fmt --manifest-path os/Cargo.toml -- --check
cargo fmt --manifest-path user/Cargo.toml -- --check
git diff --check
```

## Phase 0/1 执行记录（2026-08-05，未提交工作树）

- 基线提交为 `cf30f64`，工作树只含本阶段 RV64 启动补丁和本方案文件；没有修改
  `testsuit/cagent-test/`、镜像或官方测试逻辑。
- Phase 0 已确认：以 `-smp 2` 启动旧内核时，QEMU/OpenSBI 有两个 hart，但 `/proc/cpuinfo`
  仍只有 CPU 0；`busybox nproc=2` 来自 `DEFAULT_CPU_AFFINITY_MASK` / `ONLINE_CPU_MASK`
  的硬编码，不能用作真实 SMP 成功证据。`SpinNoIrqLock` 才是跨核自旋锁；`NoIrqLock`
  只关闭本地 SIE，不能保护共享状态。
- Phase 1 已实现并实测：RV64 entry 按 SBI 传入的 `a0` hart id 分配 8 份 64 KiB early
  stack；第一个进入 S-mode 的 hart 以原子方式认领 boot 职责，避免错误假设 boot hart 恒为 0；
  boot 完成 MM/net/initproc 后通过 SBI HSM `hart_start` 拉起其余 hart。次 hart 在
  Acquire 观察 ready 后重载共享内核页表、安装本地 trap vector、标记 online 并 WFI。
- 当时这不是可调度的 SMP：次 hart未打开 timer，因为现有 timer trap 会访问单例
  `current_task`；`PROCESSOR`、idle context 和 scheduler 尚未 per-CPU 化。该限制已由下文
  Phase 2 的 per-CPU 基础改造解除，但仍不可用于 BuildStorm 或声称实际并行执行。
- QEMU/OpenSBI 可能选择任意 boot hart：`-smp 8` 的实测 boot hart 为 6（另一次为 1），
  修正前会停在早期等待；修正后 7 个次 hart 均到达 early-idle，boot hart 进入 Rust user shell。
  `-smp 1` 同样进入 shell；`/bin/busybox true` exit 0。多核控制台的上线文字会交错，
  是无串行 console 输出的诊断现象，不是任务调度证据。
- 已执行：`make RV_MODE=debug RV_USER_FEATURES= build-rv`、两个 `cargo fmt --check`、
  `git diff --check`；构建通过（仅既有依赖 target-feature warning）。复现启动命令：

```bash
qemu-system-riscv64 -machine virt -kernel kernel-rv -m 256M -nographic -smp 8 \
  -bios default \
  -drive file=img/sdcard-rv-pub.img,if=none,format=raw,id=x0 \
  -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot \
  -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc
```

下一步为 Phase 2：先建立 per-CPU processor/idle/trap/timer，再让任一 CPU 处理 timer；在此
之前禁止把 secondary 从 WFI 放入现有 run queue。

### Phase 2 前置架构结论（2026-08-05，待实施）

当前 RV64 不能安全地把 `tp` 直接当作 CPU id：用户态 `tp` 是 glibc TLS，进入
`__trap_from_user` 后仍是用户值；现有 `sscratch` 用于交换 user/kernel stack；而
`__switch` 还会保存/恢复内核 `tp`。因此只把 `PROCESSOR` 替换为数组会导致 syscall 或 timer
trap 依据用户 TLS 选错 CPU。RocketOS 从 `tp` 指针附近取 `hart_id` 的做法依赖其 task 内存布局，
且没有处理此 user-TLS/trap 边界，不能照搬。

推荐的下一步是先重构 RV64 trap ABI：用每 CPU 的固定 `TrapScratch` 保存 `kernel_sp`、用户
`sp`、以及 `PerCpu` 指针；user-trap 汇编先从 scratch 取本 CPU 身份和内核栈，再保存 user
寄存器。context switch 在锁内更新本 CPU scratch 的下一任务 kernel stack，进入 Rust handler
后只从 `PerCpu` 读取 current task。这样才能让 `tp` 完整保留给用户 ABI，并使 idle/trap/timer
真正 per-CPU。该改动涉及 trap.S、TaskContext/__switch 和 processor 的共同协议，属于下一轮
实现前应明确评审的边界，不能以局部数组补丁替代。

### Phase 2 执行记录（2026-08-05，部分完成）

- 已实现并验证 RV64 trap scratch 协议：`sscratch` 固定指向 `PerCpu::TrapScratch`，其中偏移
  0 是当前任务的 kernel stack top；user trap 先保存 user `t0/t1/sp`，再取本 CPU 的 kernel
  stack。内核执行时 `tp` 为 `PerCpu` 指针，返回用户前恢复 TrapContext 的 user TLS `tp`。
  每次 `__switch` 到一个已 claim 的任务前，调度路径会更新该任务保存的 kernel `tp` 与本 CPU
  scratch 的 kernel stack。这个机制不依赖 S-mode 不可访问的 `mhartid` CSR，也不改变用户 TLS ABI。
- 已将 `PROCESSOR` 与 bootstrap/idle context 改为固定 8 项的 per-CPU 状态；当前 task 查询按
  内核 `tp` 取得本 CPU processor。RV64 `-smp 1` / `-smp 2` 的 pub 镜像均进入 shell，
  `/bin/busybox uname -r` 输出 `6.10.0-dev`、`/bin/busybox true` exit 0；`-smp 2` 次 hart
  打印 `online (per-cpu idle)`。LA64 debug build 也在这一抽象改动后通过。
- 已验证每核 timer idle：次 hart设置 `sstatus.SIE`、`sie.STIE` 并使用既有 SBI TIME 路径后，
  `-smp 2` 的首个 tick 从 WFI 返回；清理探针后，`-smp 8` 的全部七个次 hart均打印
  `online (per-cpu timer idle)`。idle hart没有 current task 时只重设其本地 tick，不并发扫描
  全局 task timer。
- 已修复的独立缺陷：之前 GDB 所见低 `stvec` 不是物理/高半区转换错误。链接的
  `__trap_from_kernel` 仅有 2 字节对齐，其地址低两位为 `10`，被 `stvec` 当作保留的 trap mode，
  QEMU WARL 后表现为错误地址。给汇编入口增加 `.align 2` 后，guest 读回对齐的高半区 vector
  `0xffffffc08035de18`。此前“在 `trap::init` 补 `KERNEL_BASE`”的假设已撤销；也不再根据旧探针
  推断 Sstc 或 SBI TIME 不可用。
- 已验证 IPI trap 基础：以 SBI IPI（EID `sPI`）向**已 online 的本 hart**发送 self-IPI，
  `-smp 8` 的七个次 hart 上线日志均为 `ipi=1`。handler 先清 `sip.SSIP`，只递增 per-hart
  atomic counter，不触碰 scheduler lock。一次向 HSM `start-pending` hart 的 boot-time 广播
  得到 `ipi=0`，已撤销；因此后续 scheduler 只能对 `ONLINE_HART_MASK` 中已完成 trap 初始化的
  hart 做 publish-after-kick，不能以 `hart_start()` 成功返回代替 online。
- Phase 3 前置修复已实施：全局 `fetch_task()` 在 scheduler lock 内完成 ready queue 出队和
  `Running` claim，context switch 仍在锁外。这样两个 CPU 不会在 task 尚为 `Ready` 的窗口与
  wakeup/signal 竞争。尚未启动 secondary scheduler：现有无 runnable task 路径会在已阻塞 task
  的内核栈中自旋，且 idle timer 目前不能安全并发扫描全局 timeout。必须先实现每 CPU 的
  idle↔task 返回闭环、指定单一 timer-service CPU 或等价的 timer work 串行化，再开启该路径。
- 上述两个 idle 前置已完成且只在 boot hart 调度下验证：每 CPU `IDLE_TASKS[cpu]` 保存首次
  dispatch 的 bootstrap context；无 runnable task 的 Blocked/Exited task 现在恢复该 context，
  idle loop WFI 后重新 claim ready queue。RISC-V 从 user trap 进入 S-mode 时硬件会清 `SIE`，
  所以 idle 必须在无锁状态重新启用 timer/software interrupt；否则 timeout 不会被 kernel trap
  服务。全局 timeout 目前明确只由 boot hart 的 timer service 扫描，避免次 hart 并发进入旧的
  全局 timeout 结构。`-smp 1` pub 的 BusyBox `sleep 1` 回到 shell 后 `true` 成功；secondary
  仍未进入 `run_tasks()`，不能当作多核 scheduler 验收。
- Phase 4 基础已实施：RV64 `MemorySet` 维护“曾加载此页表的 hart”保守位图。context switch
  在恢复 task 前先置当前 bit；不在 switch-out 时清位，避免页表 writer 与仍执行旧 `satp` 的 CPU
  交错时漏发 shootdown。`flush_tlb()` 先做本地 memory barrier/`sfence.vma`，再经 SBI RFENCE
  对位图中除当前 hart 外的目标做全地址空间 `remote_sfence_vma`（当前没有 ASID allocator）。
  QEMU `-smp 8` 已以次 hart→boot hart 探针验证 RFENCE 返回成功；探针未保留在正式启动路径。
  此策略正确性优先且会 over-flush，待 BuildStorm 正确性稳定后再做 active-mask 收缩/按 ASID 优化。

## 当前代码事实（2026-08-05）

- RV64 的 `rust_main(hart_id, opaque)` 由最先进入的 hart 一次性清 BSS、初始化全局 MM/net/initproc；
  HSM 次 hart 在 ready barrier 后走 `secondary_main()`，不得重复全局初始化。early stack、`PerCpu` 与
  online/idle 位图固定为 8 项，hart ID 大于等于 8 目前不受支持。
- `task/processor.rs` 已改为每 hart 的 `PROCESSORS` 与 `IDLE_TASKS`；`current_task()` 从内核态
  `tp` 的 `PerCpu` 取得本 CPU 状态。RV64 user `tp` 继续从 trap context 恢复，保持 glibc TLS ABI。
- scheduler 仍是全局 ready/blocked 队列，但在 scheduler lock 内 claim Ready task，并通过 owner +
  idle handoff 避免保存 context 前的双运行；ready 发布会对 idle hart 发 IPI。该协议只完成有限短压。
- `MemorySet` 已有保守 TLB residency/RFENCE 基础，但没有 active-mask/request-ack；affinity syscall
  报告真实 online mask，但 dequeue 尚未执行 affinity 筛选。这两项都是后续正确性工作，不是完成态。
- 单核 trap、timer、页表切换、scheduler 已服务 CAgent；owner 栅栏之后尚未重新运行 CAgent，必须在
  下一轮调度/MM 修改前先补干净 pub 单项。

## 往年参考实现比对（2026-08-05）

本节审阅本仓库 `examples/rocketos` 的源码，不将其当作可直接合入的依赖或正确性证明。`examples/delnx0`
当前未提供可审阅的 SMP 源码，因此不从中引申结论。

| 主题 | RocketOS 的做法 | 可借鉴结论 | RespOS 的处理决定 |
| --- | --- | --- | --- |
| RV64 入口 | `entry.S` 接收 a0 hart id，以 `boot_stack_top - (hart_id << 16)` 选早期栈 | 次核必须最早按 hart 选独立栈，不能进入 Rust 后才修复 | 使用显式 `MAX_CPUS * EARLY_STACK_SIZE` 数组、范围检查和 linker 地址断言；容量至少 8，不能沿用示例的 4 核空间 |
| 次核启动 | boot hart 通过 SBI HSM `hart_start(hart, 0x80200000)` 逐核拉起 | OpenSBI HSM 是 RV64 QEMU 可行路径 | 在现有 `arch::sbi` 补标准 HSM 封装，传入次核入口与 opaque；检查/记录 SBI error，不能忽略返回值 |
| 全局初始化分支 | `IS_BOOT.compare_exchange` 让一核做 BSS/MM，其他核走另一条路径 | 必须把 boot 与 secondary 初始化分开 | 用具名 boot state（Cold/GlobalInit/Ready/Failed）和 Acquire/Release barrier；禁止次核仅靠 CAS 失败后立即访问半初始化全局对象 |
| current task | `Vec<RwLock<Processor>>` 按 hart 保存 current | current 不是全局单例 | 建立 `PerCpu`，并把 cpu id 与当前 CPU 身份绑定；不通过 task 的继承 `cpu_id` 推断当前运行 CPU |
| run queue | 每 hart 一个 `SyncUnsafeCell<Scheduler>`，按 task.cpu_id 入队 | per-CPU queue 是性能终局方向 | 第一正确性版保留受锁保护的全局队列和明确 claim；后续才引入 per-CPU queue/work stealing，不能照搬无锁 `SyncUnsafeCell` |
| idle/boot context | 所有 hart 复用同一个 `BOOT_TASK` Arc | 暴露了 idle 上下文必须 per-CPU 的问题 | 每 CPU 独立 idle context/stack，且不进入 task manager；共享 idle TCB 在 RespOS 中明确禁止 |
| TLB/IPI | SBI 常量包含 IPI/remote sfence，但该路径未构成完整 request/ack 协议 | HSM、IPI、remote fence 的架构接口可分层 | 将 IPI pending、shootdown request/ack、active CPU 位图列为 Phase 3/4 的独立门槛，不能仅调用 remote fence 即回收页表 |

这份比对改变了实施细节而不改变总体路线：Phase 1 现在明确要求由汇编在最早期依据 hart id 切栈；Phase 3 在实现 per-CPU run queue 之前先采用加锁的全局队列；boot barrier 与 idle 独占性从“建议”提升为不可跳过的不变量。

## 目标架构与不变量

```text
boot hart：一次性 BSS/MM/trap 模板/task/net/initproc 初始化
  → Release 发布 SMP_READY
  → SBI HSM 启动 secondary hart

每个 hart：专属 early/idle stack 与 PerCpu
  → 安装本核 trap/scratch/timer
  → Acquire 等待全局初始化完成并标记 online
  → 从全局 ready queue claim task；空闲时 WFI，IPI/timer 后重选任务
```

必须遵守：

1. 仅 boot hart 清 BSS、初始化 allocator/MM/net、创建 initproc；次核只进入 secondary path。
2. current task、idle context、内核栈、trap scratch、timer compare 都是 per-CPU；一个 TCB 任意时刻最多在一个 CPU 为 `Running`。
3. 初版 scheduler 可全局加锁，但 task 必须在锁内从 ready 集合 claim。更关键的是，运行中的 current
   task 不能在其 `__switch` 已保存 context 前被重新发布到 ready 集合：必须采用 handoff trampoline
   或等价协议，使“保存 current context → 发布 Ready → claim next”对其他 CPU 不暴露半完成状态。
   仅在 fetch 时设 `Running` 不足以避免双运行。
4. `add_task`、futex/signal/pipe/socket/timer 唤醒必须在发布 ready 状态后可靠 kick 一个 CPU；不能等不确定的 timer tick。
5. 关中断锁只禁止本 CPU 中断，不是跨核互斥。所有原有“单核+关中断”共享状态必须改为真实锁或原子协议。
6. 本地 context switch 刷新本 CPU TLB；并发使用同一 `MemorySet` 前必须有 active-CPU 追踪和 TLB shootdown，之后才能回收 PTE/frame。
7. `/proc/cpuinfo`、`nproc` 和 affinity 只能从真实 online CPU 集合生成。

## 分阶段实施与验收

每个内核阶段后至少执行：

```bash
make RV_MODE=debug RV_USER_FEATURES= build-rv
cargo fmt --manifest-path os/Cargo.toml -- --check
cargo fmt --manifest-path user/Cargo.toml -- --check
git diff --check
```

### Phase 0：基线、审计与资源

- 保存当前 `SMP=1` CAgent 小回归结果。
- 获取官方 BuildStorm Debian/toolchain 镜像、最终启动脚本，记录镜像 hash、QEMU 版本、宿主物理核数、内存、磁盘后端和 Linux baseline；不得用 pub 镜像替代 BuildStorm 成绩。
- 审计 main、两套 entry/trap/timer、processor/scheduler/task、frame allocator、页表修改、`SpinNoIrqLock`、futex/clone/exit 和 virtio IRQ。
- 对照 RocketOS 的 `entry.S`、`hart.rs`、`sbi.rs`、`main.rs`、processor 与 scheduler，维护一张“参考机制 / RespOS 对应所有者 / 并发安全差异 / 是否采用”的审计表；任何引用示例的实现必须补足锁、barrier 和错误路径。
- 建立最小 SMP probe：每核打印 hart/逻辑 CPU，递增独立 atomic 计数；用户态读取 `/proc/cpuinfo`、`nproc`、affinity。
- 添加独立且默认不启用的 SMP debug 启动入口；不得改变 `run-rv-pub` 默认单核。

验收：单核 boot 与 CAgent 单项不变；保存当前 `-smp 2` 的完整失败/仅 boot-hart 日志作为对照。

### Phase 1：RV64 次核 early bring-up（不调度用户任务）

- 重做 RV64 entry：读取 `mhartid`，按 hart 选择有上限检查的独立 early stack；boot hart 进主初始化，次核进 `rust_secondary_main(hartid)`。
- 调研并选择 QEMU/OpenSBI 次核协议：若 firmware 不会直接放行所有 hart，则 boot hart 在初始化完成后用 SBI HSM `hart_start` 启动其余 hart，并记录返回错误码。
- 定义固定上限至少 8 的 `PerCpu`：逻辑 CPU id、hart id、online/boot state、idle stack/context、current task。early path 不动态分配。
- boot hart Release 发布 `SMP_READY`；次核 Acquire 等待，再安装本核 trap vector、scratch、内核页表与 timer，先停在 WFI，不取用户任务。
- 为 boot state 定义失败状态和超时诊断：secondary 观察到 `Failed` 时停机并打印 hart/阶段；不得无限静默自旋掩盖 boot hart panic。

验收：`-smp 2`、`-smp 8` 都打印唯一 hart 上线记录；boot hart 可启动 shell；次核 WFI 无 trap/栈损坏；`SMP=1` 不变。

### Phase 2：per-CPU processor、idle、trap 与 timer

- 将全局 `PROCESSOR`、`IDLE_TASK` 改为按 `cpu_id()` 查询；审计全部 `current_task()`、`current_user_token()` 与 `PROCESSOR.lock()` 调用点。
- 每核建立 idle context，明确其不进 task manager、不参与 wait/reap，且不共享 boot/task stack。
- 将 RV64 `stvec`、`sscratch`、SIE 和 timer compare 全部放入每 hart 初始化；timer trap 仅抢占当前 CPU。
- 为共享 `UnsafeCell`、全局静态、driver 状态补足锁/原子协议；审计 NoIrqLock 的真实边界。

验收：2 核都能收 timer trap；任一 CPU idle 不影响另一 CPU；单核 timeout/wait/CAgent 单项回归；debug 断言无 task 双 Running。

### Phase 3：全局 run queue、多核调度与 IPI

- 初版保留全局 `Scheduler`；在锁内 claim ready task、提交 `Running`，锁外 context switch。先实现
  context-handoff trampoline（或等价的 per-task CPU owner 状态机）：抢占/yield 的 current 必须先保存
  context，才可变为可被其他 CPU claim 的 `Ready`；阻塞、抢占、退出只能由当前 CPU 提交。为每个
  TCB 增加仅调试使用的 owner CPU/迁移断言，任何 task 双 Running 立即停机记录。
- 实现 RV64 software interrupt/IPI 与 pending 位。IPI handler 只确认/清 pending 并触发本地调度，不持有高层锁做复杂切换。
- 把 `add_task`、`wakeup_task`、timer/futex/signal/pipe/socket completion 统一为 `enqueue_and_kick()`：先发布 ready 状态，再 kick idle/目标 CPU。
- 审计 `DEAD_TASKS`、task manager、tid/pid allocator、thread group、parent/children、signal 与 timer 的锁顺序；禁止持 scheduler lock 做 I/O、用户 copy、网络 poll 或睡眠。

验收：2 核运行两个 CPU-bound worker 时两个计数都增长；并发 fork/exec/wait、futex、pipe、信号中断均无任务丢失；8 核能稳定回 shell，短压力至少 30 次。

### Phase 4：MM 并发与 TLB shootdown

- 在 `MemorySet` 加 active-CPU 位图，context switch 切入/切出以明确内存序更新；同一地址空间的多个线程可占多个 CPU。
- 审计 mmap/munmap/mprotect/mremap、COW/lazy fault、fork/exec/exit、file writeback 的 PTE 与锁范围。
- 实现 request/ack shootdown：发布页表变更 → IPI → 目标 CPU 本地 `sfence.vma` → ack → 回收 frame/page table 或返回用户。初版可全地址空间 flush。
- 把现有退出后延迟页表释放扩展到远端 CPU 不再使用旧 token，不能用 Arc/frame 引用计数代替 TLB 安全。

验收：两核 pthread/futex 压力；一核反复映射/保护变化、另一核访问；fork+exec+exit 循环；无 stale mapping、double free 或 kernel fault。完成前不运行共享 MM 的多核工具链压力。

### Phase 5：真实 CPU 拓扑与 BuildStorm 功能门槛

- `/proc/cpuinfo`、`/proc/stat`、sysconf、`sched_getaffinity`、`getcpu`（如需要）从真实 online CPU 集合导出；保存 Debian `nproc` 的实际查询链路。
- 依官方顺序运行：toolchain version、cargo new+minibuild、untimed tg-xtask、timed full build。每段保存 serial log、返回码、磁盘/内存和 `/proc/uptime` 原始值。
- 遇失败按动态 loader/ELF → syscall errno → FS/MM → scheduler/futex → linker 分层定位，不能把 cargo 首个报错直接当作 SMP 错误。

验收：RV64 `-smp 8 -m 16G` 下 `nproc=8`，`BUILDSTORM_TOOLCHAIN ok`、
`BUILDSTORM_MINIBUILD ok`，并至少得到 `BUILDSTORM_COMPILE mode=multi ok=true` 与 ≥500 KiB 产物；
LA64 对应 `-smp 12 -m 36G`、`nproc=12`。成绩比较只采用新脚本标出的测试用例编译区间。

### Phase 6：性能与 LA64

- 同机、同镜像、同 QEMU 配置取得 Linux 基线 B；至少三次记录中位数和离散度。严格使用脚本 guest `/proc/uptime` timed 区间。
- 先 profile 再优化：CPU 利用率、run queue lock、context switch、futex、page fault、page cache/FS、动态 loader、allocator、virtio block。
- 优化候选：per-CPU run queue/work stealing、per-CPU allocator cache、缩短 FS/page-cache 锁、合并 block I/O、只读 executable/page cache。每项必须有前后数据与 mmap/FS/TCP/单核回归。
- LA64 复用公共 PerCpu/scheduler/MM 协议，单独实现 firmware 次核启动、IPI、timer、TLB flush；先 2 核再按最终官方核数扩展。

## 风险与停止条件

| 风险 | 处置 |
| --- | --- |
| 多 hart 共用 BSS/boot stack | 先完成 per-hart stack 和 boot barrier；未通过不得调度次核 |
| 同一 task 双运行 | task→cpu owner debug 断言；出现一次即停止 Phase 3。已在 2026-08-05 复现：先入队、后 `__switch` 的 preempt/yield 协议会使另一 CPU 恢复尚未保存的同一 TCB；必须先完成 context-handoff trampoline |
| ready task 无人运行 | 所有 enqueue 走 kick/IPI；不能以轮询或 timer 偶然唤醒代替 |
| stale TLB | active CPU 位图与 request/ack shootdown 完成前，不允许共享 MM 多核执行 |
| 单核假设的数据竞争 | 每个共享可变对象列明 owner、锁和内存序；无法证明就先串行化 |
| 镜像/资源污染 | BuildStorm 镜像独立副本；QEMU 持有 raw image 时不运行 host debugfs/e2fsck |
| 优化掩盖正确性 | 无 profile、无前后数据、无回归的优化不合入 |

## 首轮行动清单

- [ ] 获取并校验补回预编译 `tg-xtask` 的 BuildStorm 镜像与官方最终启动命令，记录 hash，并确认
  RV64 16 GiB/8 核、LA64 36 GiB/12 核、6250 秒超时和新计时边界。
- [x] 在旧镜像上完成 `tg-xtask --help`、手工 ArceOS release 链接，以及完整
  `cargo xtask arceos build -p arceos-helloworld --arch riscv64`；后者的 cargo 阶段 6584.09 秒、
  objcopy 与顶层命令均返回 0。该结果只证明内核运行时兼容性，不替代上一项的新镜像与正式资源验收。
- [ ] 当前内核跑一次 `SMP=2` 只读启动实验并保存完整 serial log。
- [ ] 列出 `PROCESSOR`、`IDLE_TASK`、`current_task`、`SCHEDULER`、timer/trap、`UnsafeCell`、`SpinNoIrqLock` 的调用点，形成共享状态审计表。
- [ ] 确定 RV64 OpenSBI HSM/IPI API 与现有 `arch::sbi` 的能力缺口。
- [ ] 评审 `PerCpu` 固定布局、最大 CPU 数、hart→逻辑 CPU 映射及 boot memory ordering。
- [ ] 决定 entry/链接脚本的 early stack 数组设计，并先验证其地址、对齐与映射。

完成首轮清单后，才开始 Phase 1 的内核实现。

## 2026-08-05 执行记录：Phase 3 context handoff

- 8 核首次接入 `run_tasks()` 后，`nproc=8` 但 `cat /proc/cpuinfo` 可触发 `sepc=0` 指令页故障。
  GDB 和调度代码确认根因是 preempt/yield 的“先把 current 入 ready queue、后 `__switch`”窗口；
  另一 hart 可恢复尚未保存的同一 TCB。此前的 ready-queue claim 原子化不足以关闭该窗口。
- 已改为 per-CPU handoff：任务先从 `Processor.current` 移入 handoff slot，再切入本核 idle；
  idle loop 在旧 context 保存后才发布 Ready。RV64 SMP=8 已通过 `nproc`、完整 `/proc/cpuinfo` 和
  四个后台 `sleep 1` + `wait`；SMP=1 的 sleep 与 CAgent kernel 单项亦通过。
- 这只完成 Phase 3 的一个必要交接不变量。下一步应为 TCB 加 owner CPU debug 断言，随后分别做
  fork/exec/wait、futex、pipe/socket 的可重复 2/4/8 核压力；共享地址空间压力必须继续受
  Phase 4 的 active-mask/shootdown 完整性约束，不能以本次短压代替。

## 2026-08-05 执行记录：owner 栅栏后的退出压力

- owner CPU 栅栏已加入，并使 Blocked→wakeup 的 context-save 窗口与 yield/preempt 使用同一 idle
  handoff。它通过了 SMP=1 sleep 与 SMP=8 procfs/四路短 sleep，但四路 `timeout 3 sleep 60` 在
  debug/release 的 8 核 QEMU 中超过 60 秒未完成。
- GDB 显示多个 timeout/sleep 子进程同时执行 `exit_process_group` 的 `MemorySet` 回收，在全局
  buddy heap spin lock 上争用；scheduler 也在 enqueue invariant 中分配。应把”全局 allocator 和
  exit page recycle 的并发锁序/分配行为”列为 Phase 3→4 的阻塞审计项。不能仅关闭 debug assertion
  宣称修复，也不能用 per-CPU allocator 作为未经 frame/page-table 所有权审计的快速替换。

## 2026-08-07 执行记录：exec/group-exit quiescence 协议

- 实现了 Phase 3→4 的协作式 sibling 终止协议，解决 exec/exit_group 时远端 CPU 仍在旧
  `MemorySet` 上执行的安全窗口。核心机制：
  - TCB 新增 `terminate_requested: AtomicBool` 字段，`request_termination()` 以 Release 写入。
  - `can_be_claimed_on_cpu()` / `try_claim_running_on_cpu()` 以 Acquire 检查，拒绝已标记终止的 task。
  - `publish_saved_handoff()` 对已终止 task 静默丢弃，不再重新发布到 ready queue。
  - `close_other_threads_for_exec()` 和 `exit_process_group()` 采用统一四步流程：
    标记终止 + 摘除 → spin-wait CPU owner 释放 → ack 确认 → 安全回收。
- 协议不依赖 IPI 主动打断远端 CPU，而是依赖 timer preempt 使远端 CPU 进入调度路径后
  自然释放 owner。最坏等待不超过一个调度 quantum。
- 同时新增五条诊断 trace channel（`quiescetrace`/`proctrace`/`pipelifetrace`/`ldtrace`/
  `illegaltrace`）用于下一轮 BuildStorm 阻塞点定位。
- 本轮未实际运行 BuildStorm；quiescence 协议和 trace 的正确性仅通过构建门禁
  （`make build-rv/build-la`、`cargo fmt --check`、`git diff --check`）。
- 与该协议相关的上下文和诊断策略详见 `current-status.md` 和 `pitfalls.md` 同日更新。
