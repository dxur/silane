//! Writes the SoC, or one of its parts, as Verilog:
//! `cargo run --release --bin export [module] [out.v]`, with `module` one of `soc`
//! (the default), `core`, `fpu`, or one block: `csr`, `mmu`, `muldiv`, `alu`, `decode`,
//! `rvc`, `plic`, `clint`, `uart`.
//!
//! The SoC's top-level ports: `clk`, `rst` (active high, synchronous), the RAM port
//! (`ram_req`/`ram_addr`/`ram_we`/`ram_wdata`/`ram_be` out, `ram_ack`/`ram_err`/
//! `ram_rdata` in: hold the request until acknowledged), the UART byte streams
//! (`tx_valid`/`tx_data`, `rx_valid`/`rx_data`/`rx_ready`), `rtc_tick` (mtime
//! advances on each cycle it is high), `boot_entry` and `dtb_addr` (where the boot
//! ROM jumps and the device tree it passes), the test device write (`test_*`) and
//! debug outputs (`dbg_*`).

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).map_or("soc", String::as_str);
    let path = args.get(2).cloned().unwrap_or_else(|| format!("{name}.v"));
    let circuit = match name {
        "soc" => rv_olt::soc::Soc::circuit(),
        "core" => rv_olt::cpu::Core::circuit(),
        "fpu" => rv_olt::fpu::Fpu::circuit(),
        "csr" => rv_olt::parts::csr::Csr::circuit(),
        "mmu" => rv_olt::parts::mmu::Ptw::circuit(),
        "muldiv" => rv_olt::parts::muldiv::MulDiv::circuit(),
        "alu" => rv_olt::parts::alu::Alu::circuit(),
        "decode" => rv_olt::parts::decode::Decode::circuit(),
        "rvc" => rv_olt::parts::rvc::Rvc::circuit(),
        "plic" => rv_olt::parts::devices::Plic::circuit(),
        "clint" => rv_olt::parts::devices::Clint::circuit(),
        "uart" => rv_olt::parts::devices::Uart::circuit(),
        other => panic!("unknown module `{other}`"),
    };
    circuit.validate().expect("invalid circuit");
    if path == "paths" {
        let n = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20);
        for p in silane::timing::critical_paths(&circuit, n) {
            println!("{}", silane::timing::describe(&circuit, &p));
        }
        return;
    }
    std::fs::write(&path, circuit.to_verilog(name)).expect("cannot write Verilog");
    println!("{path}: {} nodes, {} primitives", circuit.nodes, circuit.primitives.len());
}
