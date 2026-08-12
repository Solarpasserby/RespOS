# os/src/arch/loongarch64/entry/entry.asm
#
# LoongArch 启动入口
#
# QEMU virt 的 -kernel 将内核 ELF 加载到 0x00200000。
# 链接地址位于高半区；进入分页前，CPU 仍处于直接地址模式。
# mm::init() 会建立临时高地址页表，然后切换到分页模式。

    .section .text.entry
    .globl _start_phys
    .globl _start
    .globl _start_secondary_phys

    .equ CSR_DMW0, 0x180
    .equ CSR_DMW1, 0x181

_start_phys:
_start:
    # 参考固件启动路径保留当前执行段的 DMW 配置，随后内核会关闭 DMW1。
    pcaddi   $t0, 0x0
    srli.d   $t0, $t0, 0x30
    slli.d   $t0, $t0, 0x30
    addi.d   $t0, $t0, 0x11       # MAT=1(CC), PLV0=1
    csrwr    $t0, CSR_DMW1

    # 早期页表构建需要访问低物理页帧，额外保留低地址 DMW。
    addi.d   $t0, $zero, 0x11      # VSEG=0, PSEG=0, MAT=1(CC), PLV0=1
    csrwr    $t0, CSR_DMW0

    # CPU0 使用第一个 64 KiB early stack。使用 CPUNUM 而不是共享变量，
    # 这样 secondary 从 mailbox 进入时也能无锁选择自己的栈。
    csrrd    $t1, 0x20
    andi     $t1, $t1, 0x3ff
    addi.d   $t1, $t1, 1
    slli.d   $t1, $t1, 16
    la.local $sp, boot_stack_lower_bound
    add.d    $sp, $sp, $t1
    bl       enter_main

_start_secondary_phys:
    # QEMU auxiliary boot ROM 关闭 IPI 后跳到本物理入口。每个 hart 都要
    # 独立建立 DMW 与 early stack，不能复用 boot hart 的执行现场。
    pcaddi   $t0, 0x0
    srli.d   $t0, $t0, 0x30
    slli.d   $t0, $t0, 0x30
    addi.d   $t0, $t0, 0x11
    csrwr    $t0, CSR_DMW1
    addi.d   $t0, $zero, 0x11
    csrwr    $t0, CSR_DMW0

    csrrd    $t1, 0x20
    andi     $t1, $t1, 0x3ff
    addi.d   $t1, $t1, 1
    slli.d   $t1, $t1, 16
    la.local $sp, boot_stack_lower_bound
    add.d    $sp, $sp, $t1
    bl       enter_secondary

    .section .bss.stack
    .globl boot_stack_lower_bound
boot_stack_lower_bound:
    .space 4096 * 16 * 12
    .globl boot_stack_top
boot_stack_top:
