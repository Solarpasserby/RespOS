# os/src/trap/trap.S

.altmacro
.macro SAVE_GP n
    sd x\n, \n*8(sp)
.endm
.macro LOAD_GP n
    ld x\n, \n*8(sp)
.endm
    # 将异常处理代码作为跳板映射到代码段，跳板位于虚拟地址最高页
    # 该地址会同时被内核和用户程序映射，因而在触发异常发生特权级切换后，仍能正常调用异常处理
    .section .text.trampoline
    .globl __alltraps
    .globl __restore
    .align 2
__alltraps:
    # 有关特权级切换：在用户调用 ecall 特权级已处于 S 态，随后根据 stvec 切换到 __alltraps
    csrrw sp, sscratch, sp
    # 当前异常上下文存储于用户空间，故交换后 sscratch 存入用户栈指针
    # sp 得到的是用户空间次高地址（异常上下文段），恢复上下文时在 sscratch 中放入这个地址
    #
    # 随后在用户空间次高地址，保存异常上下文
    # 需要保存的值与 TrapContext 的定义有关
    # 当前包括：x0-x31, sstatus, sepc
    addi sp, sp, -34*8
    # x0 恒零不保存，保存 x1(ra)
    sd x1, 1*8(sp)
    # x2(sp) 为栈指针暂时跳过，保存 x3
    sd x3, 3*8(sp)
    # 跳过 x4(tp)，暂时没有使用其功能
    # 保存 x5~x31
    .set n, 5
    .rept 27
        SAVE_GP %n
        .set n, n+1
    .endr
    # 可以自由使用 t0/t1/t2，因为我们已经将其保存
    # 保存 sstatus, sepcs
    csrr t0, sstatus
    csrr t1, sepc
    sd t0, 32*8(sp)
    sd t1, 33*8(sp)
    # 从 sscratch 中读取用户栈指针的值，即 x2 的旧值，保存
    csrr t2, sscratch
    sd t2, 2*8(sp)
    # 在异常上下文中提前将 kernel_satp, trap_handler, kernel_sp 数据写入
    # 读取以进入内核地址空间处理异常
    # 读取 kernel_satp
    ld t0, 34*8(sp)
    # 读取 trap_handler
    ld t1, 36*8(sp)
    # 读取 kernel_sp 
    ld sp, 35*8(sp)
    # 切换到内核空间
    csrw satp, t0
    # 屏障
    sfence.vma
    # 跳转到异常处理函数，由于异常处理部分的代码不是与内核代码一样使用恒等映射，所以不能使用call指令调用
    jr t1

__restore:
    # 内核调用时额外传入寄存器参数，主要用于维护
    # a0: 用户程序次高地址（异常上下文）
    # a1: 用户地址空间 token
    
    # 切换用户空间
    csrw satp, a1
    sfence.vma
    # 写入 sscratch 用户程序次高地址指针（异常上下文）
    csrw sscratch, a0
    mv sp, a0
    # 现在栈指针切换到异常上下文，开始恢复用户程序上下文
    # 恢复 sstatus, sepc
    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    csrw sstatus, t0
    csrw sepc, t1
    # 恢复通用寄存器
    ld x1, 1*8(sp)
    ld x3, 3*8(sp)
    .set n, 5
    .rept 27
        LOAD_GP %n
        .set n, n+1
    .endr
    # 切换到用户栈
    ld sp, 2*8(sp)
    sret
    # 有关特权级切换：此时特权级才返回到 U 态，除部分寄存器改变，异常对用户程序而言是无感的
