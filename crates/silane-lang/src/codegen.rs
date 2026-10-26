//! Rust code generation: every elaborated module becomes a struct holding its outputs,
//! with `new(inputs...)` to instantiate it inside a silane elaboration and
//! `circuit()` to elaborate it as a top-level [`silane::Circuit`].
//!
//! A port whose type contains no bundle is one Rust parameter or field (arrays become
//! Rust arrays). A bundle port is split by direction: each scalar element flowing in
//! is a parameter and each element flowing out is a field, named `port_field`.
//!
//! Modules are generic over their clock domains `D0, D1, ...` (see [`crate::domains`]);
//! every signal's type names its domain, so rustc checks every binding the caller
//! makes. Extern modules are used through generated adapters with their declared
//! signatures, so a declaration that does not match its Rust type fails only there.

use crate::diag::SourceMap;
use crate::domains::{self, Domains};
use crate::ir::{BinKind, Design, LeafDir, ModuleIr, Node, Step, TargetKind, Ty, schedule};
use std::collections::HashSet;
use std::fmt::Write;

const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "union",
];

/// Allocates unique, valid Rust identifiers within one scope.
#[derive(Default)]
struct Names {
    used: HashSet<String>,
}

impl Names {
    fn alloc(&mut self, hint: &str) -> String {
        let base = sanitize(hint);
        let mut name = base.clone();
        let mut n = 1;
        while self.used.contains(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        self.used.insert(name.clone());
        name
    }
}

fn sanitize(hint: &str) -> String {
    let mut s = String::new();
    for c in hint.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            s.push(c);
        } else if !s.ends_with('_') {
            s.push('_');
        }
    }
    let mut s = s.trim_end_matches('_').to_string();
    if s.is_empty() {
        s.push('_');
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    if RUST_KEYWORDS.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

fn tn(width: u32) -> String {
    format!("::silane::typenum::U{width}")
}

fn wire_ty(width: u32, domain: u32) -> String {
    format!("SiWire<{}, D{domain}>", tn(width))
}

/// `<D0, D1>`, or nothing without domains.
fn domain_args(count: u32) -> String {
    if count == 0 {
        return String::new();
    }
    let args: Vec<String> = (0..count).map(|d| format!("D{d}")).collect();
    format!("<{}>", args.join(", "))
}

/// `<D0: 'static, ...>` for an impl or function, or nothing.
fn domain_params(count: u32) -> String {
    if count == 0 {
        return String::new();
    }
    let params: Vec<String> = (0..count).map(|d| format!("D{d}: 'static")).collect();
    format!("<{}>", params.join(", "))
}

/// A child's type with its domains mapped to this module's.
fn child_ty(name: &str, map: &[u32]) -> String {
    if map.is_empty() {
        return name.to_string();
    }
    let args: Vec<String> = map.iter().map(|d| format!("D{d}")).collect();
    format!("{name}<{}>", args.join(", "))
}

fn str_lit(s: &str) -> String {
    format!("{s:?}")
}

/// One Rust-level parameter or field of a module interface.
pub(crate) struct Item {
    pub(crate) name: String,
    /// The Rust type: the port type for whole ports, or a scalar for bundle leaves.
    pub(crate) ty: Ty,
    /// Flattened input (for parameters) or output (for fields) indices, in order.
    pub(crate) flat: Vec<u32>,
    /// Index suffix of each flat element within the item (`[1][0]`, or empty).
    pub(crate) paths: Vec<String>,
    /// Circuit port name of each flat element.
    pub(crate) port_names: Vec<String>,
}

pub(crate) struct Interface {
    pub(crate) inputs: Vec<Item>,
    pub(crate) outputs: Vec<Item>,
}

/// Index suffixes of every element of a bundle-free type.
fn element_paths(ty: &Ty) -> Vec<String> {
    match ty {
        Ty::Array(elem, n) => (0..*n)
            .flat_map(|i| element_paths(elem).into_iter().map(move |p| format!("[{i}]{p}")))
            .collect(),
        _ => vec![String::new()],
    }
}

pub(crate) fn interface(m: &ModuleIr, design: &Design) -> Interface {
    let mut in_names = Names::default();
    let mut out_names = Names::default();
    out_names.used.insert("_domains".to_string());
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for port in &m.ports {
        if !port.ty.has_bundle() {
            let paths = element_paths(&port.ty);
            let flat: Vec<u32> = port
                .leaves
                .iter()
                .map(|l| match l.dir {
                    LeafDir::In(k) => k,
                    LeafDir::Out(o, _) => o,
                })
                .collect();
            let port_names = port.leaves.iter().map(|l| l.flat_name.clone()).collect();
            let (names, list) = if port.is_arg {
                (&mut in_names, &mut inputs)
            } else {
                (&mut out_names, &mut outputs)
            };
            list.push(Item {
                name: names.alloc(&port.name),
                ty: port.ty.clone(),
                flat,
                paths,
                port_names,
            });
            continue;
        }
        let leaves = port.ty.leaves(&design.types);
        for (leaf, ty_leaf) in port.leaves.iter().zip(leaves) {
            let (names, list, k) = match leaf.dir {
                LeafDir::In(k) => (&mut in_names, &mut inputs, k),
                LeafDir::Out(o, _) => (&mut out_names, &mut outputs, o),
            };
            list.push(Item {
                name: names.alloc(&leaf.flat_name),
                ty: ty_leaf.ty,
                flat: vec![k],
                paths: vec![String::new()],
                port_names: vec![leaf.flat_name.clone()],
            });
        }
    }
    Interface { inputs, outputs }
}

fn rust_ty(ty: &Ty, design: &Design, domain: u32) -> String {
    match ty {
        Ty::Array(elem, n) => format!("[{}; {n}]", rust_ty(elem, design, domain)),
        scalar => wire_ty(scalar.width(&design.types), domain),
    }
}

/// Builds a nested array expression from flattened element expressions.
fn nest(ty: &Ty, flat: &mut impl Iterator<Item = String>) -> String {
    match ty {
        Ty::Array(elem, n) => {
            let items: Vec<String> = (0..*n).map(|_| nest(elem, flat)).collect();
            format!("[{}]", items.join(", "))
        }
        _ => flat.next().expect("flattened element"),
    }
}

pub fn generate(design: &Design, sources: &SourceMap, origin: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "// @generated by silc from `{origin}`. Do not edit.");
    out.push('\n');
    out.push_str("/// A bound silane signal of width `W` in clock domain `D`.\n");
    out.push_str("#[allow(dead_code)]\n");
    out.push_str("pub type SiWire<W, D = ::silane::Sys> = ::silane::Wire<::silane::Signal<W, D>>;\n");

    for def in &design.types.enums {
        out.push('\n');
        let _ = writeln!(out, "/// Enum `{}` ({}-bit encoding).", def.name, def.width);
        out.push_str("#[allow(dead_code, non_camel_case_types)]\n");
        out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n#[repr(u64)]\n");
        let name = sanitize(&def.name);
        let _ = writeln!(out, "pub enum {name} {{");
        let variants: Vec<String> = def.variants.iter().map(|(v, _)| sanitize(v)).collect();
        for (variant, (_, value)) in variants.iter().zip(&def.variants) {
            let _ = writeln!(out, "    {variant} = {value},");
        }
        out.push_str("}\n\n");
        let _ = writeln!(out, "#[allow(dead_code)]\nimpl {name} {{");
        out.push_str("    /// Width of the encoding in bits.\n");
        let _ = writeln!(out, "    pub const WIDTH: usize = {};", def.width);
        out.push_str("    /// Encoding of this variant.\n");
        out.push_str("    pub const fn bits(self) -> u64 {\n        self as u64\n    }\n");
        out.push_str("    /// Variant with the given encoding, if any.\n");
        out.push_str("    pub const fn from_bits(bits: u64) -> Option<Self> {\n        match bits {\n");
        for (variant, (_, value)) in variants.iter().zip(&def.variants) {
            let _ = writeln!(out, "            {value} => Some(Self::{variant}),");
        }
        out.push_str("            _ => None,\n        }\n    }\n}\n");
    }

    let doms = domains::infer(design);
    // Modules implemented in Rust get adapters with their declared signatures.
    for (idx, m) in design.modules.iter().enumerate() {
        out.push('\n');
        match &m.rust_path {
            Some(path) => extern_adapter(&mut out, design, idx, path, &doms[idx], sources),
            None => module(&mut out, design, m, &doms[idx], sources),
        }
    }

    for (alias, idx) in &design.aliases {
        let target = sanitize(&design.modules[*idx].rust_name);
        let alias = sanitize(alias);
        if alias != target {
            out.push('\n');
            let _ = writeln!(out, "/// Alias of [`{target}`].");
            out.push_str("#[allow(dead_code)]\n");
            let count = doms[*idx].count;
            let _ = writeln!(out, "pub type {alias}{} = {target}{};", domain_params_default(count), domain_args(count));
        }
    }
    out
}

/// Bindings of a function body, emitted grouped by level: one tuple `let` per level.
/// Every `let` opens a scope nested in the previous one, and LLVM's debug info writer
/// recurses once per nesting level; thousands of single `let`s overflow its stack.
/// Bindings of one level must not refer to each other.
#[derive(Default)]
struct Lets {
    /// (level, name, type or `_`, expression)
    items: Vec<(u32, String, String, String)>,
}

impl Lets {
    fn push(&mut self, level: u32, name: String, ty: String, expr: String) {
        self.items.push((level, name, ty, expr));
    }

    fn emit(mut self, out: &mut String) {
        self.items.sort_by_key(|(level, ..)| *level);
        for group in self.items.chunk_by(|a, b| a.0 == b.0) {
            if let [(_, name, ty, expr)] = group {
                let _ = writeln!(out, "        let {name}: {ty} = {expr};");
                continue;
            }
            let names: Vec<&str> = group.iter().map(|(_, n, ..)| n.as_str()).collect();
            let tys: Vec<&str> = group.iter().map(|(_, _, t, _)| t.as_str()).collect();
            let _ = writeln!(out, "        let ({}): ({}) = (", names.join(", "), tys.join(", "));
            for (_, _, _, expr) in group {
                let _ = writeln!(out, "            {expr},");
            }
            out.push_str("        );\n");
        }
    }
}

fn location(sources: &SourceMap, m: &ModuleIr) -> String {
    let file = sources.file(m.span.file);
    let (line, _) = file.line_col(m.span.lo as usize);
    let name = file
        .path
        .file_name()
        .map_or_else(|| file.path.display().to_string(), |n| n.to_string_lossy().into_owned());
    format!("{name}:{}", line + 1)
}

/// `<D0 = ::silane::Sys, ...>` for a struct or type alias, or nothing.
fn domain_params_default(count: u32) -> String {
    if count == 0 {
        return String::new();
    }
    let params: Vec<String> = (0..count).map(|d| format!("D{d} = ::silane::Sys")).collect();
    format!("<{}>", params.join(", "))
}

/// Domain of an interface item: that of its first element (all share one).
fn item_domain(item: &Item, classes: &[u32]) -> u32 {
    item.flat.first().map_or(0, |&f| classes[f as usize])
}

/// The adapter of extern module `idx`: a struct with exactly its declared signature,
/// whose `new()` calls the Rust type at `path`. Modules use extern modules through
/// their adapters, so if the Rust type does not match the declaration (port widths, or
/// domains it ties that the declaration keeps apart), rustc reports it here only.
fn extern_adapter(out: &mut String, design: &Design, idx: usize, path: &str, dom: &Domains, sources: &SourceMap) {
    let m = &design.modules[idx];
    let name = sanitize(&m.rust_name);
    let iface = interface(m, design);
    let impl_params = domain_params(dom.count);
    let self_ty = format!("{name}{}", domain_args(dom.count));
    let _ = writeln!(
        out,
        "/// Extern module `{}` (from `{}`), implemented by `{path}`: its outputs.",
        m.display_name,
        location(sources, m)
    );
    out.push_str("#[allow(dead_code, non_camel_case_types)]\n");
    let _ = writeln!(out, "pub struct {name}{} {{", domain_params_default(dom.count).replace(" = ", ": 'static = "));
    for item in &iface.outputs {
        let ty = rust_ty(&item.ty, design, item_domain(item, &dom.outputs));
        let _ = writeln!(out, "    pub {}: {ty},", item.name);
    }
    let _ = writeln!(out, "    _domains: ::std::marker::PhantomData<fn() -> ({})>,", (0..dom.count).map(|d| format!("D{d},")).collect::<String>());
    out.push_str("}\n\n");
    let _ = writeln!(out, "impl{impl_params} Clone for {self_ty} {{\n    fn clone(&self) -> Self {{\n        *self\n    }}\n}}\n");
    let _ = writeln!(out, "impl{impl_params} Copy for {self_ty} {{}}\n");

    let mut names = Names::default();
    let args: Vec<String> = iface.inputs.iter().map(|item| names.alloc(&item.name)).collect();
    let params: Vec<String> = iface
        .inputs
        .iter()
        .zip(&args)
        .map(|(item, name)| format!("{name}: {}", rust_ty(&item.ty, design, item_domain(item, &dom.inputs))))
        .collect();
    let mut call: Vec<String> = m.params.iter().map(|(_, v)| format!("{v}")).collect();
    call.extend(args);
    let _ = writeln!(out, "#[allow(dead_code, non_snake_case, unused_variables, clippy::all)]");
    let _ = writeln!(out, "impl{impl_params} {self_ty} {{");
    let _ = writeln!(out, "    /// Instantiates `{}` in the current silane elaboration.", m.display_name);
    let _ = writeln!(out, "    pub fn new({}) -> Self {{", params.join(", "));
    let _ = writeln!(out, "        let instance = {path}::new({});", call.join(", "));
    out.push_str("        Self {\n");
    for item in &iface.outputs {
        let _ = writeln!(out, "            {}: instance.{},", item.name, item.name);
    }
    out.push_str("            _domains: ::std::marker::PhantomData,\n");
    out.push_str("        }\n    }\n}\n");
}

fn module(out: &mut String, design: &Design, m: &ModuleIr, dom: &Domains, sources: &SourceMap) {
    let name = sanitize(&m.rust_name);
    let iface = interface(m, design);
    let args_ty = domain_args(dom.count);
    let impl_params = domain_params(dom.count);
    let self_ty = format!("{name}{args_ty}");

    // ── struct ──
    let _ = writeln!(
        out,
        "/// Module `{}` (from `{}`): its outputs.",
        m.display_name,
        location(sources, m)
    );
    if dom.count > 1 {
        let _ = writeln!(out, "///\n/// Its signals are in {} clock domains, `D0` to `D{}`.", dom.count, dom.count - 1);
    }
    out.push_str("#[allow(dead_code, non_camel_case_types)]\n");
    let _ = writeln!(out, "pub struct {name}{} {{", domain_params_default(dom.count).replace(" = ", ": 'static = "));
    for item in &iface.outputs {
        let ty = rust_ty(&item.ty, design, item_domain(item, &dom.outputs));
        let _ = writeln!(out, "    pub {}: {ty},", item.name);
    }
    let _ = writeln!(out, "    _domains: ::std::marker::PhantomData<fn() -> ({})>,", (0..dom.count).map(|d| format!("D{d},")).collect::<String>());
    out.push_str("}\n\n");
    // Not derived: those would require the domain types to be `Clone`.
    let _ = writeln!(out, "impl{impl_params} Clone for {self_ty} {{\n    fn clone(&self) -> Self {{\n        *self\n    }}\n}}\n");
    let _ = writeln!(out, "impl{impl_params} Copy for {self_ty} {{}}\n");

    // ── new ──
    let mut names = Names::default();
    let mut input_exprs = vec![String::new(); m.input_count as usize];
    let mut params = Vec::new();
    for item in &iface.inputs {
        let param = names.alloc(&item.name);
        for (k, path) in item.flat.iter().zip(&item.paths) {
            input_exprs[*k as usize] = format!("{param}{path}");
        }
        params.push(format!("{param}: {}", rust_ty(&item.ty, design, item_domain(item, &dom.inputs))));
    }

    let mut body = String::new();
    let mut read_names = Vec::new();
    let mut drive_names = Vec::new();
    if !m.targets.is_empty() {
        body.push_str("        // Nets.\n");
    }
    let mut nets = Lets::default();
    for (t_idx, t) in m.targets.iter().enumerate() {
        let read = names.alloc(&t.name);
        let drive = names.alloc(&format!("{read}_next"));
        nets.push(
            0,
            drive.clone(),
            "_".to_string(),
            format!(
                "::silane::Signal::<{}, D{}>::default().named({})",
                tn(t.width),
                dom.targets[t_idx],
                str_lit(&t.name)
            ),
        );
        nets.push(1, read.clone(), "_".to_string(), format!("{drive}.read()"));
        read_names.push(read);
        drive_names.push(drive);
    }
    nets.emit(&mut body);

    let child_ifaces: Vec<Interface> = m
        .instances
        .iter()
        .map(|inst| interface(&design.modules[inst.module], design))
        .collect();
    let mut exprs: Vec<String> = vec![String::new(); m.nodes.len()];
    let mut inst_names: Vec<String> = vec![String::new(); m.instances.len()];
    let steps = schedule(m);
    if !steps.is_empty() {
        body.push_str("        // Logic.\n");
    }
    // Logic depth of every node and instance, for grouping their bindings.
    let mut node_level = vec![0u32; m.nodes.len()];
    let mut inst_level = vec![0u32; m.instances.len()];
    let mut logic = Lets::default();
    for step in steps {
        match step {
            Step::Instance(i) => {
                let inst = &m.instances[i as usize];
                let child = &design.modules[inst.module];
                let child_iface = &child_ifaces[i as usize];
                let map = &dom.instances[i as usize];
                let var = names.alloc(&format!("u_{}", inst.name));
                let mut args = Vec::new();
                for item in &child_iface.inputs {
                    let mut flat = item
                        .flat
                        .iter()
                        .map(|&k| exprs[inst.inputs[k as usize] as usize].clone());
                    args.push(nest(&item.ty, &mut flat));
                }
                let args = args.join(", ");
                let level = 1 + inst.inputs.iter().map(|&n| node_level[n as usize]).max().unwrap_or(0);
                inst_level[i as usize] = level;
                // Extern modules too: through their adapters.
                let child_name = sanitize(&child.rust_name);
                let (ty, expr) = (child_ty(&child_name, map), format!("{child_name}::new({args})"));
                logic.push(level, var.clone(), ty, expr);
                inst_names[i as usize] = var;
            }
            Step::Node(n) => {
                let data = &m.nodes[n as usize];
                let domain = dom.nodes[n as usize];
                let e = |id: u32| exprs[id as usize].clone();
                node_level[n as usize] =
                    1 + data.node.operands().iter().map(|&a| node_level[a as usize]).max().unwrap_or(0);
                let expr = match &data.node {
                    Node::Input(k) => {
                        node_level[n as usize] = 0;
                        exprs[n as usize] = input_exprs[*k as usize].clone();
                        continue;
                    }
                    Node::Net(t) => {
                        node_level[n as usize] = 0;
                        exprs[n as usize] = read_names[*t as usize].clone();
                        continue;
                    }
                    Node::InstOut(i, o) => {
                        node_level[n as usize] = inst_level[*i as usize];
                        let item_path = child_ifaces[*i as usize]
                            .outputs
                            .iter()
                            .find_map(|item| {
                                item.flat
                                    .iter()
                                    .position(|f| f == o)
                                    .map(|j| format!("{}{}", item.name, item.paths[j]))
                            })
                            .expect("instance output");
                        exprs[n as usize] = format!("{}.{item_path}", inst_names[*i as usize]);
                        continue;
                    }
                    Node::Const(v) => format!(
                        "::silane::Signal::<{}, D{domain}>::constant_default({v})",
                        tn(data.width)
                    ),
                    Node::Not(a) => format!("!{}", e(*a)),
                    Node::Bin(op, a, b) => {
                        let (a, b) = (e(*a), e(*b));
                        match op {
                            BinKind::Add => format!("{a} + {b}"),
                            BinKind::Sub => format!("{a} - {b}"),
                            BinKind::Mul => format!("{a} * {b}"),
                            BinKind::And => format!("{a} & {b}"),
                            BinKind::Or => format!("{a} | {b}"),
                            BinKind::Xor => format!("{a} ^ {b}"),
                            BinKind::Shl => format!("{a} << {b}"),
                            BinKind::Shr => format!("{a} >> {b}"),
                            BinKind::Eq => format!("{a}.eq({b})"),
                            BinKind::Ne => format!("{a}.ne({b})"),
                            BinKind::Lt => format!("{a}.lt({b})"),
                            BinKind::Le => format!("{a}.le({b})"),
                            BinKind::Gt => format!("{a}.gt({b})"),
                            BinKind::Ge => format!("{a}.ge({b})"),
                        }
                    }
                    Node::Mux(c, t, f) => format!("{}.mux({}, {})", e(*c), e(*t), e(*f)),
                    Node::Slice(a, lo) => format!(
                        "{}.slice::<{}, {}, {}>()",
                        e(*a),
                        tn(*lo),
                        tn(lo + data.width),
                        tn(data.width)
                    ),
                    Node::Cat(a, b) => format!("{}.concat({})", e(*a), e(*b)),
                    Node::Rev(a) => format!("{}.rev()", e(*a)),
                    Node::Zext(a) => format!("{}.zext({}::default())", e(*a), tn(data.width)),
                };
                let (var, expr) = match m.names.get(&n) {
                    Some(hint) => (names.alloc(hint), format!("({expr}).named({})", str_lit(hint))),
                    None => (names.alloc(&format!("t{n}")), expr),
                };
                logic.push(node_level[n as usize], var.clone(), wire_ty(data.width, domain), expr);
                exprs[n as usize] = var;
            }
        }
    }
    logic.emit(&mut body);

    let driven: Vec<_> = m
        .targets
        .iter()
        .enumerate()
        .filter_map(|(t, target)| target.driver.map(|d| (t, target, d)))
        .collect();
    if !driven.is_empty() {
        body.push_str("        // Drivers.\n");
    }
    for (t, target, d) in driven {
        let value = &exprs[d as usize];
        match target.kind {
            TargetKind::Wire => {
                let _ = writeln!(body, "        {}.assign({value});", drive_names[t]);
            }
            TargetKind::Reg => {
                let _ = writeln!(body, "        {}.next_reset({value}, {});", drive_names[t], target.reset);
            }
        }
    }

    let _ = writeln!(out, "#[allow(dead_code, unused_variables, non_snake_case, clippy::all)]");
    let _ = writeln!(out, "impl{impl_params} {self_ty} {{");
    let _ = writeln!(out, "    /// Instantiates `{}` in the current silane elaboration.", m.display_name);
    let _ = writeln!(out, "    pub fn new({}) -> Self {{", params.join(", "));
    out.push_str(&body);
    out.push_str("        Self {\n");
    for item in &iface.outputs {
        let mut flat = item
            .flat
            .iter()
            .map(|&o| read_names[m.out_targets[o as usize] as usize].clone());
        let value = nest(&item.ty, &mut flat);
        if value == item.name {
            let _ = writeln!(out, "            {value},");
        } else {
            let _ = writeln!(out, "            {}: {value},", item.name);
        }
    }
    out.push_str("            _domains: ::std::marker::PhantomData,\n");
    out.push_str("        }\n    }\n\n");

    // ── circuit ──
    let _ = writeln!(
        out,
        "    /// Elaborates `{}` as a top-level circuit, after `bind` binds its clock domains",
        m.display_name
    );
    out.push_str("    /// (see `silane::Domain`). Ports keep their names (aggregate elements become\n");
    out.push_str("    /// `name_0`, `name_field`, ...).\n");
    out.push_str("    pub fn circuit_with(bind: impl FnOnce()) -> ::silane::Circuit {\n");
    out.push_str("        let (_, circuit) = ::silane::Circuit::elaborate(|| {\n");
    out.push_str("            bind();\n");
    // Create input ports in flattened-input order, so circuits list ports like the netlist backend.
    let mut port_inputs: Vec<(u32, String, u32)> = Vec::new();
    for port in &m.ports {
        for leaf in &port.leaves {
            if let LeafDir::In(k) = leaf.dir {
                port_inputs.push((k, leaf.flat_name.clone(), leaf.width));
            }
        }
    }
    port_inputs.sort_by_key(|(k, ..)| *k);
    for (k, port_name, width) in &port_inputs {
        let _ = writeln!(
            out,
            "            let in_{k} = ::silane::Signal::<{}, D{}>::input_port_default({});",
            tn(*width),
            dom.inputs[*k as usize],
            str_lit(port_name)
        );
    }
    let args: Vec<String> = iface
        .inputs
        .iter()
        .map(|item| {
            let mut flat = item.flat.iter().map(|k| format!("in_{k}"));
            nest(&item.ty, &mut flat)
        })
        .collect();
    let _ = writeln!(out, "            let top = Self::new({});", args.join(", "));
    let mut port_outputs: Vec<(u32, String)> = Vec::new();
    for item in &iface.outputs {
        for ((o, path), port_name) in item.flat.iter().zip(&item.paths).zip(&item.port_names) {
            port_outputs.push((*o, format!("            top.{}{path}.output_port({});", item.name, str_lit(port_name))));
        }
    }
    port_outputs.sort_by_key(|(o, _)| *o);
    for (_, line) in port_outputs {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("        });\n        circuit\n    }\n}\n");

    let all_sys: Vec<&str> = (0..dom.count).map(|_| "::silane::Sys").collect();
    let _ = writeln!(out, "\n#[allow(dead_code, clippy::all)]");
    let _ = writeln!(out, "impl {name}<{}> {{", all_sys.join(", "));
    let _ = writeln!(
        out,
        "    /// Elaborates `{}` as a top-level circuit with every clock domain on one clock:",
        m.display_name
    );
    out.push_str("    /// registers add `clk` and `rst` inputs.\n");
    out.push_str("    pub fn circuit() -> ::silane::Circuit {\n");
    out.push_str("        Self::circuit_with(|| ::silane::Domain::<::silane::Sys>::ports_on_use(\"clk\", \"rst\"))\n");
    out.push_str("    }\n}\n");
}
