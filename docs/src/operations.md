# Operations & Primitives

## Arithmetic & Bitwise Operators

Both operands have the same width; results wrap.

| Operator | Syntax | Primitive |
|---|---|---|
| Addition | `a + b` | `$add` |
| Subtraction | `a - b` | `$sub` |
| Multiplication | `a * b` | `$mul` |
| Bitwise AND / OR / XOR | `a & b`, `a \| b`, `a ^ b` | `$and`, `$or`, `$xor` |
| Bitwise NOT | `!a` | `$not` |
| Shifts | `a << n`, `a >> n` (`n`: wire of any width, or integer) | `$shl`, `$shr` |

## Comparisons

All return a 1-bit wire; ordering comparisons are unsigned.

```rust
let is_zero = acc.eq(0u64);
let below = a.lt(b);   // also ne, le, gt, ge
let any_set = a.any(); // all() checks every bit
```

## Multiplexing

```rust
let out = sel.mux(when_one, when_zero); // sel must be 1 bit
let out = idx.select([a, b, c, d]);     // out-of-range selects index 0
```

The `when!` and `select!` macros in `silane::dsl` provide procedural syntax on top.

## Bits

```rust
let opcode = word.slice::<U5, U8, U3>(); // bits [5, 8)
let wide = byte.zext(U32::default());   // zero-extend
let both = hi.concat(lo);               // hi in the most significant bits
let reversed = acc.rev();
```
