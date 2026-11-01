//! MCF5441x interrupt controller 2 register subset (MCF54418RM chapter 17).

use crate::bus::{Bus, BusError, InterruptRequest};

const BASE: u32 = 0xFC05_0000;
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
const VECTOR_BASE: u8 = 192;
const CIMR_CALL: u8 = 1 << 6;

/// INTC2 mask and priority state for the bootstrap's observed register subset.
pub struct Intc2 {
    imrh: u32,
    imrl: u32,
    intfrch: u32,
    intfrcl: u32,
    icr: [u8; SOURCE_COUNT],
}

impl Intc2 {
    pub fn new() -> Self {
        Self {
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

    pub fn pending_interrupt(&self, pending_sources: u64) -> Option<InterruptRequest> {
        let forced = (u64::from(self.intfrch) << 32) | u64::from(self.intfrcl);
        let pending = pending_sources | forced;
        if pending == 0 {
            return None;
        }
        (0..SOURCE_COUNT)
            .filter(|source| pending & (1u64 << source) != 0)
            .filter_map(|source| {
                let bit = source % 32;
                let mask = if source < 32 { self.imrl } else { self.imrh };
                let level = self.icr[source] & 7;
                (mask & (1 << bit) == 0 && level != 0).then_some(InterruptRequest {
                    source: source as u8,
                    level,
                    vector: VECTOR_BASE.wrapping_add(source as u8),
                })
            })
            .max_by_key(|request| (request.level, request.source))
    }

    fn read_word(&self, offset: u32) -> Result<u32, BusError> {
        Ok(match offset {
            REG_IPRH => self.intfrch,
            REG_IPRL => self.intfrcl,
            REG_IMRH => self.imrh,
            REG_IMRL => self.imrl,
            REG_INTFRCH => self.intfrch,
            REG_INTFRCL => self.intfrcl,
            REG_SIMR | REG_CIMR => 0,
            _ if (REG_ICR_START..REG_ICR_END).contains(&offset) => {
                u32::from(self.icr[(offset - REG_ICR_START) as usize])
            }
            _ => {
                return Err(BusError {
                    addr: BASE + offset,
                    write: false,
                })
            }
        })
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
        let bit = 1u32 << (source % 32);
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

    fn write_word(&mut self, offset: u32, value: u32) -> Result<(), BusError> {
        match offset {
            REG_IMRH => self.imrh = value,
            REG_IMRL => self.imrl = value,
            REG_INTFRCH => self.intfrch = value,
            REG_INTFRCL => self.intfrcl = value,
            _ => {
                return Err(BusError {
                    addr: BASE + offset,
                    write: true,
                })
            }
        }
        Ok(())
    }
}

impl Default for Intc2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for Intc2 {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        if !Self::contains(address) {
            return Err(BusError {
                addr: address,
                write: false,
            });
        }
        let offset = address - BASE;
        if (REG_ICR_START..REG_ICR_END).contains(&offset) {
            return Ok(self.icr[(offset - REG_ICR_START) as usize]);
        }
        let word = self.read_word(offset & !3)?;
        Ok((word >> (24 - 8 * (offset & 3))) as u8)
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        if address & 1 != 0 {
            return Err(BusError {
                addr: address,
                write: false,
            });
        }
        Ok(u16::from_be_bytes([
            self.read8(address)?,
            self.read8(address.wrapping_add(1))?,
        ]))
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        if address & 3 != 0 || !Self::contains(address) {
            return Err(BusError {
                addr: address,
                write: false,
            });
        }
        self.read_word(address - BASE)
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        if !Self::contains(address) {
            return Err(BusError {
                addr: address,
                write: true,
            });
        }
        let offset = address - BASE;
        match offset {
            REG_SIMR => self.write_mask_command(value, true),
            REG_CIMR => self.write_mask_command(value, false),
            REG_ICR_START..REG_ICR_END => {
                self.icr[(offset - REG_ICR_START) as usize] = value & 7;
            }
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
            return Err(BusError {
                addr: address,
                write: true,
            });
        }
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write8(address.wrapping_add(offset as u32), byte)?;
        }
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        if address & 3 != 0 || !Self::contains(address) {
            return Err(BusError {
                addr: address,
                write: true,
            });
        }
        self.write_word(address - BASE, value)
    }
}


impl Intc2 {
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
    fn cimr_unmasks_pit0_and_icr_selects_vector_level() {
        let mut intc = Intc2::new();
        assert_eq!(intc.pending_interrupt(1 << 13), None);

        intc.write8(BASE + REG_ICR_START + 13, 1).unwrap();
        intc.write8(BASE + REG_CIMR, 13).unwrap();

        assert_eq!(
            intc.pending_interrupt(1 << 13),
            Some(InterruptRequest {
                source: 13,
                level: 1,
                vector: 205,
            })
        );
    }
}
