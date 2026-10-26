//! Software reference model of the decoder: decodes a VP8 key frame exactly as the
//! hardware does, and as libvpx does before the loop filter.
//!
//! Supported: key frames, one token partition, no segmentation. The loop filter is not
//! applied.

pub mod bool_decoder;
pub mod header;
pub mod ivf;
pub mod macroblock;
pub mod predict;
pub mod tables;
pub mod transform;

use bool_decoder::BoolDecoder;
use header::FrameHeader;
use macroblock::{B_PRED, HeaderParser, MbCoeffs, MbHeader, TokenParser};
use predict::{Edges, SubEdges, predict_block, predict_sub};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    NotKeyFrame,
    BadStartCode,
    Unsupported(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Truncated => write!(f, "frame data is truncated"),
            Error::NotKeyFrame => write!(f, "not a key frame"),
            Error::BadStartCode => write!(f, "bad key frame start code"),
            Error::Unsupported(what) => write!(f, "unsupported feature: {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// One image plane, padded to whole macroblocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Plane {
    fn new(width: usize, height: usize) -> Self {
        Plane {
            width,
            height,
            data: vec![0; width * height],
        }
    }

    pub fn at(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.width + x]
    }

    fn set(&mut self, x: usize, y: usize, v: u8) {
        self.data[y * self.width + x] = v;
    }

    /// The `n`x`n` block with top-left corner (`x`, `y`), row by row.
    pub fn block(&self, x: usize, y: usize, n: usize) -> Vec<u8> {
        (0..n * n).map(|i| self.at(x + i % n, y + i / n)).collect()
    }

    /// Edges of the `N`x`N` block at (`x`, `y`). `right` gives the pixels above and to
    /// the right of the block (luma only).
    fn edges<const N: usize>(&self, x: usize, y: usize) -> Edges<N> {
        let has_above = y > 0;
        let has_left = x > 0;
        Edges {
            above: std::array::from_fn(|i| if has_above { self.at(x + i, y - 1) } else { 127 }),
            left: std::array::from_fn(|i| if has_left { self.at(x - 1, y + i) } else { 129 }),
            top_left: match (has_above, has_left) {
                (false, _) => 127,
                (true, false) => 129,
                (true, true) => self.at(x - 1, y - 1),
            },
            has_above,
            has_left,
        }
    }
}

/// A decoded key frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub header: FrameHeader,
    pub mb_headers: Vec<MbHeader>,
    pub mb_coeffs: Vec<MbCoeffs>,
    pub y: Plane,
    pub u: Plane,
    pub v: Plane,
}

impl Frame {
    /// Pixels of macroblock (`mbx`, `mby`) in output order: 256 luma, then 64 U and
    /// 64 V, each block row by row.
    pub fn mb_pixels(&self, mbx: usize, mby: usize) -> Vec<u8> {
        let mut out = self.y.block(mbx * 16, mby * 16, 16);
        out.extend(self.u.block(mbx * 8, mby * 8, 8));
        out.extend(self.v.block(mbx * 8, mby * 8, 8));
        out
    }

    /// The visible image as planar YUV 4:2:0 (I420).
    pub fn to_i420(&self) -> Vec<u8> {
        let (w, h) = (self.header.width as usize, self.header.height as usize);
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut out = Vec::with_capacity(w * h + 2 * cw * ch);
        for (plane, w, h) in [(&self.y, w, h), (&self.u, cw, ch), (&self.v, cw, ch)] {
            for y in 0..h {
                out.extend_from_slice(&plane.data[y * plane.width..y * plane.width + w]);
            }
        }
        out
    }
}

/// The residual of block `i` of a macroblock (luma 0-15, U 16-19, V 20-23).
pub fn residual(coeffs: &MbCoeffs, i: usize) -> [i32; 16] {
    let mut block = coeffs.blocks[i];
    if coeffs.has_y2 && i < 16 {
        block[0] = transform::iwht(&coeffs.blocks[24])[i];
    }
    transform::idct(&block)
}

/// Decodes the macroblock headers and coefficients of a frame.
pub fn parse(frame: &[u8]) -> Result<(FrameHeader, Vec<MbHeader>, Vec<MbCoeffs>), Error> {
    let (header, mut bd0) = FrameHeader::parse(frame)?;
    let mut bd1 = BoolDecoder::new(&frame[header.part1_offset()..]);
    let (mbw, mbh) = (header.mb_width as usize, header.mb_height as usize);
    let mut headers = HeaderParser::new(mbw);
    let mut tokens = TokenParser::new(mbw);
    let mut mb_headers = Vec::with_capacity(mbw * mbh);
    let mut mb_coeffs = Vec::with_capacity(mbw * mbh);
    for _ in 0..mbw * mbh {
        let mb = headers.parse(&header, &mut bd0);
        mb_coeffs.push(tokens.parse(&header, &mb, &mut bd1));
        mb_headers.push(mb);
    }
    Ok((header, mb_headers, mb_coeffs))
}

/// Decodes a key frame.
pub fn decode(frame: &[u8]) -> Result<Frame, Error> {
    let (header, mb_headers, mb_coeffs) = parse(frame)?;
    let (mbw, mbh) = (header.mb_width as usize, header.mb_height as usize);
    let mut y = Plane::new(mbw * 16, mbh * 16);
    let mut u = Plane::new(mbw * 8, mbh * 8);
    let mut v = Plane::new(mbw * 8, mbh * 8);
    for mby in 0..mbh {
        for mbx in 0..mbw {
            let i = mby * mbw + mbx;
            reconstruct_luma(&mut y, mbx, mby, mbw, &mb_headers[i], &mb_coeffs[i]);
            for (p, plane) in [&mut u, &mut v].into_iter().enumerate() {
                let (x0, y0) = (mbx * 8, mby * 8);
                let pred = predict_block::<8>(mb_headers[i].uv_mode, &plane.edges(x0, y0));
                for b in 0..4 {
                    let res = residual(&mb_coeffs[i], 16 + 4 * p + b);
                    let (bx, by) = (b % 2 * 4, b / 2 * 4);
                    for k in 0..16 {
                        let (px, py) = (bx + k % 4, by + k / 4);
                        let v = pred[py][px] as i32 + res[k];
                        plane.set(x0 + px, y0 + py, v.clamp(0, 255) as u8);
                    }
                }
            }
        }
    }
    Ok(Frame {
        header,
        mb_headers,
        mb_coeffs,
        y,
        u,
        v,
    })
}

fn reconstruct_luma(y: &mut Plane, mbx: usize, mby: usize, mbw: usize, mb: &MbHeader, coeffs: &MbCoeffs) {
    let (x0, y0) = (mbx * 16, mby * 16);
    let edges = y.edges::<16>(x0, y0);
    if mb.y_mode != B_PRED {
        let pred = predict_block::<16>(mb.y_mode, &edges);
        for b in 0..16 {
            let res = residual(coeffs, b);
            let (bx, by) = (b % 4 * 4, b / 4 * 4);
            for k in 0..16 {
                let (px, py) = (bx + k % 4, by + k / 4);
                let v = pred[py][px] as i32 + res[k];
                y.set(x0 + px, y0 + py, v.clamp(0, 255) as u8);
            }
        }
        return;
    }
    // The four pixels above and to the right of the macroblock serve every sub-block
    // in its right column. At the right edge of the frame they repeat the last pixel
    // above; above the frame they are 127.
    let above_right: [u8; 4] = if mby == 0 {
        [127; 4]
    } else if mbx == mbw - 1 {
        [edges.above[15]; 4]
    } else {
        std::array::from_fn(|i| y.at(x0 + 16 + i, y0 - 1))
    };
    for b in 0..16 {
        let (sx, sy) = (b % 4, b / 4);
        let (bx, by) = (x0 + sx * 4, y0 + sy * 4);
        let cur = |x: usize, yy: usize, y: &Plane| y.at(x, yy);
        let above: [u8; 8] = std::array::from_fn(|i| {
            if i >= 4 && sx == 3 {
                above_right[i - 4]
            } else if sy == 0 {
                if mby == 0 { 127 } else { cur(bx + i, by - 1, y) }
            } else {
                cur(bx + i, by - 1, y)
            }
        });
        let left: [u8; 4] = std::array::from_fn(|i| {
            if sx == 0 { edges.left[sy * 4 + i] } else { cur(bx - 1, by + i, y) }
        });
        let top_left = match (sx, sy) {
            (0, 0) => edges.top_left,
            (_, 0) => edges.above[sx * 4 - 1],
            (0, _) => edges.left[sy * 4 - 1],
            _ => cur(bx - 1, by - 1, y),
        };
        let pred = predict_sub(mb.b_modes[b], &SubEdges { above, left, top_left });
        let res = residual(coeffs, b);
        for k in 0..16 {
            let v = pred[k / 4][k % 4] as i32 + res[k];
            y.set(bx + k % 4, by + k / 4, v.clamp(0, 255) as u8);
        }
    }
}
