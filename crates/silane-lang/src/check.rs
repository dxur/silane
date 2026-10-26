//! Post-elaboration checks: combinational loops, and the combinational
//! input→output dependencies of every module (so loops through instances are found).

use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::{ModuleIr, Node, NodeId, TargetKind};

/// Combinational successors of a node: what it reads within the same cycle.
fn successors(ir: &ModuleIr, modules: &[ModuleIr], n: NodeId) -> Vec<NodeId> {
    match ir.node(n) {
        Node::Net(t) => {
            let target = &ir.targets[*t as usize];
            match target.kind {
                TargetKind::Wire => target.driver.into_iter().collect(),
                TargetKind::Reg => Vec::new(),
            }
        }
        Node::InstOut(i, k) => {
            let inst = &ir.instances[*i as usize];
            modules[inst.module].comb_deps[*k as usize]
                .iter()
                .map(|&j| inst.inputs[j as usize])
                .collect()
        }
        other => other.operands(),
    }
}

/// Detects combinational loops and fills in `ir.comb_deps`. Returns `false` if an
/// error was reported.
pub fn analyze(ir: &mut ModuleIr, modules: &[ModuleIr], diags: &mut Diagnostics) -> bool {
    let count = ir.nodes.len();
    let succ: Vec<Vec<NodeId>> = (0..count as NodeId)
        .map(|n| successors(ir, modules, n))
        .collect();

    // Iterative DFS: 0 = unvisited, 1 = on stack, 2 = done.
    let mut state = vec![0u8; count];
    let mut order = Vec::with_capacity(count);
    let mut ok = true;
    let mut reported = 0;
    for root in 0..count {
        if state[root] != 0 {
            continue;
        }
        let mut stack: Vec<(NodeId, usize)> = vec![(root as NodeId, 0)];
        state[root] = 1;
        while let Some(&mut (n, ref mut next)) = stack.last_mut() {
            if let Some(&s) = succ[n as usize].get(*next) {
                *next += 1;
                match state[s as usize] {
                    0 => {
                        state[s as usize] = 1;
                        stack.push((s, 0));
                    }
                    1 => {
                        ok = false;
                        if reported < 3 {
                            reported += 1;
                            report_cycle(ir, modules, &stack, s, diags);
                        }
                    }
                    _ => {}
                }
            } else {
                state[n as usize] = 2;
                order.push(n);
                stack.pop();
            }
        }
    }
    if !ok {
        ir.comb_deps = vec![Vec::new(); ir.output_count() as usize];
        return false;
    }

    // Post-order guarantees successors are processed first.
    let mut deps: Vec<Vec<u32>> = vec![Vec::new(); count];
    for &n in &order {
        let mut set: Vec<u32> = match ir.node(n) {
            Node::Input(k) => vec![*k],
            _ => Vec::new(),
        };
        for &s in &succ[n as usize] {
            set.extend_from_slice(&deps[s as usize]);
        }
        set.sort_unstable();
        set.dedup();
        deps[n as usize] = set;
    }
    ir.comb_deps = ir
        .out_targets
        .iter()
        .map(|&t| match ir.targets[t as usize].driver {
            Some(d) if ir.targets[t as usize].kind == TargetKind::Wire => deps[d as usize].clone(),
            _ => Vec::new(),
        })
        .collect();
    true
}

fn report_cycle(
    ir: &ModuleIr,
    modules: &[ModuleIr],
    stack: &[(NodeId, usize)],
    back_to: NodeId,
    diags: &mut Diagnostics,
) {
    let start = stack.iter().position(|(n, _)| *n == back_to).unwrap_or(0);
    let mut names = Vec::new();
    let mut spans = Vec::new();
    for &(n, _) in &stack[start..] {
        match ir.node(n) {
            Node::Net(t) => {
                let target = &ir.targets[*t as usize];
                names.push(format!("`{}`", target.name));
                spans.push((target.span, format!("`{}` is part of the loop", target.name)));
            }
            Node::InstOut(i, k) => {
                let inst = &ir.instances[*i as usize];
                let module = &modules[inst.module];
                let port = module
                    .ports
                    .iter()
                    .flat_map(|p| p.leaves.iter())
                    .find(|l| matches!(l.dir, crate::ir::LeafDir::Out(o, _) if o == *k))
                    .map_or("?", |l| l.path.as_str());
                names.push(format!("`{}.{port}`", inst.name));
                spans.push((inst.span, format!("`{port}` depends combinationally on its inputs")));
            }
            _ => {}
        }
    }
    if names.is_empty() {
        names.push("an expression".to_string());
    }
    let first = names[0].clone();
    names.push(first);
    let mut diag = Diagnostic::error(format!(
        "combinational loop: {}",
        names.join(" → ")
    ));
    let mut spans = spans.into_iter();
    if let Some((span, label)) = spans.next() {
        diag = diag.at(span, label);
    } else {
        diag = diag.at(ir.span, "in this module");
    }
    for (span, label) in spans {
        diag = diag.with(span, label);
    }
    diag = diag.note("break the loop with a register (`reg` and `<=`)");
    diags.push(diag);
}
