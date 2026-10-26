// RV64I loads, stores, branches, jumps, LUI/AUIPC and x0.
#include "test.h"
START
    la s0, buf

    // Stores of each size, then loads with and without sign extension.
    li t0, 0x8899aabbccddeeff
    sd t0, 0(s0)
    TEST(1, x14, 0x8899aabbccddeeff, ld x14, 0(s0))
    TEST(2, x14, 0xffffffffffffffff, lb x14, 0(s0))
    TEST(3, x14, 0xff, lbu x14, 0(s0))
    TEST(4, x14, 0xffffffffffffeeff, lh x14, 0(s0))
    TEST(5, x14, 0xeeff, lhu x14, 0(s0))
    TEST(6, x14, 0xffffffffccddeeff, lw x14, 0(s0))
    TEST(7, x14, 0xccddeeff, lwu x14, 0(s0))
    TEST(8, x14, 0xffffffff8899aabb, lw x14, 4(s0))
    TEST(9, x14, 0xffffffffffff8899, lh x14, 6(s0))
    TEST(10, x14, 0xffffffffffffff88, lb x14, 7(s0))
    TEST(11, x14, 0x0000000000000099, lbu x14, 6(s0))
    li t0, 0x12
    sb t0, 3(s0)
    TEST(12, x14, 0x8899aabb12ddeeff, ld x14, 0(s0))
    li t0, 0x3456
    sh t0, 6(s0)
    TEST(13, x14, 0x3456aabb12ddeeff, ld x14, 0(s0))
    li t0, 0x01020304
    sw t0, 0(s0)
    TEST(14, x14, 0x3456aabb01020304, ld x14, 0(s0))
    li t0, 0x7f
    sb t0, 8(s0)
    TEST(15, x14, 0x7f, lb x14, 8(s0))
    // Negative offsets.
    addi s1, s0, 16
    TEST(16, x14, 0x01020304, lw x14, -16(s1))

    // x0 ignores writes and reads 0.
    TEST(20, x0, 0, li x0, 5)
    TEST(21, x14, 0, addi x14, x0, 0)
    TEST(22, x14, 0, ld x0, 0(s0); mv x14, x0)

    // LUI / AUIPC.
    TEST(30, x14, 0x12345000, lui x14, 0x12345)
    TEST(31, x14, 0xfffffffffffff000, lui x14, 0xfffff)
    TEST(32, x14, 0xffffffff80000000, lui x14, 0x80000)
1:  auipc x14, 0
    la x15, 1b
    li gp, 33
    bne x14, x15, fail
2:  auipc x14, 1
    la x15, 2b
    sub x14, x14, x15
    li gp, 34
    li x15, 0x1000
    bne x14, x15, fail

    // Branches: each condition both ways, signed and unsigned.
#define BR(n, op, a, b, taken) \
    li gp, n; li x11, a; li x12, b; li x14, 0; \
    op x11, x12, 3f; li x14, 1; 3: \
    li t6, (1 - taken); bne x14, t6, fail
    BR(40, beq, 5, 5, 1)
    BR(41, beq, 5, 6, 0)
    BR(42, bne, 5, 6, 1)
    BR(43, bne, 5, 5, 0)
    BR(44, blt, -1, 1, 1)
    BR(45, blt, 1, -1, 0)
    BR(46, blt, 1, 1, 0)
    BR(47, bge, 1, -1, 1)
    BR(48, bge, 1, 1, 1)
    BR(49, bge, -2, -1, 0)
    BR(50, bltu, 1, -1, 1)
    BR(51, bltu, -1, 1, 0)
    BR(52, bgeu, -1, 1, 1)
    BR(53, bgeu, 1, -1, 0)
    BR(54, bgeu, 7, 7, 1)
    BR(55, blt, 0x8000000000000000, 0x7fffffffffffffff, 1)
    BR(56, bltu, 0x7fffffffffffffff, 0x8000000000000000, 1)

    // A backward branch loop.
    li gp, 60
    li x14, 0
    li x11, 10
4:  addi x14, x14, 3
    addi x11, x11, -1
    bnez x11, 4b
    li t6, 30
    bne x14, t6, fail

    // JAL / JALR: link values and targets.
    li gp, 70
    jal x1, 5f
6:  j fail
5:  la x15, 6b
    bne x1, x15, fail
    li gp, 71
    la x11, 7f
    jalr x5, 0(x11)
8:  j fail
7:  la x15, 8b
    bne x5, x15, fail
    // JALR clears bit 0 of the target and takes an offset.
    li gp, 72
    la x11, 9f
    addi x11, x11, -3
    jalr x0, 4(x11)
    j fail
9:  nop
    // JALR with rd == rs1.
    li gp, 73
    la x11, 10f
    jalr x11, 0(x11)
11: j fail
10: la x15, 11b
    bne x11, x15, fail

    // FENCE and FENCE.I are accepted.
    fence
    fence.i
    fence rw, rw
END

    .data
    .align 3
buf: .dword 0, 0, 0, 0
