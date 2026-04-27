# os/src/task/switch.S

.altmacro
.macro SAVE_SN n
    sd s\n, (\n+2)*8(a0)
.endm
.macro LOAD_SN n
    ld s\n, (\n+2)*8(a1)
.endm
    .section .text
    .global __switch
__switch:
    # 注意：
    # 区别于异常处理汇编 (trap.S) 中将数据保存栈上
    # 此处任务上下文作为 TaskControlBlock 或是 Processor 的一个字段
    # 从传参的角度也可以发现——这里传入的是裸指针
    # 实际上对于 TaskContext 的读写直接作用于包含它的变量中
    # fn __switch(
    # current_task_context_ptr (a0): *mut TaskContext
    # next_task_context_ptr (a1): *const TaskContext
    # )
    # 保存当前任务的上下文
    sd sp, 8(a0)
    sd ra, 0(a0)
    .set n, 0
    .rept 12
        SAVE_SN %n
        .set n, n+1
    .endr
    # 保存 satp
    csrr t0, satp
    sd t0, 14*8(a0)

    # 恢复下个任务的上下文
    ld ra, 0(a1)
    .set n, 0
    .rept 12
        LOAD_SN %n
        .set n, n+1
    .endr
    # 恢复 stap
    ld t0, 14*8(a1)
    csrw satp, t0
    # 刷新 TLB
    sfence.vma
    
    # 切换内核栈指针
    ld sp, 8(a1)
    # 返回
    ret
