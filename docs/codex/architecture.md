# RespOS 架构与不变量

本文只记录当前源码可验证的结构和长期约束。更细的专题设计可继续阅读
`docs/mm模块基础说明.md`、`docs/task模块核心功能说明.md`、
`docs/ltp-fs-abi-design.md` 与 A/B/C 重构文档。

## 启动与执行链

```text
架构入口 / linker
  → rust_main：清 BSS；LA 建立早期分页并跳转高半区
  → rust_main_high
  → trap::init
  → mm::init
  → net::init
  → task::add_initproc
  → 开启并设置下一次时钟中断
  → task::run_tasks
  → initproc / testrunner / 比赛镜像程序
```

### 双 virtio ext4 根与辅助文件系统

- 状态：RV64 与 LoongArch 均已实现并运行到 preliminary testrunner
- 适用范围：QEMU block 设备、lwext4、VFS mount tree、initproc launcher
- 最后验证：2026-08-11
- 证据：`os/src/drivers/mod.rs`、`os/src/arch/loongarch64/pci.rs`、
  `os/src/fs/{ext4,mount.rs}`、`user/src/bin/{initproc,contest_launcher}.rs`；RV64 x0-only、合法 x1 ext4 和非 ext4 x1
  QEMU 启动；LoongArch x0+x1 启动到首个 `basic-musl` 测例
- 内容：设备 index 0 是必需的官方根盘，lwext4 mountpoint 为 `/`；index 1 是可选
  辅助盘，合法 ext4 时以独立 superblock/inode identity 挂载到 `/respos`。辅助盘不存在、
  virtio 初始化失败或 ext4 挂载失败都只禁用辅助挂载，不得影响 x0 启动。
  lwext4 的 C 层挂载表是全局的，路径必须按最长 mountpoint 前缀选择实例；Rust inode
  cache key 必须包含 filesystem id，不能只用 inode number。所有 lwext4 实例仍共用唯一
  `EXT4_OP_LOCK`。关机路径先尝试 shutdown 辅助 superblock，再 shutdown 根 superblock；即使
  一个设备 flush 失败，也必须继续尝试另一个设备。
- 启动入口：内嵌 `initproc` 启动内嵌 `contest_launcher`。launcher 从 `/respos/profile`
  读取 `mode=auto|preliminary|final|diagnostic`。线上 `auto` 以及 profile 缺失、空白或无效时，先检查
  根盘上的 CAgent/BuildStorm 决赛脚本，再检查 musl/glibc basic 初赛脚本；决赛标志优先，未知镜像
  打印告警并安全回退到 preliminary。preliminary exec 原内嵌 `testrunner`；final
  在 `/glibc` 中使用 `/bin/bash` 严格串行运行当前官方决赛镜像固定的
  `cagent_testcode.sh`、`buildstorm_testcode.sh`，全部结束后关机。diagnostic 只供显式本地 profile
  使用，进入内嵌 `user_shell`，默认提交镜像不会选择它。dispatcher 失败时
  `initproc` 仍依次回退到内嵌 `testrunner` 和 `user_shell`。测例策略不进入内核。
- 后续影响：新增 inode-number lwext4 API 时必须传递所属 mountpoint；新增固有 VFS
  mountpoint 时必须同时插入并 pin 全局 dentry cache，否则 namei 会新建不同的
  dentry 并绕过 mount tree。

### 初始化顺序不可随意交换

- 状态：已确认
- 适用范围：`os/src/main.rs` 与各全局子系统
- 最后验证：2026-08-10
- 证据：`os/src/main.rs`、`os/src/mm/mod.rs`、`os/src/net/mod.rs`
- 内容：MM 初始化先于网络全局对象和 initproc，initproc 入队后才开启周期调度。LoongArch 在
  进入公共高半区路径前还有早期分页和架构扩展初始化。
- 后续影响：新增依赖 allocator、页表或 timer 的全局对象时，要确认首次访问发生在相应子系统
  初始化之后。

### RV64 物理内存上限来自 OpenSBI FDT

- 状态：已实现并验证 16 GiB
- 适用范围：RV64 early page table、frame allocator、kernel direct map、procfs/sysinfo
- 最后验证：2026-08-11
- 证据：`os/src/arch/rv64/entry/entry.asm`、`os/src/arch/rv64/config/board.rs`、
  `os/src/mm/{frame_allocator,memory_set}.rs`；16 GiB/8 核启动、专项和完整 BuildStorm 结果见
  [current-status.md](./current-status.md)。
- 内容：early Sv39 root page table 在低地址和 `KERNEL_BASE` direct map 各安装 16 个
  1 GiB leaf，覆盖 QEMU virt 的 `0x80000000..0x480000000` RAM 窗口，因而能读取 16 GiB
  客体位于 `0x47fe00000` 的 FDT。boot hart 随后从 FDT `/memory` `reg` 取实际末址，
  frame allocator 严格以该实际末址为上限。
  首个 RAM GiB 保留 4 KiB 页以分离 kernel section 权限，后续整 GiB 用 Sv39 level-2 leaf。
- 后续影响：不得把 early “最大可达窗口”当成真实 RAM 分配上限；增大支持内存时
  必须同时审计 Sv39 物理地址范围、FDT 位置、direct-map leaf 和小内存启动。

### LoongArch 物理内存上限来自 QEMU fw_cfg

- 状态：已实现并验证 4 GiB/12 GiB，36 GiB 平台实测待验证
- 适用范围：LA early DMW、frame allocator、kernel direct map、procfs/sysinfo
- 最后验证：2026-08-12
- 证据：`os/src/arch/loongarch64/config/board.rs`、
  `os/src/arch/loongarch64/{mm/page_table.rs,tlb_refill.S}`、`os/src/mm/memory_set.rs`；结果见
  [current-status.md](./current-status.md)。
- 内容：boot hart 在关闭 DMW0 前从 QEMU virt `0x1e020000` fw_cfg MMIO 读取
  `FW_CFG_RAM_SIZE`，以 256 MiB low RAM 和 `0x80000000` high RAM 起点换算实际 high end。
  正式页表保留 low RAM 的 4 KiB 权限映射，高 RAM 使用非 Global 的 PMD 2 MiB huge leaf；软件
  refill 必须区分 table pointer 与 bit-6 huge leaf，后者不能执行 table-pointer 解码的 `-1`。
  fw_cfg 无效时保留 12 GiB 兼容上限，发现值最高钳制到比赛 36 GiB。
- 后续影响：36 GiB 支持不能退回逐页 direct map；调整 RAM 上限时必须同时审计 39-bit VA、物理
  地址位宽、PMD 对齐和 frame allocator bitmap。fw_cfg 只在 DMW0 生效的 boot hart 早期读取，
  secondary 不得重复访问或修改全局物理上限。用户 ASID 已启用，但 direct map 仍非 Global；Global
  kernel 映射必须和软件 refill 的成对 G 位、huge leaf 及远端 shootdown 一起独立验证。

### kernel heap 是启动期 RAM 预留区，不属于 ELF/BSS

- 状态：已实现，RV64/LoongArch 启动烟测通过
- 适用范围：early direct map、全局 buddy allocator、frame allocator 初始化边界
- 最后验证：2026-08-11
- 证据：`os/src/mm/{heap_allocator,frame_allocator}.rs`；RV64/LA64 release 构建；RV64
  `-m 512M -smp 1 -snapshot` 运行到 libcbench，LoongArch 同配置进入 `basic-musl`
- 内容：buddy bitmap 与 heap storage 不再声明为静态 BSS，而是从页对齐的 `ekernel` 之后连续
  预留；两者通过已经生效的 high-half direct map 访问。`init_heap()` 返回物理预留末端，frame
  allocator 必须排除该区间，不能再次分配这些页。RV64 heap 位于 `ekernel` 后；LoongArch
  QEMU RAM 分为 `0..0x10000000` 与从 `0x80000000` 开始的动态 high RAM，256 MiB heap
  位于高端段起始处，frame allocator 同时管理扣除内核/heap 后的两个不连续区间。容量仍在启动时
  固定，运行期不扩容。
- 后续影响：初始化顺序必须保持 heap reservation → heap init → frame allocator init → final
  kernel page table。LoongArch early page table 必须覆盖高端 heap，正式 direct map 必须跳过
  PCI/MMIO 空洞。调整 heap 容量时必须同时验证实际 RAM 上限与 early direct-map 可达范围；
  若以后改为 frame-backed 可扩展 heap，需要先消除 frame allocator 初始化对 `Vec`/全局堆的依赖。

### LoongArch 用户 FP/LSX 状态按任务首次使用启用

- 状态：已实现并通过 12-hart 扩展程序与 SMP 专项
- 适用范围：LA `EUEN.FPE/SXE`、user trap、timer 抢占、task switch、fork/exec
- 最后验证：2026-08-12
- 证据：`os/src/arch/loongarch64/trap/{context.rs,trap.S,mod.rs}`；12-hart BusyBox、CAgent、
  `smp_shared_mm_probe` 与 `smp_phase3_probe`
- 内容：LA 用户 trap frame 为 816 字节，保存 GPR/PRMD/ERA、32 个 128-bit LSX/FP 寄存器、FCSR0、
  FCC0..7 和扩展状态激活标记。新 exec 初始关闭用户 FPE/SXE；首次 FPD/SXD 不推进 ERA，而是激活任务、
  恢复初始零状态并重试。未激活任务的 trap 跳过扩展寄存器搬运；激活后仍按每次 trap eager 保存恢复，
  因而无需维护跨 hart lazy owner。状态随 fork 复制、exec 清零。
- 后续影响：每个实际使用扩展的任务多一次 unavailable trap；已激活任务仍承担 eager 成本。Linux LA
  signal mcontext 的扩展状态接口尚未实现，不能把 task trap 隔离等同于完整 signal FP/LSX ABI。

### LoongArch 用户地址空间使用 10-bit ASID，切换不再完整失效 TLB

- 状态：已实现并通过 ASID 复用与 SMP 专项
- 适用范围：LA `TaskContext`、PGDL/PGDH、软件 TLB refill、task switch
- 最后验证：2026-08-12
- 证据：`os/src/arch/loongarch64/{task/switch.S,register/mod.rs,tlb_refill.S}`、
  `os/src/mm/memory_set.rs`；12-hart 1200 短进程与 rollover 后 shared-MM/Phase3 专项
- 内容：ASID 0 保留给 kernel/idle，用户 `MemorySet` 从 1--1023 分配。root 与低 10 位 ASID 组成
  软件 MMU token；切换汇编保存 CSR.ASID 时必须屏蔽只读 ASIDBITS 高位，恢复 PGDL/PGDH/ASID 后
  不执行逐切换 `invtlb`。无外部 CLONE_VM owner 的进程退出路径在数据页全在线失效后及时退役
  ASID；Drop 是幂等 fallback。编号耗尽时冻结 retired 批次，完成全在线失效后才复用。
- 后续影响：PTE writer 已按下述范围规则使用 op=4，direct map 仍非 Global。Global kernel
  TLB 必须独立验证；不能把 `invtlb op=3` 误当按 ASID 失效。

### RV64 返回用户态前必须保持 kernel trap 状态

- 状态：已实现并通过 8 核 BuildStorm 运行中 GDB 快照验证
- 适用范围：RV64 `TrapContext` 初始化、signal return、`__restore`/`sret`
- 最后验证：2026-08-07
- 证据：`os/src/arch/rv64/trap/{context.rs,trap.S}`；
  `/tmp/respos-rustc-pc-sample{1,2,3,4,5}.txt`、
  `/tmp/respos-rustc-pc-postfix-sample{1,2,3,4,5}.txt`
- 内容：恢复用户上下文期间，`stvec` 必须继续指向 kernel trap 入口，且写入
  `sstatus` 前必须清 `SIE`。只有通用/浮点寄存器与 per-CPU `sscratch` 都恢复完毕后，
  才切换到 user trap vector 并执行 `sret`；`sret` 按 `SPIE` 恢复用户态中断状态。
- 后续影响：不能信任来自 exec 或 signal frame 的保存 `SIE`。扩展 trap frame 或调整
  汇编顺序时，应把 bulk context restore 留在 kernel vector 生效期间，并把切换 user
  vector 后的固定收尾保持在最小范围。

## 内核边界

| 边界 | 当前所有者 | 不应放入调用方的内容 |
| --- | --- | --- |
| syscall → MM | `MemorySet`、VMA/PTE/frame helper | VMA 切分、COW、lazy fault、file backing 状态机 |
| syscall → task | TCB、thread group、scheduler、futex wait state | 退出回收和 waiter single-winner 规则 |
| syscall → FS | namei/VFS/FileOp/FdTable | path walk、descriptor/open-file 状态混用 |
| syscall → net | socket FileOp 与 `os/src/net` | smoltcp socket 生命周期和 listen table 细节 |
| 公共代码 → arch | `os/src/arch/{rv64,loongarch64}` | 直接散落的 CSR/寄存器、页表和 trap-frame 假设 |

## MM 模型

### `MemorySet` 拥有地址空间语义

- 状态：已确认
- 适用范围：mmap、munmap、mprotect、mremap、COW、lazy allocation、用户拷贝
- 最后验证：2026-08-01
- 证据：`os/src/mm/memory_set.rs`、`os/src/mm/mod.rs`、`os/src/syscall/mm.rs`、
  Git `15fe1a5`
- 内容：VMA 由 `MemorySet` 管理；`MmapBacking` 表达匿名/文件和 shared/private backing；
  `MapArea::split_by_overlap` 统一切分。用户拷贝逐页检查 VMA 权限、确保 lazy/COW 页可访问，
  再通过 PTE 对应物理页复制，不能直接解引用用户虚拟地址。
- 后续影响：所有地址范围必须检查加法、页对齐和用户上界；任何可能失败的替换应先准备资源，
  再原子更新 PTE/VMA，避免留下空洞。

### VMA/PTE/frame 不变量

- 状态：已确认
- 适用范围：`MemorySet` 修改路径
- 最后验证：2026-08-01
- 证据：`os/src/mm/memory_set.rs` debug invariant 与 split self-test；Git `15fe1a5`
- 内容：VMA 有序、非空且不重叠；`data_frames` 的 VPN 属于对应 VMA 并具有有效 PTE；用户
  PTE 带 USER；VMA/PTE 权限相容；私有 COW 页不同时可写；shared mapping 不进入私有 COW。
- 后续影响：debug 启动自检失败应视为结构性错误，不能通过关闭断言完成验收。

### RV64 `MemorySet` active CPU 与 shootdown

- 状态：已实现，目标 QEMU/OpenSBI 专项压力已通过
- 适用范围：RV64 SMP、共享 `MemorySet`、PTE 修改与 frame 回收前 fence
- 最后验证：2026-08-06
- 证据：`os/src/mm/memory_set.rs`、`os/src/task/processor.rs`、`os/src/task/task.rs`；
  `/tmp/respos-active-mask-shared-mm-smp{2,8}.log`
- 内容：task 恢复前在 `MemorySet` 读锁内设置当前 hart active bit；切回 per-CPU idle/kernel
  页表后才清除。页表修改持写锁，完成本地 fence 后，只向 active remote hart 发 SBI RFENCE。
  `exec` 与 clone 的临时 `activate()` 必须显式撤销旧/临时地址空间 bit。当前 OpenSBI RFENCE
  实现会等待远端 TLB 请求同步计数归零后返回。
- 后续影响：不得在仍执行旧 `satp` 时提前清 bit，也不得绕过 `MemorySet` 锁修改 PTE。移植到
  非 OpenSBI firmware 时必须重新确认 RFENCE completion 语义；未知实现不能沿用当前验收结论。

### 动态内核映射只能修改用户 root 已共享的下级页表

- 状态：已实现并验证 RV64 首个跨 1 GiB kernel-stack 边界
- 适用范围：RV64/LA64 共享高半区、用户页表创建、动态内核栈
- 最后验证：2026-08-13
- 证据：`os/src/arch/{rv64,loongarch64}/mm/page_table.rs`、
  `os/src/mm/memory_set.rs::new_kernel()`；强制 slot 16383→16384 的 RV64 release 日志
  `/tmp/respos-rv-kstack-slot16384.log`
- 内容：用户页表按值复制内核高半区的 root PTE，并共享这些 PTE 指向的下级页表。内核初始化必须在
  首个用户 root 创建前为所有尚为空的高半区 root 项准备共享下级分支；之后内核栈等动态映射仍可
  按需分配 leaf 与更低层页表，但不能再要求旧用户 root 学习一个全新的 root PTE。
- 后续影响：新增 vmalloc、per-CPU 区或其他动态高半区映射时，应复用已准备的根拓扑。若未来需要在
  运行期替换 root 项，必须建立同步传播到所有用户 root 的显式协议，不能只修改 `KERNEL_SPACE` 并
  依赖 TLB flush。

## task、scheduler 与 futex

### 调度状态只有一个所有者提交

- 状态：已确认
- 适用范围：ready/block/wakeup/exit/context switch
- 最后验证：2026-08-01
- 证据：`os/src/task/scheduler.rs`、`os/src/task/processor.rs`、Git `3aa1fb5`
- 内容：scheduler 使用 RT、normal、idle 多级队列并维护 `task_index`/blocked 集合；状态先准备，
  再由调度路径提交。退出任务通过 `DEAD_TASKS` 延迟 drop，避免在自身内核栈上释放自身。
- 后续影响：不要从多个路径重复入队或重复唤醒；close/signal/timeout 竞争必须保持 single-winner。

### CPU clock 以真实调度运行区间记账

- 状态：已实现并通过双架构单核/LTP 与 2-hart probe
- 适用范围：RV64/LA64 scheduler、thread group、CPU clock、POSIX CPU timer
- 最后验证：2026-08-14
- 证据：`os/src/task/{processor,task}.rs`、`os/src/syscall/time.rs`、
  `user/src/bin/task_a_clock_probe.rs`；`/tmp/respos-{rv,la}-cpu-clock-{cluster,probe-smp2}.log`
- 内容：idle scheduler 在切入 task 前开启 thread/process 运行区间，task 交回 idle 后关闭区间；idle
  栈保留该 task 的 Arc，因此即使 task 已从 manager 移除也能完成最后一次记账。thread clock 每任务
  独立；`CLONE_THREAD` 共享 process clock，后者以固定 per-hart slot 表示同时运行的线程并在读取时
  加上所有 live interval。锁为关本地中断的 spin lock，避免 timer trap 在同 CPU 重入。
- 生命周期：fork/new process 从零创建两类 clock，线程 clone 只共享 process clock，exec 保留累计
  时间。POSIX CPU timer 只强持有 detached clock state；thread clock 在创建线程退出后冻结，process
  clock 则由线程组累计状态继续前进，不借 timer 保留 TCB、MemorySet 或 fd table。
- 后续影响：CPU clock 的 begin/end 必须继续包围真实 `__switch`，不能移到 ready/block 状态变更处；
  SMP 实现不得退化为单一 `running_since`。若增加 CPU hotplug 或 hart 数量，必须同步审计 slot 上限与
  已运行区间。当前 `times/getrusage` 未区分 user/system，不能把两字段均为 total 解释为完整会计语义。

### 精确 task deadline 由 timer-service hart 单点编程

- 状态：已实现并通过双架构单核/SMP 专项
- 适用范围：nanosleep、poll/pselect/epoll timeout、futex timeout；RV64/LA64
- 最后验证：2026-08-13
- 证据：`os/src/arch/{rv64,loongarch64}/{timer,smp}.rs`、`os/src/syscall/{mod,time,fs,special_fd}.rs`、
  `os/src/task/futex/wait.rs`；双架构 8-case LTP 与 2-hart deadline 日志
- 内容：100 Hz 周期 tick 继续负责调度和未迁移 timer。需要精确唤醒的 waiter 在进入阻塞前发布一个
  微秒级 deadline 原子提示；timer-service hart 是唯一编程该全局最早 deadline 的 CPU，其他 hart
  通过 IPI 请求其重新读取提示。高层 timer scan 清空提示后检查权威 waiter 注册表，再由仍存活的
  waiter 重建最小值，因此原子状态只是可丢弃/可重复的 rearm hint，不承担 waiter 生命周期。
- QEMU 边界：compare 提前 800 us，timer trap 只在该有界窗口内等待软件 deadline，保证 timeout 不会
  提前可见，同时规避第二次 QEMU timer 注入的数百微秒延迟。实机采用该提前量前需要重新测量；不得
  将它解释为通用硬件延迟模型。
- 后续影响：新增精确 timeout 注册表时，注册完成后才能发布 hint，scan 必须在释放自身锁后重发下一
  deadline。IPI handler 只能读取原子状态并编程硬件，不能取得 waiter/scheduler 锁；撤销 waiter 不必
  同步删除 hint，最坏只能多一次早中断。

### `wait4` 的 SA_RESTART 在 signal frame 中保存重执行上下文

- 状态：已实现并通过双架构专项
- 适用范围：RV64/LA64 syscall trap、`wait4/waitpid`、signal frame、`sigreturn`
- 最后验证：2026-08-13
- 证据：`os/src/arch/{rv64,loongarch64}/trap/mod.rs`、`os/src/signal/mod.rs`、
  `user/src/bin/task_a_wait4_probe.rs`；`/tmp/respos-{rv,la}-wait4-restart-probe.log`
- 内容：`wait4` 因可投递信号返回 `EINTR` 时，trap 层暂存 syscall 原始 arg0。只有实际取出的用户
  handler 带 `SA_RESTART`，signal frame 才保存回退 4 字节的 syscall PC 和原始 arg0；handler 返回后
  `sigreturn` 恢复该上下文并重新执行 syscall。不带标志的 handler 继续观察到 `EINTR`，默认忽略信号
  不会触发该路径。
- 后续影响：不要在 `sys_wait4()` 内直接吞信号并继续阻塞，否则 handler 无法执行。扩展到 read、socket、
  futex 等 syscall 前必须先确认 Linux 的 restart class、partial side effect 与 timeout 剩余时间语义；
  不能把所有 `EINTR` 一律重启。

### 进程组资源回收按 live owner 判定

- 状态：已确认并由 RV64 frame 回收 A/B 验证
- 适用范围：`MemorySet`、FdTable、thread-group exit、`CLONE_VM`/`CLONE_FILES`
- 最后验证：2026-08-09
- 证据：`os/src/task/task.rs::exit_process_group()`、`user/src/bin/frame_reclaim_probe.rs`
- 内容：退出 handoff 和 `DEAD_TASKS` 允许已退出同组 TCB 暂时继续持有资源 Arc，因此引用计数不表达
  活跃所有权。进程组 teardown 通过一次 `TASK_MANAGER` live snapshot 同时判断 MemorySet/FdTable，
  只查找不同 tgid 的同资源 owner；同组临时引用不能阻止清空资源，真正存活的跨进程共享者必须阻止
  清空。
- 后续影响：zombie 可以保留 TCB 和 wait status，但不能因此保留已经确认由本退出组独占的 resident
  用户页或打开文件资源。

### SMP ready 选择与唤醒必须同时遵守 affinity

- 状态：已实现，RV64 8 核定向烟测已通过，退出压力仍有独立 blocker
- 适用范围：全局 ready queue、TCB CPU owner、RV64 idle hart IPI
- 最后验证：2026-08-06
- 证据：`os/src/task/scheduler.rs`、`os/src/task/task.rs`、`os/src/task/processor.rs`、
  `os/src/arch/rv64/smp.rs`；`/tmp/respos-affinity-smp8.log`
- 内容：每个 CPU 从最高优先级开始，在队列内选择第一个 affinity 允许且 owner 已释放的
  task；不兼容的队首保留原位。任务发布或 owner 释放后，IPI 目标只从 affinity 允许的
  online idle hart 中选择。
- 后续影响：不能只在 dequeue 中过滤 affinity。若 enqueue 仍唤醒任意 idle CPU，只允许高编号
  CPU 的任务可能永久留队。owner 在 handoff 后释放时还必须补一次定向 kick，覆盖第一次
  IPI 早于 context save 完成的窗口。

### 全局 heap 和 kernel timer work 的中断边界

- 状态：已实现，RV64 2/4/8 核退出压力已验证
- 适用范围：全局内核 heap、RV64 user/kernel timer trap、timeout/signal registry
- 最后验证：2026-08-06
- 证据：`os/src/mm/heap_allocator.rs`、`os/src/arch/rv64/trap/mod.rs`；
  `/tmp/respos-smp8-gdb-bt1.txt`、`/tmp/respos-smp2-dynamic-bt.txt`
- 内容：`LockedHeap` 的跨 CPU 自旋锁不能防止同 CPU 中断重入；所有 heap alloc/dealloc 及直接
  heap guard 都必须在本地 `InterruptGuard` 内执行，解锁顺序为先 heap、后恢复中断。
  `check_all_task_timers()` 会进入 task/signal/timer 高层锁，不得从中断任意 kernel 临界区的
  timer trap 重入。RV64 当前安全点是 user-mode timer trap，以及 boot hart 上
  `current_task == None` 的 per-CPU idle context。另有显式 no-lock syscall 安全点供长时间停留在
  kernel mode 的阻塞重试路径消费延迟 timer work：inet socket 尚未提供事件式 poll waiter，
  `ppoll/pselect` fallback 会在 yield 前调用该安全点；TCP/UDP `block_on` 在新一轮协议 poll 前调用。
  显式路径仍只允许 timer-service hart 扫描，并按 monotonic millisecond 至多执行一次。
- 后续影响：新增中断内工作前要列出其触及的所有锁；若需在长 syscall 期间精确处理
  timeout，应建立 lock-free pending + 安全点延迟工作，不能直接恢复 kernel trap 里的高层扫描。
  新增“循环内 yield 但不返回用户态”的 syscall 还必须证明会到达 idle，或显式接入同一 no-lock
  安全点；接入点不得持有 FileOp、socket、task、signal 或 timer registry 锁。

### futex 锁内用户访问边界

- 状态：部分实现；普通 wait 待收敛
- 适用范围：futex wait/wake/requeue
- 最后验证：2026-08-10
- 证据：`os/src/task/futex/wait.rs::{futex_wait_common,futex_wait_timed_common,futex_requeue_common}`、
  `os/src/mm/mod.rs::read_user_u32_nofault`、Git `3aa1fb5`
- 内容：`FUTEX_CMP_REQUEUE` 已在队列锁外预先确认用户页可读，锁内只做固定 4 字节 no-fault
  PTE 读取，使比较与 waiter 迁移处于同一临界区。普通和定时 `FUTEX_WAIT` 当前仍在持有
  `FUTEX_QUEUES` 时调用通用 `copy_from_user` 复核用户值，lazy/COW 页可能进入补页路径。
  wait completion 已区分 Pending/Woken/TimedOut/Interrupted，并保持单赢家。共享 futex key 对
  shared file 和已有共享 frame 使用 backing 身份，但 System V shm 当前使用每次 `shmat` 独立分配的
  attach id；两个进程分别 attach 同一段时，key 不能保证相同。
- 后续影响：普通 wait 应复用“锁外预触页、锁内 no-fault 复核”的模式；在完成前不能宣称
  futex queue lock 内已全面禁止通用用户拷贝或潜在 frame 分配。System V shm 的 futex key 应改用
  segment/frame 的稳定共享身份，并增加独立 `shmat` 的跨进程 futex probe。

## FS、VFS 与 fd 模型

```text
FdTable slot (FdEntry: descriptor flags)
  → Arc<dyn FileOp> (open-file description: offset/status)
  → File (Path + InodeOp + page cache)
  → Path (VfsMount + Dentry)
  → Dentry (name/parent/inode)
  → InodeOp / ext4 backend
```

### descriptor 与 open-file description 分层

- 状态：已确认
- 适用范围：dup/fcntl/close/exec/CLOEXEC/offset/status flags
- 最后验证：2026-08-01
- 证据：`os/src/fs/fdtable.rs`、`os/src/fs/file.rs`、Git `cba8e24`
- 内容：`FdEntry` 保存 descriptor flags；`FileInner` 保存共享 offset、path 和 open-file status
  flags。dup 后 descriptor 可独立设置 CLOEXEC，但共享同一个 open-file offset/status。
- 后续影响：实现新 fcntl 或 clone/exec 行为时，不得把 CLOEXEC 写进共享 `File` flags。

### namei 保留 final-component 策略与 trailing-slash 约束

- 状态：已实现并由 Linux/RV64 对照验证
- 适用范围：open/create/link/unlink/rename/stat/readlink、symlink 与 mount crossing
- 最后验证：2026-08-11
- 证据：`os/src/fs/namei.rs`、`scripts/fs_phase4_probe_linux.c`、
  `user/src/bin/fs_phase4_probe.rs`
- 内容：`Nameidata` 在切分非空 path segment 的同时单独保留原路径是否以 `/` 结束；普通 lookup 的
  trailing slash 会强制最终对象为目录，并在需要时穿过最终 symlink。rename 使用独立的 no-follow
  final-component 策略，link 默认不跟随旧 symlink，只有 `AT_SYMLINK_FOLLOW` 改变该选择。跨 symlink
  或 mount 后，结果 `Path` 的 mount 与 dentry 必须一起传播。
- 后续影响：不能只用 `split('/').filter(nonempty)` 表示完整 Linux pathname；新增 `*at` syscall 时
  必须先明确 final symlink、trailing slash、empty path 与 mount crossing 策略，再复用相应 namei
  入口，不能在 syscall 层重新拼路径或事后猜 errno。

### create 在属性提交成功后发布 dentry

- 状态：已实现
- 适用范围：open(O_CREAT)、mkdir/mknod/mkfifo、symlink
- 最后验证：2026-08-11
- 证据：`os/src/fs/namei.rs`、`user/src/bin/fs_phase4_probe.rs`
- 内容：在全局 namespace mutation 临界区内先准备 mode/uid/gid，调用 lower create，再对未发布的
  child dentry 提交属性；只有全部成功才插入 parent/dentry cache。属性失败会通过 parent inode
  unlink 回滚 lower entry。这样 syscall 不会返回错误却留下默认 root owner/mode 的半成品。
- 后续影响：任何新增 namespace create 操作都必须保持 prepare → lower create → metadata commit →
  publish 顺序；若 lower backend 不能回滚，必须先建立它自己的 transaction，不能恢复忽略 setattr
  错误的做法。

### ext4 命名 FIFO 的类型由 lower inode 持久化，运行态缓冲由 VFS 管理

- 状态：已实现并由双架构 LTP 验证
- 适用范围：`mknod(S_IFIFO)`、`mkfifo`、FIFO stat/open/read/write/lseek/fsync
- 最后验证：2026-08-14
- 证据：`os/src/fs/{ext4/inode.rs,pipe.rs}`；RV64/LA64 musl/glibc 的
  `fsync03,lseek02,open06,read03,write04`
- 内容：ext4 create 必须用 lower `ext4_mknod(EXT4_DE_FIFO)` 写入 FIFO inode type，之后 lookup/stat
  才能稳定恢复 `InodeType::Fifo`。`sys_openat` 根据这个持久类型把 pathname inode 转为
  `NamedFifoEnd`；同一路径的运行态 reader/writer 共享 VFS `PipeRingBuffer`，关闭最后一个端点后释放，
  不把瞬时管道内容写入 ext4。
- 后续影响：不能用普通文件占位再只改低 12 位 permission 假装特殊 inode；否则 reopen/readdir/stat
  会继续识别成 regular，并绕过所有 FIFO errno/阻塞语义。字符/块设备仍需先把 `dev` 参数贯穿 create
  接口后再切换 lower mknod，不能照搬 FIFO 的固定 payload 0。

### ext4 inode 属性以底层 transaction 成功为发布点

- 状态：已实现 mode/owner/秒级 times 组合提交
- 适用范围：chmod/chown/utimens、目录属性、hardlink alias、unlink 后打开 fd
- 最后验证：2026-08-10
- 证据：`os/src/fs/{ext4/inode.rs,file.rs}`、`vendor/lwext4_rust/c/lwext4/src/ext4.c`；
  `fs_metadata_probe` Linux/RV64 对照与跨启动 qcow2 检查
- 内容：`ext4_setattr_ino` 在一个 inode ref/transaction 内更新选定字段；Rust mode/owner/times cache
  仅在 lower commit 成功后发布。fd 操作直接使用后端 inode number，不再依赖可见路径或隐藏 orphan
  名字；同 inode hardlink 共享属性缓存。
- 后续影响：新增 setattr 字段必须加入同一 prepare/commit/publish 协议；不得把 `ENOENT` 转成成功后只改
  Rust override。持久层仍只有 32-bit 秒精度，纳秒/epoch 扩展需另做 on-disk 设计与跨重启测试。

### PageCache 是普通文件驻留页的唯一所有者

- 状态：已实现，完整 BuildStorm 已越过旧 heap 阻断；并行 rustc SIGSEGV 已定位为独立退出回收缺陷
- 适用范围：普通文件 buffered I/O、MAP_SHARED、truncate、缓存回收
- 最后验证：2026-08-09；RV64/LA64 release、RV64 8 核专项 probe 与 8 GiB BuildStorm
- 证据：`os/src/fs/page_cache.rs`、`os/src/fs/file.rs`、`os/src/mm/memory_set.rs`
- 内容：每个 PageCache page 持有一个 `Arc<FrameTracker>`；普通文件共享映射克隆同一 frame，VMA 的
  frame 强引用同时充当 cache pin。PageCache 元数据仍负责 dirty/version/LRU，truncate 在移除页前
  清零 frame，从而让仍存活的映射同步观察 EOF 后清零。无 PageCache 文件才使用 MM 全局弱表后备。
  当前全局工作集上限为 32768 页（128 MiB），底层文件 read miss 最多做 16 页顺序预读；I/O 在缓存锁
  外完成，安装前以文件长度代次排除并发 truncate 的旧快照。
- 后续影响：回收页时必须同时检查 Page 对象和 frame 的强引用；普通文件 read/write 不应再复制或
  overlay 一份 mmap 缓冲。若修改 mmap writeback，dirty 状态仍应归并到该唯一 PageCache page。

### PageCache 写回完成与错误按 batch/version 提交

- 状态：Phase 3 已完成
- 适用范围：普通文件 buffered I/O、close、fsync/fdatasync/sync/syncfs、unmount、MAP_SHARED 写回
- 最后验证：2026-08-11；RV64/LA64 构建、RV64 1/16 GiB 专项、故障注入、跨启动持久化与完整 BuildStorm
- 证据：`os/src/fs/{page_cache.rs,file.rs}`、`os/src/perf.rs`、
  `user/src/bin/fs_writeback_probe.rs`
- 内容：页在 dirty 之外保存 write-version、当前 writeback batch id 和最近错误。锁外 lower I/O 返回后，
  只有仍持有该 batch id 的完成者能结束 writeback；仅当 write-version 未改变时清 dirty，并发 write 或
  truncate 会使旧快照失效。PageCache 另维护单调 error sequence；每个独立 open 的 `FileInner` 保存
  cursor，dup/fork 共享，因而一次失败可由所有错误发生前已打开的 description 各观察一次，而新 open
  不继承历史错误。独立 dirty-owner 表在最后一个 File 消失后继续强持有 inode/PageCache/filesystem；
  safe-point writeback 每次最多处理 8 个 owner，单 cache 256 脏页或全局 128 owner 触发，失败 owner
  保持 dirty/error 且不在每个 syscall 上忙重试。`sync`/`syncfs`/unmount/shutdown 在后端 barrier 前遍历
  对应 owner；普通 close 不提交数据。data write 的共享 inode mtime/ctime 先在内存发布，数据成功后再
  持久化时间；truncate 与 lower writeback 由 inode PageCache 的 writeback lock 串行。
- 后续影响：任何新 writeback 入口都必须通过同一完成/错误发布协议，不能在 lower 返回前清 dirty，
  也不能只把错误返回给发起写回的 syscall。dirty owner 只在数据和待提交时间都干净时移除；新增
  truncate/hole/invalidate 路径必须同步维护该闭环。当前没有硬件 PTE dirty bit，MAP_SHARED resident
  页仍保守写回，但所有路径共享同一个 PageCache frame。

### filesystem ELF 使用按需 private file backing

- 状态：已实现，BuildStorm tg-xtask、手工链接与完整 xtask wrapper 已验证
- 适用范围：filesystem exec、动态程序、大 ELF、kernel heap
- 最后验证：2026-08-08；RV64 release、8 核、8 GiB
- 证据：`os/src/mm/memory_set.rs::try_from_elf_file()`、
  `os/src/task/task.rs::execve_file()`、`read_elf_metadata()`、`open_dynamic_linker()`
- 内容：exec 只把 ELF/program header 与 PT_INTERP 名称读入 kernel heap；主 ELF 的 PT_LOAD VMA
  持有 `Arc<dyn FileOp>`、page-aligned file offset 和有效文件长度，private fault 时分配独立 frame
  并按页读取，BSS 尾部保持零。文件系统中的动态解释器也走相同的 lazy PT_LOAD 路径；文件式 loader
  将元数据前缀限制为 1 MiB，并在安装 VMA 前校验 ELF64 program-header 尺寸和 PT_LOAD 文件边界；
  嵌入式 app/解释器 fallback 仍使用完整 slice 的 eager loader。
- 后续影响：PT_LOAD offset 与 virtual address 的页内偏移必须同余；不能重新用 `read_all()` 或
  单纯放大 kernel heap 规避大 ELF。工具链并发峰值仍需要 128 MiB kernel heap，但这与 ELF 整文件
  分配是两个独立问题。

### 用户栈是大范围 lazy VMA，不等于启动时一次性物理分配

- 状态：已实现，旧 BuildStorm 镜像中的 tg-xtask 已验证
- 适用范围：exec argv/envp/auxv、大型动态程序、线程初始栈
- 最后验证：2026-08-08；RV64 release、8 核、8 GiB
- 证据：`os/src/arch/{rv64,loongarch64}/config/mm.rs`、
  `os/src/mm/memory_set.rs::try_from_elf_file()`
- 内容：RV64/LA64 主用户栈窗口为 8 MiB，但 VMA 保持 lazy；exec 只确保初始启动数据实际覆盖的页
  存在。原 512 KiB 栈在 tg-xtask 启动时越过 guard，并把相邻动态解释器 RX VMA 的权限故障伪装成
  loader 问题。
- 后续影响：诊断解释器地址附近的 store fault 时必须同时计算 SP、栈 VMA 与 guard 距离；扩大栈窗口
  时不能退回 eager 分配全部页面。

### vfork 父任务必须先登记 blocked 再发布子任务

- 状态：已实现，完整 BuildStorm 仍待验证
- 适用范围：RV64 SMP `CLONE_VFORK`、posix_spawn、Rust Command/cargo
- 最后验证：2026-08-06；RV64 release、8 核受控 trace
- 证据：`os/src/syscall/process.rs::sys_clone()`；
  `/tmp/respos-buildstorm-cloneflags.log`、`/tmp/respos-buildstorm-exittrace.log`
- 内容：vfork parent 的 blocked registration 是 wakeup 协议的一部分，必须发生在 child 加入全局
  ready queue 之前。发布后 child 可在任意 CPU 立即 exec；exec/exit 的一次性 wake 此时必能从
  blocked 表取回 parent。父任务登记后再发布 child，随后直接切到 idle/下一任务。
- 后续影响：任何“发布对象后再登记 waiter”的一次性事件都要审计同类 lost-wakeup 窗口；单核
  测试因 child 无法提前运行，不能覆盖该顺序错误。

### path、dentry、inode 与 mount 各有身份

- 状态：已确认
- 适用范围：namei、rename/unlink、mount crossing、proc/dev/ext4
- 最后验证：2026-08-01
- 证据：`os/src/fs/path.rs`、`os/src/fs/vfs/dentry.rs`、`os/src/fs/namei.rs`、
  `os/src/fs/mount.rs`
- 内容：`Path` 是 mount+dentry；dentry 表达目录项身份和父子关系；inode 表达文件对象；路径
  查找由 namei 处理 `dirfd`、`.`/`..`、symlink 与 mount crossing。
- ext4 inode cache 以真实后端 inode number 为 identity，不再为新建文件生成 synthetic inode。数据、
  属性、readdir 与 readlink 通过 inode number 进入 lwext4，不在 inode 中维护 pathname alias 或隐藏
  orphan path。
- `i_nlink == 0` 只表示 inode 已脱离 namespace，不表示可以立即释放。最后 unlink/rename 覆盖先让
  lower inode 进入 nlink=0 状态；`File`、cwd、`Path`、`Dentry` 等共同持有 `Ext4Inode` Arc。最后一个
  Arc 的 Drop 只把 inode number 放入延迟队列，syscall 前后安全点或 shutdown 再持统一 ext4 锁执行
  truncate/free，避免在 Drop 或未知 VFS 锁下进入 lwext4。
- 每个 ext4 目录 inode 持有独立 metadata generation；成功 namespace mutation 在 lower commit 后失效
  实际源/目标父目录，不再用全局 generation 使所有目录快照失效。
- 后续影响：不能用 open-file 计数替代 inode 引用生命周期，因为 cwd 与临时 Path/Dentry 同样是有效
  引用；也不能在 `Drop` 中直接取得 ext4 锁。当前 syscall 安全点把正常运行时的回收窗口限制到下一次
  syscall，但 lwext4 尚无完整 ext4 orphan-list 崩溃恢复，异常断电后的 nlink=0 inode 清理仍待实现。

## 网络模型

### smoltcp 回环路径是当前主要实现

- 状态：已确认
- 适用范围：TCP/UDP socket 与本地网络 benchmark
- 最后验证：2026-08-03
- 证据：`os/src/net/mod.rs`、`os/src/net/listen.rs`、`os/src/net/socket.rs`、
  `os/src/syscall/net.rs`；RV64 CAgent 三轮并发回归
- 内容：socket syscall 经 FileOp socket 对象进入 TCP/UDP 实现，全局 `SocketSet`、loopback
  interface/device 和 listen table 由锁保护，`poll_interfaces` 驱动协议栈。smoltcp 的单个
  listener 完成握手后会成为连接 socket，因此 listen table 按受限 `listen(backlog)` 预建
  listener 池；poll 将已连接 handle 转入 accept queue 并及时补位，避免同一轮并发 SYN 在
  userspace `accept` 前收到 reset。
- 后续影响：网络失败应先区分 ABI、协议状态和测试服务端启动时序，不能只凭一次
  `Connection refused` 判断内核网络语义损坏。修改 poll/accept/close 时必须同时维护 listener
  池、accept queue 和 socket handle 的唯一所有权，避免泄漏或重复 remove。

### TCP 阻塞等待同时使用事件唤醒和短定时兜底

- 状态：已确认
- 适用范围：`TcpSocket::block_on`、smoltcp loopback 双端阻塞和空闲 listener
- 最后验证：2026-08-11
- 证据：`os/src/net/mod.rs`、`os/src/net/tcp.rs`；RV64 iperf musl 六个 UDP/TCP 模式
- 内容：TCP 操作返回 `EAGAIN` 时先公布 waiter，再 poll 并二次检查条件，阻塞后由
  `poll_interfaces()` 唤醒等待任务。同时保留 1 ms task-timeout 兜底，以维持现有 idle/listener
  场景的定时器推进语义。
- 后续影响：不能只改成固定 sleep 或只改成 yield；新等待路径必须保留“登记后复查”
  的丢失唤醒防护，并同时回归 TCP 双端交互与空闲 listener 旁的 sleep/timeout。

## 双架构差异

| 项目 | RISC-V 64 | LoongArch 64 |
| --- | --- | --- |
| 目标 | `riscv64gc-unknown-none-elf` | `loongarch64-unknown-none` |
| QEMU 启动 | OpenSBI/`-bios`，物理入口 `0x80200000` | `-kernel` ELF，物理入口 `0x00200000` |
| block 设备 | virtio-mmio | virtio-pci |
| 高半区 | `0xffffffc0...` | 同一共享高半区模型 |
| 早期分页 | 启动路径已进入公共高半区模型 | `rust_main` 显式启用 boot paging 后跳高半区 |
| 内核栈/堆 | 约 60 KiB 栈、64 MiB 堆 | 32 KiB 栈、48 MiB 堆 |

### SMP 启动与调度边界

- 状态：部分验证
- 适用范围：多核启动、per-CPU idle/context、跨核调度唤醒、地址空间切换
- 最后验证：2026-08-13
- 证据：`arch/{rv64,loongarch64}/smp.rs`、`task/processor.rs`、LoongArch QEMU
  `-m 12G -smp 12` 串口 online mask 与 BuildStorm 并行编译
- 内容：RV64 通过 SBI HSM/IPI，LoongArch QEMU-virt 通过 IOCSR mailbox/IPI 启动 secondary。
  每个 hart 使用独立 early/idle stack、processor/current-task 状态和本地 timer，ready queue 仍为
  全局串行调度器；enqueue 后会向满足 affinity 的 idle hart 发送 IPI。LoongArch 的 boot hart
  在进入用户态前最多等待 1 秒收集 online mask，较小的 `-smp` 覆盖不会阻止启动。
- 页表失效：RV64 通过 SBI RFENCE 对 active address-space mask 做同步远端 TLB shootdown；
  LoongArch 通过 IOCSR IPI vector 1 和 per-target generation request/ack 槽完成同一协议。页表 root
  切换不发远端请求；LA 槽同时携带 `all`、`address-space` 或 `range`、ASID 和页对齐区间。
  ASID 批量回收发布 `all`；普通 PTE writer 有叶修改时发布带 ASID 的 `range`，无实际叶修改时
  发布 `address-space` 请求。
  `MemorySet` 另以 residency mask 记录自上次同步失效后加载过其 ASID 的 hart，只向该集合
  shootdown，完成后把 residency 收缩回 active mask。不能直接只用当前 active mask：已经切到 idle
  的 hart 仍可能保留同 ASID 的旧项。
- frame 回收：LA 清除或替换的旧数据页由所属 `PageTable` 保持强引用，因而批次与唯一
  `MemorySet`/ASID 绑定。PTE writer 冻结本地址空间批次，对其 residency 中每个可能保留旧转换的
  hart 完成同步失效和 ack 后才释放 frame。共享 frame 若仍被其他地址空间映射，其 `Arc`
  仍阻止物理页归还。页表页不再使用固定容量 quarantine；`recycle_data_pages()` 将它们移入
  所属 `PageTable` 的退役槽，最后一个 active hart 在 `__switch` 已恢复 idle/kernel root 后清 bit，
  并在 `active_hart_mask` 变为 0 的单一转换点释放页表页。本来就没有 active hart 的构造失败/未激活
  地址空间可立即释放。LA 内核态通常关中断，SpinMutex 和 MemorySet RwLock 的等待点会轮询 IPI。
- 执行：已校验的 address-space 与 range 请求均执行一次 `invtlb op=4`，避免在 IPI handler 中
  形成与长度相关的循环。`all`、root 激活和非法请求仍执行 op=0。运行期 kernel/user PTE 当前均
  不设置 Global 位，因此 op=4 覆盖目标 ASID 的共享高半区项；boot root 的 Global 映射由 root
  激活保留的 op=0 覆盖。
- 范围传播：LA `PageTable` 的每个成功叶 PTE 写入口累积半开 VPN 最小包络，flush 时与所属
  retired-data-frame 批次一起冻结；有包络则发布 range，没有实际叶修改则发布 address-space。
  新 root 首次 activate 的 op=0 会清除构建期包络。包络是保守上界，可覆盖稀疏修改之间的页；
  当前所有包络均以一次 op=4 覆盖；最大实测包络为 10938 页。
- 边界：Global kernel 映射仍须重新验证 software refill、generation、成对 G 位与 frame 回收顺序；
  不能把 retired-frame 所有权恢复为跨 MemorySet 全局队列，也不能只依据 active mask 释放旧 frame。
  重新启用 op=5 前必须解释完整 BuildStorm 内存破坏并重跑完整 final，而不只是短窗口和专项。
