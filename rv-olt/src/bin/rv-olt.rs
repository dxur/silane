//! Boots the simulated SoC: firmware, kernel, device tree and an optional initramfs,
//! with the UART on this terminal.
//!
//! ```text
//! cargo run --release --bin rv-olt -- --fw fw_jump.bin --kernel Image [--initrd rootfs.cpio]
//! ```
//!
//! When stdin is a terminal it is put in raw mode, so keys go to the guest; type
//! Ctrl-A then `x` to quit. `--send-after <pattern> <text>` types `<text>` once the
//! console has printed `<pattern>` (repeatable, in order), for scripted sessions.
//!
//! Memory layout (OpenSBI fw_jump's defaults): firmware at 0x8000_0000, kernel at
//! 0x8020_0000, device tree at 0x8220_0000, initramfs at 0x8400_0000 (RAM: 128 MiB).

use rv_olt::machine::{Exit, Machine, RAM_BASE};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Instant;

const KERNEL_ADDR: u64 = 0x8020_0000;
const DTB_ADDR: u64 = 0x8220_0000;
const INITRD_ADDR: u64 = 0x8400_0000;
const RAM_BYTES: usize = 128 << 20;

struct Args {
    fw: String,
    kernel: Option<String>,
    initrd: Option<String>,
    dts: String,
    bootargs: Option<String>,
    max_cycles: u64,
    tick_every: u64,
    trace_from: Option<u64>,
    send_after: Vec<(String, String)>,
}

fn usage() -> ! {
    eprintln!(
        "usage: rv-olt --fw <fw_jump.bin> [--kernel <Image>] [--initrd <cpio>] [--dts <file>]\n\
         \x20             [--bootargs <args>] [--max-cycles <n>] [--tick <cycles>] [--trace-from <instret>]\n\
         \x20             [--send-after <pattern> <text>]..."
    );
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut a = Args {
        fw: String::new(),
        kernel: None,
        initrd: None,
        dts: concat!(env!("CARGO_MANIFEST_DIR"), "/dts/rv-olt.dts").into(),
        bootargs: None,
        max_cycles: u64::MAX,
        tick_every: 10,
        trace_from: None,
        send_after: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--fw" => a.fw = val(),
            "--kernel" => a.kernel = Some(val()),
            "--initrd" => a.initrd = Some(val()),
            "--dts" => a.dts = val(),
            "--bootargs" => a.bootargs = Some(val()),
            "--max-cycles" => a.max_cycles = val().parse().unwrap_or_else(|_| usage()),
            "--tick" => a.tick_every = val().parse().unwrap_or_else(|_| usage()),
            "--trace-from" => a.trace_from = Some(val().parse().unwrap_or_else(|_| usage())),
            "--send-after" => {
                let pat = val();
                let text = val().replace("\\n", "\n");
                a.send_after.push((pat, text));
            }
            _ => usage(),
        }
    }
    if a.fw.is_empty() {
        usage();
    }
    a
}

/// Compiles the device tree with dtc, adding the initramfs location and bootargs.
fn build_dtb(dts: &str, initrd: Option<(u64, u64)>, bootargs: Option<&str>) -> Vec<u8> {
    let mut src = std::fs::read_to_string(dts).unwrap_or_else(|e| panic!("reading {dts}: {e}"));
    src.push_str("\n/ {\n    chosen {\n");
    if let Some((start, end)) = initrd {
        src.push_str(&format!(
            "        linux,initrd-start = <0x{:x} 0x{:x}>;\n        linux,initrd-end = <0x{:x} 0x{:x}>;\n",
            start >> 32,
            start as u32,
            end >> 32,
            end as u32
        ));
    }
    if let Some(b) = bootargs {
        src.push_str(&format!("        bootargs = \"{b}\";\n"));
    }
    src.push_str("    };\n};\n");
    let mut child = Command::new("dtc")
        .args(["-q", "-I", "dts", "-O", "dtb", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("running dtc");
    use std::io::Write;
    child.stdin.take().unwrap().write_all(src.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "dtc failed");
    out.stdout
}

fn main() {
    let args = parse_args();
    let read = |p: &str| std::fs::read(p).unwrap_or_else(|e| panic!("reading {p}: {e}"));

    let t0 = Instant::now();
    let mut m = Machine::new(RAM_BYTES);
    eprintln!("[rv-olt] simulator built in {:.1?}", t0.elapsed());
    m.echo = true;
    m.tick_every = args.tick_every;

    m.load(RAM_BASE, &read(&args.fw));
    if let Some(k) = &args.kernel {
        m.load(KERNEL_ADDR, &read(k));
    }
    let initrd = args.initrd.as_ref().map(|p| {
        let data = read(p);
        m.load(INITRD_ADDR, &data);
        (INITRD_ADDR, INITRD_ADDR + data.len() as u64)
    });
    let dtb = build_dtb(&args.dts, initrd, args.bootargs.as_deref());
    m.load(DTB_ADDR, &dtb);
    m.boot(RAM_BASE, DTB_ADDR);

    // Console input: a thread forwards stdin bytes. On a terminal, raw mode (restored
    // on exit); Ctrl-A x quits.
    let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if tty {
        let _ = Command::new("stty").args(["raw", "-echo"]).stdin(Stdio::inherit()).status();
    }
    let (tx, rx) = mpsc::channel::<u8>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        let mut stdin = std::io::stdin();
        let mut escape = false;
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 {
                break;
            }
            for &b in &buf[..n] {
                if escape && b == b'x' {
                    restore_tty(tty);
                    eprintln!("\r\n[rv-olt] quit");
                    std::process::exit(0);
                }
                escape = b == 0x01;
                if escape {
                    continue;
                }
                if tx.send(if b == b'\n' { b'\r' } else { b }).is_err() {
                    return;
                }
            }
        }
    });
    let mut script = args.send_after.clone().into_iter().peekable();
    let mut seen = 0;

    let start = Instant::now();
    let mut last = Instant::now();
    let mut last_cycles = 0;
    let exit = loop {
        if m.cycles >= args.max_cycles {
            break Exit::Timeout;
        }
        if let Some(e) = m.cycle() {
            break e;
        }
        if m.cycles % 65536 == 0 {
            while let Ok(b) = rx.try_recv() {
                m.send(&[b]);
            }
            if let Some((pat, _)) = script.peek()
                && let Some(i) = find(&m.console[seen..], pat.as_bytes())
            {
                seen += i + pat.len();
                let (_, text) = script.next().unwrap();
                m.send(text.as_bytes());
            }
            if let Some(t) = args.trace_from {
                m.trace = m.instret >= t;
            }
            if last.elapsed().as_secs() >= 10 {
                let rate = (m.cycles - last_cycles) as f64 / last.elapsed().as_secs_f64();
                eprintln!(
                    "\r\n[rv-olt] {:.0}s: {} cycles, {} instructions, pc {:#x}, {:.0} kHz",
                    start.elapsed().as_secs_f64(),
                    m.cycles,
                    m.instret,
                    m.pc(),
                    rate / 1e3
                );
                last = Instant::now();
                last_cycles = m.cycles;
            }
        }
    };
    restore_tty(tty);
    eprintln!(
        "\r\n[rv-olt] {exit:?} after {} cycles, {} instructions, {:.1?} ({:.0} kHz)",
        m.cycles,
        m.instret,
        start.elapsed(),
        m.cycles as f64 / start.elapsed().as_secs_f64() / 1e3
    );
}

fn restore_tty(tty: bool) {
    if tty {
        let _ = Command::new("stty").arg("sane").stdin(Stdio::inherit()).status();
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
