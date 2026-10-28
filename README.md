<div align="center">

# Silane

**A small, strongly typed hardware description language, and the Rust library underneath it.**

Write hardware in `.si`. Have widths, latches and combinational loops checked at compile time.
Simulate it natively, export Verilog, and boot Linux on a CPU you wrote in it.

[Language](docs/src/language.md) · [Getting started](docs/src/getting_started.md) · [Project setup](docs/src/project.md) · [rv-olt](rv-olt/README.md) · [vp-ate](vp-ate/README.md)

</div>

---

## A taste

```rust
// A counter with enable and synchronous clear.
module Counter<W = 8>(en: u1, clear: u1) -> (count: u<W>, wrapped: u1) {
    let value: u<W>;

    if clear {
        value <= 0;
    } else if en {
        value <= value + 1;
    }

    count = value;
    wrapped = en & value.all();
}

module Counter4 = Counter::<W = 4>;
```

`<=` makes `value` a register, `=` is a plain wire. There are no clocks in the source:
registers belong to a clock domain that you bind when you build the circuit.

A module is a function from inputs to outputs. The compiler elaborates it completely and
checks, before any Rust is generated:

- every width, and that every literal fits;
- that every wire is driven on every path (no accidental latches);
- that there are no combinational loops, even through module instances;
- that `match` expressions are exhaustive;
- that bundle fields are driven in the right direction.

## What's in the box

| Path | What |
|---|---|
| [`crates/silane`](crates/silane) | The hardware construction library: typed `Signal`/`Wire`, clock domains and CDC, a cycle-accurate simulator (native C backend, interpreter fallback), `Mem`, and Verilog / Yosys JSON / Graphviz export |
| [`crates/silane-lang`](crates/silane-lang) | The `.si` compiler: library for `build.rs`, plus the `silc` command-line tool |
| [`crates/si-tests`](crates/si-tests) | Example designs (ALU, FIFO, memory, streams, a traffic light, a debounced button) and the simulation tests that exercise them |
| [`docs`](docs/src) | The book: language reference, DSL guide, project setup |
| [`editors/vscode`](editors/vscode) | Syntax highlighting for `.si` |
| [`rv-olt`](rv-olt) | An RV64GC core and SoC that boots OpenSBI and Linux in simulation |
| [`vp-ate`](vp-ate) | A VP8 key-frame decoder, bit-exact with ffmpeg on the conformance samples |

## Showcase

### rv-olt: Linux on a CPU written in silane

A five-stage in-order RV64IMAFDC core (M/S/U modes, Sv39 MMU, hardware FPU) and a
QEMU `virt`-style SoC with CLINT, PLIC and an ns16550a UART. Stock Debian OpenSBI and
kernel images boot on it unmodified. There is no software reference model; the hardware
is checked by self-checking assembly programs running on it, including 8,779 exact F/D
cases.

```sh
cd rv-olt
cargo test --release --test isa
```

See the [rv-olt README](rv-olt/README.md) for how to boot Linux.

### vp-ate: a VP8 decoder in 11k LUTs

A key-frame decoder (up to 1920 pixels wide) that emits four pixels per cycle, with the
arithmetic decoder running at one bool per cycle. It reaches about 57 MHz on an ECP5-85
(Yosys + nextpnr) in 11.5k LUT4, 12 block RAMs and 11 multipliers, and matches ffmpeg
bit for bit.

```sh
cd vp-ate
cargo run --release --bin decode -- in.ivf out.yuv   # the software model
cargo run --release --bin export                      # writes vp_ate.v
```

See the [vp-ate README](vp-ate/README.md).

## Getting started

You need a recent stable Rust toolchain (edition 2024) and a C compiler, which the native
simulator uses (set `SILANE_SIM=interp` to interpret instead).

```sh
git clone https://github.com/dxur/silane.git && cd silane
cargo test --release                  # the compiler and example-design tests
```

The `silc` tool checks a file and prints Rust, Verilog or a Yosys netlist:

```sh
cargo run -p silane-lang --bin silc -- check   crates/si-tests/hw/fifo.si
cargo run -p silane-lang --bin silc -- verilog crates/si-tests/hw/fifo.si --top Fifo -o fifo.v
```

To use `.si` from your own crate, compile the sources in `build.rs` and include the
generated modules:

```rust
// build.rs
fn main() { silane_lang::build::compile_dir("hw"); }

// src/lib.rs
mod counter { silane::include_si!("counter"); }

let circuit = counter::Counter::circuit();
let mut sim = silane::Sim::new(&circuit);
```

Each `.si` module becomes an ordinary Rust struct, so you can instantiate it inside
hand-written DSL code, drive it from a testbench, and export it to Verilog.
[Building a Hardware Project](docs/src/project.md) walks through the whole layout.

### Prefer Rust all the way?

The library stands on its own. Bit widths are type-level numbers, so a width mismatch is
a compile error, and `circuit.validate()` reports undriven nets, multiple drivers and
combinational loops:

```rust
use silane::{Circuit, Domain, Signal, Sim, Sys, typenum::U8};

let (_, circuit) = Circuit::elaborate(|| {
    Domain::<Sys>::ports("clk", "rst");
    let inc = Signal::<U8>::input_port_default("inc");

    let acc_u = Signal::<U8>::default();   // an unbound wire...
    let acc = acc_u.read();
    acc_u.next(acc + inc);                 // ...becomes a register when driven with `.next()`
    acc.output_port("acc");
});
circuit.validate().expect("invalid circuit");
std::fs::write("acc.v", circuit.to_verilog("acc")).unwrap();
```

## Documentation

The book lives in [`docs/src`](docs/src) (build it with `mdbook build docs`):

1. [Introduction](docs/src/introduction.md)
2. [Getting Started](docs/src/getting_started.md)
3. [Wires & Signals](docs/src/wires_and_signals.md)
4. [Operations & Primitives](docs/src/operations.md)
5. [Circuit Elaboration & Validation](docs/src/circuit_and_validation.md)
6. [The Silane Language](docs/src/language.md)
7. [Building a Hardware Project](docs/src/project.md)

## Editor support

A VS Code extension with a TextMate grammar for `.si` is in
[`editors/vscode`](editors/vscode). Build the package with
`python3 editors/vscode/build_vsix.py`, then install the resulting `silane.vsix`.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

The conformance and demo videos in `vp-ate/samples/` are not covered by this license;
they keep their original terms (the Big Buck Bunny clip is CC-BY).
