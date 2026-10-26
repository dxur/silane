use crate::primitive::{
    BinaryPrim, BufPrim, ConcatPrim, ConstPrim, DffPrim, MemPrim, MuxPrim, NotPrim,
    PortDirection, Primitive, RevPrim, SlicePrim,
};
use std::{
    any::TypeId,
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    sync::atomic::{AtomicU32, Ordering},
};

/// A top-level port on the circuit boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub direction: PortDirection,
    pub net: usize,
}

/// Name of the implicit clock input created by the first register.
pub const CLOCK_PORT: &str = "clk";
/// Name of the implicit asynchronous, active-high reset input created by the first register.
pub const RESET_PORT: &str = "rst";

/// A clock domain bound in a circuit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainBinding {
    pub id: TypeId,
    pub name: &'static str,
    pub clock: usize,
    pub reset: Option<usize>,
}

#[derive(Default)]
pub struct Circuit {
    /// Number of allocated nodes (node IDs are `0..nodes`).
    pub nodes: usize,
    pub primitives: Vec<Box<dyn Primitive>>,
    /// Ports in declaration order.
    pub ports: Vec<Port>,
    /// Clock domains bound with [`crate::Domain`], in binding order.
    pub domains: Vec<DomainBinding>,
    /// Domains bound with [`crate::Domain::ports_on_use`] whose ports are not created
    /// yet: (domain, clock port, reset port).
    pub pending_domains: Vec<(TypeId, String, String)>,
    /// Outputs of the registers allowed to sample another clock domain: the first
    /// stages of synchronizers.
    pub synchronizers: HashSet<usize>,
    /// Bit width of every node, indexed by node ID.
    pub node_widths: Vec<usize>,
    /// Optional human-readable names for nodes.
    pub node_names: HashMap<usize, String>,
}

impl std::fmt::Debug for Circuit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Circuit")
            .field("nodes", &self.nodes)
            .field("primitives_count", &self.primitives.len())
            .field("ports_count", &self.ports.len())
            .finish()
    }
}

impl Circuit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a new node ID of the given bit width in the graph
    pub fn new_node(&mut self, width: usize) -> usize {
        let id = self.nodes;
        self.nodes += 1;
        self.node_widths.push(width);
        id
    }

    /// Bit width of a node.
    pub fn width(&self, node: usize) -> usize {
        self.node_widths[node]
    }

    /// Attaches a human-readable name to a node. The first name given wins.
    pub fn name_node(&mut self, node: usize, name: &str) {
        self.node_names
            .entry(node)
            .or_insert_with(|| name.to_string());
    }

    /// Looks up a port by name.
    pub fn port(&self, name: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.name == name)
    }

    fn add_port(&mut self, name: &str, direction: PortDirection, net: usize) {
        assert!(
            self.port(name).is_none(),
            "duplicate port name '{name}' in circuit"
        );
        self.ports.push(Port {
            name: name.to_string(),
            direction,
            net,
        });
    }

    /// Creates a top-level input port on the circuit boundary.
    ///
    /// # Panics
    /// If a port with the same name already exists.
    pub fn input_port(&mut self, name: &str, width: usize) -> usize {
        let net = self.new_node(width);
        self.add_port(name, PortDirection::Input, net);
        self.name_node(net, name);
        net
    }

    /// Creates a top-level output port on the circuit boundary driven by a bound wire.
    ///
    /// # Panics
    /// If a port with the same name already exists.
    pub fn output_port<T>(&mut self, name: &str, wire: crate::wire::Wire<T, crate::wire::Bound>) {
        self.output_port_node_dirty(name, wire.node_dirty());
    }

    /// Low-level method to register a top-level output port by raw node ID.
    ///
    /// The caller must ensure that `net` is a valid node ID previously allocated by
    /// `Circuit::new_node()` within this circuit context.
    pub fn output_port_node_dirty(&mut self, name: &str, net: usize) {
        self.add_port(name, PortDirection::Output, net);
    }

    /// Binds clock domain `id` (a type, named `name`) to clock and reset nets.
    ///
    /// # Panics
    /// If the domain is already bound.
    pub fn bind_domain(&mut self, id: TypeId, name: &'static str, clock: usize, reset: Option<usize>) {
        self.check_unbound(id, name);
        self.domains.push(DomainBinding { id, name, clock, reset });
    }

    /// Binds clock domain `id` (named `name`) to a clock and a reset input port, created
    /// when the domain's first register is: a design without registers gets no ports.
    ///
    /// # Panics
    /// If the domain is already bound.
    pub fn bind_domain_on_use(&mut self, id: TypeId, name: &'static str, clock: &str, reset: &str) {
        self.check_unbound(id, name);
        self.pending_domains.push((id, clock.to_string(), reset.to_string()));
    }

    fn check_unbound(&self, id: TypeId, name: &str) {
        assert!(
            !self.domains.iter().any(|d| d.id == id) && !self.pending_domains.iter().any(|d| d.0 == id),
            "clock domain `{name}` is bound twice"
        );
    }

    /// Clock and reset nets of domain `id` (named `name`).
    ///
    /// # Panics
    /// If the domain is not bound.
    pub fn domain_nets(&mut self, id: TypeId, name: &'static str) -> (usize, Option<usize>) {
        if let Some(i) = self.pending_domains.iter().position(|d| d.0 == id) {
            let (_, clock, reset) = self.pending_domains.remove(i);
            let clk = self.input_port(&clock, 1);
            let rst = self.input_port(&reset, 1);
            self.domains.push(DomainBinding { id, name, clock: clk, reset: Some(rst) });
        }
        match self.domains.iter().find(|d| d.id == id) {
            Some(d) => (d.clock, d.reset),
            None => panic!(
                "a register is clocked by domain `{name}`, which has no clock: bind it first, \
                 e.g. with `Domain::<{name}>::ports(\"clk\", \"rst\")`"
            ),
        }
    }

    /// Marks the register driving `q` as a synchronizer's first stage, which may
    /// sample signals of other clock domains.
    pub fn mark_synchronizer(&mut self, q: usize) {
        self.synchronizers.insert(q);
    }

    /// Registers a boxed dynamic primitive block in the graph
    pub fn add_primitive<P: Primitive + 'static>(&mut self, prim: P) -> usize {
        let outputs = prim.outputs();
        let out_id = if let Some(&first_out) = outputs.first() {
            first_out
        } else {
            self.new_node(1)
        };
        self.primitives.push(Box::new(prim));
        out_id
    }

    /// Adds a raw dynamic boxed primitive block directly, bypassing node allocation tracking.
    ///
    /// Unlike `add_primitive`, this does not verify that the primitive's output nodes were
    /// allocated by `Circuit::new_node()`. The caller must ensure:
    /// - All node IDs referenced by `prim.inputs()` and `prim.outputs()` are valid IDs
    ///   previously allocated in this circuit context.
    /// - Output node IDs are not already driven by another primitive (no multi-driver conflicts).
    ///
    /// Violations are reported by [`Circuit::validate`].
    pub fn add_boxed_primitive_dirty(&mut self, prim: Box<dyn Primitive>) {
        self.primitives.push(prim);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndrivenWireError {
    pub net: usize,
    pub consumer: String,
}

impl std::fmt::Display for UndrivenWireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Undriven wire (Net_{}) read by consumer '{}'",
            self.net, self.consumer
        )
    }
}

impl std::error::Error for UndrivenWireError {}

/// Returned by [`Circuit::validate`] when a net has more than one driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipleDriversError {
    pub net: usize,
    pub drivers: Vec<String>,
}

impl std::fmt::Display for MultipleDriversError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Net_{} is driven by more than one source: {}",
            self.net,
            self.drivers.join(", ")
        )
    }
}

impl std::error::Error for MultipleDriversError {}

/// Returned by [`Circuit::validate`] when a combinational cycle is detected.
///
/// A combinational cycle means there is a path through purely combinational
/// primitives (anything that is **not** sequential) that loops back on itself.
/// Such a circuit has no defined steady state and cannot be synthesized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinationalCycleError {
    /// One of the node IDs that is part of the cycle.
    pub net: usize,
}

impl std::fmt::Display for CombinationalCycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Combinational cycle detected involving Net_{}", self.net)
    }
}

impl std::error::Error for CombinationalCycleError {}

/// Returned by [`Circuit::validate`] when a register or memory samples a signal of
/// another clock without being a synchronizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockCrossingError {
    /// Output of the sampling register or memory.
    pub net: usize,
    pub register: String,
    /// Clock of the register the signal comes from.
    pub from: String,
    /// Clock of the sampling register.
    pub to: String,
}

impl std::fmt::Display for ClockCrossingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "'{}' (clock '{}') samples a signal of clock '{}' without a synchronizer",
            self.register, self.to, self.from
        )
    }
}

impl std::error::Error for ClockCrossingError {}

/// Returned by [`Circuit::validate`] when a clock is used as data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockAsDataError {
    pub clock: String,
    pub consumer: String,
}

impl std::fmt::Display for ClockAsDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "clock '{}' is used as data by {}", self.clock, self.consumer)
    }
}

impl std::error::Error for ClockAsDataError {}

/// Aggregated validation errors from [`Circuit::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationErrors {
    pub undriven: Vec<UndrivenWireError>,
    pub multiple_drivers: Vec<MultipleDriversError>,
    pub cycles: Vec<CombinationalCycleError>,
    pub crossings: Vec<ClockCrossingError>,
    pub clocks_as_data: Vec<ClockAsDataError>,
}

impl ValidationErrors {
    pub fn is_empty(&self) -> bool {
        self.undriven.is_empty()
            && self.multiple_drivers.is_empty()
            && self.cycles.is_empty()
            && self.crossings.is_empty()
            && self.clocks_as_data.is_empty()
    }
}

impl std::fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for e in &self.undriven {
            writeln!(f, "[undriven] {}", e)?;
        }
        for e in &self.multiple_drivers {
            writeln!(f, "[multiple drivers] {}", e)?;
        }
        for e in &self.cycles {
            writeln!(f, "[cycle] {}", e)?;
        }
        for e in &self.crossings {
            writeln!(f, "[clock crossing] {}", e)?;
        }
        for e in &self.clocks_as_data {
            writeln!(f, "[clock as data] {}", e)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

impl Circuit {
    /// Orders the combinational primitives so that every primitive comes after the
    /// primitives driving its inputs.
    ///
    /// Returns the ordered primitive indices, or the nodes that sit on combinational
    /// cycles. Sequential primitives are not included in the order.
    pub fn combinational_order(&self) -> Result<Vec<usize>, Vec<usize>> {
        let mut driver_of: HashMap<usize, usize> = HashMap::new();
        for (idx, prim) in self.primitives.iter().enumerate() {
            if !prim.is_sequential() {
                for out in prim.outputs() {
                    driver_of.insert(out, idx);
                }
            }
        }

        let mut in_degree = vec![0usize; self.primitives.len()];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); self.primitives.len()];
        for (idx, prim) in self.primitives.iter().enumerate() {
            if prim.is_sequential() {
                continue;
            }
            for input in prim.inputs() {
                if let Some(&src) = driver_of.get(&input) {
                    dependents[src].push(idx);
                    in_degree[idx] += 1;
                }
            }
        }

        let mut queue: VecDeque<usize> = (0..self.primitives.len())
            .filter(|&i| !self.primitives[i].is_sequential() && in_degree[i] == 0)
            .collect();
        let mut order = Vec::new();
        while let Some(idx) = queue.pop_front() {
            order.push(idx);
            for &next in &dependents[idx] {
                in_degree[next] -= 1;
                if in_degree[next] == 0 {
                    queue.push_back(next);
                }
            }
        }

        let comb_count = self
            .primitives
            .iter()
            .filter(|p| !p.is_sequential())
            .count();
        if order.len() == comb_count {
            Ok(order)
        } else {
            let mut cyclic: Vec<usize> = (0..self.primitives.len())
                .filter(|&i| !self.primitives[i].is_sequential() && in_degree[i] > 0)
                .flat_map(|i| self.primitives[i].outputs())
                .collect();
            cyclic.sort_unstable();
            cyclic.dedup();
            Err(cyclic)
        }
    }

    /// Validates the circuit graph:
    ///
    /// 1. **Undriven wire check**: every wire consumed by a primitive or output port must be
    ///    driven by an input port or a primitive output.
    /// 2. **Single driver check**: no net is driven by more than one source.
    /// 3. **Acyclicity check**: the combinational sub-graph (all primitives *except* sequential
    ///    ones, which architecturally break feedback loops) must be a DAG.
    /// 4. **Clock checks**: a register or memory only samples signals of its own clock
    ///    (or of input ports), unless it is a synchronizer's first stage; clocks are not
    ///    used as data. Typed [`crate::Signal`]s guarantee this; it catches netlists built
    ///    with the raw APIs.
    pub fn validate(&self) -> Result<(), ValidationErrors> {
        let mut drivers: HashMap<usize, Vec<String>> = HashMap::new();
        for port in &self.ports {
            if port.direction == PortDirection::Input {
                drivers
                    .entry(port.net)
                    .or_default()
                    .push(format!("input port '{}'", port.name));
            }
        }
        for prim in &self.primitives {
            for out_net in prim.outputs() {
                drivers
                    .entry(out_net)
                    .or_default()
                    .push(format!("primitive {}", prim.name()));
            }
        }

        let mut undriven = Vec::new();
        for prim in &self.primitives {
            for in_net in prim.inputs() {
                if !drivers.contains_key(&in_net) {
                    undriven.push(UndrivenWireError {
                        net: in_net,
                        consumer: format!("primitive {}", prim.name()),
                    });
                }
            }
        }
        for port in &self.ports {
            if port.direction == PortDirection::Output && !drivers.contains_key(&port.net) {
                undriven.push(UndrivenWireError {
                    net: port.net,
                    consumer: format!("output port '{}'", port.name),
                });
            }
        }

        let mut multiple_drivers: Vec<MultipleDriversError> = drivers
            .into_iter()
            .filter(|(_, d)| d.len() > 1)
            .map(|(net, drivers)| MultipleDriversError { net, drivers })
            .collect();
        multiple_drivers.sort_by_key(|e| e.net);

        let (cycles, crossings, clocks_as_data) = match self.combinational_order() {
            Ok(order) => {
                let (crossings, clocks_as_data) = self.check_clocks(&order);
                (Vec::new(), crossings, clocks_as_data)
            }
            Err(nets) => (
                nets.into_iter()
                    .map(|net| CombinationalCycleError { net })
                    .collect(),
                Vec::new(),
                Vec::new(),
            ),
        };

        let errors = ValidationErrors {
            undriven,
            multiple_drivers,
            cycles,
            crossings,
            clocks_as_data,
        };
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// The clock checks of [`Circuit::validate`], given the combinational order.
    fn check_clocks(&self, order: &[usize]) -> (Vec<ClockCrossingError>, Vec<ClockAsDataError>) {
        let label = |net: usize| match self.node_names.get(&net) {
            Some(name) => name.clone(),
            None => format!("Net_{net}"),
        };
        // The clocks whose registers each net depends on within a cycle.
        let mut sources: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut clocks = HashSet::new();
        for prim in &self.primitives {
            if let Some(clk) = prim.clock() {
                clocks.insert(clk);
                for out in prim.outputs() {
                    sources.insert(out, vec![clk]);
                }
            }
        }
        for &idx in order {
            let prim = &self.primitives[idx];
            let mut from: Vec<usize> = prim
                .inputs()
                .iter()
                .filter_map(|n| sources.get(n))
                .flatten()
                .copied()
                .collect();
            from.sort_unstable();
            from.dedup();
            for out in prim.outputs() {
                sources.insert(out, from.clone());
            }
        }

        let mut crossings = Vec::new();
        let mut clocks_as_data = Vec::new();
        for prim in &self.primitives {
            let control = prim.control_inputs();
            let data = prim.inputs().into_iter().filter(|n| !control.contains(n));
            for net in data {
                if clocks.contains(&net) {
                    clocks_as_data.push(ClockAsDataError {
                        clock: label(net),
                        consumer: format!("primitive {}", prim.name()),
                    });
                }
                let (Some(clk), Some(from)) = (prim.clock(), sources.get(&net)) else {
                    continue;
                };
                let out = prim.outputs()[0];
                if self.synchronizers.contains(&out) {
                    continue;
                }
                for &src in from.iter().filter(|&&src| src != clk) {
                    crossings.push(ClockCrossingError {
                        net: out,
                        register: label(out),
                        from: label(src),
                        to: label(clk),
                    });
                }
            }
        }
        crossings.sort_by_key(|e| e.net);
        crossings.dedup();
        (crossings, clocks_as_data)
    }

    /// Picks a unique Verilog identifier for every node, preferring names attached
    /// with [`Circuit::name_node`].
    fn verilog_net_names(&self) -> Vec<String> {
        let mut taken: HashSet<String> = self.ports.iter().map(|p| p.name.clone()).collect();
        let mut names = Vec::with_capacity(self.nodes);
        for node in 0..self.nodes {
            let name = match self.node_names.get(&node) {
                Some(hint) => {
                    let mut base = sanitize_verilog_ident(hint);
                    if VERILOG_KEYWORDS.contains(&base.as_str()) {
                        base.push('_');
                    }
                    let mut candidate = format!("{base}_w");
                    let mut suffix = 1;
                    while taken.contains(&candidate) {
                        candidate = format!("{base}_w{suffix}");
                        suffix += 1;
                    }
                    candidate
                }
                None => format!("net_{node}"),
            };
            taken.insert(name.clone());
            names.push(name);
        }
        names
    }

    /// Generates clean synthesizable IEEE 1364-2005 Verilog for the circuit as a single module.
    pub fn to_verilog(&self, module_name: &str) -> String {
        let width = |n: usize| self.node_widths[n];
        let range = |n: usize| {
            let w = width(n);
            if w > 1 {
                format!("[{}:0] ", w - 1)
            } else {
                String::new()
            }
        };
        let names = self.verilog_net_names();
        let net = |n: usize| names[n].as_str();

        let reg_nodes: HashSet<usize> = self
            .primitives
            .iter()
            .filter_map(|p| {
                let any = p.as_any();
                any.downcast_ref::<DffPrim>()
                    .map(|dff| dff.q)
                    .or_else(|| any.downcast_ref::<MemPrim>().map(|m| m.rdata))
            })
            .collect();

        let mut code = String::new();
        code.push_str("// Generated by Silane HCL - Synthesizable Verilog 2005\n");
        code.push_str(&format!("module {} (\n", module_name));

        for (i, port) in self.ports.iter().enumerate() {
            let dir_str = match port.direction {
                PortDirection::Input => "input  wire",
                PortDirection::Output => "output wire",
            };
            let comma = if i + 1 < self.ports.len() { "," } else { "" };
            code.push_str(&format!(
                "    {} {}{}{}\n",
                dir_str,
                range(port.net),
                port.name,
                comma
            ));
        }
        code.push_str(");\n\n");

        code.push_str("    // Internal Net Declarations\n");
        for n in 0..self.nodes {
            let kind = if reg_nodes.contains(&n) { "reg " } else { "wire" };
            code.push_str(&format!("    {} {}{};\n", kind, range(n), net(n)));
        }
        code.push('\n');

        // Memory arrays are named after their read data.
        let mem_name = |m: &MemPrim| format!("{}_mem", net(m.rdata));
        let mems: Vec<&MemPrim> = self
            .primitives
            .iter()
            .filter_map(|p| p.as_any().downcast_ref::<MemPrim>())
            .collect();
        if !mems.is_empty() {
            code.push_str("    // Memories\n");
            for m in &mems {
                let range = if m.width > 1 { format!("[{}:0] ", m.width - 1) } else { String::new() };
                code.push_str(&format!("    reg {range}{} [0:{}];\n", mem_name(m), m.depth - 1));
            }
            for m in mems.iter().filter(|m| !m.init.is_empty()) {
                code.push_str("    initial begin\n");
                for (i, word) in m.init.iter().enumerate() {
                    code.push_str(&format!("        {}[{i}] = {}'d{word};\n", mem_name(m), m.width));
                }
                for i in m.init.len()..m.depth {
                    code.push_str(&format!("        {}[{i}] = {}'d0;\n", mem_name(m), m.width));
                }
                code.push_str("    end\n");
            }
            code.push('\n');
        }

        code.push_str("    // Top-Level Port Connections\n");
        for port in &self.ports {
            match port.direction {
                PortDirection::Input => {
                    code.push_str(&format!("    assign {} = {};\n", net(port.net), port.name))
                }
                PortDirection::Output => {
                    code.push_str(&format!("    assign {} = {};\n", port.name, net(port.net)))
                }
            }
        }
        code.push('\n');

        code.push_str("    // Combinational & Sequential Logic\n");
        for p in &self.primitives {
            let any = p.as_any();
            if let Some(c) = any.downcast_ref::<ConstPrim>() {
                code.push_str(&format!(
                    "    assign {} = {}'d{};\n",
                    net(c.out),
                    width(c.out),
                    c.value
                ));
            } else if let Some(b) = any.downcast_ref::<BinaryPrim>() {
                let expr = format!("{} {} {}", net(b.a), b.op.verilog_op(), net(b.b));
                let expr = if b.op.is_comparison() {
                    format!("({expr})")
                } else {
                    expr
                };
                code.push_str(&format!("    assign {} = {};\n", net(b.out), expr));
            } else if let Some(not) = any.downcast_ref::<NotPrim>() {
                code.push_str(&format!("    assign {} = ~{};\n", net(not.out), net(not.a)));
            } else if let Some(m) = any.downcast_ref::<MuxPrim>() {
                code.push_str(&format!(
                    "    assign {} = {} ? {} : {};\n",
                    net(m.out),
                    net(m.sel),
                    net(m.in1),
                    net(m.in0)
                ));
            } else if let Some(s) = any.downcast_ref::<SlicePrim>() {
                let sel = if width(s.input) == 1 {
                    String::new()
                } else if s.width == 1 {
                    format!("[{}]", s.offset)
                } else {
                    format!("[{}:{}]", s.offset + s.width - 1, s.offset)
                };
                code.push_str(&format!(
                    "    assign {} = {}{};\n",
                    net(s.out),
                    net(s.input),
                    sel
                ));
            } else if let Some(cat) = any.downcast_ref::<ConcatPrim>() {
                code.push_str(&format!(
                    "    assign {} = {{{}, {}}};\n",
                    net(cat.out),
                    net(cat.a),
                    net(cat.b)
                ));
            } else if let Some(r) = any.downcast_ref::<RevPrim>() {
                if r.width == 1 {
                    code.push_str(&format!("    assign {} = {};\n", net(r.out), net(r.a)));
                } else {
                    let bits: Vec<String> = (0..r.width)
                        .map(|i| format!("{}[{}]", net(r.a), i))
                        .collect();
                    code.push_str(&format!(
                        "    assign {} = {{{}}};\n",
                        net(r.out),
                        bits.join(", ")
                    ));
                }
            } else if let Some(buf) = any.downcast_ref::<BufPrim>() {
                code.push_str(&format!("    assign {} = {};\n", net(buf.out), net(buf.a)));
            } else if let Some(m) = any.downcast_ref::<MemPrim>() {
                code.push_str(&format!(
                    "    always @(posedge {clk}) begin\n        if ({we})\n            {mem}[{waddr}] <= {wdata};\n        if ({re})\n            {rdata} <= {mem}[{raddr}];\n    end\n",
                    clk = net(m.clk),
                    we = net(m.we),
                    mem = mem_name(m),
                    waddr = net(m.waddr),
                    wdata = net(m.wdata),
                    re = net(m.re),
                    rdata = net(m.rdata),
                    raddr = net(m.raddr),
                ));
            } else if let Some(dff) = any.downcast_ref::<DffPrim>() {
                let q = net(dff.q);
                if let Some(rst_net) = dff.rst {
                    code.push_str(&format!(
                        "    always @(posedge {clk} or posedge {rst}) begin\n        if ({rst})\n            {q} <= {w}'d{reset};\n        else\n            {q} <= {d};\n    end\n",
                        clk = net(dff.clk),
                        rst = net(rst_net),
                        w = width(dff.q),
                        reset = dff.reset_val,
                        d = net(dff.d),
                    ));
                } else {
                    code.push_str(&format!(
                        "    always @(posedge {}) begin\n        {} <= {};\n    end\n",
                        net(dff.clk),
                        q,
                        net(dff.d)
                    ));
                }
            }
        }

        code.push_str("\nendmodule\n");
        code
    }

    /// Helper to write generated Verilog to disk.
    pub fn emit_verilog_file(&self, module_name: &str, path: &str) -> std::io::Result<()> {
        let code = self.to_verilog(module_name);
        std::fs::write(path, code)
    }
}

fn sanitize_verilog_ident(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

const VERILOG_KEYWORDS: &[&str] = &[
    "always", "and", "assign", "begin", "buf", "case", "casex", "casez", "default", "else",
    "end", "endcase", "endfunction", "endmodule", "for", "function", "if", "initial", "inout",
    "input", "integer", "module", "nand", "negedge", "nor", "not", "or", "output", "parameter",
    "posedge", "reg", "signed", "wire", "xnor", "xor",
];

// Thread-Local Storage for active elaboration
thread_local! {
    static CURRENT_CIRCUIT: RefCell<Option<Circuit>> = const { RefCell::new(None) };
    /// Identity of the active elaboration (0: none), kept apart from the circuit so
    /// wires can read it while the circuit is borrowed.
    static CURRENT_ID: Cell<u32> = const { Cell::new(0) };
}

/// Identities of elaborations, so a wire from one can be told apart from another's.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

impl Circuit {
    /// Runs `f` inside a new elaboration context and returns its result with the
    /// circuit it built.
    ///
    /// Wires belong to the elaboration that created them: using one in another
    /// elaboration panics.
    ///
    /// # Panics
    /// If called while another elaboration is already active on this thread.
    pub fn elaborate<F, R>(f: F) -> (R, Circuit)
    where
        F: FnOnce() -> R,
    {
        CURRENT_CIRCUIT.with(|c| {
            let mut slot = c.borrow_mut();
            assert!(
                slot.is_none(),
                "Circuit::elaborate called inside an active elaboration"
            );
            *slot = Some(Circuit::default());
        });
        CURRENT_ID.set(NEXT_ID.fetch_add(1, Ordering::Relaxed));

        // Clear the context even if the design code panics, so later elaborations work.
        struct ResetOnUnwind;
        impl Drop for ResetOnUnwind {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    CURRENT_CIRCUIT.with(|c| *c.borrow_mut() = None);
                    CURRENT_ID.set(0);
                }
            }
        }
        let guard = ResetOnUnwind;
        let result = f();
        std::mem::forget(guard);

        let circuit = CURRENT_CIRCUIT.with(|c| {
            c.borrow_mut()
                .take()
                .expect("Circuit context was modified unexpectedly")
        });
        CURRENT_ID.set(0);

        (result, circuit)
    }

    /// Identity of the active elaboration, or 0 outside of one.
    pub(crate) fn current_id() -> u32 {
        CURRENT_ID.get()
    }

    /// Accesses the current thread's active elaboration context with full mutable access.
    ///
    /// Granting raw `&mut Circuit` access allows callers to arbitrarily mutate the circuit graph
    /// outside the safe allocation and registration protocols. Misuse can:
    /// - Clear or reorder `primitives`, breaking the graph topology.
    /// - Corrupt `nodes` counter, causing node ID collisions.
    /// - Modify `ports` inconsistently.
    ///
    /// Prefer using the high-level `Signal` and `Wire` APIs for all design work.
    /// Only use `with_current_dirty` when building low-level primitive registrations where
    /// you have full ownership of the node IDs involved.
    pub fn with_current_dirty<F, R>(f: F) -> R
    where
        F: FnOnce(&mut Circuit) -> R,
    {
        CURRENT_CIRCUIT.with(|c| {
            let mut guard = c.borrow_mut();
            let circuit = guard.as_mut().expect(
                "No active Circuit elaboration context! Did you forget Circuit::elaborate(|| ...)?",
            );
            f(circuit)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::{BinaryOp, BinaryPrim, ConstPrim, DffPrim};

    fn add(a: usize, b: usize, out: usize) -> BinaryPrim {
        BinaryPrim {
            op: BinaryOp::Add,
            a,
            b,
            out,
        }
    }

    #[test]
    fn test_valid_circuit_connectivity() {
        let (_, circuit) = Circuit::elaborate(|| {
            let in_a = Circuit::with_current_dirty(|c| c.input_port("in_a", 8));
            let in_b = Circuit::with_current_dirty(|c| c.input_port("in_b", 8));
            let out = Circuit::with_current_dirty(|c| {
                let out = c.new_node(8);
                c.add_primitive(add(in_a, in_b, out));
                out
            });
            Circuit::with_current_dirty(|c| c.output_port_node_dirty("out_y", out));
        });

        assert!(circuit.validate().is_ok());
    }

    #[test]
    fn test_undriven_wire_detected() {
        let (_, circuit) = Circuit::elaborate(|| {
            let undriven_net = Circuit::with_current_dirty(|c| c.new_node(8));
            let const_net = Circuit::with_current_dirty(|c| {
                let out = c.new_node(8);
                c.add_primitive(ConstPrim {
                    value: 42,
                    width: 8,
                    out,
                });
                out
            });
            Circuit::with_current_dirty(|c| {
                let out = c.new_node(8);
                c.add_primitive(add(undriven_net, const_net, out));
            });
        });

        let errors = circuit.validate().unwrap_err();
        assert_eq!(errors.undriven.len(), 1);
        assert_eq!(errors.undriven[0].net, 0);
        assert_eq!(errors.undriven[0].consumer, "primitive $add");
        assert!(errors.cycles.is_empty());
    }

    #[test]
    fn test_multiple_drivers_detected() {
        let (_, circuit) = Circuit::elaborate(|| {
            Circuit::with_current_dirty(|c| {
                let a = c.input_port("a", 1);
                let out = c.new_node(1);
                c.add_primitive(BufPrim { a, out });
                c.add_primitive(BufPrim { a, out });
                c.output_port_node_dirty("y", out);
            });
        });

        let errors = circuit.validate().unwrap_err();
        assert_eq!(errors.multiple_drivers.len(), 1);
        assert_eq!(errors.multiple_drivers[0].drivers.len(), 2);
    }

    #[test]
    fn test_combinational_cycle_detected() {
        // a -> Buf -> b -> Buf -> a: a pure combinational cycle.
        let (node_ids, circuit) = Circuit::elaborate(|| {
            let (a, b) = Circuit::with_current_dirty(|c| (c.new_node(1), c.new_node(1)));
            Circuit::with_current_dirty(|c| c.add_primitive(BufPrim { a, out: b }));
            Circuit::with_current_dirty(|c| c.add_primitive(BufPrim { a: b, out: a }));
            (a, b)
        });
        let (a, b) = node_ids;

        let errors = circuit.validate().unwrap_err();
        assert!(!errors.cycles.is_empty(), "Should have detected cycle nodes");
        assert!(errors.cycles.iter().any(|e| e.net == a || e.net == b));
    }

    #[test]
    #[should_panic(expected = "duplicate port name")]
    fn test_duplicate_port_rejected() {
        Circuit::elaborate(|| {
            Circuit::with_current_dirty(|c| {
                c.input_port("a", 1);
                c.input_port("a", 1);
            });
        });
    }

    #[test]
    #[should_panic(expected = "has no clock")]
    fn test_register_in_unbound_domain_rejected() {
        use crate::signal::Signal;
        use typenum::U8;

        Circuit::elaborate(|| {
            let r = Signal::<U8>::default();
            let q = r.read();
            r.next(q + 1u64);
        });
    }

    #[test]
    #[should_panic(expected = "bound twice")]
    fn test_domain_bound_twice_rejected() {
        use crate::signal::{Domain, Sys};

        Circuit::elaborate(|| {
            Domain::<Sys>::ports("clk", "rst");
            Domain::<Sys>::ports("clk2", "rst2");
        });
    }

    /// Typed signals cannot cross domains; a netlist built with the raw APIs can, and
    /// `validate()` reports it, but not a synchronizer's crossing.
    #[test]
    fn test_validate_reports_clock_crossings() {
        use crate::cdc::Sync2;
        use crate::signal::{Domain, Signal};
        use typenum::U1;
        struct Fast;
        struct Slow;

        let ((), circuit) = Circuit::elaborate(|| {
            Domain::<Fast>::ports("fast", "fast_rst");
            Domain::<Slow>::ports("slow", "slow_rst");
            let s = Signal::<U1, Slow>::default();
            let sq = s.read().named("slow_q");
            s.next(!sq);
            // Allowed: through a synchronizer.
            let synced = Sync2::<Slow, Fast>::new(sq).q;
            synced.output_port("synced");
            // Not allowed: a fast register reading the slow one directly.
            let (clk, rst) = Domain::<Fast>::nets_dirty();
            let raw = Circuit::with_current_dirty(|c| {
                let q = c.new_node(1);
                c.name_node(q, "raw_q");
                c.add_primitive(DffPrim { clk, rst, reset_val: 0, d: sq.node_dirty(), q });
                c.output_port_node_dirty("raw", q);
                q
            });
            let _ = raw;
        });

        let errors = circuit.validate().unwrap_err();
        assert_eq!(errors.crossings.len(), 1, "{errors}");
        assert_eq!(errors.crossings[0].register, "raw_q");
        assert_eq!((errors.crossings[0].from.as_str(), errors.crossings[0].to.as_str()), ("slow", "fast"));
        assert!(errors.undriven.is_empty() && errors.clocks_as_data.is_empty(), "{errors}");
    }

    #[test]
    #[should_panic(expected = "wire of another elaboration")]
    fn test_wire_from_another_circuit_rejected() {
        use crate::signal::Signal;
        use typenum::U8;

        let (leaked, _) = Circuit::elaborate(|| Signal::<U8>::input_port_default("a"));
        Circuit::elaborate(|| {
            let b = Signal::<U8>::input_port_default("b");
            let _ = b + leaked;
        });
    }

    #[test]
    fn test_verilog_emission() {
        use crate::signal::Signal;
        use typenum::{U0, U4, U8};

        let (_out_wire, circuit) = Circuit::elaborate(|| {
            crate::Domain::<crate::signal::Sys>::ports("clk", "rst");
            let in_a = Signal::<U8>::input_port_default("data_in");
            let reg_u = Signal::<U8>::default();
            let reg_b = reg_u.read();

            let sum = (in_a + reg_b).named("sum");
            reg_u.next(sum);

            let out_wire = sum.slice::<U0, U4, U4>();
            Signal::output_port("data_out", out_wire);
            Signal::output_port("acc", reg_b);
            out_wire
        });

        circuit.validate().unwrap();
        let verilog = circuit.to_verilog("top_test");
        assert!(verilog.contains("module top_test"));
        assert!(verilog.contains("input  wire [7:0] data_in"));
        assert!(verilog.contains("output wire [3:0] data_out"));
        assert!(verilog.contains("wire [7:0] sum_w;"));
        assert!(verilog.contains("always @(posedge clk_w or posedge rst_w)"));
        // A register driving an output port is declared and read like any other net.
        assert!(verilog.contains("assign acc = net_"));
    }
}
