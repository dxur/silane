# Circuit Elaboration & Validation

The `Circuit` struct holds the nodes (each with a bit width and an optional name),
primitive blocks, and top-level ports in declaration order.

## Elaboration Context

Run design code inside `Circuit::elaborate`:

```rust
let (result, circuit) = Circuit::elaborate(|| {
    // Hardware design logic...
});
```

Wires created inside belong to this elaboration: using one in another panics.

## Validation

`circuit.validate()` reports, in a `ValidationErrors` value:

- **undriven** nets read by a primitive or an output port;
- **multiple drivers** on one net;
- **combinational cycles** (paths through non-register primitives that loop back);
- **clock crossings**: a register or memory sampling a signal of another clock without
  being a synchronizer's first stage, and clocks used as data. Typed signals cannot
  build these; the check covers netlists built with the raw `*_dirty` APIs.

Port names must be unique; declaring a duplicate panics at elaboration.

## Exports

```rust
let verilog = circuit.to_verilog("top");          // synthesizable Verilog 2005
let json = circuit.to_netlist_json_string();      // Yosys / netlistsvg JSON
let dot = circuit.to_dot();                       // Graphviz
```
