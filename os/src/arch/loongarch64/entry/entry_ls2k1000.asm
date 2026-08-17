# os/src/arch/loongarch64/entry/entry_ls2k1000.asm
#
# 2K1000LA 真机（U-Boot `go`）启动入口。
#
# 与 QEMU `-kernel`（直接地址模式 DA=1）不同，U-Boot `go 0x9000000000200000` 时
# CPU 已处于分页模式（U-Boot 的 DMW 窗口），内核代码运行在 0x9000... cached 窗口内。
# 这里参照 StarryOS someboot：DMW0 设为 0x8000 uncached 窗口供 MMIO 访问，
# DMW1 保持 0x9000 cached 窗口供代码/数据访问。Stage 1 单核，无 secondary 入口。

    .section .text.entry
    .globl _start_phys
    .globl _start

    .equ CSR_DMW0, 0x180
    .equ CSR_DMW1, 0x181

_start_phys:
_start:
    # DMW1 = 当前执行段窗口（cached），VSEG = PC[63:48]，MAT=1(CC)，PLV0。
    pcaddi   $t0, 0x0
    srli.d   $t0, $t0, 0x30
    slli.d   $t0, $t0, 0x30
    addi.d   $t0, $t0, 0x11
    csrwr    $t0, CSR_DMW1

    # DMW0 = VSEG=0x8000（uncached IO 窗口），MAT=0(SUC)，PLV0。
    # 2K1000LA 真机 MMIO 必须 uncached；early UART 经 0x8000... 窗口访问。
    li.d     $t0, 0x8000000000000001
    csrwr    $t0, CSR_DMW0

    # CPU0 使用第一个 64 KiB early stack（按 CPUNUM 选栈）。
    csrrd    $t1, 0x20
    andi     $t1, $t1, 0x3ff
    addi.d   $t1, $t1, 1
    slli.d   $t1, $t1, 16
    la.local $sp, boot_stack_lower_bound
    add.d    $sp, $sp, $t1
    bl       enter_main

    .section .bss.stack
    .globl boot_stack_lower_bound
boot_stack_lower_bound:
    .space 4096 * 16 * 12
    .globl boot_stack_top
boot_stack_top:
