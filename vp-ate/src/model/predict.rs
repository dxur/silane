//! Intra prediction (RFC 6386, section 12), from the edges of a block.
//!
//! Edges outside the frame take libvpx's values: the row above the frame is 127
//! (including its top-left and above-right pixels) and the column left of it is 129.

use super::macroblock::*;

/// The pixels around a block of size `N`.
#[derive(Debug, Clone, Copy)]
pub struct Edges<const N: usize> {
    pub above: [u8; N],
    pub left: [u8; N],
    pub top_left: u8,
    /// Whether the above row and the left column are inside the frame (for DC).
    pub has_above: bool,
    pub has_left: bool,
}

/// Predicts a 16x16 luma or 8x8 chroma block with a whole-block mode.
pub fn predict_block<const N: usize>(mode: u8, e: &Edges<N>) -> [[u8; N]; N] {
    let mut out = [[0u8; N]; N];
    match mode {
        DC_PRED => {
            let shift = N.trailing_zeros() as i32;
            let sum = |v: &[u8; N]| v.iter().map(|&p| p as i32).sum::<i32>();
            let dc = match (e.has_above, e.has_left) {
                (true, true) => (sum(&e.above) + sum(&e.left) + N as i32) >> (shift + 1),
                (true, false) => (sum(&e.above) + (N as i32 >> 1)) >> shift,
                (false, true) => (sum(&e.left) + (N as i32 >> 1)) >> shift,
                (false, false) => 128,
            };
            out = [[dc as u8; N]; N];
        }
        V_PRED => out = [e.above; N],
        H_PRED => {
            for (row, &l) in out.iter_mut().zip(&e.left) {
                *row = [l; N];
            }
        }
        _ => {
            for (y, row) in out.iter_mut().enumerate() {
                for (x, p) in row.iter_mut().enumerate() {
                    let v = e.left[y] as i32 + e.above[x] as i32 - e.top_left as i32;
                    *p = v.clamp(0, 255) as u8;
                }
            }
        }
    }
    out
}

fn avg2(x: u8, y: u8) -> u8 {
    ((x as u16 + y as u16 + 1) >> 1) as u8
}

fn avg3(x: u8, y: u8, z: u8) -> u8 {
    ((x as u16 + 2 * y as u16 + z as u16 + 2) >> 2) as u8
}

/// The pixels around a 4x4 sub-block: `above[0..4]`, then the four above-right.
#[derive(Debug, Clone, Copy)]
pub struct SubEdges {
    pub above: [u8; 8],
    pub left: [u8; 4],
    pub top_left: u8,
}

/// Predicts a 4x4 luma sub-block. Returns rows of pixels.
pub fn predict_sub(mode: u8, e: &SubEdges) -> [[u8; 4]; 4] {
    let a = &e.above;
    let l = &e.left;
    let p = e.top_left;
    // The edge from bottom-left to top-right: L[3], L[2], L[1], L[0], P, A[0..8].
    let edge = [l[3], l[2], l[1], l[0], p, a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
    // `E[i]` in the specification's notation, with E[4] the top-left.
    let ev = |i: usize| edge[i];
    let mut b = [[0u8; 4]; 4];
    match mode {
        B_DC_PRED => {
            let sum: u32 = a[..4].iter().chain(l).map(|&v| v as u32).sum();
            b = [[((sum + 4) >> 3) as u8; 4]; 4];
        }
        B_TM_PRED => {
            for (y, row) in b.iter_mut().enumerate() {
                for (x, v) in row.iter_mut().enumerate() {
                    *v = (l[y] as i32 + a[x] as i32 - p as i32).clamp(0, 255) as u8;
                }
            }
        }
        B_VE_PRED => {
            let row = [avg3(p, a[0], a[1]), avg3(a[0], a[1], a[2]), avg3(a[1], a[2], a[3]), avg3(a[2], a[3], a[4])];
            b = [row; 4];
        }
        B_HE_PRED => {
            let col = [avg3(p, l[0], l[1]), avg3(l[0], l[1], l[2]), avg3(l[1], l[2], l[3]), avg3(l[2], l[3], l[3])];
            for (row, v) in b.iter_mut().zip(col) {
                *row = [v; 4];
            }
        }
        B_LD_PRED => {
            for (y, row) in b.iter_mut().enumerate() {
                for (x, v) in row.iter_mut().enumerate() {
                    let i = x + y;
                    *v = if i < 6 { avg3(a[i], a[i + 1], a[i + 2]) } else { avg3(a[6], a[7], a[7]) };
                }
            }
        }
        B_RD_PRED => {
            for (y, row) in b.iter_mut().enumerate() {
                for (x, v) in row.iter_mut().enumerate() {
                    // (x, y) lies on the diagonal through E[4 - y + x].
                    let i = 4 - y + x;
                    *v = avg3(ev(i - 1), ev(i), ev(i + 1));
                }
            }
        }
        B_VR_PRED => {
            b[3][0] = avg3(ev(1), ev(2), ev(3));
            b[2][0] = avg3(ev(2), ev(3), ev(4));
            b[3][1] = avg3(ev(3), ev(4), ev(5));
            b[1][0] = b[3][1];
            b[2][1] = avg2(ev(4), ev(5));
            b[0][0] = b[2][1];
            b[3][2] = avg3(ev(4), ev(5), ev(6));
            b[1][1] = b[3][2];
            b[2][2] = avg2(ev(5), ev(6));
            b[0][1] = b[2][2];
            b[3][3] = avg3(ev(5), ev(6), ev(7));
            b[1][2] = b[3][3];
            b[2][3] = avg2(ev(6), ev(7));
            b[0][2] = b[2][3];
            b[1][3] = avg3(ev(6), ev(7), ev(8));
            b[0][3] = avg2(ev(7), ev(8));
        }
        B_VL_PRED => {
            b[0][0] = avg2(a[0], a[1]);
            b[1][0] = avg3(a[0], a[1], a[2]);
            b[2][0] = avg2(a[1], a[2]);
            b[0][1] = b[2][0];
            b[1][1] = avg3(a[1], a[2], a[3]);
            b[3][0] = b[1][1];
            b[2][1] = avg2(a[2], a[3]);
            b[0][2] = b[2][1];
            b[3][1] = avg3(a[2], a[3], a[4]);
            b[1][2] = b[3][1];
            b[2][2] = avg2(a[3], a[4]);
            b[0][3] = b[2][2];
            b[3][2] = avg3(a[3], a[4], a[5]);
            b[1][3] = b[3][2];
            b[2][3] = avg3(a[4], a[5], a[6]);
            b[3][3] = avg3(a[5], a[6], a[7]);
        }
        B_HD_PRED => {
            b[3][0] = avg2(ev(0), ev(1));
            b[3][1] = avg3(ev(0), ev(1), ev(2));
            b[2][0] = avg2(ev(1), ev(2));
            b[3][2] = b[2][0];
            b[2][1] = avg3(ev(1), ev(2), ev(3));
            b[3][3] = b[2][1];
            b[2][2] = avg2(ev(2), ev(3));
            b[1][0] = b[2][2];
            b[2][3] = avg3(ev(2), ev(3), ev(4));
            b[1][1] = b[2][3];
            b[1][2] = avg2(ev(3), ev(4));
            b[0][0] = b[1][2];
            b[1][3] = avg3(ev(3), ev(4), ev(5));
            b[0][1] = b[1][3];
            b[0][2] = avg3(ev(4), ev(5), ev(6));
            b[0][3] = avg3(ev(5), ev(6), ev(7));
        }
        _ => {
            // B_HU_PRED
            b[0][0] = avg2(l[0], l[1]);
            b[0][1] = avg3(l[0], l[1], l[2]);
            b[0][2] = avg2(l[1], l[2]);
            b[1][0] = b[0][2];
            b[0][3] = avg3(l[1], l[2], l[3]);
            b[1][1] = b[0][3];
            b[1][2] = avg2(l[2], l[3]);
            b[2][0] = b[1][2];
            b[1][3] = avg3(l[2], l[3], l[3]);
            b[2][1] = b[1][3];
            b[2][2] = l[3];
            b[2][3] = l[3];
            b[3] = [l[3]; 4];
        }
    }
    b
}
