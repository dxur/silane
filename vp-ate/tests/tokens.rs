//! The token decoder against the model: the dequantized coefficients of every
//! macroblock, on real frames, with random stalls on every interface.

mod common;

use common::{Rng, SAMPLES, sample_frame};
use silane::Sim;
use silane::typenum::*;
use vp_ate::model;
use vp_ate::model::macroblock::ZIGZAG;
use vp_ate::tokens::Tokens;

fn check(name: &str, seed: u64) {
    let frame = sample_frame(name);
    let (hdr, mbs, coeffs) = model::parse(&frame).unwrap();
    let p1 = &frame[hdr.part1_offset()..];
    let mut rng = Rng::new(seed);

    let mut sim = Sim::new(&Tokens::circuit());
    let i = |sim: &Sim, n: &str| sim.input::<usize>(n);
    let rst = i(&sim, "rst");
    let (p1_valid, p1_data, p1_last) = (i(&sim, "p1_valid"), i(&sim, "p1_data"), i(&sim, "p1_last"));
    let p1_ready = sim.output::<U1>("p1_ready");
    let (prob_we, prob_addr, prob_ctx) = (i(&sim, "prob_we"), i(&sim, "prob_addr"), i(&sim, "prob_ctx"));
    let prob_data: Vec<_> = (0..11).map(|k| i(&sim, &format!("prob_data_{k}"))).collect();
    let (hdr_valid, hdr_ready) = (i(&sim, "hdr_valid"), sim.output::<U1>("hdr_ready"));
    let (h_skip, h_y, h_uv) = (i(&sim, "hdr_skip"), i(&sim, "hdr_y_mode"), i(&sim, "hdr_uv_mode"));
    let h_b: Vec<_> = (0..16).map(|k| i(&sim, &format!("hdr_b_modes_{k}"))).collect();
    let (coef_we, coef_row, coef_addr, coef_data) =
        (sim.output::<U1>("coef_we"), sim.output::<U2>("coef_row"), sim.output::<U8>("coef_addr"), sim.output::<U16>("coef_data"));
    let (eob_we, eob_addr, eob) = (sim.output::<U1>("coef_eob_we"), sim.output::<U6>("coef_eob_addr"), sim.output::<U5>("coef_eob"));
    let (mb_valid, mb_ready, mb_skip) = (sim.output::<U1>("mb_valid"), i(&sim, "mb_ready"), sim.output::<U1>("mb_hdr_skip"));

    sim.drive(&rst, 1);
    sim.step();
    sim.drive(&rst, 0);

    // Frame header results, as the header parser would provide them.
    for (plane, bands) in hdr.coeff_probs.iter().enumerate() {
        for (band, ctxs) in bands.iter().enumerate() {
            for (ctx, probs) in ctxs.iter().enumerate() {
                sim.drive(&prob_we, 1);
                sim.drive(&prob_addr, (plane * 8 + band) as u64);
                sim.drive(&prob_ctx, ctx as u64);
                for (port, &p) in prob_data.iter().zip(probs) {
                    sim.drive(port, p as u64);
                }
                sim.step();
            }
        }
    }
    sim.drive(&prob_we, 0);
    let q = &hdr.quant;
    for (port, v) in [("quant_y_dc", q.y_dc), ("quant_y_ac", q.y_ac), ("quant_y2_dc", q.y2_dc), ("quant_y2_ac", q.y2_ac), ("quant_uv_dc", q.uv_dc), ("quant_uv_ac", q.uv_ac)] {
        sim.drive(&i(&sim, port), v as u64);
    }
    sim.drive(&i(&sim, "mb_w"), hdr.mb_width as u64);
    sim.drive(&i(&sim, "mb_h"), hdr.mb_height as u64);
    sim.drive(&i(&sim, "info_valid"), 1);

    // The coefficient buffer, as written.
    let mut buf = vec![[0i32; 16]; 64];
    let mut eobs = [0usize; 64];
    let mut bank = 0;
    let (mut pos, mut sent, mut done) = (0, 0, 0);
    let mut cycles = 0;
    while done < mbs.len() {
        cycles += 1;
        assert!(cycles < 5_000_000, "{name}: stuck after {done} macroblocks");
        let offer = pos < p1.len() && rng.chance(85);
        sim.drive(&p1_valid, offer as u64);
        sim.drive(&p1_data, p1.get(pos).copied().unwrap_or(0) as u64);
        sim.drive(&p1_last, (pos + 1 == p1.len()) as u64);
        let have_hdr = sent < mbs.len() && rng.chance(80);
        sim.drive(&hdr_valid, have_hdr as u64);
        if let Some(m) = mbs.get(sent) {
            sim.drive(&h_skip, m.skip as u64);
            sim.drive(&h_y, m.y_mode as u64);
            sim.drive(&h_uv, m.uv_mode as u64);
            for (port, &b) in h_b.iter().zip(&m.b_modes) {
                sim.drive(port, b as u64);
            }
        }
        let take_mb = rng.chance(60);
        sim.drive(&mb_ready, take_mb as u64);
        sim.settle();
        if offer && sim.read(&p1_ready) == 1 {
            pos += 1;
        }
        if have_hdr && sim.read(&hdr_ready) == 1 {
            sent += 1;
        }
        if sim.read(&coef_we) == 1 {
            let addr = sim.read(&coef_addr) as usize;
            let raster = sim.read(&coef_row) as usize * 4 + (addr & 3);
            buf[addr >> 2][raster] = sim.read(&coef_data) as u16 as i16 as i32;
        }
        if sim.read(&eob_we) == 1 {
            eobs[sim.read(&eob_addr) as usize] = sim.read(&eob) as usize;
        }
        if take_mb && sim.read(&mb_valid) == 1 {
            let want = &coeffs[done];
            let skip = sim.read(&mb_skip) == 1;
            assert_eq!(skip, mbs[done].skip);
            let has_y2 = mbs[done].y_mode != model::macroblock::B_PRED;
            for b in 0..25 {
                if b == 24 && !has_y2 {
                    continue;
                }
                let first = if b < 16 && has_y2 { 1 } else { 0 };
                let slot = bank * 32 + b;
                let got: [i32; 16] = std::array::from_fn(|p| {
                    let zz = ZIGZAG.iter().position(|&r| r == p).unwrap();
                    if skip || zz < first || zz >= eobs[slot] { 0 } else { buf[slot][p] }
                });
                assert_eq!(got, want.blocks[b], "{name}: macroblock {done}, block {b}");
            }
            bank ^= 1;
            done += 1;
        }
        sim.step();
    }
    println!("{name}: {} macroblocks in {cycles} cycles, {} bytes", mbs.len(), p1.len());
}

#[test]
fn samples() {
    for (i, name) in SAMPLES.iter().enumerate() {
        check(name, i as u64);
    }
}
