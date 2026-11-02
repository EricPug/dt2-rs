//! MCF5441x INTC0 subset: DMA Timer 1 (source 33) and forced (INTFRC) sources (MCF54418RM chapter 17).

use crate::bus::{Bus, BusError, InterruptRequest};

pub const INTC0_BASE: u32 = 0xFC04_8000;
const BASE: u32 = INTC0_BASE;
const END: u32 = BASE + 0x100;
const REG_IPRH: u32 = 0x00;
const REG_IPRL: u32 = 0x04;
const REG_IMRH: u32 = 0x08;
const REG_IMRL: u32 = 0x0C;
const REG_INTFRCH: u32 = 0x10;
const REG_INTFRCL: u32 = 0x14;
const REG_SIMR: u32 = 0x1C;
const REG_CIMR: u32 = 0x1D;
const REG_ICR_START: u32 = 0x40;
const REG_ICR_END: u32 = 0x80;
const SOURCE_COUNT: usize = 64;
const SOURCE_DTIM1: u8 = 33;
const VECTOR_BASE: u8 = 64;
const CIMR_CALL: u8 = 1 << 6;

pub struct Intc0 {
    base: u32,
    vector_base: u8,
    imrh: u32,
    imrl: u32,
    intfrch: u32,
    intfrcl: u32,
    icr: [u8; SOURCE_COUNT],
}

impl Default for Intc0 {
    fn default() -> Self {
        Self::new()
    }
}

impl Intc0 {
    pub fn new() -> Self {
        Self::at(BASE, VECTOR_BASE)
    }

    /// Another controller with the same register layout (INTC1 is at 0xFC04C000, vectors from 128).
    pub fn at(base: u32, vector_base: u8) -> Self {
        Self {
            base,
            vector_base,
            imrh: u32::MAX,
            imrl: u32::MAX,
            intfrch: 0,
            intfrcl: 0,
            icr: [0; SOURCE_COUNT],
        }
    }

    pub fn contains(address: u32) -> bool {
        (BASE..END).contains(&address)
    }

    pub fn covers(&self, address: u32) -> bool {
        (self.base..self.base + 0x100).contains(&address)
    }

    /// Highest unmasked pending source: DMA timer 1 when `timer_pending`, plus forced (INTFRC) sources.
    pub fn pending_interrupt(&self, timer_pending: bool) -> Option<InterruptRequest> {
        let mut pending = (u64::from(self.intfrch) << 32) | u64::from(self.intfrcl);
        if timer_pending {
            pending |= 1u64 << SOURCE_DTIM1;
        }
        if pending == 0 {
            return None;
        }
        (0..SOURCE_COUNT)
            .filter(|source| pending & (1u64 << source) != 0)
            .filter_map(|source| {
                let mask = if source < 32 { self.imrl } else { self.imrh };
                let level = self.icr[source] & 7;
                (mask & (1 << (source % 32)) == 0 && level != 0).then_some(InterruptRequest {
                    source: source as u8,
                    level,
                    vector: self.vector_base + source as u8,
                })
            })
            .max_by_key(|request| (request.level, request.source))
    }

    /// (IMRH, IMRL, INTFRCH, INTFRCL, ICR bytes) for diagnostics.
    pub fn debug_state(&self) -> (u32, u32, u32, u32, &[u8]) {
        (self.imrh, self.imrl, self.intfrch, self.intfrcl, &self.icr)
    }

    fn error(address: u32, write: bool) -> BusError {
        BusError {
            addr: address,
            write,
        }
    }

    fn read_word(&self, offset: u32) -> Result<u32, BusError> {
        Ok(match offset {
            REG_IPRH => self.intfrch,
            REG_IPRL => self.intfrcl,
            REG_IMRH => self.imrh,
            REG_IMRL => self.imrl,
            REG_INTFRCH => self.intfrch,
            REG_INTFRCL => self.intfrcl,
            _ => {
                return Err(Self::error(self.base + offset, false));
            }
        })
    }

    fn write_word(&mut self, offset: u32, value: u32) -> Result<(), BusError> {
        match offset {
            REG_IMRH => self.imrh = value,
            REG_IMRL => self.imrl = value,
            REG_INTFRCH => self.intfrch = value,
            REG_INTFRCL => self.intfrcl = value,
            _ => return Err(Self::error(self.base + offset, true)),
        }
        Ok(())
    }

    fn write_mask_command(&mut self, value: u8, set: bool) {
        if value & CIMR_CALL != 0 {
            if set {
                self.imrh = u32::MAX;
                self.imrl = u32::MAX;
            } else {
                self.imrh = 0;
                self.imrl = 0;
            }
            return;
        }
        let source = u32::from(value & 0x3F);
        let bit = 1 << (source % 32);
        let mask = if source < 32 {
            &mut self.imrl
        } else {
            &mut self.imrh
        };
        if set {
            *mask |= bit;
        } else {
            *mask &= !bit;
        }
    }
}

impl Bus for Intc0 {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        if !self.covers(address) {
            return Err(Self::error(address, false));
        }
        let offset = address - self.base;
        if (REG_ICR_START..REG_ICR_END).contains(&offset) {
            return Ok(self.icr[(offset - REG_ICR_START) as usize]);
        }
        let word = self.read_word(offset & !3)?;
        Ok((word >> (24 - 8 * (offset & 3))) as u8)
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        if address & 1 != 0 {
            return Err(Self::error(address, false));
        }
        Ok(u16::from_be_bytes([
            self.read8(address)?,
            self.read8(address.wrapping_add(1))?,
        ]))
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        if address & 3 != 0 || !self.covers(address) {
            return Err(Self::error(address, false));
        }
        self.read_word(address - self.base)
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        if !self.covers(address) {
            return Err(Self::error(address, true));
        }
        let offset = address - self.base;
        match offset {
            REG_SIMR => self.write_mask_command(value, true),
            REG_CIMR => self.write_mask_command(value, false),
            REG_ICR_START..REG_ICR_END => self.icr[(offset - REG_ICR_START) as usize] = value & 7,
            _ => {
                let aligned = offset & !3;
                let old = self.read_word(aligned)?;
                let shift = 24 - 8 * (offset & 3);
                let next = (old & !(0xFF << shift)) | (u32::from(value) << shift);
                self.write_word(aligned, next)?;
            }
        }
        Ok(())
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        if address & 1 != 0 {
            return Err(Self::error(address, true));
        }
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write8(address.wrapping_add(offset as u32), byte)?;
        }
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        if address & 3 != 0 || !self.covers(address) {
            return Err(Self::error(address, true));
        }
        self.write_word(address - self.base, value)
    }
}


impl Intc0 {
    pub fn save_state(&self, w: &mut crate::snapshot::Writer) {
        for v in [self.imrh, self.imrl, self.intfrch, self.intfrcl] {
            w.u32(v);
        }
        w.bytes(&self.icr);
    }

    pub fn load_state(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), String> {
        self.imrh = r.u32()?;
        self.imrl = r.u32()?;
        self.intfrch = r.u32()?;
        self.intfrcl = r.u32()?;
        let icr = r.bytes()?;
        if icr.len() != self.icr.len() {
            return Err("INTC ICR size mismatch".into());
        }
        self.icr.copy_from_slice(icr);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtim1_source_33_requires_unmask_and_nonzero_priority() {
        let mut intc0 = Intc0::new();
        assert_eq!(intc0.pending_interrupt(true), None);

        intc0.write8(BASE + 0x61, 2).unwrap();
        assert_eq!(intc0.pending_interrupt(true), None);

        intc0.write8(BASE + REG_CIMR, 33).unwrap();
        assert_eq!(
            intc0.pending_interrupt(true),
            Some(InterruptRequest {
                source: 33,
                level: 2,
                vector: 97,
            })
        );
        assert_eq!(intc0.pending_interrupt(false), None);
    }

    #[test]
    fn forced_source_is_delivered_until_its_force_bit_is_cleared() {
        let mut intc0 = Intc0::new();
        // Source 57 is bit 25 of INTFRCH and ICR57 lives at +0x40+57.
        intc0.write8(BASE + 0x40 + 57, 4).unwrap();
        intc0.write32(BASE + REG_INTFRCH, 1 << 25).unwrap();
        assert_eq!(intc0.pending_interrupt(false), None, "masked after reset");

        intc0.write8(BASE + REG_CIMR, 57).unwrap();
        let expected = InterruptRequest { source: 57, level: 4, vector: 121 };
        assert_eq!(intc0.pending_interrupt(false), Some(expected));

        // A higher level wins over an earlier source; clearing the force bit ends the request.
        intc0.write8(BASE + 0x40 + 44, 6).unwrap();
        intc0.write8(BASE + REG_CIMR, 44).unwrap();
        intc0.write32(BASE + REG_INTFRCH, (1 << 25) | (1 << 12)).unwrap();
        assert_eq!(intc0.pending_interrupt(false).map(|r| r.source), Some(44));
        intc0.write32(BASE + REG_INTFRCH, 0).unwrap();
        assert_eq!(intc0.pending_interrupt(false), None);
    }
}
