// Interrupts: CLINT timer and software interrupts, WFI, vectored mtvec, delegation
// to S, the UART and the PLIC. The harness queues "Z" on the UART before starting.
#include "test.h"

#define CLINT 0x2000000
#define MTIMECMP (CLINT + 0x4000)
#define MTIME (CLINT + 0xbff8)
#define PLIC 0xc000000
#define UART 0x10000000
#define MSTATUS_MPP (3 << 11)
#define MIE_BIT 8
#define SIE_BIT 2

#define ENTER(mode) \
    li t0, MSTATUS_MPP; csrc mstatus, t0; li t0, (mode) << 11; csrs mstatus, t0; \
    la t0, 8f; csrw mepc, t0; mret; 8:

// Wait (spinning) until s10 changes from -1, at most ~2000 iterations.
#define WAIT_IRQ \
    li t5, 2000; 1: addi t5, t5, -1; beqz t5, fail; li t6, -1; beq s10, t6, 1b

START
    la t0, mtrap
    csrw mtvec, t0
    la t0, strap
    csrw stvec, t0
    li s4, MTIMECMP
    li s5, MTIME

    // Timer interrupt.
    li gp, 1
    li s10, -1
    ld t0, 0(s5)
    addi t0, t0, 20
    sd t0, 0(s4)
    li t0, 1 << 7
    csrw mie, t0
    csrsi mstatus, MIE_BIT
    WAIT_IRQ
    csrci mstatus, MIE_BIT
    li t6, 0x8000000000000007
    bne s10, t6, fail
    // mip.MTIP is clear again (the handler moved mtimecmp).
    li gp, 2
    csrr t0, mip
    andi t0, t0, 1 << 7
    bnez t0, fail

    // Software interrupt.
    li gp, 3
    li s10, -1
    li t0, 1 << 3
    csrw mie, t0
    li t1, CLINT
    li t0, 1
    sw t0, 0(t1)
    csrsi mstatus, MIE_BIT
    WAIT_IRQ
    csrci mstatus, MIE_BIT
    li t6, 0x8000000000000003
    bne s10, t6, fail

    // WFI wakes on a pending, enabled interrupt even with MIE clear.
    li gp, 4
    li s10, -1
    ld t0, 0(s5)
    addi t0, t0, 30
    sd t0, 0(s4)
    li t0, 1 << 7
    csrw mie, t0
    wfi
    li t6, -1
    bne s10, t6, fail
    csrr t0, mip
    andi t0, t0, 1 << 7
    beqz t0, fail
    li t0, -1
    sd t0, 0(s4)

    // Vectored mode: the timer goes to base + 4 * 7.
    li gp, 5
    la t0, vectors
    ori t0, t0, 1
    csrw mtvec, t0
    li s10, -1
    ld t0, 0(s5)
    addi t0, t0, 20
    sd t0, 0(s4)
    csrsi mstatus, MIE_BIT
    WAIT_IRQ
    csrci mstatus, MIE_BIT
    li t6, 0x77
    bne s10, t6, fail
    la t0, mtrap
    csrw mtvec, t0

    // An M interrupt is taken in S even with mstatus.MIE clear.
    li gp, 6
    li s10, -1
    ld t0, 0(s5)
    addi t0, t0, 40
    sd t0, 0(s4)
    ENTER(1)
    WAIT_IRQ
    // (mtrap returned to M)
    li t6, 0x8000000000000007
    bne s10, t6, fail

    // Delegated supervisor timer interrupt, raised by M through mip.STIP.
    li gp, 7
    li t0, (1 << 5) | (1 << 1)
    csrw mideleg, t0
    li t0, 1 << 5
    csrw mie, t0
    csrs mip, t0
    li s10, -1
    ENTER(1)
    csrsi sstatus, SIE_BIT
    WAIT_IRQ
    li t6, 0x8000000000000005
    bne s10, t6, fail
    // Supervisor software interrupt, raised by S itself.
    li gp, 8
    li t0, 1 << 1
    csrs sie, t0
    li s10, -1
    csrs sip, t0
    WAIT_IRQ
    li t6, 0x8000000000000001
    bne s10, t6, fail
    // Back to M.
    la s9, 2f
    ecall
2:  csrci mstatus, MIE_BIT   // mret restored MIE from MPIE
    li t0, 1 << 5
    csrc mip, t0
    csrw mideleg, zero
    csrw mie, zero

    // UART: transmit, then receive "Z" through the PLIC.
    li gp, 10
    li t1, UART
    lb t0, 5(t1)            // LSR: THR empty
    andi t0, t0, 0x60
    li t6, 0x60
    bne t0, t6, fail
    li t0, 'o'
    sb t0, 0(t1)
    li t0, 'k'
    sb t0, 0(t1)
    // Scratch register.
    li gp, 11
    li t0, 0xa5
    sb t0, 7(t1)
    lbu t2, 7(t1)
    bne t2, t0, fail
    // The PLIC: source 10 priority 1, enabled for context 0, threshold 0.
    li gp, 12
    li t2, PLIC
    li t0, 1
    sw t0, 40(t2)
    lw t3, 40(t2)
    bne t3, t0, fail
    li t3, PLIC + 0x2000
    li t0, 1 << 10
    sw t0, 0(t3)
    li t3, PLIC + 0x200000
    sw zero, 0(t3)
    // UART receive interrupt.
    li t0, 1
    sb t0, 1(t1)
    li t0, 1 << 11
    csrw mie, t0
    li s10, -1
    csrsi mstatus, MIE_BIT
    WAIT_IRQ
    csrci mstatus, MIE_BIT
    li t6, 0x800000000000000b
    bne s10, t6, fail
    li gp, 13
    li t6, 10
    bne s6, t6, fail        // the claimed source
    li t6, 'Z'
    bne s7, t6, fail        // the byte read
    // Nothing more pending.
    li gp, 14
    lw t0, 4(t3)
    bnez t0, fail
    lb t0, 5(t1)
    andi t0, t0, 1
    bnez t0, fail
END

    .align 2
mtrap:
    csrr s10, mcause
    li t0, MSTATUS_MPP
    csrs mstatus, t0
    bgez s10, 3f
    andi t0, s10, 0x3f
    li t1, 7
    beq t0, t1, 4f
    li t1, 3
    beq t0, t1, 5f
    li t1, 11
    beq t0, t1, 6f
    mret
4:  li t0, -1             // timer: disarm (and continue in M)
    sd t0, 0(s4)
    mret
5:  li t1, CLINT          // software: clear msip
    sw zero, 0(t1)
    mret
6:  li t2, PLIC + 0x200000 // external: claim, read the UART, complete
    lw s6, 4(t2)
    li t1, UART
    lbu s7, 0(t1)
    sw s6, 4(t2)
    mret
3:  // an exception (ecall to get back to M): continue at s9 in M
    csrw mepc, s9
    mret

    .align 2
strap:
    csrr s10, scause
    li t0, (1 << 5) | (1 << 1)   // mask STIE and SSIE: the sources stay pending
    csrc sie, t0
    li t0, 1 << 1
    csrc sip, t0
    sret

    .align 6
vectors:
    .option push
    .option norvc
    .rept 7
    j fail
    .endr
    j vec_timer
    .rept 8
    j fail
    .endr
    .option pop
vec_timer:
    li s10, 0x77
    li t0, -1
    sd t0, 0(s4)
    mret
