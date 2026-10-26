//! Hardware compiled from `hw/*.si` by the build script.

pub mod button {
    silane::include_si!("button");
}
pub mod alu {
    silane::include_si!("alu");
}
pub mod counter {
    silane::include_si!("counter");
}
pub mod fifo {
    silane::include_si!("fifo");
}
pub mod memory {
    silane::include_si!("memory");
}
pub mod misc {
    silane::include_si!("misc");
}
pub mod stream {
    silane::include_si!("stream");
}
pub mod traffic {
    silane::include_si!("traffic");
}
