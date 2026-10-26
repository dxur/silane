//! # Hardware DSL Macros for Silane HCL
//!
//! Provides `when!` and `select!` for procedural hardware control flow and multiplexing.

/// Unified hardware control flow, procedural statement, and multiplexing macro (`when!`).
///
/// Supports:
/// 1. Unconditional and conditional register updates (`when! { reg <= val; }`) with automatic latch prevention
/// 2. Case pattern matching expressions (`when!(sel => { 0b00 => a, _ => b })`)
/// 3. 2-way and multi-guard condition branching
///
/// # Examples
/// ```ignore
/// // Unconditional register assignment
/// when! {
///     r0_u <= 42;
/// }
///
/// // Procedural register assignment with auto-latch prevention
/// when!(we => {
///     r0_u <= wdata;
/// });
///
/// // Procedural register assignment with else branch
/// when!(we => {
///     r0_u <= wdata;
/// }, else => {
///     r0_u <= 0;
/// });
///
/// // Case pattern matching expression
/// let out = when!(opcode => {
///     0b000 => a + b,
///     0b001 => a - b,
///     _     => a,
/// });
/// ```
#[macro_export]
macro_rules! when {
    // -----------------------------------------------------------------------
    // 1a. Unconditional SEQUENTIAL statement: when! { reg <= val; }
    // -----------------------------------------------------------------------
    ($( $reg:ident <= $val:expr );+ $(;)?) => {
        $(
            let val_bound = $val;
            $reg.next(val_bound);
        )+
    };

    // -----------------------------------------------------------------------
    // 1b. Unconditional COMBINATIONAL statement: when! { wire = val; }
    // -----------------------------------------------------------------------
    ($( $wire:ident = $val:expr );+ $(;)?) => {
        $(
            let val_bound = $val;
            $wire.assign(val_bound);
        )+
    };

    // -----------------------------------------------------------------------
    // 2. Conditional SEQUENTIAL statement block with <= (with optional else)
    // -----------------------------------------------------------------------
    ($cond:expr => { $( $reg1:ident <= $val1:expr );+ $(;)? }, else => { $( $reg2:ident <= $val2:expr );+ $(;)? }) => {
        $(
            let val1_bound = $val1;
            let val2_bound = $val2;
            $reg1.next($cond.mux(val1_bound, val2_bound));
        )+
    };

    ($cond:expr => { $( $reg:ident <= $val:expr );+ $(;)? }) => {
        $(
            let prev = $reg.read();
            let val_bound = $val;
            $reg.next($cond.mux(val_bound, prev));
        )+
    };

    // -----------------------------------------------------------------------
    // 2b. Conditional COMBINATIONAL statement block with = (with optional else)
    // -----------------------------------------------------------------------
    ($cond:expr => { $( $wire1:ident = $val1:expr );+ $(;)? }, else => { $( $wire2:ident = $val2:expr );+ $(;)? }) => {
        $(
            let val1_bound = $val1;
            let val2_bound = $val2;
            $wire1.assign($cond.mux(val1_bound, val2_bound));
        )+
    };

    ($cond:expr => { $( $wire:ident = $val:expr );+ $(;)? }) => {
        $(
            let prev = $wire.read();
            let val_bound = $val;
            $wire.assign($cond.mux(val_bound, prev));
        )+
    };

    // -----------------------------------------------------------------------
    // 3. Selector block entry points: when!(sel => { ... }) / when!(sel, { ... })
    // -----------------------------------------------------------------------
    ($sel:expr => { $($body:tt)+ }) => {
        $crate::when!(@impl $sel, [], $($body)+)
    };

    ($sel:expr, { $($body:tt)+ }) => {
        $crate::when!(@impl $sel, [], $($body)+)
    };

    // TT muncher base case: default with `_`
    (@impl $sel:expr, [$( ($c:expr, $r:expr), )*], _ => $default_res:expr $(,)?) => {{
        let default_w = $default_res;
        $sel.mux_cases(&[ $( ($c, $crate::signal::IntoWire::into_wire($r, default_w.value.width)) ),* ], default_w)
    }};

    // TT muncher base case: default with `else`
    (@impl $sel:expr, [$( ($c:expr, $r:expr), )*], else => $default_res:expr $(,)?) => {{
        let default_w = $default_res;
        $sel.mux_cases(&[ $( ($c, $crate::signal::IntoWire::into_wire($r, default_w.value.width)) ),* ], default_w)
    }};

    // TT muncher recursive step: collect (case_val => res_val)
    (@impl $sel:expr, [$( ($c:expr, $r:expr), )*], $case_val:expr => $res_val:expr, $($rest:tt)+) => {
        $crate::when!(@impl $sel, [$( ($c, $r), )* ($case_val, $res_val),], $($rest)+)
    };

    // -----------------------------------------------------------------------
    // 4. Inline 2-way expression: when!(cond => then_val, else => else_val)
    // -----------------------------------------------------------------------
    ($cond:expr => $then_val:expr, else => $else_val:expr $(,)?) => {
        $cond.mux($then_val, $else_val)
    };

    // -----------------------------------------------------------------------
    // 5. Multi-guard priority list: when! { c1 => v1, c2 => v2, else => default }
    // -----------------------------------------------------------------------
    (else => $else_val:expr $(,)?) => {
        $else_val
    };

    ($cond:expr => $then_val:expr, $($rest:tt)+) => {
        $cond.mux($then_val, $crate::when!($($rest)+))
    };
}

/// Hardware array / index multiplexer (`select!`).
///
/// Multiplexes among an array or list of candidate signals using a selector wire.
///
/// # Examples
/// ```ignore
/// let rdata = select!(raddr => [r0, r1, r2, r3]);
/// let rdata = select!(raddr, r0, r1, r2, r3);
/// ```
#[macro_export]
macro_rules! select {
    ($sel:expr => [ $($elem:expr),+ $(,)? ]) => {
        $sel.select([ $($elem),+ ])
    };

    ($sel:expr, [ $($elem:expr),+ $(,)? ]) => {
        $sel.select([ $($elem),+ ])
    };

    ($sel:expr, $($elem:expr),+ $(,)?) => {
        $sel.select([ $($elem),+ ])
    };
}

#[cfg(test)]
mod tests {
    use crate::circuit::Circuit;
    use crate::signal::Signal;
    use crate::sim::Sim;
    use typenum::{U1, U2, U8};

    #[test]
    fn test_when_unconditional_procedural_statement() {
        let (_, circuit) = Circuit::elaborate(|| {
            crate::Domain::<crate::signal::Sys>::ports("clk", "rst");
            let r0_u = Signal::<U8>::default();
            let r0 = r0_u.read();

            when! {
                r0_u <= 42;
            }

            Signal::output_port("r0", r0);
        });

        let mut sim = Sim::new(&circuit);
        let r0_out = sim.output::<U8>("r0");
        sim.step();
        assert_eq!(sim.read(&r0_out), 42);
    }

    #[test]
    fn test_when_procedural_statement_auto_latch() {
        let (_, circuit) = Circuit::elaborate(|| {
            crate::Domain::<crate::signal::Sys>::ports("clk", "rst");
            let we = Signal::<U1>::input_port_default("we");
            let wdata = Signal::<U8>::input_port_default("wdata");
            let r0_u = Signal::<U8>::default();
            let r0 = r0_u.read();

            when!(we => {
                r0_u <= wdata;
            });

            Signal::output_port("r0", r0);
        });

        let mut sim = Sim::new(&circuit);
        let we_in = sim.input::<U1>("we");
        let wdata_in = sim.input::<U8>("wdata");
        let r0_out = sim.output::<U8>("r0");

        assert_eq!(sim.read(&r0_out), 0);

        sim.drive(&wdata_in, 42);
        sim.drive(&we_in, 1);
        sim.step();
        assert_eq!(sim.read(&r0_out), 42);

        sim.drive(&wdata_in, 99);
        sim.drive(&we_in, 0);
        sim.step();
        assert_eq!(sim.read(&r0_out), 42);
    }

    #[test]
    fn test_when_procedural_statement_with_else() {
        let (_, circuit) = Circuit::elaborate(|| {
            crate::Domain::<crate::signal::Sys>::ports("clk", "rst");
            let we = Signal::<U1>::input_port_default("we");
            let wdata = Signal::<U8>::input_port_default("wdata");
            let r0_u = Signal::<U8>::default();
            let r0 = r0_u.read();

            when!(we => {
                r0_u <= wdata;
            }, else => {
                r0_u <= 7.into();
            });

            Signal::output_port("r0", r0);
        });

        let mut sim = Sim::new(&circuit);
        let we_in = sim.input::<U1>("we");
        let wdata_in = sim.input::<U8>("wdata");
        let r0_out = sim.output::<U8>("r0");

        sim.drive(&wdata_in, 50);
        sim.drive(&we_in, 1);
        sim.step();
        assert_eq!(sim.read(&r0_out), 50);

        sim.drive(&we_in, 0);
        sim.step();
        assert_eq!(sim.read(&r0_out), 7);
    }

    #[test]
    fn test_when_boolean_condition() {
        let (_, circuit) = Circuit::elaborate(|| {
            let c1 = Signal::<U1>::input_port_default("c1");
            let a = Signal::<U8>::input_port_default("a");
            let b = Signal::<U8>::input_port_default("b");

            let out = when!(c1 => a, else => b);
            Signal::output_port("out", out);
        });

        let mut sim = Sim::new(&circuit);
        let c1_in = sim.input::<U1>("c1");
        let a_in = sim.input::<U8>("a");
        let b_in = sim.input::<U8>("b");
        let out = sim.output::<U8>("out");

        sim.drive(&a_in, 10);
        sim.drive(&b_in, 20);

        sim.drive(&c1_in, 1);
        sim.settle();
        assert_eq!(sim.read(&out), 10);

        sim.drive(&c1_in, 0);
        sim.settle();
        assert_eq!(sim.read(&out), 20);
    }

    #[test]
    fn test_when_multi_guard() {
        let (_, circuit) = Circuit::elaborate(|| {
            let c1 = Signal::<U1>::input_port_default("c1");
            let c2 = Signal::<U1>::input_port_default("c2");
            let a = Signal::<U8>::input_port_default("a");
            let b = Signal::<U8>::input_port_default("b");
            let c = Signal::<U8>::input_port_default("c");

            let out = when! {
                c1 => a,
                c2 => b,
                else => c,
            };

            Signal::output_port("out", out);
        });

        let mut sim = Sim::new(&circuit);
        let c1_in = sim.input::<U1>("c1");
        let c2_in = sim.input::<U1>("c2");
        let a_in = sim.input::<U8>("a");
        let b_in = sim.input::<U8>("b");
        let c_in = sim.input::<U8>("c");
        let out = sim.output::<U8>("out");

        sim.drive(&a_in, 10);
        sim.drive(&b_in, 20);
        sim.drive(&c_in, 30);

        sim.drive(&c1_in, 1);
        sim.drive(&c2_in, 0);
        sim.settle();
        assert_eq!(sim.read(&out), 10);

        sim.drive(&c1_in, 0);
        sim.drive(&c2_in, 1);
        sim.settle();
        assert_eq!(sim.read(&out), 20);

        sim.drive(&c1_in, 0);
        sim.drive(&c2_in, 0);
        sim.settle();
        assert_eq!(sim.read(&out), 30);
    }

    #[test]
    fn test_when_case_matching() {
        let (_, circuit) = Circuit::elaborate(|| {
            let op = Signal::<U8>::input_port_default("op");
            let a = Signal::<U8>::input_port_default("a");
            let b = Signal::<U8>::input_port_default("b");

            let res = when!(op => {
                0b101 => { a + b },
                0b110 => { a - b },
                _     => { a & b },
            });

            Signal::output_port("res", res);
        });

        let mut sim = Sim::new(&circuit);
        let op_in = sim.input::<U8>("op");
        let a_in = sim.input::<U8>("a");
        let b_in = sim.input::<U8>("b");
        let res = sim.output::<U8>("res");

        sim.drive(&a_in, 15);
        sim.drive(&b_in, 5);

        sim.drive(&op_in, 0b101);
        sim.settle();
        assert_eq!(sim.read(&res), 20);

        sim.drive(&op_in, 0b110);
        sim.settle();
        assert_eq!(sim.read(&res), 10);

        sim.drive(&op_in, 0);
        sim.settle();
        assert_eq!(sim.read(&res), 5);
    }

    #[test]
    fn test_select_macro() {
        let (_, circuit) = Circuit::elaborate(|| {
            let sel = Signal::<U2>::input_port_default("sel");
            let v0 = Signal::<U8>::input_port_default("v0");
            let v1 = Signal::<U8>::input_port_default("v1");
            let v2 = Signal::<U8>::input_port_default("v2");
            let v3 = Signal::<U8>::input_port_default("v3");

            let out = select!(sel => [v0, v1, v2, v3]);
            Signal::output_port("out", out);
        });

        let mut sim = Sim::new(&circuit);
        let sel_in = sim.input::<U2>("sel");
        let v0_in = sim.input::<U8>("v0");
        let v1_in = sim.input::<U8>("v1");
        let v2_in = sim.input::<U8>("v2");
        let v3_in = sim.input::<U8>("v3");
        let out = sim.output::<U8>("out");

        sim.drive(&v0_in, 100);
        sim.drive(&v1_in, 101);
        sim.drive(&v2_in, 102);
        sim.drive(&v3_in, 103);

        sim.drive(&sel_in, 2);
        sim.settle();
        assert_eq!(sim.read(&out), 102);
    }

    #[test]
    fn test_assign_unconditional() {
        let (_, circuit) = Circuit::elaborate(|| {
            let a = Signal::<U8>::input_port_default("a");
            let w_u = Signal::<U8>::default();
            let w = w_u.read();

            when! { w_u = a }

            Signal::output_port("out", w);
        });

        let mut sim = Sim::new(&circuit);
        let a_in = sim.input::<U8>("a");
        let out = sim.output::<U8>("out");

        sim.drive(&a_in, 77);
        sim.settle();
        assert_eq!(sim.read(&out), 77);

        sim.drive(&a_in, 200);
        sim.settle();
        assert_eq!(sim.read(&out), 200);
    }

    #[test]
    fn test_assign_conditional_auto_fallback() {
        let (_, circuit) = Circuit::elaborate(|| {
            let en = Signal::<U1>::input_port_default("en");
            let a = Signal::<U8>::input_port_default("a");
            let zero = Signal::<U8>::input_port_default("zero");
            let w_u = Signal::<U8>::default();
            let w = w_u.read();

            when!(en => { w_u = a; }, else => { w_u = zero; });

            Signal::output_port("out", w);
        });

        let mut sim = Sim::new(&circuit);
        let en_in = sim.input::<U1>("en");
        let a_in = sim.input::<U8>("a");
        let zero_in = sim.input::<U8>("zero");
        let out = sim.output::<U8>("out");

        sim.drive(&zero_in, 0);

        sim.drive(&en_in, 1);
        sim.drive(&a_in, 55);
        sim.settle();
        assert_eq!(sim.read(&out), 55);

        sim.drive(&en_in, 0);
        sim.drive(&a_in, 99);
        sim.settle();
        assert_eq!(sim.read(&out), 0);
    }

    #[test]
    fn test_assign_conditional_with_else() {
        let (_, circuit) = Circuit::elaborate(|| {
            let sel = Signal::<U1>::input_port_default("sel");
            let a = Signal::<U8>::input_port_default("a");
            let b = Signal::<U8>::input_port_default("b");
            let w_u = Signal::<U8>::default();
            let w = w_u.read();

            when!(sel => { w_u = a; }, else => { w_u = b; });

            Signal::output_port("out", w);
        });

        let mut sim = Sim::new(&circuit);
        let sel_in = sim.input::<U1>("sel");
        let a_in = sim.input::<U8>("a");
        let b_in = sim.input::<U8>("b");
        let out = sim.output::<U8>("out");

        sim.drive(&a_in, 10);
        sim.drive(&b_in, 20);

        sim.drive(&sel_in, 1);
        sim.settle();
        assert_eq!(sim.read(&out), 10);

        sim.drive(&sel_in, 0);
        sim.settle();
        assert_eq!(sim.read(&out), 20);
    }
}
