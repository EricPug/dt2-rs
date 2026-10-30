//! MCF5441x DMA Timer 1 subset (MCF54418RM chapter 39).
//!
//! The instruction-clock scale is diagnostic; it is not a bus-cycle model.

use crate::bus::{Bus, BusError};

pub const DTIM1_BASE: u32 = 0xFC07_4000;
const DTIM1_END: u32 = DTIM1_BASE + 0x4000;
const DTMR: u32 = 0x00;
const DTXMR: u32 = 0x02;
const DTER: u32 = 0x03;
const DTRR: u32 = 0x04;
const DTCR: u32 = 0x08;
const DTCN: u32 = 0x0C;
const DTMR_ORRI: u16 = 1 << 4;
const DTMR_FRR: u16 = 1 << 3;
const DTMR_CLOCK_MASK: u16 = 0x0006;
const DTMR_ENABLE: u16 = 1;
const DTXMR_DMAEN: u8 = 1 << 7;
const DTXMR_EN1588: u8 = 1 << 5;
const DTER_REF: u8 = 1 << 1;
const DTER_CAP: u8 = 1;

pub struct DmaTimer1 {
    mode: u16,
    extended_mode: u8,
    event: u8,
    reference: u32,
    capture: u32,
    counter: u32,
    prescaler: u64,
    ticks_per_instruction: u32,
}

impl DmaTimer1 {
    pub fn new(ticks_per_instruction: u32) -> Self {
        assert!(ticks_per_instruction > 0);
        Self {
            mode: 0,
            extended_mode: 0,
            event: 0,
            reference: u32::MAX,
            capture: 0,
            counter: 0,
            prescaler: 0,
            ticks_per_instruction,
        }
    }

    pub fn contains(address: u32) -> bool {
        (DTIM1_BASE..DTIM1_END).contains(&address)
    }

    pub fn interrupt_pending(&self) -> bool {
        self.event & DTER_REF != 0
            && self.mode & DTMR_ORRI != 0
            && self.extended_mode & (DTXMR_DMAEN | DTXMR_EN1588) == 0
    }

    pub fn advance_instruction(&mut self) {
        self.advance_ticks(u64::from(self.ticks_per_instruction));
    }

    fn advance_ticks(&mut self, ticks: u64) {
        if ticks == 0 || self.mode & DTMR_ENABLE == 0 || self.extended_mode & DTXMR_EN1588 != 0 {
            return;
        }

        let clock_divisor = match self.mode & DTMR_CLOCK_MASK {
            0x0002 => 1,
            0x0004 => 16,
            _ => return,
        };
        let prescale_divisor = u64::from((self.mode >> 8) & 0xFF) + 1;
        let divisor = prescale_divisor * clock_divisor;
        let elapsed = self.prescaler + ticks;
        let timer_ticks = elapsed / divisor;
        self.prescaler = elapsed % divisor;
        if timer_ticks == 0 {
            return;
        }

        let old_counter = u64::from(self.counter);
        let next_counter = old_counter + timer_ticks;
        let reference = u64::from(self.reference);
        if old_counter < reference && next_counter >= reference {
            self.event |= DTER_REF;
            self.counter = if self.mode & DTMR_FRR != 0 {
                (next_counter - reference) as u32
            } else {
                next_counter as u32
            };
        } else {
            self.counter = next_counter as u32;
        }
    }

    fn error(address: u32, write: bool) -> BusError {
        BusError {
            addr: address,
            write,
        }
    }

    fn mode_byte(&self, address: u32) -> u8 {
        if address & 1 == 0 {
            (self.mode >> 8) as u8
        } else {
            self.mode as u8
        }
    }

    fn write_mode(&mut self, value: u16) {
        if self.mode & DTMR_ENABLE != 0 && value & DTMR_ENABLE == 0 {
            self.counter = 0;
            self.prescaler = 0;
            self.event = 0;
        }
        if (self.mode & DTMR_CLOCK_MASK) != (value & DTMR_CLOCK_MASK) {
            self.prescaler = 0;
        }
        self.mode = value;
    }

    fn read_word(&self, address: u32) -> Result<u16, BusError> {
        if address & 1 != 0 {
            return Err(Self::error(address, false));
        }
        match address - DTIM1_BASE {
            DTMR => Ok(self.mode),
            DTXMR => Ok(u16::from(self.extended_mode) << 8),
            DTER => Ok(u16::from(self.event) << 8),
            _ => Err(Self::error(address, false)),
        }
    }

    fn read_long(&self, address: u32) -> Result<u32, BusError> {
        match address - DTIM1_BASE {
            DTRR => Ok(self.reference),
            DTCR => Ok(self.capture),
            DTCN => Ok(self.counter),
            _ => Err(Self::error(address, false)),
        }
    }
}

impl Bus for DmaTimer1 {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        if !Self::contains(address) {
            return Err(Self::error(address, false));
        }
        match address - DTIM1_BASE {
            0 | 1 => Ok(self.mode_byte(address)),
            DTXMR => Ok(self.extended_mode),
            DTER => Ok(self.event),
            4..=7 => Ok(self.reference.to_be_bytes()[(address - DTIM1_BASE - 4) as usize]),
            8..=11 => Ok(self.capture.to_be_bytes()[(address - DTIM1_BASE - 8) as usize]),
            12..=15 => Ok(self.counter.to_be_bytes()[(address - DTIM1_BASE - 12) as usize]),
            _ => Err(Self::error(address, false)),
        }
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        self.read_word(address)
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        self.read_long(address)
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        if !Self::contains(address) {
            return Err(Self::error(address, true));
        }
        match address - DTIM1_BASE {
            0 | 1 => {
                let old = self.mode;
                let mode = if address & 1 == 0 {
                    (u16::from(value) << 8) | (old & 0x00FF)
                } else {
                    (old & 0xFF00) | u16::from(value)
                };
                self.write_mode(mode);
            }
            DTXMR => self.extended_mode = value,
            DTER => self.event &= !(value & (DTER_REF | DTER_CAP)),
            _ => return Err(Self::error(address, true)),
        }
        Ok(())
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        if address != DTIM1_BASE {
            return Err(Self::error(address, true));
        }
        self.write_mode(value);
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        if address != DTIM1_BASE + DTRR {
            return Err(Self::error(address, true));
        }
        self.reference = value;
        Ok(())
    }

    fn advance_instruction(&mut self) {
        DmaTimer1::advance_instruction(self);
    }
}


impl DmaTimer1 {
    pub fn save_state(&self, w: &mut crate::snapshot::Writer) {
        w.u16(self.mode);
        w.u8(self.extended_mode);
        w.u8(self.event);
        w.u32(self.reference);
        w.u32(self.capture);
        w.u32(self.counter);
        w.u64(self.prescaler);
        w.u32(self.ticks_per_instruction);
    }

    pub fn load_state(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), String> {
        self.mode = r.u16()?;
        self.extended_mode = r.u8()?;
        self.event = r.u8()?;
        self.reference = r.u32()?;
        self.capture = r.u32()?;
        self.counter = r.u32()?;
        self.prescaler = r.u64()?;
        self.ticks_per_instruction = r.u32()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_match_sets_interrupt_and_event_is_write_one_to_clear() {
        let mut timer = DmaTimer1::new(1);
        timer.write32(DTIM1_BASE + DTRR, 2).unwrap();
        timer.write16(DTIM1_BASE, 0x841B).unwrap();

        timer.advance_ticks(2 * 133 - 1);
        assert_eq!(timer.read8(DTIM1_BASE + DTER).unwrap() & DTER_REF, 0);
        assert!(!timer.interrupt_pending());

        timer.advance_ticks(1);
        assert_eq!(timer.read8(DTIM1_BASE + DTER).unwrap() & DTER_REF, DTER_REF);
        assert!(timer.interrupt_pending());

        timer.write8(DTIM1_BASE + DTER, DTER_REF).unwrap();
        assert!(!timer.interrupt_pending());
    }

    #[test]
    fn reference_match_does_not_interrupt_without_orri_or_with_dma_enabled() {
        let mut timer = DmaTimer1::new(1);
        timer.write32(DTIM1_BASE + DTRR, 1).unwrap();
        timer.write16(DTIM1_BASE, 0x840B).unwrap();
        timer.advance_ticks(133);
        assert!(!timer.interrupt_pending());

        timer.write16(DTIM1_BASE, 0x841B).unwrap();
        timer.write8(DTIM1_BASE + DTXMR, DTXMR_DMAEN).unwrap();
        assert!(!timer.interrupt_pending());
    }
}
