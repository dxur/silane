//! Writes the decoder, or one of its parts, as Verilog:
//! `cargo run --release --bin export [module] [out.v]`, with `module` one of `vp_ate`
//! (the default), `headers`, `tokens`, `recon` or `bool_decoder`.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).map_or("vp_ate", String::as_str);
    let path = args.get(2).cloned().unwrap_or_else(|| format!("{name}.v"));
    let circuit = match name {
        "vp_ate" => vp_ate::top::VpAte::circuit(),
        "headers" => vp_ate::headers::Headers::circuit(),
        "tokens" => vp_ate::tokens::Tokens::circuit(),
        "recon" => vp_ate::recon::Recon::circuit(),
        "bool_decoder" => vp_ate::bool_decoder::BoolDecoder::circuit(),
        other => panic!("unknown module `{other}`"),
    };
    circuit.validate().expect("invalid circuit");
    std::fs::write(&path, circuit.to_verilog(name)).expect("cannot write Verilog");
    println!("{path}: {} nodes, {} primitives", circuit.nodes, circuit.primitives.len());
}
