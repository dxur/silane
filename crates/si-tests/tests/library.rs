//! Library-defined primitives bound with `extern module`, and modules as types.

use silane::typenum::{U1, U8};
use silane::{Domain, Sim};
use si_tests::button::{Presses, TapTop};

#[test]
fn synchronized_presses_are_counted() {
    let circuit = Presses::circuit();
    circuit.validate().unwrap();
    let mut sim = Sim::new(&circuit);
    let button = sim.input::<U1>("button");
    let count = sim.output::<U8>("count");
    for press in 1..=5u64 {
        sim.drive(&button, 1);
        for _ in 0..4 {
            sim.step();
        }
        sim.drive(&button, 0);
        for _ in 0..4 {
            sim.step();
        }
        assert_eq!(sim.read(&count), press);
    }
}

#[test]
fn synchronizer_delays_by_two_cycles() {
    let circuit = Presses::circuit();
    let mut sim = Sim::new(&circuit);
    let button = sim.input::<U1>("button");
    let count = sim.output::<U8>("count");
    sim.drive(&button, 1);
    sim.step(); // first stage
    sim.step(); // second stage: synchronized
    assert_eq!(sim.read(&count), 0);
    sim.step(); // the counter sees the edge
    assert_eq!(sim.read(&count), 1);
}

/// `Presses` has two clock domains, inferred from its `Sync2`: the button's and the
/// counter's. The button can come from anywhere, even another clock.
#[test]
fn presses_from_another_clock_domain() {
    struct Panel;
    struct Core;
    let circuit = Presses::<Panel, Core>::circuit_with(|| {
        Domain::<Panel>::ports("panel_clk", "panel_rst");
        Domain::<Core>::ports("clk", "rst");
    });
    circuit.validate().unwrap();
    let mut sim = Sim::new(&circuit);
    let button = sim.input::<U1>("button");
    let count = sim.output::<U8>("count");
    sim.drive(&button, 1);
    for _ in 0..3 {
        sim.step_clock("clk");
    }
    assert_eq!(sim.read(&count), 1);
}

#[test]
fn module_typed_ports_use_an_existing_instance() {
    let circuit = TapTop::circuit();
    let mut sim = Sim::new(&circuit);
    let (high, count) = (sim.output::<U1>("high"), sim.output::<U8>("count"));
    for _ in 0..130 {
        sim.step();
    }
    assert_eq!((sim.read(&count), sim.read(&high)), (130, 1));
    for _ in 0..71 {
        sim.step();
    }
    // The tap clears the counter when it reaches 200.
    assert_eq!(sim.read(&count), 0);
}

#[test]
fn memory_reads_are_registered() {
    use si_tests::memory::Chase;
    use silane::typenum::U4;

    let circuit = Chase::circuit();
    circuit.validate().unwrap();
    let mut sim = Sim::new(&circuit);
    let (we, waddr, wdata) = (sim.input::<U1>("we"), sim.input::<U4>("waddr"), sim.input::<U4>("wdata"));
    let (start, first, addr) = (sim.input::<U1>("start"), sim.input::<U4>("first"), sim.output::<U4>("addr"));

    // A chain 3 -> 9 -> 5 -> 3.
    sim.drive(&we, 1);
    for (a, d) in [(3, 9), (9, 5), (5, 3)] {
        sim.drive(&waddr, a);
        sim.drive(&wdata, d);
        sim.step();
    }
    sim.drive(&we, 0);
    sim.drive(&start, 1);
    sim.drive(&first, 3);
    assert_eq!(sim.peek(&addr), 3);
    sim.step();
    sim.drive(&start, 0);
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.push(sim.peek(&addr));
        sim.step();
    }
    assert_eq!(seen, [9, 5, 3, 9]);
}

#[test]
fn memory_parameters_reach_the_library() {
    use si_tests::memory::Ram12;

    let circuit = Ram12::circuit();
    let verilog = circuit.to_verilog("ram12");
    assert!(verilog.contains("_mem [0:999];"), "{verilog}");
    assert_eq!(circuit.width(circuit.port("raddr").unwrap().net), 10);
}
