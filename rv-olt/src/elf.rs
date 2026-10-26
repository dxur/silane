//! A minimal ELF64 loader: the `PT_LOAD` segments of a little-endian RISC-V executable.

/// A loadable segment: its physical address and contents (zero-filled to `memsz`).
pub struct Segment {
    pub addr: u64,
    pub data: Vec<u8>,
}

pub struct Elf {
    pub entry: u64,
    pub segments: Vec<Segment>,
    /// Symbols from `.symtab`, by name.
    pub symbols: Vec<(String, u64)>,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl Elf {
    pub fn parse(b: &[u8]) -> Result<Elf, String> {
        if b.len() < 64 || &b[..4] != b"\x7fELF" {
            return Err("not an ELF file".into());
        }
        if b[4] != 2 || b[5] != 1 {
            return Err("not a little-endian ELF64 file".into());
        }
        if u16_at(b, 18) != 243 {
            return Err("not a RISC-V ELF file".into());
        }
        let entry = u64_at(b, 24);
        let (phoff, shoff) = (u64_at(b, 32) as usize, u64_at(b, 40) as usize);
        let (phentsize, phnum) = (u16_at(b, 54) as usize, u16_at(b, 56) as usize);
        let (shentsize, shnum) = (u16_at(b, 58) as usize, u16_at(b, 60) as usize);

        let mut segments = Vec::new();
        for i in 0..phnum {
            let ph = phoff + i * phentsize;
            if u32_at(b, ph) != 1 {
                continue; // not PT_LOAD
            }
            let (off, paddr) = (u64_at(b, ph + 8) as usize, u64_at(b, ph + 24));
            let (filesz, memsz) = (u64_at(b, ph + 32) as usize, u64_at(b, ph + 40) as usize);
            let mut data = b[off..off + filesz].to_vec();
            data.resize(memsz, 0);
            segments.push(Segment { addr: paddr, data });
        }

        let mut symbols = Vec::new();
        for i in 0..shnum {
            let sh = shoff + i * shentsize;
            if u32_at(b, sh + 4) != 2 {
                continue; // not SHT_SYMTAB
            }
            let (off, size) = (u64_at(b, sh + 24) as usize, u64_at(b, sh + 32) as usize);
            let strtab = shoff + u32_at(b, sh + 40) as usize * shentsize;
            let str_off = u64_at(b, strtab + 24) as usize;
            for s in (off..off + size).step_by(24) {
                let name_off = str_off + u32_at(b, s) as usize;
                let end = b[name_off..].iter().position(|&c| c == 0).unwrap_or(0);
                let name = String::from_utf8_lossy(&b[name_off..name_off + end]).into_owned();
                if !name.is_empty() {
                    symbols.push((name, u64_at(b, s + 8)));
                }
            }
        }
        Ok(Elf { entry, segments, symbols })
    }

    pub fn symbol(&self, name: &str) -> Option<u64> {
        self.symbols.iter().find(|(n, _)| n == name).map(|&(_, a)| a)
    }
}
