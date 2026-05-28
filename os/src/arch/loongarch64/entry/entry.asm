# os/src/arch/loongarch64/entry/entry.asm
#
# LoongArch 启动入口
#
# QEMU virt 的启动代码通过 -kernel 将内核 ELF 加载到低 RAM。
# 链接地址 = 加载地址 = 0x00200000，初始阶段 phys == virt。
# mm::init() 之后才建立页表和 DMW，切换到分页模式。

    .section .text.entry
    .globl _start
_start:
    la.local    $sp, boot_stack_top
    bl          enter_main

    .section .bss.stack
    .globl boot_stack_lower_bound
boot_stack_lower_bound:
    .space 4096 * 16
    .globl boot_stack_top
boot_stack_top:
