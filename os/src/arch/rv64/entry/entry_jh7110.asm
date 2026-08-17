# os/src/arch/rv64/entry/entry_jh7110.asm
# VisionFive 2 (JH7110) 专用入口。
# 相比 entry.asm 多了一个 64-byte RISC-V Linux Image 头，使 U-Boot 的 `booti`
# 能识别并按其启动协议传 a0=hart id / a1=DTB；boot_pagetable 覆盖完整 4 GiB DDR。

    .section .text.entry
    .globl _start
_start:
    # RISC-V Linux Image header（64 bytes）。U-Boot `booti_setup` 校验：
    #   offset 0x38 magic == 0x05435352（"RSC\x05"），且 image_size(0x10) 非零；
    #   入口 relocated_addr = gd->ram_base + text_offset(0x08)。
    # text_offset=0x200000 使 relocated_addr = 0x40000000+0x200000 = 0x40200000
    # 正好等于本内核装载地址（kernel_addr_r），故不会发生重定位。
    .word 0x0400006f            # code0 (0x00): jal x0, +64（4 字节，禁止压缩，跳到 _start_kernel）
    .word 0                     # code1 (0x04)
    .dword 0x200000             # text_offset (0x08)
    .dword 0x02000000           # image_size (0x10)，非零即可（32 MiB 上限）
    .dword 0                    # flags (0x18)
    .word 2                     # version (0x20)
    .word 0                     # res1 (0x24)
    .dword 0                    # res2 (0x28)
    .dword 0                    # res3 (0x30)
    .word 0x05435352            # magic (0x38) = "RSC\x05"
    .word 0                     # res4 (0x3c)

_start_kernel:
    # OpenSBI/U-Boot 以 a0=hart id、a1=DTB 进入 S-mode。每个 hart 需要独立
    # early stack 后才能运行 Rust 代码。
    addi t1, a0, 1
    # 每份 early stack 为 256 KiB。virtio-net + smoltcp 接口初始化需要
    # 约 90 KiB 的启动期栈，原 64 KiB 会向下越界覆盖 .data（表现为
    # `physical_memory_end` / `PER_CPUS` 被随机值破坏）。
    slli t0, t1, 18
    la sp, boot_stack_lower_bound
    add sp, sp, t0

    la t0, boot_pagetable # PC 相对寻址，仍能定位到页表
    li t1, 8 << 60
    srli t0, t0, 12
    or t0, t0, t1
    csrw satp, t0 # 启用页表
    sfence.vma

    call enter_main

    .section .bss.stack
    .globl boot_stack_lower_bound
boot_stack_lower_bound: # 区别于 loader.rs 中的内核栈，在开始任务调度前使用该内核栈
    .space 4096 * 64 * 8
    .globl boot_stack_top
boot_stack_top:

    .section .data
    .align 12
boot_pagetable:
    # JH7110 / VisionFive 2：identity + 高半区各映射 MMIO 与完整 4 GiB DDR。
    # DTB 可能被 U-Boot 放在 RAM 顶部（如 control FDT 0xfffc56a0），因此早期页表
    # 必须覆盖整条 DDR 才能让 fdt_memory_end 解析 FDT；MMIO 区覆盖 UART0/CLINT/PLIC。
    .quad (0x0 << 10) | 0xcf     # VPN2=0:  0x00000000..0x40000000 (MMIO)
    .set boot_ppn, 0x40000
    .rept 4
    .quad (boot_ppn << 10) | 0xcf # VPN2=1..4: 0x40000000..0x140000000 (4 GiB DDR)
    .set boot_ppn, boot_ppn + 0x40000
    .endr
    .zero 8 * 251                # VPN2=5..255 未映射
    .quad (0x0 << 10) | 0xcf     # VPN2=256: 0xffffffc000000000.. (MMIO 高半)
    .set boot_ppn, 0x40000
    .rept 4
    .quad (boot_ppn << 10) | 0xcf # VPN2=257..260: DDR 高半
    .set boot_ppn, boot_ppn + 0x40000
    .endr
    .zero 8 * 251                # VPN2=261..511 未映射
