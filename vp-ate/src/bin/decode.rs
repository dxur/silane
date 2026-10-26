//! Decodes the first key frame of an IVF file with the software model and writes it
//! as raw I420 (`width x height`, planar Y, U, V).
//!
//! `cargo run --bin decode -- in.ivf out.yuv`

use vp_ate::model::{self, ivf::IVFParser};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = &args[..] else {
        eprintln!("usage: decode <in.ivf> <out.yuv>");
        std::process::exit(2);
    };
    let data = std::fs::read(input).expect("cannot read input");
    let mut ivf = IVFParser::new(&data).expect("not an IVF file");
    let frame = ivf.next_frame().expect("bad IVF frame").expect("no frames");
    let decoded = model::decode(frame.data).unwrap_or_else(|e| {
        eprintln!("{input}: {e}");
        std::process::exit(1);
    });
    std::fs::write(output, decoded.to_i420()).expect("cannot write output");
    let h = &decoded.header;
    println!("{}x{} q={} filter_level={}", h.width, h.height, h.q_index, h.filter_level);
}
