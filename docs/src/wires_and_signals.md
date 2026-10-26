# Wires & Signals

Silane uses typestates and phantom types to ensure hardware graph correctness.

## `Wire<T, N>` and `Signal<W, D>`

A `Wire<Signal<W, D>, N>` represents a net in the circuit topology:
- **`W`**: bit width — a typenum type such as `U8`, or `usize` for widths only known
  at runtime (checked when wires are combined).
- **`D`**: clock domain — any type naming one (see below); `Sys` by default, so
  `Signal<U8>` is `Signal<U8, Sys>`.
- **`N`**: typestate marker (`UnBound` or `Bound`, default `Bound`).

A wire belongs to the elaboration that created it. Using it in another elaboration
panics, since its node would name an unrelated net there.

### `UnBound` vs `Bound` Wires

- **`UnBound`**: a net without a driver yet. Drive it exactly once: combinationally
  with `.assign(value)`, or from a register with `.next(value)` (reset value 0) or
  `.next_reset(value, reset)`.
- **`Bound`**: a readable net. Implements `Copy` so it can feed many blocks. Get one
  from an unbound wire with `.read()`; it can be read before the wire is driven.

```rust
let acc_u = Signal::<U8>::default();
let acc = acc_u.read();
acc_u.next_reset(acc + 1u64, 3); // counts up from 3 after reset
```

Reset values belong to registers rather than to signal types, so signals with
different reset values combine freely.

## Integer Conversions

Operators and methods accept integers wherever a wire of the same width is expected;
they become constants. A constant that does not fit the width panics at elaboration
instead of being silently truncated.

```rust
let is_add = opcode.eq(0b001u64);
let next = acc + 1u64;
```

## Names and width conversions

- `.named("count")` attaches a name used for the net in Verilog output.
- `.erase()` turns a typed wire into a `usize`-width wire; `.typed::<U8>()` converts
  back (panicking if the width differs).

## Clock domains

Clock domains are correct by construction:

- Logic only combines signals of one domain: `a + b` with `a` in `Fast` and `b` in
  `Slow` does not compile.
- A register is clocked by its signal's domain: `.next()` on a `Signal<U8, Fast>`
  makes a register clocked by `Fast`. Nothing else names a clock.
- Signals change domain only through synchronizers (`silane::cdc`), such as
  `Sync2::<Slow, Fast>::new(bit)`.

A domain is a type, bound to a clock once per circuit by whoever builds the circuit,
before its first register:

```rust
struct Fast;
struct Slow;
Domain::<Fast>::ports("fast", "fast_rst");      // new clock and reset input ports
Domain::<Slow>::ports_on_use("slow", "slow_rst"); // ... created by the first register
Domain::<Sys>::same_as::<Fast>();               // two domains on one clock

let a = Signal::<U8, Fast>::default();          // a register in `Fast`
let s = Sync2::<Slow, Fast>::new(slow_bit).q;   // a `Slow` bit, synchronized into `Fast`
```

Code generic over its domains says what it accepts: `fn f<D>(a: W<U8, D>, b: W<U8, D>)`
needs one domain, while `fn g<A, B>(a: W<U8, A>, b: W<U8, B>)` accepts two, or the same
one twice. A signal that never reaches a register needs no clock: input ports and
constants take whatever domain they are used in. A register in an unbound domain, or a
domain bound twice, panics at elaboration.

In simulation, `sim.step_clock("fast")` clocks one domain and `sim.step()` clocks all
of them at once.
