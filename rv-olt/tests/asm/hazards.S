// Pipeline hazards: results used right after they are produced (forwarding,
// load-use, multi-cycle units), CSR reads right behind the instructions that
// update them, and jumps through just-loaded addresses.
#include "fp.h"
START
    FP_ON
    // 1: fmv.d.x round trip, spaced
    li gp, 1; li t0, 0x1234; fmv.d.x f1, t0; nop; nop; nop; nop; fmv.x.d x14, f1; nop; nop; nop; li t6, 0x1234; bne x14, t6, fail
    // 2: back-to-back round trip
    li gp, 2; li t0, 0x5678; fmv.d.x f1, t0; fmv.x.d x14, f1; li t6, 0x5678; bne x14, t6, fail
    // 3: fadd spaced
    li gp, 3; li t0, 0xffffffff00000000; fmv.d.x f1, t0; li t0, 0xffffffff80000000; fmv.d.x f2, t0; nop; nop; nop; nop
    fadd.s f3, f1, f2; nop; nop; nop; nop; fmv.x.d x14, f3; li t6, 0xffffffff00000000; bne x14, t6, fail
    // 4: fadd back to back
    li gp, 4; li t0, 0xffffffff00000000; fmv.d.x f1, t0; li t0, 0xffffffff80000000; fmv.d.x f2, t0
    fadd.s f3, f1, f2; fmv.x.d x14, f3; li t6, 0xffffffff00000000; bne x14, t6, fail
    // 5: flags read right after the op that raised them
    li gp, 5; li t0, 0xffffffff3f800000; fmv.d.x f1, t0; li t0, 0xffffffff40400000; fmv.d.x f2, t0
    csrw fflags, zero; fdiv.s f3, f1, f2; csrr t5, fflags; li t6, 1; bne t5, t6, fail
    // 6: fflags written right after an op that raises flags: the write wins
    li gp, 6; fdiv.s f3, f1, f2; csrw fflags, zero; csrr t5, fflags; bne t5, zero, fail
    // 7: mul result used at once, then a load-use, then store of a load
    li gp, 7; li t0, 7; li t1, 6; mul t2, t0, t1; addi t2, t2, 1; li t6, 43; bne t2, t6, fail
    la s0, data; li t0, 99; sd t0, 0(s0); ld t1, 0(s0); addi t1, t1, 1; sd t1, 8(s0); ld t2, 8(s0); li t6, 100; bne t2, t6, fail
    // 8: jalr through a loaded address
    li gp, 8; la t0, 1f; sd t0, 0(s0); ld t1, 0(s0); jalr t1; j fail
1:  li gp, 9; div t0, t6, t6; li t1, 1; bne t0, t1, fail
END
.data
.align 3
data: .dword 0, 0
