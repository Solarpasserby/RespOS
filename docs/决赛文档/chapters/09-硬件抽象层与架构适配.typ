= 9. 硬件抽象层与架构适配
<9-硬件抽象层与架构适配>
#quote(block: true)[
本章介绍 RespOS 在 RISC-V64 和 LoongArch64 上的架构适配，以及页表、trap、任务切换、时钟和启动流程的统一边界。
]

RespOS 的双架构支持采用“公共内核逻辑 + 架构目录特化”的设计。任务管理、内存管理、文件系统和系统调用等模块使用统一的公共代码；启动入口、寄存器访问、页表操作、异常返回、上下文切换和时钟触发等必须依赖硬件的部分，则分别位于 `os/src/arch/rv64/` 和 `os/src/arch/loongarch64/`。

当前架构层不是一套完全由 trait 组成的独立 HAL 库，而是由条件编译、模块重导出、同名函数和相同的数据结构约定共同形成的适配边界。`os/src/arch/mod.rs` 根据 `target_arch` 选择具体实现并重新导出，使公共代码可以调用 `read_mmu_token`、`write_mmu_token`、`sfence`、`set_next_ti_trigger` 等统一入口，而不直接散落 CSR、汇编指令和硬件寄存器假设。

== 9.1 架构适配的整体结构
<91-架构适配的整体结构>
=== 9.1.1 一套公共调用链，两套底层实现
<911-一套公共调用链两套底层实现>
RespOS 的架构目录按照启动、内存、trap、task、timer 和 interrupt 等职责组织。两套目录提供相近的模块接口，但在内部使用各自架构的寄存器和汇编实现：

```text
公共内核模块
  ├─ MemorySet / PageTable 使用页表操作入口
  ├─ task / scheduler 使用任务上下文切换入口
  ├─ syscall / signal 使用 TrapContext 访问入口
  ├─ timer / scheduler 使用统一时间接口
  └─ main 使用统一启动阶段
              │
              ▼
      os/src/arch/mod.rs
       cfg(target_arch)
          ├───────────────┐
          ▼               ▼
   arch/rv64/       arch/loongarch64/
   satp / stvec     PGD / ERA / EEntry
   sret / SBI       TLB refill / ertn
```

#strong[图 9-1 RespOS 双架构适配结构]

这种结构的关键不是让两个架构的代码完全相同，而是让公共模块只依赖稳定的语义接口。例如，公共内存管理只需要“写入当前页表根并刷新地址转换缓存”，不需要知道 RV64 使用 `satp`、`sfence.vma`，还是 LoongArch 使用 `PGDL/PGDH` 和 `invtlb`。架构层负责把同一语义翻译成对应指令序列。

=== 9.1.2 双架构差异总览
<912-双架构差异总览>
#figure(
align(center)[#table(
  columns: 4,
  align: (col, row) => (auto,auto,auto,auto,).at(col),
  inset: 6pt,
  [适配维度], [RISC-V64], [LoongArch64], [公共抽象],
  [内核入口],
  [OpenSBI 进入 S-mode，传入 hart id 和 FDT opaque],
  [QEMU/固件进入低地址直映入口],
  [`enter_main` → `rust_main_high`],
  [当前计数器],
  [`time` CSR/计数器],
  [`rdtime.d`],
  [`get_time`、频率转换和下一次触发],
  [地址转换根],
  [`satp`],
  [`PGDL/PGDH`],
  [`read_mmu_token`、`write_mmu_token`],
  [页表遍历],
  [硬件 Sv39 遍历],
  [TLB refill 汇编使用 `lddir`/`ldpte`],
  [`map/unmap/query/protect`],
  [用户 trap 状态],
  [`stvec`、`sstatus`、`sepc`、`scause`、`stval`],
  [`EEntry`、`PRMD`、`ERA`、`ESTAT`、`BADV`],
  [`TrapContext` 与公共 trap handler],
  [异常返回],
  [`sret`],
  [`ertn`],
  [restore 汇编后的用户态返回],
  [任务页表切换],
  [写 `satp` 后 `sfence.vma`],
  [同时写 `PGDL/PGDH` 后刷新 TLB],
  [`TaskContext` 保存地址空间 token],
  [外部设备形态],
  [QEMU virt 的 MMIO 设备],
  [QEMU LoongArch 的 PCI 设备],
  [drivers 层统一设备操作],
)]
)

#strong[表 9-1 RV64 与 LA64 的架构差异及统一接口]

== 9.2 地址空间与页表适配
<92-地址空间与页表适配>
=== 9.2.1 共享高半区内核模型
<921-共享高半区内核模型>
两个架构最终采用相同的高半区内核地址模型。公共内核代码通过统一的 `KERNEL_BASE` 访问内核映像、内核堆和内核设备映射；创建用户地址空间时，架构层提供 `PageTable::from_kernel()`，复制内核根页表中属于内核高半区的部分。

这样，`MemorySet` 可以在两种架构上复用相同的 VMA、页帧和映射逻辑。用户地址空间的差异主要集中在页表项编码、地址转换寄存器和 TLB 刷新，不会扩散到 COW、lazy allocation、file-backed mapping 等上层机制。

=== 9.2.2 RISC-V Sv39 页表
<922-risc-v-sv39-页表>
RISC-V 页表采用 Sv39 三级结构。`PageTable` 保存根页表物理页号和由 `FrameTracker` 管理的页表页帧；映射时按 VPN 的三级索引逐级创建中间页表，叶页表项保存物理页号和读写执行权限。`token()` 将根页表物理页号编码为 `satp`，任务切换时由汇编保存和恢复该 token。

RISC-V 还为内核 direct map 提供了大页映射入口。启动阶段的 early page table 先建立能够覆盖 QEMU 最大可达内存窗口的映射，之后由 FDT 识别实际 RAM 末址，并由正式 frame allocator 严格按实际内存上限分配。这样既能在早期读取固件提供的设备树，又不会把预留的最大窗口误认为真实物理内存。

=== 9.2.3 LoongArch 页表与 TLB refill
<923-loongarch-页表与-tlb-refill>
LoongArch 的页表同样采用三级、4 KiB 页粒度的结构，但硬件地址转换通过 TLB 完成。发生 TLB miss 时，`tlb_refill.S` 使用 `PGD` 根地址和 `lddir`、`ldpte` 指令读取多级页表项，装载偶数页和奇数页的 TLBLo 后执行 `tlbfill`；如果目录项无效，则构造无效 TLB 项并返回异常路径。

LoongArch 的 `read_mmu_token` 返回 `PGDL` 根地址，`write_mmu_token` 同时更新 `PGDL` 和 `PGDH`，并设置 ASID 和页表根同步状态。任务切换汇编也保存一个根 token，恢复时写入两个根页表寄存器并刷新 TLB。公共代码因此只看到“切换地址空间并使地址转换状态生效”，不需要处理两个根寄存器的细节。

=== 9.2.4 页表操作的统一语义
<924-页表操作的统一语义>
两套 `PageTable` 都提供创建、映射、解除映射、查询、修改权限和替换页表项等操作。公共 `MemorySet` 依靠这些操作实现：

```text
MemorySet
    │
    ├─ 创建 VMA 和物理页帧
    ├─ PageTable::map / unmap
    ├─ page fault 时 query / 建立叶映射
    ├─ COW 时 replace_pte / modify_pte
    └─ 地址空间切换时 token / sfence
```

#strong[图 9-2 公共内存管理与架构页表实现的关系]

页表页帧由页表对象持有并自动追踪。进程退出或地址空间替换时，页表页帧不会立即在任意执行上下文中复用，而是经过有限的 quarantine 队列延迟回收，避免硬件仍可能观察旧页表根时发生页表页帧复用。这一机制在两套架构中保持相同语义，只在页表根和刷新指令上有所不同。

== 9.3 Trap 与特权级切换
<93-trap-与特权级切换>
=== 9.3.1 统一的 trap 三阶段
<931-统一的-trap-三阶段>
两种架构的 trap 入口虽然使用不同的 CSR 和汇编语法，但都遵循相同的三阶段流程：

```text
entry：硬件保存原因和返回地址，汇编保存寄存器
        │
        ▼
handler：构造 TrapContext，进入公共 Rust trap 分发
        │
        ▼
return：恢复 TrapContext，恢复用户态特权状态并返回
```

#strong[图 9-3 双架构 trap 处理的公共流程]

RISC-V 从 `stvec` 进入 `trap.S`，硬件提供 `sepc`、`scause` 和 `stval`；LoongArch 从 `EEntry` 进入对应入口，硬件提供 `ERA`、`ESTAT` 和 `BADV`。两条路径随后都把通用寄存器和返回地址组织成 `TrapContext`，交给公共 syscall、page fault、timer 和 signal 逻辑处理。

=== 9.3.2 `TrapContext` 的统一字段与架构差异
<932-trapcontext-的统一字段与架构差异>
`TrapContext` 表示一次用户态执行被内核接管时需要保存的完整状态。两个架构都保存 32 个通用寄存器、用户返回 PC、用户栈指针以及返回特权级所需的状态，但字段形式不同：

#figure(
align(center)[#table(
  columns: 3,
  align: (col, row) => (auto,auto,auto,).at(col),
  inset: 6pt,
  [内容], [RISC-V64], [LoongArch64],
  [通用寄存器],
  [`x[32]`，ABI 参数使用 `a0-a7`],
  [`x[32]`，ABI 参数使用 `r4-r11`],
  [返回地址],
  [`sepc`],
  [`era`],
  [前一特权级],
  [`sstatus.SPP`],
  [`prmd.PPLV`],
  [返回中断状态],
  [`sstatus.SPIE/SIE`],
  [`prmd.PIE`],
  [浮点状态],
  [`f[32]`、`fcsr`],
  [当前 TrapContext 不单独保存浮点寄存器],
  [公共访问],
  [`get_a0/set_a0/get_sp/set_sepc` 等],
  [提供同名语义访问方法，内部映射到不同寄存器编号],
)]
)

#strong[表 9-2 两种架构的 TrapContext 对照]

公共 syscall 代码通过 `get_a0`、`set_a0`、`get_sepc` 等语义方法访问上下文，不直接读取 `x[10]` 或 `x[4]`。例如，RISC-V 的 syscall 返回值写入 `x[10]`，LoongArch 则写入 `x[4]`，但公共代码只需要调用 `set_a0`。这使 Linux ABI 参数处理和系统调用分发逻辑能够跨架构复用。

=== 9.3.3 返回用户态的安全顺序
<933-返回用户态的安全顺序>
RISC-V 的用户态返回路径包含一个必须保持的安全顺序：恢复过程中先清除 `SIE`，保持 `stvec` 指向内核 trap 入口，完成通用寄存器、浮点寄存器和 `sscratch` 等状态恢复后，才切换到用户 trap vector，最后执行 `sret`。用户态中断使能由 `sstatus.SPIE` 在 `sret` 时恢复。

如果提前恢复用户态 trap vector 或提前打开 SIE，timer 可能在 `sscratch` 和寄存器恢复尚未完成时重入用户 trap 入口，破坏 trap 保存约定。该顺序同时适用于普通 syscall 返回、exec 初始上下文和 signal `sigreturn`，因此被放在架构汇编层统一保证。

LoongArch 返回路径则恢复 `PRMD`、`ERA` 和通用寄存器，最后通过 `ertn` 返回。两者的返回指令不同，但共同遵守“完成内核上下文恢复后再恢复用户态特权状态”的原则。

== 9.4 任务上下文与调度切换
<94-任务上下文与调度切换>
=== 9.4.1 `TaskContext` 与 `TrapContext` 的分工
<941-taskcontext-与-trapcontext-的分工>
RespOS 将用户态异常上下文和内核态调度上下文分开。`TrapContext` 保存用户程序被中断时的完整执行状态，供 syscall、信号和 page fault 处理使用；`TaskContext` 只保存一次内核任务切换所需的调用约定寄存器、内核栈位置和地址空间 token，供 scheduler 的 `__switch` 使用。

```text
用户程序运行
    │ timer/syscall/page fault
    ▼
TrapContext：用户寄存器 + 返回地址 + 特权状态
    │ trap handler / syscall / signal
    ▼
内核调度点
    │ __switch
    ▼
TaskContext：ra + tp + callee-saved + kernel stack + MMU token
```

#strong[图 9-4 TrapContext 与 TaskContext 的职责分工]

这种分工避免每次内核调度都复制完整用户寄存器集合。任务被阻塞或主动让出 CPU 时，架构汇编只保存当前内核执行点能够继续运行所需的最小上下文；任务重新获得 CPU 后，从自己的内核栈恢复并继续执行。

=== 9.4.2 两套 `switch.S` 的共同布局
<942-两套-switchs-的共同布局>
RISC-V 和 LoongArch 的 `switch.S` 使用相同的抽象参数：下一个任务的内核栈指针和当前任务控制块中的栈指针位置。两套汇编都保存 `ra`、`tp`、callee-saved 寄存器和当前地址空间 token，然后恢复下一个任务的对应内容，切换栈并返回。

差异集中在寄存器名称、加载存储指令和地址转换刷新指令：RISC-V 写入 `satp` 后执行 `sfence.vma`；LoongArch 写入 `PGDL/PGDH` 后执行数据屏障、TLB invalidate 和指令屏障。调度器不需要知道这些差异，因此任务调度和阻塞唤醒逻辑保持在公共 `task` 模块中。

== 9.5 启动流程适配
<95-启动流程适配>
=== 9.5.1 公共启动阶段
<951-公共启动阶段>
两个架构最终都会进入 `rust_main_high`，公共启动顺序为：

```text
架构入口
    │
    ├─ 清零 BSS，建立早期栈和必要的地址映射
    ├─ 进入高半区公共 Rust 入口
    ├─ 初始化 trap
    ├─ 初始化内存管理
    ├─ 初始化网络和 initproc
    ├─ 初始化多核 idle task（RV64）
    ├─ 开启 timer interrupt 并设置第一次触发
    └─ 进入 task::run_tasks
```

#strong[图 9-5 双架构公共启动阶段]

公共启动顺序保证了依赖关系：trap 必须在处理异常前初始化，内存必须在创建任务和网络对象前可用，initproc 入队后才开始调度，timer interrupt 则在调度器准备好之后开启。

=== 9.5.2 RISC-V 启动路径
<952-risc-v-启动路径>
RISC-V `_start` 从 OpenSBI 进入 S-mode，获得 hart id 和 FDT opaque 参数。汇编入口根据 hart id 选择独立 early stack，写入 boot page table 的根到 `satp`，执行 `sfence.vma` 后跳转到 `enter_main`。Rust 入口随后完成 BSS 清零、读取 FDT 中的实际内存范围，并进入公共启动阶段。

RISC-V 的启动页表预先覆盖 QEMU virt 的最大可达内存窗口，以保证 boot hart 能访问 OpenSBI 放置在 RAM 顶部的 FDT；正式内存管理仍依据 FDT 的实际 `reg` 范围建立 frame allocator 和 kernel direct map。这体现了“早期可达窗口”和“真实资源上限”的明确分工。

=== 9.5.3 LoongArch 启动路径
<953-loongarch-启动路径>
LoongArch `_start` 进入时仍处于低地址直接映射环境。入口先保留当前执行段所需的 DMW 配置，再开启低地址 DMW，使早期代码可以访问低物理地址；随后 `enter_main` 跳转到 Rust `rust_main`。在公共启动前，LoongArch 建立覆盖早期内核镜像和内核堆的临时高地址页表，开启分页并跳转到共享高半区内核入口。

这一过渡解决了两个地址模型之间的切换问题：启动入口可以依赖固件提供的低地址直映，公共内核则可以从一开始使用与 RISC-V 相同的高半区地址约定。进入正式页表后，`LOW_DIRECT_MAP_ACTIVE` 等架构状态用于记录早期直映是否仍在使用，避免公共代码误把启动阶段映射当成最终地址空间模型。

== 9.6 功能与设计总结
<96-功能与设计总结>
RespOS 的硬件抽象与架构适配可以归纳为表 9-3：

#figure(
align(center)[#table(
  columns: 2,
  align: (col, row) => (auto,auto,).at(col),
  inset: 6pt,
  [设计成果], [实现方式],
  [公共内核跨架构复用],
  [`arch/mod.rs` 使用条件编译选择架构实现，并通过同名入口向公共模块提供统一语义],
  [双架构地址空间],
  [两个架构共享高半区内核模型，分别封装 `satp` 或 `PGDL/PGDH`、页表遍历和 TLB 刷新],
  [统一 trap 处理],
  [架构汇编保存不同硬件上下文，公共 Rust handler 复用 syscall、page fault、timer 和 signal 分发],
  [统一任务切换],
  [`TaskContext` 采用相同抽象布局，`switch.S` 仅特化寄存器保存、页表根切换和地址转换刷新],
  [双架构启动],
  [RV64 从 OpenSBI/FDT 进入，LA64 从低地址直映过渡到高半区，最终进入同一公共初始化顺序],
  [架构安全约束],
  [在 arch 层保证 trap return 顺序、页表根切换、TLB 刷新和早期映射过渡的正确性],
)]
)

#strong[表 9-3 硬件抽象层与架构适配的设计成果]

这里的做法不是把 RISC-V 和 LoongArch 的硬件差异全部抹平，而是把它们集中在入口、寄存器、页表、trap 汇编和任务切换等边界。公共内核围绕统一的任务、地址空间和系统调用语义运行，架构层再把这些语义落实到各自的指令集和特权体系上。
