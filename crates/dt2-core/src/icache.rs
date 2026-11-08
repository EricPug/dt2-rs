//! Instruction-word cache for the ColdFire interpreter.
//!
//! The cache holds the raw words an instruction consumed the last time it ran at a given PC, so
//! replaying it skips the bus fetches. ColdFire instruction length depends only on the opcode and
//! extension words, never on register data, so replaying the same words is exactly equivalent to
//! fetching them again while the code bytes are unchanged.
//!
//! Validity is tracked with [`CodeMap`], owned by whoever owns the RAM: it marks the 256-byte
//! pages that back cached code, and any write to a marked page bumps an epoch. Entries carry the
//! epoch they were filled in, so one epoch bump invalidates the whole cache in O(1).

pub const MAX_WORDS: usize = 6;
/// No handler: run the reference `execute`.
pub const HANDLER_NONE: u8 = 0;
/// MOVE.L / MOVEA.L with register, indirect, post/pre-increment or d16(An) operands.
pub const HANDLER_MOVE_L: u8 = 1;
const PAGE_SHIFT: u32 = 8;
const ENTRIES: usize = 1 << 16;

#[derive(Clone, Copy)]
pub struct Entry {
    pub pc: u32,
    pub epoch: u32,
    pub len: u8,
    /// Pre-classified handler (`HANDLER_*`), chosen once when the entry is filled.
    pub handler: u8,
    pub words: [u16; MAX_WORDS],
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            pc: 0,
            epoch: 0,
            len: 0,
            handler: 0,
            words: [0; MAX_WORDS],
        }
    }
}

/// Direct-mapped by word address.
#[derive(Clone)]
pub struct ICache {
    entries: Vec<Entry>,
}

impl Default for ICache {
    fn default() -> Self {
        Self {
            entries: vec![Entry::default(); ENTRIES],
        }
    }
}

impl ICache {
    #[inline]
    pub fn lookup(&self, pc: u32, epoch: u32) -> Option<&Entry> {
        let e = &self.entries[(pc >> 1) as usize & (ENTRIES - 1)];
        (e.pc == pc && e.epoch == epoch && e.len != 0).then_some(e)
    }

    pub fn insert(&mut self, pc: u32, epoch: u32, handler: u8, words: &[u16]) {
        let mut entry = Entry {
            pc,
            epoch,
            len: words.len() as u8,
            handler,
            words: [0; MAX_WORDS],
        };
        entry.words[..words.len()].copy_from_slice(words);
        self.entries[(pc >> 1) as usize & (ENTRIES - 1)] = entry;
    }
}

/// Write tracking for the pages that back cached code.
#[derive(Clone, Default)]
pub struct CodeMap {
    base: u32,
    pages: usize,
    bits: Vec<u64>,
    marked: Vec<u32>,
    epoch: u32,
    flushes: u64,
}

impl CodeMap {
    pub fn enable(&mut self, base: u32, size: u32) {
        self.base = base;
        self.pages = (size as usize).div_ceil(1 << PAGE_SHIFT);
        self.bits = vec![0; self.pages.div_ceil(64)];
        self.marked.clear();
        self.epoch = 1;
    }

    pub fn enabled(&self) -> bool {
        self.epoch != 0
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn flushes(&self) -> u64 {
        self.flushes
    }

    fn page(&self, address: u32) -> Option<usize> {
        let page = (address.wrapping_sub(self.base) >> PAGE_SHIFT) as usize;
        (address >= self.base && page < self.pages).then_some(page)
    }

    /// Marks the pages covering `bytes` bytes at `address`; false if they are not tracked RAM.
    pub fn mark(&mut self, address: u32, bytes: u32) -> bool {
        if !self.enabled() {
            return false;
        }
        let (Some(first), Some(last)) = (
            self.page(address),
            self.page(address.wrapping_add(bytes - 1)),
        ) else {
            return false;
        };
        for page in first..=last {
            let (word, bit) = (page / 64, 1u64 << (page % 64));
            if self.bits[word] & bit == 0 {
                self.bits[word] |= bit;
                self.marked.push(page as u32);
            }
        }
        true
    }

    /// Call for every write to tracked RAM.
    #[inline]
    pub fn note_write(&mut self, address: u32, width: u32) {
        if !self.marked.is_empty() {
            self.note_write_slow(address, width);
        }
    }

    fn note_write_slow(&mut self, address: u32, width: u32) {
        let first = self.page(address);
        let last = self.page(address.wrapping_add(width - 1));
        let hit = [first, last]
            .into_iter()
            .flatten()
            .any(|page| self.bits[page / 64] & (1u64 << (page % 64)) != 0);
        if hit {
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        for page in self.marked.drain(..) {
            self.bits[page as usize / 64] = 0;
        }
        self.epoch = self.epoch.wrapping_add(1).max(1);
        self.flushes += 1;
    }
}
