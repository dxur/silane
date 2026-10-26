//! Synchronization primitives, bound in `.si` by `import "std:cdc";`.
//!
//! These are the only places where a signal changes clock domain: everywhere else,
//! [`Signal`]'s domain parameter keeps domains apart.

use crate::circuit::Circuit;
use crate::primitive::DffPrim;
use crate::signal::{Domain, Signal, Sys};
use crate::wire::{Bound, Wire};
use std::marker::PhantomData;
use typenum::U1;

type W<N, D> = Wire<Signal<N, D>, Bound>;

/// Two-register synchronizer: brings a bit of domain `A` (or from outside the chip)
/// into domain `B`.
pub struct Sync2<A: 'static = Sys, B: 'static = Sys> {
    pub q: W<U1, B>,
    from: PhantomData<fn() -> A>,
}

// Not derived: those would require the domains to be `Clone`.
impl<A, B> Clone for Sync2<A, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A, B> Copy for Sync2<A, B> {}

impl<A, B> Sync2<A, B> {
    pub fn new(d: W<U1, A>) -> Self {
        let stage1 = Signal::<U1, B>::default().named("sync_stage1");
        let stage2 = Signal::<U1, B>::default().named("sync_stage2");
        let (s1, s2) = (stage1.read(), stage2.read());
        // The first stage samples `d` in `B`: the crossing itself, which typed signals
        // cannot express.
        let (clk, rst) = Domain::<B>::nets_dirty();
        Circuit::with_current_dirty(|c| {
            c.add_primitive(DffPrim {
                clk,
                rst,
                reset_val: 0,
                d: d.node_dirty(),
                q: stage1.node_dirty(),
            });
            c.mark_synchronizer(stage1.node_dirty());
        });
        stage2.next(s1);
        Sync2 { q: s2, from: PhantomData }
    }
}
