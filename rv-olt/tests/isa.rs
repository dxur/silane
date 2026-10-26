//! ISA tests: self-checking programs in tests/asm, run on the simulated SoC.

mod common;

#[test]
fn smoke() {
    common::run("smoke", 10_000);
}

#[test]
fn rv64i_alu() {
    common::run("rv64i_alu", 2_000_000);
}

#[test]
fn rv64i_mem() {
    common::run("rv64i_mem", 200_000);
}

#[test]
fn rv64m() {
    common::run("rv64m", 2_000_000);
}

#[test]
fn rv64a() {
    common::run("rv64a", 200_000);
}

#[test]
fn rvc() {
    common::run("rvc", 200_000);
}

#[test]
fn privileged() {
    common::run("priv", 400_000);
}

#[test]
fn mmu() {
    common::run("mmu", 400_000);
}

#[test]
fn interrupts() {
    let m = common::run_with("irq", 400_000, |m| m.send(b"Z"));
    assert_eq!(m.console, b"ok");
}

#[test]
fn rv64fd() {
    common::run("rv64fd", 20_000_000);
}

#[test]
fn hazards() {
    common::run("hazards", 100_000);
}
