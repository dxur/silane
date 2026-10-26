//! Memories: synchronous RAMs and ROMs, bound in `.si` by `import "std:mem";`.
//!
//! Both have a registered read port, the shape FPGA block RAMs implement: the word read
//! at a clock edge appears on `rdata` after that edge. See [`MemPrim`] for the exact
//! semantics.

use crate::circuit::Circuit;
use crate::primitive::MemPrim;
use crate::signal::{Domain, Num, Signal, Sys};
use crate::wire::{Bound, Wire};
use typenum::U1;

type W<N, Dom> = Wire<Signal<N, Dom>, Bound>;

/// Builds a memory clocked by domain `Dom` and returns its read data.
fn build<D: Num, A: Num, Dom>(
    depth: usize,
    init: &[u64],
    we: W<U1, Dom>,
    waddr: W<A, Dom>,
    wdata: W<D, Dom>,
    re: W<U1, Dom>,
    raddr: W<A, Dom>,
) -> W<D, Dom> {
    let width = wdata.width();
    assert!(depth > 0, "a memory needs at least one word");
    assert!(
        init.len() <= depth,
        "{} initial words for a memory of {depth} words",
        init.len()
    );
    if let Some(&word) = init.iter().find(|&&w| width < 64 && w >> width != 0) {
        panic!("initial word {word:#x} does not fit in {width} bits");
    }
    assert_eq!(waddr.width(), raddr.width(), "read and write addresses differ in width");
    let (clk, _) = Domain::<Dom>::nets_dirty();
    let rdata = Circuit::with_current_dirty(|c| {
        let rdata = c.new_node(width);
        c.add_primitive(MemPrim {
            clk,
            width,
            depth,
            we: we.node_dirty(),
            waddr: waddr.node_dirty(),
            wdata: wdata.node_dirty(),
            re: re.node_dirty(),
            raddr: raddr.node_dirty(),
            rdata,
            init: init.to_vec(),
        });
        rdata
    });
    Wire::new_dirty(rdata, wdata.value)
}

/// A RAM of `depth` words with one write port and one registered read port, in clock
/// domain `Dom`.
pub struct Mem<D, Dom: 'static = Sys> {
    pub rdata: W<D, Dom>,
}

// Not derived: those would require `Dom` to be `Clone`.
impl<D: Copy, Dom> Clone for Mem<D, Dom> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D: Copy, Dom> Copy for Mem<D, Dom> {}

impl<D: Num, Dom> Mem<D, Dom> {
    /// The constructor `.si` calls for `Mem::<W = width, D = depth>(...)`: the module
    /// parameters, then the inputs.
    ///
    /// # Panics
    /// If `width` is not the width of `wdata`.
    pub fn new<A: Num>(
        width: u64,
        depth: u64,
        we: W<U1, Dom>,
        waddr: W<A, Dom>,
        wdata: W<D, Dom>,
        re: W<U1, Dom>,
        raddr: W<A, Dom>,
    ) -> Self {
        assert_eq!(width as usize, wdata.width(), "memory width parameter");
        Self::with_init(depth as usize, &[], we, waddr, wdata, re, raddr)
    }

    /// A RAM whose first words start as `init`.
    pub fn with_init<A: Num>(
        depth: usize,
        init: &[u64],
        we: W<U1, Dom>,
        waddr: W<A, Dom>,
        wdata: W<D, Dom>,
        re: W<U1, Dom>,
        raddr: W<A, Dom>,
    ) -> Self {
        Mem {
            rdata: build(depth, init, we, waddr, wdata, re, raddr),
        }
    }
}

/// A read-only memory holding `data`, with a registered read port, in clock domain
/// `Dom`.
pub struct Rom<D, Dom: 'static = Sys> {
    pub rdata: W<D, Dom>,
}

impl<D: Copy, Dom> Clone for Rom<D, Dom> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D: Copy, Dom> Copy for Rom<D, Dom> {}

impl<D: Num, Dom> Rom<D, Dom> {
    /// # Panics
    /// If a word of `data` does not fit in `width` bits.
    pub fn new<A: Num>(width: D, data: &[u64], re: W<U1, Dom>, raddr: W<A, Dom>) -> Self {
        let we = Signal::<U1, Dom>::constant(0, U1::default());
        let waddr = Signal::<A, Dom>::constant(0, raddr.value.width);
        let wdata = Signal::<D, Dom>::constant(0, width);
        Rom {
            rdata: build(data.len().max(1), data, we, waddr, wdata, re, raddr),
        }
    }
}
