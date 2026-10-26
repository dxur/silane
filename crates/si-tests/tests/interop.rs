//! Generated modules are ordinary Rust: they can be mixed with hand-written silane DSL.

use silane::typenum::{U1, U8};
use silane::{Circuit, Domain, Signal, Sim, Sys, when};
use si_tests::{counter::Counter, fifo::Fifo};

#[test]
fn instantiate_si_modules_from_rust() {
    let (_, circuit) = Circuit::elaborate(|| {
        // The caller binds the clock domain the modules' registers run in.
        Domain::<Sys>::ports("clk", "rst");
        let go = Signal::<U1>::input_port_default("go");
        // A .si counter drives a .si FIFO; glue logic is plain Rust DSL.
        let counter = Counter::new(go, Signal::constant_default(0));
        let fifo = Fifo::new(go & !counter.wrapped, Signal::constant_default(0), counter.count);
        let doubled = fifo.rdata + fifo.rdata;
        let flag = when!(fifo.full => Signal::<U8>::constant_default(0xFF), else => doubled);
        flag.output_port("out");
        fifo.count.output_port("count");
    });
    circuit.validate().unwrap();

    let mut sim = Sim::new(&circuit);
    let go = sim.input::<U1>("go");
    let out = sim.output::<U8>("out");
    sim.drive(&go, 1);
    sim.step();
    sim.settle();
    // The first value pushed is the counter's reset value 0; the FIFO is not full yet.
    assert_eq!(sim.read(&out), 0);
    for _ in 0..4 {
        sim.step();
    }
    assert_eq!(sim.read(&out), 0xFF);
}

#[test]
fn generated_verilog_names_signals() {
    let verilog = Fifo::circuit().to_verilog("fifo");
    assert!(verilog.contains("module fifo ("));
    assert!(verilog.contains("wr_ptr_w"));
    assert!(verilog.contains("mem_0__w") || verilog.contains("mem_0_w"));
}
