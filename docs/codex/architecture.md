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
  读取 `mode=preliminary|final`；缺失、无效或 preliminary 时 exec 原内嵌 `testrunner`，final
  时在 `/glibc` 中使用 `/bin/bash` 严格串行运行当前官方决赛镜像固定的
  `cagent_testcode.sh`、`buildstorm_testcode.sh`，全部结束后关机。dispatcher 失败时
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

### kernel heap 是启动期 RAM 预留区，不属于 ELF/BSS

- 状态：已实现，RV64/LoongArch 启动烟测通过
- 适用范围：early direct map、全局 buddy allocator、frame allocator 初始化边界
- 最后验证：2026-08-11
- 证据：`os/src/mm/{heap_allocator,frame_allocator}.rs`；RV64/LA64 release 构建；RV64
  `-m 512M -smp 1 -snapshot` 运行到 libcbench，LoongArch 同配置进入 `basic-musl`
- 内容：buddy bitmap 与 heap storage 不再声明为静态 BSS，而是从页对齐的 `ekernel` 之后连续
  预留；两者通过已经生效的 high-half direct map 访问。`init_heap()` 返回物理预留末端，frame
  allocator 必须排除该区间，不能再次分配这些页。RV64 heap 位于 `ekernel` 后；LoongArch
  QEMU 12 GiB RAM 实际分为 `0..0x10000000` 与 `0x80000000..0x370000000`，256 MiB heap
  位于高端段起始处，frame allocator 同时管理扣除内核/heap 后的两个不连续区间。容量仍在启动时
  固定，运行期不扩容。
- 后续影响：初始化顺序必须保持 heap reservation → heap init → frame allocator init → final
  kernel page table。LoongArch early page table 必须覆盖高端 heap，正式 direct map 必须跳过
  PCI/MMIO 空洞。调整 heap 容量时必须同时验证实际 RAM 上限与 early direct-map 可达范围；
  若以后改为 frame-backed 可扩展 heap，需要先消除 frame allocator 初始化对 `Vec`/全局堆的依赖。

### LoongArch 用户 trap eager 保存 FP/LSX 状态

- 状态：已实现；BuildStorm 工具链与 minibuild 回归通过
- 适用范围：LA `EUEN.FPE/SXE`、user trap、timer 抢占、task switch、fork/exec
- 最后验证：2026-08-11
- 证据：`os/src/arch/loongarch64/trap/{context.rs,trap.S}`；`/tmp/respos-la-lsx-context.log`
- 内容：LA 用户 trap frame 为 800 字节，保存 GPR/PRMD/ERA、32 个 128-bit LSX/FP 寄存器、
  FCSR0 和 FCC0..7；关键 Rust field offset 与总大小由 const assertion 固定。扩展状态保存在任务内核栈
  的固定 trap frame 中，因此随任务调度隔离，并由 fork 复制、exec 清零。
- 后续影响：当前 eager save/restore 优先保证正确性；lazy extension state 可作为性能优化。Linux LA
  signal mcontext 的扩展状态接口尚未实现，不能把 task trap 隔离等同于完整 signal FP/LSX ABI。

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

## task、scheduler 与 futex

### 调度状态只有一个所有者提交

- 状态：已确认
- 适用范围：ready/block/wakeup/exit/context switch
- 最后验证：2026-08-01
- 证据：`os/src/task/scheduler.rs`、`os/src/task/processor.rs`、Git `3aa1fb5`
- 内容：scheduler 使用 RT、normal、idle 多级队列并维护 `task_index`/blocked 集合；状态先准备，
  再由调度路径提交。退出任务通过 `DEAD_TASKS` 延迟 drop，避免在自身内核栈上释放自身。
- 后续影响：不要从多个路径重复入队或重复唤醒；close/signal/timeout 竞争必须保持 single-winner。

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
  `current_task == None` 的 per-CPU idle context。
- 后续影响：新增中断内工作前要列出其触及的所有锁；若需在长 syscall 期间精确处理
  timeout，应建立 lock-free pending + 安全点延迟工作，不能直接恢复 kernel trap 里的高层扫描。

### futex 锁内不触发用户缺页

- 状态：已确认
- 适用范围：futex wait/wake/requeue
- 最后验证：2026-08-01
- 证据：`os/src/task/futex/wait.rs`、`os/src/mm/mod.rs`、Git `3aa1fb5`
- 内容：可能 fault 的用户页检查在 futex queue 临界区之外完成；比较重排锁内使用固定 4 字节
  no-fault PTE 读取。wait completion 区分 Pending/Woken/TimedOut/Interrupted。
- 后续影响：不能在全局 futex queue lock 下调用一般 `copy_from_user` 或分配 frame。

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

### 当前不是已验证 SMP 内核

- 状态：待验证
- 适用范围：多核调度、全局锁、TLB shootdown
- 最后验证：2026-08-01
- 证据：顶层 `Makefile` 默认 `SMP=1`；整合审查明确缺少真实 SMP 压力证据
- 内容：构建和 QEMU 参数允许设置 `SMP`，但当前验收只覆盖单核，不能据此宣称多核正确。
- 后续影响：任何 SMP 工作要单独设计 per-CPU 状态、跨核唤醒和 TLB 同步测试。
