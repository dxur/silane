use silane::{
    circuit::Circuit,
    signal::Signal,
    sim::{Sim, Waveform},
    when,
    wire::Wire,
};
use typenum::{U1, U3, U8};

// 8-slot depth (3-bit pointers), 8-bit data width
type W1 = Wire<Signal<U1>>;
type W8 = Wire<Signal<U8>>;

pub struct FifoOut {
    pub read_data: W8,
    pub full: W1,
    pub empty: W1,
}

pub fn fifo(wr_en: W1, rd_en: W1, write_data: W8) -> FifoOut {
    // Internal state: 8x8 register file, 3-bit pointers, and 4-bit item count
    let mem = [
        Signal::<U8>::default(),
        Signal::<U8>::default(),
        Signal::<U8>::default(),
        Signal::<U8>::default(),
        Signal::<U8>::default(),
        Signal::<U8>::default(),
        Signal::<U8>::default(),
        Signal::<U8>::default(),
    ];

    let memr = mem.iter().map(|s| s.read()).collect::<Vec<_>>();

    let wr_ptr = Signal::<U3>::default();
    let rd_ptr = Signal::<U3>::default();
    let count = Signal::<typenum::U4>::default(); // Range 0..8

    // Status flags
    let empty = count.eq(0u64);
    let full = count.eq(8u64);

    // Operational handshakes
    let do_write = wr_en & !full;
    let do_read = rd_en & !empty;

    // Memory write-back pass
    for (i, s) in mem.into_iter().enumerate() {
        let is_target_slot = wr_ptr.eq(i as u64);
        let wd = when!(do_write & is_target_slot => write_data, else => *s);
        when! { s <= wd }
    }

    // Dynamic Read Output Routing
    let read_data = when!(rd_ptr => {
        0 => memr[0],
        1 => memr[1],
        2 => memr[2],
        3 => memr[3],
        4 => memr[4],
        5 => memr[5],
        6 => memr[6],
        _ => memr[7],
    });

    // Pointer and Count updates
    when! { wr_ptr <= when!(do_write => *wr_ptr + 1u64, else => *wr_ptr) }
    when! { rd_ptr <= when!(do_read  => *rd_ptr + 1u64, else => *rd_ptr) }

    let count_next = when!(do_write & !do_read => *count + 1u64,
                           else => when!(do_read & !do_write => *count - 1u64,
                           else => *count));
    when! { count <= count_next }

    FifoOut {
        read_data,
        full,
        empty,
    }
}

fn main() {
    let (top, mut circuit) = Circuit::elaborate(|| {
        silane::Domain::<silane::Sys>::ports("clk", "rst");
        fifo(
            Signal::input_port_default("wr_en"),
            Signal::input_port_default("rd_en"),
            Signal::input_port_default("write_data"),
        )
    });

    circuit.output_port("read_data", top.read_data);
    circuit.output_port("full", top.full);
    circuit.output_port("empty", top.empty);

    circuit.validate().expect("Circuit validation failed!");
    circuit.emit_verilog_file("fifo", "fifo.v").unwrap();

    // Simulation & Verification Pass
    let mut sim = Sim::new(&circuit);
    let wr_en_in = sim.input::<U1>("wr_en");
    let rd_en_in = sim.input::<U1>("rd_en");
    let data_in = sim.input::<U8>("write_data");

    let mut wave = Waveform::new();
    wave.probe(&sim, "full", &sim.output::<U1>("full"))
        .probe(&sim, "empty", &sim.output::<U1>("empty"))
        .probe(&sim, "read_data", &sim.output::<U8>("read_data"));

    println!("--- FIFO Execution Testbench ---");
    for cycle in 0..15 {
        // Push 0xA1, 0xB2, 0xC3 into FIFO on cycles 1..3
        if cycle >= 1 && cycle <= 3 {
            sim.drive(&wr_en_in, 1);
            sim.drive(&rd_en_in, 0);
            sim.drive(&data_in, match cycle { 1 => 0xA1, 2 => 0xB2, _ => 0xC3 });
        } 
        // Pop data back out on cycles 5..7
        else if cycle >= 5 && cycle <= 7 {
            sim.drive(&wr_en_in, 0);
            sim.drive(&rd_en_in, 1);
            sim.drive(&data_in, 0);
        } else {
            sim.drive(&wr_en_in, 0);
            sim.drive(&rd_en_in, 0);
            sim.drive(&data_in, 0);
        }

        wave.capture(&sim, cycle);
        sim.step();
    }

    wave.print();
}