//! A rough static timing estimate: the logic depth of every register-to-register
//! path, in LUT4 levels, without synthesizing.
//!
//! The model is crude but ranks paths usefully: adders and comparisons cost their
//! carry chain (one level plus one per 16 bits), equality and variable shifts are
//! log-depth, muxes and bitwise ops cost half a level (a LUT4 absorbs about two),
//! and wiring (slices, concatenation, constants, buffers, inversion) is free. A
//! 32x32 multiply (a DSP) costs four levels.

use crate::Circuit;
use crate::primitive::{BinaryOp, BinaryPrim, ConstPrim, MuxPrim};

/// A combinational path: its estimated depth and the nodes along it, start first.
#[derive(Debug, Clone)]
pub struct Path {
    pub levels: f64,
    pub nodes: Vec<usize>,
}

fn log2_ceil(x: usize) -> f64 {
    (usize::BITS - x.max(1).saturating_sub(1).leading_zeros()) as f64
}

/// The estimated depth a primitive adds.
fn cost(c: &Circuit, idx: usize, consts: &std::collections::HashSet<usize>) -> f64 {
    let p = &c.primitives[idx];
    let any = p.as_any();
    if let Some(b) = any.downcast_ref::<BinaryPrim>() {
        let w = c.node_widths[b.a].max(c.node_widths[b.b]);
        return match b.op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Lt => 1.0 + w as f64 / 16.0,
            BinaryOp::Eq => 0.5 + log2_ceil(w) / 2.0,
            BinaryOp::Mul => 4.0,
            BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => 0.5,
            BinaryOp::Shl | BinaryOp::Shr => {
                if consts.contains(&b.b) {
                    0.0
                } else {
                    log2_ceil(c.node_widths[b.a]) / 2.0
                }
            }
        };
    }
    if any.downcast_ref::<MuxPrim>().is_some() {
        return 0.5;
    }
    0.0
}

/// The `k` deepest paths ending at a register, memory or output, deepest first; at
/// most one per end point.
pub fn critical_paths(c: &Circuit, k: usize) -> Vec<Path> {
    let order = c.combinational_order().unwrap_or_default();
    let consts: std::collections::HashSet<usize> = c
        .primitives
        .iter()
        .filter_map(|p| p.as_any().downcast_ref::<ConstPrim>().map(|k| k.out))
        .collect();
    // Arrival time of every node and the node it came through.
    let mut at = vec![0.0f64; c.nodes];
    let mut from: Vec<Option<usize>> = vec![None; c.nodes];
    for idx in order {
        let p = &c.primitives[idx];
        let (mut best, mut via) = (0.0, None);
        for i in p.inputs() {
            if at[i] > best || via.is_none() {
                best = at[i];
                via = Some(i);
            }
        }
        let t = best + cost(c, idx, &consts);
        for o in p.outputs() {
            at[o] = t;
            from[o] = via;
        }
    }
    // End points: inputs of sequential primitives, and output ports.
    let mut ends: Vec<usize> = c
        .primitives
        .iter()
        .filter(|p| p.is_sequential())
        .flat_map(|p| {
            let ctl = p.control_inputs();
            p.inputs().into_iter().filter(move |i| !ctl.contains(i))
        })
        .collect();
    ends.extend(
        c.ports
            .iter()
            .filter(|p| p.direction == crate::primitive::PortDirection::Output)
            .map(|p| p.net),
    );
    ends.sort_by(|a, b| at[*b].total_cmp(&at[*a]));
    ends.dedup();
    ends.into_iter()
        .take(k)
        .map(|end| {
            let mut nodes = vec![end];
            let mut n = end;
            while let Some(prev) = from[n] {
                nodes.push(prev);
                n = prev;
            }
            nodes.reverse();
            Path { levels: at[end], nodes }
        })
        .collect()
}

/// A readable form of a path: its depth and the named nodes along it.
pub fn describe(c: &Circuit, p: &Path) -> String {
    let names: Vec<&str> = p
        .nodes
        .iter()
        .filter_map(|n| c.node_names.get(n).map(String::as_str))
        .collect();
    let mut dedup: Vec<&str> = Vec::new();
    for n in names {
        if dedup.last() != Some(&n) {
            dedup.push(n);
        }
    }
    format!("{:5.1} levels: {}", p.levels, dedup.join(" -> "))
}
