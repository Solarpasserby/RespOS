# RespOS → Loongson 2K1000LA 真机移植执行方案

> 基于 RespOS `main` 分支真实代码（2026-08-16 通读）+ StarryOS/TGOSKits 2K1000LA 参考实现源码
> （`/tmp/tgoskits` 精读）+ 龙芯官方手册定位。所有地址/频率/中断号能给出处就给出处；
> 无法可靠确认的明确标 `待验证` 并给出确认方式，不猜测。

---

## 0. 结论速览（先给答案）

1. **RespOS 当前的 LoongArch64 实现有约 70% 是纯 LoongArch ISA 通用代码**（trap 上下文、任务切换、
   TLB refill、页表 PTE 编码、CSR 封装、CRMD/ECFG/ESTAT 语义），这部分**可以直接保留**。
2. **真正 QEMU-specific 的部分集中在 6 个文件**：`config/board.rs`（fw_cfg/RAM 布局/PCI/MMIO）、
   `entry/entry.asm`（DMW0 identity + QEMU aux boot ROM 次核）、`sbi.rs`（UART `0x1fe001e0` + GED 关机）、
   `pci.rs`（ECAM virtio-blk）、`smp.rs`（QEMU mailbox `0x1048` + IPI `0x1040`、12 hart）、
   `timer.rs`/`board.rs`（100 MHz 硬编码）。
3. **最大三个差异（卡点）**：① 启动链（QEMU `-kernel` ELF → U-Boot `go`/`bootelf`）；
   ② 中断控制器（QEMU 无外部中断 → 2K1000LA LIOINTC）；③ 地址空间位宽（RespOS 39-bit VA/三级页表 →
   2K1000LA **40-bit VA**）。
4. **最快上板路径**：Stage 1 只做「board 常量 + linker + entry + early NS16550 轮询打印」，
   不开 MMU/中断/SMP/FS，先稳定打出 `Hello RespOS on Loongson 2K1000LA`。

---

## 1. RespOS 当前 LoongArch64 实现全貌（已通读）

### 1.1 启动链（QEMU 现状）

```text
QEMU qemu-system-loongarch64 -machine virt -kernel <ELF>
  → QEMU 内置固件读取 ELF program headers（linker 用 AT() 指定物理装载地址）
  → ELF 载入物理 0x00200000，CPU 处于直接地址模式(CRMD.DA=1)，跳 entry=_start_phys
  → entry.asm: 设 DMW0(identity) + DMW1(PC 段窗口)，按 CSR 0x20(CPUNUM) 选 64 KiB early stack
  → enter_main → rust_main（物理地址执行）→ clear_bss → init_physical_memory_end(fw_cfg)
  → enable_boot_paging（建 boot 页表，写 PGDL/PGDH/ASID，CRMD.PG=1，关 DMW1）
  → jump_to_high_half(rust_main_high)  → trap::init → mm::init → ... → run_tasks
```

关键文件与常量：

| 事实 | 值 | 位置 |
|---|---|---|
| 装载/入口物理地址 | `0x00200000` | `linker_loongarch.ld:4` `PHYS_ADDR`、`os/Makefile:68` `KERNEL_ENTRY_PA` |
| 链接虚拟基址 | `0xffffffc000200000` | `linker_loongarch.ld:5` `BASE_ADDRESS` |
| 内核偏移 | `KERNEL_OFFSET = BASE_ADDRESS - PHYS_ADDR` | `linker_loongarch.ld:6` |
| entry 符号 | `_start_phys`（= `_start`） | `entry.asm:17-18`、`linker_loongarch.ld:2` `ENTRY(_start_phys)` |
| 初始 SP | `boot_stack_lower_bound + (CPUNUM+1)<<16`（64 KiB/hart） | `entry.asm:32-37` |
| 次核入口 | `_start_secondary_phys`（QEMU mailbox 跳入） | `entry.asm:40` |
| DTB/参数 | **无**（`rust_main()` 无参；RAM 用 fw_cfg，非 DTB） | `main.rs:53`、`board.rs:44` |
| 核号 | CSR `0x20`(CPUNUM) `& 0x3ff` | `smp.rs:171` |

### 1.2 物理内存布局（QEMU 现状）

`os/src/arch/loongarch64/config/board.rs`：

```rust
MEMORY_START       = 0
LOW_MEMORY_END     = 0x1000_0000   // 256 MiB low RAM
HIGH_MEMORY_START  = 0x8000_0000   // 2 GiB，PCI/MMIO 空洞之上的 high RAM
MEMORY_END         = 0x3_7000_0000 // 12 GiB 兼容 fallback
MAX_PHYSICAL_MEMORY_END = 0x9_7000_0000 // 36 GiB 上限
// fw_cfg: FW_CFG_DATA=0x1e020000, SELECTOR=+8, FW_CFG_RAM_SIZE=0x0003
```

- `init_physical_memory_end()` 读 fw_cfg `FW_CFG_RAM_SIZE`，换算 high end；失败保留 12 GiB。
- **RAM 是两段不连续区间**（low `0..0x10000000` + high `0x80000000..end`），中间是 PCI/MMIO 空洞。
- 依赖这三段布局的代码（需同步改）：
  - `frame_allocator.rs:164-179`（两段 range）
  - `memory_set.rs:2220-2260`（direct map：low 4 KiB 页 + high 2 MiB huge leaf）
  - `heap_allocator.rs:510-551`（kernel heap 位于 `HIGH_MEMORY_START`）
  - `mod.rs:enable_boot_paging`（boot 页表覆盖 low 128 MiB + high 512 MiB）
  - `drivers/virtio/mod.rs:132-159`（`phys_to_virt = pa + KERNEL_BASE`，direct map 判定）

### 1.3 DMW / 地址转换（QEMU 现状）

- **early boot 依赖 DMW**：`entry.asm` 设 DMW0=`0x11`（VSEG=0→PSEG=0，identity，MAT=CC，PLV0）、
  DMW1=PC[63:48] 窗口；`enable_boot_paging` 后 `write_dmw1(0)`，`disable_low_direct_map` 后 `write_dmw0(0)`。
- **DMW 编码（已由 StarryOS 2K1000LA 源码交叉证实）**：VSEG 是 **16-bit，位于 bit[63:48]**；
  `CSR_DMW0_VSEG<<48`（StarryOS `addrspace.rs:21-24` 的 `DMW_DA_BITS=PABITS=48`）。MAT 在 bit[5:4]，
  PLV0 在 bit0。RespOS `entry.asm:20-23` 的 `pcaddi; srli.d 0x30; slli.d 0x30` 正是把 PC[63:48] 放入 VSEG。
- **phys→virt 规则**：`addr + KERNEL_BASE`（`mm/address.rs:199,207`、`drivers/virtio/mod.rs:250`）。
- **MMIO 访问窗口**：DMW0 identity 期间用物理地址；开 MMU 后 `KERNEL_BASE + phys`（`sbi.rs:mmio_addr`）。
- **内核高半区**：`KERNEL_BASE = 0xffffffc000000000`（39-bit VA，三级页表 PGD/PMD/PTE 各 9 bit）。
- **用户最高地址**：`TRAMPOLINE = 0x3f_ffff_e000`（≈256 GiB 顶部，39-bit 低半区）。
- **是否假设了比 2K1000LA 更大的 VA？** —— **是，这是关键差异**：RespOS 用 39-bit VA（≤256 GiB 用户 +
  三级页表）；2K1000LA 是 **40-bit VA**（见 §3.3），且 StarryOS 因此启用 `loongarch64-low-va` 布局。

### 1.4 中断 / 定时器 / UART / 块 / 网 / SMP（QEMU 现状）

| 子系统 | QEMU 现状 | 文件 |
|---|---|---|
| 中断 | 只使能 Timer(ECFG bit11)+IPI(bit12)；**ESTAT bit2-9(HWI0-7) 外部中断 → panic** | `trap/mod.rs:196-201`、`register/mod.rs:87-115` |
| 定时器 | `rdtime.d` 稳定计数器 + `TCFG/TICLR` 单次；**硬编码 100 MHz**；100 Hz tick | `timer.rs`、`register/mod.rs:269-307`、`board.rs:12` |
| UART | NS16550 `0x1fe001e0`（byte stride），**轮询**；GED `0x100e0000+0x1c` 复位/关机 | `sbi.rs:9-23` |
| 块 | virtio-blk **PCI**（ECAM `0x20000000`），`BlockDevice`→`Disk`→lwext4 | `pci.rs`、`drivers/{device,disk,virtio}` |
| 网 | **仅 loopback**（smoltcp `LoopbackDev`），无 NetDevice 抽象、无 GMAC | `net/mod.rs` |
| SMP | QEMU IOCSR mailbox `0x1048`+IPI `0x1040`，`MAX_HARTS=12`，依赖 QEMU aux boot ROM 停放次核 | `smp.rs:17-27` |

---

## 2. QEMU-specific vs LoongArch 通用 完整清单

### A. 原则上可直接复用（纯 LoongArch ISA 通用，不改）

| 模块 | 文件 | 依据 |
|---|---|---|
| 用户态 trap 上下文（816 B，GPR/PRMD/ERA/LSX/FCSR/FCC） | `trap/trap.S`、`trap/context.rs` | 寄存器布局与 LoongArch ABI 对应，与板无关 |
| 内核态 trap 路径 | `trap/trap.S:__trap_from_kernel` | CSR/ESTAT 语义通用 |
| 任务上下文切换（TaskContext，PGDL/PGDH/ASID） | `task/switch.S` | 通用 CSR 操作 |
| 软件 TLB refill | `tlb_refill.S` | LDDIR/LDPTE/TLBFILL 通用 |
| 页表 PTE 编码（V/D/PLV/MAT/G/NR/NX/RPLV） | `mm/page_table.rs` | LoongArch PTE 格式通用（**层级数/VA 宽需适配，见 B/C**） |
| CSR 封装（CRMD/ECFG/EUEN/ESTAT/EENTRY/ERA/BADV/TCFG/TICLR/PGDL/PGDH/ASID/PWCL/PWCH/STLBPS/TLBRENTRY/DMW/INVTLB） | `register/mod.rs` | CSR 编号与语义通用 |
| 中断守卫（CRMD.IE） | `interrupt.rs` | 通用 |
| ASID（10-bit）管理与 shootdown 协议 | `mm/page_table.rs`、`smp.rs`（TLB 部分）、`memory_set.rs` | 通用（IOCSR 通道除外） |

### B. LoongArch 通用，但需板级参数适配

| 项 | 当前值（QEMU） | 2K1000LA 目标 | 改法 |
|---|---|---|---|
| RAM 起始/末址 | `0` / fw_cfg | DDR 起于 `0x00200000`（StarryOS 证实），末址看容量 | `board.rs` 常量 + 去掉 fw_cfg，改 FDT/board 常量 |
| 两段 RAM 空洞 | low 256M + high 2G 起 | 2K1000LA DDR 是否连续需 `fdt print /memory` 确认 | `frame_allocator`/`memory_set` 的 range 数 |
| kernel heap 物理位置 | `HIGH_MEMORY_START=0x80000000` | DDR 内一段（避开内核镜像与保留区） | `config/mm.rs::KERNEL_HEAP_PHYS_START` |
| linker 装载/链接地址 | phys `0x200000` / vma `0xffffffc000200000` | 见 §6 | `linker_loongarch.ld` + `Makefile` |
| `KERNEL_BASE` | `0xffffffc000000000`（39-bit） | **40-bit VA，需重定**（见 §3.3） | `config/mm.rs` |
| 三级页表层级/PWCL/PWCH | 3 级、DIR_WIDTH=9 | 2K1000LA 页表级数与 VA 宽需按手册/CPUCFG 核验 | `register/mod.rs::configure_page_walk` |
| timer 频率 | 100 MHz 硬编码 | 2K1000LA 稳定计数器频率（`cpucfg`/FDT/手册，**待验证**） | `board.rs::HARDWARE_CLOCK_FREQ` |
| UART base | `0x1fe001e0` | 2K1000LA UART0 同为 `0x1fe001e0`（QEMU virt 即仿 Loongson），**待板载 DTB 复核** | `sbi.rs` |
| 用户栈顶/TRAMPOLINE/MMAP 区 | 39-bit 布局 | 40-bit low-va 布局（参考 StarryOS `USER_STACK_TOP=0x4_0000_0000`） | `config/mm.rs` |

### C. 必须新增/替换的板级实现

| 项 | 说明 | 参考实现 |
|---|---|---|
| **LIOINTC** | 2K1000LA 主中断控制器，32 输入，级联到 CPU HWI 线 | StarryOS `platforms/somehal/src/arch/loongarch64/liointc.rs` |
| **AHCI/SATA** | 替换 virtio-blk-pci | StarryOS `drivers/ax-driver/src/block/ahci.rs`（FDT compatible `loongson,ls2k1000-ahci`） |
| **GMAC** | 替换 virtio-net（当前仅 loopback） | StarryOS `drivers/ax-driver/src/net/loongson_gmac.rs`（GMAC0=`0x4004_0000`） |
| **SMP 次核启动/IPI** | QEMU mailbox → 2K1000LA IOCSR IPI | StarryOS `entry.rs:_secondary_entry`（IOCSR `0x1028` mailbox） |
| **关机/复位** | GED → 2K1000LA 电源寄存器（LS7A/LS2K1000 PM 相关，**待验证**） | 手册/StarryOS `power.rs` |

---

## 3. 2K1000LA 关键硬件事实（来自 StarryOS 源码 + 官方手册定位）

> 这些是"不要猜"的核心，均已给出处。仍不能确认的标 `待验证`。

### 3.1 启动链与 U-Boot `go` 寄存器约定（关键结论）

```text
Boot ROM → PMON/固件 → U-Boot → (TFTP 加载内核 + DTB) → go ${entry} ${fdt_addr} → 内核
```

**U-Boot `go` 的传参约定（LoongArch，已由 StarryOS 源码证实，勿套 RISC-V a0/a1）**：

- U-Boot `do_go()` 把被启动程序当作 C 函数调用：**`a0 = argc`，`a1 = argv`（`char*[]` 指针数组）**。
- 各参数是**字符串**（hex 地址），不是裸指针值：`go 0x9000... 0x8f000000` → argc=2，
  argv[0]="0x9000..."（entry），argv[1]="0x8f000000"（fdt）。
- StarryOS 依据：`platforms/someboot/src/arch/loongarch64/entry.rs::setup_non_efi_fdt/uboot_go_fdt_arg`
  （`arg0 ∈ 1..=16` 且 `arg1!=0`，从 argv[1..] 解析 hex 字符串作为 FDT 地址）。
- 另有一条 UHI 协议：`a0 = usize::MAX-1` 时 `a1 = FDT 物理地址`（`UHI_FDT_ARG0`）。

> **结论**：RespOS 若走 `go`，entry 必须按 `a0=argc, a1=argv` 解析 FDT（不能按 `a0=hartid, a1=dtb`）。

### 3.2 StarryOS 的 header 与 load/entry 地址

- `starryos.bin` 是 **Linux 风格 PE/EFI 镜像**（`_head` 以 "MZ" 开头 + PE header），不是裸 binary。
  header 编码：`phys_link_kaddr = KERNEL_LOAD_ADDRESS`，`_kernel_entry = KERNEL_LOAD_ADDRESS + (kernel_entry - _head)`。
  见 `platforms/someboot/src/arch/loongarch64/head.rs`、`link.ld:62`。
- 实际数值（`someboot/build.rs`）：`kernel_paddr = 0x200000`，`kernel_vaddr = 0xffff_ffff_8000_0000`
  （prepare_loongarch64 强制），`efi_image_base = 0`（非 hv）。
- **内核位置无关**：`kernel_entry` 先设 DMW 窗口，跳到 `CACHE_BASE=0x9000_0000_0000_0000` 窗口，
  开 MMU，`relocate()` 按 `running(_head) - VM_LOAD_ADDRESS` 应用 `R_LARCH_RELATIVE` 重定位
  （`relocate.rs`）。因此 U-Boot 能把镜像加载到任意地址，`go` 到 `load + (entry 偏移)`。

### 3.3 2K1000LA 的 40-bit VA（关键差异）

- StarryOS 注释原文（`os/StarryOS/kernel/src/config/loongarch64.rs:8-10`）：
  > "LS2K1000 reports 40-bit virtual addresses, so it cannot use the 48-bit LoongArch layout below;
  > the board build enables `loongarch64-low-va` to use the older low VA window instead."
- `loongarch64-low-va` 布局：`USER_SPACE_SIZE = 0x3f_ffff_f000`（≈256 GiB）、
  `USER_STACK_TOP = 0x4_0000_0000`（16 GiB，"initial stack 必须低于 16 GiB，更高地址会在页表遍历前触发
  硬件 address-error"）、内核走 DMW 窗口（见下）。
- **StarryOS 的 DMW 窗口**（`addrspace.rs`）：PABITS=48；
  - DMW0：VSEG=`0x8000`、MAT=0（**uncached**）→ `UNCACHE_BASE=0x8000_0000_0000_0000`（MMIO 用）
  - DMW1：VSEG=`0x9000`、MAT=1（cached）→ `CACHE_BASE=0x9000_0000_0000_0000`（内核/直映用）
  - DMW2：VSEG=`0xa000`、MAT=2
  - 即 `to_cache(pa) = pa | 0x9000_0000_0000_0000`，`to_uncache(pa) = pa | 0x8000_...`。

> **结论**：RespOS 的 `KERNEL_BASE=0xffffffc000000000`（39-bit）**不能直接搬到 2K1000LA**。需在
> Stage 2 依据 `龙芯2K1000LA处理器用户手册` + CPUCFG + StarryOS `paging.rs` 重新确定：
> ① 页表级数（40-bit VA 需 4 级或非对称级）；② 内核高半区基址；③ 是否复用 DMW 窗口做内核直映。

### 3.4 中断拓扑（2K1000LA）

```text
CPU ESTAT HWI0-7（bit2-9，外部硬中断）
   ↑ 级联（DEFAULT_CASCADE_IRQ=2，4 条 parent HWI 线，CPU_HWI_BASE_IRQ=2）
LIOINTC（MMIO 0x1fe0_1400，ISR 0x1fe0_1040，32 输入）
   ↑ UART / SATA(AHCI) / GMAC / RTC / ...
```

- LIOINTC 寄存器（StarryOS `liointc.rs`）：`ROUTE=0x00, ENABLE=0x28, DISABLE=0x2c, POLARITY=0x30, EDGE=0x34`。
- compatible：`loongson,liointc` / `loongson,2k1000-icu` / `loongson,ls2k1000-icu`。
- EIOINTC（IOCSR `0x420` EXT_IOI_EN + regs `0x14a0..0x1c00`）**不属于 2K1000LA**（那是 LS3A5000/LS2K2000），
  2K1000LA 只用 LIOINTC。
- 各设备 IRQ 号：从**板载 DTB** 的 `interrupts`/`interrupt-parent` 读，StarryOS 也是 FDT 探测，未硬编码。

### 3.5 外设地址（能确认的）

| 设备 | 地址/参数 | 出处 |
|---|---|---|
| UART0（NS16550） | `0x1fe0_01e0`（byte stride）——与 QEMU virt 相同（QEMU 仿 Loongson），**待 DTB 复核 clock/baud** | RespOS `sbi.rs` + StarryOS `ns16550.rs`（FDT，默认 24 MHz） |
| LIOINTC | `0x1fe0_1400`（reg）/ `0x1fe0_1040`（ISR） | StarryOS `liointc.rs` |
| GMAC0 | `0x4004_0000` | StarryOS `loongson_gmac.rs:26` |
| SATA/AHCI | FDT compatible `loongson,ls2k1000-ahci`，base 从 DTB `reg` 读（**待 DTB**） | StarryOS `ahci.rs` |
| RTC | compatible `loongson,ls2k1000-rtc`（TOY 计数器） | StarryOS `time/loongson.rs` |
| DDR 基址 | `0x0020_0000`（StarryOS 内核装载点即 DDR 起点） | `someboot/build.rs:31` |
| timer 频率 | **待验证**（`cpucfg(4/5)`、FDT、或手册；QEMU 用 100 MHz 是仿 Loongson 稳定计数器） | — |

---

## 4. 分阶段移植路线

### Stage 0：确认开发板环境（只读，不改代码）
在板载 U-Boot 执行并保存输出：`version` `bdinfo` `printenv` `help go` `help bootelf` `help fdt`、
`fdt addr` `fdt print /memory` `fdt print /cpus` `fdt print /chosen` `fdt print /soc`、`pci` `scsi scan`。
目的：拿到 DDR 起止、核数、UART/LIOINTC/AHCI/GMAC 的 DTB reg/interrupt、TFTP 可用性、U-Boot 保留内存。

### Stage 1：RespOS entry + early UART（本文重点，见 §7）
单核、无中断/timer/heap/FS/MMU/用户态，只打印 `Hello RespOS on Loongson 2K1000LA`。

### Stage 2：DMW / MMU / 内存管理
恢复 DMW、页表、PGDL/PGDH、TLB refill、direct map、frame allocator、heap、FDT memory discovery。
重点验证 40-bit VA、内核虚拟布局、用户布局、MMIO 映射、DMA 地址。

### Stage 3：Exception / Interrupt / Timer
恢复 trap、timer、调度 tick、sleep、clock syscall。先让 timer 稳定，再接 LIOINTC。

### Stage 4：用户态（能不改就不改）
TaskContext/TrapContext/切换/用户页表/syscall/fork/exec/signal 基本与板无关，直接复用。

### Stage 5：块设备（AHCI/SATA 接入现有 BlockDevice）
### Stage 6：网络（GMAC 接入 smoltcp Device）
### Stage 7：SMP（IOCSR IPI + 次核启动）

---

## 5. 风险排序（最大卡点）

| 排名 | 模块 | 难度 | 不确定性 | 参考实现 | 真机调试难度 | 对上层影响 |
|---|---|---|---|---|---|---|
| 1 | 地址空间/DMW/40-bit VA | 高 | 高 | StarryOS low-va | 高（黑屏） | 高（MM 全层） |
| 2 | 启动协议（go/header/FDT） | 中 | 中 | StarryOS someboot | 中（可逐字符定位） | 低（只动 entry） |
| 3 | LIOINTC + timer | 中 | 低 | StarryOS liointc | 中 | 低（新增中断路径） |
| 4 | AHCI/SATA | 高 | 低 | StarryOS ahci-driver | 中 | 低（只换 BlockDevice 底层） |
| 5 | DMA/cache 一致性 | 高 | 中 | StarryOS GMAC/AHCI 的 dbar/ibar | 高 | 中 |
| 6 | GMAC | 高 | 中 | StarryOS loongson_gmac | 高 | 低（新增 smoltcp Device） |
| 7 | SMP（IOCSR IPI） | 中 | 低 | StarryOS _secondary_entry | 中 | 低（只换 arch::smp） |

**最可能卡住的点**：① 40-bit VA / 页表层级（Stage 2，最容易黑屏难定位）；② AHCI 的 DMA/cache
一致性（Stage 5，QEMU 下 virtio 不严格 flush 也能跑，真机必炸）。

---

## 6. 启动方式与 linker/DMW 决策

### 6.1 采用 `go` 还是 `bootelf`？
**推荐 `go`（raw binary）**，理由：
- 比赛高频 TFTP 调试，raw binary 最简单、最可控；StarryOS 已证明 2K1000LA 走 `TFTP + go` 可行。
- `bootelf` 需要 U-Boot 支持 ELF relocation 且把 ELF 段正确摆放，多一层不确定性。
- 不建议照搬 StarryOS 的 Linux PE header（那是为 EFI 兼容）；RespOS 只需**带一个极简 header 或纯裸 binary + `go`**。

**最简做法**：`objcopy -O binary` 生成裸 binary，链接时固定 `PHYS_ADDR=0x00200000`，U-Boot
`tftpboot 0x00200000 respos.bin` 后 `go 0x00200000 ${fdt_addr}`。entry 从 `a0=argc,a1=argv` 解析 FDT。

### 6.2 linker 处理（区分 load/link/runtime/DMW 地址）
- **load address（物理）**：`0x00200000`（DDR 起点，与 QEMU 现状一致，StarryOS 同值）。
- **link address（虚拟）**：Stage 1 保持与 QEMU 相同的高半区 `0xffffffc000200000` 也能跑通第一行打印
  （因为 entry 用 PC 相对寻址 + DMW0 identity，`rust_main` 在物理地址执行、打印用物理 UART）。
  Stage 2 再依据 40-bit VA 决定是否切到 StarryOS 的 `0x9000_...` DMW 窗口布局。
- **entry 偏移**：`entry = load + (kernel_entry - _head)`，裸 binary 下即 `0x00200000`（entry 在镜像开头）。
- 关键：`_start_phys` 与 `KERNEL_OFFSET` 关系保留（linker `AT()` 已把物理/虚拟分离）。

### 6.3 DMW 处理（Stage 1 → Stage 2）
- Stage 1：沿用 RespOS 的 DMW0 identity（`VSEG=0`），不开 MMU，用物理地址轮询 UART 即可打印。
- Stage 2：**不要直接照搬 StarryOS 的 `0x8000/0x9000/0xa000` 窗口**，先按手册/CPUCFG 确定 40-bit VA
  下：页表级数、内核基址、用户上界；再决定 DMW 是只做 early 过渡（RespOS 现状）还是做常驻直映窗口
  （StarryOS 方式）。

---

## 7. 第一阶段可执行修改任务清单（Stage 1：Hello RespOS on 2K1000LA）

> 目标：不碰 MMU/interrupt/heap/FS/SMP，只把「board + build + linker + entry + FDT + early UART」打通。

1. **新增 board 配置分离**（QEMU 与 LS2K1000 分叉，不复制整个 arch 目录）
   - 新增 `os/src/arch/loongarch64/config/` 下的 board 分叉：Cargo feature `board_ls2k1000`
     （`os/Cargo.toml [features]` 增 `board_ls2k1000 = []`），`config/board.rs` 内 `#[cfg(feature="board_ls2k1000")]`
     提供 LS2K1000 常量集（RAM `0x00200000..`、UART `0x1fe001e0`、无 fw_cfg、无 PCI ECAM 初值），
     否则保持 QEMU 常量。`config/mod.rs` 同步 `pub use`。
   - 顶层 `Makefile` 增 `make build-la-ls2k1000`（`LA_KERNEL_FEATURES=board_ls2k1000`）。

2. **linker**
   - 新增/参数化 `linker_loongarch_ls2k1000.ld`（或脚本内按符号覆盖）：`PHYS_ADDR=0x00200000`、
     `BASE_ADDRESS`（Stage 1 可先保持 `0xffffffc000200000`）、`ENTRY(_start_phys)`。

3. **entry**
   - `entry.asm` 增 `_start_ls2k1000`（或复用 `_start_phys`）：关中断（`crmd` 清 IE）、
     保存 `a0/a1`（argc/argv）到固定内存/寄存器、设 DMW0 identity、按 CPUNUM 选栈、跳 `rust_main`。
   - 删掉 QEMU 次核 mailbox 依赖（Stage 1 单核，次核入口先空转）。

4. **FDT 传入**
   - `rust_main` 增 `rust_main_ls2k1000(argc, argv)`：从 argv[1..] 解析 FDT 地址（复用 RV64 的
     `fdt_memory_end` 解析思路，或先只保存地址、Stage 2 再解析 `/memory`）。
   - assembly 保存 argc/argv 到 `.bss` 静态槽（Stage 1 先只打印不解析）。

5. **early UART**
   - `sbi.rs` 的 `UART_BASE` 改为板级（`0x1fe001e0`，与 QEMU 相同但独立成 board 常量），
     `console_putchar` 轮询逻辑不变（LSR bit5 等 THR 空）。这是第一行输出的唯一依赖。

6. **Makefile / objcopy**
   - `build-la-ls2k1000`：`cargo build --features board_ls2k1000` + `rust-objcopy -O binary --strip-all`
     生成 `respos-ls2k1000.bin`（供 U-Boot TFTP）。
   - QEMU 目标继续用 ELF（`-kernel`），2K1000LA 用 raw binary（`go`）。

7. **U-Boot 测试流程**
   ```text
   PC 起 TFTP server
   U-Boot: setenv serverip <PC_IP>; setenv ipaddr <板IP>
           tftpboot 0x00200000 respos-ls2k1000.bin
           tftpboot ${fdtaddr} <board>.dtb        # 或复用 U-Boot control FDT
           go 0x00200000 ${fdtaddr}
   ```
   （loadaddr/fdtaddr 必须以 Stage 0 的 `bdinfo`/`printenv` 结果为准，不照抄 StarryOS。）

### 7.1 逐字符启动诊断（黑屏定位）

| 字符 | 含义 | 上一字符出现、本字符未出现时应检查 |
|---|---|---|
| `0` | 进入 entry | 没打印：TFTP 未加载/`go` 地址错/entry 不在镜像头 |
| `1` | SP 建立成功 | DMW0 identity 未生效、boot_stack 未就位 |
| `2` | argc/argv 保存成功 | 寄存器误用、栈被覆盖 |
| `3` | 进入 Rust（rust_main） | `la.local`/跳转地址错误、`bl` 越界 |
| `4` | BSS 清零完成 | `sbss/ebss` 符号错（linker 物理/虚拟不一致） |
| `5` | early UART 能发字符 | UART 基址/stride 错、THR/LSR 轮询卡死 |
| `6` | FDT 地址解析（Stage 2 前可省略） | argv 解析逻辑、FDT 地址未对齐 |
| 最终 | `Hello RespOS on Loongson 2K1000LA` | 全链路 OK |

---

## 8. 18 个问题直接回答

1. **障碍**：启动协议、40-bit VA/页表、LIOINTC、AHCI/GMAC、SMP、timer 频率——见 §2/C。
2. **纯 LoongArch 通用代码**：trap 上下文/切换/refill/PTE/CSR/中断守卫——见 §2/A。
3. **QEMU-specific**：board.rs/sbi.rs/pci.rs/smp.rs/entry.asm/timer 频率——见 §2/C + §1。
4. **完全不改**：syscall/process/task/scheduler/VFS/ext4/PageCache/signal/ELF loader/fd/pipe/socket/smoltcp 栈/BlockDevice/NetDevice 抽象。
5. **只改参数**：RAM 范围、linker、DMW、timer 频率、UART base、CPU 数、DMA 范围——见 §2/B。
6. **必须重写**：LIOINTC、AHCI/SATA、GMAC、SMP 次核启动/IPI、关机/复位——见 §2/C。
7. **最大差异**：启动链 + 40-bit VA + 中断控制器 + 块/网设备。
8. **最早进入方式**：U-Boot `tftpboot` + `go`（raw binary + FDT）。
9. **`go` vs `bootelf`**：推荐 `go`（最简、最适合高频 TFTP）。
10. **第一行串口最快**：只改 board 常量 + entry + early NS16550 轮询，不开 MMU。
11. **linker/DMW**：见 §6。
12. **FDT 传递**：U-Boot `go` → `a0=argc, a1=argv`，从 argv 解析 FDT 地址（§3.1）。
13. **timer/中断恢复顺序**：先 timer（TCFG/TVAL/TICLR + 频率实测），再 LIOINTC（使能 ECFG 外部中断 + trap 处理 HWI）。
14. **AHCI**：实现 `BlockDevice` trait（`read_block/write_block/flush/num_blocks/block_size`），`Disk` 与 ext4/VFS 不动。
15. **GMAC**：实现 smoltcp `phy::Device`（仿 `LoopbackDev` 但 Medium::Ethernet + DMA），网络栈不动。
16. **SMP**：替换 `arch::smp` 的 mailbox/IPI 为 2K1000LA IOCSR IPI，scheduler/run queue/per-CPU 不动。
17. **最大风险**：40-bit VA/页表层级 + AHCI DMA/cache 一致性（§5）。
18. **拆步骤**：按 Stage 0→7，每个 milestone 一个 commit，随时可回退（§4）。

---

## 9. 待验证清单（去板载现场确认，不猜）

| 项 | 确认方式 |
|---|---|
| DDR 基址/末址/是否连续 | U-Boot `bdinfo`、`fdt print /memory`、`龙芯2K1000LA处理器用户手册`（telecom.mirrors.ustc.edu.cn） |
| 40-bit VA 页表级数与 PWCL/PWCH | 手册 + `cpucfg` + StarryOS `paging.rs` |
| timer 稳定计数器频率 | `cpucfg(4/5)`、FDT、手册 |
| UART clock/baud/reg-shift | 板载 DTB `chosen/stdout-path` 节点 |
| LIOINTC→CPU HWI 级联与各设备 IRQ 号 | 板载 DTB `/soc` 下各节点 `interrupts` |
| SATA/AHCI base/IRQ | 板载 DTB `loongson,ls2k1000-ahci` 节点 |
| 关机/复位寄存器 | 手册电源管理章 / 原理图 |
