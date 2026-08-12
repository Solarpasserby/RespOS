    .section .text.__rfill
    .globl __rfill

    .equ CSR_TLBRSAVE, 0x8b
    .equ CSR_KSAVE2,   0x32
    .equ CSR_PGD,      0x1b
    .equ CSR_TLBRELO0, 0x8c
    .equ CSR_TLBRELO1, 0x8d

__rfill:
    csrwr   $t0, CSR_TLBRSAVE
    csrwr   $t1, CSR_KSAVE2

    csrrd   $t0, CSR_PGD
    lddir   $t0, $t0, 3
    beqz    $t0, construct_invalid
    addi.d  $t0, $t0, -1

    lddir   $t0, $t0, 1
    beqz    $t0, construct_invalid
    andi    $t1, $t0, 0x40
    bnez    $t1, load_pte
    addi.d  $t0, $t0, -1

load_pte:
    ldpte   $t0, 0
    ldpte   $t0, 1
    tlbfill
    b       restore_t0

construct_invalid:
    csrwr   $zero, CSR_TLBRELO0
    csrwr   $zero, CSR_TLBRELO1
    tlbfill

restore_t0:
    csrrd   $t1, CSR_KSAVE2
    csrrd   $t0, CSR_TLBRSAVE
    ertn
