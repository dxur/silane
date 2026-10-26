//! The whole decoder builds as a native simulation.

#[test]
fn decoder_compiles_natively() {
    let t = std::time::Instant::now();
    let circuit = vp_ate::top::VpAte::circuit();
    eprintln!("elaborated: {} nodes in {:.1?}", circuit.nodes, t.elapsed());
    let t = std::time::Instant::now();
    circuit.combinational_order().unwrap();
    eprintln!("ordered in {:.1?}", t.elapsed());
    let t = std::time::Instant::now();
    let sim = silane::Sim::new(&circuit);
    eprintln!("sim built in {:.1?}, native: {}", t.elapsed(), sim.is_native());
    assert!(sim.is_native());
}
