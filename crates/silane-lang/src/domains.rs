//! Clock domain inference. A `.si` module never names a clock: its signals are in
//! whatever domains they are used in. Every module gets as many domain parameters as
//! its ports allow, so the generated Rust accepts any binding that is valid:
//!
//! - logic, and a register and the value it samples, are in one domain;
//! - an instance joins its pins as its own domains do, so domains flow through the
//!   hierarchy;
//! - an `extern module` keeps ports with different `@labels` apart (unlabelled ports
//!   share one domain): that is how a synchronizer crosses domains.
//!
//! Ports in one class share a parameter, numbered `D0, D1, ...` in port order. Signals
//! connected to no port (dead logic, constants) join `D0`.

use crate::codegen::interface;
use crate::ir::{Design, LeafDir, ModuleIr, Node};

/// Domains of one module.
#[derive(Debug, Clone, Default)]
pub struct Domains {
    /// Number of domain parameters.
    pub count: u32,
    /// Parameter of every flattened input and output.
    pub inputs: Vec<u32>,
    pub outputs: Vec<u32>,
    /// Parameter of every node and target (empty for extern modules).
    pub nodes: Vec<u32>,
    pub targets: Vec<u32>,
    /// For every instance, the parameter of each of the child's parameters.
    pub instances: Vec<Vec<u32>>,
}

struct UnionFind(Vec<u32>);

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind((0..n as u32).collect())
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            let parent = self.0[self.0[x as usize] as usize];
            self.0[x as usize] = parent;
            x = parent;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b) as usize] = a.min(b);
        }
    }
}

/// Infers the domains of every module of `design`.
pub fn infer(design: &Design) -> Vec<Domains> {
    let mut memo: Vec<Option<Domains>> = vec![None; design.modules.len()];
    for idx in 0..design.modules.len() {
        module(design, idx, &mut memo);
    }
    memo.into_iter().map(|d| d.expect("inferred")).collect()
}

fn module(design: &Design, idx: usize, memo: &mut Vec<Option<Domains>>) -> Domains {
    if let Some(d) = &memo[idx] {
        return d.clone();
    }
    let m = &design.modules[idx];
    let domains = if m.rust_path.is_some() {
        external(m)
    } else {
        let children: Vec<Domains> = m.instances.iter().map(|i| module(design, i.module, memo)).collect();
        internal(design, m, &children)
    };
    memo[idx] = Some(domains.clone());
    domains
}

/// An `extern module`: one domain per distinct port label, unlabelled ports sharing one.
fn external(m: &ModuleIr) -> Domains {
    let mut labels: Vec<Option<&str>> = Vec::new();
    let mut d = Domains {
        inputs: vec![0; m.input_count as usize],
        outputs: vec![0; m.out_targets.len()],
        ..Domains::default()
    };
    for port in &m.ports {
        let label = port.domain.as_deref();
        let class = match labels.iter().position(|&l| l == label) {
            Some(c) => c,
            None => {
                labels.push(label);
                labels.len() - 1
            }
        } as u32;
        for leaf in &port.leaves {
            match leaf.dir {
                LeafDir::In(k) => d.inputs[k as usize] = class,
                LeafDir::Out(o, _) => d.outputs[o as usize] = class,
            }
        }
    }
    d.count = labels.len().max(1) as u32;
    d
}

/// A module with a body: unifies what its logic connects.
fn internal(design: &Design, m: &ModuleIr, children: &[Domains]) -> Domains {
    // Variables: inputs, then nodes, then targets, then each instance's parameters.
    let n_in = m.input_count as usize;
    let node0 = n_in;
    let target0 = node0 + m.nodes.len();
    let mut inst0 = Vec::new();
    let mut total = target0 + m.targets.len();
    for child in children {
        inst0.push(total);
        total += child.count as usize;
    }
    let mut uf = UnionFind::new(total);
    let node = |n: u32| (node0 + n as usize) as u32;
    let target = |t: u32| (target0 + t as usize) as u32;

    for (n, data) in m.nodes.iter().enumerate() {
        let me = node(n as u32);
        match &data.node {
            Node::Input(k) => uf.union(me, *k),
            Node::Net(t) => uf.union(me, target(*t)),
            Node::InstOut(i, o) => {
                let class = children[*i as usize].outputs[*o as usize];
                uf.union(me, (inst0[*i as usize] + class as usize) as u32);
            }
            Node::Const(_) => {}
            other => {
                for &a in other.operands().iter() {
                    uf.union(me, node(a));
                }
            }
        }
    }
    for (t, target_ir) in m.targets.iter().enumerate() {
        if let Some(d) = target_ir.driver {
            uf.union(target(t as u32), node(d));
        }
    }
    for (i, inst) in m.instances.iter().enumerate() {
        for (k, &n) in inst.inputs.iter().enumerate() {
            let class = children[i].inputs[k];
            uf.union(node(n), (inst0[i] + class as usize) as u32);
        }
    }
    // Every Rust parameter or field (a whole array) has a single type.
    let iface = interface(m, design);
    for item in &iface.inputs {
        for pair in item.flat.windows(2) {
            uf.union(pair[0], pair[1]);
        }
    }
    for item in &iface.outputs {
        for pair in item.flat.windows(2) {
            let (a, b) = (m.out_targets[pair[0] as usize], m.out_targets[pair[1] as usize]);
            uf.union(target(a), target(b));
        }
    }

    // Number the classes of the ports, inputs first.
    let mut params: Vec<u32> = Vec::new();
    let mut param_of = |uf: &mut UnionFind, var: u32| -> u32 {
        let root = uf.find(var);
        match params.iter().position(|&r| r == root) {
            Some(p) => p as u32,
            None => {
                params.push(root);
                params.len() as u32 - 1
            }
        }
    };
    let inputs: Vec<u32> = (0..n_in as u32).map(|k| param_of(&mut uf, k)).collect();
    let outputs: Vec<u32> = m.out_targets.iter().map(|&t| param_of(&mut uf, target(t))).collect();
    let count = params.len().max(1) as u32;
    let rest = |uf: &mut UnionFind, var: u32| -> u32 {
        let root = uf.find(var);
        params.iter().position(|&r| r == root).unwrap_or(0) as u32
    };
    let nodes = (0..m.nodes.len() as u32).map(|n| rest(&mut uf, node(n))).collect();
    let targets = (0..m.targets.len() as u32).map(|t| rest(&mut uf, target(t))).collect();
    let instances = children
        .iter()
        .enumerate()
        .map(|(i, child)| (0..child.count).map(|c| rest(&mut uf, (inst0[i] + c as usize) as u32)).collect())
        .collect();
    Domains {
        count,
        inputs,
        outputs,
        nodes,
        targets,
        instances,
    }
}
