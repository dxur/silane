// Floating-point test cases: operands are raw register bits (singles NaN-boxed);
// each case checks the result bits and the fflags it raised.
#include "test.h"

#define FP_ON li t0, 1 << 13; csrs mstatus, t0; csrw fflags, zero

#define CHECK_FL(fl) csrrw t5, fflags, zero; li t6, fl; bne t5, t6, fail

// f3 = op(f1, f2) with rounding mode rm
#define TEST_FFF(n, op, rm, res, fl, a, b) \
    li gp, n; li t0, a; fmv.d.x f1, t0; li t0, b; fmv.d.x f2, t0; \
    op f3, f1, f2, rm; fmv.x.d x14, f3; li t6, res; bne x14, t6, fail; CHECK_FL(fl)
// without a rounding mode (min/max, sign injection)
#define TEST_FFF_NORM(n, op, res, fl, a, b) \
    li gp, n; li t0, a; fmv.d.x f1, t0; li t0, b; fmv.d.x f2, t0; \
    op f3, f1, f2; fmv.x.d x14, f3; li t6, res; bne x14, t6, fail; CHECK_FL(fl)
#define TEST_FFFF(n, op, rm, res, fl, a, b, c) \
    li gp, n; li t0, a; fmv.d.x f1, t0; li t0, b; fmv.d.x f2, t0; li t0, c; fmv.d.x f4, t0; \
    op f3, f1, f2, f4, rm; fmv.x.d x14, f3; li t6, res; bne x14, t6, fail; CHECK_FL(fl)
#define TEST_FF(n, op, rm, res, fl, a) \
    li gp, n; li t0, a; fmv.d.x f1, t0; \
    op f3, f1, rm; fmv.x.d x14, f3; li t6, res; bne x14, t6, fail; CHECK_FL(fl)
// integer result
#define TEST_XFF(n, op, res, fl, a, b) \
    li gp, n; li t0, a; fmv.d.x f1, t0; li t0, b; fmv.d.x f2, t0; \
    op x14, f1, f2; li t6, res; bne x14, t6, fail; CHECK_FL(fl)
#define TEST_XF(n, op, rm, res, fl, a) \
    li gp, n; li t0, a; fmv.d.x f1, t0; \
    op x14, f1, rm; li t6, res; bne x14, t6, fail; CHECK_FL(fl)
#define TEST_XF_NORM(n, op, res, fl, a) \
    li gp, n; li t0, a; fmv.d.x f1, t0; \
    op x14, f1; li t6, res; bne x14, t6, fail; CHECK_FL(fl)
// integer source
#define TEST_FX(n, op, rm, res, fl, a) \
    li gp, n; li x11, a; \
    op f3, x11, rm; fmv.x.d x14, f3; li t6, res; bne x14, t6, fail; CHECK_FL(fl)
#define TEST_FX_NORM(n, op, res, fl, a) \
    li gp, n; li x11, a; \
    op f3, x11; fmv.x.d x14, f3; li t6, res; bne x14, t6, fail; CHECK_FL(fl)
