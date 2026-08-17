# os/src/arch/rv64/entry/entry_jh7110.asm
# VisionFive 2 (JH7110) 专用入口。除 boot_pagetable 外与 entry.asm 一致。

    .section .text.entry
    .globl _start
_start:
    # OpenSBI/U-Boot 以 a0=hart id、a1=DTB 进入 S-mode。每个 hart 需要独立
    # early stack 后才能运行 Rust 代码。
    addi t1, a0, 1
    # 每份 early stack 为 256 KiB。virtio-net + smoltcp 接口初始化需要
    # 约 90 KiB 的启动期栈，原 64 KiB 会向下越界覆盖 .data（表现为
    # `physical_memory_end` / `PER_CPUS` 被随机值破坏）。
    slli t0, t1, 18
    la sp, boot_stack_lower_bound
    add sp, sp, t0

    la t0, boot_pagetable # PC 相对寻址，仍能定位到页表
    li t1, 8 << 60
    srli t0, t0, 12
    or t0, t0, t1
    csrw satp, t0 # 启用页表
    sfence.vma

    call enter_main

    .section .bss.stack
    .globl boot_stack_lower_bound
boot_stack_lower_bound: # 区别于 loader.rs 中的内核栈，在开始任务调度前使用该内核栈
    .space 4096 * 64 * 8
    .globl boot_stack_top
boot_stack_top:

    .section .data
    .align 12
boot_pagetable:
    # JH7110 / VisionFive 2：identity + 高半区各映射 MMIO 与首 1 GiB DDR。
    # 覆盖 kernel(0x40200000)/DTB(0x46000000)/heap，以及 UART0/CLINT/PLIC
    # 所在的 MMIO 区（0x10000000..）。其余 DDR 由 mm::init 的正式 direct map
    # 按 FDT 实际末址补齐。
    .quad (0x0 << 10) | 0xcf     # VPN2=0:  0x00000000..0x40000000 (MMIO)
    .quad (0x40000 << 10) | 0xcf # VPN2=1:  0x40000000..0x80000000 (首 1 GiB DDR)
    .zero 8 * 254                # VPN2=2..255 未映射
    .quad (0x0 << 10) | 0xcf     # VPN2=256: 0xffffffc000000000.. (MMIO 高半)
    .quad (0x40000 << 10) | 0xcf # VPN2=257: 0xffffffc040000000.. (首 1 GiB DDR 高半)
    .zero 8 * 254                # VPN2=258..511 未映射
