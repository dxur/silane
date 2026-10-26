//! Builds the assembly test programs with clang and mold, and runs them.

#![allow(dead_code)]

use rv_olt::elf::Elf;
use rv_olt::machine::{Exit, Machine};
use std::path::PathBuf;
use std::process::Command;

/// Assembles and links `tests/asm/<name>.S`; returns the ELF image.
pub fn build(name: &str) -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/asm");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("asm");
    std::fs::create_dir_all(&out).unwrap();
    let obj = out.join(format!("{name}.o"));
    let elf = out.join(format!("{name}.elf"));
    let cc = std::env::var("RV_CC").unwrap_or_else(|_| "clang".into());
    let ld = std::env::var("RV_LD").unwrap_or_else(|_| "ld.mold".into());
    let st = Command::new(&cc)
        .args(["--target=riscv64", "-march=rv64gc", "-mabi=lp64d", "-mno-relax", "-c"])
        .arg("-I")
        .arg(&dir)
        .arg(dir.join(format!("{name}.S")))
        .arg("-o")
        .arg(&obj)
        .status()
        .unwrap_or_else(|e| panic!("running {cc}: {e}"));
    assert!(st.success(), "assembling {name}.S failed");
    let st = Command::new(&ld)
        .args(["-m", "elf64lriscv", "-static", "-nostdlib", "-e", "_start", "--image-base=0x80000000", "--no-rosegment", "-z", "norelro"])
        .arg(&obj)
        .arg("-o")
        .arg(&elf)
        .status()
        .unwrap_or_else(|e| panic!("running {ld}: {e}"));
    assert!(st.success(), "linking {name} failed");
    std::fs::read(&elf).unwrap()
}

/// Runs test program `name` to completion and checks that it passes.
pub fn run(name: &str, max_cycles: u64) -> Machine {
    run_with(name, max_cycles, |_| {})
}

/// Like [`run`], with `setup` applied to the machine before it starts.
pub fn run_with(name: &str, max_cycles: u64, setup: impl FnOnce(&mut Machine)) -> Machine {
    let elf = Elf::parse(&build(name)).unwrap();
    let mut m = Machine::new(16 << 20);
    m.trace = std::env::var("RV_TRACE").is_ok();
    m.load_elf(&elf);
    setup(&mut m);
    let exit = m.run(max_cycles);
    assert_eq!(
        exit,
        Exit::Pass,
        "{name}: {exit:?} after {} cycles, {} instructions, pc {:#x}; console: {:?}",
        m.cycles,
        m.instret,
        m.pc(),
        String::from_utf8_lossy(&m.console)
    );
    eprintln!("{name}: passed in {} cycles, {} instructions", m.cycles, m.instret);
    m
}
