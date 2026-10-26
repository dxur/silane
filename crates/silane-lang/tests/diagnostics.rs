//! Every class of error is reported at compile time, with a useful message.

use silane_lang::compile_str;

/// Compiles `src` and returns rendered diagnostics, asserting that it failed.
fn errors(src: &str) -> String {
    let c = compile_str("test.si", src);
    let out = c.render_diagnostics();
    assert!(c.has_errors(), "expected errors, got:\n{out}");
    assert!(c.rust().is_none());
    out
}

fn assert_error(src: &str, needle: &str) {
    let out = errors(src);
    assert!(out.contains(needle), "expected `{needle}` in:\n{out}");
}

fn warnings(src: &str) -> String {
    let c = compile_str("test.si", src);
    let out = c.render_diagnostics();
    assert!(!c.has_errors(), "unexpected errors:\n{out}");
    out
}

#[test]
fn latch_is_rejected() {
    let out = errors("module M(c: u1, a: u8) -> (y: u8) { if c { y = a; } }");
    assert!(out.contains("`y` is not driven on every path"), "{out}");
    assert!(out.contains("make it a register with `<=`"), "{out}");
}

#[test]
fn match_without_default_is_a_latch() {
    assert_error(
        "module M(s: u2) -> (y: u8) { match s { 0 => y = 1, 1 => y = 2 } }",
        "`y` is not driven on every path",
    );
}

#[test]
fn undriven_nets() {
    assert_error("module M() -> (y: u8) { }", "output `y` is never driven");
    assert_error("module M() -> (y: u8) { let n: u8; y = n; }", "net `n` is never driven");
}

#[test]
fn a_net_is_either_a_wire_or_a_register() {
    let out = errors("module M(a: u8, c: u1) -> (y: u8) { let n: u8; n = a; if c { n <= 0; } y = n; }");
    assert!(out.contains("`n` is driven with `<=` here, but it is a wire"), "{out}");
    assert!(out.contains("first driven with `=` here"), "{out}");
    assert_error(
        "module M(a: u8) -> (y: u8) { let x = a; x = a; y = x; }",
        "cannot assign to `x`",
    );
    assert_error("module M(a: u8) -> (y: u8) { a = 1; y = a; }", "cannot drive `a`: it is an input of this module");
}

#[test]
fn reset_values_belong_to_registers() {
    assert_error(
        "module M(a: u8) -> (y: u8) { let n: u8 reset 3; n = a; y = n; }",
        "`n` has a reset value but is driven with `=`",
    );
    assert_error("module M(a: u8 reset 1) -> (y: u8) { y = a; }", "`a` is an input, so it cannot have a reset value");
}

#[test]
fn old_keywords_point_to_the_new_syntax() {
    assert_error("module M(in a: u1) -> (y: u1) { y = a; }", "inputs go in `(...)`");
    assert_error("module M(a: u1) -> (y: u1) { reg r: u1; r <= a; y = r; }", "declare nets with `let name: type;`");
}

#[test]
fn widths_must_match() {
    let out = errors("module M(a: u8, b: u4) -> (y: u8) { y = a + b; }");
    assert!(out.contains("mismatched types: `u8` + `u4`"), "{out}");
    assert!(out.contains("as u8"), "{out}");
    assert_error("module M(a: u4) -> (y: u8) { y = a; }", "expected `u8`, found `u4`");
}

#[test]
fn literals_must_fit() {
    assert_error("module M() -> (y: u4) { y = 20; }", "literal `20` does not fit in `u4`");
    assert_error("module M() -> (y: u4) { y = -1; }", "!0u4");
    assert_error("module M(a: u4) -> (y: u4) { let r: u4 reset 16; r <= a; y = r; }", "does not fit");
}

#[test]
fn combinational_loops_are_found() {
    assert_error(
        "module M() -> (y: u1) { let a: u1; let b: u1; a = b; b = !a; y = a; }",
        "combinational loop",
    );
    assert_error("module M() -> (y: u8) { y = y + 1; }", "combinational loop: `y` → `y`");
}

#[test]
fn loops_through_instances_are_found() {
    assert_error(
        "module Inv(a: u1) -> (y: u1) { y = !a; }
         module Top() -> (y: u1) { let i = Inv(); i.a = i.y; y = i.y; }",
        "combinational loop",
    );
}

#[test]
fn feedback_through_registers_is_fine() {
    warnings(
        "module Delay(a: u1) -> (y: u1) { y <= a; }
         module Top() -> (y: u1) { let d = Delay(); d.a = !d.y; y = d.y; }",
    );
}

#[test]
fn match_must_be_exhaustive() {
    assert_error(
        "enum E { A, B, C }
         module M(e: E) -> (y: u8) { y = match e { E::A => 1, E::B => 2 }; }",
        "non-exhaustive match: `E::C` not covered",
    );
    assert_error(
        "module M(s: u2) -> (y: u8) { y = match s { 0 => 1, 1 => 2, 3 => 4 }; }",
        "`2` not covered",
    );
    warnings("module M(s: u1) -> (y: u8) { y = match s { 0 => 1, 1 => 2 }; }");
}

#[test]
fn names_must_resolve() {
    assert_error("module M() -> (y: u8) { y = nope; }", "cannot find `nope` in this scope");
    assert_error("module M() -> (y: u8) { y = Missing(); }", "cannot find module or function `Missing`");
    assert_error("enum E { A } module M() -> (y: E) { y = E::B; }", "enum `E` has no variant `B`");
}

#[test]
fn declarations_inside_conditions_are_rejected() {
    assert_error(
        "module M(c: u1) -> (y: u8) { y = 0; if c { let r: u8; r <= 1; } }",
        "nets cannot be declared inside a conditional block",
    );
}

#[test]
fn casts_only_widen() {
    assert_error("module M(a: u8) -> (y: u4) { y = a as u4; }", "cannot narrow `u8` to `u4`");
}

#[test]
fn instance_inputs_must_be_connected() {
    assert_error(
        "module Sub(a: u1, b: u1) -> (y: u1) { y = a & b; }
         module Top(x: u1) -> (y: u1) { let s = Sub(a: x); y = s.y; }",
        "input `b` of instance `s` is not connected",
    );
    assert_error(
        "module Sub(a: u1) -> (y: u1) { y = a; }
         module Top(x: u1) -> (y: u1) { let s = Sub(x); y = s.z; }",
        "has no field `z`",
    );
    assert_error(
        "module Sub(a: u1) -> (y: u1) { y = a; }
         module Top(x: u1) -> (y: u1) { let s = Sub(x); s.y = x; y = x; }",
        "cannot drive `s.y`: it is an output of the instance",
    );
    assert_error(
        "module Sub(a: u1) -> (y: u1) { y = a; }
         module Top(x: u1) -> (y: u1) { let s = Sub(); s.a <= x; y = s.y; }",
        "instance input `s.a` is connected with `=`",
    );
}

#[test]
fn recursion_is_rejected() {
    assert_error("module R(a: u1) -> (y: u1) { let r = R(a); y = r.y; }", "instantiates itself");
}

#[test]
fn reserved_port_names() {
    assert_error("module M(clk: u1) -> (y: u1) { y = clk; }", "`clk` is a reserved port name");
}

#[test]
fn parse_errors_point_at_the_token() {
    let out = errors("module M(a: u8) -> (y: u8) { y = a + ; }");
    assert!(out.contains("expected an expression, found `;`"), "{out}");
    assert!(out.contains("test.si:1:"), "{out}");
}

#[test]
fn dynamic_bit_select_is_rejected() {
    assert_error(
        "module M(a: u8, i: u3) -> (y: u1) { y = a[i]; }",
        "bit index must be a compile-time integer",
    );
    assert_error("module M(a: u8) -> (y: u8) { y[0..4] = a[0..4]; y = a; }", "cannot assign to a bit range");
}

#[test]
fn unsized_branches_need_a_type() {
    assert_error(
        "module M(c: u1) -> (y: u8) { let x = if c { 1 } else { 2 }; y = x; }",
        "cannot infer the width",
    );
    warnings("module M(c: u1) -> (y: u8) { let x: u8 = if c { 1 } else { 2 }; y = x; }");
    warnings("module M(c: u1) -> (y: u8) { y = if c { 1 } else { 2 }; }");
}

#[test]
fn conditions_must_be_single_bits() {
    assert_error(
        "module M(a: u8) -> (y: u8) { y = if a { 1 } else { 0 }; }",
        "expected a `u1` condition, found `u8`",
    );
}

#[test]
fn hardware_division_is_rejected() {
    assert_error("module M(a: u8) -> (y: u8) { y = a / 2; }", "only available on compile-time integers");
}

#[test]
fn shadowing_is_rejected() {
    assert_error("module M(a: u8) -> (y: u8) { let a = 1; y = a; }", "`a` is already defined");
}

#[test]
fn missing_parameter() {
    assert_error(
        "module Sub<N>() -> (y: u<N>) { y = 0; }
         module Top() -> (y: u4) { let s = Sub(); y = s.y; }",
        "missing value for parameter `N`",
    );
}

#[test]
fn errors_in_instances_name_the_parameters() {
    let out = errors(
        "module Sub<N = 4>() -> (y: u<N>) { y = 20; }
         module Top() -> (y: u8) { let s = Sub::<N = 8>(); y = s.y; }",
    );
    assert!(out.contains("literal `20` does not fit in `u4`"), "{out}");
}

#[test]
fn widths_are_bounded() {
    assert_error("module M() -> (y: u65) { y = 0; }", "unsupported width 65");
    assert_error("module M(a: u40) -> (y: u8) { y = cat(a, a)[0..8]; }", "`cat` produces 80 bits");
}

#[test]
fn bundle_direction_errors() {
    let bundle = "module S(ready: u1) -> (valid: u1, data: u8);\n";
    // A consumer cannot drive the producer's fields.
    assert_error(
        &format!("{bundle} module M(s: S) -> (y: u8) {{ s.valid = 1; s.ready = 1; y = s.data; }}"),
        "cannot drive `s.valid`: it is an input of this module",
    );
    // Connecting in the wrong direction suggests swapping.
    let out = errors(&format!("{bundle} module M(i: S) -> (o: S) {{ i = o; }}"));
    assert!(out.contains("swap the sides"), "{out}");
    // Bundles with flipped fields cannot be registered as a whole.
    assert_error(
        &format!("{bundle} module M(i: S) -> (o: S) {{ o <= i; }}"),
        "has flipped fields, so it cannot be driven with `<=`",
    );
    // Bundles are types, not modules.
    assert_error(&format!("{bundle} module M() -> (y: u1) {{ let s = S(); y = 0; }}"), "is a bundle type, not a module");
    // A module-typed port is the module's interface: its inputs must be driven.
    assert_error(
        "module A(x: u1) -> (y: u1) { y = x; } module M(a: A) -> (y: u1) { y = a.y; }",
        "output `a.x` is never driven",
    );
}

#[test]
fn unused_things_warn() {
    let out = warnings("module M(a: u8, b: u8) -> (y: u8) { let t = b; y = a; }");
    assert!(out.contains("warning: unused binding `t`"), "{out}");
    let out = warnings("module M(a: u8) -> (y: u8) { let r: u8; r <= a; y = a; }");
    assert!(out.contains("`r` is never read"), "{out}");
    let quiet = warnings("module M(a: u8, _b: u8) -> (y: u8) { y = a; }");
    assert!(!quiet.contains("warning"), "{quiet}");
}

#[test]
fn unreachable_arms_warn() {
    let out = warnings("module M(s: u2) -> (y: u8) { y = match s { 0 => 1, _ => 2, 1 => 3 }; }");
    assert!(out.contains("unreachable match arm"), "{out}");
}

#[test]
fn modules_with_required_parameters_must_be_instantiated() {
    let out = warnings("module Sub<N>() -> (y: u<N>) { y = 0; }");
    assert!(out.contains("module `Sub` is never used"), "{out}");
}

#[test]
fn imports_resolve_and_report_missing_files() {
    let c = silane_lang::compile_sources(&[
        ("top.si", "import \"lib/sub.si\"; module Top(a: u1) -> (y: u1) { let s = Sub(a); y = s.y; }"),
        ("lib/sub.si", "module Sub(a: u1) -> (y: u1) { y = !a; }"),
    ]);
    assert!(!c.has_errors(), "{}", c.render_diagnostics());
    let c = compile_str("top.si", "import \"nope.si\"; module Top() -> (y: u1) { y = 0; }");
    assert!(c.render_diagnostics().contains("cannot read `nope.si`"));
}

#[test]
fn modules_as_types() {
    // An instance declared without arguments still needs every input connected.
    assert_error(
        "module Sub(a: u1) -> (y: u1) { y = a; }
         module Top() -> (y: u1) { let s: Sub; y = s.y; }",
        "input `a` of instance `s` is not connected",
    );
    assert_error(
        "module Sub(a: u1) -> (y: u1) { y = a; }
         module Top(x: u1) -> (y: u1) { let s: Sub reset 1; s.a = x; y = s.y; }",
        "an instance cannot have a reset value",
    );
    // A module-typed port is an interface: the module drives its inputs.
    assert_error(
        "module Sub(a: u1) -> (y: u1) { y = a; }
         module User(s: Sub) -> (y: u1) { y = s.y; }",
        "output `s.a` is never driven",
    );
    assert_error(
        "module Loop(x: Loop) -> (y: u1) { y = 0; }",
        "instantiates itself",
    );
}

#[test]
fn extern_modules() {
    assert_error(
        "module M() -> (y: u1) = \"some::Type\";",
        "must be declared `extern module`",
    );
    assert_error("extern module M() -> (y: u1);", "needs the path of its Rust type");
    // Extern outputs are assumed to depend on every input.
    assert_error(
        "extern module Buf(a: u1) -> (y: u1) = \"x::Buf\";
         module Top() -> (y: u1) { let b: Buf; b.a = b.y; y = b.y; }",
        "combinational loop",
    );
    let c = compile_str(
        "t.si",
        "import \"std:cdc\"; module M(a: u1) -> (y: u1) { let s = Sync2(a); y = s.q; }",
    );
    assert!(!c.has_errors(), "{}", c.render_diagnostics());
    let rust = c.rust().unwrap();
    // Externs are used through adapters with their declared signatures.
    assert!(rust.contains("pub struct Sync2<D0: 'static = ::silane::Sys, D1: 'static = ::silane::Sys>"), "{rust}");
    assert!(rust.contains("::silane::cdc::Sync2::new(d)"), "{rust}");
    // The synchronizer separates `a` from `y`: two clock domains.
    assert!(rust.contains("pub struct M<D0: 'static = ::silane::Sys, D1: 'static = ::silane::Sys>"), "{rust}");
    assert!(rust.contains("pub fn new(a: SiWire<::silane::typenum::U1, D0>)"), "{rust}");
    assert!(rust.contains("pub y: SiWire<::silane::typenum::U1, D1>"), "{rust}");
    // The netlist backend cannot build Rust-implemented modules.
    assert!(c.circuit("M").is_none());
    assert_error(
        "module M(a: u1 @fast) -> (y: u1) { y = a; }",
        "only `extern module` ports have domain labels",
    );
}

#[test]
fn clock_domains_are_inferred() {
    let domains = |src: &str, module: &str| -> usize {
        let c = compile_str("t.si", src);
        assert!(!c.has_errors(), "{}", c.render_diagnostics());
        let rust = c.rust().unwrap();
        let line = rust
            .lines()
            .find(|l| l.starts_with(&format!("pub struct {module}")))
            .unwrap_or_else(|| panic!("no struct {module}: {rust}"));
        line.matches("'static").count()
    };
    // Connected logic and registers share a domain.
    assert_eq!(domains("module R(a: u8) -> (y: u8) { let r: u8; r <= a + 1; y = r; }", "R"), 1);
    // Independent paths are independent domains: nothing ties them.
    assert_eq!(domains("module P(a: u1, b: u1) -> (x: u1, y: u1) { x = a; y = !b; }", "P"), 2);
    // A synchronizer keeps its sides apart, unless the logic joins them again.
    let sync = "import \"std:cdc\";";
    assert_eq!(domains(&format!("{sync} module S(a: u1) -> (y: u1) {{ y = Sync2(a).q; }}"), "S"), 2);
    assert_eq!(domains(&format!("{sync} module J(a: u1) -> (y: u1) {{ y = Sync2(a).q & a; }}"), "J"), 1);
    // Domains flow through instances.
    assert_eq!(
        domains(&format!("{sync} module S(a: u1) -> (y: u1) {{ y = Sync2(a).q; }} module T(a: u1) -> (y: u1) {{ let s = S(a); y = s.y; }}"), "T"),
        2
    );
}

#[test]
fn empty_port_lists_can_be_omitted() {
    warnings("module Blink -> (led: u1) { led <= !led; }");
    warnings("module Pixel -> (r: u4, g: u4);");
    warnings("module Sink(a: u1) { }");
}
