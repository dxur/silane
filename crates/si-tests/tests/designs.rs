//! Simulates the generated Rust for each design in `hw/` against a reference model.

use silane::Sim;
use silane::typenum::{U1, U3, U4, U5, U8, U16, U32};
use si_tests::{alu, counter, fifo, misc, traffic};

/// Holds reset for one clock edge, the way a real design comes out of reset.
fn reset(sim: &mut Sim) {
    let rst = sim.input::<U1>("rst");
    sim.drive(&rst, 1);
    sim.step();
    sim.drive(&rst, 0);
}

#[test]
fn counter_counts_wraps_and_clears() {
    let circuit = counter::Counter::circuit();
    circuit.validate().unwrap();
    let mut sim = Sim::new(&circuit);
    reset(&mut sim);
    let en = sim.input::<U1>("en");
    let clear = sim.input::<U1>("clear");
    let count = sim.output::<U8>("count");
    let wrapped = sim.output::<U1>("wrapped");

    sim.drive(&en, 1);
    for expected in 1..=300u64 {
        sim.settle();
        assert_eq!(sim.read(&wrapped), (count_before(expected) == 255) as u64);
        sim.step();
        assert_eq!(sim.read(&count), expected % 256);
    }
    sim.drive(&clear, 1);
    sim.step();
    assert_eq!(sim.read(&count), 0);
    sim.drive(&clear, 0);
    sim.drive(&en, 0);
    sim.step();
    assert_eq!(sim.read(&count), 0);
}

fn count_before(next: u64) -> u64 {
    (next - 1) % 256
}

#[test]
fn counter_alias_has_its_own_width() {
    let circuit = counter::Counter4::circuit();
    let mut sim = Sim::new(&circuit);
    let en = sim.input::<U1>("en");
    let count = sim.output::<U4>("count");
    sim.drive(&en, 1);
    for _ in 0..17 {
        sim.step();
    }
    assert_eq!(sim.read(&count), 1);
}

#[test]
fn fifo_matches_a_queue() {
    use std::collections::VecDeque;

    let circuit = fifo::Fifo::circuit();
    circuit.validate().unwrap();
    let mut sim = Sim::new(&circuit);
    reset(&mut sim);
    let push = sim.input::<U1>("push");
    let pop = sim.input::<U1>("pop");
    let wdata = sim.input::<U8>("wdata");
    let rdata = sim.output::<U8>("rdata");
    let full = sim.output::<U1>("full");
    let empty = sim.output::<U1>("empty");
    let count = sim.output::<U3>("count");

    let mut model: VecDeque<u64> = VecDeque::new();
    let mut seed = 0x1234_5678u32;
    for _ in 0..2000 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let (p, q, d) = (seed & 1, (seed >> 1) & 1 & ((seed >> 2) | (seed >> 3)), (seed >> 8) & 0xFF);
        sim.drive(&push, p as u64);
        sim.drive(&pop, q as u64);
        sim.drive(&wdata, d as u64);
        sim.settle();

        assert_eq!(sim.read(&count), model.len() as u64);
        assert_eq!(sim.read(&empty), model.is_empty() as u64);
        assert_eq!(sim.read(&full), (model.len() == 4) as u64);
        if let Some(&front) = model.front() {
            assert_eq!(sim.read(&rdata), front);
        }

        let can_push = p == 1 && model.len() < 4;
        let can_pop = q == 1 && !model.is_empty();
        if can_pop {
            model.pop_front();
        }
        if can_push {
            model.push_back(d as u64);
        }
        sim.step();
    }
}

#[test]
fn fifo_with_odd_depth() {
    let circuit = fifo::Fifo3x16::circuit();
    let mut sim = Sim::new(&circuit);
    let push = sim.input::<U1>("push");
    let pop = sim.input::<U1>("pop");
    let wdata = sim.input::<U16>("wdata");
    let rdata = sim.output::<U16>("rdata");
    let full = sim.output::<U1>("full");

    sim.drive(&push, 1);
    for v in [0xAAAA, 0xBBBB, 0xCCCC, 0xDDDD] {
        sim.drive(&wdata, v);
        sim.step();
    }
    assert_eq!(sim.read(&full), 1);
    sim.drive(&push, 0);
    sim.drive(&pop, 1);
    for v in [0xAAAA, 0xBBBB, 0xCCCC] {
        sim.settle();
        assert_eq!(sim.read(&rdata), v);
        sim.step();
    }
}

#[test]
fn alu_matches_reference() {
    let circuit = alu::Alu::circuit();
    let mut sim = Sim::new(&circuit);
    let op = sim.input::<U3>("op");
    let a = sim.input::<U16>("a");
    let b = sim.input::<U16>("b");
    let y = sim.output::<U16>("y");
    let zero = sim.output::<U1>("zero");
    let carry = sim.output::<U1>("carry");

    let samples = [(0u64, 0u64), (1, 1), (0xFFFF, 1), (0x1234, 0x00F3), (0x8000, 0x8000), (7, 9)];
    for raw in 0..8 {
        let o = alu::Op::from_bits(raw).unwrap();
        for &(x, z) in &samples {
            sim.drive(&op, o.bits());
            sim.drive(&a, x);
            sim.drive(&b, z);
            sim.settle();
            let (expected, c) = match o {
                alu::Op::Add => ((x + z) & 0xFFFF, (x + z) >> 16),
                alu::Op::Sub => (x.wrapping_sub(z) & 0xFFFF, (x.wrapping_sub(z) >> 16) & 1),
                alu::Op::And => (x & z, 0),
                alu::Op::Or => (x | z, 0),
                alu::Op::Xor => (x ^ z, 0),
                alu::Op::Shl => ((x << (z & 15)) & 0xFFFF, 0),
                alu::Op::Shr => (x >> (z & 15), 0),
                alu::Op::Pass => (x, 0),
            };
            assert_eq!(sim.read(&y), expected, "{o:?} {x:#x} {z:#x}");
            assert_eq!(sim.read(&zero), (expected == 0) as u64);
            assert_eq!(sim.read(&carry), c, "carry {o:?} {x:#x} {z:#x}");
        }
    }
}

#[test]
fn traffic_light_sequence() {
    use traffic::Light;

    let circuit = traffic::TrafficLight::circuit();
    let mut sim = Sim::new(&circuit);
    reset(&mut sim);
    let request = sim.input::<U1>("request");
    let light = sim.output::<U2Light>("light");
    let walk = sim.output::<U1>("walk");

    let mut seen = Vec::new();
    for _ in 0..40 {
        let l = Light::from_bits(sim.read(&light)).unwrap();
        assert_eq!(sim.read(&walk), (l == Light::Red) as u64);
        if seen.last() != Some(&l) {
            seen.push(l);
        }
        sim.step();
    }
    assert_eq!(
        &seen[..5],
        &[Light::Red, Light::RedAmber, Light::Green, Light::Amber, Light::Red]
    );

    // A request during green moves to amber on the next edge.
    while Light::from_bits(sim.read(&light)) != Some(Light::Green) {
        sim.step();
    }
    sim.drive(&request, 1);
    sim.step();
    assert_eq!(Light::from_bits(sim.read(&light)), Some(Light::Amber));
}

type U2Light = silane::typenum::U2;

#[test]
fn pwm_duty_cycle() {
    let circuit = misc::Pwm::circuit();
    let mut sim = Sim::new(&circuit);
    reset(&mut sim);
    let duty = sim.input::<U8>("duty");
    let pwm = sim.output::<U1>("pwm");
    sim.drive(&duty, 64);
    let mut high = 0;
    for _ in 0..256 {
        sim.settle();
        high += sim.read(&pwm);
        sim.step();
    }
    assert_eq!(high, 64);
}

#[test]
fn shift_register_taps() {
    let circuit = misc::ShiftReg::circuit();
    let mut sim = Sim::new(&circuit);
    let d = sim.input::<U8>("d");
    let q = sim.output::<U8>("q");
    let taps: Vec<_> = (0..4).map(|i| sim.output::<U8>(&format!("taps_{i}"))).collect();
    for v in 1..=6u64 {
        sim.drive(&d, v * 10);
        sim.step();
    }
    let values: Vec<u64> = taps.iter().map(|t| sim.read(t)).collect();
    assert_eq!(values, vec![60, 50, 40, 30]);
    assert_eq!(sim.read(&q), 30);
}

#[test]
fn popcount_and_priority_encoder() {
    let circuit = misc::PopCount::circuit();
    let mut sim = Sim::new(&circuit);
    let x = sim.input::<U16>("x");
    let ones = sim.output::<U5>("ones");
    for v in [0u64, 1, 0xFFFF, 0x8001, 0x1234, 0xF0F0] {
        sim.drive(&x, v);
        sim.settle();
        assert_eq!(sim.read(&ones), v.count_ones() as u64);
    }

    let circuit = misc::PriorityEncoder::circuit();
    let mut sim = Sim::new(&circuit);
    let req = sim.input::<U8>("req");
    let index = sim.output::<U3>("index");
    let valid = sim.output::<U1>("valid");
    for v in 0..256u64 {
        sim.drive(&req, v);
        sim.settle();
        assert_eq!(sim.read(&valid), (v != 0) as u64);
        let expected = if v == 0 { 0 } else { 63 - v.leading_zeros() as u64 };
        assert_eq!(sim.read(&index), expected, "req {v:#b}");
    }
}

#[test]
fn swizzle() {
    let circuit = misc::Swizzle::circuit();
    let mut sim = Sim::new(&circuit);
    let x = sim.input::<U16>("x");
    sim.drive(&x, 0x12F0);
    sim.settle();
    assert_eq!(sim.read(&sim.output::<U16>("swapped")), 0xF012);
    assert_eq!(sim.read(&sim.output::<U16>("reversed")), (0x12F0u16).reverse_bits() as u64);
    assert_eq!(sim.read(&sim.output::<U32>("doubled")), 0xF0F0_F0F0);
}
