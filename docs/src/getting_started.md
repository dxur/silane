# Getting Started

This guide elaborates a small design with the Rust DSL, validates it, simulates it and
exports Verilog. To write the same hardware in the `.si` language instead, see
[The Silane Language](./language.md).

```rust
use silane::{Circuit, Domain, Signal, Sim, Sys, typenum::U8};

fn main() {
    let (_, circuit) = Circuit::elaborate(|| {
        // Registers of the default domain are clocked by `clk` and reset by `rst`.
        Domain::<Sys>::ports("clk", "rst");
        let inc = Signal::<U8>::input_port_default("inc");

        // An unbound wire becomes a register when driven with `.next()`.
        let acc_u = Signal::<U8>::default();
        let acc = acc_u.read();
        acc_u.next(acc + inc);

        acc.output_port("acc");
    });

    // Undriven nets, multiple drivers and combinational loops are reported here.
    circuit.validate().expect("invalid circuit");

    let mut sim = Sim::new(&circuit);
    let inc = sim.input::<U8>("inc");
    let acc = sim.output::<U8>("acc");
    sim.drive(&inc, 5);
    sim.step();
    assert_eq!(sim.read(&acc), 5);

    std::fs::write("acc.v", circuit.to_verilog("acc")).unwrap();
}
```

A register is clocked by its signal's clock domain, here the default `Sys`, which the
design binds to its `clk` and `rst` input ports (asynchronous, active-high reset). See
[Wires & Signals](./wires_and_signals.md#clock-domains) for several clocks.
