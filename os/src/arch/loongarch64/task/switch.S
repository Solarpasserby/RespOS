# os/src/arch/loongarch64/task/switch.S

# LoongArch 任务上下文切换
#
# 函数签名: fn __switch(next_task_kstack_ptr: usize)
#   参数: a0 = next_task_kstack_ptr (下个任务的内核栈指针)
#
# TaskContext 内存布局 (与 task/context.rs 一致):
#   sp + 0*8:   ra (r1)
#   sp + 1*8:   tp (r2)
#   sp + 2*8:   s0 (r23)
#   sp + 3*8:   s1 (r24)
#   sp + 4*8:   s2 (r25)
#   sp + 5*8:   s3 (r26)
#   sp + 6*8:   s4 (r27)
#   sp + 7*8:   s5 (r28)
#   sp + 8*8:   s6 (r29)
#   sp + 9*8:   s7 (r30)
#   sp + 10*8:  s8 (r31)
#   sp + 11*8:  padding (s[9])
#   sp + 12*8:  padding (s[10])
#   sp + 13*8:  padding (s[11])
#   sp + 14*8:  PGDH (mmu_token)
#   sp + 15*8:  _padding

    .section .text
    .global __switch
__switch:
    # 在内核栈上分配 TaskContext 空间 (16 * 8 = 128 bytes)
    addi.d      $sp, $sp, -128

    # 保存 callee-saved 寄存器
    st.d        $ra, $sp, 0      # ra
    st.d        $tp, $sp, 8      # tp
    st.d        $r23, $sp, 16    # s0
    st.d        $r24, $sp, 24    # s1
    st.d        $r25, $sp, 32    # s2
    st.d        $r26, $sp, 40    # s3
    st.d        $r27, $sp, 48    # s4
    st.d        $r28, $sp, 56    # s5
    st.d        $r29, $sp, 64    # s6
    st.d        $r30, $sp, 72    # s7
    st.d        $r31, $sp, 80    # s8
    # s[9..11] = padding, 保持为0
    st.d        $zero, $sp, 88
    st.d        $zero, $sp, 96
    st.d        $zero, $sp, 104

    # 保存 PGDH (根页表)
    csrrd       $t0, 0x1b        # PGDH
    st.d        $t0, $sp, 112

    # 更新当前任务的内核栈指针 (存入 tp 指向的第一个字段)
    st.d        $sp, $tp, 0

    # 恢复下个任务的上下文
    # a0 = next_task_kstack_ptr
    ld.d        $ra, $a0, 0
    ld.d        $tp, $a0, 8
    ld.d        $r23, $a0, 16   # s0
    ld.d        $r24, $a0, 24   # s1
    ld.d        $r25, $a0, 32   # s2
    ld.d        $r26, $a0, 40   # s3
    ld.d        $r27, $a0, 48   # s4
    ld.d        $r28, $a0, 56   # s5
    ld.d        $r29, $a0, 64   # s6
    ld.d        $r30, $a0, 72   # s7
    ld.d        $r31, $a0, 80   # s8

    # 恢复 PGDH 并刷新 TLB
    ld.d        $t0, $a0, 112
    csrwr       $t0, 0x1b        # PGDH
    invtlb      0, $zero, $zero  # TLB 全刷新

    # 弹出 TaskContext，切换到新栈
    addi.d      $a0, $a0, 128
    ori         $sp, $a0, 0

    # 返回
    jirl        $zero, $ra, 0
