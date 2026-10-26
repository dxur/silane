//! Direct backend: builds a [`silane::Circuit`] from the elaborated design without
//! going through generated Rust. Used by `silc verilog` and the compiler's tests.
//! The resulting circuit has the same ports as the generated `circuit()` function.

use crate::elab::MEM_PATH;
use crate::ir::{BinKind, Design, LeafDir, Node, Step, TargetKind, schedule};
use silane::{Bound, Circuit, Domain, Signal, Sys, UnBound, Wire};

type W = Wire<Signal<usize>, Bound>;

/// Whether module `idx` contains, at any depth, a module implemented in Rust (which
/// only the generated Rust can build). The `std:mem` RAM is built directly.
pub fn uses_rust_modules(design: &Design, idx: usize) -> bool {
    let m = &design.modules[idx];
    match &m.rust_path {
        Some(path) => path != MEM_PATH,
        None => m.instances.iter().any(|i| uses_rust_modules(design, i.module)),
    }
}

/// Elaborates module `top` (an index into `design.modules`) as a top-level circuit.
///
/// # Panics
/// If the design uses modules implemented in Rust (see [`uses_rust_modules`]).
pub fn build_circuit(design: &Design, top: usize) -> Circuit {
    let ((), circuit) = Circuit::elaborate(|| {
        // Without Rust modules, a design has no synchronizer: one clock domain.
        Domain::<Sys>::ports_on_use("clk", "rst");
        let m = &design.modules[top];
        let leaves: Vec<_> = m.ports.iter().flat_map(|p| p.leaves.iter()).collect();
        let mut in_leaves: Vec<_> = leaves
            .iter()
            .filter_map(|l| match l.dir {
                LeafDir::In(k) => Some((k, l)),
                LeafDir::Out(..) => None,
            })
            .collect();
        in_leaves.sort_by_key(|(k, _)| *k);
        let inputs: Vec<W> = in_leaves
            .iter()
            .map(|(_, l)| Signal::<usize>::input_port(&l.flat_name, l.width as usize))
            .collect();
        let outputs = instantiate(design, top, &inputs);
        let mut out_leaves: Vec<_> = leaves
            .iter()
            .filter_map(|l| match l.dir {
                LeafDir::Out(o, _) => Some((o, l)),
                LeafDir::In(_) => None,
            })
            .collect();
        out_leaves.sort_by_key(|(o, _)| *o);
        for (o, l) in out_leaves {
            outputs[o as usize].output_port(&l.flat_name);
        }
    });
    circuit
}

/// Builds one instance of module `idx` and returns its flattened outputs.
fn instantiate(design: &Design, idx: usize, inputs: &[W]) -> Vec<W> {
    let m = &design.modules[idx];
    if m.rust_path.as_deref() == Some(MEM_PATH) {
        let param = |name: &str| m.params.iter().find(|(n, _)| n == name).expect("Mem parameter").1;
        let [we, waddr, wdata, re, raddr] = inputs else {
            panic!("Mem takes five inputs");
        };
        let ram = silane::mem::Mem::new(param("W") as u64, param("D") as u64, we.typed(), *waddr, *wdata, re.typed(), *raddr);
        return vec![ram.rdata];
    }
    assert!(
        m.rust_path.is_none(),
        "module `{}` is implemented in Rust; build the design through the generated code",
        m.display_name
    );
    let drivers: Vec<Wire<Signal<usize>, UnBound>> = m
        .targets
        .iter()
        .map(|t| Signal::<usize>::new(t.width as usize).named(&t.name))
        .collect();
    let reads: Vec<W> = drivers.iter().map(|d| d.read()).collect();

    let mut values: Vec<Option<W>> = vec![None; m.nodes.len()];
    let mut inst_outputs: Vec<Vec<W>> = vec![Vec::new(); m.instances.len()];
    for step in schedule(m) {
        match step {
            Step::Instance(i) => {
                let inst = &m.instances[i as usize];
                let args: Vec<W> = inst
                    .inputs
                    .iter()
                    .map(|&n| values[n as usize].expect("scheduled input"))
                    .collect();
                inst_outputs[i as usize] = instantiate(design, inst.module, &args);
            }
            Step::Node(n) => {
                let data = &m.nodes[n as usize];
                let width = data.width as usize;
                let v = |id: u32| values[id as usize].expect("scheduled operand");
                let wire = match &data.node {
                    Node::Input(k) => inputs[*k as usize],
                    Node::Const(c) => Signal::<usize>::constant(*c, width),
                    Node::Net(t) => reads[*t as usize],
                    Node::InstOut(i, k) => inst_outputs[*i as usize][*k as usize],
                    Node::Not(a) => !v(*a),
                    Node::Bin(op, a, b) => {
                        let (a, b) = (v(*a), v(*b));
                        match op {
                            BinKind::Add => a + b,
                            BinKind::Sub => a - b,
                            BinKind::Mul => a * b,
                            BinKind::And => a & b,
                            BinKind::Or => a | b,
                            BinKind::Xor => a ^ b,
                            BinKind::Shl => a << b,
                            BinKind::Shr => a >> b,
                            BinKind::Eq => a.eq(b).erase(),
                            BinKind::Ne => a.ne(b).erase(),
                            BinKind::Lt => a.lt(b).erase(),
                            BinKind::Le => a.le(b).erase(),
                            BinKind::Gt => a.gt(b).erase(),
                            BinKind::Ge => a.ge(b).erase(),
                        }
                    }
                    Node::Mux(c, t, f) => v(*c).mux(v(*t), v(*f)),
                    Node::Slice(a, lo) => v(*a).slice_dyn(*lo as usize, width),
                    Node::Cat(a, b) => v(*a).concat(v(*b)),
                    Node::Rev(a) => v(*a).rev(),
                    Node::Zext(a) => v(*a).zext(width),
                };
                let wire = match m.names.get(&n) {
                    Some(name) => wire.named(name),
                    None => wire,
                };
                values[n as usize] = Some(wire);
            }
        }
    }

    for (driver, target) in drivers.into_iter().zip(&m.targets) {
        let Some(d) = target.driver else { continue };
        let value = values[d as usize].expect("scheduled driver");
        match target.kind {
            TargetKind::Wire => driver.assign(value),
            TargetKind::Reg => driver.next_reset(value, target.reset),
        }
    }

    m.out_targets.iter().map(|&t| reads[t as usize]).collect()
}

