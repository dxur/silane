//! The simulated machine: the SoC's RTL in silane's simulator, with RAM, a console
//! and a timer tick supplied from Rust.

use crate::elf::Elf;
use crate::soc::Soc;
use silane::Sim;
use silane::sim::{SimInput, SimOutput};
use silane::typenum::{U1, U8, U32, U64};
use std::collections::VecDeque;

pub const RAM_BASE: u64 = 0x8000_0000;

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The test device was told PASS (0x5555, also the poweroff of `syscon-poweroff`).
    Pass,
    /// The test device was told FAIL (`code << 16 | 0x3333`).
    Fail(u32),
    /// The test device was told RESET (0x7777, `syscon-reboot`).
    Reset,
    /// The cycle budget ran out.
    Timeout,
}

struct Ports {
    rst: SimInput<U1>,
    ram_ack: SimInput<U1>,
    ram_err: SimInput<U1>,
    ram_rdata: SimInput<U64>,
    rx_valid: SimInput<U1>,
    rx_data: SimInput<U8>,
    rtc_tick: SimInput<U1>,
    boot_entry: SimInput<U64>,
    dtb_addr: SimInput<U64>,
    ram_req: SimOutput<U1>,
    ram_addr: SimOutput<U64>,
    ram_we: SimOutput<U1>,
    ram_wdata: SimOutput<U64>,
    ram_be: SimOutput<U8>,
    rx_ready: SimOutput<U1>,
    tx_valid: SimOutput<U1>,
    tx_data: SimOutput<U8>,
    test_valid: SimOutput<U1>,
    test_data: SimOutput<U32>,
    dbg_retire: SimOutput<U1>,
    dbg_trap: SimOutput<U1>,
    dbg_pc: SimOutput<U64>,
    dbg_insn: SimOutput<U32>,
}

pub struct Machine {
    sim: Sim,
    p: Ports,
    /// RAM from `RAM_BASE`, in 64-bit words.
    pub ram: Vec<u64>,
    pub cycles: u64,
    pub instret: u64,
    /// mtime advances once every `tick_every` cycles.
    pub tick_every: u64,
    /// Everything the UART transmitted.
    pub console: Vec<u8>,
    /// Copy UART output to stdout as it arrives.
    pub echo: bool,
    /// Print every retired instruction (and trap) to stderr.
    pub trace: bool,
    rx: VecDeque<u8>,
    /// Signals printed every cycle while tracing: `RV_PROBE` names them, comma
    /// separated (matching the end of a node's hierarchical name).
    probes: Vec<(String, usize)>,
}

fn byte_mask(be: u64) -> u64 {
    (0..8).filter(|i| be >> i & 1 != 0).fold(0, |m, i| m | 0xff << (8 * i))
}

impl Machine {
    /// A machine with `ram_bytes` of RAM, reset, about to run the boot ROM, which jumps
    /// to `RAM_BASE` with `a1 = 0` until [`Machine::boot`] says otherwise.
    pub fn new(ram_bytes: usize) -> Machine {
        let circuit = Soc::circuit();
        let mut probes = Vec::new();
        if let Ok(names) = std::env::var("RV_PROBE") {
            for want in names.split(',').filter(|n| !n.is_empty()) {
                let mut found: Vec<_> = circuit
                    .node_names
                    .iter()
                    .filter(|(_, n)| *n == want || n.ends_with(&format!(".{want}")))
                    .map(|(&id, n)| (n.clone(), id))
                    .collect();
                found.sort();
                if found.is_empty() {
                    eprintln!("RV_PROBE: no node named {want}");
                }
                probes.extend(found.into_iter().take(1));
            }
        }
        let sim = Sim::new(&circuit);
        let p = Ports {
            rst: sim.input("rst"),
            ram_ack: sim.input("ram_ack"),
            ram_err: sim.input("ram_err"),
            ram_rdata: sim.input("ram_rdata"),
            rx_valid: sim.input("rx_valid"),
            rx_data: sim.input("rx_data"),
            rtc_tick: sim.input("rtc_tick"),
            boot_entry: sim.input("boot_entry"),
            dtb_addr: sim.input("dtb_addr"),
            ram_req: sim.output("ram_req"),
            ram_addr: sim.output("ram_addr"),
            ram_we: sim.output("ram_we"),
            ram_wdata: sim.output("ram_wdata"),
            ram_be: sim.output("ram_be"),
            rx_ready: sim.output("rx_ready"),
            tx_valid: sim.output("tx_valid"),
            tx_data: sim.output("tx_data"),
            test_valid: sim.output("test_valid"),
            test_data: sim.output("test_data"),
            dbg_retire: sim.output("dbg_retire"),
            dbg_trap: sim.output("dbg_trap"),
            dbg_pc: sim.output("dbg_pc"),
            dbg_insn: sim.output("dbg_insn"),
        };
        let mut m = Machine {
            sim,
            p,
            ram: vec![0; ram_bytes / 8],
            cycles: 0,
            instret: 0,
            tick_every: 10,
            console: Vec::new(),
            echo: false,
            trace: false,
            rx: VecDeque::new(),
            probes,
        };
        m.boot(RAM_BASE, 0);
        m.sim.drive(&m.p.rst, 1);
        m.sim.step();
        m.sim.drive(&m.p.rst, 0);
        m.sim.settle();
        m
    }

    /// Sets where the boot ROM jumps, and the device tree address it passes in a1.
    pub fn boot(&mut self, entry: u64, dtb: u64) {
        self.sim.drive(&self.p.boot_entry, entry);
        self.sim.drive(&self.p.dtb_addr, dtb);
    }

    /// Copies `data` into RAM at physical address `addr`.
    pub fn load(&mut self, addr: u64, data: &[u8]) {
        assert!(
            addr >= RAM_BASE && addr + data.len() as u64 <= RAM_BASE + 8 * self.ram.len() as u64,
            "load of {} bytes at {addr:#x} is outside RAM",
            data.len()
        );
        for (i, &b) in data.iter().enumerate() {
            let a = addr - RAM_BASE + i as u64;
            let (w, sh) = ((a / 8) as usize, (a % 8) * 8);
            self.ram[w] = self.ram[w] & !(0xff << sh) | (b as u64) << sh;
        }
    }

    /// Reads `len` bytes of RAM at `addr`.
    pub fn read(&self, addr: u64, len: usize) -> Vec<u8> {
        (0..len as u64)
            .map(|i| {
                let a = addr - RAM_BASE + i;
                (self.ram[(a / 8) as usize] >> (a % 8 * 8)) as u8
            })
            .collect()
    }

    /// Loads an ELF executable and makes the boot ROM jump to its entry point.
    pub fn load_elf(&mut self, elf: &Elf) {
        for s in &elf.segments {
            self.load(s.addr, &s.data);
        }
        self.boot(elf.entry, 0);
    }

    /// Queues bytes for the UART to receive.
    pub fn send(&mut self, bytes: &[u8]) {
        self.rx.extend(bytes);
    }

    /// Simulates one clock cycle; returns an exit if the test device was written.
    pub fn cycle(&mut self) -> Option<Exit> {
        let (sim, p) = (&mut self.sim, &self.p);
        // Outputs are settled: they depend on registers only.
        let mut exit = None;
        if sim.read(&p.ram_req) == 1 {
            let addr = sim.read(&p.ram_addr);
            let w = ((addr.wrapping_sub(RAM_BASE)) / 8) as usize;
            if addr < RAM_BASE || w >= self.ram.len() {
                sim.drive(&p.ram_err, 1);
            } else {
                sim.drive(&p.ram_err, 0);
                if sim.read(&p.ram_we) == 1 {
                    let m = byte_mask(sim.read(&p.ram_be));
                    self.ram[w] = self.ram[w] & !m | sim.read(&p.ram_wdata) & m;
                } else {
                    sim.drive(&p.ram_rdata, self.ram[w]);
                }
            }
            sim.drive(&p.ram_ack, 1);
        } else {
            sim.drive(&p.ram_ack, 0);
        }

        if sim.read(&p.tx_valid) == 1 {
            let c = sim.read(&p.tx_data) as u8;
            self.console.push(c);
            if self.echo {
                use std::io::Write;
                let mut out = std::io::stdout();
                let _ = out.write_all(&[c]);
                let _ = out.flush();
            }
        }
        if sim.read(&p.test_valid) == 1 {
            let v = sim.read(&p.test_data) as u32;
            exit = match v & 0xffff {
                0x5555 => Some(Exit::Pass),
                0x3333 => Some(Exit::Fail(v >> 16)),
                0x7777 => Some(Exit::Reset),
                _ => None,
            };
        }
        let rx = sim.read(&p.rx_ready) == 1 && !self.rx.is_empty();
        sim.drive(&p.rx_valid, rx as u64);
        if rx {
            sim.drive(&p.rx_data, self.rx.pop_front().unwrap() as u64);
        }
        sim.drive(&p.rtc_tick, (self.cycles % self.tick_every == 0) as u64);

        if self.trace && !self.probes.is_empty() {
            let vals: Vec<String> = self
                .probes
                .iter()
                .map(|(n, id)| format!("{}={:x}", n.rsplit('.').next().unwrap(), sim.node_value(*id)))
                .collect();
            eprintln!("  [{}] {}", self.cycles, vals.join(" "));
        }
        if sim.read(&p.dbg_retire) == 1 {
            self.instret += 1;
            if self.trace {
                eprintln!("{:016x}: {:08x}", sim.read(&p.dbg_pc), sim.read(&p.dbg_insn));
            }
        } else if self.trace && sim.read(&p.dbg_trap) == 1 {
            eprintln!("{:016x}: {:08x}  trap", sim.read(&p.dbg_pc), sim.read(&p.dbg_insn));
        }

        sim.step();
        self.cycles += 1;
        exit
    }

    /// Runs until the test device is written or `max_cycles` have passed.
    pub fn run(&mut self, max_cycles: u64) -> Exit {
        let end = self.cycles + max_cycles;
        while self.cycles < end {
            if let Some(e) = self.cycle() {
                return e;
            }
        }
        Exit::Timeout
    }

    /// The program counter of the instruction in flight.
    pub fn pc(&self) -> u64 {
        self.sim.read(&self.p.dbg_pc)
    }
}
