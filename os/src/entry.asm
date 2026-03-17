# os/src/entry.S

    .section .text.entry
    .globl _start
_start:
    la sp, boot_stack_top
    call rust_main

    .section .bss.stack
    .globl boot_stack_lower_bound
boot_stack_lower_bound: # 区别于 loder.rs 中的内核栈，在开始任务调度前使用该内核栈
    .space 4096 * 16
    .globl boot_stack_top
boot_stack_top: