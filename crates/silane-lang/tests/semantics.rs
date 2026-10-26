//! Language semantics, checked by simulating the directly-built netlist.

use silane::Sim;
use silane::typenum::{U1, U2, U3, U4, U8};
use silane_lang::compile_str;

fn sim(src: &str, top: &str) -> Sim {
    let c = compile_str("test.si", src);
    assert!(!c.has_errors(), "{}", c.render_diagnostics());
    let circuit = c.circuit(top).expect("module");
    circuit.validate().unwrap();
    Sim::new(&circuit)
}

#[test]
fn last_assignment_wins() {
    let mut s = sim(
        "module M(a: u1, b: u1) -> (y: u8) {
            y = 1;
            if a { y = 2; }
            if b { y = 3; }
        }",
        "M",
    );
    let (a, b, y) = (s.input::<U1>("a"), s.input::<U1>("b"), s.output::<U8>("y"));
    for (av, bv, expected) in [(0, 0, 1), (1, 0, 2), (0, 1, 3), (1, 1, 3)] {
        s.drive(&a, av);
        s.drive(&b, bv);
        s.settle();
        assert_eq!(s.read(&y), expected);
    }
}

#[test]
fn registers_are_inferred_and_hold() {
    let mut s = sim(
        "module M(en: u1, d: u8) -> (q: u8 reset 7) {
            if en { q <= d; }
        }",
        "M",
    );
    let (en, d, q) = (s.input::<U1>("en"), s.input::<U8>("d"), s.output::<U8>("q"));
    assert_eq!(s.read(&q), 7);
    s.drive(&d, 42);
    s.step();
    assert_eq!(s.read(&q), 7);
    s.drive(&en, 1);
    s.step();
    assert_eq!(s.read(&q), 42);
}

#[test]
fn kind_fixed_inside_a_branch_still_holds_outside_it() {
    // `r` becomes a register in the `else` branch; the `then` branch must hold it.
    let mut s = sim(
        "module M(c: u1, d: u8) -> (q: u8) {
            let r: u8;
            if c { } else { r <= d; }
            q = r;
        }",
        "M",
    );
    let (c, d, q) = (s.input::<U1>("c"), s.input::<U8>("d"), s.output::<U8>("q"));
    s.drive(&d, 5);
    s.step();
    assert_eq!(s.read(&q), 5);
    s.drive(&c, 1);
    s.drive(&d, 9);
    s.step();
    assert_eq!(s.read(&q), 5);
}

#[test]
fn reads_see_final_net_values() {
    let mut s = sim("module M(a: u8) -> (y: u8) { let w: u8; y = w + 1; w = a; }", "M");
    let (a, y) = (s.input::<U8>("a"), s.output::<U8>("y"));
    s.drive(&a, 9);
    s.settle();
    assert_eq!(s.read(&y), 10);
}

#[test]
fn registers_swap() {
    let mut s = sim(
        "module M() -> (a: u8 reset 1, b: u8 reset 2) { a <= b; b <= a; }",
        "M",
    );
    let (a, b) = (s.output::<U8>("a"), s.output::<U8>("b"));
    s.step();
    assert_eq!((s.read(&a), s.read(&b)), (2, 1));
    s.step();
    assert_eq!((s.read(&a), s.read(&b)), (1, 2));
}

#[test]
fn dynamic_array_access() {
    let mut s = sim(
        "module M(we: u1, waddr: u2, raddr: u2, d: u8) -> (q: u8) {
            let mem: [u8; 4];
            if we { mem[waddr] <= d; }
            q = mem[raddr];
        }",
        "M",
    );
    let we = s.input::<U1>("we");
    let waddr = s.input::<U2>("waddr");
    let raddr = s.input::<U2>("raddr");
    let d = s.input::<U8>("d");
    let q = s.output::<U8>("q");
    s.drive(&we, 1);
    for i in 0..4 {
        s.drive(&waddr, i);
        s.drive(&d, 10 + i);
        s.step();
    }
    s.drive(&we, 0);
    for i in 0..4 {
        s.drive(&raddr, i);
        s.settle();
        assert_eq!(s.read(&q), 10 + i);
    }
}

#[test]
fn compile_time_branches_select_hardware() {
    let src = "
        module Sel<INVERT = 0>(a: u4) -> (y: u4) {
            if INVERT == 1 { y = !a; } else { y = a; }
        }
        module Top(a: u4) -> (plain: u4, inverted: u4) {
            plain = Sel(a).y;
            inverted = Sel::<INVERT = 1>(a).y;
        }";
    let mut s = sim(src, "Top");
    let a = s.input::<U4>("a");
    s.drive(&a, 0b0101);
    s.settle();
    assert_eq!(s.read(&s.output::<U4>("plain")), 0b0101);
    assert_eq!(s.read(&s.output::<U4>("inverted")), 0b1010);
}

#[test]
fn match_with_or_patterns_and_enums() {
    let src = "
        enum Kind: u2 { Zero, Small, Big }
        module M(x: u4) -> (k: Kind, code: u8) {
            k = match x {
                0 => Kind::Zero,
                1 | 2 | 3 => Kind::Small,
                _ => Kind::Big,
            };
            match k {
                Kind::Zero => code = 0xA0,
                Kind::Small => code = 0xB0 | (x as u8),
                Kind::Big => code = 0xFF,
            }
        }";
    let mut s = sim(src, "M");
    let x = s.input::<U4>("x");
    let k = s.output::<U2>("k");
    let code = s.output::<U8>("code");
    for (xv, kv, cv) in [(0, 0, 0xA0), (2, 1, 0xB2), (3, 1, 0xB3), (9, 2, 0xFF)] {
        s.drive(&x, xv);
        s.settle();
        assert_eq!((s.read(&k), s.read(&code)), (kv, cv), "x = {xv}");
    }
}

#[test]
fn shifts_slices_and_cat() {
    let mut s = sim(
        "module M(a: u8, n: u3) -> (l: u8, r: u8, c: u8, d: u8, b: u1) {
            l = a << 3;
            r = a >> 3;
            c = cat(a[0..4], a[4..8]);
            d = a << n;
            b = a[7];
        }",
        "M",
    );
    let a = s.input::<U8>("a");
    let n = s.input::<U3>("n");
    s.drive(&a, 0b1011_0110);
    s.drive(&n, 2);
    s.settle();
    assert_eq!(s.read(&s.output::<U8>("l")), 0b1011_0000);
    assert_eq!(s.read(&s.output::<U8>("r")), 0b0001_0110);
    assert_eq!(s.read(&s.output::<U8>("c")), 0b0110_1011);
    assert_eq!(s.read(&s.output::<U8>("d")), 0b1101_1000);
    assert_eq!(s.read(&s.output::<U1>("b")), 1);
}

#[test]
fn constant_expressions_fold() {
    let c = compile_str(
        "test.si",
        "const BASE = 0x40;
         const MASK: u8 = 0x0F;
         module M() -> (y: u8) { y = (BASE + 2 * 3) as u8 | MASK; }",
    );
    assert!(!c.has_errors(), "{}", c.render_diagnostics());
    let rust = c.rust().unwrap();
    assert!(rust.contains("constant_default(79)"), "{rust}");
}

#[test]
fn defaults_then_overrides() {
    let mut s = sim(
        "module M(sel: u2) -> (y: u8) {
            y = 0;
            match sel {
                1 => y = 10,
                2 => { y = 20; }
                _ => {}
            }
        }",
        "M",
    );
    let sel = s.input::<U2>("sel");
    let y = s.output::<U8>("y");
    for (sv, yv) in [(0, 0), (1, 10), (2, 20), (3, 0)] {
        s.drive(&sel, sv);
        s.settle();
        assert_eq!(s.read(&y), yv);
    }
}

#[test]
fn instance_inputs_can_be_connected_later() {
    let mut s = sim(
        "module Add(a: u8, b: u8) -> (y: u8) { y = a + b; }
         module Top(x: u8) -> (y: u8) {
            let add = Add(a: x);
            add.b = 3;
            y = add.y;
         }",
        "Top",
    );
    let x = s.input::<U8>("x");
    s.drive(&x, 4);
    s.settle();
    assert_eq!(s.read(&s.output::<U8>("y")), 7);
}

#[test]
fn bundles_connect_in_both_directions() {
    let src = "
        module S(ready: u1) -> (valid: u1, data: u8);
        module Through(i: S) -> (o: S) { let mid: S; mid = i; o = mid; }";
    let mut s = sim(src, "Through");
    let iv = s.input::<U1>("i_valid");
    let id = s.input::<U8>("i_data");
    let or = s.input::<U1>("o_ready");
    s.drive(&iv, 1);
    s.drive(&id, 77);
    s.drive(&or, 1);
    s.settle();
    assert_eq!(s.read(&s.output::<U1>("o_valid")), 1);
    assert_eq!(s.read(&s.output::<U8>("o_data")), 77);
    assert_eq!(s.read(&s.output::<U1>("i_ready")), 1);
}

#[test]
fn data_bundles_act_like_values() {
    // A bundle without flipped fields is plain data: it can be registered and muxed.
    let src = "
        module Pixel() -> (r: u4, g: u4);
        module M(sel: u1, a: Pixel, b: Pixel) -> (o: Pixel) {
            o <= if sel { a } else { b };
        }";
    let mut s = sim(src, "M");
    let sel = s.input::<U1>("sel");
    for (name, v) in [("a_r", 1), ("a_g", 2), ("b_r", 3), ("b_g", 4)] {
        let p = s.input::<U4>(name);
        s.drive(&p, v);
    }
    s.drive(&sel, 1);
    s.step();
    assert_eq!((s.read(&s.output::<U4>("o_r")), s.read(&s.output::<U4>("o_g"))), (1, 2));
    s.drive(&sel, 0);
    s.step();
    assert_eq!((s.read(&s.output::<U4>("o_r")), s.read(&s.output::<U4>("o_g"))), (3, 4));
}

#[test]
fn verilog_backend_runs() {
    let c = compile_str("test.si", "module M(a: u8) -> (y: u8) { y <= a; }");
    let v = c.circuit("M").unwrap().to_verilog("m");
    assert!(v.contains("always @(posedge clk_w or posedge rst_w)"), "{v}");
}

#[test]
fn declared_instances_connect_later() {
    let mut s = sim(
        "module Add(a: u8, b: u8) -> (y: u8) { y = a + b; }
         module Top(x: u8) -> (y: u8) {
            let add: Add;
            add.a = x;
            add.b = x;
            y = add.y;
         }",
        "Top",
    );
    let x = s.input::<U8>("x");
    s.drive(&x, 21);
    s.settle();
    assert_eq!(s.read(&s.output::<U8>("y")), 42);
}
