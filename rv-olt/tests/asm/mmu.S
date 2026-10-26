// Sv39 translation: permissions, A/D, U/SUM/MXR, superpages, faults, MPRV.
//
// The tables map RAM (0x8000_0000, 1 GiB) one-to-one for S mode, and test pages at
// 0x4000_0000 + n * 0x1000 (see the PTE list below).
#include "test.h"

#define V 1
#define R 2
#define W 4
#define X 8
#define U 16
#define A 64
#define D 128
#define MSTATUS_MPP (3 << 11)
#define SUM (1 << 18)
#define MXR (1 << 19)
#define MPRV (1 << 17)

#define EXPECT_TRAP(n, cause, code...) \
    li gp, n; la s9, 9f; li s10, -1; code; j fail; 9: \
    li t6, cause; bne s10, t6, fail
#define ENTER(mode) \
    li t0, MSTATUS_MPP; csrc mstatus, t0; li t0, (mode) << 11; csrs mstatus, t0; \
    la t0, 8f; csrw mepc, t0; mret; 8:
#define TO_M  la s9, 7f; ecall; 7:
// Store PTE `flags` for the page at `target` into slot n of table `tbl`.
#define PTE(tbl, n, target, flags) \
    la t0, target; srli t0, t0, 2; ori t0, t0, flags; la t1, tbl; sd t0, (n) * 8(t1)

START
    la t0, mtrap
    csrw mtvec, t0

    // Root: [1] -> l1, [2] -> RAM gigapage, [3] -> a misaligned gigapage.
    PTE(root, 1, l1, V)
    li t0, (0x80000000 >> 2) | V | R | W | X | A | D
    la t1, root
    sd t0, 16(t1)
    li t0, (0x80001000 >> 2) | V | R | A
    sd t0, 24(t1)
    PTE(l1, 0, l0, V)
    PTE(l0, 0, page, V | R | W | A | D)
    PTE(l0, 1, page, V | R | A)
    PTE(l0, 2, page, V | R | W | U | A | D)
    PTE(l0, 3, page, V | R | W | D)
    PTE(l0, 4, page, V | R | W | A)
    PTE(l0, 5, page, R | W | A | D)
    PTE(l0, 6, page, V | X | A)
    PTE(l0, 7, page, V | W | A | D)
    PTE(l0, 8, ucode, V | R | X | U | A)
    PTE(l0, 10, ucross, V | R | X | U | A)
    // l0[11] stays invalid.

    la t0, root
    srli t0, t0, 12
    li t1, 8 << 60
    or t0, t0, t1
    csrw satp, t0

    la s0, page
    li t0, 0x1234
    sd t0, 0(s0)
    li s1, 0x40000000

    // S-mode accesses through the 4 KiB pages.
    ENTER(1)
    TEST(1, x14, 0x1234, ld x14, 0(s1))
    TEST(2, x14, 0x55, li t0, 0x55; sd t0, 8(s1); ld x14, 8(s1))
    TO_M
    TEST(3, x14, 0x55, ld x14, 8(s0))
    ENTER(1)
    EXPECT_TRAP(4, 15, li t0, 0x40001000; sd x0, 0(t0))
    li gp, 5
    li t0, 0x40001000
    bne s8, t0, fail
    ENTER(1)
    TEST(6, x14, 0x1234, li t0, 0x40001000; ld x14, 0(t0))
    // U page from S: only with SUM, and never for fetches.
    EXPECT_TRAP(7, 13, li t0, 0x40002000; ld x14, 0(t0))
    li t0, SUM
    csrs mstatus, t0
    ENTER(1)
    TEST(8, x14, 0x1234, li t0, 0x40002000; ld x14, 0(t0))
    TO_M
    li t0, SUM
    csrc mstatus, t0
    ENTER(1)
    // A clear: any access faults. D clear: stores fault.
    EXPECT_TRAP(9, 13, li t0, 0x40003000; ld x14, 0(t0))
    ENTER(1)
    EXPECT_TRAP(10, 15, li t0, 0x40004000; sd x14, 0(t0))
    ENTER(1)
    TEST(11, x14, 0x1234, li t0, 0x40004000; ld x14, 0(t0))
    // Invalid, and W without R.
    EXPECT_TRAP(12, 13, li t0, 0x40005000; ld x14, 0(t0))
    ENTER(1)
    EXPECT_TRAP(13, 13, li t0, 0x40007000; ld x14, 0(t0))
    // Execute-only: loads fault unless MXR.
    ENTER(1)
    EXPECT_TRAP(14, 13, li t0, 0x40006000; ld x14, 0(t0))
    li t0, MXR
    csrs mstatus, t0
    ENTER(1)
    TEST(15, x14, 0x1234, li t0, 0x40006000; ld x14, 0(t0))
    TO_M
    li t0, MXR
    csrc mstatus, t0
    // A non-canonical address.
    ENTER(1)
    EXPECT_TRAP(16, 13, li t0, 0x0000008000000000; ld x14, 0(t0))
    // A misaligned gigapage.
    ENTER(1)
    EXPECT_TRAP(17, 13, li t0, 0xc0000000; ld x14, 0(t0))
    // Fetching from a U page in S.
    ENTER(1)
    EXPECT_TRAP(18, 12, li t0, 0x40008000; jr t0)
    li gp, 19
    li t0, 0x40008000
    bne s8, t0, fail
    // An unmapped address (no l1 entry).
    ENTER(1)
    EXPECT_TRAP(20, 13, li t0, 0x7ff00000; ld x14, 0(t0))
    // SFENCE.VMA is allowed in S.
    ENTER(1)
    sfence.vma
    TO_M

    // U mode: code at 0x4000_8000 does `ld x14, 0x2000(0x4000_0000)` and ecall.
    li gp, 30
    li t0, MSTATUS_MPP
    csrc mstatus, t0
    li t0, 0x40008000
    csrw mepc, t0
    la s9, 1f
    li s10, -1
    mret
1:  li t6, 8
    bne s10, t6, fail
    li gp, 31
    li t6, 0x1234
    bne x14, t6, fail
    // U accessing an S page.
    li gp, 32
    la t0, ucode2
    la t1, ucode
    sub t0, t0, t1
    li t1, 0x40008000
    add t0, t0, t1
    csrw mepc, t0
    la s9, 1f
    li s10, -1
    mret
1:  li t6, 13
    bne s10, t6, fail
    // An instruction crossing into an unmapped page faults at the second page.
    li gp, 33
    li t0, 0x4000affe
    csrw mepc, t0
    la s9, 1f
    li s10, -1
    mret
1:  li t6, 12
    bne s10, t6, fail
    li t0, 0x4000b000
    bne s8, t0, fail

    // MPRV: M-mode loads and stores translated as S.
    li t0, MSTATUS_MPP
    csrc mstatus, t0
    li t0, 1 << 11
    csrs mstatus, t0
    li t0, MPRV
    csrs mstatus, t0
    li t0, 0x40000000
    ld x14, 0(t0)
    li t0, MPRV
    csrc mstatus, t0
    li gp, 40
    li t6, 0x1234
    bne x14, t6, fail
    // M mode is never translated.
    TEST(41, x14, 0x1234, ld x14, 0(s0))
    csrw satp, zero
END

    .align 2
mtrap:
    csrr s10, mcause
    csrr s11, mepc
    csrr s8, mtval
    csrw mepc, s9
    li t0, MSTATUS_MPP
    csrs mstatus, t0
    mret

    // U-mode code pages.
    .balign 4096
ucode:
    li t0, 0x40002000
    ld x14, 0(t0)
    ecall
ucode2:
    li t0, 0x40000000
    ld x14, 0(t0)
    ecall
    .balign 4096
ucross:
    .skip 0xffe
    .word 0x00500093       // li x1, 5: its upper half is on the next page

    .data
    .balign 4096
root: .skip 4096
l1: .skip 4096
l0: .skip 4096
page: .skip 4096
