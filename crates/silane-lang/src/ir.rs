//! Elaborated design: one [`ModuleIr`] per module instantiation (parameters resolved,
//! loops unrolled, procedural assignments lowered to multiplexers).

use crate::diag::Span;
use std::collections::HashMap;

pub type NodeId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    UInt(u32),
    /// Index into [`Types::enums`].
    Enum(usize),
    /// Index into [`Types::bundles`].
    Bundle(usize),
    Array(Box<Ty>, u32),
}

/// Enum and bundle definitions referenced by [`Ty`].
#[derive(Debug, Clone, Default)]
pub struct Types {
    pub enums: Vec<EnumDef>,
    pub bundles: Vec<BundleDef>,
}

/// A bundle: the signature of a body-less module (or, internally, of a module
/// instance). Fields from the argument list are `flipped`: they flow into whoever
/// produces the bundle.
#[derive(Debug, Clone)]
pub struct BundleDef {
    pub name: String,
    /// `Stream<W = 8>`
    pub display: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: Ty,
    pub flipped: bool,
}

/// A scalar element of a (possibly aggregate) type.
#[derive(Debug, Clone)]
pub struct Leaf {
    /// Source-level suffix: `.valid`, `[2].data`.
    pub path: String,
    /// Identifier suffix: `_valid`, `_2_data`.
    pub flat: String,
    pub ty: Ty,
    /// Whether the leaf flows against the direction of the whole value.
    pub flipped: bool,
}

impl Ty {
    /// Number of scalar elements.
    pub fn flat_len(&self, types: &Types) -> u32 {
        match self {
            Ty::Array(elem, n) => elem.flat_len(types) * n,
            Ty::Bundle(b) => types.bundles[*b]
                .fields
                .iter()
                .map(|f| f.ty.flat_len(types))
                .sum(),
            _ => 1,
        }
    }

    pub fn is_scalar(&self) -> bool {
        matches!(self, Ty::UInt(_) | Ty::Enum(_))
    }

    /// Whether the type contains a bundle.
    pub fn has_bundle(&self) -> bool {
        match self {
            Ty::Bundle(_) => true,
            Ty::Array(elem, _) => elem.has_bundle(),
            _ => false,
        }
    }

    /// Bit width of a scalar type.
    pub fn width(&self, types: &Types) -> u32 {
        match self {
            Ty::UInt(w) => *w,
            Ty::Enum(e) => types.enums[*e].width,
            Ty::Array(elem, _) => elem.width(types),
            Ty::Bundle(_) => panic!("width of a bundle"),
        }
    }

    /// Scalar elements in flattening order (row-major arrays, fields in order).
    pub fn leaves(&self, types: &Types) -> Vec<Leaf> {
        let mut out = Vec::new();
        self.collect_leaves(types, String::new(), String::new(), false, &mut out);
        out
    }

    fn collect_leaves(&self, types: &Types, path: String, flat: String, flipped: bool, out: &mut Vec<Leaf>) {
        match self {
            Ty::Array(elem, n) => {
                for i in 0..*n {
                    elem.collect_leaves(types, format!("{path}[{i}]"), format!("{flat}_{i}"), flipped, out);
                }
            }
            Ty::Bundle(b) => {
                for f in &types.bundles[*b].fields {
                    f.ty.collect_leaves(
                        types,
                        format!("{path}.{}", f.name),
                        format!("{flat}_{}", f.name),
                        flipped ^ f.flipped,
                        out,
                    );
                }
            }
            scalar => out.push(Leaf {
                path,
                flat,
                ty: scalar.clone(),
                flipped,
            }),
        }
    }

    pub fn display(&self, types: &Types) -> String {
        match self {
            Ty::UInt(w) => format!("u{w}"),
            Ty::Enum(e) => types.enums[*e].name.clone(),
            Ty::Bundle(b) => types.bundles[*b].display.clone(),
            Ty::Array(elem, n) => format!("[{}; {n}]", elem.display(types)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    pub width: u32,
    pub variants: Vec<(String, u64)>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinKind {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
}

impl BinKind {
    pub fn is_comparison(&self) -> bool {
        matches!(
            self,
            BinKind::Eq | BinKind::Ne | BinKind::Lt | BinKind::Le | BinKind::Gt | BinKind::Ge
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Node {
    /// Flattened input port element.
    Input(u32),
    Const(u64),
    /// Value of a target: the current output of a register, or the final value of a wire.
    Net(u32),
    /// Flattened output element of an instance.
    InstOut(u32, u32),
    Not(NodeId),
    Bin(BinKind, NodeId, NodeId),
    /// `Mux(sel, then, else)`
    Mux(NodeId, NodeId, NodeId),
    /// `Slice(input, offset)`; the width is the node's width.
    Slice(NodeId, u32),
    /// `Cat(msb, lsb)`
    Cat(NodeId, NodeId),
    Rev(NodeId),
    /// Zero-extension to the node's width.
    Zext(NodeId),
}

impl Node {
    pub fn operands(&self) -> Vec<NodeId> {
        match *self {
            Node::Input(_) | Node::Const(_) | Node::Net(_) | Node::InstOut(..) => vec![],
            Node::Not(a) | Node::Slice(a, _) | Node::Rev(a) | Node::Zext(a) => vec![a],
            Node::Bin(_, a, b) | Node::Cat(a, b) => vec![a, b],
            Node::Mux(c, t, f) => vec![c, t, f],
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeData {
    pub node: Node,
    pub width: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Wire,
    Reg,
}

/// A driven scalar net: a wire (combinational) or a register.
#[derive(Debug, Clone)]
pub struct Target {
    /// Source-level name, e.g. `count`, `mem[3]` or `fifo.push`.
    pub name: String,
    pub kind: TargetKind,
    pub width: u32,
    pub reset: u64,
    pub driver: Option<NodeId>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafDir {
    /// Flattened input index.
    In(u32),
    /// Flattened output index and the target driving it.
    Out(u32, u32),
}

#[derive(Debug, Clone)]
pub struct PortLeaf {
    /// `enq.valid`
    pub path: String,
    /// `enq_valid`: the circuit port name.
    pub flat_name: String,
    pub width: u32,
    pub dir: LeafDir,
}

#[derive(Debug, Clone)]
pub struct Port {
    pub name: String,
    pub ty: Ty,
    /// Declared in the argument list (as opposed to `-> (...)`).
    pub is_arg: bool,
    /// Domain label of an extern module's port (see [`crate::ast::PortDecl::domain`]).
    pub domain: Option<String>,
    pub leaves: Vec<PortLeaf>,
}

#[derive(Debug, Clone)]
pub struct Instance {
    /// Index into [`Design::modules`].
    pub module: usize,
    pub name: String,
    /// One node per flattened input of the instantiated module.
    pub inputs: Vec<NodeId>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ModuleIr {
    /// Name of the generated Rust type.
    pub rust_name: String,
    /// `Fifo<DEPTH = 16>`
    pub display_name: String,
    pub source_name: String,
    pub span: Span,
    pub params: Vec<(String, i128)>,
    /// Rust path of a module implemented in Rust (declared `module M(...) = "path";`).
    pub rust_path: Option<String>,
    pub ports: Vec<Port>,
    pub input_count: u32,
    /// Target driving every flattened output.
    pub out_targets: Vec<u32>,
    pub nodes: Vec<NodeData>,
    pub names: HashMap<NodeId, String>,
    pub targets: Vec<Target>,
    pub instances: Vec<Instance>,
    /// For every flattened output: the flattened inputs it depends on combinationally.
    pub comb_deps: Vec<Vec<u32>>,
    /// Source location each node was created for.
    pub node_spans: Vec<Span>,
    /// Location recorded for nodes added from now on.
    pub cur_span: Span,
    dedup: HashMap<(Node, u32), NodeId>,
}

impl ModuleIr {
    pub fn new(source_name: String, span: Span) -> Self {
        ModuleIr {
            rust_name: source_name.clone(),
            display_name: source_name.clone(),
            source_name,
            span,
            params: Vec::new(),
            rust_path: None,
            ports: Vec::new(),
            input_count: 0,
            out_targets: Vec::new(),
            nodes: Vec::new(),
            names: HashMap::new(),
            targets: Vec::new(),
            instances: Vec::new(),
            comb_deps: Vec::new(),
            node_spans: Vec::new(),
            cur_span: span,
            dedup: HashMap::new(),
        }
    }

    pub fn output_count(&self) -> u32 {
        self.out_targets.len() as u32
    }

    /// Adds a node, reusing an identical existing one.
    pub fn add(&mut self, node: Node, width: u32) -> NodeId {
        let key = (node, width);
        if let Some(&id) = self.dedup.get(&key) {
            return id;
        }
        let id = self.nodes.len() as NodeId;
        self.nodes.push(NodeData {
            node: key.0.clone(),
            width,
        });
        self.node_spans.push(self.cur_span);
        self.dedup.insert(key, id);
        id
    }

    pub fn width(&self, id: NodeId) -> u32 {
        self.nodes[id as usize].width
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize].node
    }

    pub fn const_value(&self, id: NodeId) -> Option<u64> {
        match self.node(id) {
            Node::Const(v) => Some(*v),
            _ => None,
        }
    }

    /// `sel ? then : els`, folding constant selectors and identical branches.
    pub fn mux(&mut self, sel: NodeId, then: NodeId, els: NodeId) -> NodeId {
        if then == els {
            return then;
        }
        match self.const_value(sel) {
            Some(0) => els,
            Some(_) => then,
            None => {
                let w = self.width(then);
                self.add(Node::Mux(sel, then, els), w)
            }
        }
    }

    pub fn name(&mut self, id: NodeId, name: &str) {
        self.names.entry(id).or_insert_with(|| name.to_string());
    }
}

#[derive(Debug, Clone, Default)]
pub struct Design {
    pub modules: Vec<ModuleIr>,
    pub types: Types,
    /// `module Alias = Target::<...>;` declarations: (alias name, module index).
    pub aliases: Vec<(String, usize)>,
}

impl Design {
    /// Finds a module by its Rust name or alias name.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.aliases
            .iter()
            .find(|(alias, _)| alias == name)
            .map(|(_, idx)| *idx)
            .or_else(|| self.modules.iter().position(|m| m.rust_name == name))
    }
}

pub fn mask(value: u64, width: u32) -> u64 {
    if width >= 64 {
        value
    } else {
        value & ((1u64 << width) - 1)
    }
}

/// Evaluates a binary operator on constants of the given operand width.
pub fn fold_bin(op: BinKind, a: u64, b: u64, width: u32) -> u64 {
    match op {
        BinKind::Add => mask(a.wrapping_add(b), width),
        BinKind::Sub => mask(a.wrapping_sub(b), width),
        BinKind::Mul => mask(a.wrapping_mul(b), width),
        BinKind::And => a & b,
        BinKind::Or => a | b,
        BinKind::Xor => a ^ b,
        BinKind::Eq => (a == b) as u64,
        BinKind::Ne => (a != b) as u64,
        BinKind::Lt => (a < b) as u64,
        BinKind::Le => (a <= b) as u64,
        BinKind::Gt => (a > b) as u64,
        BinKind::Ge => (a >= b) as u64,
        BinKind::Shl => mask(a.checked_shl(b.min(64) as u32).unwrap_or(0), width),
        BinKind::Shr => a.checked_shr(b.min(64) as u32).unwrap_or(0),
    }
}

/// One step of building a module: a node, or an instance (which must come after the
/// nodes feeding its inputs and before the nodes reading its outputs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Node(NodeId),
    Instance(u32),
}

/// Orders every node and instance a module needs so that each step comes after the
/// steps it reads. Wire and register reads are leaves: their drivers are connected
/// after all steps, so feedback through them needs no ordering.
pub fn schedule(m: &ModuleIr) -> Vec<Step> {
    enum Work {
        Enter(Step),
        Exit(Step),
    }
    let mut node_seen = vec![false; m.nodes.len()];
    let mut inst_seen = vec![false; m.instances.len()];
    let mut steps = Vec::new();
    let mut work: Vec<Work> = Vec::new();

    let roots = m
        .targets
        .iter()
        .rev()
        .filter_map(|t| t.driver.map(Step::Node))
        .chain((0..m.instances.len() as u32).rev().map(Step::Instance));
    for root in roots {
        work.push(Work::Enter(root));
        while let Some(item) = work.pop() {
            match item {
                Work::Enter(step) => {
                    let seen = match step {
                        Step::Node(n) => &mut node_seen[n as usize],
                        Step::Instance(i) => &mut inst_seen[i as usize],
                    };
                    if *seen {
                        continue;
                    }
                    *seen = true;
                    work.push(Work::Exit(step));
                    let deps: Vec<Step> = match step {
                        Step::Node(n) => match m.node(n) {
                            Node::InstOut(i, _) => vec![Step::Instance(*i)],
                            other => other.operands().into_iter().map(Step::Node).collect(),
                        },
                        Step::Instance(i) => m.instances[i as usize]
                            .inputs
                            .iter()
                            .map(|&n| Step::Node(n))
                            .collect(),
                    };
                    for dep in deps.into_iter().rev() {
                        work.push(Work::Enter(dep));
                    }
                }
                Work::Exit(step) => steps.push(step),
            }
        }
    }
    steps
}
