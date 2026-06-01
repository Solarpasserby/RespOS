# os/src/trap/trap.S

.altmacro
.macro SAVE_GP n
    sd x\n, \n*8(sp)
.endm
.macro LOAD_GP n
    ld x\n, \n*8(sp)
.endm
    # 将异常处理代码正常映射
    .section .text
    .globl __trap_from_user
    .globl __restore
    .globl __trap_from_kernel
    .align 2
__trap_from_user:
    # 有关特权级切换：在用户调用 ecall 特权级已处于 S 态，随后根据 stvec 切换到 __alltraps
    csrrw sp, sscratch, sp
    # 当前异常上下文存储于用户空间，故交换后 sscratch 存入用户栈指针
    # sp 得到的是内核栈指针，恢复上下文时在 sscratch 中放入这个地址
    #
    # 在栈上保存 TrapContext
    addi sp, sp, -34*8
    # 当前包括：x0-x31, sstatus, sepc
    # x0 恒零不保存，保存 x1(ra)
    sd x1, 1*8(sp)
    # x2(sp) 为栈指针暂时跳过，保存 x3
    sd x3, 3*8(sp)
    # x4(tp) 在用户态可被 libc 用作 TLS 指针，需要保存用户值。
    sd x4, 4*8(sp)
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
    # 从 sscratch 中读取用户栈指针的值并保存
    csrr t2, sscratch
    sd t2, 2*8(sp)
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
    # 设置异常相关状态寄存器
    la t0, __trap_from_user
    csrw stvec, t0
    # 恢复 sstatus, sepc
    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    csrw sstatus, t0
    csrw sepc, t1
    # 恢复通用寄存器
    ld x1, 1*8(sp)
    ld x3, 3*8(sp)
    ld x4, 4*8(sp)
    .set n, 5
    .rept 27
        LOAD_GP %n
        .set n, n+1
    .endr
    # 异常上下文出内核栈
    addi sp, sp, 34*8
    # sscratch 指向内核栈顶，sp 恢复为用户栈
    csrw sscratch, sp
    ld sp, -32*8(sp)
    # 设置 sstatus 的 SPP 位为 0，表示返回用户态
    csrc sstatus, 8 
    # 返回到用户态
    sret
    # 有关特权级切换：此时特权级才返回到 U 态，除部分寄存器改变，异常对用户程序而言是无感的

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
