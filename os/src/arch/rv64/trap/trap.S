# os/src/trap/trap.S

.altmacro
.option arch, +f
.option arch, +d
.macro SAVE_GP n
    sd x\n, \n*8(sp)
.endm
.macro LOAD_GP n
    ld x\n, \n*8(sp)
.endm
.macro SAVE_FP n
    fsd f\n, (\n+34)*8(sp)
.endm
.macro LOAD_FP n
    fld f\n, (\n+34)*8(sp)
.endm
    # 将异常处理代码正常映射
    .section .text
    .globl __trap_from_user
    .globl __restore
    .globl __trap_from_kernel
    .align 2
__trap_from_user:
    # sscratch 永远指向本 CPU 的 TrapScratch；不能再用它交换用户/内核
    # 栈，否则内核无法在 user trap 中可靠识别当前 CPU。
    #
    # 两次交换先保留 user t0/t1。TrapScratch 的 kernel_sp 位于偏移 0，
    # user_sp 临时槽位于偏移 8（见 arch::smp::TrapScratch）。
    csrrw t0, sscratch, t0
    csrrw t1, sscratch, t1
    sd sp, 8(t0)
    ld sp, 0(t0)

    # 在当前 task 的内核栈上保存 TrapContext。
    addi sp, sp, -68*8
    # 当前包括：x0-x31、sstatus、sepc、f0-f31、fcsr 和对齐填充
    # x0 恒零不保存，保存 x1(ra)
    sd x1, 1*8(sp)
    # x2(sp) 为栈指针暂时跳过，保存 x3
    sd x3, 3*8(sp)
    # x4(tp) 在用户态可被 libc 用作 TLS 指针，需要保存用户值。
    sd x4, 4*8(sp)
    # t0/t1 已在上面的 CSR 交换中使用，手工写回原始 user 值。
    sd t1, 5*8(sp)
    csrr t1, sscratch
    sd t1, 6*8(sp)
    # 保存 x7~x31
    .set n, 7
    .rept 25
        SAVE_GP %n
        .set n, n+1
    .endr
    # 保存 user sp，并从这里开始把 tp 固定为本 CPU 的 PerCpu 指针。
    ld t2, 8(t0)
    sd t2, 2*8(sp)
    mv tp, t0
    # 可以自由使用 t0/t1/t2，因为我们已经将其保存
    # 保存 sstatus, sepcs
    csrr t0, sstatus
    csrr t1, sepc
    sd t0, 32*8(sp)
    sd t1, 33*8(sp)
    # 保存浮点寄存器
    frcsr t0
    .set n, 0
    .rept 32
        SAVE_FP %n
        .set n, n+1
    .endr
    sd t0, 66*8(sp)
    # 用户页表映射内核后无需切换页表
    # csrw satp, t0
    # 向 trap_handler 传递参数
    mv a0, sp
    # 设置异常相关状态寄存器
    la t0, __trap_from_kernel
    csrw stvec, t0
    # 设置 sstatus 的 SPP 位为 1，表示进入内核态
    csrs sstatus, 8
    # 跳转到异常处理函数，由于异常处理部分的代码不是与内核代码一样使用恒等映射，所以不能使用call指令调用
    call trap_handler
    j __restore

__restore:
    # 内核调用时额外传入寄存器参数，主要用于维护
    
    # 用户页表映射内核后无需切换页表
    # csrw satp, a1
    # 恢复 sstatus, sepc
    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    # TrapContext 可来自 exec/sigreturn，不得在真正 sret 前恢复 SIE。
    # 否则 timer 会在 sscratch/stvec 过渡窗口中重入 user trap 入口。
    andi t0, t0, -3
    csrw sstatus, t0
    csrw sepc, t1
    # 恢复浮点寄存器
    ld t0, 66*8(sp)
    fscsr t0
    .set n, 0
    .rept 32
        LOAD_FP %n
        .set n, n+1
    .endr
    # tp 仍是本 CPU 的 PerCpu 指针，先保留它以恢复 sscratch；用户 tp
    # 会在最后按 TrapContext 恢复。
    mv t0, tp
    # 恢复通用寄存器（x5/x6 最后恢复，因为 t0 仍保存 scratch 指针）。
    ld x1, 1*8(sp)
    ld x3, 3*8(sp)
    ld x4, 4*8(sp)
    .set n, 7
    .rept 25
        LOAD_GP %n
        .set n, n+1
    .endr
    # sscratch 保持本 CPU scratch。只在批量上下文恢复完成后才切换
    # user trap vector，把 S 模式使用 user vector 的收尾窗口压到最小。
    csrw sscratch, t0
    la t0, __trap_from_user
    csrw stvec, t0
    ld t1, 6*8(sp)
    ld t0, 5*8(sp)
    ld sp, 2*8(sp)
    # 设置 sstatus 的 SPP 位为 0，表示返回用户态
    csrc sstatus, 8 
    # 返回到用户态
    sret
    # 有关特权级切换：此时特权级才返回到 U 态，除部分寄存器改变，异常对用户程序而言是无感的

    # stvec 的低两位编码 mode，kernel trap vector 本身必须 4-byte 对齐。
    # 前面的压缩指令可能使该标签仅有 2-byte 对齐，导致硬件把保留 mode
    # 的地址 WARL 改写为错误的入口。
    .align 2
__trap_from_kernel:
    # 保存数据到内核栈
    addi sp, sp, -34*8
    # 保存通用寄存器
    sd x1, 1*8(sp)
    # 先跳过栈指针寄存器 x2
    # 保存 x3~x31
    .set n, 3
    .rept 29
        SAVE_GP %n
        .set n, n+1
    .endr
    # 保存特权状态寄存器
    csrr t0, sstatus
    csrr t1, sepc
    sd t0, 32*8(sp)
    sd t1, 33*8(sp)
    # 保存栈指针
    sd sp, 2*8(sp)
    # 通过 a0 向 kernel_trap_handler 函数传参
    mv a0, sp
    call kernel_trap_handler
    # kernel_trap_handler 返回后, 恢复内核态的上下文
    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    # 恢复特权状态寄存器
    csrw sstatus, t0
    csrw sepc, t1
    # 恢复通用寄存器
    ld x1, 1*8(sp)
    # 恢复 x3~x31
    .set n, 3
    .rept 29
        LOAD_GP %n
        .set n, n+1
    .endr
    # 弹出上下文
    addi sp, sp, 34*8
    sret
