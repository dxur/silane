// Privileged architecture: CSRs, traps, privilege modes, delegation.
//
// Every trap to M lands in `mtrap`, which records mcause/mepc/mtval in s10/s11/s8
// (and mstatus in s7) and continues in M mode at the address in s9. Traps delegated to S land in
// `strap`, which records scause/sepc/stval (and sstatus in s7) and continues in S mode
// at s9.
#include "test.h"

#define MSTATUS_MPP (3 << 11)
#define MSTATUS_SPP (1 << 8)

// Run `code` expecting a trap with this cause (and mtval), continuing after it.
#define EXPECT_TRAP(n, cause, code...) \
    li gp, n; la s9, 9f; li s10, -1; code; j fail; 9: \
    li t6, cause; bne s10, t6, fail

// Switch to privilege `mode` (0 U, 1 S) at the next instruction, via mret.
#define ENTER(mode) \
    li t0, MSTATUS_MPP; csrc mstatus, t0; li t0, (mode) << 11; csrs mstatus, t0; \
    la t0, 8f; csrw mepc, t0; mret; 8:

// From S or U, back to M at the next instruction (via ecall and mtrap).
#define TO_M  la s9, 7f; ecall; 7:

START
    la t0, mtrap
    csrw mtvec, t0
    la t0, strap
    csrw stvec, t0

    // CSR instructions on mscratch.
    TEST(1, x14, 0, csrrw x14, mscratch, zero)
    TEST(2, x14, 0, li t0, 0x1234; csrrw x14, mscratch, t0)
    TEST(3, x14, 0x1234, csrr x14, mscratch)
    TEST(4, x14, 0x1234, li t0, 0xf0000; csrrs x14, mscratch, t0)
    TEST(5, x14, 0xf1234, li t0, 0x30; csrrc x14, mscratch, t0)
    TEST(6, x14, 0xf1204, csrrwi x14, mscratch, 7)
    TEST(7, x14, 7, csrrsi x14, mscratch, 0x18)
    TEST(8, x14, 0x1f, csrrci x14, mscratch, 3)
    TEST(9, x14, 0x1c, csrr x14, mscratch)
    // Read-only machine information.
    TEST(10, x14, 0x800000000014112d, csrr x14, misa)
    TEST(11, x14, 0, csrr x14, mhartid)
    TEST(12, x14, 0, csrr x14, mvendorid)
    // PMP and HPM registers read as zero and ignore writes.
    TEST(13, x14, 0, li t0, -1; csrw pmpaddr0, t0; csrr x14, pmpaddr0)
    TEST(14, x14, 0, csrr x14, mhpmcounter3)
    // mstatus: UXL/SXL read as 2 (64-bit); MPP keeps legal values only.
    TEST(15, x14, 0xa00000000, csrr x14, mstatus; li t0, 0xf00000000; and x14, x14, t0)
    TEST(16, x14, 0, li t0, MSTATUS_MPP; csrc mstatus, t0; li t0, 2 << 11; csrs mstatus, t0; csrr x14, mstatus; li t0, MSTATUS_MPP; and x14, x14, t0)
    // mepc ignores bit 0; mtvec mode 2/3 is not supported.
    TEST(17, x14, 0x1000, li t0, 0x1001; csrw mepc, t0; csrr x14, mepc)
    TEST(18, x14, 0x0, csrr t1, mtvec; li t0, 0x2; csrs mtvec, t0; csrr x14, mtvec; csrw mtvec, t1; andi x14, x14, 3)

    // Exceptions in M mode.
    EXPECT_TRAP(20, 11, ecall)
    EXPECT_TRAP(22, 11, 1: ecall)
    li gp, 23
    la t0, 1b
    bne s11, t0, fail
    EXPECT_TRAP(24, 3, 2: ebreak)
    li gp, 25
    la t0, 2b
    bne s8, t0, fail
    EXPECT_TRAP(26, 2, .word 0xffffffff)
    li gp, 27
    li t0, 0xffffffff
    bne s8, t0, fail
    EXPECT_TRAP(28, 2, .word 0)
    li gp, 29
    bne s8, zero, fail
    // An unknown CSR is illegal.
    EXPECT_TRAP(30, 2, csrr x14, 0x7c0)
    // Writing a read-only CSR is illegal (but reading it is not).
    EXPECT_TRAP(31, 2, csrw mhartid, zero)
    // Misaligned accesses.
    la s0, buf
    EXPECT_TRAP(32, 4, ld x14, 4(s0))
    li gp, 33
    addi t0, s0, 4
    bne s8, t0, fail
    EXPECT_TRAP(34, 6, sw x14, 2(s0))
    EXPECT_TRAP(35, 6, addi t0, s0, 4; amoadd.d x14, x0, (t0))
    EXPECT_TRAP(36, 4, addi t0, s0, 2; lr.w x14, (t0))
    // Access faults: nothing at address 8, nothing at 0x2000_0000.
    EXPECT_TRAP(37, 5, li t0, 8; ld x14, 0(t0))
    EXPECT_TRAP(38, 7, li t0, 0x20000000; sd x14, 0(t0))
    EXPECT_TRAP(39, 1, li t0, 0x20000000; jr t0)
    li gp, 40
    li t0, 0x20000000
    bne s8, t0, fail
    // mret: MPIE -> MIE, MPIE set, MPP -> U.
    li gp, 41
    li t0, 0x88
    csrc mstatus, t0
    li t0, 0x80
    csrs mstatus, t0
    li t0, MSTATUS_MPP
    csrs mstatus, t0
    la t0, 3f
    csrw mepc, t0
    mret
3:  csrr x14, mstatus
    andi t1, x14, 0x88
    li t6, 0x88
    bne t1, t6, fail
    li t0, MSTATUS_MPP
    and t1, x14, t0
    bne t1, zero, fail
    csrci mstatus, 8

    // S mode: ecall traps to M with cause 9 and MPP = S.
    ENTER(1)
    EXPECT_TRAP(50, 9, ecall)
    li gp, 51
    li t1, MSTATUS_MPP
    and t0, s7, t1
    li t1, 1 << 11
    bne t0, t1, fail
    // M-mode CSRs are illegal from S; S-mode ones are fine.
    ENTER(1)
    EXPECT_TRAP(52, 2, csrr x14, mstatus)
    ENTER(1)
    li gp, 53
    li t0, 0x5a
    csrw sscratch, t0
    csrr x14, sscratch
    bne x14, t0, fail
    // sstatus shows only S fields.
    csrr x14, sstatus
    TO_M
    li gp, 54
    li t0, ~0x80000003000de762
    and t0, x14, t0
    bnez t0, fail
    // MRET is illegal in S; SRET is fine in S unless TSR.
    ENTER(1)
    EXPECT_TRAP(55, 2, mret)
    li t0, 1 << 22          // TSR
    csrs mstatus, t0
    ENTER(1)
    EXPECT_TRAP(56, 2, sret)
    li t0, 1 << 22
    csrc mstatus, t0
    // TVM: satp and SFENCE.VMA are illegal in S.
    li t0, 1 << 20
    csrs mstatus, t0
    ENTER(1)
    EXPECT_TRAP(57, 2, csrr x14, satp)
    ENTER(1)
    EXPECT_TRAP(58, 2, sfence.vma)
    li t0, 1 << 20
    csrc mstatus, t0
    // TW: WFI is illegal in S.
    li t0, 1 << 21
    csrs mstatus, t0
    ENTER(1)
    EXPECT_TRAP(59, 2, wfi)
    li t0, 1 << 21
    csrc mstatus, t0

    // U mode.
    ENTER(0)
    EXPECT_TRAP(60, 8, ecall)
    ENTER(0)
    EXPECT_TRAP(61, 2, csrr x14, sscratch)
    ENTER(0)
    EXPECT_TRAP(62, 2, wfi)
    ENTER(0)
    EXPECT_TRAP(63, 2, sret)
    // Counters from U: only with mcounteren and scounteren.
    csrw mcounteren, zero
    csrw scounteren, zero
    ENTER(0)
    EXPECT_TRAP(64, 2, rdcycle x14)
    li t0, 7
    csrw mcounteren, t0
    ENTER(0)
    EXPECT_TRAP(65, 2, rdtime x14)
    li t0, 5
    csrw scounteren, t0
    ENTER(0)
    EXPECT_TRAP(66, 2, rdtime x14)
    ENTER(0)
    rdcycle x14
    rdcycle x15
    rdinstret x16
    rdinstret x17
    TO_M
    li gp, 67
    bgeu x14, x15, fail
    sub t0, x17, x16
    li t1, 1
    bne t0, t1, fail
    ENTER(1)
    rdtime x14
    TO_M

    // Delegation: U ecalls and illegal instructions go to S.
    li t0, (1 << 8) | (1 << 2)
    csrw medeleg, t0
    // U -> S trap, then S -> M.
    ENTER(0)
    li gp, 70
    la s9, 4f
    li s10, -1
    ecall
    j fail
4:  // now in S (strap returned here with SPP = S)
    li t6, 8
    bne s10, t6, fail
    // Taken from U: SPP was 0. We are in S now: sscratch is accessible.
    li gp, 71
    andi x14, s7, MSTATUS_SPP
    bnez x14, fail
    csrr x14, sscratch
    TO_M
    // An illegal instruction taken from S is delegated too (SPP = 1).
    ENTER(1)
    li gp, 72
    la s9, 5f
    li s10, -1
    .word 0
    j fail
5:  li t6, 2
    bne s10, t6, fail
    andi x14, s7, MSTATUS_SPP
    beqz x14, fail
    TO_M
    // From M, delegated causes still trap to M.
    EXPECT_TRAP(73, 2, .word 0)
    csrw medeleg, zero

    // medeleg/mideleg hold only legal bits.
    TEST(80, x14, 0xb3ff, li t0, -1; csrw medeleg, t0; csrr x14, medeleg; csrw medeleg, zero)
    TEST(81, x14, 0x222, li t0, -1; csrw mideleg, t0; csrr x14, mideleg; csrw mideleg, zero)
    TEST(82, x14, 0xaaa, li t0, -1; csrw mie, t0; csrr x14, mie; csrw mie, zero)
END

    .align 2
mtrap:
    csrr s7, mstatus
    csrr s10, mcause
    csrr s11, mepc
    csrr s8, mtval
    csrw mepc, s9
    li t0, MSTATUS_MPP
    csrs mstatus, t0
    mret

    .align 2
strap:
    csrr s7, sstatus
    csrr s10, scause
    csrr s11, sepc
    csrr s8, stval
    csrw sepc, s9
    li t0, MSTATUS_SPP
    csrs sstatus, t0
    sret

    .data
    .align 3
buf: .dword 0, 0
