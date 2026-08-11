# os/src/arch/rv64/entry/entry.S

    .section .text.entry
    .globl _start
_start:
    # OpenSBI enters S-mode with a0=hart id and a1=opaque.  Every hart needs
    # a private early stack before it can run Rust code.
    addi t1, a0, 1
    # 每份 early stack 正好为 64 KiB；release RV64 target 不要求 M/Zmmul，
    # 用基础整数左移而非 mul，以保持最小 ISA 配置可启动。
    slli t0, t1, 16
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
    .space 4096 * 16 * 8
    .globl boot_stack_top
boot_stack_top:

    .section .data
    .align 12
boot_pagetable:
    # 先映射 QEMU virt 最多 16 GiB RAM 的 1 GiB 叶页，使 boot hart 能读取
    # OpenSBI 放在 RAM 顶部的 FDT。实际可分配上限仍由 FDT 决定，所以
    # 小内存配置不会访问未安装的物理内存。
    .zero 8 * 2
    .set boot_ppn, 0x80000
    .rept 16
    .quad (boot_ppn << 10) | 0xcf # VRWXAD
    .set boot_ppn, boot_ppn + 0x40000
    .endr
    .zero 8 * 240
    .set boot_ppn, 0x80000
    .rept 16
    .quad (boot_ppn << 10) | 0xcf # VRWXAD
    .set boot_ppn, boot_ppn + 0x40000
    .endr
    .zero 8 * 238
