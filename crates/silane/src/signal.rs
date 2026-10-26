use std::{
    any::TypeId,
    marker::PhantomData,
    ops::{Add, BitAnd, BitOr, BitXor, Mul, Not, Shl, Shr, Sub},
};
use typenum::{Bit, U1, UInt, UTerm, Unsigned};

use crate::{
    circuit::Circuit,
    primitive::{
        BinaryOp, BinaryPrim, BufPrim, ConcatPrim, ConstPrim, DffPrim, MuxPrim, NotPrim, Primitive,
        RevPrim, SlicePrim,
    },
    wire::{Bound, UnBound, Wire},
};

/// Trait for bit width types (both typenum types like `U8` and runtime `usize`).
pub trait Num: 'static + Copy {
    fn get(&self) -> usize;
}

impl Num for usize {
    fn get(&self) -> usize {
        *self
    }
}

impl Num for UTerm {
    fn get(&self) -> usize {
        0
    }
}

impl<U: Unsigned + Copy + Default + 'static, B: Bit + Copy + Default + 'static> Num for UInt<U, B> {
    fn get(&self) -> usize {
        Self::USIZE
    }
}

/// Hardware Signal payload carried inside a `Wire<Signal<W, D>, N>`
/// - `W`: Bit Width (generic type implementing `Num`)
/// - `D`: clock domain, a type naming it (see [`Domain`]); [`Sys`] by default
///
/// Combinational logic only combines signals of one domain, and a register is clocked
/// by its signal's domain, so signals cross between domains only through primitives
/// built for it (see [`crate::cdc`]). A signal that never reaches a register is
/// generic over `D`: its domain is whatever it is used in.
///
/// Reset values are a property of registers, not of signals: see
/// [`Wire::next_reset`].
pub struct Signal<W, D: 'static = Sys> {
    pub width: W,
    domain: PhantomData<fn() -> D>,
}

// Not derived: those would require `D` itself to implement the traits.
impl<W: Clone, D> Clone for Signal<W, D> {
    fn clone(&self) -> Self {
        Signal { width: self.width.clone(), domain: PhantomData }
    }
}

impl<W: Copy, D> Copy for Signal<W, D> {}

impl<W: PartialEq, D> PartialEq for Signal<W, D> {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width
    }
}

impl<W: Eq, D> Eq for Signal<W, D> {}

impl<W: std::fmt::Debug, D> std::fmt::Debug for Signal<W, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signal")
            .field("width", &self.width)
            .field("domain", &std::any::type_name::<D>())
            .finish()
    }
}

impl<W, D> Signal<W, D> {
    fn of(width: W) -> Self {
        Signal { width, domain: PhantomData }
    }
}

/// The default clock domain: `Signal<W, D>` is `Signal<W, Sys>`.
pub struct Sys;

/// Largest supported signal width (the simulator stores one `u64` per net).
pub const MAX_WIDTH: usize = 64;

fn check_width(width: usize) {
    assert!(
        (1..=MAX_WIDTH).contains(&width),
        "signal width {width} is out of range (1..={MAX_WIDTH})"
    );
}

fn check_fits(value: u64, width: usize) {
    assert!(
        width >= 64 || value >> width == 0,
        "constant {value} does not fit in {width} bits"
    );
}

/// Allocates an output node of `width` and registers the primitive built for it.
fn emit<W: Num, D, P: Primitive + 'static>(
    width: W,
    build: impl FnOnce(usize) -> P,
) -> Wire<Signal<W, D>, Bound> {
    let out = Circuit::with_current_dirty(|c| {
        let out = c.new_node(width.get());
        c.add_primitive(build(out));
        out
    });
    Wire::new_dirty(
        out,
        Signal::of(width),
    )
}

impl<W: Num, D> Signal<W, D> {
    /// Allocates an unbound wire carrying a Signal payload with explicit width.
    pub fn new(width: W) -> Wire<Self, UnBound> {
        check_width(width.get());
        let node = Circuit::with_current_dirty(|c| c.new_node(width.get()));
        Wire::new_dirty(
            node,
            Signal::of(width),
        )
    }

    /// Allocates a bound wire driven by a constant value.
    ///
    /// # Panics
    /// If `val` does not fit in `width` bits.
    pub fn constant(val: u64, width: W) -> Wire<Self, Bound> {
        check_width(width.get());
        check_fits(val, width.get());
        emit(width, |out| ConstPrim {
            value: val,
            width: width.get(),
            out,
        })
    }

    /// Allocates a top-level input port carrying a bound Signal wire.
    pub fn input_port(name: &str, width: W) -> Wire<Self, Bound> {
        check_width(width.get());
        let node = Circuit::with_current_dirty(|c| c.input_port(name, width.get()));
        Wire::new_dirty(
            node,
            Signal::of(width),
        )
    }

    /// Registers a top-level output port for a bound Signal wire.
    pub fn output_port(name: &str, wire: Wire<Self, Bound>) {
        Circuit::with_current_dirty(|c| c.output_port(name, wire));
    }
}

impl<W: Num + Default, D> Signal<W, D> {
    /// Allocates an unbound wire using `W`'s `Default` value.
    pub fn default() -> Wire<Self, UnBound> {
        Self::new(W::default())
    }

    /// Allocates a bound constant wire using `W`'s `Default` value.
    pub fn constant_default(val: u64) -> Wire<Self, Bound> {
        Self::constant(val, W::default())
    }

    /// Allocates a top-level input port using `W`'s `Default` value.
    pub fn input_port_default(name: &str) -> Wire<Self, Bound> {
        Self::input_port(name, W::default())
    }
}

/// Conversion trait into a `Wire<Signal<W, D>, Bound>` of a given width: a wire of
/// domain `D`, or a constant.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a signal of width `{W}` in clock domain `{D}`",
    label = "another width or clock domain",
    note = "signals combine at one width, in one clock domain; different clock domains only meet through a synchronizer, such as `silane::cdc::Sync2`"
)]
pub trait IntoWire<W: Num, D = Sys> {
    fn into_wire(self, width: W) -> Wire<Signal<W, D>, Bound>;
}

impl<W: Num, D> IntoWire<W, D> for Wire<Signal<W, D>, Bound> {
    fn into_wire(self, width: W) -> Wire<Signal<W, D>, Bound> {
        assert_eq!(
            self.value.width.get(),
            width.get(),
            "width mismatch: expected a {}-bit wire, got {} bits",
            width.get(),
            self.value.width.get()
        );
        self
    }
}

macro_rules! impl_into_wire_int {
    ($($t:ty),*) => {
        $(
            impl<W: Num, D: 'static> IntoWire<W, D> for $t {
                fn into_wire(self, width: W) -> Wire<Signal<W, D>, Bound> {
                    let value = u64::try_from(self)
                        .unwrap_or_else(|_| panic!("negative constant {} cannot be a signal", self));
                    Signal::<W, D>::constant(value, width)
                }
            }
        )*
    };
}

impl_into_wire_int!(u64, u32, u16, u8, usize, i64, i32, i16, i8);

impl<W: Num, D: 'static> IntoWire<W, D> for bool {
    fn into_wire(self, width: W) -> Wire<Signal<W, D>, Bound> {
        Signal::<W, D>::constant(self as u64, width)
    }
}

impl<W: Num + Default, D> From<u64> for Wire<Signal<W, D>, Bound> {
    fn from(val: u64) -> Self {
        Signal::<W, D>::constant_default(val)
    }
}

// Drivers for unbound wires
impl<W: Num, D> Wire<Signal<W, D>, UnBound> {
    /// Drives this wire from a register (reset value 0) whose next state is `value`.
    pub fn next(self, value: impl IntoWire<W, D>) {
        self.next_reset(value, 0);
    }

    /// Drives this wire from a register whose next state is `value` and whose
    /// value after reset is `reset`. The register is clocked and reset by the wire's
    /// domain `D`.
    ///
    /// # Panics
    /// If `reset` does not fit in the wire's width, or if no clock is bound to `D`
    /// (see [`Domain`]).
    pub fn next_reset(self, value: impl IntoWire<W, D>, reset: u64) {
        check_fits(reset, self.value.width.get());
        let value = value.into_wire(self.value.width);
        let (clk, rst) = Domain::<D>::nets_dirty();
        Circuit::with_current_dirty(|c| {
            c.add_primitive(DffPrim {
                clk,
                rst,
                reset_val: reset,
                d: value.node_dirty(),
                q: self.node_dirty(),
            });
        });
    }

    /// Binds an unbound signal wire combinationally to `value`.
    pub fn assign(self, value: impl IntoWire<W, D>) {
        let value = value.into_wire(self.value.width);
        Circuit::with_current_dirty(|c| {
            c.add_primitive(BufPrim {
                a: value.node_dirty(),
                out: self.node_dirty(),
            });
        });
    }

    /// Alias for `assign`.
    pub fn bind(self, value: impl IntoWire<W, D>) {
        self.assign(value);
    }

    /// Attaches a human-readable name to this net (used by the Verilog backend).
    pub fn named(self, name: &str) -> Self {
        Circuit::with_current_dirty(|c| c.name_node(self.node_dirty(), name));
        self
    }
}

impl<W: Num, D> Wire<Signal<W, D>, Bound> {
    /// Register top-level output port directly from a bound wire handle.
    pub fn output_port(self, name: &str) {
        Circuit::with_current_dirty(|c| c.output_port(name, self));
    }

    /// Attaches a human-readable name to this net (used by the Verilog backend).
    /// The first name given to a net wins.
    pub fn named(self, name: &str) -> Self {
        Circuit::with_current_dirty(|c| c.name_node(self.node_dirty(), name));
        self
    }

    /// Bit width of this wire.
    pub fn width(&self) -> usize {
        self.value.width.get()
    }

    /// Forgets the static width, giving a wire whose width is checked at runtime.
    pub fn erase(self) -> Wire<Signal<usize, D>, Bound> {
        Wire::new_dirty(
            self.node_dirty(),
            Signal::of(self.width()),
        )
    }

    /// Converts to a statically sized wire.
    ///
    /// # Panics
    /// If the wire is not exactly `W2` bits wide.
    pub fn typed<W2: Num + Default>(self) -> Wire<Signal<W2, D>, Bound> {
        let width = W2::default();
        assert_eq!(
            self.width(),
            width.get(),
            "expected a {}-bit wire, got {} bits",
            width.get(),
            self.width()
        );
        Wire::new_dirty(
            self.node_dirty(),
            Signal::of(width),
        )
    }

    fn binary(self, op: BinaryOp, rhs: impl IntoWire<W, D>) -> Self {
        let rhs = rhs.into_wire(self.value.width);
        emit(self.value.width, |out| BinaryPrim {
            op,
            a: self.node_dirty(),
            b: rhs.node_dirty(),
            out,
        })
    }

    fn compare(self, op: BinaryOp, rhs: Self) -> Wire<Signal<U1, D>, Bound> {
        emit(U1::default(), |out| BinaryPrim {
            op,
            a: self.node_dirty(),
            b: rhs.node_dirty(),
            out,
        })
    }
}

// Arithmetic and Logic Operators
macro_rules! impl_binary_operator {
    ($($trait:ident :: $method:ident => $op:ident),* $(,)?) => {
        $(
            impl<W: Num, D, RHS: IntoWire<W, D>> $trait<RHS> for Wire<Signal<W, D>, Bound> {
                type Output = Wire<Signal<W, D>, Bound>;

                fn $method(self, rhs: RHS) -> Self::Output {
                    self.binary(BinaryOp::$op, rhs)
                }
            }
        )*
    };
}

impl_binary_operator! {
    Add::add => Add,
    Sub::sub => Sub,
    Mul::mul => Mul,
    BitAnd::bitand => And,
    BitOr::bitor => Or,
    BitXor::bitxor => Xor,
}

impl<W: Num, D> Not for Wire<Signal<W, D>, Bound> {
    type Output = Wire<Signal<W, D>, Bound>;

    fn not(self) -> Self::Output {
        emit(self.value.width, |out| NotPrim {
            a: self.node_dirty(),
            out,
        })
    }
}

/// Shift amounts: a wire of any width in domain `D`, or an integer constant.
pub trait IntoShift<D = Sys> {
    fn into_shift(self) -> usize;
}

impl<W: Num, D> IntoShift<D> for Wire<Signal<W, D>, Bound> {
    fn into_shift(self) -> usize {
        self.node_dirty()
    }
}

macro_rules! impl_into_shift_int {
    ($($t:ty),*) => {
        $(
            impl<D> IntoShift<D> for $t {
                fn into_shift(self) -> usize {
                    let amount = u64::try_from(self)
                        .unwrap_or_else(|_| panic!("negative shift amount {}", self));
                    Signal::<usize>::constant(amount.min(127), 7).node_dirty()
                }
            }
        )*
    };
}

impl_into_shift_int!(u64, u32, u16, u8, usize, i64, i32, i16, i8);

impl<W: Num, D, RHS: IntoShift<D>> Shl<RHS> for Wire<Signal<W, D>, Bound> {
    type Output = Wire<Signal<W, D>, Bound>;

    fn shl(self, rhs: RHS) -> Self::Output {
        let amount = rhs.into_shift();
        emit(self.value.width, |out| BinaryPrim {
            op: BinaryOp::Shl,
            a: self.node_dirty(),
            b: amount,
            out,
        })
    }
}

impl<W: Num, D, RHS: IntoShift<D>> Shr<RHS> for Wire<Signal<W, D>, Bound> {
    type Output = Wire<Signal<W, D>, Bound>;

    fn shr(self, rhs: RHS) -> Self::Output {
        let amount = rhs.into_shift();
        emit(self.value.width, |out| BinaryPrim {
            op: BinaryOp::Shr,
            a: self.node_dirty(),
            b: amount,
            out,
        })
    }
}

// Multiplexing
impl<WCond: Num, D> Wire<Signal<WCond, D>, Bound> {
    /// Hardware MUX: Selects `then_val` if self is 1, else `else_val`
    ///
    /// # Panics
    /// If the selector is not a single bit, or the two inputs differ in width.
    pub fn mux<W: Num>(
        self,
        then_wire: Wire<Signal<W, D>>,
        else_wire: Wire<Signal<W, D>>,
    ) -> Wire<Signal<W, D>, Bound> {
        assert_eq!(self.width(), 1, "mux selector must be a single bit");
        assert_eq!(
            then_wire.width(),
            else_wire.width(),
            "mux inputs must have the same width"
        );
        emit(else_wire.value.width, |out| MuxPrim {
            sel: self.node_dirty(),
            in0: else_wire.node_dirty(),
            in1: then_wire.node_dirty(),
            out,
        })
    }

    /// Selects from an array of N candidate inputs based on selector index self.
    /// Out-of-range selector values select `inputs[0]`.
    pub fn select<W: Num, const N: usize>(
        self,
        inputs: [Wire<Signal<W, D>, Bound>; N],
    ) -> Wire<Signal<W, D>, Bound> {
        assert!(N > 0, "select requires at least 1 input");
        let mut acc = inputs[0];
        for (idx, &item) in inputs.iter().enumerate().skip(1) {
            let match_bit = self.eq(idx as u64);
            acc = match_bit.mux(item, acc);
        }
        acc
    }

    /// Case-matching MUX: Matches `self` against discrete case values `[(val0, res0), (val1, res1), ...]`.
    /// Earlier cases take priority.
    pub fn mux_cases<W: Num>(
        self,
        cases: &[(u64, Wire<Signal<W, D>, Bound>)],
        default_res: Wire<Signal<W, D>, Bound>,
    ) -> Wire<Signal<W, D>, Bound> {
        let mut acc = default_res;
        for &(case_val, item) in cases.iter().rev() {
            let match_bit = self.eq(case_val);
            acc = match_bit.mux(item, acc);
        }
        acc
    }
}

// Comparisons
impl<W: Num, D> Wire<Signal<W, D>, Bound> {
    /// Equality comparator: 1 if self == rhs, else 0.
    pub fn eq(self, rhs: impl IntoWire<W, D>) -> Wire<Signal<U1, D>, Bound> {
        let rhs = rhs.into_wire(self.value.width);
        self.compare(BinaryOp::Eq, rhs)
    }

    /// Inequality comparator: 1 if self != rhs, else 0.
    pub fn ne(self, rhs: impl IntoWire<W, D>) -> Wire<Signal<U1, D>, Bound> {
        !self.eq(rhs)
    }

    /// Unsigned less-than comparator.
    pub fn lt(self, rhs: impl IntoWire<W, D>) -> Wire<Signal<U1, D>, Bound> {
        let rhs = rhs.into_wire(self.value.width);
        self.compare(BinaryOp::Lt, rhs)
    }

    /// Unsigned greater-than comparator.
    pub fn gt(self, rhs: impl IntoWire<W, D>) -> Wire<Signal<U1, D>, Bound> {
        let rhs = rhs.into_wire(self.value.width);
        rhs.compare(BinaryOp::Lt, self)
    }

    /// Unsigned less-or-equal comparator.
    pub fn le(self, rhs: impl IntoWire<W, D>) -> Wire<Signal<U1, D>, Bound> {
        !self.gt(rhs)
    }

    /// Unsigned greater-or-equal comparator.
    pub fn ge(self, rhs: impl IntoWire<W, D>) -> Wire<Signal<U1, D>, Bound> {
        !self.lt(rhs)
    }

    /// OR-reduction: 1 if any bit is set.
    pub fn any(self) -> Wire<Signal<U1, D>, Bound> {
        self.ne(0u64)
    }

    /// AND-reduction: 1 if every bit is set.
    pub fn all(self) -> Wire<Signal<U1, D>, Bound> {
        let width = self.width();
        let ones = if width >= 64 { u64::MAX } else { (1 << width) - 1 };
        self.eq(ones)
    }
}

// Bit Slicing, Extension & Bit Reversal
impl<W: Num, D> Wire<Signal<W, D>, Bound> {
    /// Bit slice `[LO, HI)` using LO and HI types (where HI - LO = OUT)
    pub fn slice<LO: Num + Default, HI: Num + Default, OUT: Num>(
        self,
    ) -> Wire<Signal<OUT, D>, Bound>
    where
        HI: Sub<LO, Output = OUT>,
    {
        let lo_val = LO::default();
        let hi_val = HI::default();
        self.slice_dyn(lo_val.get(), hi_val - lo_val)
    }

    /// Bit slice of `width` bits starting at bit `offset`, with a runtime width.
    ///
    /// # Panics
    /// If the slice is empty or reaches past the end of the wire.
    pub fn slice_dyn<OUT: Num>(self, offset: usize, width: OUT) -> Wire<Signal<OUT, D>, Bound> {
        let len = width.get();
        assert!(
            len > 0 && offset + len <= self.width(),
            "slice [{offset}, {}) is out of range for a {}-bit wire",
            offset + len,
            self.width()
        );
        emit(width, |out| SlicePrim {
            input: self.node_dirty(),
            offset,
            width: len,
            out,
        })
    }

    /// Zero-extends the wire to `width` bits.
    ///
    /// # Panics
    /// If `width` is narrower than the wire.
    pub fn zext<OUT: Num>(self, width: OUT) -> Wire<Signal<OUT, D>, Bound> {
        let from = self.width();
        let to = width.get();
        assert!(to >= from, "cannot zero-extend a {from}-bit wire to {to} bits");
        if to == from {
            return Wire::new_dirty(
                self.node_dirty(),
                Signal::of(width),
            );
        }
        let zeros = Signal::<usize, D>::constant(0, to - from);
        emit(width, |out| ConcatPrim {
            a: zeros.node_dirty(),
            b: self.node_dirty(),
            out,
        })
    }

    /// Reverses the bit direction/order of the wire.
    pub fn rev(self) -> Wire<Signal<W, D>, Bound> {
        let w_val = self.width();
        emit(self.value.width, |out| RevPrim {
            a: self.node_dirty(),
            width: w_val,
            out,
        })
    }

    /// Concatenates `self` (MSBs) and `rhs` (LSBs) into a wire of width `OUT`.
    pub fn concat<W2: Num, OUT: Num>(
        self,
        rhs: Wire<Signal<W2, D>, Bound>,
    ) -> Wire<Signal<OUT, D>, Bound>
    where
        W: Add<W2, Output = OUT>,
    {
        let out_width = self.value.width + rhs.value.width;
        check_width(out_width.get());
        emit(out_width, |out| ConcatPrim {
            a: self.node_dirty(),
            b: rhs.node_dirty(),
            out,
        })
    }
}

/// Clock domain `D`: the clock and optional asynchronous, active-high reset of every
/// register whose signals are in `D`. Any type names a domain:
///
/// ```ignore
/// struct Video;
/// Domain::<Video>::ports("vclk", "vrst");
/// ```
///
/// A domain is bound once per circuit, by whoever builds it, before registers are
/// created in it. Two domains may be bound to the same clock ([`Domain::same_as`]);
/// their signals still only meet through a synchronizer.
pub struct Domain<D: 'static>(PhantomData<fn() -> D>);

impl<D: 'static> Domain<D> {
    /// Binds `D` to two new top-level input ports, a clock and a reset.
    pub fn ports(clock: &str, reset: &str) {
        let (clk, rst) = Circuit::with_current_dirty(|c| (c.input_port(clock, 1), c.input_port(reset, 1)));
        Self::bind_dirty(clk, Some(rst));
    }

    /// Binds `D` to a clock and a reset input port, created when `D`'s first register
    /// is: a design with no registers in `D` gets no ports.
    pub fn ports_on_use(clock: &str, reset: &str) {
        Circuit::with_current_dirty(|c| {
            c.bind_domain_on_use(TypeId::of::<D>(), std::any::type_name::<D>(), clock, reset)
        });
    }

    /// Binds `D` to a new top-level clock input, without a reset.
    pub fn ports_without_reset(clock: &str) {
        let clk = Circuit::with_current_dirty(|c| c.input_port(clock, 1));
        Self::bind_dirty(clk, None);
    }

    /// Binds `D` to the clock and reset of domain `E`.
    ///
    /// # Panics
    /// If `E` is not bound.
    pub fn same_as<E: 'static>() {
        let (clk, rst) = Domain::<E>::nets_dirty();
        Self::bind_dirty(clk, rst);
    }

    /// Binds `D` to raw clock and reset nets, for primitives that make clocks.
    ///
    /// # Panics
    /// If `D` is already bound in this circuit.
    pub fn bind_dirty(clock: usize, reset: Option<usize>) {
        Circuit::with_current_dirty(|c| {
            c.bind_domain(TypeId::of::<D>(), std::any::type_name::<D>(), clock, reset)
        });
    }

    /// Clock and reset nets of `D`, for primitives that build registers.
    ///
    /// # Panics
    /// If `D` is not bound in this circuit.
    pub fn nets_dirty() -> (usize, Option<usize>) {
        Circuit::with_current_dirty(|c| c.domain_nets(TypeId::of::<D>(), std::any::type_name::<D>()))
    }
}
