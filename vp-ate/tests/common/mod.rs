//! Helpers shared by the testbenches.
#![allow(dead_code)]

use vp_ate::model::ivf::IVFParser;

/// A small deterministic random number generator (xorshift64*).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `0..n`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// True with probability `percent / 100`.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// The first frame of a sample IVF file.
pub fn sample_frame(name: &str) -> Vec<u8> {
    let path = format!("{}/samples/{name}.ivf", env!("CARGO_MANIFEST_DIR"));
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut ivf = IVFParser::new(&data).expect("IVF header");
    ivf.next_frame().expect("IVF frame").expect("a frame").data.to_vec()
}

/// Samples the decoder supports (key frames, one partition, no segmentation).
pub const SAMPLES: &[&str] = &[
    "vp80-01-intra-1400",
    "vp80-01-intra-1411",
    "vp80-01-intra-1416",
    "vp80-01-intra-1417",
    "vp80-02-inter-1418",
    "vp80-02-inter-1424",
    "vp80-05-sharpness-1428",
    "vp80-05-sharpness-1429",
    "vp80-05-sharpness-1430",
    "vp80-05-sharpness-1431",
    "vp80-05-sharpness-1433",
    "vp80-05-sharpness-1434",
    "vp80-05-sharpness-1438",
    "vp80-05-sharpness-1439",
    "vp80-05-sharpness-1440",
    "vp80-05-sharpness-1443",
    "big-buck-bunny-1",
];
