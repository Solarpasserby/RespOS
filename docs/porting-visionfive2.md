# RespOS → VisionFive 2 真机移植执行方案

> 本文基于对 RespOS 当前 `main` 分支源码的逐行分析 + StarFive 官方 JH7110 设备树（U-Boot/Linux 树）
> 的直接核对，不是通用 RISC-V 教程。每个结论都标注了「代码依据」与「文档依据」。
> 标记为 **待验证** 的条目必须到真机用 `bdinfo / printenv / fdt / md` 确认，禁止猜测后烧板。

## 0. 结论速览（先回答八个问题）

1. **RespOS 现在离 VisionFive 2 启动还有哪些障碍？** 没有不可逾越的架构障碍。真正的硬障碍只有 4 处集中
   在 `rv64/` 的 QEMU 常量（linker 装载地址、early page table RAM 基址、10 MHz 时钟、VirtIO MMIO 地址）
   和 1 处 console 依赖（legacy SBI `console_putchar`）。块设备/网络/中断控制器是「缺失驱动」而非「需要重写」。
2. **哪些代码完全不用改？** syscall、task、scheduler、VFS、ext4、page cache、signal、绝大部分 MM、
   smoltcp 协议层、SBI abstraction（HSM/IPI/RFNC/SRST 与 JH7110 OpenSBI 完全兼容）。
3. **哪些是 QEMU-specific 代码？** 见 §2 完整清单（13 处常量/汇编 + 2 处驱动选择）。
4. **第一步改什么？** §7 的 Stage 1 最小改动（linker + entry.asm + board.rs 常量 + 一个 VF2 board 开关）。
5. **怎样最快看到第一行真机输出？** §4：`booti`（raw binary）+ SBI DBCN 或直写 UART0。预计半天内可见。
6. **怎样逐步恢复 MM/timer/task/FS/network/SMP？** §5 的 Stage 2–7，每阶段一个可验证里程碑。
7. **最困难、风险最大在哪？** SDIO/MMC 驱动（dw_mmc 变体 + syscon 电压配置）与 GMAC（stmmac + PHY + 时钟复位）。
8. **如何拆成可测试、可回退的小步？** 每阶段一个 UART 可见 marker + 独立 TFTP 启动，改动集中在一个
   `cfg` 门后，QEMU `virt` 构建保持可用作回归基线。

---

## 1. virtio-net 现状（随分支更新）

> 注意：最初分析基于 `main@9bc6ecd`，当时**没有** virtio-net。当前 `port/jh7110` 已切到
> `main@6f3fd4d`（合入 `feat/http`），**此时 virtio-net 驱动已经存在**：
> - `drivers/virtio/net_dev.rs`（`VirtIoNetDev`）、`drivers/mod.rs::find_virtio_net_mmio()`、
>   `net/ethernet.rs`（`EthernetDevice` 实现 smoltcp `Device`）、`net/mod.rs::init()` 拉起
>   `ETH_DEV`/`ETH_IFACE`（QEMU slirp 静态 IP `10.0.2.15/24`）。
> - 另有 `net/http.rs`（`kernel_http` feature）与 goldfish RTC（`timer.rs` `GOLDFISH_RTC_BASE=0x101000`、
>   `syscall::init_realtime_from_rtc()`）。

**因此 Stage 6 是「把 virtio-net 换成 JH7110 GMAC」**：`EthernetDevice` 目前硬编码适配
`VirtIoNetDev`，需改为适配 GMAC `phy::Device`；smoltcp/socket/tcp/udp 层与 `LoopbackDev` 完全可复用。

---

## 2. QEMU-specific 假设完整清单（代码级）

### 2.1 用户假设的逐条验证结论

| 用户假设 | 结论 | 证据 |
| --- | --- | --- |
| 1. linker 以 `0x80200000` 为基址 | ✅ 准确 | `os/src/linker_riscv.ld:4` `LOAD_ADDRESS = 0x80200000` |
| 2. entry.asm early page table 按 QEMU `0x80000000` RAM 建 | ✅ 准确 | `entry.asm:38,44` `boot_ppn = 0x80000`（=0x80000000>>12），16 个 1 GiB leaf |
| 3a. board.rs RAM 范围 | ✅ 准确 | `board.rs:8,9,11` `MEMORY_START=0x8020_0000`、`MEMORY_END=0x9000_0000`、`MAX=0x4_8000_0000` |
| 3b. 10 MHz 时钟 | ✅ 准确 | `board.rs:5` `HARDWARE_CLOCK_FREQ = 10_000_000` |
| 3c. VirtIO MMIO 地址 | ✅ 准确 | `board.rs:156-158` `0x1000_1000` / `0x1000_2000` |
| 4. 磁盘用 virtio-blk | ✅ 准确 | `drivers/mod.rs:24` `BlockDeviceImpl = VirtIoBlkDev<MmioTransport>` |
| 5. 网络用 virtio-net | ❌ **不成立** | 网络只有 loopback，无 NIC 驱动（见 §1） |
| 6. 已有 FDT 解析 RAM 范围代码 | ✅ 准确（且可复用） | `board.rs:27-154` `init_physical_memory_end` / `fdt_memory_end` |
| 7. SBI 完成 timer/IPI/HSM/shutdown/console | ✅ 准确 | `sbi.rs` 全文件；timer 用 `set_timer`，SMP 用 `hart_start`/`send_ipi`/`remote_sfence_vma` |
| 8. `_start` 按 `a0=hart id, a1=DTB` 启动 | ✅ 准确 | `entry.asm:6` 注释 + `main.rs:41` `rust_main(hart_id, opaque)` + `main.rs:47` 用 `opaque` 当 FDT |

### 2.2 尚未被用户点名的 QEMU-specific 假设（重要补充）

| # | 位置 | 当前值/行为 | 问题 | 建议 |
| --- | --- | --- | --- | --- |
| A1 | `os/src/linker_riscv.ld:3` | `BASE_ADDRESS = 0xffffffc080200000` | 高半区虚拟基址 = KERNEL_BASE + 0x80200000 | 改为 `0xffffffc040200000` |
| A2 | `os/src/arch/rv64/smp.rs:14` | `MAX_HARTS = 8` | QEMU 8 hart | 改 4（U74=hart 1..4，见 §3） |
| A3 | `os/src/arch/rv64/smp.rs:17` | `RV64_ENTRY_PHYS = 0x8020_0000` | HSM 次 hart 入口 | 改为内核装载地址 `0x4020_0000` |
| A4 | `os/src/arch/rv64/smp.rs:195-208` | `start_secondary_harts` 遍历 `0..MAX_HARTS` | 假设 hart id 从 0 连续；JH7110 的 hart0 是 S7 monitor（无 MMU），不可当 CPU 启动 | 遍历 U74 hart id `1..=4`，跳过 boot hart |
| A5 | `os/src/arch/rv64/smp.rs:60,65-72` | `PER_CPUS[hart_id]`、`init_current_hart` assert `hart_id < MAX_HARTS` | JH7110 hart id 最大是 4，若 MAX_HARTS=4 则 `hart_id=4` 越界 | 数组大小要覆盖 hart id 4（即 5 份）或做 id→index 映射 |
| A6 | `os/src/mm/memory_set.rs:2224` | `first_gigabyte_end = 0xc000_0000` | 硬编码 QEMU「RAM 基址 + 1 GiB」（0x80000000+1GiB） | JH7110 应 = `0x8000_0000`（0x40000000+1GiB） |
| A7 | `Makefile:170` | `rust-objcopy --set-start=0x80200000` | ELF entry 写死 0x80200000 | 随 board 变为 0x40200000（或用 `-O binary` + `booti`） |
| A8 | `Makefile:244-258` | `-machine virt -bios default ... virtio-mmio-bus` | QEMU 专用启动参数 | 新增独立 `make vf2` 目标，不影响比赛 `make all` |
| A9 | `os/src/arch/rv64/sbi.rs:99,107` | `sbi_rt::legacy::console_putchar/getchar` | 依赖 OpenSBI legacy 扩展（EID 0x01/0x02），较新 OpenSBI 可能 `FW_LEGACY` 关掉 | 换 SBI v0.2+ DBCN（`console_write`）或直写 UART0（§7） |
| A10 | `os/src/arch/rv64/timer.rs:6,98` | 时间换算用 `HARDWARE_CLOCK_FREQ=10 MHz` | JH7110 `timebase-frequency=4 MHz` | 改 4_000_000 |
| A11 | `os/src/arch/rv64/trap/mod.rs` | 只处理 SupervisorTimer / SupervisorSoft，无 SupervisorExternal | 无 PLIC/外部中断驱动 | Stage 3/5/6 需要时补 PLIC + `sie::set_sext` |
| A12 | `os/src/drivers/virtio/mod.rs:156-157` | `virt_to_phys` 用 `KERNEL_BASE..KERNEL_BASE+physical_memory_end()` 判 direct map | 依赖 direct map = KERNEL_BASE + pa 的约定 | 约定保留（JH7110 同构），只换 MMIO 表 |

### 2.3 一个被 QEMU 掩盖、在 JH7110 上会暴露的真实 bug

`fdt_memory_end`（`board.rs`）从 `/memory` `reg` 取到的是**整条 DDR 上限**（JH7110 = 8 GiB），
`init_frame_allocator`（`mm/frame_allocator.rs:156-161`）就以 `[reserved_end, memory_end)` 作为可分配区间。
QEMU 里 OpenSBI/DTB 位于 RAM 顶部，但栈式分配器自底向上分配、workload 远不能把 16 GiB 吃光，所以从不触顶。

JH7110 上**不一样**（真机 `bdinfo` 已实测，4 GiB 版）：
- DTB 被 U-Boot 放在 `fdt_addr_r=0x46000000`（DDR 中下部，**处于 frame allocator 区间内**）；
- OpenSBI 在 DDR **底部** `0x40000000`（`Firmware Base: 0x40000000`，`reserved[0] [0x40000000-0x4007ffff]`；
  内核装在 `0x40200000` 紧跟其后，所以底部 OpenSBI 区天然被排除在 frame allocator 之外）；
- U-Boot 重定位到 DDR **顶部**（`relocaddr=0xfff44000`，紧贴 4 GiB 末址 `0x13fffffff`），**在区间内**。

一旦分配器推进到这些地址，会覆盖 DTB/顶部 U-Boot。**Stage 2 必须处理**：保留 `/reserved-memory`、
把 DTB 复制/保留到安全地址，并把 `memory_end` 上界压到 U-Boot 重定位区（`0xfff44000`）之下。
Stage 1（只输出 Hello，几乎不分配）可暂不处理。

### 2.4 A/B/C 复用分类（对应需求第五部分）

**A. 可直接复用（零改动）**
- syscall 全套、task/scheduler/processor、VFS/namei/fd、ext4、page cache、signal、用户程序与 testrunner。
- smoltcp 协议层 + `LoopbackDev`（`net/` 全部）。
- SBI 抽象：HSM `hart_start`、SPI `send_ipi`、RFNC `remote_sfence_vma`、SRST `system_reset`、
  `set_timer`——JH7110 OpenSBI 均实现，语义与 QEMU OpenSBI 一致。
- MM：`mm/page_table.rs`（Sv39 通用）、`frame_allocator.rs`（栈式）、`heap_allocator.rs`（buddy）、
  `memory_set.rs` 的用户态路径。
- `trap.S`/`TrapContext`：U74 是 `rv64imafdc_zba_zbb`，与编译 `-Ctarget-feature=+f,+d` 一致，无需改。

**B. 少量参数适配（改常量/汇编表）**
- `linker_riscv.ld`：`LOAD_ADDRESS`/`BASE_ADDRESS`。
- `entry.asm`：`boot_pagetable` 的 RAM 基址 `0x80000`→`0x40000`、GiB 数 16→8、布局表。
- `board.rs`：时钟 10 MHz→4 MHz、`MEMORY_START/END/MAX`、`VIRTIO_MMIO`→JH7110 MMIO 表。
- `mm/memory_set.rs`：`first_gigabyte_end`。
- `smp.rs`：`MAX_HARTS=4`、`RV64_ENTRY_PHYS`、hart id 1..4 拓扑与 `PER_CPUS` 尺寸。
- `Makefile`：`-O binary` 出 `kernel-vf2.bin` + 独立 `vf2` 目标。
- `timer.rs`：已参数化，只改 `HARDWARE_CLOCK_FREQ` 常量。

**C. 必须新写的板级驱动**
- UART0（DW8250，仅当 SBI console 不可用时才需要；否则可省）。
- PLIC + 外部中断使能（`sie::set_sext` + `trap_handler` 增加 SupervisorExternal 分支）——块/网设备要
  中断时必需，纯轮询可暂缓。
- SDIO/MMC（`starfive,jh7110-mmc` = dw_mmc 变体 + `sys_syscon` 电压/驱动配置）。
- GMAC（`starfive,jh7110-dwmac` = stmmac + MDIO/PHY + `aoncrg`/`syscrg` 时钟复位）。
- （可选）clock/reset 控制器封装，UART/SDIO/GMAC 共享。

---

## 3. JH7110 官方文档核对结果（StarFive JH7110 设备树）

以下直接取自官方 `jh7110.dtsi` / `jh7110-starfive-visionfive-2.dtsi`（StarFive U-Boot 树，
commit `625e68ef`，googlesource 镜像），比博客可靠：

| 项目 | 值 | DTS 依据 |
| --- | --- | --- |
| DDR 基址 / 大小 | `0x40000000`，8 GiB（`0x40000000..0x240000000`） | board dtsi `memory@40000000 { reg = <0x0 0x40000000 0x2 0x0>; }` |
| timebase-frequency | **4 MHz**（`4000000`） | board dtsi `cpus { timebase-frequency = <4000000>; }` |
| console UART | **UART0** `0x10000000`，115200 8N1，DW8250 | `aliases serial0=&uart0`、`chosen stdout-path="serial0:115200n8"`、`uart0 serial@10000000 compatible="snps,dw-apb-uart"` |
| UART 寄存器 | `reg-io-width=<4>`、`reg-shift=<2>`（寄存器 4 字节步进） | uart0 节点 |
| CLINT | `0x2000000`（64 KB） | `clint timer@2000000 compatible "starfive,jh7110-clint","sifive,clint0"` |
| PLIC | `0xc000000`（64 MB），`riscv,ndev=<136>` | `plic interrupt-controller@c000000` |
| CPU 拓扑 | 1×S7（hart0，`rv64imac`，无 MMU，DTS `status=disabled`）+ 4×U74（hart1..4，`rv64imafdc_zba_zbb`，Sv39） | cpus 节点 `S7_0 cpu@0`、`U74_1..4 cpu@1..4` |
| SD/eMMC | mmc0 `0x16010000`（SD 卡）、mmc1 `0x16020000`（eMMC） | `mmc0/mmc1 compatible "starfive,jh7110-mmc"` |
| 以太网 | gmac0 `0x16030000`（RJ45）、gmac1 `0x16040000` | `gmac0 ethernet@16030000 compatible "starfive,jh7110-dwmac","snps,dwmac-5.20"` |
| PCIe/NVMe | pcie0 `0x2b000000`、pcie1 `0x2c000000`（M.2 槽） | `pcie0/pcie1` 节点 |
| 根节点 cells | `#address-cells=<2>`、`#size-cells=<2>` | board dtsi 头 |

**版本差异说明**：`timebase-frequency` 曾从 `cpus` 节点移到 `.dtsi` 又移回（LKML 2023-11/2024-01 系列
patch）。当前（StarFive devel 树）位于 board dtsi 的 `cpus` 节点、值为 4000000，以此为准。若真机
`fdt` 显示不同，以真机为准。

**启动链**：BootROM → SPL（QSPI/SD/eMMC）→ OpenSBI（M-mode）→ U-Boot（S-mode）→ kernel（S-mode）。
OpenSBI 在 **hart 1** 上启动（`OpenSBI boots on Hart 1`，官方 OpenSBI 平台代码）；U-Boot 的
`boot_jump_linux` 会以 `a0=gd->arch.boot_hart`、`a1=gd->fdt_blob` 进入内核，**正好匹配 RespOS `_start` 的
`a0=hart id / a1=DTB` 约定**——这是选择 `booti`/`bootm` 而非 `go`/`bootelf` 的根本原因（见 §4.2）。

**U-Boot 默认环境变量（已从 StarFive U-Boot 源码确认）**：`include/configs/starfive-visionfive2.h`
（[JH7110_VisionFive2_devel 分支](https://raw.githubusercontent.com/starfive-tech/u-boot/JH7110_VisionFive2_devel/include/configs/starfive-visionfive2.h)）：

| 变量 | 值 |
| --- | --- |
| `kernel_addr_r` | `0x40200000` |
| `fdt_addr_r` | `0x46000000` |
| `ramdisk_addr_r` | `0x46100000` |
| `scriptaddr` | `0x43900000` |
| `loadaddr` | `0x60000000` |
| `kernel_comp_addr_r` / `kernel_comp_size` | `0x5a000000` / `0x4000000` |
| `fdt_high` / `initrd_high` | `0xffffffffffffffff`（**禁用 DTB 重定位**，即内核收到的 `a1` 指向 `0x46000000`） |
| `CONFIG_SYS_BOOTM_LEN` | 64 MiB（RespOS ~11 MiB 远小于上限） |

`fdt_high=0xffffffffffffffff` 是关键：U-Boot 不会把 DTB 搬到高地址，`booti` 后内核 `a1=0x46000000`。

**StarryOS 源码印证**（crate `ax-plat-riscv64-visionfive2` 0.1.10，与上述 DTS 完全一致）：
- `boot.rs` `_start`：注释明确 `PC = 0x40200000, a0 = hartid, a1 = dtb`；进入后 `addi a0, a0, -1`
  把 hart id（1..4）归一为 0-based cpu id——RespOS 目前直接用 `a0` 当 hart id，Stage 7 需照此处理。
- `console.rs`：**不走 SBI**，直接 `uart_16550::MmioSerialPort::new_with_stride(addr, 4)`（stride=4 =
  `reg-shift=2`），且不调 `init()`，证明 firmware 已初始化 UART0、直写即可。
- `irq.rs`：PLIC S-mode context = `2 × hart_id`（hart0=S7 无 S 态）；timer 用 `riscv::register::time` +
  `sbi_rt::set_timer`，与 RespOS 完全同构。
- `mem.rs`：free RAM 从 `KERNEL_BASE_PADDR=0x40200000` 起算（`0x40000000..0x40200000` 被 SBI 占用），
  且 `reserved_phys_ram_ranges()` 暂返回空、有 `TODO: parse dtb`——即它也未解析 `/reserved-memory`，
  印证 §2.3 的 DDR 顶部保留问题在 StarryOS 同样未闭环。

**真机实测确认（2026-08，用户 VisionFive 2 v1.3B / 4 GiB，U-Boot 2021.10 SDK 5.15）**：
- `DRAM: 4 GiB`（`0x40000000..0x13fffffff`）；`PCB revision: 0xb2`、`chip_vision=B`、`BOM revision: A`。
- OpenSBI v1.2：`Firmware Base 0x40000000`（底部 512 KB `reserved[0]`）、`aclint-mtimer @ 4000000Hz`、
  `Boot HART ID: 1`、`Domain0 Next Address: 0x40200000`、`Platform Console Device: uart8250`。
- U-Boot 重定位到顶部 `relocaddr=0xfff44000`；`booti`/`tftpboot`/`dhcp`/`ping` 均可用；
  `fdtfile=starfive/starfive_visionfive2.dtb`；`bootargs=...earlycon=sbi` 说明 **SBI console 在本固件可用**。
- 结论：§4/§7 的 `0x40200000` 装载、`0x46000000` DTB、`booti` 选型、4 MHz、直写 UART0 全部成立。

---

## 4. 最小真机启动方案（Stage 0/1）

### 4.1 U-Boot 启动形式选型

| 方式 | a0/a1 是否合规 | 可靠性 | 结论 |
| --- | --- | --- | --- |
| `bootelf kernel-rv`（ELF） | ❌ 通常传 `(0, fdt)` 甚至 `(argc,argv)`，版本相关 | 中 | 不选 |
| `go 0x40200000`（raw） | ❌ `(0, fdt_blob)`，hart id 丢失 | 高（裸跳） | 仅 Stage 0 快速试跳可用 |
| `booti 0x40200000 - 0x46000000`（raw + 显式 DTB） | ✅ `boot_jump_linux` 传 `(boot_hart, fdt_blob)` | 高 | **推荐** |
| `bootm`（FIT/legacy uImage） | ✅ 同上 | 高但需打包 | 后续需要 initrd/多镜像时再上 |

**结论**：产物转成 raw binary，用 `booti`。因为 `booti` 走 U-Boot RISC-V 的 `boot_jump_linux`，
它按 RISC-V 启动协议传 `a0=hart id, a1=DTB`，与 `_start` 完全一致，且不需要 Linux `Image` 头。

### 4.2 需要改什么（Stage 1 = 输出 "Hello RespOS on VisionFive 2"）

改动集中在 `rv64/`，用一个 `JH7110` 编译开关门住（保证 QEMU 回归不受影响）：

1. **`os/src/linker_riscv.ld`**
   `LOAD_ADDRESS = 0x40200000; BASE_ADDRESS = 0xffffffc040200000;`（KERNEL_OFFSET 不变，仍是
   `0xffffffc000000000`）。对应新增 `linker_jh7110.ld`，或让 `LOAD_ADDRESS` 由构建脚本符号传入。
2. **`os/src/arch/rv64/entry/entry.asm`** — `boot_pagetable` 重写为 JH7110 布局（§7.3 给出精确结构）。
3. **`os/src/arch/rv64/config/board.rs`**
   - `HARDWARE_CLOCK_FREQ = 4_000_000`（USER/ACCOUNTING 跟随）。
   - `MEMORY_START = 0x4020_0000`（内核装载地址；与当前 QEMU 语义一致，且 StarryOS 的
     `KERNEL_BASE_PADDR=0x40200000` 同样从装载地址起算，因为 `0x40000000..0x40200000` 被 SBI 占用）、
     `MEMORY_END`（FDT 兜底）= `0x8000_0000`、`MAX_PHYSICAL_MEMORY_END = 0x2_4000_0000`（8 GiB）。
   - `init_physical_memory_end` 的 `clamp` 下界相应改。
   - `VIRTIO_MMIO` 表暂时清空（Stage 1 不用），后续替换为 JH7110 MMIO 表（UART/CLINT/PLIC/SDIO/GMAC）。
4. **`os/src/mm/memory_set.rs:2224`** — `first_gigabyte_end = 0x8000_0000.min(memory_end)`。
5. **`os/src/arch/rv64/smp.rs`** — Stage 1 只让 boot hart 跑：`MAX_HARTS` 暂留、`start_secondary_harts`
   先不调用（在 `rust_main_high` 里注释掉或 gate）。SMP 细节留到 Stage 7。
6. **SBI console**（最早期日志仍可继续用 SBI，但换掉 legacy）：
   `sbi.rs::console_putchar` 从 `sbi_rt::legacy::console_putchar` 改为 SBI DBCN
   `console_write`（或 raw ecall `EID=0x4442434E FID=0`）。若真机 OpenSBI 连 DBCN 都没有（很少见），
   回退直写 UART0（DW8250，寄存器 `reg-shift=2`，约 40 行）。
7. **Makefile**：新增 `build-vf2` / `run-vf2`（TFTP 用，见 §7.6），
   `rust-objcopy -O binary --gap-fill=0` 输出 `kernel-vf2.bin`（raw binary，byte 0 = `_start`）。

### 4.3 U-Boot 加载命令（TFTP，不烧板）

```text
setenv ipaddr 192.168.x.10          # 板子 IP，按你的网段改
setenv serverip 192.168.x.1         # TFTP 服务器 IP
tftpboot 0x40200000 kernel-vf2.bin  # 拉取 raw binary
tftpboot 0x46000000 jh7110-visionfive-v2.dtb  # 拉取 DTB（可选，Stage1 可先不用）
booti 0x40200000 - 0x46000000       # 跳转；- 表示无 initrd
```

Stage 1 若暂时不用 FDT（内存硬编码），可省略 DTB、`booti 0x40200000 -`（但 a1 会是 0，`fdt_memory_end`
会因 magic 校验失败而安全回退——已有代码路径）。

---

## 5. 分阶段移植路线（每个阶段一个 UART 可见 marker）

### Stage 0：验证开发板启动环境
- 上电看 U-Boot banner；`bdinfo`（确认 DRAM 范围、boot hart）、`printenv`（确认
  `kernel_addr_r/fdt_addr_r/ramdisk_addr_r/scriptaddr`）、`fdt addr` + `fdt list /memory`（确认 DDR）、
  `md 0x10000000`（UART0 可读）、`tftpboot` 通（`ping` 服务器）。
- 出口条件：`bdinfo` DRAM = 8 GiB、`fdt` 能看到 `memory@40000000`。

### Stage 1：`_start` → SBI console → "Hello RespOS on VisionFive 2"
- 单核、不开 driver、不启动用户程序、不要求 FS。出口：串口 115200 输出 Hello。

### Stage 2：MMU / 内存管理
- early page table、高半区内核映射、frame allocator、heap、FDT memory discovery 在 JH7110 上工作。
- 出口：`MemTotal`/`free_frames` 反映 8 GiB（且不覆盖 DTB/OpenSBI，见 §2.3）。

### Stage 3：异常、中断、Timer
- trap、timer interrupt、scheduler、sleep、时钟相关 syscall。
- 关键改动：`HARDWARE_CLOCK_FREQ=4 MHz` 让 `rdtime`→ms 换算正确；SBI `set_timer` 在 JH7110 直接可用。
- 出口：`sleep`/`gettimeofday` 走时正确、scheduler tick 正常。

### Stage 4：用户态
- task/fork/exec/syscall/signal。**此层与板级几乎无关**（唯一耦合是 trampoline/trap context，已在
  RV64 通用路径里）。出口：跑一个静态/动态 hello 用户程序。

### Stage 5：块设备
- 把 `VirtIoBlkDev` 换掉，但**只换最底层 `BlockDevice` trait 实现**，`Disk`/ext4/VFS/page cache 不动。
- 选型分析：比赛环境第一个真实块设备**首选 SD 卡（mmc0, dw_mmc 变体）**——TFTP 加载的启动流里 SD 卡
  已有 ext4 根盘、供电/时钟/复位由 firmware 配置好、接口成熟；eMMC（mmc1）与 SD 同为
  `starfive,jh7110-mmc` 驱动，只差基址与 syscon；NVMe（PCIe）工作量大得多（PCIe RC + NVMe 协议），不优先。
- 出口：`BlockDevice` 读到 SD 卡 ext4 superblock，`/` 挂载。

### Stage 6：网络
- 不是「换 virtio-net」，而是新增 `phy::Device`（gmac0, `snps,dwmac-5.20`/stmmac）+ PHY（RGMII）+ 时钟复位。
- 可复用：smoltcp `Interface`/`SocketSet`/tcp/udp/`LoopbackDev` 的全部上层。
- 出口：ping/iperf 走真实 RJ45。

### Stage 7：SMP
- 直接可保留：SBI HSM `hart_start`、SPI `send_ipi`、RFNC `remote_sfence_vma`、per-hart stack 结构。
- 要改：`MAX_HARTS=4` + hart id 1..4 映射、`RV64_ENTRY_PHYS=0x40200000`、`start_secondary_harts`
  只启动 U74、跳过 hart0(S7) 与 boot hart、`PER_CPUS` 数组按 hart id 4 定尺寸。
- 出口：`online mask` 显示 4 个 U74 全上线、`nproc`=4。

---

## 6. 设备树平台抽象：比赛最小改法 vs 后续架构改法

- **比赛期间最小改法**：保留 `board.rs` 的常量风格，新增 `JH7110` 的 `const`（DDR/时钟/MMIO 表）并用
  `#[cfg(feature="board_jh7110")]` 切换；FDT 只继续用于**内存上限发现**（现有 `fdt_memory_end` 已够用）。
  设备地址先硬编码（上面 DTS 表）。**不要**为「架构漂亮」重构成统一 device abstraction。
- **后续更合理架构**：把 `fdt_memory_end` 泛化为一个最小 libfdt 风格的 FDT walker，在启动期解析
  `/memory`、`/chosen`（bootargs/stdout）、`/soc/{uart,clint,plic,mmc,gmac}` 的 `reg`/`interrupts`，
  填充一个 `BoardDescription` 结构，替代散落的 `pub const`。这需要 `alloc`（heap）可用后执行，正好放在
  `mm::init()` 之后。前提是 DTB 要保留（不能像现在一样可被 frame allocator 覆盖）。

---

## 7. 第一批可执行修改任务（Stage 1）

### 7.1 增加 VisionFive 2 board 配置
- `os/Cargo.toml` 增 `board_jh7110` feature（与 `la_global_kernel` 并列，**不在 default**）。
- `board.rs` 用 `#[cfg(feature="board_jh7110")]` 提供 JH7110 常量组（§4.2 第 3 条）。
- **隔离落地（已实现）**：JH7110 专属内容放独立文件，QEMU 零改动——
  `os/src/linker_jh7110.ld`、`os/src/arch/rv64/entry/entry_jh7110.asm`、
  `os/cargo/config-jh7110.toml`；`entry/mod.rs` 按 feature 选择汇编，
  `Makefile` 新增 `prepare-jh7110-cargo-config` + `build-vf2`（`--features board_jh7110`）。
  默认 `make build-rv` 仍走 `entry.asm` + `linker_riscv.ld` + QEMU 常量，行为不变。

### 7.2 linker script
`LOAD_ADDRESS` 0x80200000 → **0x40200000**；`BASE_ADDRESS` → **0xffffffc040200000**。
`KERNEL_OFFSET` 保持 `0xffffffc000000000`。

### 7.3 `_start` / early page table
`entry.asm` 的 `boot_pagetable` 改为 StarryOS 已验证的最小 4 项布局（额外**补上 MMIO 区**，使早期可直写
UART0 定位失败，这点比我原方案的「只映射 DDR」更优）：

```text
boot_pagetable:
    .align 12
    # identity 0x0000_0000..0x4000_0000（含 MMIO：UART0/CLINT/PLIC）
    .quad (0x0 << 10) | 0xcf
    # identity 0x4000_0000..0x8000_0000（首 1 GiB DDR：kernel/DTB/heap）
    .quad (0x40000 << 10) | 0xcf
    .zero 8 * 254                      # VPN2=2..255 未映射
    # high-half 0xffffffc0_0000_0000..0xffffffc0_4000_0000（MMIO 高半，供 KERNEL_BASE+pa 访问）
    .quad (0x0 << 10) | 0xcf
    # high-half 0xffffffc0_4000_0000..0xffffffc0_8000_0000（首 1 GiB DDR 高半，含 0x40200000 内核）
    .quad (0x40000 << 10) | 0xcf
    .zero 8 * 254                      # VPN2=258..511 未映射
```
（1+1+254+1+1+254 = 512 项。StarryOS `boot.rs::init_boot_page_table` 正是这 4 项、flags 用 `0xef` 带
Global 位；RespOS 沿用现有 `0xcf`（无 Global）以保持与 QEMU 页表一致。首 1 GiB DDR 足够覆盖
kernel(0x40200000)/DTB(0x46000000)/heap(0x40b00000..)，其余 7 GiB 由 `mm::init` 的正式 direct map
按 FDT 实际末址补齐——比映射满 8 GiB 更省、更贴合 StarryOS 实机验证。）

### 7.4 FDT 地址保存与传递
- 现有 `rust_main(hart_id, opaque)` 已经把 `a1`（=DTB PA）作为 `opaque` 传入，并立即
  `config::init_physical_memory_end(opaque)`。**这段不用改**；只需保证 early page table 覆盖 DTB
  所在物理地址（U-Boot 的 `fdt_addr_r≈0x46000000`，落在 8 GiB identity/high-half 映射内）。
- Stage 1 若不加载 DTB，`fdt_memory_end` 对 magic 校验失败安全返回 `None`，走 `MEMORY_END` 兜底，不会 panic。

### 7.5 SBI console 是否可继续用于最早期日志
可以，但**不要依赖 legacy `console_putchar`**。改 `sbi.rs::console_putchar` 为 SBI DBCN（`console_write`，
EID `0x4442434E`）。若真机无输出，再直写 UART0：基址 `0x10000000`，`reg-shift=2`，用 `KERNEL_BASE + pa`
作虚拟地址，DW8250 LSR/THR 轮询写（无需开时钟，firmware 已配好）。

**StarryOS 印证**：它的 `console.rs` **不走 SBI**，而是直接 `uart_16550::MmioSerialPort::new_with_stride(addr, 4)`
（stride=4 即 `reg-shift=2`），且 `init_early()` 不调 `uart.init()`——证明 firmware 已把 UART0 初始化好、
直写即可用。这比 SBI console 更可靠，建议 Stage 1 直接采用直写 UART0（约 40 行 16550 驱动），SBI DBCN 作为
备选。

### 7.6 Makefile 生成真机 kernel
```make
build-vf2:
    # 复用 build-rv 的 cargo build，再转 raw binary
    rust-objcopy -O binary --gap-fill=0 os/target/riscv64gc-unknown-none-elf/release/os kernel-vf2.bin
```
TFTP 上传 `kernel-vf2.bin`。**不要**覆盖线上 `make all`（比赛入口固定 `kernel-rv` 仍是 QEMU ELF）。

### 7.7 U-Boot 命令
见 §4.3。

### 7.8 如何用 UART 判断失败在哪一阶段
- 无任何输出：`_start` 之前就死 → 查装载地址是否 0x40200000（`booti` 前 `md 0x40200000` 看 ELF 头/
  代码是否在）、U-Boot 是否跳到错误地址；或 SBI console 不可用（换 UART0 直写）。
- 输出乱码/无输出但板子跑：波特率或 UART 引脚不对 → 用 `printenv` 确认 console UART。
- 卡在 `_start` 之后：early page table 错误 → 加一个直写 UART0 的「stage marker」在 `enter_main` 前后。
- 进了 `rust_main` 但卡在 `mm::init()`：多半是 heap/frame allocator 越界（`§2.3` 的 DDR 顶部问题）。
- 每阶段在关键路径打印 `[vf2] stage N` 到 UART0（绕过 SBI），即可二分定位。

---

## 8. 风险排序与最困难部分

1. **SDIO/MMC（Stage 5，最高风险）**：`starfive,jh7110-mmc` 是 dw_mmc 变体，带 `sys_syscon` 电压/驱动
   配置与 `data-addr` 特殊性，且首次要正确初始化卡、CMD 序列、DMA。建议先用轮询 + 单块读跑通 superblock。
2. **GMAC（Stage 6，次高）**：stmmac 规模大，需 PHY（MDIO）、DMA 描述符环、时钟/复位（aoncrg/syscrg）。
   建议先最小 `phy::Device` 收包/发包跑通 ping。
3. **SMP hart 拓扑（Stage 7，中等）**：hart0=S7 无 MMU，不能启动；hart id 1..4 与数组索引的 off-by-one。
4. **legacy SBI console（Stage 1，低但最先踩）**：换 DBCN 或 UART0 直写即可。
5. **DDR 顶部保留区（Stage 2，中）**：frame allocator 不得覆盖 DTB/OpenSBI/U-Boot（§2.3）。

> 附：所有「地址/频率」均已从 StarFive 官方 DTS 直接核对；真机仍应先用
> `bdinfo / printenv / fdt / md` 二次确认，DTS 与固件版本冲突时以真机为准。
