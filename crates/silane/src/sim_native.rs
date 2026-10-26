//! The compiled simulation backend: the evaluation plan of a [`super::Sim`] as C,
//! compiled to a shared library and loaded with `dlopen`.
//!
//! The library works on the simulator's own state (one `u64` per node, then the
//! memory words), so driving and reading ports is the same for both backends. It
//! exports `settle(state)` and `edge(state, clock)`, which clocks the registers and
//! memories of clock number `clock` (every clock for -1) without settling.
//!
//! Only the nodes something outside `settle` reads are stored in the state: ports,
//! register and memory connections, clocks. Every other combinational node is a C
//! local, which the compiler keeps in registers or drops; their state entries are
//! left stale.

use super::{DffStep, EvalStep, MemStep};
use crate::primitive::BinaryOp;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fmt::Write;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::Once;

type SettleFn = unsafe extern "C" fn(*mut u64);
// `settle_in` re-evaluates only the logic that depends on input ports.
type EdgeFn = unsafe extern "C" fn(*mut u64, c_int);

pub(super) struct Native {
    handle: *mut c_void,
    settle: SettleFn,
    settle_in: SettleFn,
    edge: EdgeFn,
    /// Length of the state the library was generated for.
    len: usize,
}

// The library has no state of its own: its functions only touch the slice they get.
unsafe impl Send for Native {}
unsafe impl Sync for Native {}

impl std::fmt::Debug for Native {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Native").field("len", &self.len).finish()
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        unsafe { dlclose(self.handle) };
    }
}

#[cfg_attr(target_os = "linux", link(name = "dl"))]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlerror() -> *const c_char;
}

const RTLD_NOW: c_int = 2;

impl Native {
    /// Compiles and loads the plan, or returns `None` (with a warning, once) if the
    /// interpreter should be used.
    pub(super) fn load(
        len: usize,
        comb: &[EvalStep],
        dffs: &[DffStep],
        mems: &[MemStep],
        clocks: &[usize],
        ports: &[usize],
        inputs: &[usize],
    ) -> Option<Native> {
        if std::env::var("SILANE_SIM").is_ok_and(|v| v == "interp") {
            return None;
        }
        match Self::build(&c_source(comb, dffs, mems, clocks, ports, inputs)) {
            Ok((handle, settle, settle_in, edge)) => Some(Native { handle, settle, settle_in, edge, len }),
            Err(e) => {
                static WARN: Once = Once::new();
                WARN.call_once(|| eprintln!("silane: interpreting the simulation ({e})"));
                None
            }
        }
    }

    fn build(src: &str) -> Result<(*mut c_void, SettleFn, SettleFn, EdgeFn), String> {
        let cc = std::env::var("SILANE_CC").unwrap_or_else(|_| "cc".to_string());
        let flags = ["-O3", "-shared", "-fPIC", "-w"];
        let mut hasher = DefaultHasher::new();
        (src, &cc, flags).hash(&mut hasher);
        let dir: PathBuf = std::env::temp_dir().join("silane-sim");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let stem = format!("{:016x}", hasher.finish());
        let lib = dir.join(format!("{stem}.so"));
        if !lib.exists() {
            // Unique temporaries, renamed into place: parallel tests may race.
            let tag = format!("{}-{:?}", std::process::id(), std::thread::current().id());
            let tag: String = tag.chars().filter(char::is_ascii_alphanumeric).collect();
            let c = dir.join(format!("{stem}-{tag}.c"));
            let tmp = dir.join(format!("{stem}-{tag}.so"));
            std::fs::write(&c, src).map_err(|e| format!("{}: {e}", c.display()))?;
            let out = std::process::Command::new(&cc)
                .args(flags)
                .arg("-o")
                .arg(&tmp)
                .arg(&c)
                .output()
                .map_err(|e| format!("cannot run `{cc}`: {e}"))?;
            let _ = std::fs::remove_file(&c);
            if !out.status.success() {
                return Err(format!("`{cc}` failed: {}", String::from_utf8_lossy(&out.stderr)));
            }
            std::fs::rename(&tmp, &lib).map_err(|e| format!("{}: {e}", lib.display()))?;
        }
        let path = CString::new(lib.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        unsafe {
            let handle = dlopen(path.as_ptr(), RTLD_NOW);
            if handle.is_null() {
                return Err(CStr::from_ptr(dlerror()).to_string_lossy().into_owned());
            }
            let settle = dlsym(handle, c"silane_settle".as_ptr());
            let settle_in = dlsym(handle, c"silane_settle_in".as_ptr());
            let edge = dlsym(handle, c"silane_edge".as_ptr());
            if settle.is_null() || settle_in.is_null() || edge.is_null() {
                dlclose(handle);
                return Err("missing symbols in the compiled simulation".to_string());
            }
            Ok((
                handle,
                std::mem::transmute::<*mut c_void, SettleFn>(settle),
                std::mem::transmute::<*mut c_void, SettleFn>(settle_in),
                std::mem::transmute::<*mut c_void, EdgeFn>(edge),
            ))
        }
    }

    pub(super) fn settle(&self, state: &mut [u64]) {
        assert!(state.len() >= self.len);
        unsafe { (self.settle)(state.as_mut_ptr()) }
    }

    /// Like [`Native::settle`], for a state settled before inputs were driven.
    pub(super) fn settle_inputs(&self, state: &mut [u64]) {
        assert!(state.len() >= self.len);
        unsafe { (self.settle_in)(state.as_mut_ptr()) }
    }

    pub(super) fn edge(&self, state: &mut [u64], clock: i32) {
        assert!(state.len() >= self.len);
        unsafe { (self.edge)(state.as_mut_ptr(), clock) }
    }
}

/// `& mask` for a `width`-bit result, or nothing for 64 bits.
fn mask(width: usize) -> String {
    if width >= 64 {
        String::new()
    } else {
        format!(" & 0x{:x}ULL", (1u64 << width) - 1)
    }
}

/// The node a step writes.
fn step_out(step: &EvalStep) -> usize {
    match *step {
        EvalStep::Const { out, .. }
        | EvalStep::Binary { out, .. }
        | EvalStep::Not { out, .. }
        | EvalStep::Mux { out, .. }
        | EvalStep::Slice { out, .. }
        | EvalStep::Concat { out, .. }
        | EvalStep::Rev { out, .. }
        | EvalStep::Buf { out, .. } => out,
    }
}

/// The nodes a step reads, in order (a mux: select, then input 0, then input 1).
fn step_inputs(step: &EvalStep) -> Vec<usize> {
    match *step {
        EvalStep::Const { .. } => vec![],
        EvalStep::Binary { a, b, .. } | EvalStep::Concat { a, b, .. } => vec![a, b],
        EvalStep::Not { a, .. } | EvalStep::Rev { a, .. } | EvalStep::Buf { a, .. } => vec![a],
        EvalStep::Slice { input, .. } => vec![input],
        EvalStep::Mux { sel, in0, in1, .. } => vec![sel, in0, in1],
    }
}

/// The C expression for a step other than a mux, with `n` naming nodes.
fn step_expr(step: &EvalStep, n: &dyn Fn(usize) -> String) -> String {
    match *step {
        EvalStep::Const { value, .. } => format!("0x{value:x}ULL"),
        EvalStep::Binary { op, a, b, width, .. } => {
            let m = mask(width);
            let (a, b) = (n(a), n(b));
            match op {
                BinaryOp::Add => format!("({a} + {b}){m}"),
                BinaryOp::Sub => format!("({a} - {b}){m}"),
                BinaryOp::Mul => format!("({a} * {b}){m}"),
                BinaryOp::And => format!("{a} & {b}"),
                BinaryOp::Or => format!("{a} | {b}"),
                BinaryOp::Xor => format!("{a} ^ {b}"),
                BinaryOp::Eq => format!("{a} == {b}"),
                BinaryOp::Lt => format!("{a} < {b}"),
                BinaryOp::Shl => format!("({b} >= 64 ? 0 : {a} << {b}){m}"),
                BinaryOp::Shr => format!("({b} >= 64 ? 0 : {a} >> {b}){m}"),
            }
        }
        EvalStep::Not { a, width, .. } => format!("~{}{}", n(a), mask(width)),
        EvalStep::Mux { sel, in0, in1, .. } => format!("{} ? {} : {}", n(sel), n(in1), n(in0)),
        EvalStep::Slice { input, offset, width, .. } => format!("({} >> {offset}){}", n(input), mask(width)),
        EvalStep::Concat { a, b, width_b, .. } => {
            if width_b >= 64 {
                n(b)
            } else {
                format!("({} << {width_b}) | {}", n(a), n(b))
            }
        }
        EvalStep::Rev { a, width, .. } => format!("rev({}, {width})", n(a)),
        EvalStep::Buf { a, .. } => n(a),
    }
}

/// An arm whose logic, with everything nested in it, is smaller than this is
/// computed unconditionally: a branch would cost more than it saves.
const MIN_ARM: usize = 2;

/// Where `settle` computes each step: logic needed only by one input of a mux is
/// computed inside an `if` on its select, so only the selected side is evaluated.
///
/// Regions form a tree: the root (the whole function) and, for each mux, one region
/// per input, nested in the region of the mux. A step belongs to the innermost
/// region containing every use of its result. Steps whose result nothing uses are
/// dropped.
struct Regions {
    /// Per region: its parent (the root is its own parent) and depth.
    parent: Vec<usize>,
    depth: Vec<usize>,
    /// Per step: its region, if the step is used at all.
    owner: Vec<Option<usize>>,
    /// Per mux step: the regions of input 0 and input 1, if they are kept.
    arms: std::collections::HashMap<usize, [Option<usize>; 2]>,
}

impl Regions {
    fn new(comb: &[EvalStep], kept: &std::collections::HashSet<usize>) -> Regions {
        let mut consumers: std::collections::HashMap<usize, Vec<(usize, usize)>> = Default::default();
        for (i, step) in comb.iter().enumerate() {
            for (slot, input) in step_inputs(step).into_iter().enumerate() {
                consumers.entry(input).or_default().push((i, slot));
            }
        }
        let mut r = Regions {
            parent: vec![0],
            depth: vec![0],
            owner: vec![None; comb.len()],
            arms: Default::default(),
        };
        let mut arm_of: std::collections::HashMap<(usize, usize), usize> = Default::default();
        for i in (0..comb.len()).rev() {
            let out = step_out(&comb[i]);
            let mut pos = kept.contains(&out).then_some(0);
            for &(c, slot) in consumers.get(&out).map(Vec::as_slice).unwrap_or(&[]) {
                let Some(oc) = r.owner[c] else { continue };
                let p = match comb[c] {
                    EvalStep::Mux { .. } if slot > 0 => arm_of[&(c, slot - 1)],
                    _ => oc,
                };
                pos = Some(match pos {
                    None => p,
                    Some(q) => r.lca(p, q),
                });
            }
            r.owner[i] = pos;
            if let (EvalStep::Mux { .. }, Some(o)) = (&comb[i], pos) {
                for arm in 0..2 {
                    arm_of.insert((i, arm), r.parent.len());
                    r.parent.push(o);
                    r.depth.push(r.depth[o] + 1);
                }
            }
        }

        // Fold small arms into their parents, innermost first (children are created
        // after their parents).
        let n = r.parent.len();
        let mut size = vec![0usize; n];
        for o in r.owner.iter().flatten() {
            size[*o] += 1;
        }
        let mut folded = vec![false; n];
        for reg in (1..n).rev() {
            if size[reg] < MIN_ARM {
                folded[reg] = true;
            }
            let p = r.parent[reg];
            size[p] += size[reg];
        }
        let resolve = |mut reg: usize| {
            while folded[reg] {
                reg = r.parent[reg];
            }
            reg
        };
        let owner: Vec<Option<usize>> = r.owner.iter().map(|o| o.map(resolve)).collect();
        for (&(m, arm), &reg) in &arm_of {
            if !folded[reg] {
                r.arms.entry(m).or_insert([None, None])[arm] = Some(reg);
            }
        }
        r.owner = owner;
        r
    }

    fn lca(&self, mut a: usize, mut b: usize) -> usize {
        while self.depth[a] > self.depth[b] {
            a = self.parent[a];
        }
        while self.depth[b] > self.depth[a] {
            b = self.parent[b];
        }
        while a != b {
            a = self.parent[a];
            b = self.parent[b];
        }
        a
    }
}

fn c_source(
    comb: &[EvalStep],
    dffs: &[DffStep],
    mems: &[MemStep],
    clocks: &[usize],
    ports: &[usize],
    inputs: &[usize],
) -> String {
    // Nodes that must live in the state array.
    let mut kept = std::collections::HashSet::new();
    kept.extend(ports.iter().copied());
    kept.extend(clocks.iter().copied());
    for d in dffs {
        kept.extend([d.d, d.q, d.clk]);
        kept.extend(d.rst);
    }
    for m in mems {
        kept.extend([m.we, m.waddr, m.wdata, m.re, m.raddr, m.rdata, m.clk]);
    }

    // The steps that depend on inputs (in order), and the other nodes they read,
    // which must be in the state for `settle_in`.
    let mut dirty: std::collections::HashSet<usize> = inputs.iter().copied().collect();
    let mut cone = Vec::new();
    for step in comb {
        if step_inputs(step).iter().any(|i| dirty.contains(i)) {
            dirty.insert(step_out(step));
            cone.push(step.clone());
        }
    }
    for step in &cone {
        for i in step_inputs(step) {
            if !dirty.contains(&i) {
                kept.insert(i);
            }
        }
    }
    let n = |i: usize| if kept.contains(&i) { format!("s[{i}]") } else { format!("n{i}") };

    let mut c = String::new();
    c.push_str("#include <stdint.h>\ntypedef uint64_t u64;\n\n");
    c.push_str(
        "static u64 rev(u64 x, int w) {\n    u64 r = 0;\n    for (int i = 0; i < w; i++)\n        r |= ((x >> i) & 1) << (w - 1 - i);\n    return r;\n}\n\n",
    );

    for (name, steps) in [("silane_settle", comb), ("silane_settle_in", &cone[..])] {
        let regions = Regions::new(steps, &kept);
        let mut by_region: Vec<Vec<usize>> = vec![Vec::new(); regions.parent.len()];
        for (i, o) in regions.owner.iter().enumerate() {
            if let Some(o) = o {
                by_region[*o].push(i);
            }
        }
        let _ = writeln!(c, "void {name}(u64 *restrict s) {{");
        emit_region(&mut c, 0, 1, steps, &regions, &by_region, &kept, &n);
        c.push_str("}\n\n");
    }

    // Registers and memories: all next values are computed from the state before any
    // is committed. Memories read before they are written.
    let clock_of = |net: usize| clocks.iter().position(|&k| k == net).expect("numbered clock");
    let _ = writeln!(c, "void silane_edge(u64 *restrict s, int clk) {{");
    let _ = writeln!(c, "    u64 t[{}];", dffs.len().max(1));
    let _ = writeln!(c, "    u64 r[{}];", mems.len().max(1));
    let _ = writeln!(c, "    unsigned char rv[{}];", mems.len().max(1));
    for k in 0..clocks.len() {
        let _ = writeln!(c, "    int c{k} = clk < 0 || clk == {k};");
    }
    for (i, d) in dffs.iter().enumerate() {
        let k = clock_of(d.clk);
        let next = match d.rst {
            Some(rst) => format!("s[{rst}] ? 0x{:x}ULL : s[{}]", d.reset_val, d.d),
            None => format!("s[{}]", d.d),
        };
        let _ = writeln!(c, "    if (c{k}) t[{i}] = {next};");
    }
    for (i, m) in mems.iter().enumerate() {
        let k = clock_of(m.clk);
        let _ = writeln!(c, "    rv[{i}] = c{k} && s[{}];", m.re);
        let _ = writeln!(
            c,
            "    if (rv[{i}]) r[{i}] = s[{ra}] < {d} ? s[{b} + s[{ra}]] : 0;",
            ra = m.raddr,
            d = m.depth,
            b = m.base
        );
        let _ = writeln!(
            c,
            "    if (c{k} && s[{we}] && s[{wa}] < {d}) s[{b} + s[{wa}]] = s[{wd}];",
            we = m.we,
            wa = m.waddr,
            wd = m.wdata,
            d = m.depth,
            b = m.base
        );
    }
    for (i, d) in dffs.iter().enumerate() {
        let k = clock_of(d.clk);
        let _ = writeln!(c, "    if (c{k}) s[{}] = t[{i}];", d.q);
    }
    for (i, m) in mems.iter().enumerate() {
        let _ = writeln!(c, "    if (rv[{i}]) s[{}] = r[{i}];", m.rdata);
    }
    c.push_str("}\n");
    c
}

/// Emits the steps of region `reg`, with the arms of its muxes as nested `if`s.
#[allow(clippy::too_many_arguments)]
fn emit_region(
    c: &mut String,
    reg: usize,
    indent: usize,
    comb: &[EvalStep],
    regions: &Regions,
    by_region: &[Vec<usize>],
    kept: &std::collections::HashSet<usize>,
    n: &dyn Fn(usize) -> String,
) {
    let pad = "    ".repeat(indent);
    for &i in &by_region[reg] {
        let out = step_out(&comb[i]);
        match (&comb[i], regions.arms.get(&i)) {
            (&EvalStep::Mux { sel, in0, in1, .. }, Some(arms)) => {
                if !kept.contains(&out) {
                    let _ = writeln!(c, "{pad}u64 n{out};");
                }
                let _ = writeln!(c, "{pad}if ({}) {{", n(sel));
                if let Some(r1) = arms[1] {
                    emit_region(c, r1, indent + 1, comb, regions, by_region, kept, n);
                }
                let _ = writeln!(c, "{pad}    {} = {};", n(out), n(in1));
                let _ = writeln!(c, "{pad}}} else {{");
                if let Some(r0) = arms[0] {
                    emit_region(c, r0, indent + 1, comb, regions, by_region, kept, n);
                }
                let _ = writeln!(c, "{pad}    {} = {};", n(out), n(in0));
                let _ = writeln!(c, "{pad}}}");
            }
            (step, _) => {
                let expr = step_expr(step, n);
                if kept.contains(&out) {
                    let _ = writeln!(c, "{pad}s[{out}] = {expr};");
                } else {
                    let _ = writeln!(c, "{pad}const u64 n{out} = {expr};");
                }
            }
        }
    }
}
