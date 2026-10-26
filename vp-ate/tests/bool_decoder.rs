//! The hardware bool decoder against the model, with random stalls on both sides.

mod common;

use common::Rng;
use silane::Sim;
use silane::typenum::{U1, U8};
use vp_ate::bool_decoder::BoolDecoder;
use vp_ate::model::bool_decoder::BoolDecoder as Model;

fn run(seed: u64, len: usize, bools: usize) {
    let mut rng = Rng::new(seed);
    let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
    let mut model = Model::new(&data);

    let mut sim = Sim::new(&BoolDecoder::circuit());
    let clear = sim.input::<U1>("clear");
    let (in_valid, in_data, in_last) = (sim.input::<U1>("in_valid"), sim.input::<U8>("in_data"), sim.input::<U1>("in_last"));
    let (req, prob) = (sim.input::<U1>("req"), sim.input::<U8>("prob"));
    let (in_ready, ready, bit) = (sim.output::<U1>("in_ready"), sim.output::<U1>("ready"), sim.output::<U1>("bit"));

    let rst = sim.input::<U1>("rst");
    sim.drive(&rst, 1);
    sim.step();
    sim.drive(&rst, 0);
    sim.drive(&clear, 0);

    let mut pos = 0;
    let mut decoded = 0;
    let mut p = rng.next() as u8;
    let mut cycles = 0;
    while decoded < bools {
        cycles += 1;
        assert!(cycles < 10 * bools + 100, "decoder stalled");
        let offer = pos < data.len() && rng.chance(80);
        sim.drive(&in_valid, offer as u64);
        sim.drive(&in_data, data.get(pos).copied().unwrap_or(0) as u64);
        sim.drive(&in_last, (pos + 1 == data.len()) as u64);
        let ask = rng.chance(70);
        sim.drive(&req, ask as u64);
        sim.drive(&prob, p as u64);
        sim.settle();
        if offer && sim.read(&in_ready) == 1 {
            pos += 1;
        }
        if ask && sim.read(&ready) == 1 {
            assert_eq!(sim.read(&bit) == 1, model.read_bool(p), "bool {decoded} (prob {p})");
            decoded += 1;
            p = match rng.below(4) {
                0 => 128,
                1 => 1 + rng.below(255) as u8,
                _ => rng.next() as u8,
            };
        }
        sim.step();
    }
}

#[test]
fn matches_model() {
    for seed in 0..4 {
        run(seed, 4000, 20_000);
    }
}

#[test]
fn reads_zeros_after_the_last_byte() {
    // Far more bools than 8 bytes hold: the tail decodes from zeros, as in the model.
    for seed in 10..14 {
        run(seed, 8, 2000);
    }
}
