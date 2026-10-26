// Self-checking test programs. A program starts at `_start` in M mode with
// everything else zero; it ends with PASS, or FAIL with the number of the failing
// case (in gp) through the test device.

#define TESTDEV 0x100000

#define PASS              \
    li t0, TESTDEV;       \
    li t1, 0x5555;        \
    sw t1, 0(t0);         \
1:  j 1b

#define FAIL_GP           \
    slli t1, gp, 16;      \
    ori t1, t1, 0x333;    \
    li t2, 0x3000;        \
    or t1, t1, t2;        \
    li t0, TESTDEV;       \
    sw t1, 0(t0);         \
1:  j 1b

// Case n: run `code`, then check that `reg` holds `val`.
#define TEST(n, reg, val, code...) \
    li gp, n;             \
    code;                 \
    li t6, val;           \
    bne reg, t6, fail

// Register-register and register-immediate ALU cases.
#define TEST_RR(n, op, val, a, b) TEST(n, x14, val, li x11, a; li x12, b; op x14, x11, x12)
#define TEST_RI(n, op, val, a, imm) TEST(n, x14, val, li x11, a; op x14, x11, imm)

#define START          \
    .section .text;    \
    .globl _start;     \
_start:

#define END            \
    PASS;              \
fail:                  \
    FAIL_GP;
