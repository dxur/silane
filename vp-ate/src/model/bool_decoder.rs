//! The VP8 boolean entropy decoder (RFC 6386, section 7).

/// Decodes booleans from one partition. Reading past the end of the data behaves as
/// if the partition were followed by zero bytes, like libvpx.
#[derive(Debug, Clone)]
pub struct BoolDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    range: u32,
    value: u32,
    bit_count: u32,
}

impl<'a> BoolDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let mut bd = BoolDecoder {
            data,
            pos: 0,
            range: 255,
            value: 0,
            bit_count: 0,
        };
        bd.value = (bd.next_byte() << 8) | bd.next_byte();
        bd
    }

    fn next_byte(&mut self) -> u32 {
        let byte = self.data.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        byte as u32
    }

    /// Bytes consumed so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Reads a boolean that is 0 with probability `prob / 256`.
    pub fn read_bool(&mut self, prob: u8) -> bool {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        let split_shifted = split << 8;
        let bit = if self.value >= split_shifted {
            self.range -= split;
            self.value -= split_shifted;
            true
        } else {
            self.range = split;
            false
        };
        while self.range < 128 {
            self.value <<= 1;
            self.range <<= 1;
            self.bit_count += 1;
            if self.bit_count == 8 {
                self.bit_count = 0;
                self.value |= self.next_byte();
            }
        }
        bit
    }

    pub fn read_flag(&mut self) -> bool {
        self.read_bool(128)
    }

    /// An unsigned `bits`-bit literal, most significant bit first.
    pub fn read_literal(&mut self, bits: u32) -> u32 {
        (0..bits).fold(0, |v, _| (v << 1) | self.read_flag() as u32)
    }

    /// A magnitude of `bits` bits followed by a sign flag.
    pub fn read_signed(&mut self, bits: u32) -> i32 {
        let magnitude = self.read_literal(bits) as i32;
        if self.read_flag() { -magnitude } else { magnitude }
    }

    /// An optional signed value: a presence flag, then [`Self::read_signed`].
    pub fn read_optional_signed(&mut self, bits: u32) -> i32 {
        if self.read_flag() { self.read_signed(bits) } else { 0 }
    }

    /// Reads a symbol coded with `tree` (RFC 6386 section 8.1): positive entries are
    /// indices of the next node pair, other entries are negated leaf values. Node pair
    /// `i` is decided with probability `probs[i / 2]`.
    pub fn read_tree(&mut self, tree: &[i8], probs: &[u8]) -> u8 {
        self.read_tree_from(tree, probs, 0)
    }

    /// Like [`Self::read_tree`], starting at node pair `start`.
    pub fn read_tree_from(&mut self, tree: &[i8], probs: &[u8], start: usize) -> u8 {
        let mut i = start;
        loop {
            let next = tree[i + self.read_bool(probs[i >> 1]) as usize];
            if next <= 0 {
                return (-next) as u8;
            }
            i = next as usize;
        }
    }
}
