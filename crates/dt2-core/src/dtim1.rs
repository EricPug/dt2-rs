//! MCF5441x DMA Timer 1 subset (MCF54418RM chapter 39).
//!
//! The instruction-clock scale is diagnostic; it is not a bus-cycle model.
//! Ticks are applied lazily (see `pit.rs`); `set_eager(true)` applies them per instruction.

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

pub const DTIM3_BASE: u32 = 0xFC07_C000;

pub struct DmaTimer1 {
    base: u32,
    mode: u16,
    extended_mode: u8,
    event: u8,
    reference: u32,
    capture: u32,
    counter: u32,
    prescaler: u64,
    ticks_per_instruction: u32,
    eager: bool,
    lag: u64,
    deadline: u64,
}

impl DmaTimer1 {
    pub fn new(ticks_per_instruction: u32) -> Self {
        Self::at(DTIM1_BASE, ticks_per_instruction)
    }

    /// Another DMA timer with the same register layout (DTIM3 is at 0xFC07C000).
    pub fn at(base: u32, ticks_per_instruction: u32) -> Self {
        assert!(ticks_per_instruction > 0);
        Self {
            base,
            mode: 0,
            extended_mode: 0,
            event: 0,
            reference: u32::MAX,
            capture: 0,
            counter: 0,
            prescaler: 0,
            ticks_per_instruction,
            eager: false,
            lag: 0,
            deadline: u64::MAX,
        }
    }

    pub fn set_eager(&mut self, eager: bool) {
        self.sync();
        self.eager = eager;
    }

    pub fn contains(address: u32) -> bool {
        (DTIM1_BASE..DTIM1_END).contains(&address)
    }

    pub fn covers(&self, address: u32) -> bool {
        (self.base..self.base + 0x4000).contains(&address)
    }

    pub fn interrupt_pending(&self) -> bool {
        self.event & DTER_REF != 0
            && self.mode & DTMR_ORRI != 0
            && self.extended_mode & (DTXMR_DMAEN | DTXMR_EN1588) == 0
    }

    /// (DTMR, DTXMR, DTER, DTRR, DTCN) for diagnostics.
    pub fn debug_state(&mut self) -> (u16, u8, u8, u32, u32) {
        self.sync();
        (self.mode, self.extended_mode, self.event, self.reference, self.counter)
    }

    /// Changes the tick rate; used to override the rate stored in a snapshot.
    pub fn set_ticks_per_instruction(&mut self, ticks_per_instruction: u32) {
        assert!(ticks_per_instruction > 0);
        self.sync();
        self.ticks_per_instruction = ticks_per_instruction;
        self.recompute_deadline();
    }

    pub fn advance_instruction(&mut self) {
        if self.eager {
            self.advance_ticks(u64::from(self.ticks_per_instruction));
            return;
        }
        self.lag += 1;
        if self.lag >= self.deadline {
            self.sync();
        }
    }

    /// Instruction retirements until the next event can be raised (1 if it may fire immediately).
    pub fn instructions_until_event(&self) -> u64 {
        if self.eager {
            return 1;
        }
        self.deadline.saturating_sub(self.lag).max(1)
    }

    /// Instructions that can retire before the next event without changing visible state.
    pub fn idle_headroom(&self) -> u64 {
        if self.eager {
            return 0;
        }
        self.deadline.saturating_sub(self.lag).saturating_sub(1)
    }

    /// Retire `count` instructions that `idle_headroom` proved event-free.
    pub fn skip_idle(&mut self, count: u64) {
        self.lag += count;
    }

    /// Apply all pending ticks so timer state is current.
    pub fn sync(&mut self) {
        if self.lag > 0 {
            let ticks = self.lag * u64::from(self.ticks_per_instruction);
            self.lag = 0;
            self.advance_ticks(ticks);
        }
        self.recompute_deadline();
    }

    fn clock_divisor(&self) -> Option<u64> {
        let clock_divisor = match self.mode & DTMR_CLOCK_MASK {
            0x0002 => 1,
            0x0004 => 16,
            _ => return None,
        };
        Some((u64::from((self.mode >> 8) & 0xFF) + 1) * clock_divisor)
    }

    /// Instructions until the reference event can next be set; u64::MAX if it cannot.
    fn recompute_deadline(&mut self) {
        self.deadline = u64::MAX;
        if self.mode & DTMR_ENABLE == 0
            || self.extended_mode & DTXMR_EN1588 != 0
            || self.event & DTER_REF != 0
            || self.counter >= self.reference
        {
            return;
        }
        let Some(divisor) = self.clock_divisor() else {
            return;
        };
        // The prescaler can exceed the divisor after a prescale-only mode change; the event then fires on the next tick.
        let ticks = (u64::from(self.reference - self.counter) * divisor)
            .saturating_sub(self.prescaler)
            .max(1);
        self.deadline = ticks.div_ceil(u64::from(self.ticks_per_instruction));
    }

    fn advance_ticks(&mut self, ticks: u64) {
        if ticks == 0 || self.mode & DTMR_ENABLE == 0 || self.extended_mode & DTXMR_EN1588 != 0 {
            return;
        }

        let Some(divisor) = self.clock_divisor() else {
            return;
        };
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
                ((next_counter - reference) % reference) as u32
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
        match address - self.base {
            DTMR => Ok(self.mode),
            DTXMR => Ok(u16::from(self.extended_mode) << 8),
            DTER => Ok(u16::from(self.event) << 8),
            _ => Err(Self::error(address, false)),
        }
    }

    fn read_long(&self, address: u32) -> Result<u32, BusError> {
        match address - self.base {
            DTRR => Ok(self.reference),
            DTCR => Ok(self.capture),
            DTCN => Ok(self.counter),
            _ => Err(Self::error(address, false)),
        }
    }
}

impl Bus for DmaTimer1 {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        self.sync();
        if !self.covers(address) {
            return Err(Self::error(address, false));
        }
        match address - self.base {
            0 | 1 => Ok(self.mode_byte(address)),
            DTXMR => Ok(self.extended_mode),
            DTER => Ok(self.event),
            4..=7 => Ok(self.reference.to_be_bytes()[(address - self.base - 4) as usize]),
            8..=11 => Ok(self.capture.to_be_bytes()[(address - self.base - 8) as usize]),
            12..=15 => Ok(self.counter.to_be_bytes()[(address - self.base - 12) as usize]),
            _ => Err(Self::error(address, false)),
        }
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        self.sync();
        self.read_word(address)
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        self.sync();
        self.read_long(address)
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        self.sync();
        let result = self.write8_synced(address, value);
        self.recompute_deadline();
        result
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        self.sync();
        if address != self.base {
            return Err(Self::error(address, true));
        }
        self.write_mode(value);
        self.recompute_deadline();
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        self.sync();
        match address - self.base {
            DTRR => self.reference = value,
            DTCN => {
                self.counter = value;
                self.prescaler = 0;
            }
            _ => return Err(Self::error(address, true)),
        }
        self.recompute_deadline();
        Ok(())
    }

    fn advance_instruction(&mut self) {
        DmaTimer1::advance_instruction(self);
    }
}

impl DmaTimer1 {
    fn write8_synced(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        if !self.covers(address) {
            return Err(Self::error(address, true));
        }
        match address - self.base {
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

    pub fn save_state(&self, w: &mut crate::snapshot::Writer) {
        debug_assert_eq!(self.lag, 0);
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
        self.lag = 0;
        self.recompute_deadline();
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
    fn restart_mode_keeps_firing_after_a_multi_period_gap() {
        let mut timer = DmaTimer1::new(1);
        timer.write32(DTIM1_BASE + DTRR, 2).unwrap();
        timer.write16(DTIM1_BASE, 0x841B).unwrap();

        // Event left pending across several periods, as when the ISR is delayed.
        timer.advance_ticks(7 * 133);
        timer.write8(DTIM1_BASE + DTER, DTER_REF).unwrap();
        assert!(!timer.interrupt_pending());
        timer.advance_ticks(2 * 133);
        assert!(timer.interrupt_pending());
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

    #[test]
    fn lazy_and_eager_timers_stay_identical_under_random_traffic() {
        let mut seed = 0x9E37_79B9_u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as u32
        };
        for ticks in [1, 5] {
            let mut eager = DmaTimer1::new(ticks);
            let mut lazy = DmaTimer1::new(ticks);
            eager.set_eager(true);
            for step in 0..20_000 {
                match next() % 40 {
                    0 => {
                        let value = [1, 2, 7, 300, 5000][(next() % 5) as usize];
                        for timer in [&mut eager, &mut lazy] {
                            timer.write32(DTIM1_BASE + DTRR, value).unwrap();
                        }
                    }
                    1 => {
                        let value = [0x841B, 0x840B, 0x8419, 0x8400, 0x0013, 0x0A1D][(next() % 6) as usize];
                        for timer in [&mut eager, &mut lazy] {
                            timer.write16(DTIM1_BASE, value).unwrap();
                        }
                    }
                    2 => {
                        for timer in [&mut eager, &mut lazy] {
                            timer.write8(DTIM1_BASE + DTER, DTER_REF).unwrap();
                        }
                    }
                    3 => assert_eq!(
                        eager.read32(DTIM1_BASE + DTCN).unwrap(),
                        lazy.read32(DTIM1_BASE + DTCN).unwrap(),
                        "counter diverged at step {step}"
                    ),
                    _ => {
                        eager.advance_instruction();
                        lazy.advance_instruction();
                    }
                }
                assert_eq!(
                    eager.interrupt_pending(),
                    lazy.interrupt_pending(),
                    "interrupt diverged at step {step}: eager mode={:#x} xm={:#x} ev={} ref={} cnt={} pre={} | lazy cnt={} pre={} lag={} deadline={} ticks={ticks}",
                    eager.mode, eager.extended_mode, eager.event, eager.reference, eager.counter, eager.prescaler,
                    lazy.counter, lazy.prescaler, lazy.lag, lazy.deadline
                );
            }
            eager.sync();
            lazy.sync();
            assert_eq!(eager.read32(DTIM1_BASE + DTCN).unwrap(), lazy.read32(DTIM1_BASE + DTCN).unwrap());
            assert_eq!(eager.read8(DTIM1_BASE + DTER).unwrap(), lazy.read8(DTIM1_BASE + DTER).unwrap());
        }
    }
}
