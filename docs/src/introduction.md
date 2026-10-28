# Introduction

**Silane HCL** is a lightweight, strongly typed Hardware Construction Library (HCL) written in Rust. It enables designers to synthesize gate-level hardware netlists using expressive Rust idioms.

Hardware can also be written in **the Silane language** (`.si` files), which compiles to the same Rust DSL and adds compile-time checking of widths, latches and combinational loops. See [The Silane Language](./language.md).

## Key Features

- **Clock Domain Safety**: A signal's clock domain is part of its type, so logic only combines signals of one domain and crossing domains takes a synchronizer (`silane::cdc`).
- **Typestate Wire Safety**: Distinguishes between `UnBound` (unassigned drivers) and `Bound` (readable physical nets) wires.
- **Static Widths**: Bit widths are typenum types, so width mismatches are compile errors.
- **Rich Operator Suite**: Arithmetic, bitwise, shift and comparison operators, slicing, extension, concatenation, multiplexing.
- **Graph Validation**: `validate()` finds undriven nets, multiple drivers and combinational loops.
- **Simulation & Export**: A cycle-accurate simulator, Verilog, Yosys JSON and Graphviz output.
