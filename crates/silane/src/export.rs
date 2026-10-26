use crate::circuit::Circuit;
use serde_json::json;

impl Circuit {
    /// Yosys bit indices for every node: node `n` of width `w` owns `w` consecutive bits.
    /// Bits 0 and 1 are reserved by Yosys for constant 0 and 1.
    fn netlist_bits(&self) -> Vec<Vec<usize>> {
        let mut next = 2;
        self.node_widths
            .iter()
            .map(|&w| {
                let bits = (next..next + w).collect();
                next += w;
                bits
            })
            .collect()
    }

    /// Renders the circuit graph into official Yosys / netlist-svg JSON format.
    /// Includes top-level module ports (clock, reset, I/O) and physical net connections!
    pub fn to_netlist_json(&self) -> serde_json::Value {
        let bits = self.netlist_bits();
        let mut ports = serde_json::Map::new();

        for port in &self.ports {
            ports.insert(
                port.name.clone(),
                json!({
                    "direction": port.direction.as_str(),
                    "bits": bits[port.net]
                }),
            );
        }

        let mut cells = serde_json::Map::new();

        for (idx, prim) in self.primitives.iter().enumerate() {
            let cell_name = format!("cell_{}_{}", prim.name().trim_start_matches('$'), idx);

            let mut port_directions = serde_json::Map::new();
            let mut connections = serde_json::Map::new();

            for binding in prim.port_bindings() {
                let nets: Vec<usize> = binding
                    .nets
                    .iter()
                    .flat_map(|&n| bits[n].iter().copied())
                    .collect();
                port_directions.insert(binding.name.to_string(), json!(binding.direction.as_str()));
                connections.insert(binding.name.to_string(), json!(nets));
            }

            cells.insert(
                cell_name,
                json!({
                    "type": prim.name(),
                    "port_directions": port_directions,
                    "connections": connections
                }),
            );
        }

        let mut netnames = serde_json::Map::new();
        for (&node, name) in &self.node_names {
            netnames.insert(
                format!("{name}#{node}"),
                json!({ "hide_name": 0, "bits": bits[node] }),
            );
        }

        json!({
            "modules": {
                "top": {
                    "ports": ports,
                    "cells": cells,
                    "netnames": netnames
                }
            }
        })
    }

    /// Renders the netlist JSON as a formatted, pretty-printed string.
    pub fn to_netlist_json_string(&self) -> String {
        serde_json::to_string_pretty(&self.to_netlist_json())
            .expect("Failed to serialize circuit to netlist JSON")
    }

    /// Renders the circuit graph into Graphviz DOT format.
    pub fn to_dot(&self) -> String {
        let mut dot = String::new();
        dot.push_str("digraph SilaneCircuit {\n");
        dot.push_str("  rankdir=LR;\n");
        dot.push_str("  node [fontname=\"Helvetica\", fontsize=10];\n");
        dot.push_str("  edge [fontname=\"Helvetica\", fontsize=9];\n\n");

        for i in 0..self.nodes {
            let label = match self.node_names.get(&i) {
                Some(name) => format!("{name} [{}]", self.node_widths[i]),
                None => format!("Net {i} [{}]", self.node_widths[i]),
            };
            dot.push_str(&format!(
                "  net_{} [label=\"{}\", shape=ellipse, style=filled, fillcolor=\"#E3F2FD\", color=\"#1565C0\"];\n",
                i, label
            ));
        }

        for (idx, prim) in self.primitives.iter().enumerate() {
            let prim_node_id = format!("prim_{}", idx);
            let label = prim.label();

            dot.push_str(&format!(
                "  {} [label=\"{}\", shape=box, style=\"rounded,filled\", fillcolor=\"#FFF3E0\", color=\"#E65100\"];\n",
                prim_node_id, label
            ));

            for in_net in prim.inputs() {
                dot.push_str(&format!("  net_{} -> {};\n", in_net, prim_node_id));
            }

            for out_net in prim.outputs() {
                dot.push_str(&format!("  {} -> net_{};\n", prim_node_id, out_net));
            }
        }

        dot.push_str("}\n");
        dot
    }
}
