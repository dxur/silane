//! Key-frame header: the uncompressed frame tag and the first part of partition 0
//! (RFC 6386, sections 9 and 19.2).

use super::Error;
use super::bool_decoder::BoolDecoder;
use super::tables::{AC_QUANT, COEFF_UPDATE_PROBS, DC_QUANT, DEFAULT_COEFF_PROBS};

/// Coefficient probabilities, indexed `[plane][band][context][node]`.
pub type CoeffProbs = [[[[u8; 11]; 3]; 8]; 4];

/// Dequantization factors of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quant {
    pub y_dc: u16,
    pub y_ac: u16,
    pub y2_dc: u16,
    pub y2_ac: u16,
    pub uv_dc: u16,
    pub uv_ac: u16,
}

impl Quant {
    /// The factors for base index `q` and the five deltas `[y_dc, y2_dc, y2_ac, uv_dc,
    /// uv_ac]`.
    pub fn new(q: u8, deltas: [i32; 5]) -> Quant {
        let index = |delta: i32| (q as i32 + delta).clamp(0, 127) as usize;
        let [y_dc, y2_dc, y2_ac, uv_dc, uv_ac] = deltas.map(index);
        Quant {
            y_dc: DC_QUANT[y_dc] as u16,
            y_ac: AC_QUANT[q as usize] as u16,
            y2_dc: DC_QUANT[y2_dc] as u16 * 2,
            y2_ac: (AC_QUANT[y2_ac] as u32 * 155 / 100).max(8) as u16,
            uv_dc: (DC_QUANT[uv_dc] as u16).min(132),
            uv_ac: AC_QUANT[uv_ac] as u16,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FrameHeader {
    pub width: u16,
    pub height: u16,
    pub h_scale: u8,
    pub v_scale: u8,
    pub mb_width: u16,
    pub mb_height: u16,
    pub show_frame: bool,
    /// Size of partition 0 in bytes.
    pub part0_len: usize,
    /// Offset of partition 0 in the frame (after the 10-byte uncompressed header).
    pub part0_offset: usize,
    pub color_space: bool,
    pub clamping_type: bool,
    pub filter_type: bool,
    pub filter_level: u8,
    pub sharpness: u8,
    pub q_index: u8,
    pub quant: Quant,
    pub refresh_entropy_probs: bool,
    pub coeff_probs: CoeffProbs,
    /// Probability that a macroblock is not skipped, when skipping is enabled.
    pub prob_skip_false: Option<u8>,
}

impl FrameHeader {
    /// Parses the frame tag and the frame header. Returns the header and the bool
    /// decoder of partition 0, positioned at the first macroblock header.
    pub fn parse(frame: &[u8]) -> Result<(FrameHeader, BoolDecoder<'_>), Error> {
        if frame.len() < 10 {
            return Err(Error::Truncated);
        }
        let tag = frame[0] as u32 | (frame[1] as u32) << 8 | (frame[2] as u32) << 16;
        if tag & 1 != 0 {
            return Err(Error::NotKeyFrame);
        }
        if (tag >> 1) & 7 > 3 {
            return Err(Error::Unsupported("version"));
        }
        let show_frame = (tag >> 4) & 1 != 0;
        let part0_len = (tag >> 5) as usize;
        if frame[3..6] != [0x9d, 0x01, 0x2a] {
            return Err(Error::BadStartCode);
        }
        let raw_w = u16::from_le_bytes([frame[6], frame[7]]);
        let raw_h = u16::from_le_bytes([frame[8], frame[9]]);
        let (width, height) = (raw_w & 0x3fff, raw_h & 0x3fff);
        let part0_offset = 10;
        if frame.len() < part0_offset + part0_len {
            return Err(Error::Truncated);
        }

        let mut bd = BoolDecoder::new(&frame[part0_offset..part0_offset + part0_len]);
        let color_space = bd.read_flag();
        let clamping_type = bd.read_flag();
        if bd.read_flag() {
            return Err(Error::Unsupported("segmentation"));
        }
        let filter_type = bd.read_flag();
        let filter_level = bd.read_literal(6) as u8;
        let sharpness = bd.read_literal(3) as u8;
        if bd.read_flag() {
            // Loop filter deltas: parsed, but there is no loop filter.
            if bd.read_flag() {
                for _ in 0..8 {
                    bd.read_optional_signed(6);
                }
            }
        }
        if bd.read_literal(2) != 0 {
            return Err(Error::Unsupported("multiple token partitions"));
        }
        let q_index = bd.read_literal(7) as u8;
        let deltas = [(); 5].map(|_| bd.read_optional_signed(4));
        let quant = Quant::new(q_index, deltas);
        let refresh_entropy_probs = bd.read_flag();

        let mut coeff_probs = DEFAULT_COEFF_PROBS;
        for (i, plane) in coeff_probs.iter_mut().enumerate() {
            for (j, band) in plane.iter_mut().enumerate() {
                for (k, ctx) in band.iter_mut().enumerate() {
                    for (t, prob) in ctx.iter_mut().enumerate() {
                        if bd.read_bool(COEFF_UPDATE_PROBS[i][j][k][t]) {
                            *prob = bd.read_literal(8) as u8;
                        }
                    }
                }
            }
        }
        let prob_skip_false = bd.read_flag().then(|| bd.read_literal(8) as u8);

        let header = FrameHeader {
            width,
            height,
            h_scale: (raw_w >> 14) as u8,
            v_scale: (raw_h >> 14) as u8,
            mb_width: width.div_ceil(16),
            mb_height: height.div_ceil(16),
            show_frame,
            part0_len,
            part0_offset,
            color_space,
            clamping_type,
            filter_type,
            filter_level,
            sharpness,
            q_index,
            quant,
            refresh_entropy_probs,
            coeff_probs,
            prob_skip_false,
        };
        Ok((header, bd))
    }

    /// Offset of the token partition in the frame.
    pub fn part1_offset(&self) -> usize {
        self.part0_offset + self.part0_len
    }
}
