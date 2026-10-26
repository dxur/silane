//! Inverse transforms (RFC 6386, section 14), with libvpx's 16-bit intermediates.

/// Inverse Walsh-Hadamard transform of the Y2 block: the DC of each luma block.
pub fn iwht(input: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let a1 = input[i] + input[12 + i];
        let b1 = input[4 + i] + input[8 + i];
        let c1 = input[4 + i] - input[8 + i];
        let d1 = input[i] - input[12 + i];
        tmp[i] = wrap16(a1 + b1);
        tmp[4 + i] = wrap16(c1 + d1);
        tmp[8 + i] = wrap16(a1 - b1);
        tmp[12 + i] = wrap16(d1 - c1);
    }
    let mut out = [0i32; 16];
    for i in 0..4 {
        let r = &tmp[4 * i..4 * i + 4];
        let a1 = r[0] + r[3];
        let b1 = r[1] + r[2];
        let c1 = r[1] - r[2];
        let d1 = r[0] - r[3];
        out[4 * i] = wrap16((a1 + b1 + 3) >> 3);
        out[4 * i + 1] = wrap16((c1 + d1 + 3) >> 3);
        out[4 * i + 2] = wrap16((a1 - b1 + 3) >> 3);
        out[4 * i + 3] = wrap16((d1 - c1 + 3) >> 3);
    }
    out
}

/// Truncates to a signed 16-bit value, as libvpx stores coefficients.
pub fn wrap16(v: i32) -> i32 {
    v as i16 as i32
}

/// `x * cos(pi/8) * sqrt(2)` in the transform's fixed point.
pub fn mul_c(x: i32) -> i32 {
    x + ((x * 20091) >> 16)
}

/// `x * sin(pi/8) * sqrt(2)` in the transform's fixed point.
pub fn mul_s(x: i32) -> i32 {
    (x * 35468) >> 16
}

/// One-dimensional inverse DCT of four values.
pub fn idct4(x: [i32; 4]) -> [i32; 4] {
    let a1 = x[0] + x[2];
    let b1 = x[0] - x[2];
    let c1 = mul_s(x[1]) - mul_c(x[3]);
    let d1 = mul_c(x[1]) + mul_s(x[3]);
    [a1 + d1, b1 + c1, b1 - c1, a1 - d1]
}

/// Inverse DCT of a 4x4 block: residuals in raster order.
pub fn idct(input: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let col = idct4([input[i], input[4 + i], input[8 + i], input[12 + i]]);
        for (k, v) in col.into_iter().enumerate() {
            tmp[4 * k + i] = wrap16(v);
        }
    }
    let mut out = [0i32; 16];
    for i in 0..4 {
        let row = idct4([tmp[4 * i], tmp[4 * i + 1], tmp[4 * i + 2], tmp[4 * i + 3]]);
        for (k, v) in row.into_iter().enumerate() {
            out[4 * i + k] = wrap16((v + 4) >> 3);
        }
    }
    out
}
