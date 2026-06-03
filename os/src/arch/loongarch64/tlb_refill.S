    .section .text.__rfill
    .globl __rfill

    .equ CSR_TLBRSAVE, 0x8b
    .equ CSR_PGD,      0x1b
    .equ CSR_TLBRELO0, 0x8c
    .equ CSR_TLBRELO1, 0x8d
    .equ CSR_TLBREHI,  0x8e
    .equ CSR_TLBEHI,   0x11

__rfill:
    csrwr   $t0, CSR_TLBRSAVE

    csrrd   $t0, CSR_PGD
    lddir   $t0, $t0, 2
    beqz    $t0, construct_invalid
    addi.d  $t0, $t0, -1

    lddir   $t0, $t0, 1
    beqz    $t0, construct_invalid
    addi.d  $t0, $t0, -1

    ldpte   $t0, 0
    ldpte   $t0, 1
    tlbfill
    b       restore_t0

construct_invalid:
    csrrd   $t0, CSR_TLBREHI
    ori     $t0, $t0, 0xC
    csrwr   $t0, CSR_TLBEHI

    rotri.d $t0, $t0, 61
    ori     $t0, $t0, 3
    rotri.d $t0, $t0, 3

    csrwr   $t0, CSR_TLBRELO0
    csrwr   $t0, CSR_TLBRELO1
    tlbfill

restore_t0:
    csrrd   $t0, CSR_TLBRSAVE
    ertn
