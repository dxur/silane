#!/usr/bin/env python3
"""Generates tests/asm/rv64fd.S: F and D instruction cases with the expected result
bits and fflags, computed exactly here (rationals, then IEEE 754 rounding with
tininess after rounding, as RISC-V specifies).
Run from rv-olt/: python3 tests/asm/gen/fp_vectors.py"""

import random
from fractions import Fraction
from math import isqrt

M64 = (1 << 64) - 1
NV, DZ, OF, UF, NX = 16, 8, 4, 2, 1
RMS = ["rne", "rtz", "rdn", "rup", "rmm"]


class Fmt:
    def __init__(self, name, ebits, mbits):
        self.name, self.e, self.m = name, ebits, mbits
        self.bias = (1 << (ebits - 1)) - 1
        self.p = mbits + 1
        self.emin = 1 - self.bias
        self.emax = self.bias
        self.width = 1 + ebits + mbits

    def box(self, b):
        return b if self.width == 64 else (0xFFFF_FFFF_0000_0000 | b)

    def canon(self):
        return self.box((((1 << self.e) - 1) << self.m) | (1 << (self.m - 1)))

    def inf(self, s):
        return self.box((s << (self.width - 1)) | (((1 << self.e) - 1) << self.m))

    def zero(self, s):
        return self.box(s << (self.width - 1))

    def maxf(self, s):
        return self.box((s << (self.width - 1)) | ((((1 << self.e) - 2) << self.m) | ((1 << self.m) - 1)))


S = Fmt("s", 8, 23)
D = Fmt("d", 11, 52)


def decode(f, reg):
    """('nan', snan) | ('inf', sign) | ('num', sign, magnitude)"""
    if f.width == 32 and (reg >> 32) != 0xFFFF_FFFF:
        return ("nan", False)
    b = reg & ((1 << f.width) - 1)
    s = b >> (f.width - 1)
    e = (b >> f.m) & ((1 << f.e) - 1)
    m = b & ((1 << f.m) - 1)
    if e == (1 << f.e) - 1:
        if m == 0:
            return ("inf", s)
        return ("nan", not (m >> (f.m - 1)) & 1)
    if e == 0:
        v = Fraction(m, 1) * Fraction(2) ** (f.emin - f.m)
    else:
        v = Fraction((1 << f.m) | m) * Fraction(2) ** (e - f.bias - f.m)
    return ("num", s, v)


def floor_log2(v):
    e = v.numerator.bit_length() - v.denominator.bit_length()
    if Fraction(2) ** e > v:
        e -= 1
    if Fraction(2) ** (e + 1) <= v:
        e += 1
    return e


def round_int(n, rem, s, rm):
    """Round n + rem (0 <= rem < 1) by mode; returns the increment."""
    half = Fraction(1, 2)
    if rm == "rne":
        return int(rem > half or (rem == half and n & 1))
    if rm == "rtz":
        return 0
    if rm == "rdn":
        return int(s == 1 and rem > 0)
    if rm == "rup":
        return int(s == 0 and rem > 0)
    return int(rem >= half)  # rmm


def round_pack(f, s, v, rm):
    """Round sign s, magnitude v (> 0) to f; returns (bits, flags)."""
    e = floor_log2(v)
    q = max(e, f.emin) - f.p + 1
    sc = v / Fraction(2) ** q
    n = sc.numerator // sc.denominator
    rem = sc - n
    n += round_int(n, rem, s, rm)
    flags = NX if rem else 0
    if n == 1 << f.p:
        n >>= 1
        q += 1
    # Tininess after rounding: round to p bits with an unbounded exponent.
    qu = e - f.p + 1
    scu = v / Fraction(2) ** qu
    nu = scu.numerator // scu.denominator
    nu += round_int(nu, scu - nu, s, rm)
    if nu * Fraction(2) ** qu < Fraction(2) ** f.emin and rem:
        flags |= UF
    if n >= 1 << (f.p - 1) and q + f.p - 1 > f.emax:
        to_inf = {"rtz": False, "rdn": s == 1, "rup": s == 0}.get(rm, True)
        return (f.inf(s) if to_inf else f.maxf(s)), OF | NX
    if n < 1 << (f.p - 1):
        bits = n
    else:
        bits = ((q + f.p - 1 + f.bias) << f.m) | (n - (1 << (f.p - 1)))
    return f.box((s << (f.width - 1)) | bits), flags


def result(f, s, v, rm, zero_sign=None):
    if v == 0:
        return f.zero(zero_sign if zero_sign is not None else s), 0
    return round_pack(f, s, abs(v), rm)


def signed(x):
    return -x[2] if x[1] else x[2]


def is_snan(x):
    return x[0] == "nan" and x[1]


def op_add(f, a, b, rm, sub=False):
    A, B = decode(f, a), decode(f, b)
    if B[0] == "inf" and sub:
        B = ("inf", 1 - B[1])
    if B[0] == "num" and sub:
        B = ("num", 1 - B[1], B[2])
    if A[0] == "nan" or B[0] == "nan":
        return f.canon(), NV if is_snan(A) or is_snan(B) else 0
    if A[0] == "inf" and B[0] == "inf":
        return (f.inf(A[1]), 0) if A[1] == B[1] else (f.canon(), NV)
    if A[0] == "inf" or B[0] == "inf":
        return f.inf(A[1] if A[0] == "inf" else B[1]), 0
    v = signed(A) + signed(B)
    if v == 0:
        zs = A[1] if (A[2] == 0 and B[2] == 0 and A[1] == B[1]) else int(rm == "rdn")
        return f.zero(zs), 0
    return result(f, int(v < 0), v, rm)


def op_mul(f, a, b, rm):
    A, B = decode(f, a), decode(f, b)
    if A[0] == "nan" or B[0] == "nan":
        return f.canon(), NV if is_snan(A) or is_snan(B) else 0
    s = A[1] ^ B[1]
    if A[0] == "inf" or B[0] == "inf":
        if (A[0] == "num" and A[2] == 0) or (B[0] == "num" and B[2] == 0):
            return f.canon(), NV
        return f.inf(s), 0
    return result(f, s, A[2] * B[2], rm)


def op_div(f, a, b, rm):
    A, B = decode(f, a), decode(f, b)
    if A[0] == "nan" or B[0] == "nan":
        return f.canon(), NV if is_snan(A) or is_snan(B) else 0
    s = A[1] ^ B[1]
    if A[0] == "inf":
        return (f.canon(), NV) if B[0] == "inf" else (f.inf(s), 0)
    if B[0] == "inf":
        return f.zero(s), 0
    if B[2] == 0:
        return (f.canon(), NV) if A[2] == 0 else (f.inf(s), DZ)
    return result(f, s, A[2] / B[2], rm)


def op_sqrt(f, a, rm):
    A = decode(f, a)
    if A[0] == "nan":
        return f.canon(), NV if A[1] else 0
    if A[0] == "inf":
        return (f.canon(), NV) if A[1] else (f.inf(0), 0)
    if A[2] == 0:
        return f.zero(A[1]), 0
    if A[1]:
        return f.canon(), NV
    v = A[2]
    # Scale so the integer root has p + 12 bits or more.
    k = max(0, (2 * (f.p + 12) - floor_log2(v)) // 2 + 2)
    num = v.numerator * 4 ** k
    r = isqrt(num // v.denominator)
    exact = r * r * v.denominator == num
    root = Fraction(r, 2 ** k) if exact else Fraction(2 * r + 1, 2 ** (k + 1))
    return round_pack(f, 0, root, rm)


def op_fma(f, a, b, c, rm, neg_prod, neg_add):
    A, B, C = decode(f, a), decode(f, b), decode(f, c)
    inf_zero = (A[0] == "inf" and B[0] == "num" and B[2] == 0) or (B[0] == "inf" and A[0] == "num" and A[2] == 0)
    if A[0] == "nan" or B[0] == "nan" or C[0] == "nan":
        return f.canon(), NV if is_snan(A) or is_snan(B) or is_snan(C) or inf_zero else 0
    if inf_zero:
        return f.canon(), NV
    sp = A[1] ^ B[1] ^ neg_prod
    sc = C[1] ^ neg_add
    if A[0] == "inf" or B[0] == "inf":
        if C[0] == "inf" and sc != sp:
            return f.canon(), NV
        return f.inf(sp), 0
    if C[0] == "inf":
        return f.inf(sc), 0
    p = A[2] * B[2] * (-1 if sp else 1)
    cv = C[2] * (-1 if sc else 1)
    v = p + cv
    if v == 0:
        if p == 0 and cv == 0:
            zs = sp if sp == sc else int(rm == "rdn")
        else:
            zs = int(rm == "rdn")
        return f.zero(zs), 0
    return result(f, int(v < 0), v, rm)


def op_cvt_fi(f, a, rm, kind):
    """kind: w, wu, l, lu. Returns (64-bit integer register, flags)."""
    A = decode(f, a)
    bits = 32 if kind[0] == "w" else 64
    uns = kind.endswith("u")
    lo, hi = (0, (1 << bits) - 1) if uns else (-(1 << (bits - 1)), (1 << (bits - 1)) - 1)

    def reg(v):
        v &= (1 << bits) - 1
        if bits == 32:
            v = (v ^ 0x8000_0000) - 0x8000_0000
        return v & M64

    if A[0] == "nan" or (A[0] == "inf" and A[1] == 0):
        return reg(hi), NV
    if A[0] == "inf":
        return reg(lo), NV
    v = signed(A)
    n = v.numerator // v.denominator  # floor
    rem = v - n
    s = int(v < 0)
    # Round the value (not its magnitude) per mode.
    if rm == "rne":
        inc = int(rem > Fraction(1, 2) or (rem == Fraction(1, 2) and n & 1))
    elif rm == "rtz":
        inc = int(rem > 0 and s == 1)
    elif rm == "rdn":
        inc = 0
    elif rm == "rup":
        inc = int(rem > 0)
    else:
        inc = int(rem > Fraction(1, 2) or (rem == Fraction(1, 2) and s == 0))
    r = n + inc
    if r < lo or r > hi:
        return reg(lo if v < 0 else hi), NV
    return reg(r), (NX if rem else 0)


def op_cvt_if(f, x, rm, kind):
    bits = 32 if kind[0] == "w" else 64
    v = x & ((1 << bits) - 1)
    if not kind.endswith("u") and v >> (bits - 1):
        v -= 1 << bits
    if v == 0:
        return f.zero(0), 0
    return round_pack(f, int(v < 0), Fraction(abs(v)), rm)


def op_cvt_ff(dst, src, a, rm):
    A = decode(src, a)
    if A[0] == "nan":
        return dst.canon(), NV if A[1] else 0
    if A[0] == "inf":
        return dst.inf(A[1]), 0
    if A[2] == 0:
        return dst.zero(A[1]), 0
    return round_pack(dst, A[1], A[2], rm)


def lt_total(A, B):
    va, vb = signed(A) if A[0] == "num" else None, signed(B) if B[0] == "num" else None
    if A[0] == "inf":
        va = -Fraction(10) ** 400 if A[1] else Fraction(10) ** 400
    if B[0] == "inf":
        vb = -Fraction(10) ** 400 if B[1] else Fraction(10) ** 400
    if va == vb:
        return A[1] == 1 and B[1] == 0  # -0 < +0
    return va < vb


def op_minmax(f, a, b, is_max):
    A, B = decode(f, a), decode(f, b)
    fl = NV if is_snan(A) or is_snan(B) else 0
    if A[0] == "nan" and B[0] == "nan":
        return f.canon(), fl
    if A[0] == "nan":
        return b, fl
    if B[0] == "nan":
        return a, fl
    a_lt = lt_total(A, B)
    return (a if a_lt != is_max else b), fl


def op_cmp(f, a, b, kind):
    A, B = decode(f, a), decode(f, b)
    if A[0] == "nan" or B[0] == "nan":
        if kind == "eq":
            return 0, NV if is_snan(A) or is_snan(B) else 0
        return 0, NV
    va = signed(A) if A[0] == "num" else (-Fraction(10) ** 400 if A[1] else Fraction(10) ** 400)
    vb = signed(B) if B[0] == "num" else (-Fraction(10) ** 400 if B[1] else Fraction(10) ** 400)
    return int({"eq": va == vb, "lt": va < vb, "le": va <= vb}[kind]), 0


def op_class(f, a):
    A = decode(f, a)
    if A[0] == "nan":
        return 1 << (8 if A[1] else 9)
    if A[0] == "inf":
        return 1 << (0 if A[1] else 7)
    b = a & ((1 << f.width) - 1)
    sub = (b >> f.m) & ((1 << f.e) - 1) == 0
    if A[2] == 0:
        return 1 << (3 if A[1] else 4)
    if sub:
        return 1 << (2 if A[1] else 5)
    return 1 << (1 if A[1] else 6)


def op_sgnj(f, a, b, kind):
    A, B = a, b
    if f.width == 32:
        if (a >> 32) != 0xFFFF_FFFF:
            A = f.canon()
        if (b >> 32) != 0xFFFF_FFFF:
            B = f.canon()
    sb = (B >> (f.width - 1)) & 1
    sa = (A >> (f.width - 1)) & 1
    s = {"j": sb, "jn": 1 - sb, "jx": sa ^ sb}[kind]
    mask = (1 << (f.width - 1)) - 1
    return f.box((s << (f.width - 1)) | (A & mask)), 0


def values(f, rng):
    one = f.bias << f.m
    w = f.width
    v = [0, 1 << (w - 1), 1, (1 << f.m) - 1, 1 << f.m, one, one | (1 << (f.m - 1)), one + (1 << f.m),
         (((1 << f.e) - 2) << f.m) | ((1 << f.m) - 1), ((1 << f.e) - 1) << f.m,
         (((1 << f.e) - 1) << f.m) | (1 << (f.m - 1)), (((1 << f.e) - 1) << f.m) | 1,
         ((f.bias + 3) << f.m) | 0x5555 & ((1 << f.m) - 1), ((f.bias - 3) << f.m) | 0x2aaa,
         one | 1, one | ((1 << f.m) - 1), ((f.bias + f.m) << f.m) | 3]
    v += [x | (1 << (w - 1)) for x in v[2:10]]
    v += [rng.getrandbits(w) for _ in range(6)]
    v += [(rng.randrange(f.bias - 4, f.bias + 5) << f.m) | rng.getrandbits(f.m) | (rng.getrandbits(1) << (w - 1))
          for _ in range(6)]
    return [f.box(x) for x in v]


def h(x):
    return f"0x{x & M64:x}"


def main():
    rng = random.Random(7)
    lines = []
    n = 1

    def add(kind, *args):
        nonlocal n
        lines.append(f"{kind}({n}, " + ", ".join(args) + ")")
        n += 1

    for f in (S, D):
        sfx = f.name
        vs = values(f, rng)
        pairs = [(a, b) for a in vs[::2] for b in vs[1::3]]
        pairs += [(rng.choice(vs), rng.choice(vs)) for _ in range(40)]
        for i, (a, b) in enumerate(pairs):
            rm = RMS[i % 5]
            for name, fn in (("fadd", lambda a, b, rm: op_add(f, a, b, rm)),
                             ("fsub", lambda a, b, rm: op_add(f, a, b, rm, True)),
                             ("fmul", lambda a, b, rm: op_mul(f, a, b, rm)),
                             ("fdiv", lambda a, b, rm: op_div(f, a, b, rm))):
                r, fl = fn(a, b, rm)
                add("TEST_FFF", f"{name}.{sfx}", rm, h(r), str(fl), h(a), h(b))
            for name, mx in (("fmin", False), ("fmax", True)):
                r, fl = op_minmax(f, a, b, mx)
                add("TEST_FFF_NORM", f"{name}.{sfx}", h(r), str(fl), h(a), h(b))
            for name, k in (("feq", "eq"), ("flt", "lt"), ("fle", "le")):
                r, fl = op_cmp(f, a, b, k)
                add("TEST_XFF", f"{name}.{sfx}", h(r), str(fl), h(a), h(b))
            for name, k in (("fsgnj", "j"), ("fsgnjn", "jn"), ("fsgnjx", "jx")):
                r, fl = op_sgnj(f, a, b, k)
                add("TEST_FFF_NORM", f"{name}.{sfx}", h(r), str(fl), h(a), h(b))
        # Halfway and near-halfway sums in every mode.
        one = f.box(f.bias << f.m)
        for k in range(1, 6):
            tiny = f.box((f.bias - f.p + k - 3) << f.m)
            for rm in RMS:
                for name in ("fadd", "fsub"):
                    r, fl = op_add(f, one, tiny, rm, name == "fsub")
                    add("TEST_FFF", f"{name}.{sfx}", rm, h(r), str(fl), h(one), h(tiny))
        triples = [(rng.choice(vs), rng.choice(vs), rng.choice(vs)) for _ in range(60)]
        # Cancellation-heavy FMAs: c close to -(a * b).
        for _ in range(20):
            a, b = rng.choice(vs[5:8] + vs[-6:]), rng.choice(vs[5:8] + vs[-6:])
            r, _ = op_mul(f, a, b, "rne")
            triples.append((a, b, r ^ (1 << (f.width - 1))))
        for i, (a, b, c) in enumerate(triples):
            rm = RMS[i % 5]
            for name, np_, na in (("fmadd", 0, 0), ("fmsub", 0, 1), ("fnmsub", 1, 0), ("fnmadd", 1, 1)):
                r, fl = op_fma(f, a, b, c, rm, np_, na)
                add("TEST_FFFF", f"{name}.{sfx}", rm, h(r), str(fl), h(a), h(b), h(c))
        for i, a in enumerate(vs + [f.box((f.bias + 2 * k) << f.m | rng.getrandbits(f.m)) for k in range(-5, 6)]):
            rm = RMS[i % 5]
            r, fl = op_sqrt(f, a, rm)
            add("TEST_FF", f"fsqrt.{sfx}", rm, h(r), str(fl), h(a))
            add("TEST_XF_NORM", f"fclass.{sfx}", h(op_class(f, a)), "0", h(a))
            for kind in ("w", "wu", "l", "lu"):
                r, fl = op_cvt_fi(f, a, rm, kind)
                add("TEST_XF", f"fcvt.{kind}.{sfx}", rm, h(r), str(fl), h(a))
        # Conversions of near-integer values.
        for k in range(-3, 70):
            for frac in (0, 1, 1 << (f.m - 1), (1 << (f.m - 1)) + 1, (1 << f.m) - 1):
                a = f.box(((f.bias + k) << f.m) | frac | (rng.getrandbits(1) << (f.width - 1)))
                rm = RMS[(k + frac) % 5]
                kind = ("w", "wu", "l", "lu")[k % 4]
                r, fl = op_cvt_fi(f, a, rm, kind)
                add("TEST_XF", f"fcvt.{kind}.{sfx}", rm, h(r), str(fl), h(a))
        ints = [0, 1, M64, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0x7FFF_FFFF_FFFF_FFFF,
                0x8000_0000_0000_0000, 0x1234_5678_9ABC_DEF0, 0xFFFF_FFFF_8000_0001, 3, 0x0100_0001,
                0x20_0000_0000_0001] + [rng.getrandbits(64) for _ in range(10)]
        for i, x in enumerate(ints):
            for j, kind in enumerate(("w", "wu", "l", "lu")):
                rm = RMS[(i + j) % 5]
                r, fl = op_cvt_if(f, x, rm, kind)
                add("TEST_FX", f"fcvt.{sfx}.{kind}", rm, h(r), str(fl), h(x))
        # Moves.
        for a in vs[:8]:
            mv = a if f is D else ((a & 0xFFFF_FFFF) ^ 0x8000_0000) - 0x8000_0000
            add("TEST_XF_NORM", f"fmv.x.{'d' if f is D else 'w'}", h(mv), "0", h(a))
            add("TEST_FX_NORM", f"fmv.{'d' if f is D else 'w'}.x", h(f.box(a & ((1 << f.width) - 1))), "0", h(a))
    # Precision conversions.
    for i, a in enumerate(values(D, rng) + [D.box(((D.bias + k) << D.m) | rng.getrandbits(52)) for k in (-150, -140, -127, -126, -125, 127, 128)]):
        rm = RMS[i % 5]
        r, fl = op_cvt_ff(S, D, a, rm)
        add("TEST_FF", "fcvt.s.d", rm, h(r), str(fl), h(a))
    for i, a in enumerate(values(S, rng)):
        rm = RMS[i % 5]
        r, fl = op_cvt_ff(D, S, a, rm)
        add("TEST_FF", "fcvt.d.s", rm, h(r), str(fl), h(a))
    # Improperly boxed singles read as the canonical NaN.
    for a in (0x0000_0000_3F80_0000, 0x1234_5678_0000_0000):
        r, fl = op_add(S, a, S.box(0x3F80_0000), "rne")
        add("TEST_FFF", "fadd.s", "rne", h(r), str(fl), h(a), h(S.box(0x3F80_0000)))
        add("TEST_XF_NORM", "fclass.s", h(op_class(S, a)), "0", h(a))

    with open("tests/asm/rv64fd.S", "w") as out:
        out.write("// F and D instructions: results and fflags.\n")
        out.write("// Generated by tests/asm/gen/fp_vectors.py; do not edit.\n")
        out.write('#include "fp.h"\nSTART\n    FP_ON\n')
        for l in lines:
            out.write("    " + l + "\n")
        out.write("END\n")
    print(f"{n - 1} cases")


main()
