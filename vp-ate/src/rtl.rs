//! Constant tables as block-RAM ROMs, built from the model's tables so hardware and
//! model share one source. Each has a registered read port: `rdata` is the word at
//! the `raddr` of the previous cycle. The `.si` side drives `raddr` from a register.
//! Each works in whatever clock domain `D` its address is in.

use crate::model::header::Quant;
use crate::model::tables::{COEFF_UPDATE_PROBS, DEFAULT_COEFF_PROBS, KF_BMODE_PROB};
use silane::mem::Rom;
use silane::typenum::{U1, U7, U11, U16, U27, U40, U32};
use silane::{Signal, Wire};

type W<N, D> = Wire<Signal<N, D>>;

fn always<D: 'static>() -> W<U1, D> {
    Signal::<U1, D>::constant_default(1)
}

/// Coefficient probabilities in header order (plane, band, context, node): the
/// default in bits 15..8, the update probability in bits 7..0.
pub struct CoeffRom<D: 'static> {
    pub rdata: W<U16, D>,
}

impl<D> CoeffRom<D> {
    pub fn new(raddr: W<U11, D>) -> Self {
        let words: Vec<u64> = DEFAULT_COEFF_PROBS
            .as_flattened()
            .as_flattened()
            .as_flattened()
            .iter()
            .zip(COEFF_UPDATE_PROBS.as_flattened().as_flattened().as_flattened())
            .map(|(&d, &u)| (d as u64) << 8 | u as u64)
            .collect();
        CoeffRom {
            rdata: Rom::new(U16::default(), &words, always(), raddr).rdata,
        }
    }
}

/// Dequantization factors by index: the DC factor in bits 26..18, the AC factor in bits
/// 17..9 and the Y2 AC factor in bits 8..0 (already scaled by 155/100).
pub struct QuantRom<D: 'static> {
    pub rdata: W<U27, D>,
}

/// The ROM word for quantizer index `q`.
pub fn quant_word(q: u8) -> u64 {
    let f = Quant::new(q, [0; 5]);
    (f.y_dc as u64) << 18 | (f.y_ac as u64) << 9 | f.y2_ac as u64
}

impl<D> QuantRom<D> {
    pub fn new(raddr: W<U7, D>) -> Self {
        let words: Vec<u64> = (0..128).map(quant_word).collect();
        QuantRom {
            rdata: Rom::new(U27::default(), &words, always(), raddr).rdata,
        }
    }
}

/// Sub-block mode probabilities for (above, left) at address `above * 10 + left`: tree
/// nodes 0-4 in `lo` (node 0 in bits 7..0), nodes 5-8 in `hi`.
pub struct BmodeRom<D: 'static> {
    pub lo: W<U40, D>,
    pub hi: W<U32, D>,
}

impl<D> BmodeRom<D> {
    pub fn new(raddr: W<U7, D>) -> Self {
        let pack = |probs: &[u8]| probs.iter().rev().fold(0u64, |w, &p| w << 8 | p as u64);
        let rows = KF_BMODE_PROB.as_flattened();
        let lo: Vec<u64> = rows.iter().map(|p| pack(&p[..5])).collect();
        let hi: Vec<u64> = rows.iter().map(|p| pack(&p[5..])).collect();
        BmodeRom {
            lo: Rom::new(U40::default(), &lo, always(), raddr).rdata,
            hi: Rom::new(U32::default(), &hi, always(), raddr).rdata,
        }
    }
}
