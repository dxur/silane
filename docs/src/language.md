# The Silane Language (`.si`)

Silane is a small hardware description language that compiles to the silane Rust DSL.
The compiler (`silane-lang`) fully elaborates and checks a `.si` file, then emits
ordinary Rust that builds the same circuit through the `silane` library. Because the
output is Rust, you can instantiate `.si` modules from hand-written Rust, simulate them
with `silane::Sim`, and export them to Verilog.

The compiler checks the following before any Rust is generated:

- every width;
- that every wire is driven on every path (no latches);
- that there are no combinational loops, including through module instances;
- that bundle fields are driven in the right direction;
- that `match` expressions are exhaustive;
- that every literal fits its width.

A module is a function from inputs to outputs, like a DSL function that returns a struct
of wires:

```rust
// counter.si
module Counter<W = 8>(en: u1) -> (count: u<W>) {
    if en {
        count <= count + 1;   // `<=` makes `count` a register
    }
}
```

## Using it from Rust

Add `silane-lang` as a build dependency and compile the sources in `build.rs`:

```rust
// build.rs
fn main() {
    silane_lang::build::compile_dir("hw"); // or build::compile("hw/counter.si")
}
```

Include the generated code, one Rust module per `.si` file:

```rust
mod counter {
    silane::include_si!("counter");
}
```

Each module becomes a struct holding its outputs:

```rust
// Instantiate inside any silane elaboration, mixed with hand-written DSL:
let c = counter::Counter::new(en);      // inputs in declaration order
let doubled = c.count + c.count;

// Or elaborate it as a top-level circuit (ports keep their names):
let circuit = counter::Counter::circuit();
let mut sim = silane::Sim::new(&circuit);

// ... binding its clock domains yourself (here, a two-domain module):
let circuit = button::Presses::<Panel, Core>::circuit_with(|| {
    Domain::<Panel>::ports("panel_clk", "panel_rst");
    Domain::<Core>::ports("clk", "rst");
});
```

How ports map to Rust:

- Scalars become `SiWire<U<width>, D>`, an alias of `Wire<Signal<W, D>>`, where `D` is
  one of the module's clock domains (see Clocking).
- `circuit()` puts every clock domain on one clock, `clk`/`rst`; `circuit_with(bind)`
  lets `bind` bind them (see `silane::Domain`).
- Arrays become Rust arrays.
- A bundle port is split by direction. Elements flowing in are `new()` parameters and
  elements flowing out are struct fields, each named `port_field`.
- In `circuit()`, aggregates are flattened to ports named `name_0`, `name_field`, and
  so on.
- Enums become Rust enums with `bits()` and `from_bits()`, for use in testbenches.

Build errors are printed rustc-style and fail the build. Warnings are forwarded to Cargo.

The `silc` command-line tool runs the same pipeline:

```text
silc check   file.si             # diagnostics only
silc rust    file.si -o out.rs   # generated Rust
silc verilog file.si --top Name  # Verilog, built directly from the design
silc netlist file.si --top Name  # Yosys JSON netlist
```

## Lexical structure

- Comments: `// line` and `/* block */` (block comments nest).
- Identifiers: `[A-Za-z_][A-Za-z0-9_]*`. Names starting with `_` never trigger
  unused warnings.
- Integers: `42`, `0xFF`, `0b1010`, `0o17`, with optional `_` separators. A width
  suffix makes a sized constant: `5u8`, `0xFF_u8`.
- `true` and `false` are `u1` constants.

## Types

| Type        | Meaning                                                    |
|-------------|------------------------------------------------------------|
| `u8`, `u1`… | unsigned integer of that many bits (1 to 64)               |
| `u<expr>`   | unsigned integer with a compile-time width, e.g. `u<W + 1>`|
| `[T; N]`    | array of `N` elements                                      |
| `Name`      | an enum                                                    |
| `Name<...>` | a bundle (a module without a body), or a module's interface |

There is no separate boolean type: conditions are `u1`. Arithmetic wraps at the
operand width. There are no implicit conversions between widths.

Unsized integer literals and compile-time values take the type their context needs.
The compiler reports an error if the value does not fit.

## Items

```rust
import "lib/fifo.si";               // path relative to this file

const DEPTH = 16;                   // unsized compile-time integer
const MAGIC: u8 = 0x5A;             // sized constant

enum State { Idle, Busy, Done = 5 } // encoding width inferred (here 3 bits)
enum Op: u4 { Add, Sub }            // or given explicitly

module Name<P = 4, Q = P * 2>(inputs) -> (outputs) { body }
module Stream<W = 8>(ready: u1) -> (valid: u1, data: u<W>);   // no body: a bundle
module Blink -> (led: u1) { led <= !led; }                     // empty `()` may be left out
extern module Sync2(d: u1 @src) -> (q: u1 @dst) = "::silane::cdc::Sync2"; // implemented in Rust

module Fifo16 = Fifo::<DEPTH = 16>; // export a parameterised instance
```

All items share one global namespace across imported files. Items may be used before
they are defined.

### Modules and parameters

Parameters are compile-time integers. A default may refer to earlier parameters.
Modules are elaborated once for each distinct set of parameter values. Every
module whose parameters all have defaults is emitted under its own name. Other
instances get a mangled name, e.g. `Fifo_DEPTH16_WIDTH8`, or the name of an alias
(`module Fifo16 = ...`).

A module with required parameters that is never instantiated is not checked, and
produces a warning.

### Ports

The argument list holds the inputs. The `-> (...)` list holds the outputs:

```rust
module Example(a: u8, data: [u8; 4]) -> (y: u8, count: u4 reset 3) { ... }
```

- Inside the body, inputs are read-only. Outputs are nets you drive, exactly like
  `let x: T;` below, so an output becomes a wire or a register depending on how it is
  driven.
- `reset v` gives the value an output takes on reset if it is a register.
- `()` can be left out when there are no inputs, and `-> (...)` when there are no
  outputs.

**Clocking.** Clocks are implicit: there is no clock type and there are no clock
statements. The compiler infers a module's clock domains from how its signals connect:
logic, and a register and the value it samples, are in one domain; ports that nothing
connects are free to be in different ones; a synchronizer (an extern module whose
ports have different domain labels, such as `Sync2`) separates its two sides. Every
module is generic over its domains, as many as its ports allow: a module whose ports
form two groups becomes `M<D0, D1>` in Rust, and whoever instantiates it chooses the
clocks, checked by rustc. A design never mixes domains without a synchronizer, and a
signal that never reaches a register needs no clock at all. `clk` and `rst` are
reserved port names, used by `circuit()`.

## Nets and values

```rust
let x = a + b;            // an immutable name for a value (compile-time or hardware)
let y: u8 = 3;            // with a type annotation
let n: u8;                // a net, driven later
let s: State reset State::Idle;   // a net with a reset value (if it becomes a register)
let mem: [u8; 16];        // arrays: one reset value for every element
let q = Fifo(push, pop, d);       // an instance (see below)
let r: Fifo;                      // an instance, with its inputs connected later
```

A net's kind comes from how it is driven, just as in the DSL, where an unbound wire
becomes a register with `.next()` and a wire with `.assign()`:

- `n = expr;` makes `n` a **wire** (combinational).
- `n <= expr;` makes `n` a **register**. The value is its next state, taken at the
  clock edge.

A net driven with both `=` and `<=` is an error, and so is a reset value on a wire.
Names cannot be shadowed within a module. Nets and instances cannot be declared inside
a hardware `if`/`match`; declare them before it and drive them inside.

## Assignment semantics

- **Last assignment wins.** Statements are read top to bottom, and a later assignment
  overrides an earlier one on the paths where both happen. This gives the usual
  "default, then override" style:

  ```rust
  next = state;
  if go { next = State::Busy; }
  ```

- **Wires must be driven on every path.** If some path through the `if`/`match`
  tree leaves a wire unassigned, the compiler reports an error rather than inferring a
  latch.
- **Registers hold their value** on paths that do not assign them.
- **Reads see nets, not variables.** Reading a wire gives its final value, wherever
  the read appears. Reading a register gives its current (pre-clock-edge) value. So
  `a <= b; b <= a;` swaps, and `y = y + 1;` is a combinational loop, which the
  compiler reports.
- **Element-wise assignment.** Elements of arrays and fields of bundles can be
  assigned individually. An array index may be compile-time or hardware
  (`mem[addr] <= d;`). Bits of a scalar cannot be assigned individually; build the
  value with `cat(...)` instead.

## Instances

```rust
let f = Fifo::<DEPTH = 8>(push, pop, wdata);   // positional
let g = Fifo(push: p, pop: q, wdata: d);        // named
let h = Fifo(push: p);                          // the rest connected later:
h.pop = q;
h.wdata = d;
rdata = f.rdata;                                // outputs are fields
```

- `let f: Fifo;` creates an instance with every input still unconnected, just like
  `let s: Stream;` creates an undriven bundle net. Either way, the design only
  compiles once everything is driven.
- An instance is a value whose type is its module's signature. Reading its fields
  gives its outputs. Assigning (`=`) to its fields drives its inputs.
- **A module as a port type is only its interface.** `module Tap(c: Counter)` creates
  no counter. `Tap` reads `c`'s outputs and drives `c`'s inputs, and whoever
  instantiates `Tap` connects a real counter: `let c: Counter; let t = Tap(c);`.
- Every input must be connected exactly once.
- `Sub(...).y` can also be used inline in an expression.

## Bundles

A module without a body is a bundle type. It is written from the producer's point of
view: its outputs flow from the producer and its inputs flow back.

```rust
module Stream<W = 8>(ready: u1) -> (valid: u1, data: u<W>);
```

Where a bundle port is declared decides its direction. In the output list it is used
as written, so this module drives `deq.valid` and `deq.data` and reads `deq.ready`.
In the argument list every field is flipped.

```rust
module StreamReg(enq: Stream) -> (deq: Stream) {
    enq.ready = !full | deq.ready;      // flipped side: this module drives `ready`
    ...
}
```

**Connecting whole bundles.** `a = b` connects whole bundles: `a` is fed by `b`.
Forward fields flow from `b` to `a`, and flipped fields from `a` back to `b`. Every
field is checked for type and for who drives it, so a connection in the wrong
direction is reported, with a hint to swap the sides.

```rust
module Pipe2(enq: Stream) -> (deq: Stream) {
    let a = StreamReg(enq);       // a.enq = enq
    let b = StreamReg(a.deq);     // b.enq = a.deq
    deq = b.deq;
}
```

**Data bundles.** A bundle whose fields are all outputs is plain data, a struct. It
can be registered with `<=`, selected with `if`/`match`, and passed around like any
other value. A bundle with flipped fields can only be connected place-to-place with
`=`.

## Modules implemented in Rust

A module can be implemented in Rust and declared in `.si` by its signature and the
path of its Rust type:

```rust
extern module Sync2(d: u1 @src) -> (q: u1 @dst) = "::silane::cdc::Sync2";
```

The Rust type must have the shape of a generated module:

- a constructor `new(inputs...)`, taking the inputs in declaration order;
- one public field per output, named after it;
- generic over its clock domains: `Signal<W, D>` with a type parameter `D`, never a
  fixed domain.

`@labels` describe the Rust type's domains: ports with different labels may be in
different domains (as `Sync2`'s `d` and `q` are), and unlabelled ports share one. Most
extern modules need none. Only extern module ports have labels.

An extern module may have parameters, like any module. Their values are passed to
`new` first, as integers, in declaration order: `extern module M<W = 8>(a: u<W>) -> ...`
instantiated as `M::<W = 12>(x)` calls `M::new(12, x)`. An extern module cannot be
exported under another name with `module Alias = ...;`.

So hand-written DSL modules, library primitives, and modules generated from other
`.si` files can all be bound this way. Modules use an extern module through a generated
adapter with its declared signature, so rustc checks the declaration against the real
type there, and reports a mismatch (a width, or domains the type ties that the
declaration keeps apart) at the adapter, which names the `.si` declaration.

The compiler cannot see inside an `extern module`, so it assumes every output depends
combinationally on every input. This may reject a safe design, but it never accepts a
combinational loop. (The library's `Mem` is the exception: the compiler knows its read
data is registered.) Designs that use extern modules are built through the generated
Rust; `silc verilog` and the compiler's direct netlist backend cannot build them, except
for `Mem`.

The silane library's primitives are declared in built-in files:

```rust
import "std:cdc";   // Sync2: two-register synchronizer between clock domains
import "std:mem";   // Mem: a RAM with a registered read port
```

## Memories

`Mem` from `std:mem` is a RAM of `D` words of `W` bits, with one write port and one
registered read port: the shape FPGA block RAMs implement, and the Verilog backend
emits it in the form synthesis tools infer as one.

```rust
import "std:mem";

let lb = Mem::<W = 32, D = 1024>(
    we: wr, waddr: wa, wdata: px,   // the word at `waddr` becomes `wdata`
    re: rd, raddr: ra,              // `rdata` takes the word at `raddr`
);
let word = lb.rdata;                // the word read at the previous clock edge
```

- On each clock edge, if `re` is high, `rdata` takes the word at `raddr` as it was
  before the edge (a write to the same address in the same cycle is seen by the next
  read). `rdata` holds while `re` is low.
- Addresses are `u<max(clog2(D), 1)>`. Addresses past the end read 0 and ignore writes.
- `rdata` never depends combinationally on the inputs, so it may feed the read address
  directly, for example to follow a linked list.
- Contents are not reset. A ROM with initial contents is built in Rust with
  `silane::mem::Rom` and bound with `extern module`.

## Control flow

```rust
if cond { ... } else if other { ... } else { ... }

match sel {
    0 => y = a,
    1 | 2 => { y = b; z = c; }
    3 => if go { y = d; } else { y = e; },
    _ => {}
}

for i in 0..N { stages[i + 1] <= stages[i]; }   // also `0..=N`
```

The behaviour depends on whether the condition is known at compile time:

- If the condition is a hardware `u1`, both branches become hardware, merged with
  multiplexers.
- If the condition or the scrutinee is a compile-time value, only the matching branch
  is elaborated. This is how you generate hardware conditionally:
  `if WIDTH > 8 { ... }`.

`for` loops are unrolled at compile time. The loop variable is a compile-time integer.

## Expressions

Operators follow Rust precedence. From tightest to loosest:

| Operators               | Notes                                             |
|-------------------------|---------------------------------------------------|
| `x[i]` `x[a..b]` `.f`   | index / bit range / field / method                |
| `!x` `-x`               | bitwise not, wrapping negation                    |
| `as`                    | zero-extension, or conversion to and from enums   |
| `*` `/` `%`             | `/` and `%` only on compile-time integers         |
| `+` `-`                 | wrapping, same width on both sides                |
| `<<` `>>`               | amount: compile-time or any-width hardware value  |
| `&`                     |                                                   |
| `^`                     |                                                   |
| `\|`                    |                                                   |
| `==` `!=` `<` `<=` `>` `>=` | result `u1`; ordering is unsigned; no chaining |
| `&&`                    | on `u1` only                                      |
| `\|\|`                  | on `u1` only                                      |

`<=` is an assignment at the start of a statement and a comparison inside an
expression.

- **Bits.** `x[3]` is bit 3, as a `u1`. `x[4..8]` and `x[4..=7]` are bit ranges,
  LSB first, with compile-time bounds. To select a bit at a dynamic position, shift
  first: `(x >> i)[0]`.
- **Arrays.** `a[i]` with a hardware index builds a multiplexer. An out-of-range
  index reads element 0.
- **Casts.** `x as u16` zero-extends. Narrowing is an error; use a bit range
  instead. `x as State` reinterprets bits as an enum, and `state as u3` goes the
  other way.
- **Conditional expressions.** `if c { a } else { b }` and `match` both produce
  values. A `match` expression must be exhaustive: it needs a `_` arm, all enum
  variants, or every value of a type up to 16 bits wide. Blocks may start with
  `let`s: `{ let t = a + b; t >> 1 }`.

### Built-in functions and methods

| Call             | Result                                                 |
|------------------|--------------------------------------------------------|
| `cat(a, b, ...)` | concatenation, first argument in the most significant bits |
| `rep(x, n)`      | `x` repeated `n` times                                 |
| `x.rev()`        | bit-reversed `x`                                       |
| `x.any()`        | `u1`: any bit set                                      |
| `x.all()`        | `u1`: all bits set                                     |
| `x.width()`      | width of `x`, as a compile-time integer                |
| `a.len()`        | length of an array, as a compile-time integer          |
| `clog2(n)`       | compile-time ⌈log₂ n⌉                                  |
| `min(a, b)`, `max(a, b)` | compile-time minimum / maximum                 |

## Limits

- Signals are at most 64 bits wide, because the simulator stores one `u64` per net.
- There are no signed types yet.
- Clocks themselves (ports, generated clocks) are bound in Rust with `silane::Domain`;
  new kinds of synchronizers are written in Rust and declared with labelled
  `extern module` ports.
- The Verilog output is flat. Module hierarchy is kept in the generated Rust, as one
  call per instance.

More examples, each with a simulation test, are in `crates/si-tests/hw/`.
