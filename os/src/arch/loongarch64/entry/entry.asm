# os/src/arch/loongarch64/entry/entry.asm
#
# LoongArch 启动入口
#
# 初始阶段运行在物理地址模式 (CRMD.DA=1，复位默认值)。
# 链接地址 = 加载地址 = 0x1c000000，phys == virt。
# mm::init() 之后才建立页表和 DMW，切换到虚拟地址模式。

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
