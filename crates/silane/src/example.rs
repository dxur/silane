use std::ops::{Add, Sub};
use typenum::{Sum, U0, U1};

use crate::{
    signal::{Num, Signal},
    wire::{Bound, Wire},
};

type W<T> = Wire<Signal<T>, Bound>;

pub fn concat<A: Num, B: Num, C: Num>(a: W<A>, b: W<B>) -> W<C>
where
    A: Add<B, Output = C>,
{
    a.concat(b)
}

// slice takes bit range [LO, HI) out of a W-wide signal, HI - LO = OUT
pub fn slice<W_IN: Num, LO: Num + Default, HI: Num + Default, OUT: Num>(
    s: W<W_IN>,
) -> W<OUT>
where
    HI: Sub<LO, Output = OUT>,
{
    s.slice::<LO, HI, OUT>()
}

pub fn needs_c<C: Num>(_s: W<C>) {}

pub fn pipeline<A, B, C>(x: W<A>, y: W<B>) -> W<C>
where
    A: Num + Add<B, Output = C> + Sub<Sum<B, U1>>,
    B: Num + Add<U1> + Default,
    C: Num + Default,
    U0: Default,
    Sum<B, U1>: Num + Default + Sub<U0, Output = B>,
    Sum<A, B>: Num + Default + Sub<B, Output = A>,
{
    let c = concat(x, y);

    let b = slice::<C, U0, Sum<B, U1>, B>(c);
    let d = slice::<C, B, Sum<A, B>, A>(c);

    concat(d, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::Circuit;
    use typenum::{U4, U8};

    #[test]
    fn test_example_pipeline() {
        let (_, _circuit) = Circuit::elaborate(|| {
            let x = Signal::<U4>::input_port("x", U4);
            let y = Signal::<U4>::input_port("y", U4);

            let _res = pipeline::<U4, U4, U8>(x, y);
        });
    }
}
