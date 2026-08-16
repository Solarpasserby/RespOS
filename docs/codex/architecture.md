# RespOS 架构与不变量

本文只记录当前源码可验证的结构和长期约束。更细的专题设计可继续阅读
`docs/mm模块基础说明.md`、`docs/task模块核心功能说明.md`、
`docs/ltp-fs-abi-design.md` 与 A/B/C 重构文档。

## Regular-file advice 与 PageCache

- 状态：六种 Linux/POSIX advice 当前范围已实现并双架构验证
- 适用范围：`fadvise64`、buffered read/pread、file-backed mmap fault、PageCache reclaim/writeback
- 最后验证：2026-08-16
- 证据：`os/src/fs/{file,page_cache}.rs`、`user/src/bin/fadvise_phase5_probe.rs`、
  `scripts/fadvise_phase5_probe_linux.c`；双架构专项、LTP、mmap 与 rusage 日志见
  [current-status.md](./current-status.md)。
- 内容：advice 状态属于 `FileInner`，因此 dup/fork 共享而另一次 open 独立。NORMAL/RANDOM/SEQUENTIAL
  选择 16/1/32 页读取窗口；NOREUSE 控制已有页的 LRU promotion。buffered I/O 与 mmap fault 必须从同一
  PageCache 取 frame，并传递同一 advice 状态。WILLNEED 可包含边界部分页并尽力同步预取。DONTNEED 先
  启动范围写回，再只失效 advice 完整覆盖且 clean/unmapped/unpinned 的页；到达 EOF 时末尾部分页可失效。
  writeback/prefetch 错误不改变 fadvise 返回值，失败 dirty page 留给既有 error cursor。
- 后续影响：不能把 advice 放在 fd table entry，否则 dup 不共享；不能无条件驱逐部分覆盖页、dirty 页、
  mmap pin 或无 lower backing 的 tmpfile。引入异步 flusher 时必须保持“先 writeback、后 invalidation”顺序。

## File size grow 与 sub-page filesystem block 的 page-mkwrite 边界

- 状态：当前 ext4/synthetic block mount 范围已实现并双架构验证
- 适用范围：writable `MAP_SHARED`、`ftruncate` grow、filesystem block size 小于 VM page、ENOSPC/SIGBUS
- 最后验证：2026-08-16
- 证据：`os/src/fs/{dev,mount,file,page_cache}.rs`、`os/src/mm/memory_set.rs`；双架构双 libc
  `mmap16` 与 Phase 5 mmap probe 日志见 [current-status.md](./current-status.md)
- 内容：mount geometry 必须区分 block-device capacity、formatted filesystem capacity 和 filesystem
  block size。若 grow 越过 old EOF 所在 filesystem block、仍停留在同一 VM page 且确实暴露新范围，
  按稳定 `(dev, ino)` 扫描 live shared mappings，将对应 resident PTE 写保护；LoongArch 清 W/D，RV64
  清 W。下一次 store 经 page-mkwrite 为尚未覆盖的 page prefix 建立 backing，成功后才恢复 WRITE；
  ENOSPC 由 trap 转为 SIGBUS。shrink 同时裁剪 PageCache 的 reservation prefix，避免 truncate/regrow
  复用失效预留。
- 后续影响：不能只依据 VM page size 判断 grow 是否需要 refault，也不能把 device admission size 当成
  filesystem 可分配空间。真实 formatter 可从 ext superblock恢复 geometry；bundled no-op mkfs 必须显式
  记录参数。修改 PTE 后必须完成跨 hart TLB shootdown。

## 特殊零填充映射与 `MAP_GROWSDOWN`

- 状态：当前 `/dev/zero` 与单页 stack-growth 范围已实现并双架构验证
- 适用范围：`mmap10`、`mmap18`、user-mode page fault 与 kernel user-copy
- 最后验证：2026-08-16
- 证据：`os/src/fs/{dev/zero,file,vfs/inode}.rs`、`os/src/syscall/mm.rs`、
  `os/src/mm/memory_set.rs`与双架构 trap 入口；日志见 [current-status.md](./current-status.md)。
- 内容：只有 inode/file 显式声明 `mmap_zero_filled` 才绕过 file EOF，普通零长文件仍必须
  SIGBUS。`MAP_GROWSDOWN` 只能在用户 trap 传入的 SP 位于紧邻下方 fault page 时扩展，
  并与前一 VMA 保持 256 页 guard gap；不带 SP 的内核代访路径不会扩展。
- 后续影响：不能以 inode size=0 通用推导设备 mmap 语义，也不能见到 grow-down
  VMA 就对任意下方 fault 扩展。若后续支持跨多页栈跳跃，仍须保留 SP 与 guard-gap
  验证，不得让 `copyin/copyout` 变成隐式栈增长触发器。

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
  进入公共高半区路径前还有早期分页和架构扩展初始化；secondary 可以先发布 online bit，但必须等
  boot hart 完成 bounded discovery、启用 timer interrupt 并编程首个 compare 后才进入 scheduler。
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
- 最后验证：2026-08-15
- 证据：`os/src/arch/loongarch64/config/board.rs`、
  `os/src/arch/loongarch64/{mm/page_table.rs,tlb_refill.S}`、`os/src/mm/memory_set.rs`；结果见
  [current-status.md](./current-status.md)。
- 内容：boot hart 在关闭 DMW0 前从 QEMU virt `0x1e020000` fw_cfg MMIO 读取
  `FW_CFG_RAM_SIZE`，以 256 MiB low RAM 和 `0x80000000` high RAM 起点换算实际 high end。
  正式页表保留 low RAM 的 4 KiB 权限映射；最终 root 首次激活前，已有 kernel half 仅对偶/奇两页
  都有效的 4 KiB leaf 成对设置 Global。高 RAM 仍使用非 Global 的 PMD 2 MiB huge leaf；运行期新增
  的 kernel-stack leaf 也保持 ASID-scoped。软件 refill 必须区分 table pointer 与 bit-6 huge leaf，
  后者不能执行 table-pointer 解码的 `-1`。fw_cfg 无效时保留 12 GiB 兼容上限，发现值最高钳制到
  比赛 36 GiB。
- 后续影响：36 GiB 支持不能退回逐页 direct map；调整 RAM 上限时必须同时审计 39-bit VA、物理
  地址位宽、PMD 对齐和 frame allocator bitmap。fw_cfg 只在 DMW0 生效的 boot hart 早期读取，
  secondary 不得重复访问或修改全局物理上限。Global 只覆盖启动期已审计的成对 4 KiB leaf；不得
  外推到 huge/runtime mapping，也不得绕过 op=4 本地/远端失效、residency mask 或帧退役完成协议。

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

### `/proc/uptime` 使用硬件单调时间域

- 第一列 uptime 由 `get_timeout_us()` 生成，使用 timer/deadline 的硬件频率，
  不依赖可调的 user/accounting clock；因此可作为客体内 workload wall time。
- 第二列是所有 hart 在 `wait_for_interrupt()` 中完成的 idle 时间之和。
  记账按 hart 和 cache line 分片，每次 idle 返回只更新本 hart 原子；
  当前尚未返回的 idle 区间最多滞后一个 timer period，不会倒退。
- 输出保持 Linux `uptime idle\n` 的两位小数格式。不得用 user clock
  频率、常量或 workload 特判生成该文件。

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

### file mmap 的 live EOF、private provenance 与 truncate/punch invalidation

- 状态：M3 EOF/truncate、ext4 punch-hole 与 shared-write ENOSPC 已通过双架构专项
- 适用范围：普通 file mmap、MAP_SHARED/MAP_PRIVATE、ftruncate、fallocate punch、PageCache、COW、SMP TLB
- 最后验证：2026-08-16
- 证据：`os/src/mm/memory_set.rs`、`os/src/fs/file.rs`、`mmap_phase5_probe`；
  `/tmp/respos-{rv,la}-mmap-punch-phase5-errors.log`
- 内容：普通 mmap backing 以 live EOF 判定 fault；整页起点越当前 EOF 返回 SIGBUS，扩容后可再次
  fault-in。clean private file page 共享 PageCache frame并以只读+COW PTE 表示，首写才成为匿名 private
  page。truncate 先更新 backing/清零尾页，再在不持 File lock 时两阶段扫描唯一 MemorySet，移除新 EOF
  外所有 resident file PTE/frame，并复用地址空间既有 TLB flush/shootdown。
- punch-hole 边界：lower ext4 只对已分配的边界块清零并释放范围内完整块；PageCache 对部分页原地清零、
  对完整页失效。跨地址空间扫描按 inode identity 撤销 shared/clean-private 的完整 punched PTE，使其
  refault hole；已完成 COW 的 writable private 页是匿名 provenance，不再随 backing hole 改写。
- shared write fault：writable shared file resident PTE 初始 write-protected；store fault 先由 File backing
  物化/确认当前页的持久块，成功后才开放 WRITE，`ENOSPC` 由 trap 转为 SIGBUS。当前 ext4 在没有
  unwritten-extent API 时写入 frame 的当前字节来物化 hole，并以 File state lock 与 truncate 串行；释放
  空间后的新 fault 可以重新成功，不把 mapping 永久标坏。
- ELF 边界：`PT_LOAD` 是固定 file-backed prefix，不使用普通 mmap 的动态 EOF；段尾 BSS 是匿名零页，
  固定 prefix 的 partial last page 必须只复制有效字节，不能直接共享含后续文件数据的完整 PageCache 页。
- 后续影响：新增 file-backed VMA 类型必须明确是 dynamic EOF 还是 fixed prefix。跨 MemorySet 扫描不得
  在 File lock 下做，也不得在 MemorySet lock 下调用可能等待该 MemorySet 的 truncate/punch 路径。
  后续若引入硬件 dirty-bit 或异步分配，仍须保留“backing 成功后才开放 shared WRITE”的提交边界。

### LA64 `PROT_NONE` 叶子区分软件 present 与硬件 valid

- 状态：已实现并通过双架构聚焦回归
- 适用范围：LA64 用户 `mmap(PROT_NONE)`、`mprotect(PROT_NONE)`、fault、fork、munmap
- 最后验证：2026-08-14
- 证据：`os/src/arch/loongarch64/mm/page_table.rs`；LA64 `mmap05`/`mprotect04` 与 RV64
  `mmap05,mprotect05` release、4 GiB/2 hart 日志，详见 `current-status.md`
- 内容：普通 LA64 resident leaf 同时具有硬件 `V` 与 bit 7 software-present；用户 `PROT_NONE`
  leaf 保留 PPN、software-present 和 bit 10 `PROTNONE`，但清硬件 `V`。页表 API 的
  `PageTableEntry::is_valid()` 是软件 resident/present 谓词，而 refill 只认硬件 `V`。因此权限 fault
  不会被误判为 lazy 未分配，mprotect 可在原 PPN 上恢复权限，fork/munmap 也不会遗失 resident frame。
- 后续影响：新增叶子状态时必须明确“软件是否拥有映射”和“硬件能否访问”两个维度；不能用硬件
  `V=0` 单独判断 lazy hole，也不能把 `PROTNONE` 用到中间页表项。其他无读/无执行组合仍由 NR/NX
  表达，是否需要针对旧模拟器增加更强隔离必须由独立契约和测试决定。

### `mprotect` 在修改前完成参数、映射与 backing 权限校验

- 状态：当前参数/权限/映射缺口失败路径已双架构验证
- 适用范围：`sys_mprotect()`、`MemorySet::remap_area_with_overlap_range()`、file-backed VMA
- 最后验证：2026-08-15
- 证据：Linux/RV64/LA64 `mprotect_failure_probe`，日志
  `/tmp/respos-{rv,la}-mprotect-failure.log`
- 内容：未知 protection bit 与未对齐地址在进入 MemorySet 修改前返回 `EINVAL`；MemorySet 先遍历完整
  VPN range，任一 unmapped page 使调用以 `ENOMEM` 返回，再对所有相交 shared file VMA 预检查写权限，
  因此单页只读 backing 升级写权限返回 `EACCES` 且不会获得写权限。通过预检查后才切分 VMA、修改
  resident PTE 与发布新 `map_perm`。
- 后续影响：POSIX 允许非 `EINVAL` 失败已经改变部分页面，不能把当前较强的预校验行为扩张为所有
  `mprotect` 失败必须事务回滚的架构承诺。未来仍需单测 `privatize_one()` 内存分配中途失败、VMA 数量
  上限和并发 user-copy；涉及这类可失败提交点时，要么明确接受规范允许的部分结果，要么设计可回滚的
  prepare/commit，不得仅凭现有 unmapped-hole probe 宣称全面失败原子性。

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

### 用户 PID/TID 从 1 开始，0 只表示 ABI 选择值

- 状态：当前工作树已实现并通过双架构 session/task/signal 专项
- 适用范围：initproc、fork/clone、session/process group、signal、wait
- 最后验证：2026-08-14
- 证据：`os/src/task/tid.rs`、`os/src/syscall/process.rs`、`session_phase5_probe`；RV64/LA64 2-hart
  session、wait4、signal Phase 5 回归
- 内容：首个用户 task 是 PID/TID 1，初始 PGID/SID 同为 1。数值 0 不分配给用户 task，只在
  `wait/kill/setpgid/getsid/sched_*` 等 ABI 中表示“当前进程、当前进程组或特殊选择”。fork 创建的新
  process 继承父 PGID/SID，只有 `setsid()` 成功才以自身 PID 同时建立新 session 和 process group。
- 后续影响：新增 pid lookup 不得把 selector 0 当作 `TASK_MANAGER` 中的真实用户 task；PID 1 的
  reparent/init 角色与普通正 PID 身份必须同时保留。

### 进程身份独立于任一线程 TCB

- 状态：M2.1 核心路径已实现并通过双架构 2/8 hart 专项
- 适用范围：thread group、PID/session、parent/children/wait、leader exit、exec de-thread、process signal
- 最后验证：2026-08-15
- 证据：`os/src/task/{process,task}.rs`、`os/src/syscall/{process,signal,time}.rs`；
  `/tmp/respos-{rv,la}-process-identity-{race,smp8}.log`
- 内容：每个进程由 `Arc<ProcessState>` 稳定表示，成员 TCB 和父进程 children 表强持有；全局
  `ProcessTable[tgid]` 只保存 Weak 索引。`TaskManager[tid]` 只表示线程。leader 先退出不会销毁 TGID、
  parent/children 或 wait 身份；最后 member 才发布 Zombie。non-leader exec 在 sibling quiescence 后
  接管 TGID 的 TID 索引，kernel stack 不依赖数值 TID。
- 提交规则：exec 和 group exit 通过 process lifecycle CAS 取得单一 owner；wait copyout 成功后才 Reaped；
  用户映像的所有可失败准备发生在 exec teardown 前；等待 remote CPU acknowledgement 时不持有
  members/children 锁。
- 后续影响：按 PID 的新路径必须查 `ProcessTable`，按 TID 的接口才查 `TaskManager`。不得恢复 exited
  leader tombstone。共享 handler/resource owner 与兼容身份双写仍需继续迁移，不能从
  当前 M2.1 结果外推完整 signal/job-control 支持。

### controlling tty 状态属于 terminal，进程只保存关联关系

- 状态：M2.2 terminal 状态与孤儿组转换已实现并通过 Linux/RV64/LA64 专项
- 适用范围：stdio、`/dev/tty`、session/pgrp、termios、job-control signal、wait stopped/continued
- 最后验证：2026-08-15
- 证据：`os/src/fs/tty.rs`、`user/src/bin/job_control_phase5_probe.rs`、
  `scripts/job_control_phase5_probe_linux.c`；`/tmp/respos-{rv,la}-job-control-orphan-stress8.log`
- 内容：单一 console terminal 保存 controlling session、foreground PGID 与共享 termios；每个
  `ProcessState` 只保存本进程是否仍关联该 controlling tty。fork 继承关联，`setsid()` 脱离，只有
  terminal 对象提交前台组切换或 session release。stdio 和 `/dev/tty` 的读写在进入物理 console 前
  统一执行后台组检查，不能让每个 fd 或 `sys_ioctl` 各自维护状态。孤儿组判定扫描稳定 ProcessState；
  `setpgid/setsid/exit+reparent` 在关系提交前后检测非孤儿→孤儿转换，且只有存在 stopped member 才按
  `SIGHUP`、`SIGCONT` 顺序通知整个存活进程组。
- 后续影响：增加 PTY 时每个 terminal 实例必须独立持有相同状态机；line discipline、hangup I/O、
  forced steal 副作用及并发关系变更的统一锁域仍需扩展，不能塞回单个 ioctl 分支。

### 调度状态只有一个所有者提交

- 状态：已确认
- 适用范围：ready/block/wakeup/exit/context switch
- 最后验证：2026-08-01
- 证据：`os/src/task/scheduler.rs`、`os/src/task/processor.rs`、Git `3aa1fb5`
- 内容：scheduler 使用 RT、normal、idle 多级队列并维护 `task_index`/blocked 集合；状态先准备，
  再由调度路径提交。退出任务通过 `DEAD_TASKS` 延迟 drop，避免在自身内核栈上释放自身。
- 后续影响：不要从多个路径重复入队或重复唤醒；close/signal/timeout 竞争必须保持 single-winner。

### CPU clock 以真实调度运行区间记账

- 状态：已实现并通过双架构单核/LTP 与 2-hart probe；user/system 拆分已闭合当前范围
- 适用范围：RV64/LA64 scheduler、thread group、CPU clock、POSIX CPU timer、times/getrusage/wait
- 最后验证：2026-08-15
- 证据：`os/src/task/{processor,task}.rs`、`os/src/syscall/time.rs`、
  `user/src/bin/task_a_clock_probe.rs`；`/tmp/respos-{rv,la}-cpu-clock-{cluster,probe-smp2}.log`
- 内容：idle scheduler 在切入 task 前开启 thread/process 运行区间，task 交回 idle 后关闭区间；idle
  栈保留该 task 的 Arc，因此即使 task 已从 manager 移除也能完成最后一次记账。thread clock 每任务
  独立；`CLONE_THREAD` 共享 process clock，后者以固定 per-hart slot 表示同时运行的线程并在读取时
  加上所有 live interval。锁为关本地中断的 spin lock，避免 timer trap 在同 CPU 重入。
- 生命周期：fork/new process 从零创建两类 clock，线程 clone 只共享 process clock，exec 保留累计
  时间。POSIX CPU timer 只强持有 detached clock state；thread clock 在创建线程退出后冻结，process
  clock 则由线程组累计状态继续前进，不借 timer 保留 TCB、MemorySet 或 fd table。
- user/system accounting 复用 process clock 的 per-hart slot，并在 slot 中记录当前 mode。task 保存跨
  context switch 的 mode：新任务从 user 开始；user trap 入口切到 system，trap 返回前切回 user；若
  syscall 阻塞，scheduler end/begin 在 system mode 上关闭/恢复区间。exit 将两类 tick 分别冻结到
  ProcessState，wait 输出成功后才分别累计到 parent children usage。process/thread CPU clock 的 total
  仍是两类 interval 之和，不另建可能漂移的第三份时钟。thread clock 同样保存 user/system 两项，但只需
  一个 running interval；`RUSAGE_THREAD` 快照调用线程，`RUSAGE_SELF` 继续快照共享 process clock。
- 后续影响：CPU clock 的 begin/end 必须继续包围真实 `__switch`，不能移到 ready/block 状态变更处；
  SMP 实现不得退化为单一 `running_since`。若增加 CPU hotplug 或 hart 数量，必须同步审计 slot 上限与
  已运行区间。新增 user trap 返回路径必须配对 mode transition；不能只在 syscall 分支记 system，否则
  page fault、signal 和 timer trap 会被错误计入 user。rusage 非时间字段使用下述独立资源快照，不能从
  CPU clock 推导或用全局 perf counter 冒充。

### getrusage 非时间字段随 stable process identity 聚合

- 状态：fault/RSS/context-switch/block-I/O 当前范围已实现并通过 Linux/RV64/LA64 专项
- 适用范围：`RUSAGE_THREAD/SELF/CHILDREN`、`wait4`、raw `waitid`、user page fault、scheduler handoff、
  VirtIO block read 与 disk-backed PageCache clean-to-dirty
- 最后验证：2026-08-16
- 内容：每个 TCB 保存 thread fault/context-switch counters，所有增量同时写入 stable `ProcessState` 的
  process counters，故已退出线程无需继续保留 TCB 也不会从 SELF 消失。Zombie 发布前冻结进程资源；
  wait copyout 成功后，parent children counters 对 fault/switch 求和、对 maxrss 取最大值。THREAD 的
  maxrss 与 Linux 一样使用进程 mm 高水位，而不是虚构 thread-private RSS。
- 缺页边界：`MemorySet::handle_page_fault` 返回 `Minor/Major/Retry`；只有 architecture user-trap 消费分类并
  计数，`ensure_user_page_access` 只完成内核代用户访问所需映射。RSS 统计 user VMA 的 resident frames，
  fault/getrusage/exit 更新 process high-water；unmap 只降低 current resident，不降低 high-water。
- I/O 边界：实际成功提交的当前任务 block read 按设备 512-byte block 记 `inblock`；disk-backed cache page
  首次从 clean 变 dirty 时记 `oublock`，而不是在 write syscall 或最终 writeback 时重复记账。boot/background
  I/O 没有可归属 task 时不污染用户进程。`POSIX_FADV_DONTNEED` 的当前实现仅驱逐 clean、未映射且无外部
  owner 的 cache page，使下一次 file fault 能可靠走 lower fill；dirty/mapped page 继续保留。
- switch 边界：RespOS 为安全发布 outgoing saved context 必须先切到 per-CPU idle stack，但该内部跳板不是
  Linux task switch。Processor handoff 保存 voluntary/involuntary 原因；idle loop 发布 outgoing、选择 next
  后，仅在 next 与 outgoing 不是同一 `Arc<TaskControlBlock>` 时提交 counter。next 为空表示切向 idle 或
  outgoing 已被另一 hart claim，仍是真实 switch；exit handoff 使用 `None`，不在最终 snapshot 后补账。
- 后续影响：新增 fault handler 成功分支必须明确分类；wait/reap 新入口必须沿用“copyout 完成后才累计”的
  提交边界。新增块设备、page-cache dirty 入口必须沿用同一 task/process snapshot，不能改用全局设备/
  perf counter；全部 Linux-zero legacy rusage 字段保持 0，除非目标 ABI 契约明确改变。

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

### SIGCHLD disposition 在 child 退出发布点决定 Zombie 或 Reaped

- 状态：`SA_NOCLDWAIT`/显式 `SIG_IGN` 首轮已实现并通过三方专项
- 适用范围：SIGCHLD action、child exit、ProcessTable、wait4/waitid
- 最后验证：2026-08-15
- 证据：`os/src/signal/sig_handler.rs`、`os/src/task/{process,task}.rs`、
  `os/src/syscall/process.rs`；`signal_phase5_probe` 与双架构 wait4 回归
- 内容：默认忽略和显式 `SIG_IGN` 保持不同 action 表示；默认 SIGCHLD 保留 Zombie，显式 ignore
  自动回收且不投递，`SA_NOCLDWAIT` 自动回收但仍向已安装 handler 投递。自动回收和成功 wait 都删除
  parent children 强引用及 ProcessTable 索引。signal 唤醒的 wait 必须先重扫 child state，再决定
  `ECHILD`、child status 或 `EINTR`。
- 后续影响：不能重新把默认 ignored signal 初始化为 `SIG_IGN`。迁移共享 handler 到 ProcessState 时，
  child 退出的 disposition snapshot 与 children removal 仍须保持同一语义提交顺序。

### signal pending 的 bitmap 只负责选择，info 必须按信号类别排队

- 状态：标准/实时队列核心与进程内配额已通过三方专项
- 适用范围：thread/process pending、`rt_sigtimedwait`、`tkill/tgkill/rt_sigqueueinfo`、线程退出
- 最后验证：2026-08-15
- 证据：`os/src/signal/sig_struct.rs`、`os/src/task/{process,task}.rs`、`signal_phase5_probe`；
  `/tmp/respos-{rv,la}-signal-rtqueue-{quota,tgkill02}.log`
- 内容：pending bitmap 只表达“该信号号至少有一个实例”并维持最低编号优先级；每号 info queue 的
  front 才是下一次递送对象。标准信号队列长度最多一且不覆盖首条 info，实时信号 FIFO 保存全部实例；
  pop 最后一个实例时才清 bitmap。ProcessState 的原子额度覆盖同进程 thread/process 两级队列，消费和
  thread teardown 都必须归还。
- 后续影响：任何直接清空 `SigPending` 的新路径都必须同步释放实时额度；引入 real-UID credential owner
  后，应把配额计数上移并覆盖同 UID 多进程，但不得破坏 ProcessState 下两级队列的原子 reserve/release。

### 显式 restart-class syscall 在 signal frame 中保存重执行上下文

- 状态：`wait4` 与已覆盖的零进展阻塞 I/O 已实现并通过双架构专项
- 适用范围：RV64/LA64 syscall trap、`wait4/waitpid`、`read/write/readv/writev`、
  `accept/accept4/connect(no SO_SNDTIMEO)/sendto/recvfrom/sendmsg/recvmsg/sendmmsg/recvmmsg`、signal frame、
  `FUTEX_WAIT/FUTEX_WAIT_BITSET(null timeout)`、`sigreturn`
- 最后验证：2026-08-15
- 证据：`os/src/arch/{rv64,loongarch64}/trap/mod.rs`、`os/src/signal/mod.rs`、
  `user/src/bin/task_a_wait4_probe.rs`；`/tmp/respos-{rv,la}-wait4-restart-probe.log`
- 内容：显式表中的 syscall 因可投递信号返回 `EINTR` 时，trap 层暂存 syscall 原始 arg0。只有实际取出的用户
  handler 带 `SA_RESTART`，signal frame 才保存回退 4 字节的 syscall PC 和原始 arg0；handler 返回后
  `sigreturn` 恢复该上下文并重新执行 syscall。不带标志的 handler 继续观察到 `EINTR`，默认忽略信号
  不会触发该路径。当前显式表为
  `wait4/read/write/readv/writev/accept/accept4/connect/sendto/recvfrom/sendmsg/recvmsg/sendmmsg/recvmmsg`，避免
  未经验证的 syscall 被全局自动重启；向量 I/O、`MSG_WAITALL` 或 mmsg 已有进展时仍优先返回字节数/
  message count。分类器同时读取 syscall 参数：futex 只有 null-timeout 路径属于当前证据范围；
  `recvmmsg` 的 null/non-null timeout 都可重启，但仍由 socket timeout 状态进一步约束。所有 socket I/O
  还查询 fd 对应 Socket：
  accept/recv/read/readv 仅在无 `SO_RCVTIMEO` 时可重启，connect/send/write/writev 仅在无
  `SO_SNDTIMEO` 时可重启；非 socket 的 read/write 仍走既有 restart class。trap 在执行 syscall 前快照
  该分类，使共享 fd 的并发 `setsockopt` 不会把返回后的状态误套到已经按旧 deadline 开始的操作上。
- 后续影响：不要在 `sys_wait4()` 内直接吞信号并继续阻塞，否则 handler 无法执行。扩展到其余
  timeout-bearing syscall 前必须先确认 Linux 的 restart class、partial side effect 与剩余时间语义；
  不能把所有 `EINTR` 一律重启。

### recvmmsg timeout 是 message 后检查/写回，不是独立 wake deadline

- 状态：原生 64-bit timeout、restart、partial count 与 MSG_WAITFORONE 已完成三方专项
- 适用范围：`recvmmsg`、signal restart classifier、AF_UNIX stream 当前 probe、timeout user pointer
- 最后验证：2026-08-15
- 证据：`os/src/syscall/{mod,net}.rs`、`scripts/socket_phase5_probe_linux.c`、
  `user/src/bin/socket_phase5_probe.rs`；`/tmp/respos-{rv,la}-socket-recvmmsg-timeout-stress8.log`
- 内容：syscall 入场读取并验证 relative timespec，计算只用于 message 后写回的 deadline，不注册 task
  timer。每条成功 message 写 `max(deadline-now, 0)`；为零时返回当前 count。零进展 `EINTR` 不改 timeout，
  使 `SA_RESTART` 从原用户值重新执行；已有 message 后错误返回 count，timeout 保留最近成功 message 的
  写回。`MSG_WAITFORONE` 在首条成功后只给后续 receive 添加 `MSG_DONTWAIT`。
- 后续影响：不要把它统一成 poll/nanosleep 的 timeout waiter，否则会偏离 Linux“无消息时超期不自行
  唤醒”的历史 ABI。time64/compat 或 user-copy fault 扩展必须独立取 oracle，不能从原生指针布局外推。

### signal interruption 标志只是 wake hint，发布后必须重新验证 pending 条件

- 状态：已实现并通过双架构 signal/socket/wait4/task 专项
- 适用范围：thread/process signal enqueue、interruptible blocking syscall、SMP signal delivery
- 最后验证：2026-08-15
- 证据：`os/src/task/task.rs::mark_signal_interrupted()`；
  `/tmp/respos-{rv,la}-signal-wait-timeout-nonrestart-stress8.log`（RV64 最终等价日志名为
  `/tmp/respos-rv-signal-scheduler-gdb.log`）
- 内容：pending queue 是“是否仍有 signal 可递送”的权威状态，`interrupted` 只用于跨核唤醒。发送者在
  入队后选择目标并发布 hint；等待者可能在这两个动作之间已观察 pending、退出 syscall 并消费 signal。
  因此发送者写 hint 后必须再次检查目标仍 interruptible 且仍有未屏蔽、非 ignored signal，否则撤销
  hint，避免下一次阻塞调用被已经消费的 signal 伪造 `EINTR`。
- 后续影响：新增 signal wake 路径不得把 `interrupted=true` 当作不可撤销事件，也不能仅用该 bool 代替
  pending queue。所有“先发布权威状态、后写 wake hint”的路径都要审计 consumer-wins 竞态。

### 带 timeout 的 poll/sleep 等待保持非重启，remaining time 按接口单独处理

- 状态：`nanosleep/ppoll/pselect6/epoll_pwait/clock_nanosleep` 已完成 Linux/RV64/LA64 三方专项
- 适用范围：signal trap restart classifier、relative sleep、poll/select/epoll timeout
- 最后验证：2026-08-15
- 证据：`scripts/signal_phase5_probe_linux.c`、`user/src/bin/signal_phase5_probe.rs`；上述 signal 8 轮日志
- 内容：上述调用在普通 handler 和 `SA_RESTART` handler 下均返回 `EINTR`，默认忽略 signal 不打断并
  等到 timeout；它们不进入显式 restart-class 表。`nanosleep` 的 relative remainder 单独 copyout 并由
  probe 校验，不能通过回退 PC 从原始完整 timeout 重新开始；relative `clock_nanosleep` 同样写 remainder，
  `TIMER_ABSTIME` 形式不修改 remainder。epoll probe 使用已注册但未就绪的 pipe fd，
  保证验证真实 waiter/block 路径而非空 interest 集合的调度让步行为。
- 后续影响：`recvmmsg` 非空 timeout 等必须分别固定 Linux 的 remainder/absolute-deadline/partial-result
  契约，不能从本组调用类推。

### stop state 必须先于 parent wait event 发布，handoff 不得覆盖并发 continue

- 状态：已实现并通过双架构 signal 8 轮与 job-control 专项
- 适用范围：默认 Stop signal、SIGCONT、WUNTRACED、SMP task handoff
- 最后验证：2026-08-15
- 证据：`os/src/signal/mod.rs`、`os/src/task/scheduler.rs`；
  `/tmp/respos-{rv,la}-signal-clock-nanosleep-stress8.log`、
  `/tmp/respos-{rv,la}-job-control-after-stop-publish-fix.log`
- 内容：Stop 递送先把 task 状态发布为 `Stopped`，随后写 wait event 并通知父进程，最后仅保存/handoff
  当前 context。父进程一旦观察到 stop event，立即发送的 SIGCONT 必须看到 `Stopped` 并把 task 入 ready
  queue；handoff 可能发生在该并发 wake 之后，因此不能再次无条件写 `Stopped` 覆盖 `Ready`。
- 后续影响：任何“状态变化 + 对外事件 + context handoff”协议都必须按可观察顺序提交。不得用 probe
  打印或额外 yield 让 stop 先完成，因为这会隐藏真实 shell `kill(SIGCONT)` 可触发的窗口。

### wait 的 child-event generation 闭合扫描到登记之间的丢唤醒

- 状态：已实现并通过双架构 8 轮 signal 压力
- 适用范围：wait4/waitid、child stop/continue/exit、SA_NOCLDSTOP
- 最后验证：2026-08-15
- 证据：`os/src/task/{process,task}.rs`、`os/src/syscall/process.rs`；
  `/tmp/respos-{rv,la}-signal-read-restart-stress8.log`
- 内容：父进程 ProcessState 持有单调 child-event generation；child 在唤醒 waiter 前先发布新代际。
  wait 在扫描 child 状态前保存代际，随后登记 waiter 并发布 Blocked；若此时代际已变化，立即撤销
  Blocked 并重扫。代际只用于检测“有事件发生”，具体 pid/options/status 仍由下一轮权威 children 扫描决定。
- 后续影响：不得用 `exited_child_ids` 代替代际，因为 stop/continue 不进入 exit 集合；也不能在代际变化
  时直接伪造 wait result，否则会绕过 pid/pgrp 与 WUNTRACED/WCONTINUED 过滤。登记后的复查同样不得以
  “存在任意旧 exited child”强制重扫；pid-specific wait 只由新的代际变化撤销睡眠。

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
- 最后验证：2026-08-14
- 证据：`os/src/task/futex/wait.rs::{futex_wait_common,futex_wait_timed_common,futex_requeue_common}`、
  `os/src/mm/mod.rs::read_user_u32_nofault`、`os/src/mm/memory_set.rs::shared_futex_key()`、Git
  `3aa1fb5`；RV64/LA64 `sysv_shm_futex_probe` 与 `futex_wait01,futex_wake03`
- 内容：`FUTEX_CMP_REQUEUE` 已在队列锁外预先确认用户页可读，锁内只做固定 4 字节 no-fault
  PTE 读取，使比较与 waiter 迁移处于同一临界区。普通和定时 `FUTEX_WAIT` 当前仍在持有
  `FUTEX_QUEUES` 时调用通用 `copy_from_user` 复核用户值，lazy/COW 页可能进入补页路径。
  wait completion 已区分 Pending/Woken/TimedOut/Interrupted，并保持单赢家。共享 futex key 对
  shared file 使用 backing 身份；System V shm 使用 resident shared frame 的 PPN 与页内 offset，因此
  同一 segment 的不同 attach 地址会聚合到同一队列。每次 `shmat` 独立分配的 attach id 只负责
  `shmdt` 映射分组，不再参与同步身份。
- 后续影响：普通 wait 应复用“锁外预触页、锁内 no-fault 复核”的模式；在完成前不能宣称
  futex queue lock 内已全面禁止通用用户拷贝或潜在 frame 分配。System V shm 后续仍需验证
  并发回收与 frame 复用压力，不能把跨 attach wait/wake 通过解释为生命周期全部闭合。

### SysV SHM attach/detach 由 table reservation 与 MM commit 线性化

- 状态：当前工作树已实现并通过双架构 lifecycle、nattch、双 attacher attach-race、顺序回收循环、
  `SHMMIN/SHMMAX` size 矩阵、默认 `SHMMNI` 耗尽、clean-table `SHMALL` 页额度、已有对象时动态下调
  `SHMALL/SHMMNI`、固定配额下双创建者线性化、核心 metadata、基础权限与 lock flag 专项
- 适用范围：`shmat` 发布/失败、显式 `shmdt`、成功 exec、group exit、fork/`CLONE_VM` 继承、
  `IPC_RMID`
- 最后验证：2026-08-15
- 证据：`MemorySet::{shm_attach_ids,remove_shm_attachment}`、
  `ipc::release_shm_attachments()`、`task::{install_exec_image,exit_process_group}`；Linux/RV64/LA64
  `sysv_shm_lifecycle_probe`、`sysv_shm_nattch_probe`、`sysv_shm_attach_race_probe`、
  `sysv_shm_metadata_probe`
- 内容：VMA/PTE/frame 归 `MemorySet`，segment key、删除标记和 owner 索引归 `SHM_TABLE`；销毁
  MemorySet 本身不能完成 SysV 生命周期。显式 detach 或旧 MM 已从 task handle 替换/完成 recycle 后，
  才向 table 提交 attach id。提交扫描 live MM，fork/CLONE_VM peer 仍持有同 id 时 segment 保持存活；
  `IPC_RMID` segment 只在全局 attachment 归零后释放。`shm_nattch` 按唯一 MM handle
  `Arc<RwLock<MemorySet>>` 扫描其中的 attach id：共享 MM 的多个线程只算一次，fork 复制出的独立 MM
  分别累计。`shmat` 在释放 table lock、进入 MM 写锁前先增加 segment 的 `pending_attaches`；VMA 安装
  成功或失败后再撤销 reservation，因此最后 detach/`IPC_RMID` 只有在 pending 与已安装 attachment
  同时归零时才能删除 segment。非空 `shmaddr` 不是 mmap hint：无 `SHM_REMAP` 时必须精确且不可覆盖，
  地址冲突返回 `EINVAL`。
- metadata 分层：`shm_segsz` 保留用户请求的原始 byte size，frame/map 与 `SHM_INFO.shm_tot` 使用向上
  取整后的页数；`IPC_RMID` 仅标记且仍有 attachment 时，`IPC_STAT` mode 暴露 `SHM_DEST`，segment 继续
  计入 `SHM_INFO`，直到最后 detach 才同时移除 id、metadata 和 frames。`SHM_STAT/SHM_STAT_ANY` 的
  index 是 table 中独立的 segment index，不等同于 shmid。
- 权限分层：`shmget` 只检查 `shmflg` 明确请求的 `0400/0200`；因此 mode `0000` segment 的
  `shmget(key, 0, 0)` 仍可返回 id，带 read 请求才为 `EACCES`。普通 `IPC_STAT/SHM_STAT` 与 `shmat`
  按 owner/group/other mode 检查，`SHM_STAT_ANY` 显式绕过 read mode；`IPC_SET/IPC_RMID` 只允许 root、
  owner 或 creator。当前 flat credential 模型的非 owner UID/GID 65534 路径已双架构验证。`SHM_LOCK`
  只设置 segment `locked` metadata，`IPC_STAT` 以 `SHM_LOCKED` 回报，unlock 清零；由于 frames 创建时
  已常驻且系统无 swap，这不等同于 Linux 的按用户 `RLIMIT_MEMLOCK`/page-pinning accounting。
- 后续影响：不得在旧 MM 仍可被 task 访问时先删 table/frame，也不得只依赖 `Drop<MemorySet>` 隐式
  猜测 IPC owner。任何跨 table/MM 的新 attach 路径都必须在可删除性检查之前登记 reservation，并在
  所有成功/失败出口撤销；新增 `CLONE_VM` 形式不得退回按 TCB 数量累计 attachment。当前 probe 已覆盖
  两个同时在途 attacher、128 轮顺序单页回收循环、`SHMMIN/SHMMAX` 与 existing-key size/flag errno
  优先级、默认 `SHMMNI=4096` 的顺序耗尽/槽位归还，以及 clean-table 下调 `SHMALL=2` 后的页计数/
  回收、已有对象时把 `SHMALL/SHMMNI` 降到当前用量以下的保留/阻塞/阈值恢复，以及核心单进程
  metadata 状态转换；固定配额 `SHMALL=1`/`SHMMNI=1` 下两个并发创建者由全局 table lock 线性化为
  一成一败。更宽 N 路并发、`SHM_REMAP` 并发覆盖、并发 sysctl/create、IPC namespace、物理内存、
  单调 segment/attach ID 溢出、namespace capability、lock 的
  `RLIMIT_MEMLOCK`/真实 pinning accounting 与绝对 realtime timestamp 仍需独立验证。

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

### ext4 特殊节点的类型与 device payload 由 lower inode 持久化

- 状态：已实现并由双架构 LTP 验证
- 适用范围：`mknod`/`mknodat`、`mkfifo`、特殊节点 stat/xattr；命名 FIFO open/read/write/lseek/fsync
- 最后验证：2026-08-14
- 证据：`os/src/fs/{namei.rs,ext4/inode.rs,pipe.rs}`、`os/src/syscall/fs.rs`；`mknod_xattr_probe`；
  RV64/LA64 musl/glibc 的 13-case mknod/xattr 簇、4-case statx 回归及既有五项 FIFO LTP
- 内容：`sys_mknodat` 将 device payload 经 namei `create_special()` 一直传给 lower inode；ext4 对
  FIFO、character、block、socket 都用 `ext4_mknod()` 建立真实特殊 inode，并从 raw inode device slot
  恢复 `KStat.rdev`；statx 按 Linux device 布局拆分 12-bit major 与 20-bit minor，不退化到 legacy
  8-bit minor。`sys_openat` 根据持久的 FIFO 类型将 pathname inode 转为 `NamedFifoEnd`；同一路径的
  运行态 reader/writer 共享 VFS `PipeRingBuffer`，关闭最后一个端点后释放，不把管道内容写入 ext4。
- 后续影响：不能用普通文件占位再只改低 12 位 permission 假装特殊 inode；否则 reopen/readdir/stat
  会继续识别成 regular，并绕过类型相关 errno、xattr 限制与 FIFO 阻塞语义。特殊 inode 正确不等于
  已有对应设备驱动；超出 Linux kernel 32-bit device encoding 的 libc-only device number 不在本范围。

### ext4 inode 属性与扩展时间戳以底层 transaction 成功为发布点

- 状态：已实现 mode/owner/times 组合提交及 ext4 纳秒/epoch 编解码；`UTIME_NOW/UTIME_OMIT` 状态机及
  当前 flat credential 范围的非 owner 权限矩阵已双架构验证
- 适用范围：chmod/chown/utimens、目录属性、hardlink alias、unlink 后打开 fd
- 最后验证：2026-08-15
- 证据：`os/src/fs/{ext4/inode.rs,file.rs}`、`vendor/lwext4_rust/c/lwext4/src/ext4.c`；
  `fs_metadata_probe` Linux/RV64 对照；`utimens_special_probe` RV64/LA64 权限门禁；
  `ext4_timestamp_phase5_probe` 双架构跨冷启动检查
- 内容：`ext4_setattr_ino` 在一个 inode ref/transaction 内更新选定字段；Rust mode/owner/times cache
  仅在 lower commit 成功后发布。fd 操作直接使用后端 inode number，不再依赖可见路径或隐藏 orphan
  名字；同 inode hardlink 共享属性缓存。`resolve_utimens_times()` 在任何字段发布前验证两个 `tv_nsec`，
  对 NOW 使用同一份当前时间，对 OMIT 生成 `None`；双 OMIT 在 path/fd lookup 前直接成功，因此既不
  改 atime/mtime/ctime，也保留 Linux 对不存在 pathname 成功的 no-op 扩展。其余请求按双 NOW/current
  time 与 arbitrary/mixed 分类：非 owner 前者要求 inode mode 写权限，否则 `EACCES`；后者返回
  `EPERM`。path、打开 fd 和 empty-path 最终均在 inode 修改前执行同一检查。
- ext4 lower ABI 保留 signed 64-bit seconds 与 nsec，在同一事务内把低 32-bit 秒和
  `nsec << 2 | epoch` 写入 classic/extra 字段；读取以 signed low word 加 2-bit epoch 恢复。当前精确
  可表示范围为 `[-2147483648, 15032385535]`；按 Linux VFS 规则，超范围值截断到端点且端点 nsec
  归零，无 extra field 时截断为 signed 32-bit 秒级。新 inode 的三项初始时间也先按 realtime 落盘再
  发布缓存；缓存发布按每个 inode 的 `extra_isize` 分别选择 atime/mtime/ctime 精度，因此 128-byte
  旧 inode 在立即 stat 与冷启动重读时均为 signed 32-bit 秒级，不会暂存无法落盘的扩展值。
- 后续影响：新增 setattr 字段必须加入同一 prepare/commit/publish 协议；不得把 `ENOENT` 转成成功后只改
  Rust override。双 OMIT 是由契约明确允许的唯一 pathname-free no-op，不能推广到其他时间向量。
  `CAP_FOWNER`、ACL、user namespace 和特殊 inode flags 不在当前已闭合范围内。

### atime policy 与 lazytime durability 分层

- 状态：regular/directory policy、24-hour relatime、lazytime 显式同步、background/eviction 与 crash-image
  已双架构验证
- 适用范围：read/readdir、relatime/strictatime/noatime/nodiratime、`MS_LAZYTIME`、fsync/sync/unmount
- 最后验证：2026-08-16
- 证据：`os/src/fs/{file.rs,mount.rs,dentry_cache.rs,ext4/{inode,super_block}.rs}`、
  `os/src/task/processor.rs`、`user/src/bin/{atime_phase5_probe,lazytime_persist_probe}.rs`；
  `/tmp/respos-{rv,la}-atime-dirtytime-eviction-phase5{,-perf}.log` 与
  `/tmp/respos-{rv,la}-lazytime-crash-{prepare,verify}.log`
- 内容：VFS 先按 readonly、mount/open noatime、directory nodiratime、strictatime，以及
  `atime <= mtime || atime <= ctime || now-atime >= 24h` 决定是否生成自动访问事件。普通 ext4 文件的
  可见时间只属于共享 inode metadata cache，不属于 open-file description；tmpfile 才使用 File override。
  非 lazytime 事件立即调用只改 atime、不改 ctime 的 lower setattr；lazytime 事件更新 inode cache 和
  pending generation，并把 inode 强引用登记到 filesystem registry。
- durability：文件 `fsync/fdatasync` 通过该 inode 的 metadata flush 提交自身；mount-wide
  `sync/syncfs/unmount/reboot` 先提交 dirty data，再按 filesystem 清空 lazy registry，最后执行 lower
  cache barrier。remount 关闭 `MS_LAZYTIME` 前走同一 filesystem sync。generation 检查保证并发新事件
  不会被旧 flush 错清。
- background/eviction：pending 保存首次 dirty 的 monotonic 时间；默认 43,200 秒阈值可由
  `/proc/sys/vm/dirtytime_expire_seconds` 调整，0 禁止后台到期。timer-service idle 每批最多 flush 8 个；
  dentry eviction 只在最后真实 owner 消失时提交，并在进入 lower I/O 前释放 cache/dentry 锁。失败保持强
  引用并重试，成功按 generation 清理 registry。
- 后续影响：不能把普通 inode timestamp 缓存在单个 fd 上，否则 pathname setattr 或其他 fd 更新后会
  产生陈旧 relatime 判断。dirtytime aging 必须使用 monotonic clock，不能受 system realtime 跳变影响；
  真实断电、volatile device cache、I/O failure injection 和超大 registry soak 仍是独立边界。

### Realtime 由平台 RTC 与 monotonic offset 组合

- 状态：RV64/LA64 QEMU virt 已实现；clock 与 RTC set/reset 专项均已双架构验证
- 适用范围：`CLOCK_REALTIME/gettimeofday`、filesystem/IPC 当前时间、`RTC_RD_TIME/RTC_SET_TIME`
- 最后验证：2026-08-16
- 证据：`os/src/arch/{rv64,loongarch64}/timer.rs`、`os/src/syscall/{time,fs}.rs`；
  `/tmp/respos-{rv,la}-rtc-clock-phase5.log`、`/tmp/respos-{rv,la}-rtc-set-phase5-current.log`、
  `/tmp/respos-{rv,la}-rtc-reset-persist-phase5.log`
- 内容：goldfish RTC 直接提供 epoch ns；LS7A TOY 复位后默认关闭，必须先置 `EO|TOYEN` 再读取 packed
  calendar/year 并换算 Unix epoch。内核保存 `rtc_epoch - monotonic` offset，因此 realtime 可由
  `clock_settime` 调整而不改变 timeout/uptime/CPU clock 的 monotonic 基础。
- `RTC_RD_TIME/RTC_SET_TIME` 访问平台 RTC 本身，不复用 `REALTIME_OFFSET_US`。RV64 goldfish 写回 high word
  后写 low word；LA64 TOY 写回先落安全日期、再切 year、最后落目标 calendar。RTC 写回不改变 system
  realtime，system `clock_settime` 也不隐式写 RTC；只有启动初始化显式从 RTC 建立 offset。
- reboot restart 在同步 filesystem 后走 RV64 SBI cold reboot 或 LA64 ACPI GED reset；QEMU goldfish
  `tick_offset` 与 LS7A `offset_toy` 均跨设备 reset 保留，因此下一次启动能重新读取。新 QEMU 进程则按
  `-rtc base` 新建设备，不属于此持久化模型。
- 后续影响：普通 MMIO 映射集合与 RV64 virtio transport 枚举集合必须分离；把 RTC 塞入
  `VIRTIO_MMIO` 会使 block transport index 偏移并导致根盘不可用。当前已关闭 QEMU 同设备 reset 保持；
  跨新 QEMU 进程的电池后备、RTC 写回宿主和真实硬件校准不由当前设备模型提供。

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

### LoongArch ELF HWCAP 来自 CPUCFG 能力探测

- 状态：已实现，嵌套 QEMU TCG 初始化通过
- 适用范围：LoongArch exec auxv、glibc `getauxval()`、native QEMU/TCG
- 最后验证：2026-08-15；LA64 4 GiB/2 hart diagnostic guest
- 证据：`os/src/arch/loongarch64/mod.rs::elf_hwcap()`、
  `os/src/task/{aux.rs,task.rs}`；guest `LD_SHOW_AUXV=1 /bin/true` 与 5 秒 native QEMU TCG 烟测
- 内容：exec 读取 `CPUCFG(1)`，只在 `CPUCFG1.UAL` bit 20 存在时通过 `AT_HWCAP`
  暴露 `HWCAP_LOONGARCH_UAL` bit 2。这与 Linux LoongArch 对 UAL 的探测契约一致，不会在
  不支持非对齐访问的 CPU 上伪造能力。
- 后续影响：不得为绕过用户程序检查而无条件硬编码 UAL；增加 FPU/LSX 等其他 HWCAP 前
  必须同时核对 CPUCFG 与内核用户上下文保存能力。

### vfork 共享旧 MM，且父任务必须先登记 blocked 再发布子任务

- 状态：已实现，双架构 clone/vfork 与 RV64 command/minibuild 已验证
- 适用范围：RV64/LA64 SMP `CLONE_VFORK`、posix_spawn、Rust Command/cargo
- 最后验证：2026-08-14；RV64/LA64 release、4 GiB/2 hart
- 证据：`os/src/syscall/process.rs::sys_clone()`、`os/src/task/task.rs::{clone_,install_exec_image}`；
  `/tmp/respos-{rv,la}-clone05-vfork-mm-fix.log`、`/tmp/respos-rv-vfork-cagent-regression.log`
- 内容：带 `CLONE_VM` 的 vfork child 与 parent 共享旧 `MemorySet`，所以 child 在 exec/exit 前的用户
  内存写入对 parent 可见。每个 task 持有可替换的 MM handle；child exec 安装新 handle，不覆盖 parent
  仍持有的旧 MM。vfork parent 的 blocked registration 是 wakeup 协议的一部分，必须发生在 child 加入全局
  ready queue 之前。发布后 child 可在任意 CPU 立即 exec；exec/exit 的一次性 wake 此时必能从
  blocked 表取回 parent。父任务登记后再发布 child，随后直接切到 idle/下一任务。
- 后续影响：不得再次以“避免 exec 覆盖 parent”为由让 vfork 忽略 `CLONE_VM`；应维护 per-task handle
  替换边界。任何“发布对象后再登记 waiter”的一次性事件都要审计 lost-wakeup 窗口；单核测试因
  child 无法提前运行，不能覆盖该顺序错误。

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

### 文件 syscall 的 bounce buffer 由有界 per-hart `KernelIoBuffer` 管理

- `read/write/pread/pwrite`、`sendfile/copy_file_range`、`splice/tee` 仍通过最多 64 KiB
  kernel slice 与现有 `FileOp` 交互；本边界未引入 user VA 直接解引用或页固定。
- `mm::KernelIoBuffer` 在 `io_buffer_pool` 开启时每 hart 保留至多一个已初始化 `Vec`。
  本 hart `SpinNoIrqLock<Option<Vec<u8>>>` 只保护所有权移动，不得在其内执行 user-copy、
  fault、VFS/PageCache/lwext4 I/O 或调度。任务迁移可将 buffer 归还到新 hart；新槽已被占用时
  直接丢弃本 buffer，不等待另一个缓存使用者。
- 每个 buffer 容量超过 64 KiB 时不得进入 pool；当前 syscall chunk 本身也限制在 64 KiB。
  RV64/LA64 最大保留量因此为 512/768 KiB。`drain_io_buffers()` 先推进 drain epoch
  再释放空闲槽；所有在旧 epoch 借出的 buffer 归还时也必须直接释放，避免执行 drain
  的 `/proc` write syscall 在返回后把自身 buffer 重新放回 pool。
- 缓存字节不保证每次 checkout 为零；安全契约是所有字节始终处于 Rust 已初始化状态，
  producer 只允许下游消费其返回的 `n` 字节。任何新 `FileOp`/pipe producer 若返回超过实际
  写入的长度，都是会导致旧数据暴露的契约错误，不得依赖过去 `vec![0; len]` 掩盖。

### `splice` 在消费输入前按 FileOp 状态完成预检

- 状态：AF_UNIX 输入错误语义已实现并完成双架构 SMP 专项
- 适用范围：`sys_splice`、FileOp/pipe 方向、AF_UNIX/inet socket 输入
- 最后验证：2026-08-14
- 证据：`os/src/{syscall/fs.rs,fs/file.rs,net/socket.rs}`、`scripts/splice_socket_probe_linux.c`、
  `user/src/bin/splice_socket_probe.rs`；RV64/LA64 4 GiB/2 hart probe 与 musl/glibc `splice01`--`splice07`
- 内容：通用 splice 层先解析两端 fd，检查 O_PATH、至少一端为 pipe、splice capability、pipe offset、
  fd 读写方向和 output append；只有这些通用错误都排除后，才调用输入 FileOp 的
  `validate_splice_read()`，最后进入复制循环。AF_UNIX socket 在该预检中把未连接输入映射为 `EINVAL`；
  inet 保留传输层 `ENOTCONN`，connected AF_UNIX 继续真实读取和传输。预检只观察状态，不消费数据。
- 后续影响：新增 splice-capable 对象若有调用前状态约束，应在 FileOp 预检提交精确 errno，不能等到
  `read()` 已产生协议专用错误或副作用后再翻译；也不能把 AF_UNIX 规则泛化到 inet。当前 bounded-copy
  实现满足返回值/数据语义，不代表 Linux pipe-buffer zero-copy 所有权模型。

### pwrite 的 append 选位与数据写入仅在单个 chunk 内共用 open-file 锁

- 状态：Linux `O_APPEND` 选位语义已实现；整 syscall 并发原子性已有双架构 expected-fail 反证
- 适用范围：`pwrite/pwrite64`、普通 `File`、page cache/lower inode EOF、open-file offset
- 最后验证：2026-08-14
- 证据：`os/src/{syscall/fs.rs,fs/file.rs}`、`scripts/pwrite_append_probe_linux.c`、
  `scripts/pwrite_append_atomic_probe_linux.c` 与 guest `pwrite_append_atomic_probe`；RV64/LA64
  4 GiB/2 hart 日志
- 内容：`sys_pwrite64` 保留用户显式 offset 作为非 append 定位，并通过专用
  `FileOp::pwrite_at_offset()` 提交数据。普通 `File` 在持有共享 open-file inner lock 时同时
  读取 `O_APPEND`、从 page cache 或 lower inode 选择 EOF，并完成写入；该路径不更新
  `inner.offset`。普通 `write` 复用同一 locked writer 但在成功后推进 offset。
- 后续影响：mmap 写回、splice 显式 offset 等非 pwrite 路径必须继续调用普通
  `write_at_offset()`，不得被 open-file `O_APPEND` 改写。当前 syscall 为有界 kernel buffer
  分块 copy/write，两个 128 KiB writer 已证明会在 64 KiB chunk 边界交错。修复不得持有 spin lock
  进入通用 usercopy/fault；必须为整 syscall 建立可阻塞序列化或可回滚 range reservation，并覆盖不同
  open description、EFAULT/short-write 与 truncate 竞态。

### 目录脱离 namespace 后，getdents 不得继续返回 open-file 缓存

- 状态：ext4 已删除目录 fd 语义已实现并完成双架构 SMP 聚焦回归
- 适用范围：ext4 `rmdir`、deferred inode reclaim、`File::readdir_cached`、`getdents64`
- 最后验证：2026-08-14
- 证据：`os/src/fs/{ext4/inode.rs,file.rs}`、`scripts/getdents_unlinked_probe_linux.c`；
  RV64/LA64 4 GiB/2 hart 的 musl/glibc `getdents01/getdents02`
- 内容：ext4 成功删除最后 namespace link 后先以 release store 标记 inode `unlinked`，
  lower inode 的物理回收仍等到最后 VFS Arc 消失。目录 `File` 每次 `readdir_cached()` 都在
  返回旧 `dirent_cache` 或重建快照之前 acquire-load 该状态；已脱离 namespace 则返回
  `ENOENT`，而不是继续暴露 deferred lower inode 中的 `.`/`..`。
- 后续影响：目录项缓存是 open-file 遍历快照，不是 namespace 生存证明；任何新的
  unlinkable 目录后端都必须提供同等的 detached 状态。该检查不能改成按 pathname 重新
  lookup，否则 rename 或 open-after-unlink 会把 inode identity 退化为路径猜测。

### chroot 在路径与 search permission 验证后检查 privilege，最后提交 root

- 状态：已采用并完成双架构 SMP 聚焦回归
- 适用范围：`sys_chroot`、namei、目录 search permission、task root
- 最后验证：2026-08-14
- 证据：`os/src/syscall/fs.rs`、`scripts/chroot_permission_probe_linux.c`；RV64/LA64 4 GiB/2 hart
  的 musl/glibc `chroot01`--`chroot04`
- 内容：调用顺序固定为复制用户 pathname、namei lookup、目录类型与 search permission 检查、
  privilege 检查，最后才以 `task.set_root()` 提交。这样无效指针、路径解析、类型和访问错误不会被
  非特权 `EPERM` 遮蔽，任一步失败也不会改变 task root。
- 后续影响：新增 capability 或 namespace 模型时只能替换 privilege predicate，不能把它提前到
  pathname 观察之前；并发 namespace 可见性若另行实现，仍必须保持 root 在全部检查成功后单点提交。

## 设备与 DMA 模型

### VirtIO descriptor 的虚拟连续不等于物理连续

- 状态：已确认并修复 HAL 边界
- 适用范围：RV64 virtio-mmio、LA64 virtio-pci、块设备及后续复用同一 HAL 的设备
- 最后验证：2026-08-14
- 证据：`os/src/drivers/virtio/mod.rs`；RV64 页尾 `BlkReq` QEMU/GDB 抓取；
  `/tmp/respos-rv-virtio-bounce.log`
- 内容：virtio descriptor 只携带一个物理起点和长度，要求整个范围对设备物理连续；Rust slice 只保证
  虚拟连续。HAL 对 direct map、单页或逐页翻译后确认为相邻物理页的范围直接共享；跨非连续物理页时，
  在 direct-map kernel heap 建立连续 bounce。`DriverToDevice`/`Both` 在 share 时拷入，
  `DeviceToDriver`/`Both` 在 unshare 时拷回，active 记录保证设备完成前 allocation 存活；完成后的
  同尺寸 buffer 可复用，但空闲池同时受 64 项和 1 MiB 总预算约束。
- 后续影响：新增设备不能绕过 `Hal::share/unshare` 直接把任意栈、页缓存或用户 backing 的首 PA 加长度
  交给 DMA；若要消除 bounce，应由上层生成 scatter-gather descriptor，或使用有证明的连续 DMA
  allocation。对结构体做对齐只覆盖某个布局，不构成通用修复。

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

### socket 单次接收 flag 共享一个绝对 deadline 和短 I/O 规则

- 状态：已实现并完成双架构 SMP 专项
- 适用范围：`recvfrom/recvmsg/recvmmsg`、AF_UNIX/TCP/UDP、`MSG_PEEK/MSG_WAITALL`
- 最后验证：2026-08-14
- 证据：`os/src/{syscall/net.rs,net/socket.rs,net/tcp.rs,net/udp.rs}`、
  `scripts/socket_flags_probe_linux.c`、`user/src/bin/socket_flags_probe.rs`；RV64/LA64 4 GiB/2 hart 日志
- 内容：syscall 层只解析 flags 和用户缓冲区，socket 层在一次调用开始时生成绝对 recv deadline。
  流式 WAITALL 可以多次消费分片，但每次重试沿用同一 deadline；timeout、EOF 或 EINTR 前已经消费的
  字节以短读返回。PEEK 从协议/Unix 接收队列复制而不推进队列；UDP WAITALL 不跨数据报聚合。
- 后续影响：不得在 WAITALL 分片循环中按 SO_RCVTIMEO 重新生成 deadline，也不得在部分数据之后把
  timeout/EINTR 覆盖成错误。扩展 `MSG_TRUNC/OOB/ERRQUEUE` 时需要先建立 Linux 对照，不能复用流式
  WAITALL 假设到数据报控制面。

### TCP 异步 connect、pending error 与失败后重置由 socket 持有

- 状态：loopback success/refused/同 fd 重连已实现并完成双架构 SMP 专项
- 适用范围：nonblocking `connect`、poll/epoll write/error readiness、`SO_ERROR`
- 最后验证：2026-08-14
- 证据：`os/src/net/{tcp.rs,socket.rs}`、`os/src/syscall/net.rs`、
  `scripts/socket_connect_probe_linux.c`、`user/src/bin/socket_connect_probe.rs`
- 内容：首次非阻塞 connect 只发起状态转换并返回 `EINPROGRESS`。协议 poll 是
  `CONNECTING -> CONNECTED/FAILED` 的提交点；FAILED 同时发布正 errno 到 socket 的原子 pending-error
  槽，使 poll/epoll 可报告 write/error readiness。`SO_ERROR` 用 swap 读取并消费该槽，不能由 readiness
  扫描提前清除，也不能在 syscall 层固定伪造 0。失败被消费后，同一 fd 的首次重连以
  `ECONNABORTED` 作为旧 handle 复位提交点；替换 handle、清空端点并回到 CLOSED 后，下一次 connect
  重新走正常 `EINPROGRESS -> CONNECTED/FAILED` 状态机。
- 后续影响：新增 timeout/unreachable/reset 映射时必须在同一状态提交点写入精确 errno；poll 只能观察，
  `SO_ERROR` 才能消费。close/retry 替换 smoltcp handle 时必须保持唯一所有权；不得跳过
  `ECONNABORTED` 复位边界或让旧 pending error 污染新连接。

### TCP 发送 half-close 不关闭反向数据流，并独立发布 peer RDHUP

- 状态：loopback 基础状态机已完成双架构 SMP 专项
- 适用范围：TCP `shutdown(SHUT_WR)`、dup open-file、FIN/EOF、read readiness 与 RDHUP
- 最后验证：2026-08-15
- 证据：`os/src/net/tcp.rs`、`scripts/tcp_half_close_probe_linux.c`、
  `user/src/bin/tcp_half_close_probe.rs`；RV64/LA64 4 GiB/2 hart 日志
- 内容：`TcpSocket::shutdown_write()` 只发布共享 socket 的 send-shutdown 并令 smoltcp 发送 FIN，不把
  RespOS socket 状态改为 CLOSED，也不设置 recv-shutdown。因此同一 open-file 的 duplicate fd 后续发送
  返回 `EPIPE`，但接收半边继续消费对端数据。peer FIN 到达后，协议队列中的数据优先返回；队列清空且
  `may_recv()` 为 false 时 read readiness 成立并由 read 返回 0。`FileOp::poll_rdhup()` 则观察
  smoltcp peer-FIN states，即使接收缓冲仍有未读数据也为 true；ppoll/epoll 只在 interest 中包含
  `POLLRDHUP/EPOLLRDHUP` 时返回该位，不把它合并成无条件 HUP。
- 后续影响：close、`SHUT_RDWR` 与错误恢复不能把发送 half-close 提前升级为本地全关闭。当前证据不含
  `SHUT_RD` 丢弃、跨线程阻塞唤醒、reset/linger 或非 loopback 网络。

### UDP shutdown 由 socket 层半边标志同时驱动 I/O 与 readiness

- 状态：connected loopback 基础收发半边、poll/epoll readiness 及 blocked recv EOF 已完成双架构专项
- 适用范围：UDP `shutdown(SHUT_RD/SHUT_WR/SHUT_RDWR)`、空队列 EOF/来源地址、`EPIPE`、HUP/RDHUP
- 最后验证：2026-08-15
- 证据：`os/src/net/{udp.rs,socket.rs}`、`scripts/udp_shutdown_probe_linux.c`、
  `user/src/bin/udp_shutdown_probe.rs`；RV64/LA64 4 GiB/2 hart 日志
- 内容：UDP 只有 connect 建立默认 peer 后才允许 shutdown；未连接或仅 bind 时返回
  `ENOTCONN`。`recv_shutdown/send_shutdown` 存在共享 `UdpSocket` 上，因此 dup 观察同一状态。
  `SHUT_WR` 令发送返回 `EPIPE`；`SHUT_RD` 时协议队列仍优先返回已排队或后续数据报，
  只在队列为空时返回 0。shutdown 不调用 smoltcp `close()`；Drop 才负责 close 和 remove
  handle，避免把单边状态折叠成整个 socket 关闭。readiness 复用同一对标志：
  recv shutdown 产生 read-ready/RDHUP，send shutdown 产生 write-ready，两者同时成立才产生无条件
  HUP；这与 Linux `datagram_poll_queue()` 的 shutdown mask 观察点一致。接收结果用
  `Option<SocketAddr>` 区分带来源的数据报与不带来源的 shutdown EOF：零长数据报仍是
  `Some(source)`，只有空队列 EOF 是 None，使 `recvfrom` 能只清零 addrlen 而不伪造地址。
- 后续影响：不能套用 stream `SHUT_RD` 的“丢弃当前与未来数据”假设。本地 shutdown
  标志已接入 UDP poll/epoll level readiness，阻塞 recv 也会在 RD shutdown 后返回 EOF。
  ET/ONESHOT、数据/timeout/signal/shutdown 竞争、并发阻塞 send shutdown 竞争、
  disconnect/reconnect、error queue 和非 loopback 网络需另立 Linux 对照。

### AF_UNIX RDHUP 复用既有 peer shutdown 状态与 waiter

- 状态：stream socketpair level/edge/oneshot 及 RDHUP-only blocking poll/epoll 已完成双架构 SMP 专项
- 适用范围：AF_UNIX stream `SHUT_WR`、buffered data、`POLLRDHUP/EPOLLRDHUP`
- 最后验证：2026-08-15
- 证据：`os/src/net/socket.rs`、`scripts/socket_phase5_probe_linux.c`、
  `user/src/bin/socket_phase5_probe.rs`；RV64/LA64 4 GiB/2 hart 与 RV64 16 GiB/8 hart 日志
- 内容：建链时双方已互持 `peer_write_shutdown` 和 `peer_closed` 原子状态；SHUT_WR 发布前者，close 发布
  后者，并通过 peer receive buffer 的既有 waiter 集合唤醒。`UnixSocket::poll_rdhup()` 只读取这两个状态，
  因此 buffered data 提供 IN、peer half-close 同时提供 RDHUP，而只有 peer close 才继续提供 HUP。
  ppoll/epoll 注册即使只订阅 RDHUP，也复用内部 HUP-class waiter；通用 epoll `last_ready` 与 oneshot
  disabled 状态负责 edge 去重和 `EPOLL_CTL_MOD` rearm，不下沉到 Unix socket。
- 后续影响：不得为 RDHUP 再建一套 Unix lifecycle 或消费接收队列。edge/oneshot、只订阅 RDHUP 的阻塞
  waiter 已闭合；seqpacket/datagram 与 shutdown/close 竞态仍需独立验证。

### AF_UNIX 在建链提交点快照双方 raw 地址，查询时原子写回

- 状态：错误路径与 stream 的 unnamed/pathname/abstract 地址已完成双架构 SMP 专项
- 适用范围：`accept`、`getsockname`、`getpeername` 的 AF_UNIX stream 地址及输出参数
- 最后验证：2026-08-14
- 证据：`os/src/{net/socket.rs,syscall/net.rs}`、`scripts/getpeername_probe_linux.c`、
  `user/src/bin/getpeername_probe.rs`；RV64/LA64 4 GiB/2 hart probe 与 musl/glibc
  `getpeername01,getsockname01`
- 内容：syscall 先通过 fd table 解析并确认对象是 socket，再按地址族确认 remote endpoint 或 AF_UNIX
  peer 已连接；连接态成立后才由地址 writer 读取 `addrlen`、验证实际写入范围，并提交地址和最终长度。
  因而 `EBADF/ENOTSOCK` 和未连接 inet 的 `ENOTCONN` 保持 Linux 优先级；connected AF_UNIX
  socketpair 则以长度 `sizeof(sa_family_t)` 的未命名地址进入 writer，使非法长度/指针精确返回
  `EINVAL/EFAULT`，且校验失败不会产生部分写回。pathname/abstract connect 在同一提交点把 listener raw
  key 快照为 client peer/accepted local，把 connector raw key 快照为 accept 输出/accepted peer；raw
  `Vec<u8>` 保留 abstract 的非 UTF-8 字节。pathname writer 加结尾 NUL，abstract writer 保留精确长度；
  buffer 截断只限制 copy，最终 `addrlen` 始终发布完整长度。
- 后续影响：其他带 `sockaddr *` 输出的 syscall 不能简单复制连接态优先的查询顺序；应先用 Linux
  probe 固定各接口和连接态组合的错误优先级。失败 connect 必须连同 peer address、credentials、buffer/
  close/shutdown 引用一起回滚；查询路径不得依赖 listener 仍在 registry，也不得把 abstract 名称转成文本。

### AF_UNIX peer credentials 在建链提交点快照

- 状态：`SO_PEERCRED` 已实现并完成双架构 SMP 专项
- 适用范围：AF_UNIX socketpair、pathname listen/connect/accept、`getsockopt(SO_PEERCRED)`
- 最后验证：2026-08-14
- 证据：`os/src/net/socket.rs`、`os/src/syscall/net.rs`、
  `scripts/socket_peercred_probe_linux.c`、`user/src/bin/socket_peercred_probe.rs`；RV64/LA64 4 GiB/2 hart
  probe 与 musl/glibc `getsockopt02`
- 内容：socketpair 创建时把当前 TGID/real UID/GID 快照到两端；listen 时把 server 凭据放入 listener，
  connect 提交时把 listener 快照装到 client、把 connector 当前凭据装到待 accept 的 server endpoint。
  accept 只转移已带凭据的 socket，`SO_PEERCRED` 只复制固定大小的 `struct ucred` 快照，不查找 live
  task。因此对端退出、调度迁移或查询者变化不会改变已建立连接的 peer identity。
- 后续影响：AF_UNIX 建链失败必须和 buffer/close/shutdown peer 状态一起回滚 credentials，不能留下半
  发布身份；dup/fork 共享同一 socket 快照。实现 `SCM_CREDENTIALS/SO_PASSCRED` 时不能复用此固定
  连接快照代替逐消息凭据，稳定 TGID 的最终所有权仍取决于 Phase 5 process-identity 重构。

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
- 最后验证：2026-08-15
- 证据：`arch/{rv64,loongarch64}/smp.rs`、`task/processor.rs`、LoongArch QEMU
  `-m 12G -smp 12` 串口 online mask、1/2/12 hart `socket_timeout_probe` 与 BuildStorm 并行编译
- 内容：RV64 通过 SBI HSM/IPI，LoongArch QEMU-virt 通过 IOCSR mailbox/IPI 启动 secondary。
  每个 hart 使用独立 early/idle stack、processor/current-task 状态和本地 timer，ready queue 仍为
  全局串行调度器；enqueue 后会向满足 affinity 的 idle hart 发送 IPI。LoongArch 的 boot hart
  最多等待 1 秒收集 online mask，较小的 `-smp` 覆盖不会阻止启动。secondary 在发布 online 后等待
  `BOOT_RELEASED`；boot hart 完成收集并启用、首次编程全局 timer 后才释放它们进入调度，因而用户
  timeout 不会早于 timer-service 就绪。
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
