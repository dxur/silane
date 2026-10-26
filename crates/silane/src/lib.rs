pub mod cdc;
pub mod circuit;
pub mod dsl;
pub mod export;
pub mod mem;
pub mod primitive;
pub mod signal;
pub mod sim;
pub mod timing;
pub mod wire;

pub use circuit::Circuit;
pub use primitive::Primitive;
pub use signal::{Domain, IntoWire, Num, Signal, Sys};
pub use sim::Sim;
pub use wire::{Bound, UnBound, Wire};

pub use typenum;

/// Includes Rust code generated from a `.si` file by `silane_lang::build` in a build
/// script. The argument is the source file's stem:
///
/// ```ignore
/// mod fifo {
///     silane::include_si!("fifo");
/// }
/// ```
#[macro_export]
macro_rules! include_si {
    ($name:literal) => {
        include!(concat!(env!("OUT_DIR"), "/silane/", $name, ".rs"));
    };
}
