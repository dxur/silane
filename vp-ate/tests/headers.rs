//! The partition 0 parser against the model: frame header fields, coefficient
//! probabilities and every macroblock header, on real frames.

mod common;

use common::{Rng, SAMPLES, sample_frame};
use silane::Sim;
use silane::typenum::*;
use vp_ate::headers::Headers;
use vp_ate::model;

fn check(name: &str, seed: u64) {
    let frame = sample_frame(name);
    let (hdr, mbs, _) = model::parse(&frame).unwrap();
    let p0 = &frame[..hdr.part1_offset()];
    let mut rng = Rng::new(seed);

    let mut sim = Sim::new(&Headers::circuit());
    let rst = sim.input::<U1>("rst");
    let (p0_valid, p0_data, frame_done, hdr_ready) =
        (sim.input::<U1>("p0_valid"), sim.input::<U8>("p0_data"), sim.input::<U1>("frame_done"), sim.input::<U1>("hdr_ready"));
    let p0_ready = sim.output::<U1>("p0_ready");
    let (error, info_valid) = (sim.output::<U1>("error"), sim.output::<U1>("info_valid"));
    let (prob_we, prob_addr, prob_ctx) = (sim.output::<U1>("prob_we"), sim.output::<U5>("prob_addr"), sim.output::<U2>("prob_ctx"));
    let prob_data: Vec<_> = (0..11).map(|k| sim.output::<U8>(&format!("prob_data_{k}"))).collect();
    let hdr_valid = sim.output::<U1>("hdr_valid");
    let (skip, y_mode, uv_mode) = (sim.output::<U1>("hdr_skip"), sim.output::<U3>("hdr_y_mode"), sim.output::<U2>("hdr_uv_mode"));
    let b_modes: Vec<_> = (0..16).map(|k| sim.output::<U4>(&format!("hdr_b_modes_{k}"))).collect();

    sim.drive(&rst, 1);
    sim.step();
    sim.drive(&rst, 0);
    sim.drive(&frame_done, 0);

    let mut probs = [[[[0u8; 11]; 3]; 8]; 4];
    let mut pos = 0;
    let mut n = 0;
    let mut cycles = 0;
    while n < mbs.len() {
        cycles += 1;
        assert!(cycles < 2_000_000, "{name}: stuck after {n} macroblock headers");
        let offer = pos < p0.len() && rng.chance(90);
        sim.drive(&p0_valid, offer as u64);
        sim.drive(&p0_data, p0.get(pos).copied().unwrap_or(0) as u64);
        let ready = rng.chance(70);
        sim.drive(&hdr_ready, ready as u64);
        sim.settle();
        assert_eq!(sim.read(&error), 0, "{name}: error");
        if offer && sim.read(&p0_ready) == 1 {
            pos += 1;
        }
        if sim.read(&prob_we) == 1 {
            let a = sim.read(&prob_addr) as usize;
            let c = sim.read(&prob_ctx) as usize;
            for (k, port) in prob_data.iter().enumerate() {
                probs[a >> 3][a & 7][c][k] = sim.read(port) as u8;
            }
        }
        if ready && sim.read(&hdr_valid) == 1 {
            assert_eq!(sim.read(&info_valid), 1);
            let want = &mbs[n];
            let got = model::macroblock::MbHeader {
                skip: sim.read(&skip) == 1,
                y_mode: sim.read(&y_mode) as u8,
                b_modes: std::array::from_fn(|k| sim.read(&b_modes[k]) as u8),
                uv_mode: sim.read(&uv_mode) as u8,
            };
            assert_eq!(&got, want, "{name}: macroblock {n}");
            n += 1;
        }
        sim.step();
    }
    assert_eq!(probs, hdr.coeff_probs, "{name}: coefficient probabilities");
    let q = &hdr.quant;
    for (port, want) in [
        ("quant_y_dc", q.y_dc),
        ("quant_y_ac", q.y_ac),
        ("quant_y2_dc", q.y2_dc),
        ("quant_y2_ac", q.y2_ac),
        ("quant_uv_dc", q.uv_dc),
        ("quant_uv_ac", q.uv_ac),
    ] {
        assert_eq!(sim.read(&sim.output::<U9>(port)), want as u64, "{name}: {port}");
    }
    assert_eq!(sim.read(&sim.output::<U14>("width")), hdr.width as u64);
    assert_eq!(sim.read(&sim.output::<U14>("height")), hdr.height as u64);
    assert_eq!(sim.read(&sim.output::<U7>("mb_w")), hdr.mb_width as u64);
    assert_eq!(sim.read(&sim.output::<U11>("mb_h")), hdr.mb_height as u64);
    assert_eq!(sim.read(&sim.output::<U20>("part1_offset")), hdr.part1_offset() as u64);
    // All of partition 0 is consumed.
    for _ in 0..100 {
        sim.drive(&p0_valid, (pos < p0.len()) as u64);
        sim.drive(&p0_data, p0.get(pos).copied().unwrap_or(0) as u64);
        sim.settle();
        if pos < p0.len() && sim.read(&p0_ready) == 1 {
            pos += 1;
        }
        sim.step();
    }
    assert_eq!(pos, p0.len(), "{name}: partition 0 not drained");
    println!("{name}: {} macroblocks in {cycles} cycles", mbs.len());
}

#[test]
fn samples() {
    for (i, name) in SAMPLES.iter().enumerate() {
        check(name, i as u64);
    }
}
