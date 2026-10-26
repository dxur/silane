//! Elaboration: turns the syntax tree into a [`Design`].
//!
//! Every module is elaborated once per distinct parameter assignment. Elaboration
//! evaluates compile-time expressions, unrolls `for` loops, picks compile-time
//! `if`/`match` branches, type-checks every expression, and lowers procedural
//! assignments to multiplexer trees with last-connect semantics.
//!
//! A net declared without a value (`let x: T;`, or an output) becomes a wire when it is
//! driven with `=` and a register when it is driven with `<=`:
//!
//! - a wire must be driven on every path through the module body;
//! - a register keeps its current value on paths that do not assign it.

use crate::ast::*;
use crate::diag::{Diagnostic, Diagnostics, Span};
use crate::ir::{
    BinKind, BundleDef, Design, EnumDef, Field, Instance, LeafDir, ModuleIr, Node, NodeId, Port,
    PortLeaf, Target, TargetKind, Ty, Types, fold_bin, mask,
};
use std::collections::{HashMap, HashSet};

/// Widest supported signal (the simulator stores one `u64` per net).
pub const MAX_WIDTH: u32 = 64;
const MAX_FLAT_LEN: u32 = 1 << 16;
const MAX_LOOP_ITERATIONS: i128 = 1 << 16;
const MAX_INSTANCE_DEPTH: usize = 64;
const BUILTINS: &[&str] = &["cat", "rep", "clog2", "min", "max"];
/// Port names created implicitly by the silane library for the register clock domain.
const RESERVED_PORTS: &[&str] = &["clk", "rst"];

/// Rust paths of library primitives whose outputs are registered: they break
/// combinational paths, unlike other `extern module`s.
pub const REGISTERED_EXTERNS: &[&str] = &[MEM_PATH];

/// Rust path of the `std:mem` RAM, which the direct netlist backend builds itself.
pub const MEM_PATH: &str = "::silane::mem::Mem";
/// Item names used by the generated Rust code.
const RESERVED_ITEMS: &[&str] = &["SiWire"];

#[derive(Debug, Clone)]
enum Value {
    /// Unsized compile-time integer (literals, parameters, loop variables).
    Int(i128),
    /// Sized constant with a known value. The type is scalar.
    Const(u64, Ty),
    /// Hardware signal. The type is scalar.
    Hw(NodeId, Ty),
    /// Array value: elements and their (common) type.
    Array(Vec<Value>, Ty),
    /// Bundle value: one value per field.
    Bundle(Vec<Value>, usize),
    /// Result of an expression that already produced an error.
    Poison,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValKind {
    Param,
    Let,
    LoopVar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaceKind {
    /// An argument port.
    Arg,
    /// An output port.
    Ret,
    /// A net declared with `let x: T;`.
    Net,
    Instance,
}

#[derive(Debug, Clone)]
enum BindItem {
    Val(Value, ValKind),
    /// A signal with a location: type and first slot.
    Place(Ty, u32, PlaceKind),
}

#[derive(Debug, Clone)]
struct Binding {
    item: BindItem,
    span: Span,
    used: bool,
}

/// What a scalar element of a place refers to.
#[derive(Debug, Clone, Copy)]
enum Slot {
    /// An input of this module (read-only).
    Input(NodeId),
    /// An output of an instance (read-only).
    InstOut(NodeId),
    /// A net this module drives.
    Target(u32),
}

/// Elaboration-time facts about a target.
#[derive(Debug, Clone)]
struct NetInfo {
    /// Fixed by the first assignment (or by being an instance input).
    kind: Option<TargetKind>,
    kind_span: Option<Span>,
    /// Instance inputs can only be connected with `=`.
    instance_input: bool,
    reset: Option<Span>,
    is_output: bool,
}

/// Procedural value of a target at the current point of the module body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Drv {
    node: Option<NodeId>,
    /// Whether every path so far assigns the target.
    total: bool,
}

#[derive(Debug, Clone, Copy)]
enum Cond {
    Static(bool),
    Dyn(NodeId),
}

/// A resolved place expression: its type and the slot(s) it denotes. A dynamic array
/// index gives one entry per possible element, each with its selection condition.
#[derive(Debug, Clone)]
struct Place {
    ty: Ty,
    entries: Vec<(u32, Option<NodeId>)>,
}

struct Ctx {
    ir: ModuleIr,
    scopes: Vec<HashMap<String, Binding>>,
    slots: Vec<Slot>,
    slot_names: Vec<String>,
    drivers: Vec<Drv>,
    nets: Vec<NetInfo>,
    /// Number of enclosing hardware (non-static) conditional branches.
    cond_depth: u32,
    loop_iterations: i128,
}

impl Ctx {
    fn new(ir: ModuleIr) -> Self {
        Ctx {
            ir,
            scopes: vec![HashMap::new()],
            slots: Vec::new(),
            slot_names: Vec::new(),
            drivers: Vec::new(),
            nets: Vec::new(),
            cond_depth: 0,
            loop_iterations: 0,
        }
    }

    /// A context for compile-time evaluation outside any module.
    fn global(span: Span) -> Self {
        Ctx::new(ModuleIr::new(String::new(), span))
    }
}

enum Global<'a> {
    Const(&'a ConstDecl),
    Enum(usize),
    Module(&'a ModuleDecl),
    Alias(&'a AliasDecl),
}

enum ConstState {
    InProgress,
    Done(Value),
}

pub fn elaborate(files: &[File], diags: &mut Diagnostics) -> Design {
    let mut elab = Elab {
        diags,
        globals: HashMap::new(),
        consts: HashMap::new(),
        types: Types::default(),
        modules: Vec::new(),
        cache: HashMap::new(),
        bundle_cache: HashMap::new(),
        signatures: HashMap::new(),
        stack: Vec::new(),
        errors: 0,
    };
    elab.run(files)
}

struct Elab<'a, 'd> {
    diags: &'d mut Diagnostics,
    globals: HashMap<String, (Global<'a>, Span)>,
    consts: HashMap<String, ConstState>,
    types: Types,
    modules: Vec<ModuleIr>,
    /// `None` while the instantiation is being elaborated (recursion detection).
    cache: HashMap<(String, Vec<i128>), Option<Option<usize>>>,
    bundle_cache: HashMap<(String, Vec<i128>), Option<usize>>,
    /// Signature bundle of each elaborated module.
    signatures: HashMap<usize, usize>,
    stack: Vec<String>,
    /// Errors reported so far, including ones de-duplicated away by `Diagnostics`.
    errors: usize,
}

fn fits(v: i128, width: u32) -> bool {
    v >= 0 && (width >= 64 && v <= u64::MAX as i128 || width < 64 && v < (1i128 << width))
}

fn max_value(width: u32) -> u128 {
    if width >= 64 {
        u64::MAX as u128
    } else {
        (1u128 << width) - 1
    }
}

fn clog2(n: i128) -> i128 {
    let mut bits = 0;
    while (1i128 << bits) < n {
        bits += 1;
    }
    bits
}

fn ty_of(v: &Value) -> Option<Ty> {
    match v {
        Value::Const(_, t) | Value::Hw(_, t) => Some(t.clone()),
        Value::Array(elems, t) => Some(Ty::Array(Box::new(t.clone()), elems.len() as u32)),
        Value::Bundle(_, b) => Some(Ty::Bundle(*b)),
        _ => None,
    }
}

/// Whether `e` instantiates a module (as opposed to calling a built-in function).
fn is_module_call(globals: &HashMap<String, (Global<'_>, Span)>, e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Call { callee, .. } => matches!(
            globals.get(&callee.name),
            Some((Global::Module(_) | Global::Alias(_), _))
        ),
        _ => false,
    }
}

type ParamArgs = Vec<(Option<Ident>, i128, Span)>;

/// A module without a body or a Rust implementation: a bundle type.
fn is_bundle(m: &ModuleDecl) -> bool {
    m.body.is_none() && m.rust_path.is_none()
}

impl<'a> Elab<'a, '_> {
    // ── diagnostics ──────────────────────────────────────────────────────

    fn error(&mut self, mut diag: Diagnostic) {
        self.errors += 1;
        if let Some(top) = self.stack.last()
            && top.contains('<')
        {
            diag = diag.note(format!("in instance `{top}`"));
        }
        self.diags.push(diag);
    }

    fn ty_name(&self, ty: &Ty) -> String {
        ty.display(&self.types)
    }

    fn value_desc(&self, v: &Value) -> String {
        match v {
            Value::Int(i) => format!("integer `{i}`"),
            Value::Poison => "an error".to_string(),
            other => format!("`{}`", self.ty_name(&ty_of(other).unwrap())),
        }
    }

    fn width(&self, ty: &Ty) -> u32 {
        ty.width(&self.types)
    }

    // ── top level ────────────────────────────────────────────────────────

    fn run(&mut self, files: &'a [File]) -> Design {
        let mut enum_decls = Vec::new();
        let mut modules = Vec::new();
        let mut aliases = Vec::new();
        for file in files {
            for item in &file.items {
                let (name, global) = match item {
                    Item::Import { .. } => continue,
                    Item::Const(c) => (&c.name, Global::Const(c)),
                    Item::Enum(e) => {
                        enum_decls.push(e);
                        (&e.name, Global::Enum(enum_decls.len() - 1))
                    }
                    Item::Module(m) => {
                        modules.push(m);
                        (&m.name, Global::Module(m))
                    }
                    Item::Alias(a) => {
                        aliases.push(a);
                        (&a.name, Global::Alias(a))
                    }
                };
                if BUILTINS.contains(&name.name.as_str())
                    || RESERVED_ITEMS.contains(&name.name.as_str())
                {
                    self.error(
                        Diagnostic::error(format!("`{}` is a reserved name", name.name))
                            .at(name.span, "choose another name"),
                    );
                    continue;
                }
                if let Some((_, prev)) = self.globals.get(&name.name) {
                    let prev = *prev;
                    self.error(
                        Diagnostic::error(format!("`{}` is defined more than once", name.name))
                            .at(name.span, "redefined here")
                            .with(prev, "first defined here"),
                    );
                    continue;
                }
                self.globals.insert(name.name.clone(), (global, name.span));
            }
        }

        self.resolve_enums(&enum_decls);

        for m in &modules {
            let all_default = m.params.iter().all(|p| p.default.is_some());
            if !all_default {
                continue;
            }
            if !is_bundle(m) {
                self.instantiate(m, Vec::new(), m.name.span);
            } else {
                // Check bundle declarations even when unused.
                self.bundle_type(m, Vec::new(), m.name.span);
            }
        }

        let mut design_aliases = Vec::new();
        for alias in &aliases {
            let decl = match self.globals.get(&alias.target.name) {
                Some((Global::Module(m), _)) if m.rust_path.is_some() => {
                    self.error(
                        Diagnostic::error(format!(
                            "`{}` is implemented in Rust and cannot be exported under another name",
                            alias.target.name
                        ))
                        .at(alias.target.span, "declared `extern module`")
                        .note("wrap it in a module, or use its Rust type directly"),
                    );
                    continue;
                }
                Some((Global::Module(m), _)) if !is_bundle(m) => *m,
                Some((Global::Module(_), _)) => {
                    self.error(
                        Diagnostic::error(format!("`{}` is a bundle, not a module", alias.target.name))
                            .at(alias.target.span, "has no body"),
                    );
                    continue;
                }
                Some((Global::Alias(_), _)) => {
                    self.error(
                        Diagnostic::error("an alias cannot refer to another alias")
                            .at(alias.target.span, "this is an alias"),
                    );
                    continue;
                }
                _ => {
                    self.error(
                        Diagnostic::error(format!("cannot find module `{}`", alias.target.name))
                            .at(alias.target.span, "not found"),
                    );
                    continue;
                }
            };
            let mut cx = Ctx::global(alias.span);
            let args = self.generic_args(&mut cx, &alias.params);
            if let Some(idx) = self.instantiate(decl, args, alias.span) {
                design_aliases.push((alias.name.name.clone(), idx));
            }
        }

        for m in &modules {
            let used = self.cache.keys().any(|(name, _)| *name == m.name.name)
                || self.bundle_cache.keys().any(|(name, _)| *name == m.name.name);
            if !used && m.params.iter().any(|p| p.default.is_none()) {
                let what = if is_bundle(m) { "bundle" } else { "module" };
                self.diags.push(
                    Diagnostic::warning(format!(
                        "{what} `{}` is never used, so it was not checked",
                        m.name.name
                    ))
                    .at(m.name.span, "")
                    .note(format!(
                        "give every parameter a default, or export an instance with `module Name = {}::<...>;`",
                        m.name.name
                    )),
                );
            }
        }

        Design {
            modules: std::mem::take(&mut self.modules),
            types: std::mem::take(&mut self.types),
            aliases: design_aliases,
        }
    }

    fn resolve_enums(&mut self, decls: &[&'a EnumDecl]) {
        for decl in decls {
            let mut cx = Ctx::global(decl.name.span);
            let mut variants: Vec<(String, u64)> = Vec::new();
            let mut spans: HashMap<String, Span> = HashMap::new();
            let mut next: i128 = 0;
            for (name, value) in &decl.variants {
                let v = match value {
                    Some(e) => self.expect_int(&mut cx, e).unwrap_or(next),
                    None => next,
                };
                if !(0..=u64::MAX as i128).contains(&v) {
                    let span = value.as_ref().map_or(name.span, |e| e.span);
                    self.error(
                        Diagnostic::error(format!("enum value {v} is out of range"))
                            .at(span, "must be a non-negative 64-bit value"),
                    );
                }
                if let Some(prev) = spans.get(&name.name) {
                    let prev = *prev;
                    self.error(
                        Diagnostic::error(format!("duplicate variant `{}`", name.name))
                            .at(name.span, "redefined here")
                            .with(prev, "first defined here"),
                    );
                    continue;
                }
                if let Some((other, _)) = variants.iter().find(|(_, x)| *x as i128 == v) {
                    self.error(
                        Diagnostic::error(format!(
                            "variants `{other}` and `{}` have the same value {v}",
                            name.name
                        ))
                        .at(name.span, ""),
                    );
                }
                spans.insert(name.name.clone(), name.span);
                variants.push((name.name.clone(), v.max(0) as u64));
                next = v + 1;
            }
            if variants.is_empty() {
                self.error(
                    Diagnostic::error(format!("enum `{}` has no variants", decl.name.name))
                        .at(decl.name.span, ""),
                );
            }
            let max = variants.iter().map(|(_, v)| *v).max().unwrap_or(0);
            let needed = (64 - max.leading_zeros()).max(1);
            let width = match &decl.repr {
                Some(TypeExpr::UInt { width, span }) => {
                    let span = *span;
                    match self.expect_int(&mut cx, width) {
                        Some(w) if w >= needed as i128 && w <= MAX_WIDTH as i128 => w as u32,
                        Some(w) => {
                            self.error(
                                Diagnostic::error(format!(
                                    "enum `{}` needs at least {needed} bits, but is declared as `u{w}`",
                                    decl.name.name
                                ))
                                .at(span, ""),
                            );
                            needed
                        }
                        None => needed,
                    }
                }
                Some(other) => {
                    self.error(
                        Diagnostic::error("an enum representation must be an unsigned type")
                            .at(other.span(), "expected `uN`"),
                    );
                    needed
                }
                None => needed,
            };
            self.types.enums.push(EnumDef {
                name: decl.name.name.clone(),
                width,
                variants,
                span: decl.name.span,
            });
        }
    }

    fn generic_args(&mut self, cx: &mut Ctx, args: &'a [GenericArg]) -> ParamArgs {
        args.iter()
            .filter_map(|a| {
                self.expect_int(cx, &a.value)
                    .map(|v| (a.name.clone(), v, a.value.span))
            })
            .collect()
    }

    /// Resolves parameter arguments against a module's parameter list. Returns a
    /// context with the parameters bound, their values, and whether they are all defaults.
    fn resolve_params(
        &mut self,
        decl: &'a ModuleDecl,
        args: ParamArgs,
        site: Span,
    ) -> Option<(Ctx, Vec<(String, i128)>, bool)> {
        let mut values: Vec<Option<i128>> = vec![None; decl.params.len()];
        let mut positional_done = false;
        let mut ok = true;
        for (i, (name, value, span)) in args.iter().enumerate() {
            let idx = match name {
                Some(name) => {
                    positional_done = true;
                    match decl.params.iter().position(|p| p.name.name == name.name) {
                        Some(idx) => idx,
                        None => {
                            self.error(
                                Diagnostic::error(format!(
                                    "`{}` has no parameter `{}`",
                                    decl.name.name, name.name
                                ))
                                .at(name.span, "unknown parameter")
                                .with(decl.name.span, "defined here"),
                            );
                            ok = false;
                            continue;
                        }
                    }
                }
                None => {
                    if positional_done {
                        self.error(
                            Diagnostic::error("positional parameters must come before named ones")
                                .at(*span, ""),
                        );
                        ok = false;
                        continue;
                    }
                    if i >= decl.params.len() {
                        self.error(
                            Diagnostic::error(format!(
                                "`{}` takes {} parameter(s)",
                                decl.name.name,
                                decl.params.len()
                            ))
                            .at(*span, "unexpected parameter"),
                        );
                        ok = false;
                        continue;
                    }
                    i
                }
            };
            if values[idx].is_some() {
                self.error(
                    Diagnostic::error(format!(
                        "parameter `{}` is given more than once",
                        decl.params[idx].name.name
                    ))
                    .at(*span, ""),
                );
                ok = false;
            }
            values[idx] = Some(*value);
        }

        let mut cx = Ctx::global(decl.name.span);
        let mut params = Vec::new();
        let mut all_default = true;
        for (i, param) in decl.params.iter().enumerate() {
            let default = match &param.default {
                Some(e) => self.expect_int(&mut cx, e),
                None => None,
            };
            let value = match (values[i], default) {
                (Some(v), d) => {
                    if d != Some(v) {
                        all_default = false;
                    }
                    v
                }
                (None, Some(d)) => d,
                (None, None) => {
                    if param.default.is_none() {
                        self.error(
                            Diagnostic::error(format!(
                                "missing value for parameter `{}` of `{}`",
                                param.name.name, decl.name.name
                            ))
                            .at(site, format!("`{}` must be given here", param.name.name))
                            .with(param.name.span, "parameter declared here"),
                        );
                    }
                    ok = false;
                    0
                }
            };
            cx.scopes[0].insert(
                param.name.name.clone(),
                Binding {
                    item: BindItem::Val(Value::Int(value), ValKind::Param),
                    span: param.name.span,
                    used: true,
                },
            );
            params.push((param.name.name.clone(), value));
        }
        ok.then_some((cx, params, all_default))
    }

    fn display_name(name: &str, params: &[(String, i128)]) -> String {
        if params.is_empty() {
            name.to_string()
        } else {
            let list: Vec<String> = params.iter().map(|(n, v)| format!("{n} = {v}")).collect();
            format!("{name}<{}>", list.join(", "))
        }
    }

    /// Elaborates the bundle type declared by a body-less module.
    fn bundle_type(&mut self, decl: &'a ModuleDecl, args: ParamArgs, site: Span) -> Option<Ty> {
        let (mut cx, params, _) = self.resolve_params(decl, args, site)?;
        let key = (
            decl.name.name.clone(),
            params.iter().map(|(_, v)| *v).collect::<Vec<_>>(),
        );
        if let Some(cached) = self.bundle_cache.get(&key) {
            return cached.map(Ty::Bundle);
        }
        self.bundle_cache.insert(key.clone(), None);
        let errors_before = self.errors;
        let mut fields = Vec::new();
        let mut names: HashMap<String, Span> = HashMap::new();
        for (port, flipped) in decl
            .args
            .iter()
            .map(|p| (p, true))
            .chain(decl.rets.iter().map(|p| (p, false)))
        {
            if let Some(prev) = names.insert(port.name.name.clone(), port.name.span) {
                self.error(
                    Diagnostic::error(format!("duplicate field `{}`", port.name.name))
                        .at(port.name.span, "redefined here")
                        .with(prev, "first declared here"),
                );
                continue;
            }
            if let Some(reset) = &port.reset {
                self.error(
                    Diagnostic::error("bundle fields cannot have reset values")
                        .at(reset.span, "")
                        .note("give the reset where a signal of the bundle type is declared"),
                );
            }
            if let Some(ty) = self.resolve_ty(&mut cx, &port.ty) {
                fields.push(Field {
                    name: port.name.name.clone(),
                    ty,
                    flipped,
                });
            }
        }
        if fields.is_empty() {
            self.error(
                Diagnostic::error(format!("bundle `{}` has no fields", decl.name.name))
                    .at(decl.name.span, ""),
            );
        }
        if self.errors > errors_before {
            return None;
        }
        self.types.bundles.push(BundleDef {
            name: decl.name.name.clone(),
            display: Self::display_name(&decl.name.name, &params),
            fields,
        });
        let id = self.types.bundles.len() - 1;
        self.bundle_cache.insert(key, Some(id));
        Some(Ty::Bundle(id))
    }

    /// Signature of an elaborated module as a bundle: argument ports are flipped.
    fn signature(&mut self, module: usize) -> usize {
        if let Some(&b) = self.signatures.get(&module) {
            return b;
        }
        let m = &self.modules[module];
        let fields = m
            .ports
            .iter()
            .map(|p| Field {
                name: p.name.clone(),
                ty: p.ty.clone(),
                flipped: p.is_arg,
            })
            .collect();
        self.types.bundles.push(BundleDef {
            name: m.source_name.clone(),
            display: m.display_name.clone(),
            fields,
        });
        let id = self.types.bundles.len() - 1;
        self.signatures.insert(module, id);
        id
    }

    /// Elaborates `decl` with the given parameter arguments, returning its index in
    /// the design (or `None` if it failed).
    fn instantiate(&mut self, decl: &'a ModuleDecl, args: ParamArgs, site: Span) -> Option<usize> {
        let (cx, params, all_default) = self.resolve_params(decl, args, site)?;
        let key = (
            decl.name.name.clone(),
            params.iter().map(|(_, v)| *v).collect::<Vec<_>>(),
        );
        let display = Self::display_name(&decl.name.name, &params);
        match self.cache.get(&key) {
            Some(Some(result)) => return *result,
            Some(None) => {
                self.error(
                    Diagnostic::error(format!("`{display}` instantiates itself"))
                        .at(site, "recursive instantiation"),
                );
                return None;
            }
            None => {}
        }
        if self.stack.len() >= MAX_INSTANCE_DEPTH {
            self.error(
                Diagnostic::error("module hierarchy is too deep").at(site, "while instantiating this"),
            );
            return None;
        }

        self.cache.insert(key.clone(), None);
        self.stack.push(display.clone());
        let mut ir = ModuleIr::new(decl.name.name.clone(), decl.name.span);
        ir.display_name = display;
        ir.params = params.clone();
        ir.rust_name = if all_default {
            decl.name.name.clone()
        } else {
            let parts: Vec<String> = params
                .iter()
                .map(|(n, v)| {
                    if *v < 0 {
                        format!("{n}m{}", -v)
                    } else {
                        format!("{n}{v}")
                    }
                })
                .collect();
            format!("{}_{}", decl.name.name, parts.join("_"))
        };
        let errors_before = self.errors;
        let (ir, checked) = self.elaborate_module(decl, cx, ir);
        self.stack.pop();

        let result = if self.errors > errors_before || !checked {
            None
        } else {
            self.modules.push(ir);
            Some(self.modules.len() - 1)
        };
        self.cache.insert(key, Some(result));
        result
    }

    fn elaborate_module(&mut self, decl: &'a ModuleDecl, params_cx: Ctx, ir: ModuleIr) -> (ModuleIr, bool) {
        let mut cx = Ctx::new(ir);
        cx.scopes = params_cx.scopes;
        cx.scopes.push(HashMap::new());

        let mut port_names: HashMap<String, Span> = HashMap::new();
        let mut flat_names: HashSet<String> = HashSet::new();
        for (port, is_arg) in decl
            .args
            .iter()
            .map(|p| (p, true))
            .chain(decl.rets.iter().map(|p| (p, false)))
        {
            let name = &port.name;
            if let Some(prev) = port_names.get(&name.name) {
                let prev = *prev;
                self.error(
                    Diagnostic::error(format!("duplicate port `{}`", name.name))
                        .at(name.span, "redefined here")
                        .with(prev, "first declared here"),
                );
                continue;
            }
            port_names.insert(name.name.clone(), name.span);
            if RESERVED_PORTS.contains(&name.name.as_str()) {
                self.error(
                    Diagnostic::error(format!("`{}` is a reserved port name", name.name))
                        .at(name.span, "")
                        .note("every module has an implicit clock `clk` and reset `rst`"),
                );
                continue;
            }
            let Some(ty) = self.resolve_ty(&mut cx, &port.ty) else {
                continue;
            };
            let base = cx.slots.len() as u32;
            let mut leaves = Vec::new();
            for leaf in ty.leaves(&self.types) {
                let path = format!("{}{}", name.name, leaf.path);
                let flat_name = format!("{}{}", name.name, leaf.flat);
                if !flat_names.insert(flat_name.clone()) {
                    self.error(
                        Diagnostic::error(format!(
                            "port element `{path}` collides with another port named `{flat_name}`"
                        ))
                        .at(name.span, "")
                        .note("aggregate ports are flattened to `name_0`, `name_field`, ... in circuits"),
                    );
                }
                let width = self.width(&leaf.ty);
                // Argument leaves flow in unless flipped; output leaves flow out unless flipped.
                let is_input = is_arg != leaf.flipped;
                let dir = if is_input {
                    let k = cx.ir.input_count;
                    cx.ir.input_count += 1;
                    cx.ir.cur_span = name.span;
                    let node = cx.ir.add(Node::Input(k), width);
                    cx.ir.name(node, &path);
                    cx.slots.push(Slot::Input(node));
                    LeafDir::In(k)
                } else {
                    let t = self.new_target(&mut cx, &path, width, name.span, true);
                    cx.slots.push(Slot::Target(t));
                    let o = cx.ir.out_targets.len() as u32;
                    cx.ir.out_targets.push(t);
                    LeafDir::Out(o, t)
                };
                cx.slot_names.push(path.clone());
                leaves.push(PortLeaf {
                    path,
                    flat_name,
                    width,
                    dir,
                });
            }
            if let Some(reset) = &port.reset {
                self.apply_reset(&mut cx, base, &ty, reset, name);
            }
            if let Some(label) = &port.domain
                && decl.rust_path.is_none()
            {
                self.error(
                    Diagnostic::error("only `extern module` ports have domain labels")
                        .at(label.span, "")
                        .note("clock domains of `.si` modules are inferred from how their signals connect"),
                );
            }
            cx.ir.ports.push(Port {
                name: name.name.clone(),
                ty: ty.clone(),
                is_arg,
                domain: port.domain.as_ref().map(|d| d.name.clone()),
                leaves,
            });
            let kind = if is_arg { PlaceKind::Arg } else { PlaceKind::Ret };
            self.bind(&mut cx, name, BindItem::Place(ty, base, kind));
        }

        if let Some(path) = &decl.rust_path {
            // Implemented in Rust: nothing to elaborate. Its outputs are assumed to
            // depend combinationally on all of its inputs, except for library
            // primitives known to have registered outputs.
            let mut ir = cx.ir;
            ir.rust_path = Some(path.clone());
            let all: Vec<u32> = if REGISTERED_EXTERNS.contains(&path.as_str()) {
                Vec::new()
            } else {
                (0..ir.input_count).collect()
            };
            ir.comb_deps = vec![all; ir.output_count() as usize];
            return (ir, true);
        }
        if let Some(body) = &decl.body {
            self.stmts(&mut cx, &body.stmts);
        }
        while cx.scopes.len() > 1 {
            self.pop_scope(&mut cx);
        }

        self.finalize_targets(&mut cx);
        let mut ir = cx.ir;
        let checked = crate::check::analyze(&mut ir, &self.modules, self.diags);
        (ir, checked)
    }

    fn new_target(&mut self, cx: &mut Ctx, name: &str, width: u32, span: Span, is_output: bool) -> u32 {
        let t = cx.ir.targets.len() as u32;
        cx.ir.targets.push(Target {
            name: name.to_string(),
            kind: TargetKind::Wire,
            width,
            reset: 0,
            driver: None,
            span,
        });
        cx.drivers.push(Drv {
            node: None,
            total: false,
        });
        cx.nets.push(NetInfo {
            kind: None,
            kind_span: None,
            instance_input: false,
            reset: None,
            is_output,
        });
        t
    }

    /// Records the reset value of every target in a place (used if it becomes a register).
    fn apply_reset(&mut self, cx: &mut Ctx, base: u32, ty: &Ty, reset: &'a Expr, name: &Ident) {
        let leaves = ty.leaves(&self.types);
        let scalar = leaves[0].ty.clone();
        if leaves.iter().any(|l| l.ty != scalar) {
            self.error(
                Diagnostic::error(format!(
                    "`{}` has elements of different types, so it cannot take one reset value",
                    name.name
                ))
                .at(reset.span, ""),
            );
            return;
        }
        let value = self.reset_value(cx, reset, &scalar);
        let mut any_target = false;
        for j in 0..leaves.len() as u32 {
            if let Slot::Target(t) = cx.slots[(base + j) as usize] {
                cx.ir.targets[t as usize].reset = value;
                cx.nets[t as usize].reset = Some(reset.span);
                any_target = true;
            }
        }
        if !any_target {
            self.error(
                Diagnostic::error(format!("`{}` is an input, so it cannot have a reset value", name.name))
                    .at(reset.span, ""),
            );
        }
    }

    fn finalize_targets(&mut self, cx: &mut Ctx) {
        // Group failures per declaration so arrays report once.
        let mut undriven: Vec<(Span, usize, Diagnostic)> = Vec::new();
        let mut reset_on_wire: HashSet<Span> = HashSet::new();
        for t in 0..cx.ir.targets.len() {
            let drv = self.current(cx, t as u32);
            let target = cx.ir.targets[t].clone();
            let info = cx.nets[t].clone();
            let kind = info.kind.unwrap_or(TargetKind::Wire);
            cx.ir.targets[t].kind = kind;
            cx.ir.targets[t].driver = drv.node;
            if kind == TargetKind::Reg {
                continue;
            }
            if let Some(reset) = info.reset
                && info.kind.is_some()
                && reset_on_wire.insert(reset)
            {
                self.error(
                    Diagnostic::error(format!("`{}` has a reset value but is driven with `=`", target.name))
                        .at(reset, "reset value given here")
                        .with(info.kind_span.unwrap_or(target.span), "driven combinationally here")
                        .note("only registers (driven with `<=`) have reset values"),
                );
            }
            if drv.node.is_some() && drv.total {
                continue;
            }
            if let Some(entry) = undriven.iter_mut().find(|(s, ..)| *s == target.span) {
                entry.1 += 1;
                continue;
            }
            let diag = if drv.node.is_none() && info.instance_input {
                let (inst, port) = target.name.split_once('.').unwrap_or(("?", &target.name));
                Diagnostic::error(format!("input `{port}` of instance `{inst}` is not connected"))
                    .at(target.span, "instantiated here")
                    .note(format!("pass it as an argument, or connect it with `{} = <value>;`", target.name))
            } else if drv.node.is_none() {
                let what = if info.is_output { "output" } else { "net" };
                Diagnostic::error(format!("{what} `{}` is never driven", target.name))
                    .at(target.span, "declared here")
                    .note(format!(
                        "drive it with `{0} = <value>;` (a wire) or `{0} <= <value>;` (a register)",
                        target.name
                    ))
            } else {
                Diagnostic::error(format!("`{}` is not driven on every path", target.name))
                    .at(target.span, "declared here")
                    .with(info.kind_span.unwrap_or(target.span), "driven with `=` here, so it is a wire")
                    .note("a wire needs a value in every case; give it a default first (`x = 0;`), add the missing `else`/`_` branch, or make it a register with `<=`")
            };
            undriven.push((target.span, 1, diag));
        }
        for (_, count, mut diag) in undriven {
            if count > 1 {
                diag.message.push_str(&format!(" (and {} more element(s))", count - 1));
            }
            self.error(diag);
        }
    }

    // ── scopes and bindings ──────────────────────────────────────────────

    fn find_binding(&self, cx: &Ctx, name: &str) -> Option<Binding> {
        cx.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn mark_used(cx: &mut Ctx, name: &str) {
        for scope in cx.scopes.iter_mut().rev() {
            if let Some(b) = scope.get_mut(name) {
                b.used = true;
                return;
            }
        }
    }

    fn bind(&mut self, cx: &mut Ctx, name: &Ident, item: BindItem) {
        if let Some(prev) = self.find_binding(cx, &name.name) {
            self.error(
                Diagnostic::error(format!("`{}` is already defined", name.name))
                    .at(name.span, "redefined here")
                    .with(prev.span, "first defined here")
                    .note("names cannot be shadowed within a module"),
            );
            return;
        }
        if let Some((Global::Const(_), span)) = self.globals.get(&name.name) {
            let span = *span;
            self.error(
                Diagnostic::error(format!("`{}` is already a global constant", name.name))
                    .at(name.span, "")
                    .with(span, "constant defined here"),
            );
            return;
        }
        cx.scopes.last_mut().unwrap().insert(
            name.name.clone(),
            Binding {
                item,
                span: name.span,
                used: false,
            },
        );
    }

    fn pop_scope(&mut self, cx: &mut Ctx) {
        let scope = cx.scopes.pop().unwrap_or_default();
        let mut unused: Vec<(String, Binding)> = scope
            .into_iter()
            .filter(|(name, b)| !b.used && !name.starts_with('_'))
            .collect();
        unused.sort_by_key(|(_, b)| b.span);
        for (name, b) in unused {
            let what = match &b.item {
                BindItem::Val(_, ValKind::Let) => format!("unused binding `{name}`"),
                BindItem::Val(_, ValKind::LoopVar) => format!("unused loop variable `{name}`"),
                BindItem::Place(ty, base, kind @ (PlaceKind::Arg | PlaceKind::Instance)) => {
                    let first = *base as usize;
                    let count = ty.flat_len(&self.types) as usize;
                    let slots = &cx.slots[first..first + count];
                    if *kind == PlaceKind::Arg {
                        // Only warn for ports that are purely inputs.
                        if slots.iter().any(|s| matches!(s, Slot::Target(_))) {
                            continue;
                        }
                        format!("unused input `{name}`")
                    } else {
                        if !slots.iter().any(|s| matches!(s, Slot::InstOut(_))) {
                            continue;
                        }
                        format!("the outputs of instance `{name}` are never read")
                    }
                }
                BindItem::Place(_, _, PlaceKind::Net) => format!("`{name}` is never read"),
                BindItem::Val(_, ValKind::Param) | BindItem::Place(_, _, PlaceKind::Ret) => continue,
            };
            self.diags.push(
                Diagnostic::warning(what)
                    .at(b.span, "")
                    .note(format!("if this is intentional, prefix it with an underscore: `_{name}`")),
            );
        }
    }

    fn with_scope(&mut self, cx: &mut Ctx, f: impl FnOnce(&mut Self, &mut Ctx)) {
        cx.scopes.push(HashMap::new());
        f(self, cx);
        self.pop_scope(cx);
    }

    fn read_slot(&mut self, cx: &mut Ctx, slot: u32) -> NodeId {
        match cx.slots[slot as usize] {
            Slot::Input(n) | Slot::InstOut(n) => n,
            Slot::Target(t) => {
                let w = cx.ir.targets[t as usize].width;
                cx.ir.add(Node::Net(t), w)
            }
        }
    }

    /// Value of a place: the current register outputs and final wire values.
    fn read_place(&mut self, cx: &mut Ctx, ty: &Ty, base: u32) -> Value {
        match ty {
            Ty::Array(elem, n) => {
                let stride = elem.flat_len(&self.types);
                Value::Array(
                    (0..*n)
                        .map(|i| self.read_place(cx, elem, base + i * stride))
                        .collect(),
                    (**elem).clone(),
                )
            }
            Ty::Bundle(b) => {
                let fields: Vec<Ty> = self.types.bundles[*b].fields.iter().map(|f| f.ty.clone()).collect();
                let mut offset = base;
                let mut values = Vec::new();
                for fty in fields {
                    values.push(self.read_place(cx, &fty, offset));
                    offset += fty.flat_len(&self.types);
                }
                Value::Bundle(values, *b)
            }
            scalar => {
                let node = self.read_slot(cx, base);
                Value::Hw(node, scalar.clone())
            }
        }
    }

    fn lookup(&mut self, cx: &mut Ctx, id: &Ident) -> Value {
        if let Some(b) = self.find_binding(cx, &id.name) {
            Self::mark_used(cx, &id.name);
            return match b.item {
                BindItem::Val(v, _) => v,
                BindItem::Place(ty, base, _) => self.read_place(cx, &ty, base),
            };
        }
        match self.globals.get(&id.name) {
            Some((Global::Const(decl), _)) => {
                let decl = *decl;
                self.global_const(decl, id.span)
            }
            Some((Global::Enum(_), _)) => {
                self.error(
                    Diagnostic::error(format!("`{}` is an enum type, not a value", id.name))
                        .at(id.span, "")
                        .note(format!("use one of its variants, e.g. `{}::<Variant>`", id.name)),
                );
                Value::Poison
            }
            Some((Global::Module(_) | Global::Alias(_), _)) => {
                self.error(
                    Diagnostic::error(format!("`{}` is a module, not a value", id.name))
                        .at(id.span, "")
                        .note(format!("instantiate it with `{}(...)`", id.name)),
                );
                Value::Poison
            }
            None => {
                let mut diag = Diagnostic::error(format!("cannot find `{}` in this scope", id.name))
                    .at(id.span, "not found");
                if id.name.starts_with('u') && id.name[1..].bytes().all(|b| b.is_ascii_digit()) {
                    diag = diag.note("types are not values; to build a constant, write e.g. `5u8` or `5 as u8`");
                } else if BUILTINS.contains(&id.name.as_str()) {
                    diag = diag.note(format!("`{}` is a function; call it with `{}(...)`", id.name, id.name));
                }
                self.error(diag);
                Value::Poison
            }
        }
    }

    fn global_const(&mut self, decl: &'a ConstDecl, use_span: Span) -> Value {
        match self.consts.get(&decl.name.name) {
            Some(ConstState::Done(v)) => return v.clone(),
            Some(ConstState::InProgress) => {
                self.error(
                    Diagnostic::error(format!("constant `{}` depends on itself", decl.name.name))
                        .at(use_span, "cyclic reference")
                        .with(decl.name.span, "constant defined here"),
                );
                return Value::Poison;
            }
            None => {}
        }
        self.consts.insert(decl.name.name.clone(), ConstState::InProgress);
        let mut cx = Ctx::global(decl.name.span);
        let ty = decl.ty.as_ref().and_then(|t| self.resolve_ty(&mut cx, t));
        let mut value = self.eval(&mut cx, &decl.value, ty.as_ref());
        if let Some(ty) = &ty {
            value = self.coerce(value, ty, decl.value.span);
        }
        value = match value {
            v @ (Value::Int(_) | Value::Const(..) | Value::Poison) => v,
            Value::Array(..) if ty.is_some() => value,
            _ => {
                self.error(Diagnostic::error("constants must be compile-time values").at(decl.value.span, ""));
                Value::Poison
            }
        };
        self.consts.insert(decl.name.name.clone(), ConstState::Done(value.clone()));
        value
    }

    // ── types ────────────────────────────────────────────────────────────

    fn resolve_ty(&mut self, cx: &mut Ctx, te: &'a TypeExpr) -> Option<Ty> {
        match te {
            TypeExpr::UInt { width, span } => {
                let w = self.expect_int(cx, width)?;
                if w < 1 || w > MAX_WIDTH as i128 {
                    self.error(
                        Diagnostic::error(format!("unsupported width {w}"))
                            .at(*span, format!("widths must be between 1 and {MAX_WIDTH}")),
                    );
                    return None;
                }
                Some(Ty::UInt(w as u32))
            }
            TypeExpr::Named(id, args) => match self.globals.get(&id.name) {
                Some((Global::Enum(e), _)) => {
                    let e = *e;
                    if args.is_some() {
                        self.error(
                            Diagnostic::error(format!("enum `{}` takes no parameters", id.name)).at(id.span, ""),
                        );
                    }
                    Some(Ty::Enum(e))
                }
                Some((Global::Module(m), _)) if is_bundle(m) => {
                    let m = *m;
                    let params = match args {
                        Some(a) => self.generic_args(cx, a),
                        None => Vec::new(),
                    };
                    self.bundle_type(m, params, id.span)
                }
                Some((Global::Module(_) | Global::Alias(_), _)) => {
                    // A module used as a type is its interface (its signature).
                    let (decl, params) = self.module_ref(cx, id, args.as_deref())?;
                    let module = self.instantiate(decl, params, id.span)?;
                    Some(Ty::Bundle(self.signature(module)))
                }
                _ => {
                    self.error(
                        Diagnostic::error(format!("unknown type `{}`", id.name))
                            .at(id.span, "not found")
                            .note("types are `uN`, `u<expr>`, `[T; N]`, an enum, or a bundle (a module without a body)"),
                    );
                    None
                }
            },
            TypeExpr::Array { elem, len, span } => {
                let elem = self.resolve_ty(cx, elem)?;
                let n = self.expect_int(cx, len)?;
                if n < 1 || n * elem.flat_len(&self.types) as i128 > MAX_FLAT_LEN as i128 {
                    self.error(
                        Diagnostic::error(format!("unsupported array length {n}"))
                            .at(*span, format!("arrays hold between 1 and {MAX_FLAT_LEN} elements")),
                    );
                    return None;
                }
                Some(Ty::Array(Box::new(elem), n as u32))
            }
        }
    }

    /// Evaluates a compile-time integer (unsized literal, parameter or sized constant).
    fn expect_int(&mut self, cx: &mut Ctx, e: &'a Expr) -> Option<i128> {
        match self.eval(cx, e, None) {
            Value::Int(v) => Some(v),
            Value::Const(v, Ty::UInt(_)) => Some(v as i128),
            Value::Poison => None,
            other => {
                let desc = match other {
                    Value::Hw(..) => "a hardware signal".to_string(),
                    other => self.value_desc(&other),
                };
                self.error(Diagnostic::error("expected a compile-time integer").at(e.span, format!("found {desc}")));
                None
            }
        }
    }

    // ── values ───────────────────────────────────────────────────────────

    fn coerce(&mut self, v: Value, ty: &Ty, span: Span) -> Value {
        match (&v, ty) {
            (Value::Poison, _) => Value::Poison,
            (Value::Int(i), Ty::UInt(w)) => {
                if fits(*i, *w) {
                    Value::Const(*i as u64, ty.clone())
                } else {
                    let why = if *i < 0 {
                        "negative values are not representable".to_string()
                    } else {
                        format!("the largest `u{w}` is {}", max_value(*w))
                    };
                    let mut diag = Diagnostic::error(format!("literal `{i}` does not fit in `u{w}`")).at(span, why);
                    if *i < 0 {
                        diag = diag.note(format!("write all ones as `!0u{w}`"));
                    }
                    self.error(diag);
                    Value::Poison
                }
            }
            (Value::Int(_), Ty::Enum(e)) => {
                let def = &self.types.enums[*e];
                let note = format!(
                    "use a variant such as `{}::{}`, or convert bits with `as {}`",
                    def.name,
                    def.variants.first().map_or("A", |v| v.0.as_str()),
                    def.name
                );
                let name = def.name.clone();
                self.error(
                    Diagnostic::error(format!("expected `{name}`, found an integer"))
                        .at(span, "")
                        .note(note),
                );
                Value::Poison
            }
            (Value::Array(elems, _), Ty::Array(elem_ty, n)) if elems.len() as u32 == *n => {
                let elems = elems.clone();
                Value::Array(
                    elems.into_iter().map(|e| self.coerce(e, elem_ty, span)).collect(),
                    (**elem_ty).clone(),
                )
            }
            _ => {
                if ty_of(&v).as_ref() == Some(ty) {
                    return v;
                }
                self.mismatch(ty, &v, span);
                Value::Poison
            }
        }
    }

    fn mismatch(&mut self, expected: &Ty, found: &Value, span: Span) {
        let mut diag = Diagnostic::error(format!(
            "mismatched types: expected `{}`, found {}",
            self.ty_name(expected),
            self.value_desc(found)
        ))
        .at(span, format!("expected `{}`", self.ty_name(expected)));
        if let (Ty::UInt(want), Some(Ty::UInt(have))) = (expected, ty_of(found)) {
            if have < *want {
                diag = diag.note(format!("zero-extend it with `as u{want}`"));
            } else {
                diag = diag.note(format!("truncate it with a bit range, e.g. `x[0..{want}]`"));
            }
        }
        self.error(diag);
    }

    fn node_of(&mut self, cx: &mut Ctx, v: &Value) -> NodeId {
        match v {
            Value::Const(c, t) => {
                let w = self.width(t);
                cx.ir.add(Node::Const(*c), w)
            }
            Value::Hw(n, _) => *n,
            other => unreachable!("node_of called on {other:?}"),
        }
    }

    fn flatten(&mut self, cx: &mut Ctx, v: &Value, out: &mut Vec<NodeId>) {
        match v {
            Value::Array(elems, _) | Value::Bundle(elems, _) => {
                for e in elems {
                    self.flatten(cx, e, out);
                }
            }
            scalar => {
                let n = self.node_of(cx, scalar);
                out.push(n);
            }
        }
    }

    fn mux_values(&mut self, cx: &mut Ctx, sel: NodeId, a: Value, b: Value) -> Value {
        match (a, b) {
            (Value::Poison, _) | (_, Value::Poison) => Value::Poison,
            (Value::Array(xs, t), Value::Array(ys, _)) => Value::Array(
                xs.into_iter().zip(ys).map(|(x, y)| self.mux_values(cx, sel, x, y)).collect(),
                t,
            ),
            (Value::Bundle(xs, t), Value::Bundle(ys, _)) => Value::Bundle(
                xs.into_iter().zip(ys).map(|(x, y)| self.mux_values(cx, sel, x, y)).collect(),
                t,
            ),
            (Value::Const(x, t), Value::Const(y, _)) if x == y => Value::Const(x, t),
            (a, b) => {
                let ty = ty_of(&a).unwrap();
                let na = self.node_of(cx, &a);
                let nb = self.node_of(cx, &b);
                Value::Hw(cx.ir.mux(sel, na, nb), ty)
            }
        }
    }

    fn cond_and(&mut self, cx: &mut Ctx, a: Option<NodeId>, b: NodeId) -> NodeId {
        match a {
            None => b,
            Some(a) => cx.ir.add(Node::Bin(BinKind::And, a, b), 1),
        }
    }

    fn cond_or(&mut self, cx: &mut Ctx, a: Cond, b: Cond) -> Cond {
        match (a, b) {
            (Cond::Static(true), _) | (_, Cond::Static(true)) => Cond::Static(true),
            (Cond::Static(false), c) | (c, Cond::Static(false)) => c,
            (Cond::Dyn(x), Cond::Dyn(y)) => Cond::Dyn(cx.ir.add(Node::Bin(BinKind::Or, x, y), 1)),
        }
    }

    /// Compares a scalar value with a constant, producing a condition.
    fn eq_const(&mut self, cx: &mut Ctx, v: &Value, c: u64) -> Cond {
        match v {
            Value::Const(x, _) => Cond::Static(*x == c),
            Value::Hw(n, t) => {
                let w = self.width(t);
                let k = cx.ir.add(Node::Const(c), w);
                Cond::Dyn(cx.ir.add(Node::Bin(BinKind::Eq, *n, k), 1))
            }
            _ => Cond::Static(false),
        }
    }

    // ── statements ───────────────────────────────────────────────────────

    fn stmts(&mut self, cx: &mut Ctx, stmts: &'a [Stmt]) {
        for s in stmts {
            self.stmt(cx, s);
        }
    }

    fn block(&mut self, cx: &mut Ctx, block: &'a Block) {
        self.with_scope(cx, |this, cx| this.stmts(cx, &block.stmts));
    }

    fn stmt(&mut self, cx: &mut Ctx, s: &'a Stmt) {
        match s {
            Stmt::Let {
                name,
                ty,
                value: Some(value),
                ..
            } => {
                if ty.is_none()
                    && let ExprKind::Call { callee, generics, args } = &value.kind
                    && is_module_call(&self.globals, value)
                {
                    match self.instance(cx, callee, generics.as_deref(), args, value.span, &name.name) {
                        Some((ty, base)) => self.bind(cx, name, BindItem::Place(ty, base, PlaceKind::Instance)),
                        None => self.bind(cx, name, BindItem::Val(Value::Poison, ValKind::Let)),
                    }
                    return;
                }
                let resolved = ty.as_ref().map(|t| self.resolve_ty(cx, t));
                let mut v = self.eval(cx, value, resolved.clone().flatten().as_ref());
                match &resolved {
                    Some(Some(ty)) => v = self.coerce(v, ty, value.span),
                    Some(None) => v = Value::Poison,
                    None => {}
                }
                self.name_value(cx, &v, &name.name);
                self.bind(cx, name, BindItem::Val(v, ValKind::Let));
            }
            Stmt::Let {
                name,
                ty,
                value: None,
                reset,
                span,
            } => {
                if cx.cond_depth > 0 {
                    self.error(
                        Diagnostic::error("nets cannot be declared inside a conditional block")
                            .at(*span, "")
                            .note("declare it before the `if`/`match` and drive it inside"),
                    );
                    return;
                }
                // `let s: Module;` creates an instance with its inputs still unconnected.
                if let Some(TypeExpr::Named(id, generics)) = ty
                    && matches!(
                        self.globals.get(&id.name),
                        Some((Global::Alias(_), _)) | Some((Global::Module(_), _))
                    )
                    && !matches!(self.globals.get(&id.name), Some((Global::Module(m), _)) if is_bundle(m))
                {
                    if let Some(reset) = reset {
                        self.error(
                            Diagnostic::error("an instance cannot have a reset value")
                                .at(reset.span, "")
                                .note("reset values belong to the registers inside the module"),
                        );
                    }
                    match self.instance(cx, id, generics.as_deref(), &[], *span, &name.name) {
                        Some((ty, base)) => self.bind(cx, name, BindItem::Place(ty, base, PlaceKind::Instance)),
                        None => self.bind(cx, name, BindItem::Val(Value::Poison, ValKind::Let)),
                    }
                    return;
                }
                let Some(ty) = ty.as_ref().and_then(|t| self.resolve_ty(cx, t)) else {
                    self.bind(cx, name, BindItem::Val(Value::Poison, ValKind::Let));
                    return;
                };
                let base = cx.slots.len() as u32;
                for leaf in ty.leaves(&self.types) {
                    let path = format!("{}{}", name.name, leaf.path);
                    let width = self.width(&leaf.ty);
                    let t = self.new_target(cx, &path, width, name.span, false);
                    cx.slots.push(Slot::Target(t));
                    cx.slot_names.push(path);
                }
                if let Some(reset) = reset {
                    self.apply_reset(cx, base, &ty, reset, name);
                }
                self.bind(cx, name, BindItem::Place(ty, base, PlaceKind::Net));
            }
            Stmt::Assign {
                target,
                op,
                op_span,
                value,
            } => self.assign(cx, target, *op, *op_span, value),
            Stmt::If { cond, then, els, .. } => {
                let c = self.condition(cx, cond);
                match c {
                    None => {}
                    Some(Cond::Static(true)) => self.block(cx, then),
                    Some(Cond::Static(false)) => match els {
                        Some(Else::Block(b)) => self.block(cx, b),
                        Some(Else::If(s)) => self.stmt(cx, s),
                        None => {}
                    },
                    Some(Cond::Dyn(c)) => self.branch(
                        cx,
                        c,
                        &mut |this, cx| this.block(cx, then),
                        &mut |this, cx| match els {
                            Some(Else::Block(b)) => this.block(cx, b),
                            Some(Else::If(s)) => this.stmt(cx, s),
                            None => {}
                        },
                    ),
                }
            }
            Stmt::Match { scrutinee, arms, span } => {
                let sv = self.eval(cx, scrutinee, None);
                let pats: Vec<&[Pattern]> = arms.iter().map(|a| a.pats.as_slice()).collect();
                let Some((conds, exhaustive, _)) =
                    self.match_conditions(cx, &sv, &pats, scrutinee.span, *span, false)
                else {
                    return;
                };
                self.match_chain(cx, arms, &conds, 0, exhaustive);
            }
            Stmt::For {
                var,
                start,
                end,
                inclusive,
                body,
                ..
            } => {
                let (Some(lo), Some(hi)) = (self.expect_int(cx, start), self.expect_int(cx, end)) else {
                    return;
                };
                let hi = if *inclusive { hi + 1 } else { hi };
                cx.loop_iterations += (hi - lo).max(0);
                if cx.loop_iterations > MAX_LOOP_ITERATIONS {
                    self.error(Diagnostic::error("too many loop iterations").at(
                        start.span.to(end.span),
                        format!("loops may run at most {MAX_LOOP_ITERATIONS} iterations per module"),
                    ));
                    return;
                }
                for i in lo..hi {
                    cx.scopes.push(HashMap::new());
                    self.bind(cx, var, BindItem::Val(Value::Int(i), ValKind::LoopVar));
                    self.block(cx, body);
                    self.pop_scope(cx);
                }
            }
        }
    }

    fn reset_value(&mut self, cx: &mut Ctx, e: &'a Expr, scalar: &Ty) -> u64 {
        match self.eval(cx, e, Some(scalar)) {
            v @ Value::Int(_) => match self.coerce(v, scalar, e.span) {
                Value::Const(c, _) => c,
                _ => 0,
            },
            Value::Const(c, t) => {
                if t != *scalar {
                    self.mismatch(scalar, &Value::Const(c, t), e.span);
                }
                c
            }
            Value::Poison => 0,
            other => {
                let desc = match other {
                    Value::Hw(..) => "a hardware signal".to_string(),
                    other => self.value_desc(&other),
                };
                self.error(
                    Diagnostic::error("reset values must be compile-time constants")
                        .at(e.span, format!("found {desc}"))
                        .note("arrays take one value that every element resets to"),
                );
                0
            }
        }
    }

    fn name_value(&mut self, cx: &mut Ctx, v: &Value, name: &str) {
        match v {
            Value::Hw(n, _) => cx.ir.name(*n, name),
            Value::Array(elems, _) => {
                for (i, e) in elems.iter().enumerate() {
                    self.name_value(cx, e, &format!("{name}[{i}]"));
                }
            }
            Value::Bundle(elems, b) => {
                let names: Vec<String> = self.types.bundles[*b].fields.iter().map(|f| f.name.clone()).collect();
                for (e, f) in elems.iter().zip(names) {
                    self.name_value(cx, e, &format!("{name}.{f}"));
                }
            }
            _ => {}
        }
    }

    /// Evaluates an `if` condition: a `u1` signal or a compile-time value.
    fn condition(&mut self, cx: &mut Ctx, e: &'a Expr) -> Option<Cond> {
        let v = self.eval(cx, e, Some(&Ty::UInt(1)));
        match v {
            Value::Poison => None,
            Value::Int(i) => Some(Cond::Static(i != 0)),
            Value::Const(c, Ty::UInt(1)) => Some(Cond::Static(c != 0)),
            Value::Hw(n, Ty::UInt(1)) => Some(Cond::Dyn(n)),
            other => {
                let mut diag =
                    Diagnostic::error(format!("expected a `u1` condition, found {}", self.value_desc(&other)))
                        .at(e.span, "");
                if let Some(Ty::UInt(_)) = ty_of(&other) {
                    diag = diag.note("compare explicitly, e.g. `x != 0`");
                }
                self.error(diag);
                None
            }
        }
    }

    /// Procedural value of a target; an unassigned register holds its value.
    fn current(&self, cx: &mut Ctx, t: u32) -> Drv {
        let drv = cx.drivers[t as usize].clone();
        if drv.node.is_none() && cx.nets[t as usize].kind == Some(TargetKind::Reg) {
            let w = cx.ir.targets[t as usize].width;
            return Drv {
                node: Some(cx.ir.add(Node::Net(t), w)),
                total: true,
            };
        }
        drv
    }

    /// Runs `then` and `els` as the two sides of a hardware condition and merges
    /// the assignments they made.
    fn branch(
        &mut self,
        cx: &mut Ctx,
        cond: NodeId,
        then: &mut dyn FnMut(&mut Self, &mut Ctx),
        els: &mut dyn FnMut(&mut Self, &mut Ctx),
    ) {
        let snapshot = cx.drivers.clone();
        cx.cond_depth += 1;
        then(self, cx);
        let then_drivers = std::mem::replace(&mut cx.drivers, snapshot);
        els(self, cx);
        cx.cond_depth -= 1;
        for (t, then_drv) in then_drivers.into_iter().enumerate() {
            if then_drv == cx.drivers[t] {
                continue;
            }
            // Kinds may have been fixed inside a branch: resolve register holds now.
            let else_drv = self.current(cx, t as u32);
            cx.drivers[t] = then_drv;
            let then_drv = self.current(cx, t as u32);
            cx.drivers[t] = Self::merge(cx, cond, then_drv, else_drv);
        }
    }

    fn merge(cx: &mut Ctx, cond: NodeId, a: Drv, b: Drv) -> Drv {
        match (a.node, b.node) {
            (Some(x), Some(y)) => Drv {
                node: Some(cx.ir.mux(cond, x, y)),
                total: a.total && b.total,
            },
            (Some(x), None) | (None, Some(x)) => Drv {
                node: Some(x),
                total: false,
            },
            (None, None) => Drv {
                node: None,
                total: false,
            },
        }
    }

    /// Drives one slot with `node`, under an optional condition. Returns `false` if an
    /// error was reported.
    #[allow(clippy::too_many_arguments)]
    fn drive_slot(
        &mut self,
        cx: &mut Ctx,
        slot: u32,
        node: NodeId,
        op: AssignOp,
        cond: Option<NodeId>,
        op_span: Span,
        target_span: Span,
        connecting: bool,
    ) -> bool {
        let name = cx.slot_names[slot as usize].clone();
        let read_only = match cx.slots[slot as usize] {
            Slot::Input(_) => Some("it is an input of this module"),
            Slot::InstOut(_) => Some("it is an output of the instance"),
            Slot::Target(_) => None,
        };
        if let Some(why) = read_only {
            let mut diag = Diagnostic::error(format!("cannot drive `{name}`: {why}")).at(target_span, "");
            if connecting {
                diag = diag.note("`a = b` feeds `a` from `b` (and `b` from `a` for flipped fields); swap the sides?");
            }
            self.error(diag);
            return false;
        }
        let Slot::Target(t) = cx.slots[slot as usize] else {
            unreachable!()
        };
        let kind = match op {
            AssignOp::Comb => TargetKind::Wire,
            AssignOp::Reg => TargetKind::Reg,
        };
        let info = cx.nets[t as usize].clone();
        if info.instance_input && kind == TargetKind::Reg {
            self.error(
                Diagnostic::error(format!("instance input `{name}` is connected with `=`"))
                    .at(op_span, "expected `=`")
                    .note("to register the value first, drive a net with `<=` and connect that"),
            );
            return false;
        }
        match info.kind {
            None => {
                cx.nets[t as usize].kind = Some(kind);
                cx.nets[t as usize].kind_span = Some(op_span);
            }
            Some(k) if k != kind => {
                let (now, before, what) = match kind {
                    TargetKind::Reg => ("<=", "=", "a wire"),
                    TargetKind::Wire => ("=", "<=", "a register"),
                };
                let base = name.split(['[', '.']).next().unwrap_or(&name).to_string();
                let mut diag = Diagnostic::error(format!("`{base}` is driven with `{now}` here, but it is {what}"))
                    .at(op_span, format!("expected `{before}`"));
                if let Some(first) = info.kind_span {
                    diag = diag.with(first, format!("first driven with `{before}` here"));
                }
                diag = diag.note("a net is a register if it is driven with `<=` and a wire if it is driven with `=`; it cannot be both");
                self.error(diag);
                return false;
            }
            Some(_) => {}
        }
        let new = Drv {
            node: Some(node),
            total: true,
        };
        cx.drivers[t as usize] = match cond {
            None => new,
            Some(c) => {
                let cur = self.current(cx, t);
                Self::merge(cx, c, new, cur)
            }
        };
        true
    }

    fn poison_place(&mut self, cx: &mut Ctx, place: &Place) {
        let len = place.ty.flat_len(&self.types);
        for &(base, _) in &place.entries {
            for slot in base..base + len {
                if let Slot::Target(t) = cx.slots[slot as usize] {
                    let width = cx.ir.targets[t as usize].width;
                    let zero = cx.ir.add(Node::Const(0), width);
                    cx.drivers[t as usize] = Drv {
                        node: Some(zero),
                        total: true,
                    };
                }
            }
        }
    }

    /// Resolves a place expression (`x`, `x.f`, `x[i]`). `Ok(None)` means the
    /// expression is not a place; `Err` means an error was reported.
    fn place(&mut self, cx: &mut Ctx, e: &'a Expr, mark: bool) -> Result<Option<Place>, ()> {
        match &e.kind {
            ExprKind::Name(id) => match self.find_binding(cx, &id.name) {
                Some(Binding {
                    item: BindItem::Place(ty, base, _),
                    ..
                }) => {
                    if mark {
                        Self::mark_used(cx, &id.name);
                    }
                    Ok(Some(Place {
                        ty,
                        entries: vec![(base, None)],
                    }))
                }
                _ => Ok(None),
            },
            ExprKind::Field(base, field) => {
                let Some(p) = self.place(cx, base, mark)? else {
                    return Ok(None);
                };
                let Ty::Bundle(b) = p.ty else {
                    let desc = self.ty_name(&p.ty);
                    self.error(Diagnostic::error(format!("`{desc}` has no fields")).at(field.span, ""));
                    return Err(());
                };
                let def = self.types.bundles[b].clone();
                let mut offset = 0;
                for f in &def.fields {
                    if f.name == field.name {
                        return Ok(Some(Place {
                            ty: f.ty.clone(),
                            entries: p.entries.iter().map(|(s, c)| (s + offset, *c)).collect(),
                        }));
                    }
                    offset += f.ty.flat_len(&self.types);
                }
                self.unknown_field(&def, field);
                Err(())
            }
            ExprKind::Index(base, idx_e) => {
                let Some(p) = self.place(cx, base, mark)? else {
                    return Ok(None);
                };
                let Ty::Array(elem, n) = &p.ty else {
                    let desc = self.ty_name(&p.ty);
                    self.error(
                        Diagnostic::error(format!("cannot index into `{desc}` in an assignment"))
                            .at(idx_e.span, "")
                            .note("bits of a signal cannot be assigned one by one; assign the whole value, e.g. with `cat(...)`"),
                    );
                    return Err(());
                };
                let stride = elem.flat_len(&self.types);
                let iv = self.eval(cx, idx_e, None);
                let mut entries = Vec::new();
                match &iv {
                    Value::Poison => return Err(()),
                    Value::Int(_) | Value::Const(_, Ty::UInt(_)) => {
                        let k = match iv {
                            Value::Int(k) => k,
                            Value::Const(k, _) => k as i128,
                            _ => unreachable!(),
                        };
                        if k < 0 || k >= *n as i128 {
                            self.error(
                                Diagnostic::error(format!("index {k} is out of bounds"))
                                    .at(idx_e.span, format!("the array has {n} elements")),
                            );
                            return Err(());
                        }
                        for (b, c) in &p.entries {
                            entries.push((b + k as u32 * stride, *c));
                        }
                    }
                    Value::Hw(node, Ty::UInt(w)) => {
                        for (b, c) in &p.entries {
                            for k in 0..*n {
                                if (k as u128) > max_value(*w) {
                                    break;
                                }
                                let kn = cx.ir.add(Node::Const(k as u64), *w);
                                let eq = cx.ir.add(Node::Bin(BinKind::Eq, *node, kn), 1);
                                let cc = self.cond_and(cx, *c, eq);
                                entries.push((b + k * stride, Some(cc)));
                            }
                        }
                    }
                    other => {
                        let desc = self.value_desc(other);
                        self.error(
                            Diagnostic::error("array index must be an unsigned integer")
                                .at(idx_e.span, format!("found {desc}")),
                        );
                        return Err(());
                    }
                }
                Ok(Some(Place {
                    ty: (**elem).clone(),
                    entries,
                }))
            }
            _ => Ok(None),
        }
    }

    fn unknown_field(&mut self, def: &BundleDef, field: &Ident) {
        let names: Vec<&str> = def.fields.iter().map(|f| f.name.as_str()).collect();
        self.error(
            Diagnostic::error(format!("`{}` has no field `{}`", def.display, field.name))
                .at(field.span, "")
                .note(format!("fields: {}", names.join(", "))),
        );
    }

    fn assign(&mut self, cx: &mut Ctx, target: &'a Expr, op: AssignOp, op_span: Span, value: &'a Expr) {
        let place = match self.place(cx, target, false) {
            Err(()) => {
                self.eval(cx, value, None);
                return;
            }
            Ok(Some(p)) => p,
            Ok(None) => {
                self.not_assignable(cx, target);
                return;
            }
        };
        if !self.assign_place(cx, &place, target.span, op, op_span, value) {
            // An error was reported: treat the place as driven so that it is not also
            // reported as undriven.
            self.poison_place(cx, &place);
        }
    }

    fn not_assignable(&mut self, cx: &mut Ctx, target: &Expr) {
        let diag = match &target.kind {
            ExprKind::Name(id) => match self.find_binding(cx, &id.name) {
                Some(Binding {
                    item: BindItem::Val(_, kind),
                    span,
                    ..
                }) => {
                    let what = match kind {
                        ValKind::Let => "`let x = ...` bindings are immutable values",
                        ValKind::Param => "it is a compile-time parameter",
                        ValKind::LoopVar => "it is a loop variable",
                    };
                    let mut diag = Diagnostic::error(format!("cannot assign to `{}`", id.name))
                        .at(id.span, what)
                        .with(span, "defined here");
                    if kind == ValKind::Let {
                        diag = diag.note("declare a net with `let x: T;` to drive it with `=` or `<=`");
                    }
                    diag
                }
                _ => match self.globals.get(&id.name) {
                    Some((Global::Const(_), _)) => {
                        Diagnostic::error(format!("cannot assign to constant `{}`", id.name)).at(id.span, "")
                    }
                    _ => Diagnostic::error(format!("cannot find `{}` in this scope", id.name)).at(id.span, "not found"),
                },
            },
            ExprKind::Slice { .. } => Diagnostic::error("cannot assign to a bit range")
                .at(target.span, "bit ranges are read-only")
                .note("assign the whole signal, e.g. `x = cat(hi, lo);`"),
            _ => Diagnostic::error("cannot assign to this expression")
                .at(target.span, "")
                .note("assignment targets are nets, outputs, instance inputs, their fields and array elements"),
        };
        self.error(diag);
    }

    fn assign_place(
        &mut self,
        cx: &mut Ctx,
        place: &Place,
        target_span: Span,
        op: AssignOp,
        op_span: Span,
        value: &'a Expr,
    ) -> bool {
        let leaves = place.ty.leaves(&self.types);
        if !leaves.iter().any(|l| l.flipped) {
            let v = self.eval(cx, value, Some(&place.ty));
            let v = self.coerce(v, &place.ty, value.span);
            if matches!(v, Value::Poison) {
                return false;
            }
            let mut nodes = Vec::new();
            self.flatten(cx, &v, &mut nodes);
            let mut ok = true;
            for &(base, cond) in &place.entries {
                for (j, &n) in nodes.iter().enumerate() {
                    ok &= self.drive_slot(cx, base + j as u32, n, op, cond, op_span, target_span, false);
                }
            }
            return ok;
        }

        // Bundles with fields in both directions: connect place to place.
        if op == AssignOp::Reg {
            let name = self.ty_name(&place.ty);
            self.error(
                Diagnostic::error(format!("`{name}` has flipped fields, so it cannot be driven with `<=`"))
                    .at(op_span, "")
                    .note("register the individual fields that flow forward instead"),
            );
            return false;
        }
        let rhs = match self.place(cx, value, true) {
            Err(()) => return false,
            Ok(Some(r)) => r,
            Ok(None) => {
                let name = self.ty_name(&place.ty);
                self.error(
                    Diagnostic::error(format!(
                        "`{name}` has fields in both directions, so it can only be connected to another signal"
                    ))
                    .at(value.span, "expected a net, port or instance port"),
                );
                return false;
            }
        };
        if rhs.ty != place.ty {
            let (a, b) = (self.ty_name(&place.ty), self.ty_name(&rhs.ty));
            self.error(Diagnostic::error(format!("mismatched types: expected `{a}`, found `{b}`")).at(value.span, ""));
            return false;
        }
        let (Some(&(lbase, None)), Some(&(rbase, None)), 1, 1) =
            (place.entries.first(), rhs.entries.first(), place.entries.len(), rhs.entries.len())
        else {
            self.error(
                Diagnostic::error("bundles with flipped fields cannot be connected through a hardware index")
                    .at(target_span.to(value.span), "")
                    .note("connect the fields individually, or use a compile-time index"),
            );
            return false;
        };
        let mut ok = true;
        for (j, leaf) in leaves.iter().enumerate() {
            let j = j as u32;
            let (dst, src, span) = if leaf.flipped {
                (rbase + j, lbase + j, value.span)
            } else {
                (lbase + j, rbase + j, target_span)
            };
            let n = self.read_slot(cx, src);
            ok &= self.drive_slot(cx, dst, n, AssignOp::Comb, None, op_span, span, true);
        }
        ok
    }

    fn match_chain(&mut self, cx: &mut Ctx, arms: &'a [MatchArm<Block>], conds: &[Cond], i: usize, exhaustive: bool) {
        if i >= arms.len() {
            return;
        }
        if exhaustive && i == arms.len() - 1 {
            if !matches!(conds[i], Cond::Static(false)) {
                self.block(cx, &arms[i].body);
            }
            return;
        }
        match conds[i] {
            Cond::Static(true) => self.block(cx, &arms[i].body),
            Cond::Static(false) => self.match_chain(cx, arms, conds, i + 1, exhaustive),
            Cond::Dyn(c) => self.branch(
                cx,
                c,
                &mut |this, cx| this.block(cx, &arms[i].body),
                &mut |this, cx| this.match_chain(cx, arms, conds, i + 1, exhaustive),
            ),
        }
    }

    /// Computes one condition per arm. Returns the conditions, whether the arms are
    /// exhaustive, and the scrutinee type.
    fn match_conditions(
        &mut self,
        cx: &mut Ctx,
        sv: &Value,
        arms: &[&'a [Pattern]],
        scrut_span: Span,
        match_span: Span,
        require_exhaustive: bool,
    ) -> Option<(Vec<Cond>, bool, Option<Ty>)> {
        let sty = match sv {
            Value::Poison => return None,
            Value::Int(_) => None,
            Value::Const(_, t) | Value::Hw(_, t) => Some(t.clone()),
            other => {
                let desc = self.value_desc(other);
                self.error(Diagnostic::error(format!("cannot match on {desc}")).at(scrut_span, "expected a scalar value"));
                return None;
            }
        };
        let mut covered: HashSet<i128> = HashSet::new();
        let mut has_wild = false;
        let mut conds = Vec::new();
        for pats in arms {
            if has_wild {
                let span = pats[0].span();
                self.diags.push(
                    Diagnostic::warning("unreachable match arm")
                        .at(span, "a previous `_` arm already matches every value"),
                );
            }
            let mut cond = Cond::Static(false);
            for pat in pats.iter() {
                let c = match pat {
                    Pattern::Wildcard(_) => {
                        has_wild = true;
                        Cond::Static(true)
                    }
                    Pattern::Value(e) => {
                        let pv = self.eval(cx, e, sty.as_ref());
                        let pv = match (&pv, &sty) {
                            (Value::Poison, _) => continue,
                            (Value::Int(_) | Value::Const(..), Some(t)) => self.coerce(pv, t, e.span),
                            (Value::Int(_), None) => pv,
                            (Value::Const(..), None) => {
                                self.error(
                                    Diagnostic::error("sized pattern on an unsized compile-time value").at(e.span, ""),
                                );
                                continue;
                            }
                            _ => {
                                self.error(
                                    Diagnostic::error("match patterns must be compile-time constants")
                                        .at(e.span, "not a constant"),
                                );
                                continue;
                            }
                        };
                        let key = match pv {
                            Value::Int(i) => i,
                            Value::Const(c, _) => c as i128,
                            _ => continue,
                        };
                        if !covered.insert(key) {
                            self.diags.push(
                                Diagnostic::warning("unreachable pattern")
                                    .at(e.span, "this value is already matched by an earlier arm"),
                            );
                        }
                        match sv {
                            Value::Int(s) => Cond::Static(*s == key),
                            _ => self.eq_const(cx, sv, key as u64),
                        }
                    }
                };
                cond = self.cond_or(cx, cond, c);
            }
            conds.push(cond);
        }

        let exhaustive = has_wild
            || match &sty {
                Some(Ty::Enum(e)) => self.types.enums[*e]
                    .variants
                    .iter()
                    .all(|(_, v)| covered.contains(&(*v as i128))),
                Some(Ty::UInt(w)) => *w <= 16 && covered.len() as u128 == 1u128 << *w,
                _ => false,
            };
        let statically_matched = conds.iter().any(|c| matches!(c, Cond::Static(true)));
        if require_exhaustive && !exhaustive && !statically_matched {
            let missing = match &sty {
                Some(Ty::Enum(e)) => {
                    let def = &self.types.enums[*e];
                    let names: Vec<String> = def
                        .variants
                        .iter()
                        .filter(|(_, v)| !covered.contains(&(*v as i128)))
                        .map(|(n, _)| format!("`{}::{n}`", def.name))
                        .collect();
                    let shown = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
                    if names.len() > 3 {
                        format!("{shown} and {} more", names.len() - 3)
                    } else {
                        shown
                    }
                }
                Some(Ty::UInt(w)) if *w <= 16 => {
                    let first = (0..(1i128 << *w)).find(|v| !covered.contains(v)).unwrap_or(0);
                    format!("`{first}`")
                }
                Some(t) => format!("most values of `{}`", self.ty_name(t)),
                None => "this value".to_string(),
            };
            self.error(
                Diagnostic::error(format!("non-exhaustive match: {missing} not covered"))
                    .at(match_span, "")
                    .note("add the missing arms, or a `_` arm for every other value"),
            );
            return None;
        }
        Some((conds, exhaustive, sty))
    }

    // ── expressions ──────────────────────────────────────────────────────

    fn eval(&mut self, cx: &mut Ctx, e: &'a Expr, expected: Option<&Ty>) -> Value {
        let outer = cx.ir.cur_span;
        cx.ir.cur_span = e.span;
        let v = self.eval_inner(cx, e, expected);
        cx.ir.cur_span = outer;
        v
    }

    fn eval_inner(&mut self, cx: &mut Ctx, e: &'a Expr, expected: Option<&Ty>) -> Value {
        match &e.kind {
            ExprKind::Int { value, width } => {
                let Ok(v) = i128::try_from(*value) else {
                    self.error(Diagnostic::error("integer literal is too large").at(e.span, ""));
                    return Value::Poison;
                };
                match width {
                    None => Value::Int(v),
                    Some(w) => {
                        if *w < 1 || *w > MAX_WIDTH {
                            self.error(
                                Diagnostic::error(format!("unsupported width {w}"))
                                    .at(e.span, format!("widths must be between 1 and {MAX_WIDTH}")),
                            );
                            return Value::Poison;
                        }
                        self.coerce(Value::Int(v), &Ty::UInt(*w), e.span)
                    }
                }
            }
            ExprKind::Bool(b) => Value::Const(*b as u64, Ty::UInt(1)),
            ExprKind::Name(id) => self.lookup(cx, id),
            ExprKind::Path(ty, variant) => match self.globals.get(&ty.name) {
                Some((Global::Enum(idx), _)) => {
                    let idx = *idx;
                    let def = &self.types.enums[idx];
                    match def.variants.iter().find(|(n, _)| *n == variant.name) {
                        Some((_, v)) => Value::Const(*v, Ty::Enum(idx)),
                        None => {
                            let names: Vec<&str> = def.variants.iter().map(|(n, _)| n.as_str()).collect();
                            let note = format!("variants of `{}`: {}", def.name, names.join(", "));
                            self.error(
                                Diagnostic::error(format!("enum `{}` has no variant `{}`", ty.name, variant.name))
                                    .at(variant.span, "")
                                    .note(note),
                            );
                            Value::Poison
                        }
                    }
                }
                _ => {
                    self.error(Diagnostic::error(format!("`{}` is not an enum", ty.name)).at(ty.span, ""));
                    Value::Poison
                }
            },
            ExprKind::Unary(op, inner) => {
                let v = self.eval(cx, inner, expected);
                self.unary(cx, *op, v, e.span)
            }
            ExprKind::Binary(op, l, r) => self.binary(cx, *op, l, r, expected, e.span),
            ExprKind::Cast(inner, te) => {
                let Some(ty) = self.resolve_ty(cx, te) else {
                    return Value::Poison;
                };
                let v = self.eval(cx, inner, None);
                self.cast(cx, v, &ty, inner.span, e.span)
            }
            ExprKind::Index(base, idx) => {
                let bv = self.eval(cx, base, None);
                self.index(cx, bv, idx, base.span)
            }
            ExprKind::Slice { base, lo, hi, inclusive } => {
                let bv = self.eval(cx, base, None);
                let (Some(lo_v), Some(hi_v)) = (self.expect_int(cx, lo), self.expect_int(cx, hi)) else {
                    return Value::Poison;
                };
                let hi_v = if *inclusive { hi_v + 1 } else { hi_v };
                self.slice(cx, bv, lo_v, hi_v, base.span, lo.span.to(hi.span))
            }
            ExprKind::Field(base, field) => {
                let bv = self.eval(cx, base, None);
                match bv {
                    Value::Poison => Value::Poison,
                    Value::Bundle(values, b) => {
                        let def = self.types.bundles[b].clone();
                        match def.fields.iter().position(|f| f.name == field.name) {
                            Some(i) => values[i].clone(),
                            None => {
                                self.unknown_field(&def, field);
                                Value::Poison
                            }
                        }
                    }
                    other => {
                        let desc = self.value_desc(&other);
                        self.error(Diagnostic::error(format!("{desc} has no fields")).at(base.span, ""));
                        Value::Poison
                    }
                }
            }
            ExprKind::Method { recv, name, args } => {
                let rv = self.eval(cx, recv, None);
                self.method(cx, rv, name, args, recv.span)
            }
            ExprKind::Call { callee, generics, args } => {
                if is_module_call(&self.globals, e) {
                    return match self.instance(cx, callee, generics.as_deref(), args, e.span, &callee.name) {
                        Some((ty, base)) => self.read_place(cx, &ty, base),
                        None => Value::Poison,
                    };
                }
                self.call(cx, callee, generics.as_deref(), args, e.span)
            }
            ExprKind::If { cond, then, els } => {
                let c = self.condition(cx, cond);
                match c {
                    None => Value::Poison,
                    Some(Cond::Static(true)) => self.expr_block(cx, then, expected),
                    Some(Cond::Static(false)) => self.eval(cx, els, expected),
                    Some(Cond::Dyn(c)) => {
                        let a = self.expr_block(cx, then, expected);
                        let b = self.eval(cx, els, expected);
                        let spans = [then.value.span, els.span];
                        let mut vals = self.unify(vec![a, b], &spans, expected, e.span);
                        let b = vals.pop().unwrap();
                        let a = vals.pop().unwrap();
                        self.mux_values(cx, c, a, b)
                    }
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                let sv = self.eval(cx, scrutinee, None);
                let pats: Vec<&[Pattern]> = arms.iter().map(|a| a.pats.as_slice()).collect();
                let Some((conds, exhaustive, _)) = self.match_conditions(cx, &sv, &pats, scrutinee.span, e.span, true)
                else {
                    return Value::Poison;
                };
                // Arms that can be taken, in priority order.
                let mut taken: Vec<(Option<NodeId>, &'a Expr)> = Vec::new();
                for (cond, arm) in conds.iter().zip(arms) {
                    match cond {
                        Cond::Static(false) => {}
                        Cond::Static(true) => {
                            taken.push((None, &arm.body));
                            break;
                        }
                        Cond::Dyn(c) => taken.push((Some(*c), &arm.body)),
                    }
                }
                if exhaustive && let Some(last) = taken.last_mut() {
                    last.0 = None;
                }
                let values: Vec<Value> = taken.iter().map(|(_, b)| self.eval(cx, b, expected)).collect();
                let spans: Vec<Span> = taken.iter().map(|(_, b)| b.span).collect();
                let mut values = self.unify(values, &spans, expected, e.span);
                let mut acc = values.pop().unwrap_or(Value::Poison);
                for ((cond, _), v) in taken.iter().zip(values).rev() {
                    if let Some(c) = cond {
                        acc = self.mux_values(cx, *c, v, acc);
                    }
                }
                acc
            }
            ExprKind::Block(block) => self.expr_block(cx, block, expected),
        }
    }

    fn expr_block(&mut self, cx: &mut Ctx, block: &'a ExprBlock, expected: Option<&Ty>) -> Value {
        let mut result = Value::Poison;
        self.with_scope(cx, |this, cx| {
            for (name, ty, value) in &block.lets {
                let ty = ty.as_ref().and_then(|t| this.resolve_ty(cx, t));
                let mut v = this.eval(cx, value, ty.as_ref());
                if let Some(ty) = &ty {
                    v = this.coerce(v, ty, value.span);
                }
                this.name_value(cx, &v, &name.name);
                this.bind(cx, name, BindItem::Val(v, ValKind::Let));
            }
            result = this.eval(cx, &block.value, expected);
        });
        result
    }

    /// Brings the values of alternative branches to a common type.
    fn unify(&mut self, values: Vec<Value>, spans: &[Span], expected: Option<&Ty>, span: Span) -> Vec<Value> {
        if values.iter().any(|v| matches!(v, Value::Poison)) {
            return values.into_iter().map(|_| Value::Poison).collect();
        }
        let ty = expected.cloned().or_else(|| values.iter().find_map(ty_of));
        match ty {
            Some(ty) => values
                .into_iter()
                .zip(spans)
                .map(|(v, s)| self.coerce(v, &ty, *s))
                .collect(),
            None => {
                self.error(
                    Diagnostic::error("cannot infer the width of this expression")
                        .at(span, "every branch is an unsized integer")
                        .note("give one branch a suffix like `1u8`, or annotate the binding: `let x: u8 = ...`"),
                );
                values.into_iter().map(|_| Value::Poison).collect()
            }
        }
    }

    fn unary(&mut self, cx: &mut Ctx, op: UnOp, v: Value, span: Span) -> Value {
        match (op, v) {
            (_, Value::Poison) => Value::Poison,
            (UnOp::Neg, Value::Int(i)) => Value::Int(-i),
            (UnOp::Not, Value::Int(_)) => {
                self.error(
                    Diagnostic::error("cannot apply `!` to an unsized integer")
                        .at(span, "the width is unknown")
                        .note("give it a width first, e.g. `!0u8`"),
                );
                Value::Poison
            }
            (op, Value::Const(c, Ty::UInt(w))) => {
                let r = match op {
                    UnOp::Not => mask(!c, w),
                    UnOp::Neg => mask(c.wrapping_neg(), w),
                };
                Value::Const(r, Ty::UInt(w))
            }
            (op, Value::Hw(n, Ty::UInt(w))) => {
                let node = match op {
                    UnOp::Not => cx.ir.add(Node::Not(n), w),
                    UnOp::Neg => {
                        let zero = cx.ir.add(Node::Const(0), w);
                        cx.ir.add(Node::Bin(BinKind::Sub, zero, n), w)
                    }
                };
                Value::Hw(node, Ty::UInt(w))
            }
            (op, other) => {
                let sym = if op == UnOp::Not { "!" } else { "-" };
                let desc = self.value_desc(&other);
                self.error(
                    Diagnostic::error(format!("cannot apply `{sym}` to {desc}"))
                        .at(span, "")
                        .note("operators work on unsigned integers; cast enums with `as uN`"),
                );
                Value::Poison
            }
        }
    }

    fn binary(&mut self, cx: &mut Ctx, op: BinOp, le: &'a Expr, re: &'a Expr, expected: Option<&Ty>, span: Span) -> Value {
        match op {
            BinOp::LogicAnd | BinOp::LogicOr => {
                let u1 = Ty::UInt(1);
                let l = self.eval(cx, le, Some(&u1));
                let r = self.eval(cx, re, Some(&u1));
                if let (Value::Int(a), Value::Int(b)) = (&l, &r) {
                    for (v, s) in [(*a, le.span), (*b, re.span)] {
                        if v != 0 && v != 1 {
                            self.error(Diagnostic::error(format!("`{}` needs 0 or 1, found {v}", op.symbol())).at(s, ""));
                            return Value::Poison;
                        }
                    }
                    let r = if op == BinOp::LogicAnd { a & b } else { a | b };
                    return Value::Int(r);
                }
                let l = self.coerce_bit(l, le.span, op);
                let r = self.coerce_bit(r, re.span, op);
                let kind = if op == BinOp::LogicAnd { BinKind::And } else { BinKind::Or };
                self.bin_values(cx, kind, l, r, Ty::UInt(1))
            }
            BinOp::Div | BinOp::Rem => {
                let l = self.eval(cx, le, None);
                let r = self.eval(cx, re, None);
                match (l, r) {
                    (Value::Poison, _) | (_, Value::Poison) => Value::Poison,
                    (Value::Int(a), Value::Int(b)) => {
                        if b == 0 {
                            self.error(Diagnostic::error("division by zero").at(re.span, ""));
                            return Value::Poison;
                        }
                        Value::Int(if op == BinOp::Div { a.div_euclid(b) } else { a.rem_euclid(b) })
                    }
                    _ => {
                        self.error(
                            Diagnostic::error(format!("`{}` is only available on compile-time integers", op.symbol()))
                                .at(span, "")
                                .note("there is no hardware divider; for powers of two, use shifts or bit ranges"),
                        );
                        Value::Poison
                    }
                }
            }
            BinOp::Shl | BinOp::Shr => {
                let l = self.eval(cx, le, expected);
                let r = self.eval(cx, re, None);
                self.shift(cx, op == BinOp::Shl, l, r, le.span, re.span, expected)
            }
            _ if op.is_comparison() => {
                let l = self.eval(cx, le, None);
                let lt = ty_of(&l);
                let r = self.eval(cx, re, lt.as_ref());
                let (l, r) = self.align(l, r, le.span, re.span, op, span);
                let kind = match op {
                    BinOp::Eq => BinKind::Eq,
                    BinOp::Ne => BinKind::Ne,
                    BinOp::Lt => BinKind::Lt,
                    BinOp::Le => BinKind::Le,
                    BinOp::Gt => BinKind::Gt,
                    _ => BinKind::Ge,
                };
                match (&l, &r) {
                    (Value::Poison, _) | (_, Value::Poison) => Value::Poison,
                    (Value::Int(a), Value::Int(b)) => {
                        let res = match kind {
                            BinKind::Eq => a == b,
                            BinKind::Ne => a != b,
                            BinKind::Lt => a < b,
                            BinKind::Le => a <= b,
                            BinKind::Gt => a > b,
                            _ => a >= b,
                        };
                        Value::Int(res as i128)
                    }
                    _ => {
                        let ty = ty_of(&l).unwrap();
                        let ordered = !matches!(kind, BinKind::Eq | BinKind::Ne);
                        if ordered && !matches!(ty, Ty::UInt(_)) || !ty.is_scalar() {
                            let name = self.ty_name(&ty);
                            self.error(
                                Diagnostic::error(format!("cannot compare `{name}` values with `{}`", op.symbol()))
                                    .at(span, "")
                                    .note("comparisons work on scalars; ordering comparisons on unsigned integers"),
                            );
                            return Value::Poison;
                        }
                        self.bin_values(cx, kind, l, r, Ty::UInt(1))
                    }
                }
            }
            _ => {
                let l = self.eval(cx, le, expected);
                let hint = ty_of(&l).or_else(|| expected.cloned());
                let r = self.eval(cx, re, hint.as_ref());
                let (l, r) = self.align(l, r, le.span, re.span, op, span);
                let kind = match op {
                    BinOp::Add => BinKind::Add,
                    BinOp::Sub => BinKind::Sub,
                    BinOp::Mul => BinKind::Mul,
                    BinOp::And => BinKind::And,
                    BinOp::Or => BinKind::Or,
                    _ => BinKind::Xor,
                };
                match (&l, &r) {
                    (Value::Poison, _) | (_, Value::Poison) => Value::Poison,
                    (Value::Int(a), Value::Int(b)) => {
                        let res = match kind {
                            BinKind::Add => a.checked_add(*b),
                            BinKind::Sub => a.checked_sub(*b),
                            BinKind::Mul => a.checked_mul(*b),
                            BinKind::And => Some(a & b),
                            BinKind::Or => Some(a | b),
                            _ => Some(a ^ b),
                        };
                        match res {
                            Some(v) => Value::Int(v),
                            None => {
                                self.error(Diagnostic::error("arithmetic overflow in constant").at(span, ""));
                                Value::Poison
                            }
                        }
                    }
                    _ => {
                        let ty = ty_of(&l).unwrap();
                        if !matches!(ty, Ty::UInt(_)) {
                            let name = self.ty_name(&ty);
                            self.error(
                                Diagnostic::error(format!("cannot apply `{}` to `{name}` values", op.symbol()))
                                    .at(span, "")
                                    .note("operators work on unsigned integers; cast enums with `as uN`"),
                            );
                            return Value::Poison;
                        }
                        self.bin_values(cx, kind, l, r, ty)
                    }
                }
            }
        }
    }

    fn coerce_bit(&mut self, v: Value, span: Span, op: BinOp) -> Value {
        match &v {
            Value::Int(_) => self.coerce(v, &Ty::UInt(1), span),
            Value::Poison | Value::Const(_, Ty::UInt(1)) | Value::Hw(_, Ty::UInt(1)) => v,
            other => {
                let desc = self.value_desc(other);
                self.error(
                    Diagnostic::error(format!("`{}` needs `u1` operands, found {desc}", op.symbol()))
                        .at(span, "")
                        .note("compare explicitly, e.g. `x != 0`"),
                );
                Value::Poison
            }
        }
    }

    /// Gives two operands the same type: unsized integers adopt the other side's type.
    fn align(&mut self, l: Value, r: Value, ls: Span, rs: Span, op: BinOp, span: Span) -> (Value, Value) {
        match (ty_of(&l), ty_of(&r)) {
            (None, None) => (l, r),
            (Some(t), None) => {
                let r = self.coerce(r, &t, rs);
                (l, r)
            }
            (None, Some(t)) => {
                let l = self.coerce(l, &t, ls);
                (l, r)
            }
            (Some(a), Some(b)) => {
                if a == b {
                    return (l, r);
                }
                let (an, bn) = (self.ty_name(&a), self.ty_name(&b));
                let mut diag = Diagnostic::error(format!("mismatched types: `{an}` {} `{bn}`", op.symbol()))
                    .at(span, "")
                    .with(ls, format!("this is `{an}`"))
                    .with(rs, format!("this is `{bn}`"));
                if let (Ty::UInt(x), Ty::UInt(y)) = (&a, &b) {
                    diag = diag.note(format!("zero-extend the narrower operand with `as u{}`", x.max(y)));
                }
                self.error(diag);
                (Value::Poison, Value::Poison)
            }
        }
    }

    fn bin_values(&mut self, cx: &mut Ctx, kind: BinKind, l: Value, r: Value, rty: Ty) -> Value {
        if let (Value::Const(a, t), Value::Const(b, _)) = (&l, &r) {
            let w = self.width(t);
            return Value::Const(fold_bin(kind, *a, *b, w), rty);
        }
        let a = self.node_of(cx, &l);
        let b = self.node_of(cx, &r);
        let w = if kind.is_comparison() { 1 } else { cx.ir.width(a) };
        Value::Hw(cx.ir.add(Node::Bin(kind, a, b), w), rty)
    }

    #[allow(clippy::too_many_arguments)]
    fn shift(&mut self, cx: &mut Ctx, left: bool, l: Value, r: Value, ls: Span, rs: Span, expected: Option<&Ty>) -> Value {
        let sym = if left { "<<" } else { ">>" };
        let l = match (&l, &r) {
            (Value::Poison, _) | (_, Value::Poison) => return Value::Poison,
            (Value::Int(a), Value::Int(b)) => {
                if *b < 0 || *b > 127 {
                    self.error(Diagnostic::error(format!("shift amount {b} is out of range")).at(rs, ""));
                    return Value::Poison;
                }
                let res = if left { a.checked_shl(*b as u32) } else { Some(a >> *b) };
                return match res {
                    Some(v) if !left || v >> *b == *a => Value::Int(v),
                    _ => {
                        self.error(Diagnostic::error("arithmetic overflow in constant").at(ls.to(rs), ""));
                        Value::Poison
                    }
                };
            }
            (Value::Int(_), _) => match expected {
                Some(t @ Ty::UInt(_)) => self.coerce(l, t, ls),
                _ => {
                    self.error(
                        Diagnostic::error(format!("cannot infer the width of the value shifted by `{sym}`"))
                            .at(ls, "unsized integer")
                            .note("give it a width, e.g. `1u8 << n`"),
                    );
                    return Value::Poison;
                }
            },
            _ => l,
        };
        let Some(Ty::UInt(w)) = ty_of(&l) else {
            let desc = self.value_desc(&l);
            self.error(Diagnostic::error(format!("cannot shift {desc}")).at(ls, ""));
            return Value::Poison;
        };
        let amount = match &r {
            Value::Int(k) => Some(*k),
            Value::Const(k, Ty::UInt(_)) => Some(*k as i128),
            Value::Hw(_, Ty::UInt(_)) => None,
            other => {
                let desc = self.value_desc(other);
                self.error(Diagnostic::error(format!("shift amount must be an unsigned integer, found {desc}")).at(rs, ""));
                return Value::Poison;
            }
        };
        let ty = Ty::UInt(w);
        match amount {
            Some(k) if k < 0 => {
                self.error(Diagnostic::error(format!("negative shift amount {k}")).at(rs, ""));
                Value::Poison
            }
            Some(k) => {
                let k = k.min(64) as u32;
                if let Value::Const(c, _) = l {
                    let kind = if left { BinKind::Shl } else { BinKind::Shr };
                    return Value::Const(fold_bin(kind, c, k as u64, w), ty);
                }
                let n = self.node_of(cx, &l);
                if k == 0 {
                    return l;
                }
                if k >= w {
                    return Value::Const(0, ty);
                }
                let node = if left {
                    let lo = cx.ir.add(Node::Slice(n, 0), w - k);
                    let zeros = cx.ir.add(Node::Const(0), k);
                    cx.ir.add(Node::Cat(lo, zeros), w)
                } else {
                    let hi = cx.ir.add(Node::Slice(n, k), w - k);
                    cx.ir.add(Node::Zext(hi), w)
                };
                Value::Hw(node, ty)
            }
            None => {
                let kind = if left { BinKind::Shl } else { BinKind::Shr };
                let a = self.node_of(cx, &l);
                let b = self.node_of(cx, &r);
                Value::Hw(cx.ir.add(Node::Bin(kind, a, b), w), ty)
            }
        }
    }

    fn cast(&mut self, cx: &mut Ctx, v: Value, ty: &Ty, inner_span: Span, span: Span) -> Value {
        if !ty.is_scalar() {
            self.error(Diagnostic::error("can only cast to a scalar type").at(span, ""));
            return Value::Poison;
        }
        let from = match &v {
            Value::Poison => return Value::Poison,
            Value::Int(_) => return self.coerce(v, ty, inner_span),
            Value::Const(_, t) | Value::Hw(_, t) => t.clone(),
            other => {
                let desc = self.value_desc(other);
                self.error(Diagnostic::error(format!("cannot cast {desc}")).at(inner_span, ""));
                return Value::Poison;
            }
        };
        if let (Ty::Enum(a), Ty::Enum(b)) = (&from, ty)
            && a != b
        {
            let (an, bn) = (self.ty_name(&from), self.ty_name(ty));
            self.error(
                Diagnostic::error(format!("cannot cast `{an}` to `{bn}`"))
                    .at(span, "")
                    .note(format!("go through bits explicitly: `x as u{} as {bn}`", self.width(&from))),
            );
            return Value::Poison;
        }
        let (fw, tw) = (self.width(&from), self.width(ty));
        if tw < fw {
            self.error(
                Diagnostic::error(format!(
                    "cannot narrow `{}` to `{}` with `as`",
                    self.ty_name(&from),
                    self.ty_name(ty)
                ))
                .at(span, "`as` only zero-extends")
                .note(format!("take the low bits explicitly with a bit range: `x[0..{tw}]`")),
            );
            return Value::Poison;
        }
        match v {
            Value::Const(c, _) => Value::Const(c, ty.clone()),
            Value::Hw(n, _) => {
                let node = if tw == fw { n } else { cx.ir.add(Node::Zext(n), tw) };
                Value::Hw(node, ty.clone())
            }
            _ => unreachable!(),
        }
    }

    fn index(&mut self, cx: &mut Ctx, base: Value, idx_e: &'a Expr, base_span: Span) -> Value {
        match base {
            Value::Poison => Value::Poison,
            Value::Array(elems, _) => {
                let iv = self.eval(cx, idx_e, None);
                let n = elems.len();
                match iv {
                    Value::Poison => Value::Poison,
                    Value::Int(_) | Value::Const(_, Ty::UInt(_)) => {
                        let k = match iv {
                            Value::Int(k) => k,
                            Value::Const(k, _) => k as i128,
                            _ => unreachable!(),
                        };
                        if k < 0 || k >= n as i128 {
                            self.error(
                                Diagnostic::error(format!("index {k} is out of bounds"))
                                    .at(idx_e.span, format!("the array has {n} elements")),
                            );
                            return Value::Poison;
                        }
                        elems[k as usize].clone()
                    }
                    Value::Hw(node, Ty::UInt(w)) => {
                        let mut acc = elems[0].clone();
                        for (k, elem) in elems.iter().enumerate().skip(1) {
                            if (k as u128) > max_value(w) {
                                break;
                            }
                            let kn = cx.ir.add(Node::Const(k as u64), w);
                            let eq = cx.ir.add(Node::Bin(BinKind::Eq, node, kn), 1);
                            acc = self.mux_values(cx, eq, elem.clone(), acc);
                        }
                        acc
                    }
                    other => {
                        let desc = self.value_desc(&other);
                        self.error(
                            Diagnostic::error("array index must be an unsigned integer")
                                .at(idx_e.span, format!("found {desc}")),
                        );
                        Value::Poison
                    }
                }
            }
            Value::Const(_, Ty::UInt(_)) | Value::Hw(_, Ty::UInt(_)) => {
                let iv = self.eval(cx, idx_e, None);
                let k = match iv {
                    Value::Poison => return Value::Poison,
                    Value::Int(k) => k,
                    Value::Const(k, Ty::UInt(_)) => k as i128,
                    _ => {
                        self.error(
                            Diagnostic::error("bit index must be a compile-time integer")
                                .at(idx_e.span, "")
                                .note("to select a bit dynamically, shift first: `(x >> i)[0]`"),
                        );
                        return Value::Poison;
                    }
                };
                self.slice(cx, base, k, k + 1, base_span, idx_e.span)
            }
            other => {
                let desc = self.value_desc(&other);
                self.error(Diagnostic::error(format!("cannot index into {desc}")).at(base_span, ""));
                Value::Poison
            }
        }
    }

    fn slice(&mut self, cx: &mut Ctx, base: Value, lo: i128, hi: i128, base_span: Span, range_span: Span) -> Value {
        let w = match &base {
            Value::Poison => return Value::Poison,
            Value::Const(_, Ty::UInt(w)) | Value::Hw(_, Ty::UInt(w)) => *w,
            other => {
                let desc = self.value_desc(other);
                self.error(
                    Diagnostic::error(format!("cannot take bits of {desc}"))
                        .at(base_span, "")
                        .note("bit ranges work on unsigned integers; cast enums with `as uN`"),
                );
                return Value::Poison;
            }
        };
        if lo < 0 || hi <= lo || hi > w as i128 {
            self.error(
                Diagnostic::error(format!("bit range {lo}..{hi} is out of range"))
                    .at(range_span, format!("the value has bits 0..{w}")),
            );
            return Value::Poison;
        }
        let (lo, len) = (lo as u32, (hi - lo) as u32);
        match base {
            Value::Const(c, _) => Value::Const(mask(c >> lo, len), Ty::UInt(len)),
            Value::Hw(n, _) => {
                if lo == 0 && len == w {
                    return Value::Hw(n, Ty::UInt(w));
                }
                Value::Hw(cx.ir.add(Node::Slice(n, lo), len), Ty::UInt(len))
            }
            _ => unreachable!(),
        }
    }

    fn method(&mut self, cx: &mut Ctx, recv: Value, name: &Ident, args: &'a [Expr], recv_span: Span) -> Value {
        if let Some(arg) = args.first() {
            self.error(Diagnostic::error(format!("`.{}()` takes no arguments", name.name)).at(arg.span, ""));
            return Value::Poison;
        }
        if matches!(recv, Value::Poison) {
            return Value::Poison;
        }
        match (name.name.as_str(), &recv) {
            ("len", Value::Array(elems, _)) => Value::Int(elems.len() as i128),
            ("width", Value::Const(_, t) | Value::Hw(_, t)) => Value::Int(self.width(t) as i128),
            ("rev" | "any" | "all", Value::Const(_, Ty::UInt(w)) | Value::Hw(_, Ty::UInt(w))) => {
                let w = *w;
                if let Value::Const(c, _) = recv {
                    return match name.name.as_str() {
                        "rev" => Value::Const(c.reverse_bits() >> (64 - w), Ty::UInt(w)),
                        "any" => Value::Const((c != 0) as u64, Ty::UInt(1)),
                        _ => Value::Const((c == mask(u64::MAX, w)) as u64, Ty::UInt(1)),
                    };
                }
                let n = self.node_of(cx, &recv);
                match name.name.as_str() {
                    "rev" => Value::Hw(cx.ir.add(Node::Rev(n), w), Ty::UInt(w)),
                    "any" => {
                        let zero = cx.ir.add(Node::Const(0), w);
                        Value::Hw(cx.ir.add(Node::Bin(BinKind::Ne, n, zero), 1), Ty::UInt(1))
                    }
                    _ => {
                        let ones = cx.ir.add(Node::Const(mask(u64::MAX, w)), w);
                        Value::Hw(cx.ir.add(Node::Bin(BinKind::Eq, n, ones), 1), Ty::UInt(1))
                    }
                }
            }
            _ => {
                let desc = self.value_desc(&recv);
                self.error(
                    Diagnostic::error(format!("no method `{}` on {desc}", name.name))
                        .at(name.span, "")
                        .with(recv_span, "")
                        .note("methods: `.rev()`, `.any()`, `.all()`, `.width()` on integers; `.len()` on arrays"),
                );
                Value::Poison
            }
        }
    }

    /// Calls a built-in function.
    fn call(&mut self, cx: &mut Ctx, callee: &Ident, generics: Option<&'a [GenericArg]>, args: &'a [CallArg], span: Span) -> Value {
        if generics.is_some() {
            self.error(Diagnostic::error(format!("`{}` does not take parameters", callee.name)).at(callee.span, ""));
            return Value::Poison;
        }
        if let Some(named) = args.iter().find_map(|a| a.name.as_ref()) {
            self.error(Diagnostic::error(format!("`{}` does not take named arguments", callee.name)).at(named.span, ""));
            return Value::Poison;
        }
        let arity = |this: &mut Self, n: usize| {
            if args.len() != n {
                this.error(
                    Diagnostic::error(format!("`{}` takes {n} argument(s), found {}", callee.name, args.len()))
                        .at(span, ""),
                );
                false
            } else {
                true
            }
        };
        match callee.name.as_str() {
            "clog2" | "min" | "max" => {
                let n = if callee.name == "clog2" { 1 } else { 2 };
                if !arity(self, n) {
                    return Value::Poison;
                }
                let vals: Option<Vec<i128>> = args.iter().map(|a| self.expect_int(cx, &a.value)).collect();
                let Some(vals) = vals else {
                    return Value::Poison;
                };
                match callee.name.as_str() {
                    "clog2" => {
                        if vals[0] < 1 {
                            self.error(
                                Diagnostic::error(format!("`clog2` needs a positive argument, found {}", vals[0]))
                                    .at(args[0].value.span, ""),
                            );
                            return Value::Poison;
                        }
                        Value::Int(clog2(vals[0]))
                    }
                    "min" => Value::Int(vals[0].min(vals[1])),
                    _ => Value::Int(vals[0].max(vals[1])),
                }
            }
            "cat" | "rep" => {
                if callee.name == "cat" && args.is_empty() {
                    self.error(Diagnostic::error("`cat` needs at least one argument").at(span, ""));
                    return Value::Poison;
                }
                let (parts, total) = if callee.name == "rep" {
                    if !arity(self, 2) {
                        return Value::Poison;
                    }
                    let v = self.eval(cx, &args[0].value, None);
                    let Some(n) = self.expect_int(cx, &args[1].value) else {
                        return Value::Poison;
                    };
                    let Some(w) = self.sized_part(&v, args[0].value.span, "rep") else {
                        return Value::Poison;
                    };
                    if n < 1 || n * w as i128 > MAX_WIDTH as i128 {
                        self.error(
                            Diagnostic::error(format!("`rep` produces {} bits", n * w as i128))
                                .at(args[1].value.span, format!("the result must have between 1 and {MAX_WIDTH} bits")),
                        );
                        return Value::Poison;
                    }
                    (vec![v; n as usize], n as u32 * w)
                } else {
                    let mut parts = Vec::new();
                    let mut total = 0u32;
                    for a in args {
                        let v = self.eval(cx, &a.value, None);
                        let Some(w) = self.sized_part(&v, a.value.span, "cat") else {
                            return Value::Poison;
                        };
                        total += w;
                        parts.push(v);
                    }
                    if total > MAX_WIDTH {
                        self.error(
                            Diagnostic::error(format!("`cat` produces {total} bits"))
                                .at(span, format!("the result can have at most {MAX_WIDTH} bits")),
                        );
                        return Value::Poison;
                    }
                    (parts, total)
                };
                if parts.iter().all(|p| matches!(p, Value::Const(..))) {
                    let mut acc = 0u64;
                    for p in &parts {
                        if let Value::Const(c, t) = p {
                            let w = self.width(t);
                            acc = acc.checked_shl(w).unwrap_or(0) | c;
                        }
                    }
                    return Value::Const(acc, Ty::UInt(total));
                }
                let mut acc = self.node_of(cx, &parts[0]);
                for p in &parts[1..] {
                    let n = self.node_of(cx, p);
                    let w = cx.ir.width(acc) + cx.ir.width(n);
                    acc = cx.ir.add(Node::Cat(acc, n), w);
                }
                Value::Hw(acc, Ty::UInt(total))
            }
            _ => {
                let mut diag = Diagnostic::error(format!("cannot find module or function `{}`", callee.name))
                    .at(callee.span, "not found");
                if matches!(self.globals.get(&callee.name), Some((Global::Enum(_) | Global::Const(_), _))) {
                    diag = diag.note(format!("`{}` is not a module", callee.name));
                } else {
                    diag = diag.note("functions: cat, rep, clog2, min, max");
                }
                self.error(diag);
                Value::Poison
            }
        }
    }

    fn sized_part(&mut self, v: &Value, span: Span, func: &str) -> Option<u32> {
        match v {
            Value::Poison => None,
            Value::Const(_, Ty::UInt(w)) | Value::Hw(_, Ty::UInt(w)) => Some(*w),
            Value::Int(_) => {
                self.error(
                    Diagnostic::error(format!("`{func}` needs sized values"))
                        .at(span, "unsized integer")
                        .note("give it a width with a suffix, e.g. `0u4`"),
                );
                None
            }
            other => {
                let desc = self.value_desc(other);
                self.error(
                    Diagnostic::error(format!("`{func}` needs unsigned integers, found {desc}"))
                        .at(span, "")
                        .note("cast enums with `as uN`"),
                );
                None
            }
        }
    }

    /// Resolves a module or alias name and its parameters.
    fn module_ref(
        &mut self,
        cx: &mut Ctx,
        callee: &Ident,
        generics: Option<&'a [GenericArg]>,
    ) -> Option<(&'a ModuleDecl, ParamArgs)> {
        match self.globals.get(&callee.name) {
            Some((Global::Module(m), _)) => {
                let m = *m;
                if is_bundle(m) {
                    self.error(
                        Diagnostic::error(format!("`{}` is a bundle type, not a module", callee.name))
                            .at(callee.span, "it has no body")
                            .note(format!("declare a signal of this type with `let x: {};`", callee.name)),
                    );
                    return None;
                }
                let params = match generics {
                    Some(g) => self.generic_args(cx, g),
                    None => Vec::new(),
                };
                Some((m, params))
            }
            Some((Global::Alias(a), _)) => {
                let a = *a;
                let Some((Global::Module(target), _)) = self.globals.get(&a.target.name) else {
                    return None;
                };
                let target = *target;
                if generics.is_some() {
                    self.error(
                        Diagnostic::error(format!("`{}` is an alias; its parameters are already fixed", callee.name))
                            .at(callee.span, ""),
                    );
                    return None;
                }
                let mut gcx = Ctx::global(a.span);
                Some((target, self.generic_args(&mut gcx, &a.params)))
            }
            _ => None,
        }
    }

    /// Creates an instance of the module `callee` names and connects its arguments.
    /// Returns the instance's place: its signature bundle and first slot.
    fn instance(
        &mut self,
        cx: &mut Ctx,
        callee: &Ident,
        generics: Option<&'a [GenericArg]>,
        args: &'a [CallArg],
        span: Span,
        name: &str,
    ) -> Option<(Ty, u32)> {
        let (decl, params) = self.module_ref(cx, callee, generics)?;
        if cx.cond_depth > 0 {
            self.error(
                Diagnostic::error("instances cannot be created inside a conditional block")
                    .at(span, "")
                    .note("create the instance before the `if`/`match` and connect it inside"),
            );
            return None;
        }
        let Some(module) = self.instantiate(decl, params, span) else {
            for a in args {
                self.eval(cx, &a.value, None);
            }
            return None;
        };

        let sig = self.signature(module);
        let inst = cx.ir.instances.len() as u32;
        let child = self.modules[module].clone();
        cx.ir.instances.push(Instance {
            module,
            name: name.to_string(),
            inputs: vec![0; child.input_count as usize],
            span,
        });
        let base = cx.slots.len() as u32;
        let mut port_bases = Vec::new();
        cx.ir.cur_span = span;
        for port in &child.ports {
            port_bases.push(cx.slots.len() as u32);
            for leaf in &port.leaves {
                let path = format!("{name}.{}", leaf.path);
                match leaf.dir {
                    LeafDir::In(k) => {
                        let t = self.new_target(cx, &path, leaf.width, span, false);
                        cx.nets[t as usize].kind = Some(TargetKind::Wire);
                        cx.nets[t as usize].instance_input = true;
                        let net = cx.ir.add(Node::Net(t), leaf.width);
                        cx.ir.instances[inst as usize].inputs[k as usize] = net;
                        cx.slots.push(Slot::Target(t));
                    }
                    LeafDir::Out(o, _) => {
                        let node = cx.ir.add(Node::InstOut(inst, o), leaf.width);
                        cx.slots.push(Slot::InstOut(node));
                    }
                }
                cx.slot_names.push(path);
            }
        }

        let data_args: Vec<(usize, &CallArg)> = args.iter().enumerate().collect();
        let arg_ports: Vec<usize> = (0..child.ports.len()).filter(|&i| child.ports[i].is_arg).collect();
        let mut connected = vec![false; child.ports.len()];
        let mut named_seen = false;
        for (i, arg) in data_args {
            let idx = match &arg.name {
                Some(n) => {
                    named_seen = true;
                    match child.ports.iter().position(|p| p.name == n.name) {
                        Some(idx) if child.ports[idx].is_arg => idx,
                        Some(_) => {
                            self.error(
                                Diagnostic::error(format!(
                                    "`{}` is an output of `{}`; read it from the instance instead",
                                    n.name, child.display_name
                                ))
                                .at(n.span, ""),
                            );
                            continue;
                        }
                        None => {
                            let list: Vec<&str> = arg_ports.iter().map(|&p| child.ports[p].name.as_str()).collect();
                            self.error(
                                Diagnostic::error(format!("`{}` has no input `{}`", child.display_name, n.name))
                                    .at(n.span, "")
                                    .note(format!("inputs: {}", list.join(", "))),
                            );
                            continue;
                        }
                    }
                }
                None => {
                    if named_seen {
                        self.error(
                            Diagnostic::error("positional arguments must come before named ones").at(arg.value.span, ""),
                        );
                        continue;
                    }
                    match arg_ports.get(i) {
                        Some(&idx) => idx,
                        None => {
                            self.error(
                                Diagnostic::error(format!("`{}` takes {} argument(s)", child.display_name, arg_ports.len()))
                                    .at(arg.value.span, "unexpected argument"),
                            );
                            continue;
                        }
                    }
                }
            };
            if connected[idx] {
                self.error(
                    Diagnostic::error(format!("input `{}` is connected more than once", child.ports[idx].name))
                        .at(arg.value.span, ""),
                );
                continue;
            }
            connected[idx] = true;
            let place = Place {
                ty: child.ports[idx].ty.clone(),
                entries: vec![(port_bases[idx], None)],
            };
            if !self.assign_place(cx, &place, arg.value.span, AssignOp::Comb, arg.value.span, &arg.value) {
                self.poison_place(cx, &place);
            }
        }
        Some((Ty::Bundle(sig), base))
    }
}
