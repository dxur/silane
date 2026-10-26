//! The whole decoder against the model: every output pixel of real frames, with the
//! host stalling at random on every stream.

mod common;

use common::{Rng, SAMPLES, sample_frame};
use silane::Sim;
use silane::typenum::*;
use vp_ate::model;
use vp_ate::top::VpAte;

/// Decodes `frames` back to back and checks them; returns the cycles taken.
fn run(sim: &mut Sim, frames: &[Vec<u8>], rng: &mut Rng, stall: u64) -> u64 {
    let (p0_valid, p0_data, p0_ready) = (sim.input::<U1>("p0_valid"), sim.input::<U8>("p0_data"), sim.output::<U1>("p0_ready"));
    let (p1_valid, p1_data, p1_last, p1_ready) =
        (sim.input::<U1>("p1_valid"), sim.input::<U8>("p1_data"), sim.input::<U1>("p1_last"), sim.output::<U1>("p1_ready"));
    let (px_valid, px_data, px_ready) = (sim.output::<U1>("px_valid"), sim.output::<U32>("px_data"), sim.input::<U1>("px_ready"));
    let (error, frame_done) = (sim.output::<U1>("error"), sim.output::<U1>("frame_done"));

    // Streams of all frames, back to back.
    let mut p0 = Vec::new();
    let mut p1 = Vec::new();
    let mut p1_ends = Vec::new();
    let mut want = Vec::new();
    for f in frames {
        let decoded = model::decode(f).unwrap();
        let off = decoded.header.part1_offset();
        p0.extend_from_slice(&f[..off]);
        p1.extend_from_slice(&f[off..]);
        p1_ends.push(p1.len() - 1);
        let (w, h) = (decoded.header.mb_width as usize, decoded.header.mb_height as usize);
        for mby in 0..h {
            for mbx in 0..w {
                let px = decoded.mb_pixels(mbx, mby);
                // Output order: blocks of 4x4, rows of 4.
                for blk in 0..24 {
                    let (plane, n, b) = if blk < 16 { (0, 16, blk) } else if blk < 20 { (256, 8, blk - 16) } else { (320, 8, blk - 20) };
                    let per = n / 4;
                    let (bx, by) = (b % per, b / per);
                    for r in 0..4 {
                        let row: Vec<u8> = (0..4).map(|c| px[plane + (by * 4 + r) * n + bx * 4 + c]).collect();
                        want.push((mbx, mby, blk, r, u32::from_le_bytes(row.try_into().unwrap())));
                    }
                }
            }
        }
    }

    let (mut i0, mut i1, mut got, mut done) = (0, 0, 0, 0);
    let mut cycles = 0;
    while got < want.len() {
        cycles += 1;
        assert!(cycles < 40 * want.len() as u64 + 100_000, "stuck after {got} of {} rows (frames done: {done})", want.len());
        let o0 = i0 < p0.len() && !rng.chance(stall);
        let o1 = i1 < p1.len() && !rng.chance(stall);
        let take = !rng.chance(stall);
        sim.drive(&p0_valid, o0 as u64);
        sim.drive(&p0_data, p0.get(i0).copied().unwrap_or(0) as u64);
        sim.drive(&p1_valid, o1 as u64);
        sim.drive(&p1_data, p1.get(i1).copied().unwrap_or(0) as u64);
        sim.drive(&p1_last, p1_ends.contains(&i1) as u64);
        sim.drive(&px_ready, take as u64);
        sim.settle();
        assert_eq!(sim.read(&error), 0, "decoder error");
        if o0 && sim.read(&p0_ready) == 1 {
            i0 += 1;
        }
        if o1 && sim.read(&p1_ready) == 1 {
            i1 += 1;
        }
        if take && sim.read(&px_valid) == 1 {
            let (mbx, mby, blk, r, row) = want[got];
            let have = sim.read(&px_data) as u32;
            assert_eq!(
                have.to_le_bytes(),
                row.to_le_bytes(),
                "macroblock ({mbx}, {mby}), block {blk}, row {r}"
            );
            got += 1;
        }
        done += sim.read(&frame_done);
        sim.step();
    }
    assert_eq!(done as usize, frames.len());
    cycles
}

fn new_sim() -> Sim {
    let mut sim = Sim::new(&VpAte::circuit());
    let rst = sim.input::<U1>("rst");
    sim.drive(&rst, 1);
    sim.step();
    sim.drive(&rst, 0);
    sim
}

#[test]
fn samples() {
    for (i, name) in SAMPLES.iter().enumerate() {
        let frame = sample_frame(name);
        let mut sim = new_sim();
        let mut rng = Rng::new(i as u64);
        let cycles = run(&mut sim, &[frame.clone()], &mut rng, 0);
        let mbs = model::decode(&frame).unwrap().mb_headers.len() as u64;
        println!("{name}: {mbs} macroblocks in {cycles} cycles ({} per macroblock)", cycles / mbs);
    }
}

#[test]
fn frames_back_to_back_with_stalls() {
    let frames: Vec<Vec<u8>> = ["vp80-01-intra-1411", "vp80-05-sharpness-1430", "vp80-02-inter-1418"]
        .iter()
        .map(|n| sample_frame(n))
        .collect();
    let mut sim = new_sim();
    let mut rng = Rng::new(99);
    run(&mut sim, &frames, &mut rng, 30);
}
