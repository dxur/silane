//! A VP8 key-frame decoder in silane, and the software model it is checked against.

pub mod model;
pub mod rtl;

pub mod bool_decoder {
    silane::include_si!("bool_decoder");
}
pub mod headers {
    silane::include_si!("headers");
}
pub mod tokens {
    silane::include_si!("tokens");
}
pub mod recon {
    silane::include_si!("recon");
}
pub mod top {
    silane::include_si!("vp_ate");
}
