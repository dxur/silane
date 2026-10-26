//! Bundles: valid/ready streams through register slices, with random backpressure.

use silane::Sim;
use silane::typenum::{U1, U8};
use si_tests::stream::{Incr, Pipe2};

struct Rng(u32);

impl Rng {
    fn bit(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        ((self.0 >> 7) & 1) as u64
    }
}

/// Streams 0, 1, 2, ... through `circuit` with random gaps on the input and random
/// backpressure on the output. Returns the values received and the number sent.
fn transfers(circuit: &silane::Circuit, cycles: usize) -> (Vec<u64>, u64) {
    let mut sim = Sim::new(circuit);
    let rst = sim.input::<U1>("rst");
    sim.drive(&rst, 1);
    sim.step();
    sim.drive(&rst, 0);

    let in_valid = sim.input::<U1>("enq_valid");
    let in_data = sim.input::<U8>("enq_data");
    let out_ready = sim.input::<U1>("deq_ready");
    let in_ready = sim.output::<U1>("enq_ready");
    let out_valid = sim.output::<U1>("deq_valid");
    let out_data = sim.output::<U8>("deq_data");

    let mut rng = Rng(0xC0FFEE);
    let mut sent = 0u64;
    let mut received = Vec::new();
    for _ in 0..cycles {
        let valid = rng.bit();
        let ready = rng.bit() | rng.bit();
        sim.drive(&in_valid, valid);
        sim.drive(&in_data, sent & 0xFF);
        sim.drive(&out_ready, ready);
        sim.settle();
        if valid == 1 && sim.read(&in_ready) == 1 {
            sent += 1;
        }
        if ready == 1 && sim.read(&out_valid) == 1 {
            received.push(sim.read(&out_data));
        }
        sim.step();
    }
    (received, sent)
}

#[test]
fn pipe_preserves_order_under_backpressure() {
    let circuit = Pipe2::circuit();
    circuit.validate().unwrap();
    let (received, sent) = transfers(&circuit, 3000);
    assert!(sent > 1000, "the pipe should accept most of the time, sent {sent}");
    // Everything sent is received in order, except up to two items still in flight.
    assert!(sent - received.len() as u64 <= 2);
    for (i, v) in received.iter().enumerate() {
        assert_eq!(*v, i as u64 & 0xFF);
    }
}

#[test]
fn incr_is_combinational_passthrough() {
    let circuit = Incr::circuit();
    let mut sim = Sim::new(&circuit);
    let in_valid = sim.input::<U1>("enq_valid");
    let in_data = sim.input::<U8>("enq_data");
    let out_ready = sim.input::<U1>("deq_ready");
    sim.drive(&in_valid, 1);
    sim.drive(&in_data, 41);
    sim.drive(&out_ready, 1);
    sim.settle();
    assert_eq!(sim.read(&sim.output::<U8>("deq_data")), 42);
    assert_eq!(sim.read(&sim.output::<U1>("deq_valid")), 1);
    assert_eq!(sim.read(&sim.output::<U1>("enq_ready")), 1);
}
