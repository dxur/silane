# Building a Hardware Project

A silane hardware project is an ordinary Rust crate:

- RTL lives in `.si` files under `hw/`;
- `build.rs` compiles that RTL to Rust;
- `src/` holds the generated modules and any hand-written Rust DSL modules;
- `tests/` holds testbenches, which simulate the design against software reference
  models written in plain Rust;
- a small binary exports Verilog.

## Layout

```text
my-chip/
├── Cargo.toml
├── build.rs            # compiles hw/*.si to Rust
├── hw/                 # RTL in the .si language
│   ├── top.si
│   └── decoder.si
├── src/
│   ├── lib.rs          # includes the generated modules
│   ├── rtl.rs          # modules written directly in the Rust DSL (optional)
│   ├── model.rs        # software reference models (optional)
│   └── bin/export.rs   # writes Verilog
└── tests/
    └── decoder.rs      # testbenches: simulate and compare with the model
```

## Cargo.toml

```toml
[package]
name = "my-chip"
version = "0.1.0"
edition = "2024"

[dependencies]
silane = { path = "../silane/crates/silane" }

[build-dependencies]
silane-lang = { path = "../silane/crates/silane-lang" }
```

## build.rs

```rust
fn main() {
    // Compiles every hw/*.si file to $OUT_DIR/silane/<stem>.rs.
    silane_lang::build::compile_dir("hw");
}
```

`build.rs` behaves like this:

- **Rebuilds:** it reruns whenever a `.si` file changes, including imported ones.
- **Errors:** compile errors are printed rustc-style and fail `cargo build`.
- **Warnings:** these show up as Cargo warnings.
- **Imports:** each `.si` file is compiled on its own. Files bring each other in with
  `import "other.si";`.

## src/lib.rs

```rust
pub mod rtl;          // hand-written Rust DSL modules, if any

pub mod decoder {
    silane::include_si!("decoder");   // the stem of hw/decoder.si
}
pub mod top {
    silane::include_si!("top");
}
```

Each `.si` module becomes a struct in that Rust module:

- `Decoder::new(inputs...) -> Decoder` instantiates it inside a silane elaboration.
  The struct's fields are its outputs.
- `Decoder::circuit() -> silane::Circuit` elaborates it as a top-level circuit with
  every clock domain on one clock. Ports keep their `.si` names, and registers add the
  `clk` and `rst` inputs. `Decoder::circuit_with(bind)` lets `bind` bind its domains
  instead (see [Clock domains](./wires_and_signals.md#clock-domains)).
- Enums become Rust enums with `bits()` and `from_bits()`.
- `module Decoder16 = Decoder::<W = 16>;` exports a specific parameterisation under
  its own name.

## Hand-written Rust modules (src/rtl.rs)

Anything the language does not express goes in Rust: clocks, new kinds of
synchronizers, vendor primitives, or a block you prefer to write with the DSL. Give it
the shape of a generated module, a `new(inputs...)` constructor and one public field
per output, generic over its clock domain:

```rust
use silane::{Signal, Wire, typenum::{U1, U8}};

type W<N, D> = Wire<Signal<N, D>>;

pub struct RustCounter<D: 'static> {
    pub count: W<U8, D>,
}

impl<D> RustCounter<D> {
    pub fn new(en: W<U1, D>) -> Self {
        let count_u = Signal::<U8, D>::default();
        let count = count_u.read();
        count_u.next(en.mux(count + 1u64, count));
        RustCounter { count }
    }
}
```

Then declare it in `.si` and use it like any other module:

```rust
extern module RustCounter(en: u1) -> (count: u8) = "crate::rtl::RustCounter";

module BlinkRust(en: u1) -> (led: u1) {
    let c = RustCounter(en);
    led = c.count[7];
}
```

Library primitives come pre-declared: `import "std:cdc";` provides `Sync2`, and
`import "std:mem";` provides `Mem`, a block-RAM-shaped memory.

## Testbenches (tests/)

Testbenches are Rust tests. Simulate the generated circuit, drive it cycle by cycle,
and compare with a software model:

```rust
use silane::{Sim, typenum::{U1, U8}};
use my_chip::decoder::Decoder;

#[test]
fn matches_reference() {
    let mut sim = Sim::new(&Decoder::circuit());
    let valid = sim.input::<U1>("valid");      // handles are width-checked
    let data = sim.output::<U8>("data");

    sim.drive(&valid, 1);
    sim.settle();          // propagate combinational logic
    let now = sim.read(&data);
    sim.step();            // one rising clock edge
    // assert_eq!(..., my_chip::model::reference(...));
}
```

Useful `Sim` calls:

| Call | Effect |
|---|---|
| `drive(&input, v)` | set an input; takes effect at the next `settle` or `step` |
| `settle()` | evaluate combinational logic |
| `read(&output)` | current value of an output |
| `peek(&output)` | `settle`, then `read` |
| `step()` | one clock edge for every clock |
| `step_clock("name")` | one clock edge for one clock |
| `reset()` | put registers back to their reset values |

Hold `rst` high for one `step()` to reset the design the way hardware would.

## Exporting Verilog (src/bin/export.rs)

```rust
fn main() {
    let circuit = my_chip::top::Top::circuit();
    circuit.validate().expect("invalid circuit");
    std::fs::write("top.v", circuit.to_verilog("top")).unwrap();
}
```

`cargo run --bin export` writes `top.v`. `circuit.to_netlist_json_string()` gives
Yosys JSON for netlistsvg, and `circuit.to_dot()` gives Graphviz.

## Day-to-day commands

```text
cargo build                    # compile .si and Rust; .si errors fail the build
cargo test                     # run the testbenches
cargo run --bin export         # write Verilog
silc check hw/decoder.si       # quick diagnostics without building
silc rust hw/decoder.si        # see the generated Rust
silc verilog hw/decoder.si --top Decoder   # Verilog without a crate (no extern modules)
```

For syntax highlighting in VS Code, run `python3 editors/vscode/build_vsix.py` in the
silane repository and install the resulting `silane.vsix`.

## A typical workflow when porting a design

1. **Reference model first:** keep or write a software model in `src/model.rs`.
2. **Bottom-up RTL:** write the RTL in `hw/`, one module at a time. Each module gets a
   test in `tests/` that drives random stimulus and compares with the model.
3. **Bundles for interfaces:** use bundles for valid/ready interfaces,
   `module Stream(ready: u1) -> (valid: u1, data: u8);`, and enums for state machines.
4. **Rust for the rest:** whatever the language cannot express (clock generation,
   special cells) goes in `src/rtl.rs` behind an `extern module`.
5. **Export:** export Verilog from the top module once the tests pass.
