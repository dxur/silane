// RV64A: LR/SC and the AMOs, 32- and 64-bit.
#include "test.h"
START
    la s0, buf

    li t0, 100
    sd t0, 0(s0)
    TEST(1, x14, 100, li x12, 5; amoadd.d x14, x12, (s0))
    TEST(2, x14, 105, ld x14, 0(s0))
    TEST(3, x14, 105, li x12, 7; amoswap.d x14, x12, (s0))
    TEST(4, x14, 7, ld x14, 0(s0))
    TEST(5, x14, 7, li x12, 0xc; amoxor.d x14, x12, (s0))
    TEST(6, x14, 0xb, ld x14, 0(s0))
    TEST(7, x14, 0xb, li x12, 0x6; amoand.d x14, x12, (s0))
    TEST(8, x14, 0x2, ld x14, 0(s0))
    TEST(9, x14, 0x2, li x12, 0x5; amoor.d x14, x12, (s0))
    TEST(10, x14, 0x7, ld x14, 0(s0))
    TEST(11, x14, 0x7, li x12, -1; amomin.d x14, x12, (s0))
    TEST(12, x14, -1, ld x14, 0(s0))
    TEST(13, x14, -1, li x12, 3; amomax.d x14, x12, (s0))
    TEST(14, x14, 3, ld x14, 0(s0))
    TEST(15, x14, 3, li x12, -1; amominu.d x14, x12, (s0))
    TEST(16, x14, 3, ld x14, 0(s0))
    TEST(17, x14, 3, li x12, -1; amomaxu.d x14, x12, (s0))
    TEST(18, x14, -1, ld x14, 0(s0))

    // 32-bit AMOs sign-extend the old value and leave the upper word alone.
    li t0, 0x1111111180000000
    sd t0, 8(s0)
    addi s1, s0, 8
    TEST(20, x14, 0xffffffff80000000, li x12, 1; amoadd.w x14, x12, (s1))
    TEST(21, x14, 0x1111111180000001, ld x14, 8(s0))
    TEST(22, x14, 0xffffffff80000001, li x12, 5; amomin.w x14, x12, (s1))
    TEST(23, x14, 0x1111111180000001, ld x14, 8(s0))
    TEST(24, x14, 0xffffffff80000001, li x12, 5; amominu.w x14, x12, (s1))
    TEST(25, x14, 0x1111111100000005, ld x14, 8(s0))
    TEST(26, x14, 5, li x12, 0x80000000; amomax.w x14, x12, (s1))
    TEST(27, x14, 0x1111111100000005, ld x14, 8(s0))
    TEST(28, x14, 5, li x12, 0x80000000; amomaxu.w x14, x12, (s1))
    TEST(29, x14, 0x1111111180000000, ld x14, 8(s0))
    // The upper word, as a 32-bit AMO.
    addi s2, s0, 12
    TEST(30, x14, 0x11111111, li x12, 0x22222222; amoswap.w x14, x12, (s2))
    TEST(31, x14, 0x2222222280000000, ld x14, 8(s0))

    // LR/SC: success, then failure without a reservation.
    li t0, 40
    sd t0, 16(s0)
    addi s3, s0, 16
    TEST(40, x14, 40, lr.d x14, (s3))
    TEST(41, x14, 0, li x12, 41; sc.d x14, x12, (s3))
    TEST(42, x14, 41, ld x14, 16(s0))
    TEST(43, x14, 1, li x12, 42; sc.d x14, x12, (s3))
    TEST(44, x14, 41, ld x14, 16(s0))
    // A reservation for another address fails.
    TEST(45, x14, 41, lr.d x14, (s3))
    TEST(46, x14, 1, li x12, 9; sc.d x14, x12, (s0))
    TEST(47, x14, 41, ld x14, 16(s0))
    // 32-bit.
    TEST(48, x14, 41, lr.w x14, (s3))
    TEST(49, x14, 0, li x12, -2; sc.w x14, x12, (s3))
    TEST(50, x14, 0x00000000fffffffe, ld x14, 16(s0))
    // An atomic increment loop.
    li gp, 51
    sd zero, 24(s0)
    addi s4, s0, 24
    li x11, 20
1:  lr.d x12, (s4)
    addi x12, x12, 1
    sc.d x13, x12, (s4)
    bnez x13, 1b
    addi x11, x11, -1
    bnez x11, 1b
    ld x14, 24(s0)
    li t6, 20
    bne x14, t6, fail
END

    .data
    .align 3
buf: .dword 0, 0, 0, 0
