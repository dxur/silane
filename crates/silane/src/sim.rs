//! # Silane Simulation Backend
//!
//! Cycle-accurate simulation of an elaborated [`Circuit`].
//!
//! ```ignore
//! let mut sim = Sim::new(&circuit);
//!
//! let data_in  = sim.input::<U8>("data_in");
//! let data_out = sim.output::<U4>("data_out");
//!
//! sim.drive(&data_in, 0xFF);
//! sim.step();                         // posedge clk → DFFs latch
//! assert_eq!(sim.read(&data_out), 0xF);
//! ```
//!
//! The simulator is completely independent of the core library internals —
//! it is a pure interpreter of the already-compiled [`Circuit`] netlist.

use std::collections::HashMap;

use crate::{
    circuit::Circuit,
    primitive::{
        BinaryOp, BinaryPrim, BufPrim, ConcatPrim, ConstPrim, DffPrim, MemPrim, MuxPrim,
        NotPrim, PortDirection, RevPrim, SlicePrim,
    },
    signal::Num,
};

// ---------------------------------------------------------------------------
// Typed port handles
// ---------------------------------------------------------------------------

/// Handle to a top-level **input** port on the circuit boundary.
/// `W` is the bit width, checked against the circuit when the handle is created.
#[derive(Debug, Clone, Copy)]
pub struct SimInput<W = usize> {
    pub(crate) node: usize,
    pub(crate) width: usize,
    pub(crate) _marker: std::marker::PhantomData<W>,
}

/// Handle to a top-level **output** port on the circuit boundary.
/// `W` is the bit width, checked against the circuit when the handle is created.
#[derive(Debug, Clone, Copy)]
pub struct SimOutput<W = usize> {
    pub(crate) node: usize,
    pub(crate) _marker: std::marker::PhantomData<W>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Mask a value to `width` bits (up to 64 bits).
#[inline(always)]
fn mask(val: u64, width: usize) -> u64 {
    if width >= 64 {
        val
    } else {
        val & ((1u64 << width) - 1)
    }
}

/// Reverse the low `width` bits of `val`.
fn bit_reverse(val: u64, width: usize) -> u64 {
    let mut out = 0u64;
    for i in 0..width {
        if val & (1 << i) != 0 {
            out |= 1 << (width - 1 - i);
        }
    }
    out
}

/// Evaluates a binary operator on already-masked operands; the caller masks the result.
fn eval_binary(op: BinaryOp, a: u64, b: u64) -> u64 {
    match op {
        BinaryOp::Add => a.wrapping_add(b),
        BinaryOp::Sub => a.wrapping_sub(b),
        BinaryOp::Mul => a.wrapping_mul(b),
        BinaryOp::And => a & b,
        BinaryOp::Or => a | b,
        BinaryOp::Xor => a ^ b,
        BinaryOp::Eq => (a == b) as u64,
        BinaryOp::Lt => (a < b) as u64,
        BinaryOp::Shl => a.checked_shl(b.min(64) as u32).unwrap_or(0),
        BinaryOp::Shr => a.checked_shr(b.min(64) as u32).unwrap_or(0),
    }
}

// ---------------------------------------------------------------------------
// Simulation evaluation plan
// ---------------------------------------------------------------------------

/// A single combinational evaluation step, in topological order.
#[derive(Debug, Clone)]
enum EvalStep {
    Const { value: u64, out: usize },
    Binary { op: BinaryOp, a: usize, b: usize, out: usize, width: usize },
    Not { a: usize, out: usize, width: usize },
    Mux { sel: usize, in0: usize, in1: usize, out: usize },
    Slice { input: usize, offset: usize, width: usize, out: usize },
    Concat { a: usize, b: usize, width_b: usize, out: usize },
    Rev { a: usize, width: usize, out: usize },
    Buf { a: usize, out: usize },
}

/// A single sequential (DFF) element, evaluated at every `step()`.
#[derive(Debug)]
struct DffStep {
    clk: usize,
    rst: Option<usize>,
    reset_val: u64,
    d: usize,
    q: usize,
}

/// A memory, clocked at every `step()` of its clock. Its words live in the state,
/// at `base..base + depth`.
#[derive(Debug)]
struct MemStep {
    clk: usize,
    we: usize,
    waddr: usize,
    wdata: usize,
    re: usize,
    raddr: usize,
    rdata: usize,
    base: usize,
    depth: usize,
}

#[path = "sim_native.rs"]
mod native;

// ---------------------------------------------------------------------------
// Simulator
// ---------------------------------------------------------------------------

/// Cycle-accurate interpreter for a compiled Silane [`Circuit`].
///
/// # Model
/// - **State**: flat `Vec<u64>` with one slot per circuit node.
/// - **Combinational**: [`Sim::settle`] evaluates all combinational primitives
///   in topological order.
/// - **Sequential**: [`Sim::step`] calls `settle()` then commits all DFF next-states atomically,
///   modelling a single rising clock edge. A high reset input at the edge loads reset values.
pub struct Sim {
    /// One value slot per circuit node ID, then the words of every memory.
    state: Vec<u64>,
    /// Number of circuit nodes.
    nodes: usize,
    /// Distinct clock nets, in the order the compiled backend numbers them.
    clocks: Vec<usize>,
    /// The circuit compiled to native code, if available.
    native: Option<native::Native>,
    /// The state is settled except for inputs driven since: the native backend then
    /// only re-evaluates the logic that depends on inputs.
    settled: bool,
    /// Pre-compiled combinational evaluation steps in topological order.
    comb: Vec<EvalStep>,
    /// All DFF elements.
    dffs: Vec<DffStep>,
    /// All memories, with their contents.
    mems: Vec<MemStep>,
    /// Bit-width of every node (for masking).
    widths: Vec<usize>,
    /// Port name → (direction, node ID) lookup.
    port_nodes: HashMap<String, (PortDirection, usize)>,
}

impl Sim {
    /// Build a [`Sim`] from an elaborated [`Circuit`].
    ///
    /// The circuit is compiled to C and loaded as a shared library, which simulates far
    /// faster than interpreting it. Libraries are cached by content in
    /// `$TMPDIR/silane-sim`. The C compiler is `$SILANE_CC` (default `cc`). Set
    /// `SILANE_SIM=interp` to interpret instead; without a working compiler, the
    /// circuit is interpreted with a warning. Both behave identically.
    ///
    /// # Panics
    /// If the circuit contains a combinational cycle.
    pub fn new(circuit: &Circuit) -> Self {
        Self::build(circuit, true)
    }

    /// Like [`Sim::new`], but always interprets the circuit.
    pub fn interpreted(circuit: &Circuit) -> Self {
        Self::build(circuit, false)
    }

    fn build(circuit: &Circuit, compile: bool) -> Self {
        let widths = circuit.node_widths.clone();
        let order = circuit
            .combinational_order()
            .unwrap_or_else(|nets| panic!("Sim::new: combinational cycle through nets {nets:?}"));

        let mut comb = Vec::new();
        for idx in order {
            let any = circuit.primitives[idx].as_any();
            if let Some(c) = any.downcast_ref::<ConstPrim>() {
                comb.push(EvalStep::Const {
                    value: mask(c.value, widths[c.out]),
                    out: c.out,
                });
            } else if let Some(b) = any.downcast_ref::<BinaryPrim>() {
                comb.push(EvalStep::Binary {
                    op: b.op,
                    a: b.a,
                    b: b.b,
                    out: b.out,
                    width: widths[b.out],
                });
            } else if let Some(n) = any.downcast_ref::<NotPrim>() {
                comb.push(EvalStep::Not {
                    a: n.a,
                    out: n.out,
                    width: widths[n.out],
                });
            } else if let Some(m) = any.downcast_ref::<MuxPrim>() {
                comb.push(EvalStep::Mux {
                    sel: m.sel,
                    in0: m.in0,
                    in1: m.in1,
                    out: m.out,
                });
            } else if let Some(s) = any.downcast_ref::<SlicePrim>() {
                comb.push(EvalStep::Slice {
                    input: s.input,
                    offset: s.offset,
                    width: s.width,
                    out: s.out,
                });
            } else if let Some(cat) = any.downcast_ref::<ConcatPrim>() {
                comb.push(EvalStep::Concat {
                    a: cat.a,
                    b: cat.b,
                    width_b: widths[cat.b],
                    out: cat.out,
                });
            } else if let Some(r) = any.downcast_ref::<RevPrim>() {
                comb.push(EvalStep::Rev {
                    a: r.a,
                    width: r.width,
                    out: r.out,
                });
            } else if let Some(b) = any.downcast_ref::<BufPrim>() {
                comb.push(EvalStep::Buf { a: b.a, out: b.out });
            } else {
                panic!(
                    "Sim::new: unsupported primitive {}",
                    circuit.primitives[idx].name()
                );
            }
        }

        // A clock that passes through buffers is the same clock as its source.
        let buffers: HashMap<usize, usize> = circuit
            .primitives
            .iter()
            .filter_map(|p| p.as_any().downcast_ref::<BufPrim>())
            .map(|b| (b.out, b.a))
            .collect();
        let clock_source = |mut net: usize| {
            while let Some(&src) = buffers.get(&net) {
                net = src;
            }
            net
        };
        let dffs: Vec<DffStep> = circuit
            .primitives
            .iter()
            .filter_map(|p| p.as_any().downcast_ref::<DffPrim>())
            .map(|d| DffStep {
                clk: clock_source(d.clk),
                rst: d.rst,
                reset_val: mask(d.reset_val, widths[d.q]),
                d: d.d,
                q: d.q,
            })
            .collect();

        let mut state = vec![0u64; circuit.nodes];
        let mems: Vec<MemStep> = circuit
            .primitives
            .iter()
            .filter_map(|p| p.as_any().downcast_ref::<MemPrim>())
            .map(|m| {
                let base = state.len();
                state.extend_from_slice(&m.init);
                state.resize(base + m.depth, 0);
                MemStep {
                    clk: clock_source(m.clk),
                    we: m.we,
                    waddr: m.waddr,
                    wdata: m.wdata,
                    re: m.re,
                    raddr: m.raddr,
                    rdata: m.rdata,
                    base,
                    depth: m.depth,
                }
            })
            .collect();
        let mut clocks: Vec<usize> = Vec::new();
        for clk in dffs.iter().map(|d: &DffStep| d.clk).chain(mems.iter().map(|m| m.clk)) {
            if !clocks.contains(&clk) {
                clocks.push(clk);
            }
        }
        // Only ports are observable from outside; the compiled backend may keep every
        // other combinational node in a local.
        let ports: Vec<usize> = circuit.ports.iter().map(|p| p.net).collect();
        let inputs: Vec<usize> = circuit
            .ports
            .iter()
            .filter(|p| p.direction == PortDirection::Input)
            .map(|p| p.net)
            .collect();
        let native = compile
            .then(|| native::Native::load(state.len(), &comb, &dffs, &mems, &clocks, &ports, &inputs))
            .flatten();

        let port_nodes = circuit
            .ports
            .iter()
            .map(|p| (p.name.clone(), (p.direction, p.net)))
            .collect();

        let mut sim = Sim {
            state,
            nodes: circuit.nodes,
            clocks,
            native,
            settled: false,
            comb,
            dffs,
            mems,
            widths,
            port_nodes,
        };
        sim.reset();
        sim
    }

    // -----------------------------------------------------------------------
    // Port handle resolution
    // -----------------------------------------------------------------------

    fn port_node<W: Num + Default>(&self, name: &str, direction: PortDirection) -> (usize, usize) {
        let &(dir, node) = self
            .port_nodes
            .get(name)
            .unwrap_or_else(|| panic!("Sim: no port named '{name}'"));
        assert_eq!(
            dir,
            direction,
            "Sim: port '{name}' is an {} port",
            dir.as_str()
        );
        let width = self.widths[node];
        // `usize` (width 0 by default) opts out of the static width check.
        let expected = W::default().get();
        assert!(
            expected == 0 || expected == width,
            "Sim: port '{name}' is {width} bits wide, handle expects {expected}"
        );
        (node, width)
    }

    /// Returns a typed input port handle for `name`.
    ///
    /// # Panics
    /// If `name` is not an input port, or its width differs from `W`.
    pub fn input<W: Num + Default>(&self, name: &str) -> SimInput<W> {
        let (node, width) = self.port_node::<W>(name, PortDirection::Input);
        SimInput {
            node,
            width,
            _marker: std::marker::PhantomData,
        }
    }

    /// Returns a typed output port handle for `name`.
    ///
    /// # Panics
    /// If `name` is not an output port, or its width differs from `W`.
    pub fn output<W: Num + Default>(&self, name: &str) -> SimOutput<W> {
        let (node, _) = self.port_node::<W>(name, PortDirection::Output);
        SimOutput {
            node,
            _marker: std::marker::PhantomData,
        }
    }

    // -----------------------------------------------------------------------
    // Drive / Read
    // -----------------------------------------------------------------------

    /// Drives an input port to `val` (masked to the port width).
    /// Does **not** automatically settle — call [`Sim::settle`] or [`Sim::step`] afterwards.
    pub fn drive<W>(&mut self, port: &SimInput<W>, val: u64) {
        self.state[port.node] = mask(val, port.width);
    }

    /// Reads the current value of an output port.
    pub fn read<W>(&self, port: &SimOutput<W>) -> u64 {
        self.state[port.node]
    }

    /// Settles combinational logic, then reads an output port. Useful to observe
    /// the effect of freshly driven inputs before the next clock edge.
    pub fn peek<W>(&mut self, port: &SimOutput<W>) -> u64 {
        self.settle();
        self.state[port.node]
    }

    // -----------------------------------------------------------------------
    // Simulation control
    // -----------------------------------------------------------------------

    /// Evaluates all combinational primitives in topological order.
    ///
    /// After driving any input(s), call this to propagate values through
    /// combinational logic before reading outputs.
    pub fn settle(&mut self) {
        if let Some(native) = &self.native {
            if self.settled {
                native.settle_inputs(&mut self.state);
            } else {
                native.settle(&mut self.state);
                self.settled = true;
            }
            return;
        }
        let s = &mut self.state;
        for step in &self.comb {
            match *step {
                EvalStep::Const { value, out } => s[out] = value,
                EvalStep::Binary { op, a, b, out, width } => {
                    s[out] = mask(eval_binary(op, s[a], s[b]), width)
                }
                EvalStep::Not { a, out, width } => s[out] = mask(!s[a], width),
                EvalStep::Mux { sel, in0, in1, out } => {
                    s[out] = if s[sel] != 0 { s[in1] } else { s[in0] }
                }
                EvalStep::Slice { input, offset, width, out } => {
                    s[out] = mask(s[input] >> offset, width)
                }
                EvalStep::Concat { a, b, width_b, out } => {
                    s[out] = s[a].checked_shl(width_b as u32).unwrap_or(0) | s[b]
                }
                EvalStep::Rev { a, width, out } => s[out] = bit_reverse(s[a], width),
                EvalStep::Buf { a, out } => s[out] = s[a],
            }
        }
    }

    /// Simulates one rising edge of every clock at once:
    /// 1. Calls [`Sim::settle`] to propagate combinational logic.
    /// 2. Computes all DFF next-states from current `d` inputs (or reset values while
    ///    the reset input is high).
    /// 3. Commits all DFF `q` outputs atomically, then settles again.
    pub fn step(&mut self) {
        self.edge(None);
    }

    /// Simulates one rising edge of the clock input port `clock` only: registers in
    /// other clock domains keep their values.
    ///
    /// # Panics
    /// If `clock` is not an input port.
    pub fn step_clock(&mut self, clock: &str) {
        let &(dir, node) = self
            .port_nodes
            .get(clock)
            .unwrap_or_else(|| panic!("Sim: no port named '{clock}'"));
        assert_eq!(dir, PortDirection::Input, "Sim: port '{clock}' is not an input");
        self.edge(Some(node));
    }

    /// Clocks the registers whose clock net is `clock` (all registers for `None`).
    fn edge(&mut self, clock: Option<usize>) {
        self.settle();

        self.settled = false;
        if let Some(native) = &self.native {
            // A clock that clocks nothing has no number: nothing to do.
            match clock.map(|c| self.clocks.iter().position(|&k| k == c)) {
                None => native.edge(&mut self.state, -1),
                Some(Some(i)) => native.edge(&mut self.state, i as i32),
                Some(None) => {}
            }
            self.settle();
            return;
        }

        let next: Vec<(usize, u64)> = self
            .dffs
            .iter()
            .filter(|dff| clock.is_none_or(|c| dff.clk == c))
            .map(|dff| {
                let in_reset = dff.rst.is_some_and(|r| self.state[r] != 0);
                let next_q = if in_reset {
                    dff.reset_val
                } else {
                    self.state[dff.d]
                };
                (dff.q, next_q)
            })
            .collect();

        let state = &mut self.state;
        let mut reads = Vec::new();
        for mem in self.mems.iter().filter(|m| clock.is_none_or(|c| m.clk == c)) {
            if state[mem.re] != 0 {
                let addr = state[mem.raddr] as usize;
                let word = if addr < mem.depth { state[mem.base + addr] } else { 0 };
                reads.push((mem.rdata, word));
            }
            if state[mem.we] != 0 {
                let addr = state[mem.waddr] as usize;
                if addr < mem.depth {
                    state[mem.base + addr] = state[mem.wdata];
                }
            }
        }

        for (q, val) in next.into_iter().chain(reads) {
            state[q] = val;
        }

        self.settle();
    }

    /// Convenience: drive multiple inputs, run one clock edge, return multiple output values.
    ///
    /// ```ignore
    /// let [out0, out1] = sim.cycle(
    ///     &[(&data_in, 42), (&we_in, 1)],
    ///     &[&data_out, &valid_out],
    /// );
    /// ```
    pub fn cycle(
        &mut self,
        inputs: &[(&dyn Driveable, u64)],
        outputs: &[&dyn Readable],
    ) -> Vec<u64> {
        for (port, val) in inputs {
            port.drive_into(&mut self.state, *val);
        }
        self.step();
        outputs.iter().map(|p| p.read_from(&self.state)).collect()
    }

    /// Reset all DFF outputs to their declared reset values, then settle.
    pub fn reset(&mut self) {
        self.settled = false;
        for dff in &self.dffs {
            self.state[dff.q] = dff.reset_val;
        }
        self.settle();
    }

    /// Returns the number of circuit nodes (useful for waveform capture).
    pub fn node_count(&self) -> usize {
        self.nodes
    }

    /// Whether the circuit runs as compiled native code (see [`Sim::new`]).
    pub fn is_native(&self) -> bool {
        self.native.is_some()
    }

    /// The current value of any node (a register's output, or a wire the native
    /// backend keeps in the state). For debugging; see [`Circuit::node_names`].
    pub fn node_value(&self, node: usize) -> u64 {
        self.state[node]
    }

    /// Returns the declared bit-width of a node.
    pub fn width_of(&self, node: usize) -> usize {
        self.widths[node]
    }
}

// ---------------------------------------------------------------------------
// Object-safe drive / read traits for `cycle()` helper
// ---------------------------------------------------------------------------

/// Object-safe wrapper allowing heterogeneous `SimInput<W>` handles in slices.
pub trait Driveable {
    fn drive_into(&self, state: &mut [u64], val: u64);
}

/// Object-safe wrapper allowing heterogeneous `SimOutput<W>` handles in slices.
pub trait Readable {
    fn read_from(&self, state: &[u64]) -> u64;
}

impl<W> Driveable for SimInput<W> {
    fn drive_into(&self, state: &mut [u64], val: u64) {
        state[self.node] = mask(val, self.width);
    }
}

impl<W> Readable for SimOutput<W> {
    fn read_from(&self, state: &[u64]) -> u64 {
        state[self.node]
    }
}

// ---------------------------------------------------------------------------
// Waveform capture helper
// ---------------------------------------------------------------------------

/// A recorded snapshot of named signal values at one simulation cycle.
#[derive(Debug, Clone)]
pub struct WaveformFrame {
    pub cycle: usize,
    pub values: HashMap<String, u64>,
}

/// Accumulates waveform frames for tracing named signals over time.
pub struct Waveform {
    /// (label, node_id) pairs to capture every frame
    probes: Vec<(String, usize)>,
    pub frames: Vec<WaveformFrame>,
}

impl Waveform {
    /// Create an empty waveform recorder.
    pub fn new() -> Self {
        Waveform {
            probes: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// Add a probe on a named input/output port.
    pub fn probe_port(&mut self, sim: &Sim, name: &str) -> &mut Self {
        let (_, node) = *sim
            .port_nodes
            .get(name)
            .unwrap_or_else(|| panic!("Waveform::probe_port: no port '{name}'"));
        self.probes.push((name.to_string(), node));
        self
    }

    /// Add a probe on a typed `SimOutput<W>` handle with a custom label.
    pub fn probe<W>(
        &mut self,
        _sim: &Sim,
        label: &str,
        port: &SimOutput<W>,
    ) -> &mut Self {
        self.probes.push((label.to_string(), port.node));
        self
    }

    /// Capture the current simulation state as frame `cycle`.
    pub fn capture(&mut self, sim: &Sim, cycle: usize) {
        let values = self
            .probes
            .iter()
            .map(|(name, node)| (name.clone(), sim.state[*node]))
            .collect();
        self.frames.push(WaveformFrame { cycle, values });
    }

    /// Print a formatted table of all captured waveform frames.
    pub fn print(&self) {
        if self.frames.is_empty() {
            return;
        }
        let names: Vec<&str> = self.probes.iter().map(|(n, _)| n.as_str()).collect();

        // header
        print!("{:>6}  ", "cycle");
        for n in &names {
            print!("{:>12}  ", n);
        }
        println!();

        // rows
        for frame in &self.frames {
            print!("{:>6}  ", frame.cycle);
            for name in &names {
                let val = frame.values.get(*name).copied().unwrap_or(0);
                print!("{:>12}  ", format!("0x{:X}", val));
            }
            println!();
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{circuit::Circuit, signal::Signal};
    use typenum::{U0, U1, U3, U4, U8, U16};

    #[test]
    fn test_sim_combinational_adder() {
        let (ports, circuit) = Circuit::elaborate(|| {
            let a = Signal::<U8>::input_port_default("a");
            let b = Signal::<U8>::input_port_default("b");
            let sum = a + b;
            Signal::output_port("sum", sum);
            (a, b, sum)
        });
        let _ = ports;

        circuit.validate().unwrap();
        let mut sim = Sim::new(&circuit);

        let a_in = sim.input::<U8>("a");
        let b_in = sim.input::<U8>("b");
        let sum_out = sim.output::<U8>("sum");

        // 10 + 20 = 30
        sim.drive(&a_in, 10);
        sim.drive(&b_in, 20);
        sim.settle();
        assert_eq!(sim.read(&sum_out), 30);

        // 255 + 1 = 0 (8-bit wrap)
        sim.drive(&a_in, 255);
        sim.drive(&b_in, 1);
        sim.settle();
        assert_eq!(sim.read(&sum_out), 0);
    }

    #[test]
    fn test_sim_register_accumulator() {
        let (ports, circuit) = Circuit::elaborate(|| {
            crate::Domain::<crate::signal::Sys>::ports("clk", "rst");
            let inc = Signal::<U8>::input_port_default("inc");
            let acc_u = Signal::<U8>::default();
            let acc = acc_u.read();
            let next_acc = acc + inc;
            acc_u.next(next_acc);
            Signal::output_port("acc", acc);
            (inc, acc)
        });
        let _ = ports;

        circuit.validate().unwrap();
        let mut sim = Sim::new(&circuit);

        let inc_in = sim.input::<U8>("inc");
        let acc_out = sim.output::<U8>("acc");

        // Reset: acc = 0
        assert_eq!(sim.read(&acc_out), 0);

        // Step 1: inc=5 → acc becomes 5
        sim.drive(&inc_in, 5);
        sim.step();
        assert_eq!(sim.read(&acc_out), 5);

        // Step 2: inc=3 → acc becomes 8
        sim.drive(&inc_in, 3);
        sim.step();
        assert_eq!(sim.read(&acc_out), 8);

        // Step 3: inc=0 → acc stays 8
        sim.drive(&inc_in, 0);
        sim.step();
        assert_eq!(sim.read(&acc_out), 8);
    }

    #[test]
    fn test_sim_mux_select() {
        let (_, circuit) = Circuit::elaborate(|| {
            let a = Signal::<U8>::input_port_default("a");
            let b = Signal::<U8>::input_port_default("b");
            let sel = Signal::<U1>::input_port_default("sel");
            let out = sel.mux(a, b);
            Signal::output_port("out", out);
        });

        circuit.validate().unwrap();
        let mut sim = Sim::new(&circuit);

        let sel_in = sim.input::<U1>("sel");
        let a_in = sim.input::<U8>("a");
        let b_in = sim.input::<U8>("b");
        let out = sim.output::<U8>("out");

        sim.drive(&a_in, 0xAA);
        sim.drive(&b_in, 0x55);

        sim.drive(&sel_in, 1);
        sim.settle();
        assert_eq!(sim.read(&out), 0xAA);

        sim.drive(&sel_in, 0);
        sim.settle();
        assert_eq!(sim.read(&out), 0x55);
    }

    #[test]
    fn test_sim_slice_concat() {
        let (_, circuit) = Circuit::elaborate(|| {
            let wide = Signal::<U8>::input_port_default("wide");
            let lo = wide.slice::<U0, U4, U4>();
            let hi = wide.slice::<U4, U8, U4>();
            let cat = hi.concat(lo);
            Signal::output_port("lo", lo);
            Signal::output_port("hi", hi);
            Signal::output_port("cat", cat);
        });

        circuit.validate().unwrap();
        let mut sim = Sim::new(&circuit);

        let wide_in = sim.input::<U8>("wide");
        let lo_out = sim.output::<U4>("lo");
        let hi_out = sim.output::<U4>("hi");
        let cat_out = sim.output::<U8>("cat");

        sim.drive(&wide_in, 0xAB);
        sim.settle();

        assert_eq!(sim.read(&lo_out), 0xB);
        assert_eq!(sim.read(&hi_out), 0xA);
        assert_eq!(sim.read(&cat_out), 0xAB);
    }

    #[test]
    fn test_sim_forward_declared_wire() {
        // `w` is read before it is assigned: evaluation order must follow
        // the data dependencies, not the order primitives were created in.
        let (_, circuit) = Circuit::elaborate(|| {
            let a = Signal::<U8>::input_port_default("a");
            let w_u = Signal::<U8>::default();
            let y = w_u.read() + 1u64;
            w_u.assign(a + 2u64);
            Signal::output_port("y", y);
        });

        circuit.validate().unwrap();
        let mut sim = Sim::new(&circuit);
        let a_in = sim.input::<U8>("a");
        let y_out = sim.output::<U8>("y");
        sim.drive(&a_in, 10);
        sim.settle();
        assert_eq!(sim.read(&y_out), 13);
    }

    #[test]
    fn test_sim_shifts_and_compare() {
        let (_, circuit) = Circuit::elaborate(|| {
            let a = Signal::<U8>::input_port_default("a");
            let n = Signal::<U3>::input_port_default("n");
            Signal::output_port("shl", a << n);
            Signal::output_port("shr", a >> 4u64);
            Signal::output_port("ge", a.ge(100u64));
            Signal::output_port("x", a ^ 0xFFu64);
            Signal::output_port("wide", a.zext(U16::default()) * 4u64);
        });

        let mut sim = Sim::new(&circuit);
        let a_in = sim.input::<U8>("a");
        let n_in = sim.input::<U3>("n");
        sim.drive(&a_in, 0xC3);
        sim.drive(&n_in, 2);
        sim.settle();
        assert_eq!(sim.read(&sim.output::<U8>("shl")), 0x0C);
        assert_eq!(sim.read(&sim.output::<U8>("shr")), 0x0C);
        assert_eq!(sim.read(&sim.output::<U1>("ge")), 1);
        assert_eq!(sim.read(&sim.output::<U8>("x")), 0x3C);
        assert_eq!(sim.read(&sim.output::<U16>("wide")), 0x30C);
    }

    #[test]
    #[should_panic(expected = "handle expects 4")]
    fn test_sim_port_width_checked() {
        let (_, circuit) = Circuit::elaborate(|| {
            Signal::<U8>::input_port_default("a").output_port("y");
        });
        let sim = Sim::new(&circuit);
        let _ = sim.output::<U4>("y");
    }

    #[test]
    fn test_sim_two_clock_domains() {
        use crate::signal::Domain;
        struct Fast;
        struct Slow;

        let (_, circuit) = Circuit::elaborate(|| {
            Domain::<Fast>::ports("fast", "fast_rst");
            Domain::<Slow>::ports("slow", "slow_rst");
            // Registers are clocked by their signal's domain.
            let a_u = Signal::<U8, Fast>::default();
            let a = a_u.read();
            a_u.next(a + 1u64);
            let b_u = Signal::<U8, Slow>::default();
            let b = b_u.read();
            b_u.next(b + 1u64);
            Signal::output_port("a", a);
            Signal::output_port("b", b);
        });

        circuit.validate().unwrap();
        let mut sim = Sim::new(&circuit);
        let (a, b) = (sim.output::<U8>("a"), sim.output::<U8>("b"));
        sim.step_clock("fast");
        sim.step_clock("fast");
        sim.step_clock("slow");
        assert_eq!((sim.read(&a), sim.read(&b)), (2, 1));
        sim.step();
        assert_eq!((sim.read(&a), sim.read(&b)), (3, 2));
        // The default domain was never used, so no `clk` port exists.
        assert!(circuit.port("clk").is_none());
    }

    #[test]
    fn test_sim_reset() {
        let (_, circuit) = Circuit::elaborate(|| {
            crate::Domain::<crate::signal::Sys>::ports("clk", "rst");
            let inc = Signal::<U8>::input_port_default("inc");
            let acc_u = Signal::<U8>::default();
            let acc = acc_u.read();
            acc_u.next(acc + inc);
            Signal::output_port("acc", acc);
        });

        circuit.validate().unwrap();
        let mut sim = Sim::new(&circuit);

        let inc_in = sim.input::<U8>("inc");
        let acc_out = sim.output::<U8>("acc");

        sim.drive(&inc_in, 10);
        sim.step();
        sim.step();
        assert_eq!(sim.read(&acc_out), 20);

        sim.reset();
        assert_eq!(sim.read(&acc_out), 0);
    }

    #[test]
    fn test_sim_memory() {
        use crate::mem::{Mem, Rom};
        let (_, circuit) = Circuit::elaborate(|| {
            crate::Domain::<crate::signal::Sys>::ports("clk", "rst");
            let we = Signal::<U1>::input_port_default("we");
            let waddr = Signal::<U3>::input_port_default("waddr");
            let wdata = Signal::<U8>::input_port_default("wdata");
            let re = Signal::<U1>::input_port_default("re");
            let raddr = Signal::<U3>::input_port_default("raddr");
            let ram = Mem::new(8, 6, we, waddr, wdata, re, raddr);
            Signal::output_port("ram", ram.rdata);
            let rom = Rom::new(U4::default(), &[1, 2, 3], re, raddr);
            Signal::output_port("rom", rom.rdata);
        });

        circuit.validate().unwrap();
        let verilog = circuit.to_verilog("mem_test");
        assert!(verilog.contains("[0:5];"), "{verilog}");
        let mut sim = Sim::new(&circuit);
        let we = sim.input::<U1>("we");
        let waddr = sim.input::<U3>("waddr");
        let wdata = sim.input::<U8>("wdata");
        let re = sim.input::<U1>("re");
        let raddr = sim.input::<U3>("raddr");
        let ram = sim.output::<U8>("ram");
        let rom = sim.output::<U4>("rom");

        // Write 7 at address 2 while reading it: the read sees the old word.
        sim.drive(&we, 1);
        sim.drive(&waddr, 2);
        sim.drive(&wdata, 7);
        sim.drive(&re, 1);
        sim.drive(&raddr, 2);
        sim.step();
        assert_eq!((sim.read(&ram), sim.read(&rom)), (0, 3));

        // The next read sees it.
        sim.drive(&we, 0);
        sim.step();
        assert_eq!(sim.read(&ram), 7);

        // Read data holds while `re` is low; past the end reads 0.
        sim.drive(&re, 0);
        sim.drive(&raddr, 0);
        sim.step();
        assert_eq!((sim.read(&ram), sim.read(&rom)), (7, 3));
        sim.drive(&re, 1);
        sim.drive(&raddr, 7);
        sim.step();
        assert_eq!((sim.read(&ram), sim.read(&rom)), (0, 0));

        // Out-of-range writes are ignored.
        sim.drive(&we, 1);
        sim.drive(&waddr, 6);
        sim.drive(&raddr, 1);
        sim.step();
        sim.drive(&we, 0);
        sim.drive(&raddr, 6);
        sim.step();
        assert_eq!(sim.read(&ram), 0);
    }

    #[test]
    fn test_sim_native_matches_interpreter() {
        use crate::mem::Mem;
        let (_, circuit) = Circuit::elaborate(|| {
            crate::Domain::<crate::signal::Sys>::ports("clk", "rst");
            let a = Signal::<U8>::input_port_default("a");
            let b = Signal::<U8>::input_port_default("b");
            let sel = Signal::<U3>::input_port_default("sel");
            let acc_u = Signal::<U16>::default();
            let acc = acc_u.read();
            let wide = a.zext(U16::default()) * b.zext(U16::default());
            let shifted = acc << sel;
            let mixed = (wide ^ shifted) - acc.rev();
            acc_u.next_reset(sel.slice::<U0, U1, U1>().mux(mixed, acc + wide), 7);
            let ram = Mem::new(16, 8, sel.slice::<U0, U1, U1>(), sel, acc, a.slice::<U0, U1, U1>(), b.slice::<U0, U3, U3>());
            Signal::output_port("acc", acc);
            Signal::output_port("ram", ram.rdata);
            Signal::output_port("cat", a.concat(b));
        });
        let mut fast = Sim::new(&circuit);
        let mut slow = Sim::interpreted(&circuit);
        assert!(fast.is_native(), "no C compiler for the native backend");
        assert!(!slow.is_native());
        let mut x = 0x1234_5678_9abc_def0u64;
        for _ in 0..2000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            for sim in [&mut fast, &mut slow] {
                sim.drive(&sim.input::<U8>("a"), x & 0xff);
                sim.drive(&sim.input::<U8>("b"), (x >> 8) & 0xff);
                sim.drive(&sim.input::<U3>("sel"), (x >> 16) & 7);
                sim.drive(&sim.input::<U1>("rst"), ((x >> 20) & 63 == 0) as u64);
                sim.step();
            }
            for port in ["acc", "ram", "cat"] {
                let (f, s) = (fast.read(&fast.output::<usize>(port)), slow.read(&slow.output::<usize>(port)));
                assert_eq!(f, s, "{port}");
            }
        }
    }
}
