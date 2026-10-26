// RV64C: every compressed instruction, written explicitly.
#include "test.h"
START
    .option push
    .option rvc
    la sp, stack_top
    la s2, buf

    TEST(1, x8, 5, c.li x8, 5)
    TEST(2, x8, 4, c.addi x8, -1)
    TEST(3, x14, -32, c.li x14, -32)
    TEST(4, x9, 0x1f000, c.lui x9, 0x1f)
    TEST(5, x9, 0xfffffffffffe0000, c.lui x9, 0xfffe0)
    TEST(6, x10, 0x00000000ffffffff, li x10, 0x7fffffff; c.addiw x10, 0; li t0, 0x80000000; c.add x10, t0)
    TEST(7, x10, 0xffffffff80000000, li x10, 0x7fffffff; c.addiw x10, 1)
    // c.addi16sp / c.addi4spn
    TEST(8, x14, 0, mv x14, sp; c.addi16sp sp, -64; c.addi16sp sp, 64; sub x14, x14, sp)
    TEST(9, x14, 0, c.addi4spn x8, sp, 8; addi x14, sp, 8; sub x14, x14, x8)
    // shifts and logic on x8..x15
    TEST(10, x8, 0x0fffffffffffffff, li x8, -1; c.srli x8, 4)
    TEST(11, x8, -1, li x8, -16; c.srai x8, 4)
    TEST(12, x8, 0x1, li x8, 0x80000000; c.srli x8, 31)
    TEST(13, x8, 0x5, li x8, 0x1f5; c.andi x8, 0xf)
    TEST(14, x8, 0x30, li x8, 3; c.slli x8, 4)
    TEST(15, x8, 0x8000000000000000, li x8, 1; c.slli x8, 63)
    TEST(16, x8, 3, li x8, 5; li x9, 2; c.sub x8, x9)
    TEST(17, x8, 6, li x8, 5; li x9, 3; c.xor x8, x9)
    TEST(18, x8, 7, li x8, 5; li x9, 3; c.or x8, x9)
    TEST(19, x8, 1, li x8, 5; li x9, 3; c.and x8, x9)
    TEST(20, x8, 0xffffffff80000000, li x8, 0x7fffffff; li x9, 1; c.addw x8, x9)
    TEST(21, x8, 0x000000007fffffff, li x8, 0x80000000; li x9, 1; c.subw x8, x9)
    TEST(22, x14, 9, li x11, 9; c.mv x14, x11)
    TEST(23, x14, 13, li x14, 4; li x11, 9; c.add x14, x11)
    // loads and stores
    TEST(30, x9, 0x1122334455667788, li x10, 0x1122334455667788; mv x8, s2; c.sd x10, 0(x8); c.ld x9, 0(x8))
    TEST(31, x9, 0x55667788, mv x8, s2; c.lw x9, 0(x8))
    TEST(32, x9, 0xffffffffaabbccdd, mv x8, s2; li x10, 0xaabbccdd; c.sw x10, 4(x8); c.lw x9, 4(x8))
    TEST(33, x14, 0x0102030405060708, li x13, 0x0102030405060708; c.sdsp x13, 16(sp); c.ldsp x14, 16(sp))
    TEST(34, x14, 0xfffffffff0f0f0f0, li x13, 0xf0f0f0f0; c.swsp x13, 24(sp); c.lwsp x14, 24(sp))
    // jumps and branches
    li gp, 40
    c.j 1f
    j fail
1:  li gp, 41
    li x8, 0
    c.beqz x8, 2f
    j fail
2:  li gp, 42
    li x8, 1
    c.beqz x8, fail_far
    c.bnez x8, 3f
    j fail
3:  li gp, 43
    la x11, 4f
    c.jr x11
    j fail
4:  li gp, 44
    la x11, 5f
    c.jalr x11
6:  j fail
5:  la x15, 6b
    bne x1, x15, fail
    // A 32-bit instruction straddling an 8-byte boundary (and then a 16-bit one).
    li gp, 45
    .balign 8
    c.nop
    c.nop
    c.nop
    li x14, 0x5a5      // 32-bit addi at offset 6
    c.addi x14, 1
    li t6, 0x5a6
    bne x14, t6, fail
    .option pop
END
fail_far:
    j fail

    .data
    .align 4
buf: .dword 0, 0, 0, 0
stack: .space 256
stack_top:
