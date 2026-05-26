# os/src/arch/rv64/task/switch.S

.altmacro
.macro SAVE_SN n
    sd s\n, (\n+2)*8(sp)
.endm
.macro LOAD_SN n
    ld s\n, (\n+2)*8(a0)
.endm
    .section .text
    .global __switch
__switch:
    # 注意：
    # 现在在内核栈上保存任务上下文，发生切换时将上下文存入栈顶
    # 切换到其他任务时，根据栈指针从栈顶恢复任务上下文
    # fn __switch(
    #     next_task_kernel_stack: usize,
    #     current_task_ptr: usize,
    # )
    # 保存当前任务的上下文
    # 在内核栈上存储 `TaskContext` 数据
    addi sp, sp, -16*8
    sd ra, 0(sp)
    sd tp, 8(sp)
    .set n, 0
    .rept 12
        SAVE_SN %n
        .set n, n+1
    .endr
    # 保存 satp
    csrr t0, satp
    sd t0, 14*8(sp)
    # 更新当前任务的内核栈顶指针。
    # TaskControlBlock 的第一个字段是 KernelStack，KernelStack 的第一个字段是 top。
    sd sp, 0(a1)

    # 恢复下个任务的上下文
    # a0 为下个任务的栈指针
    ld ra, 0(a0)
    ld tp, 8(a0)
    .set n, 0
    .rept 12
        LOAD_SN %n
        .set n, n+1
    .endr
    # 恢复 stap
    ld t0, 14*8(a0)
    csrw satp, t0
    # 刷新 TLB
    sfence.vma
    # 移动栈指针，弹出 `TaskContext` 数据
    addi a0, a0, 16*8
    
    # 切换内核栈指针
    mv sp, a0
    # 返回
    ret
