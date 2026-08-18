# os/src/arch/loongarch64/entry/entry_ls2k1000.asm
#
# 2K1000LA 真机（U-Boot `go`）启动入口。
#
# 与 QEMU `-kernel`（直接地址模式 DA=1）不同，U-Boot `go 0x9000000000200000` 时
# CPU 已处于分页模式（U-Boot 的 DMW 窗口），内核代码运行在 0x9000... cached 窗口内。
# 这里参照 StarryOS someboot：DMW0 设为 0x8000 uncached 窗口供 MMIO 访问，
# DMW1 保持 0x9000 cached 窗口供代码/数据访问。

    .section .text.entry
    .globl _start_phys
    .globl _start
    .globl _start_secondary_phys

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
    # 即 0x8000_0000_0000_0001。用 ori + lu52i.d 构造，避免 li.d 伪指令。
    ori      $t0, $zero, 0x1       # t0 = 1 (PLV0)
    lu52i.d  $t0, $t0, -0x800       # t0[63:52] = 0x800 → 0x8000_0000_0000_0001
    csrwr    $t0, CSR_DMW0

    # CPU0 使用第一个 64 KiB early stack（按 CPUNUM 选栈）。
    csrrd    $t1, 0x20
    andi     $t1, $t1, 0x3ff
    addi.d   $t1, $t1, 1
    slli.d   $t1, $t1, 16
    la.local $sp, boot_stack_lower_bound
    add.d    $sp, $sp, $t1
    bl       enter_main

_start_secondary_phys:
    # 2K1000LA 次核入口（Stage 7 SMP 前未用，保留符号避免链接期缺失）。
    pcaddi   $t0, 0x0
    srli.d   $t0, $t0, 0x30
    slli.d   $t0, $t0, 0x30
    addi.d   $t0, $t0, 0x11
    csrwr    $t0, CSR_DMW1
    ori      $t0, $zero, 0x1
    lu52i.d  $t0, $t0, -0x800
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
