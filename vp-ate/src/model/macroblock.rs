//! Macroblock headers (partition 0) and residual tokens (partition 1) of key frames
//! (RFC 6386, sections 11 and 13).

use super::bool_decoder::BoolDecoder;
use super::header::FrameHeader;
use super::tables::KF_BMODE_PROB;
use super::transform::wrap16;

/// Luma prediction modes. `B` predicts each 4x4 sub-block on its own.
pub const DC_PRED: u8 = 0;
pub const V_PRED: u8 = 1;
pub const H_PRED: u8 = 2;
pub const TM_PRED: u8 = 3;
pub const B_PRED: u8 = 4;

/// Sub-block prediction modes.
pub const B_DC_PRED: u8 = 0;
pub const B_TM_PRED: u8 = 1;
pub const B_VE_PRED: u8 = 2;
pub const B_HE_PRED: u8 = 3;
pub const B_LD_PRED: u8 = 4;
pub const B_RD_PRED: u8 = 5;
pub const B_VR_PRED: u8 = 6;
pub const B_VL_PRED: u8 = 7;
pub const B_HD_PRED: u8 = 8;
pub const B_HU_PRED: u8 = 9;

const KF_YMODE_PROB: [u8; 4] = [145, 156, 163, 128];
#[rustfmt::skip]
const KF_YMODE_TREE: [i8; 8] = [-(B_PRED as i8), 2, 4, 6, -(DC_PRED as i8), -(V_PRED as i8), -(H_PRED as i8), -(TM_PRED as i8)];
const KF_UV_MODE_PROB: [u8; 3] = [142, 114, 183];
#[rustfmt::skip]
const UV_MODE_TREE: [i8; 6] = [-(DC_PRED as i8), 2, -(V_PRED as i8), 4, -(H_PRED as i8), -(TM_PRED as i8)];
#[rustfmt::skip]
pub const BMODE_TREE: [i8; 18] = [
    -(B_DC_PRED as i8), 2,
    -(B_TM_PRED as i8), 4,
    -(B_VE_PRED as i8), 6,
    8, 12,
    -(B_HE_PRED as i8), 10,
    -(B_RD_PRED as i8), -(B_VR_PRED as i8),
    -(B_LD_PRED as i8), 14,
    -(B_VL_PRED as i8), 16,
    -(B_HD_PRED as i8), -(B_HU_PRED as i8),
];

/// The sub-block mode a whole-macroblock luma mode stands for, as context for
/// neighbouring `B_PRED` macroblocks.
fn implied_bmode(y_mode: u8) -> u8 {
    match y_mode {
        DC_PRED => B_DC_PRED,
        V_PRED => B_VE_PRED,
        H_PRED => B_HE_PRED,
        _ => B_TM_PRED,
    }
}

/// A macroblock header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MbHeader {
    /// No residual tokens: every coefficient is zero.
    pub skip: bool,
    pub y_mode: u8,
    /// Sub-block modes in raster order: the `B_PRED` modes, or the mode implied by
    /// `y_mode`.
    pub b_modes: [u8; 16],
    pub uv_mode: u8,
}

/// Parses macroblock headers in raster order, keeping the sub-block mode context.
pub struct HeaderParser {
    mb_width: usize,
    /// Bottom-row sub-block modes of the macroblocks above, per column.
    above: Vec<[u8; 4]>,
    /// Right-column sub-block modes of the macroblock to the left.
    left: [u8; 4],
    col: usize,
}

impl HeaderParser {
    pub fn new(mb_width: usize) -> Self {
        HeaderParser {
            mb_width,
            above: vec![[B_DC_PRED; 4]; mb_width],
            left: [B_DC_PRED; 4],
            col: 0,
        }
    }

    pub fn parse(&mut self, hdr: &FrameHeader, bd: &mut BoolDecoder) -> MbHeader {
        let skip = hdr.prob_skip_false.is_some_and(|p| bd.read_bool(p));
        let y_mode = bd.read_tree(&KF_YMODE_TREE, &KF_YMODE_PROB);
        let mut b_modes = [implied_bmode(y_mode); 16];
        if y_mode == B_PRED {
            for i in 0..16 {
                let (x, y) = (i % 4, i / 4);
                let above = if y > 0 { b_modes[i - 4] } else { self.above[self.col][x] };
                let left = if x > 0 { b_modes[i - 1] } else { self.left[y] };
                b_modes[i] = bd.read_tree(&BMODE_TREE, &KF_BMODE_PROB[above as usize][left as usize]);
            }
        }
        let uv_mode = bd.read_tree(&UV_MODE_TREE, &KF_UV_MODE_PROB);

        self.above[self.col] = [12, 13, 14, 15].map(|i| b_modes[i]);
        self.left = [3, 7, 11, 15].map(|i| b_modes[i]);
        self.col += 1;
        if self.col == self.mb_width {
            self.col = 0;
            self.left = [B_DC_PRED; 4];
        }
        MbHeader {
            skip,
            y_mode,
            b_modes,
            uv_mode,
        }
    }
}

/// Token probability set ("plane") of each kind of block.
pub const PLANE_Y_AFTER_Y2: usize = 0;
pub const PLANE_Y2: usize = 1;
pub const PLANE_UV: usize = 2;
pub const PLANE_Y_WITH_DC: usize = 3;

pub const DCT_EOB: u8 = 11;
#[rustfmt::skip]
pub const COEFF_TREE: [i8; 22] = [
    -(DCT_EOB as i8), 2, 0, 4, -1, 6, 8, 12, -2, 10, -3, -4, 14, 16, -5, -6, 18, 20, -7, -8, -9, -10,
];
/// Coefficient band of each position in zigzag order.
pub const COEFF_BANDS: [usize; 17] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7, 0];
/// Raster position of each coefficient in zigzag order.
pub const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];
/// Extra-bit probabilities of the categories DCT_CAT1..DCT_CAT6.
pub const PCAT: [&[u8]; 6] = [
    &[159],
    &[165, 145],
    &[173, 148, 140],
    &[176, 155, 140, 135],
    &[180, 157, 141, 134, 130],
    &[254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129],
];
pub const CAT_BASE: [i32; 6] = [5, 7, 11, 19, 35, 67];

/// Dequantized coefficients of a macroblock, in raster order within each block.
/// Blocks 0-15 are luma, 16-19 U, 20-23 V and 24 is Y2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MbCoeffs {
    pub blocks: [[i32; 16]; 25],
    /// Whether the Y2 block is present (every mode but `B_PRED`).
    pub has_y2: bool,
}

/// Whether each block of the neighbouring macroblocks had non-zero coefficients:
/// the context of the first token of a block.
#[derive(Debug, Clone, Copy, Default)]
struct NonZero {
    y2: bool,
    y: [bool; 4],
    u: [bool; 2],
    v: [bool; 2],
}

/// Parses residual tokens in raster order, keeping the non-zero context.
pub struct TokenParser {
    mb_width: usize,
    above: Vec<NonZero>,
    left: NonZero,
    col: usize,
}

/// Reads the tokens of one block into `out` (dequantized, raster order). Returns
/// whether the block had any token other than an immediate end of block.
pub fn read_block(
    bd: &mut BoolDecoder,
    probs: &[[[u8; 11]; 3]; 8],
    first: usize,
    ctx: usize,
    dc_q: i32,
    ac_q: i32,
    out: &mut [i32; 16],
) -> bool {
    let mut ctx = ctx;
    let mut i = first;
    // After a zero token, the next token cannot be an end of block.
    let mut start = 0;
    while i < 16 {
        let p = &probs[COEFF_BANDS[i]][ctx];
        let token = bd.read_tree_from(&COEFF_TREE, p, start);
        if token == DCT_EOB {
            break;
        }
        let magnitude = match token {
            0 => {
                ctx = 0;
                start = 2;
                i += 1;
                continue;
            }
            1..=4 => token as i32,
            _ => {
                let cat = (token - 5) as usize;
                let extra = PCAT[cat].iter().fold(0, |v, &p| (v << 1) | bd.read_bool(p) as i32);
                CAT_BASE[cat] + extra
            }
        };
        ctx = if magnitude == 1 { 1 } else { 2 };
        start = 0;
        let value = if bd.read_flag() { -magnitude } else { magnitude };
        // Dequantized coefficients are 16-bit, as in libvpx.
        out[ZIGZAG[i]] = wrap16(value * if i == 0 { dc_q } else { ac_q });
        i += 1;
    }
    i > first
}

impl TokenParser {
    pub fn new(mb_width: usize) -> Self {
        TokenParser {
            mb_width,
            above: vec![NonZero::default(); mb_width],
            left: NonZero::default(),
            col: 0,
        }
    }

    pub fn parse(&mut self, hdr: &FrameHeader, mb: &MbHeader, bd: &mut BoolDecoder) -> MbCoeffs {
        let has_y2 = mb.y_mode != B_PRED;
        let mut out = MbCoeffs {
            blocks: [[0; 16]; 25],
            has_y2,
        };
        let above = &mut self.above[self.col];
        let left = &mut self.left;
        if mb.skip {
            // Skipped macroblocks reset the context, except Y2 for `B_PRED` (which has
            // no Y2 block).
            let (y2_above, y2_left) = (above.y2, left.y2);
            *above = NonZero::default();
            *left = NonZero::default();
            if !has_y2 {
                above.y2 = y2_above;
                left.y2 = y2_left;
            }
        } else {
            let q = &hdr.quant;
            let probs = &hdr.coeff_probs;
            if has_y2 {
                let ctx = above.y2 as usize + left.y2 as usize;
                let nz = read_block(bd, &probs[PLANE_Y2], 0, ctx, q.y2_dc as i32, q.y2_ac as i32, &mut out.blocks[24]);
                above.y2 = nz;
                left.y2 = nz;
            }
            let (plane, first) = if has_y2 { (PLANE_Y_AFTER_Y2, 1) } else { (PLANE_Y_WITH_DC, 0) };
            for i in 0..16 {
                let (x, y) = (i % 4, i / 4);
                let ctx = above.y[x] as usize + left.y[y] as usize;
                let nz = read_block(bd, &probs[plane], first, ctx, q.y_dc as i32, q.y_ac as i32, &mut out.blocks[i]);
                above.y[x] = nz;
                left.y[y] = nz;
            }
            for i in 0..8 {
                let (x, y) = (i % 2, (i / 2) % 2);
                let (a, l) = if i < 4 { (&mut above.u, &mut left.u) } else { (&mut above.v, &mut left.v) };
                let ctx = a[x] as usize + l[y] as usize;
                let nz = read_block(bd, &probs[PLANE_UV], 0, ctx, q.uv_dc as i32, q.uv_ac as i32, &mut out.blocks[16 + i]);
                a[x] = nz;
                l[y] = nz;
            }
        }
        self.col += 1;
        if self.col == self.mb_width {
            self.col = 0;
            self.left = NonZero::default();
        }
        out
    }
}
