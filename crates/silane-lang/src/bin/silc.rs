//! `silc`: command-line driver for the Silane language compiler.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: silc <command> <file.si> [options]

commands:
  check     type-check and report diagnostics
  rust      print the generated Rust code
  verilog   print Verilog for a module
  netlist   print Yosys JSON netlist for a module

options:
  -o <path>      write output to a file instead of stdout
  --top <name>   module to emit (verilog/netlist; default: the last module)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut command = None;
    let mut file = None;
    let mut out: Option<PathBuf> = None;
    let mut top: Option<String> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-o" => out = it.next().map(PathBuf::from),
            "--top" => top = it.next(),
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if command.is_none() => command = Some(arg),
            _ if file.is_none() => file = Some(PathBuf::from(arg)),
            _ => {
                eprintln!("unexpected argument `{arg}`\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }
    let (Some(command), Some(file)) = (command, file) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    if !matches!(command.as_str(), "check" | "rust" | "verilog" | "netlist") {
        eprintln!("unknown command `{command}`\n\n{USAGE}");
        return ExitCode::FAILURE;
    }

    let compilation = silane_lang::compile_file(&file);
    eprint!("{}", compilation.render_diagnostics());
    let Some(design) = &compilation.design else {
        eprintln!(
            "error: could not compile `{}` ({} error(s))",
            file.display(),
            compilation.diagnostics.error_count()
        );
        return ExitCode::FAILURE;
    };

    let output = match command.as_str() {
        "check" => {
            eprintln!(
                "ok: {} module instance(s) in `{}`",
                design.modules.len(),
                file.display()
            );
            return ExitCode::SUCCESS;
        }
        "rust" => compilation.rust().unwrap_or_default(),
        _ => {
            let name = match top {
                Some(t) => t,
                None => match design.modules.last() {
                    Some(m) => m.rust_name.clone(),
                    None => {
                        eprintln!("error: `{}` defines no modules", file.display());
                        return ExitCode::FAILURE;
                    }
                },
            };
            let Some(circuit) = compilation.circuit(&name) else {
                let names: Vec<&str> = design.modules.iter().map(|m| m.rust_name.as_str()).collect();
                eprintln!("error: no module `{name}` (available: {})", names.join(", "));
                return ExitCode::FAILURE;
            };
            if command == "verilog" {
                circuit.to_verilog(&name)
            } else {
                circuit.to_netlist_json_string()
            }
        }
    };
    match out {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, output) {
                eprintln!("error: cannot write `{}`: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
        None => print!("{output}"),
    }
    ExitCode::SUCCESS
}
