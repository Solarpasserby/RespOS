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
    # 映射两个大页，大小均为 1GB
    # 0x0000_0000_8000_0000 -> 0x0000_0000_8000_0000 直接映射，用于 frame_allocator
    # 0xffff_fc00_8000_0000 -> 0x0000_0000_8000_0000 线性映射，用于内核的虚拟地址
    .quad 0
    .quad 0
    .quad (0x80000 << 10) | 0xcf # VRWXAD
    .zero 8 * 255
    .quad (0x80000 << 10) | 0xcf # VRWXAD
    .zero 8 * 253
