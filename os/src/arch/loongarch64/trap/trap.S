# os/src/arch/loongarch64/trap/trap.S

# LoongArch 异常处理汇编
#
# 寄存器别名（ABI）:
#   $r0=zero, $r1=ra, $r2=tp, $r3=sp, $r4=a0, $r5=a1
#   $r12-$r20=t0-t8 (临时寄存器)
#   $r21=reserved/u0, $r22=fp/s9, $r23-$r31=s0-s8
#
# LoongArch CSR:
#   KSAVE0-7 (0x30-0x37): 内核可用暂存寄存器
#   PRMD   (0x1): 异常前模式（PPLV=bits[1:0], PIE=bit2）
#   ERA    (0x6): 异常返回地址
#   ESTAT  (0x5): 异常状态（Ecode=bits[15:0], IS=bit11）
#   BADV   (0x7): 触发异常的坏地址
#   EENTRY (0xC): 异常入口地址
#   ECFG   (0x4): 异常配置
#   CRMD   (0x0): 当前模式
#
#   KSAVE0 (0x30): trap entry/restore 时交换用户 sp 和内核 sp
#   KSAVE1 (0x31): 保存内核 tp，进入 Rust trap_handler 前恢复
#
# 上下文保存布局 (每个slot 8字节):
#   sp + 0..31*8:   x1..x31 (跳过x0=zero)
#   实际按 x0..x31 全部保存方便索引:
#     x0@sp+0, x1@sp+8, ..., x31@sp+248
#     prmd@sp+256 (32*8)
#     era@sp+264  (33*8)

    .section .text
    .globl __trap_from_user
    .globl __restore
    # LoongArch EENTRY 保存普通异常入口基址，入口必须按页边界对齐。
    # release 链接布局变化时，如果这里只有 16 字节对齐，硬件可能跳到
    # 截断后的错误地址，表现为用户态异常/中断后卡死。
    .align 12
__trap_from_user:
    # 步骤1: 用 KSAVE0 保存 sp
    csrwr       $sp, 0x30       # KSAVE0 <-> sp, 现在 sp=原KS值, KSAVE0=用户sp
    # 现在 $sp 指向内核栈（由之前恢复流程保留在 KSAVE0 中的值）
    # 如果没有之前的值，说明是首次trap，KSAVE0需要预先初始化

    # 步骤2: 在内核栈上分配 TrapContext 空间 (32*8 + 2*8 = 272 bytes)
    addi.d      $sp, $sp, -272

    # 步骤3: 保存通用寄存器
    # x0 恒为0不需要保存，但为了对齐我们保存0
    st.d        $zero, $sp, 0       # x0
    # x1(ra), x2(tp)
    st.d        $ra, $sp, 8        # x1
    st.d        $tp, $sp, 16       # x2
    st.d        $a0, $sp, 32       # x4
    st.d        $a1, $sp, 40       # x5
    st.d        $a2, $sp, 48       # x6
    st.d        $a3, $sp, 56       # x7
    st.d        $a4, $sp, 64       # x8
    st.d        $a5, $sp, 72       # x9
    st.d        $a6, $sp, 80       # x10
    st.d        $a7, $sp, 88       # x11
    st.d        $t0, $sp, 96       # x12
    st.d        $t1, $sp, 104      # x13
    st.d        $t2, $sp, 112      # x14
    st.d        $t3, $sp, 120      # x15
    st.d        $t4, $sp, 128      # x16
    st.d        $t5, $sp, 136      # x17
    st.d        $t6, $sp, 144      # x18
    st.d        $t7, $sp, 152      # x19
    st.d        $t8, $sp, 160      # x20
    st.d        $r21, $sp, 168     # x21
    st.d        $r22, $sp, 176     # x22 = fp/s9
    st.d        $s0, $sp, 184      # x23
    st.d        $s1, $sp, 192      # x24
    st.d        $s2, $sp, 200      # x25
    st.d        $s3, $sp, 208      # x26
    st.d        $s4, $sp, 216      # x27
    st.d        $s5, $sp, 224      # x28
    st.d        $s6, $sp, 232      # x29
    st.d        $s7, $sp, 240      # x30
    st.d        $s8, $sp, 248      # x31

    # 从 KSAVE0 读回用户 sp。这里必须在保存 t0 原值之后做，
    # 否则会破坏用户态 x12/t0。
    csrrd       $t0, 0x30
    st.d        $t0, $sp, 24       # x3 = 用户sp

    # 步骤4: 保存 PRMD 和 ERA
    csrrd       $t0, 0x1           # PRMD
    st.d        $t0, $sp, 256      # offset 32*8
    csrrd       $t0, 0x6           # ERA
    st.d        $t0, $sp, 264      # offset 33*8

    # 用户态可以自由使用 tp。进入 Rust 内核前恢复内核 tp，避免
    # trap_handler 在用户 tp 下运行。
    csrrd       $tp, 0x31

    # 步骤5: 切换到内核上下文
    # 设置 CRMD: PLV=0(kernel), IE=0(disable interrupts)
    csrrd       $t0, 0x0           # CRMD
    bstrins.d   $t0, $zero, 1, 0   # PLV=0
    bstrins.d   $t0, $zero, 2, 2   # IE=0
    csrwr       $t0, 0x0

    # 处理用户 trap 期间，内核态异常应进入 kernel trap 入口。
    la.local    $t0, __trap_from_kernel
    csrwr       $t0, 0xc

    # 步骤6: 调用 Rust trap_handler
    # 参数: a0 = &mut TrapContext (sp)
    ori         $a0, $sp, 0
    bl          trap_handler

    # 步骤7: 恢复上下文并返回用户态
    # 直接落入 __restore

__restore:
    # 恢复 PRMD 和 ERA
    ld.d        $t0, $sp, 256
    # 返回用户态时必须保持 PPLV=3 且 PIE=1。压力场景下如果沿用
    # 某次 trap 保存下来的 PIE=0，用户态 CPU-bound 任务不会再被
    # timer interrupt 抢占。显式覆盖 PRMD[2:0]，避免保留异常低位。
    ori         $t1, $zero, 0x7
    bstrins.d   $t0, $t1, 2, 0
    csrwr       $t0, 0x1           # PRMD
    ld.d        $t0, $sp, 264
    csrwr       $t0, 0x6           # ERA

    # 返回用户态前，把普通异常入口切到用户 trap 保存路径。
    la.local    $t0, __trap_from_user
    csrwr       $t0, 0xc

    # 即将恢复用户寄存器，先把当前内核 tp 留到 KSAVE1，供下次 trap entry 使用。
    ori         $t0, $tp, 0
    csrwr       $t0, 0x31

    # 先保存下次 trap entry 要使用的内核栈顶。这个步骤会使用 $t0，
    # 必须发生在恢复用户 $t0 之前，否则异步中断返回会破坏用户寄存器。
    addi.d      $t0, $sp, 272      # t0 = kernel stack top
    csrwr       $t0, 0x30          # KSAVE0 = kernel sp (下次 trap entry 时 csrwr 恢复)

    # 恢复通用寄存器
    ld.d        $ra, $sp, 8
    ld.d        $tp, $sp, 16
    ld.d        $a0, $sp, 32
    ld.d        $a1, $sp, 40
    ld.d        $a2, $sp, 48
    ld.d        $a3, $sp, 56
    ld.d        $a4, $sp, 64
    ld.d        $a5, $sp, 72
    ld.d        $a6, $sp, 80
    ld.d        $a7, $sp, 88
    ld.d        $t0, $sp, 96
    ld.d        $t1, $sp, 104
    ld.d        $t2, $sp, 112
    ld.d        $t3, $sp, 120
    ld.d        $t4, $sp, 128
    ld.d        $t5, $sp, 136
    ld.d        $t6, $sp, 144
    ld.d        $t7, $sp, 152
    ld.d        $t8, $sp, 160
    ld.d        $r21, $sp, 168
    ld.d        $r22, $sp, 176
    ld.d        $s0, $sp, 184
    ld.d        $s1, $sp, 192
    ld.d        $s2, $sp, 200
    ld.d        $s3, $sp, 208
    ld.d        $s4, $sp, 216
    ld.d        $s5, $sp, 224
    ld.d        $s6, $sp, 232
    ld.d        $s7, $sp, 240
    ld.d        $s8, $sp, 248

    # 恢复用户 sp (从 x3 的位置，offset = 24)
    ld.d        $sp, $sp, 24       # sp = user sp
    # ERTN: 异常返回 (恢复 PLV 和 PC, 根据 PRMD 返回到用户态)
    ertn

    .globl __trap_from_kernel
    .align 12
__trap_from_kernel:
    # 在内核栈上分配 TrapContext 空间 (34 × 8 = 272 bytes)
    addi.d      $sp, $sp, -272

    # 保存通用寄存器（与 __trap_from_user 布局一致）
    st.d        $zero, $sp, 0       # x0
    st.d        $ra, $sp, 8        # x1
    st.d        $tp, $sp, 16       # x2
    # 先保存 t0 原值，后续将其用作临时寄存器
    st.d        $t0, $sp, 96       # x12 (t0) — 必须在用作临时寄存器前保存
    # 用 t0 计算进入前的 sp 并保存为 x3
    addi.d      $t0, $sp, 272
    st.d        $t0, $sp, 24       # x3 = 进入前的 sp
    st.d        $a0, $sp, 32       # x4
    st.d        $a1, $sp, 40       # x5
    st.d        $a2, $sp, 48       # x6
    st.d        $a3, $sp, 56       # x7
    st.d        $a4, $sp, 64       # x8
    st.d        $a5, $sp, 72       # x9
    st.d        $a6, $sp, 80       # x10
    st.d        $a7, $sp, 88       # x11
    st.d        $t1, $sp, 104      # x13
    st.d        $t2, $sp, 112      # x14
    st.d        $t3, $sp, 120      # x15
    st.d        $t4, $sp, 128      # x16
    st.d        $t5, $sp, 136      # x17
    st.d        $t6, $sp, 144      # x18
    st.d        $t7, $sp, 152      # x19
    st.d        $t8, $sp, 160      # x20
    st.d        $r21, $sp, 168     # x21
    st.d        $r22, $sp, 176     # x22
    st.d        $s0, $sp, 184      # x23
    st.d        $s1, $sp, 192      # x24
    st.d        $s2, $sp, 200      # x25
    st.d        $s3, $sp, 208      # x26
    st.d        $s4, $sp, 216      # x27
    st.d        $s5, $sp, 224      # x28
    st.d        $s6, $sp, 232      # x29
    st.d        $s7, $sp, 240      # x30
    st.d        $s8, $sp, 248      # x31

    # 保存 PRMD 和 ERA（t0 原值已保存，此处可自由使用）
    csrrd       $t0, 0x1           # PRMD
    st.d        $t0, $sp, 256      # offset 32*8
    csrrd       $t0, 0x6           # ERA
    st.d        $t0, $sp, 264      # offset 33*8

    # 确保处于内核模式
    csrrd       $t0, 0x0           # CRMD
    bstrins.d   $t0, $zero, 1, 0   # PLV=0
    bstrins.d   $t0, $zero, 2, 2   # IE=0
    csrwr       $t0, 0x0

    # 调用 trap_from_kernel(cx: &mut TrapContext)
    ori         $a0, $sp, 0
    bl          trap_from_kernel

    # 恢复 PRMD 和 ERA
    ld.d        $t0, $sp, 256
    csrwr       $t0, 0x1           # PRMD
    ld.d        $t0, $sp, 264
    csrwr       $t0, 0x6           # ERA

    # 恢复通用寄存器（x3/sp 通过后续 addi.d 恢复，此处不加载）
    ld.d        $ra, $sp, 8
    ld.d        $tp, $sp, 16
    # skip x3 @ offset 24
    ld.d        $a0, $sp, 32
    ld.d        $a1, $sp, 40
    ld.d        $a2, $sp, 48
    ld.d        $a3, $sp, 56
    ld.d        $a4, $sp, 64
    ld.d        $a5, $sp, 72
    ld.d        $a6, $sp, 80
    ld.d        $a7, $sp, 88
    ld.d        $t0, $sp, 96
    ld.d        $t1, $sp, 104
    ld.d        $t2, $sp, 112
    ld.d        $t3, $sp, 120
    ld.d        $t4, $sp, 128
    ld.d        $t5, $sp, 136
    ld.d        $t6, $sp, 144
    ld.d        $t7, $sp, 152
    ld.d        $t8, $sp, 160
    ld.d        $r21, $sp, 168
    ld.d        $r22, $sp, 176
    ld.d        $s0, $sp, 184
    ld.d        $s1, $sp, 192
    ld.d        $s2, $sp, 200
    ld.d        $s3, $sp, 208
    ld.d        $s4, $sp, 216
    ld.d        $s5, $sp, 224
    ld.d        $s6, $sp, 232
    ld.d        $s7, $sp, 240
    ld.d        $s8, $sp, 248

    # 弹出上下文帧，恢复内核栈指针
    addi.d      $sp, $sp, 272
    ertn
