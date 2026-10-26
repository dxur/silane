//! rv-olt: an RV64GC core and SoC written in silane, and the Rust around it: the
//! simulation harness with its RAM and console, and an ELF loader.

pub mod elf;
pub mod machine;

pub mod soc {
    silane::include_si!("soc");
}

// The core and the FPU alone, for exporting them on their own.
pub mod cpu {
    silane::include_si!("core");
}
pub mod fpu {
    silane::include_si!("fpu");
}
pub mod parts {
    //! The blocks of the core, for exporting and measuring them on their own.
    pub mod csr {
        silane::include_si!("csr");
    }
    pub mod mmu {
        silane::include_si!("mmu");
    }
    pub mod muldiv {
        silane::include_si!("muldiv");
    }
    pub mod devices {
        silane::include_si!("devices");
    }
    pub mod decode {
        silane::include_si!("decode");
    }
    pub mod rvc {
        silane::include_si!("rvc");
    }
    pub mod alu {
        silane::include_si!("alu");
    }
}
