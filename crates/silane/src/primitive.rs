use std::fmt::Debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDirection {
    Input,
    Output,
}

impl PortDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            PortDirection::Input => "input",
            PortDirection::Output => "output",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PortBinding {
    pub name: &'static str,
    pub direction: PortDirection,
    pub nets: Vec<usize>,
}

impl PortBinding {
    pub fn input(name: &'static str, net: usize) -> Self {
        Self {
            name,
            direction: PortDirection::Input,
            nets: vec![net],
        }
    }

    pub fn output(name: &'static str, net: usize) -> Self {
        Self {
            name,
            direction: PortDirection::Output,
            nets: vec![net],
        }
    }
}

/// Trait defining any primitive hardware block on the circuit graph.
pub trait Primitive: Debug + Send + Sync {
    /// Name of the primitive block (e.g. "$add", "$sub", "$and", "$or", "$mux", "$dff", "$const")
    fn name(&self) -> &str;

    /// Input wire node IDs entering this primitive
    fn inputs(&self) -> Vec<usize>;

    /// Output wire node IDs driven by this primitive
    fn outputs(&self) -> Vec<usize>;

    /// Port bindings with direction and connected net IDs for Netlist-SVG schema
    fn port_bindings(&self) -> Vec<PortBinding>;

    /// Label formatted for display
    fn label(&self) -> String {
        self.name().to_string()
    }

    /// Whether this primitive is a state element. Sequential primitives break
    /// combinational paths: their outputs do not depend on their inputs within a cycle.
    fn is_sequential(&self) -> bool {
        false
    }

    /// Clock of a sequential primitive.
    fn clock(&self) -> Option<usize> {
        None
    }

    /// Inputs that are clocks or resets rather than data: all but these are sampled
    /// by [`Primitive::clock`].
    fn control_inputs(&self) -> Vec<usize> {
        Vec::new()
    }

    /// Upcasting method for Any downcasting
    fn as_any(&self) -> &dyn std::any::Any;
}

// Fundamental Hardware Primitives

#[derive(Debug, Clone)]
pub struct ConstPrim {
    pub value: u64,
    pub width: usize,
    pub out: usize,
}
impl Primitive for ConstPrim {
    fn name(&self) -> &str {
        "$const"
    }
    fn inputs(&self) -> Vec<usize> {
        vec![]
    }
    fn outputs(&self) -> Vec<usize> {
        vec![self.out]
    }
    fn port_bindings(&self) -> Vec<PortBinding> {
        vec![PortBinding::output("Y", self.out)]
    }
    fn label(&self) -> String {
        format!("CONST({})", self.value)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Two-input operators. Arithmetic and bitwise operators produce a result as wide as
/// their operands (wrapping); comparisons produce a single bit. Shifts take an amount
/// of any width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    /// Equality comparison (1-bit result).
    Eq,
    /// Unsigned less-than comparison (1-bit result).
    Lt,
    /// Logical shift left by a dynamic amount.
    Shl,
    /// Logical shift right by a dynamic amount.
    Shr,
}

impl BinaryOp {
    /// Yosys cell type name.
    pub fn cell_name(&self) -> &'static str {
        match self {
            BinaryOp::Add => "$add",
            BinaryOp::Sub => "$sub",
            BinaryOp::Mul => "$mul",
            BinaryOp::And => "$and",
            BinaryOp::Or => "$or",
            BinaryOp::Xor => "$xor",
            BinaryOp::Eq => "$eq",
            BinaryOp::Lt => "$lt",
            BinaryOp::Shl => "$shl",
            BinaryOp::Shr => "$shr",
        }
    }

    /// Verilog infix operator.
    pub fn verilog_op(&self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::And => "&",
            BinaryOp::Or => "|",
            BinaryOp::Xor => "^",
            BinaryOp::Eq => "==",
            BinaryOp::Lt => "<",
            BinaryOp::Shl => "<<",
            BinaryOp::Shr => ">>",
        }
    }

    /// Whether the result is a single bit regardless of operand width.
    pub fn is_comparison(&self) -> bool {
        matches!(self, BinaryOp::Eq | BinaryOp::Lt)
    }
}

#[derive(Debug, Clone)]
pub struct BinaryPrim {
    pub op: BinaryOp,
    pub a: usize,
    pub b: usize,
    pub out: usize,
}
impl Primitive for BinaryPrim {
    fn name(&self) -> &str {
        self.op.cell_name()
    }
    fn inputs(&self) -> Vec<usize> {
        vec![self.a, self.b]
    }
    fn outputs(&self) -> Vec<usize> {
        vec![self.out]
    }
    fn port_bindings(&self) -> Vec<PortBinding> {
        vec![
            PortBinding::input("A", self.a),
            PortBinding::input("B", self.b),
            PortBinding::output("Y", self.out),
        ]
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct MuxPrim {
    pub sel: usize,
    pub in0: usize,
    pub in1: usize,
    pub out: usize,
}
impl Primitive for MuxPrim {
    fn name(&self) -> &str {
        "$mux"
    }
    fn inputs(&self) -> Vec<usize> {
        vec![self.sel, self.in0, self.in1]
    }
    fn outputs(&self) -> Vec<usize> {
        vec![self.out]
    }
    fn port_bindings(&self) -> Vec<PortBinding> {
        vec![
            PortBinding::input("A", self.in0),
            PortBinding::input("B", self.in1),
            PortBinding::input("S", self.sel),
            PortBinding::output("Y", self.out),
        ]
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct SlicePrim {
    pub input: usize,
    pub offset: usize,
    pub width: usize,
    pub out: usize,
}

impl Primitive for SlicePrim {
    fn name(&self) -> &str {
        "$slice"
    }

    fn inputs(&self) -> Vec<usize> {
        vec![self.input]
    }

    fn outputs(&self) -> Vec<usize> {
        vec![self.out]
    }

    fn port_bindings(&self) -> Vec<PortBinding> {
        vec![
            PortBinding::input("A", self.input),
            PortBinding::output("Y", self.out),
        ]
    }

    fn label(&self) -> String {
        format!("SLICE[{}:{}]", self.offset + self.width - 1, self.offset)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct DffPrim {
    pub clk: usize,
    pub rst: Option<usize>,
    pub reset_val: u64,
    pub d: usize,
    pub q: usize,
}

impl Primitive for DffPrim {
    fn name(&self) -> &str {
        if self.rst.is_some() { "$adff" } else { "$dff" }
    }

    fn inputs(&self) -> Vec<usize> {
        let mut inps = vec![self.clk];
        if let Some(r) = self.rst {
            inps.push(r);
        }
        inps.push(self.d);
        inps
    }

    fn outputs(&self) -> Vec<usize> {
        vec![self.q]
    }

    fn port_bindings(&self) -> Vec<PortBinding> {
        let mut bindings = vec![PortBinding::input("CLK", self.clk)];
        if let Some(r) = self.rst {
            bindings.push(PortBinding::input("ARST", r));
        }
        bindings.push(PortBinding::input("D", self.d));
        bindings.push(PortBinding::output("Q", self.q));
        bindings
    }
    fn clock(&self) -> Option<usize> {
        Some(self.clk)
    }
    fn control_inputs(&self) -> Vec<usize> {
        let mut nets = vec![self.clk];
        nets.extend(self.rst);
        nets
    }
    fn is_sequential(&self) -> bool {
        true
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct NotPrim {
    pub a: usize,
    pub out: usize,
}

impl Primitive for NotPrim {
    fn name(&self) -> &str {
        "$not"
    }
    fn inputs(&self) -> Vec<usize> {
        vec![self.a]
    }
    fn outputs(&self) -> Vec<usize> {
        vec![self.out]
    }
    fn port_bindings(&self) -> Vec<PortBinding> {
        vec![
            PortBinding::input("A", self.a),
            PortBinding::output("Y", self.out),
        ]
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct RevPrim {
    pub a: usize,
    pub width: usize,
    pub out: usize,
}

impl Primitive for RevPrim {
    fn name(&self) -> &str {
        "$rev"
    }
    fn inputs(&self) -> Vec<usize> {
        vec![self.a]
    }
    fn outputs(&self) -> Vec<usize> {
        vec![self.out]
    }
    fn port_bindings(&self) -> Vec<PortBinding> {
        vec![
            PortBinding::input("A", self.a),
            PortBinding::output("Y", self.out),
        ]
    }
    fn label(&self) -> String {
        format!("REV[{}]", self.width)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Concatenation: `a` forms the most significant bits, `b` the least significant.
#[derive(Debug, Clone)]
pub struct ConcatPrim {
    pub a: usize,
    pub b: usize,
    pub out: usize,
}

impl Primitive for ConcatPrim {
    fn name(&self) -> &str {
        "$concat"
    }
    fn inputs(&self) -> Vec<usize> {
        vec![self.a, self.b]
    }
    fn outputs(&self) -> Vec<usize> {
        vec![self.out]
    }
    fn port_bindings(&self) -> Vec<PortBinding> {
        vec![
            PortBinding::input("A", self.a),
            PortBinding::input("B", self.b),
            PortBinding::output("Y", self.out),
        ]
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct BufPrim {
    pub a: usize,
    pub out: usize,
}

impl Primitive for BufPrim {
    fn name(&self) -> &str {
        "$buf"
    }
    fn inputs(&self) -> Vec<usize> {
        vec![self.a]
    }
    fn outputs(&self) -> Vec<usize> {
        vec![self.out]
    }
    fn port_bindings(&self) -> Vec<PortBinding> {
        vec![
            PortBinding::input("A", self.a),
            PortBinding::output("Y", self.out),
        ]
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A synchronous memory of `depth` words of `width` bits: one write port and one
/// registered read port, both clocked by `clk`. On a clock edge, when `re` is high
/// `rdata` takes the word at `raddr` as it was before the edge (read-first), and when
/// `we` is high the word at `waddr` becomes `wdata`. `rdata` holds its value while `re`
/// is low. Addresses past `depth` read 0 and ignore writes.
///
/// This is the shape FPGA block RAMs implement, so the Verilog backend emits it in the
/// form synthesis tools infer as a block RAM. `init` gives the initial contents (a ROM
/// when nothing writes it); missing words start at 0. Memories are not reset.
#[derive(Debug, Clone)]
pub struct MemPrim {
    pub clk: usize,
    pub width: usize,
    pub depth: usize,
    pub we: usize,
    pub waddr: usize,
    pub wdata: usize,
    pub re: usize,
    pub raddr: usize,
    pub rdata: usize,
    pub init: Vec<u64>,
}

impl Primitive for MemPrim {
    fn name(&self) -> &str {
        "$mem"
    }
    fn inputs(&self) -> Vec<usize> {
        vec![self.clk, self.we, self.waddr, self.wdata, self.re, self.raddr]
    }
    fn outputs(&self) -> Vec<usize> {
        vec![self.rdata]
    }
    fn port_bindings(&self) -> Vec<PortBinding> {
        vec![
            PortBinding::input("CLK", self.clk),
            PortBinding::input("WE", self.we),
            PortBinding::input("WADDR", self.waddr),
            PortBinding::input("WDATA", self.wdata),
            PortBinding::input("RE", self.re),
            PortBinding::input("RADDR", self.raddr),
            PortBinding::output("RDATA", self.rdata),
        ]
    }
    fn label(&self) -> String {
        format!("MEM({}x{})", self.depth, self.width)
    }
    fn clock(&self) -> Option<usize> {
        Some(self.clk)
    }
    fn control_inputs(&self) -> Vec<usize> {
        vec![self.clk]
    }
    fn is_sequential(&self) -> bool {
        true
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
