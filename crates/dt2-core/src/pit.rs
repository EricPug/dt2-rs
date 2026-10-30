//! MCF5441x programmable interrupt timers (MCF54418RM chapter 38).
//!
//! Time advances a configurable number of ticks per retired CPU instruction
//! when the owning bus calls `advance_instruction`. This diagnostic clock is
//! deterministic but is not equivalent to MCF bus-clock timing.
//! Interrupt-controller delivery is intentionally outside this model.

use crate::bus::{Bus, BusError};

const BASE: u32 = 0xFC08_0000;
const MODULE_STRIDE: u32 = 0x4000;
const MODULE_COUNT: usize = 4;
const APERTURE_END: u32 = BASE + MODULE_STRIDE * MODULE_COUNT as u32;
const PCSR_PRE: u16 = 0x0F00;
const PCSR_PIF: u16 = 0x0004;
const PCSR_PIE: u16 = 0x0008;
const PCSR_RLD: u16 = 0x0002;
const PCSR_EN: u16 = 0x0001;
const PCSR_CONFIG: u16 = 0x0F7B;

struct Pit {
    pcsr: u16,
    pmr: u16,
    counter: u16,
    prescaler: u32,
    first_tick_delay: u64,
    first_tick_delay_remaining: u64,
    delay_next_enable: bool,
}

impl Pit {
    fn new() -> Self {
        Self {
            pcsr: 0,
            pmr: u16::MAX,
            counter: u16::MAX,
            prescaler: 0,
            first_tick_delay: 0,
            first_tick_delay_remaining: 0,
            delay_next_enable: false,
        }
    }

    fn set_first_tick_delay(&mut self, ticks: u64) {
        self.first_tick_delay = ticks;
        self.delay_next_enable = ticks > 0;
    }

    fn read(&self, register: u32) -> u16 {
        match register {
            0 => self.pcsr,
            2 => self.pmr,
            4 => self.counter,
            _ => unreachable!(),
        }
    }

    fn write(&mut self, register: u32, value: u16) {
        match register {
            0 => {
                let old_pre = self.pcsr & PCSR_PRE;
                let old_enabled = self.pcsr & PCSR_EN != 0;
                let old_pif = self.pcsr & PCSR_PIF;
                let next_pcsr = value & PCSR_CONFIG;
                self.pcsr = next_pcsr | if value & PCSR_PIF == 0 { old_pif } else { 0 };
                if !old_enabled && next_pcsr & PCSR_EN != 0 && self.delay_next_enable {
                    self.first_tick_delay_remaining = self.first_tick_delay;
                    self.delay_next_enable = false;
                }
                if (next_pcsr & PCSR_PRE) != old_pre {
                    self.prescaler = 0;
                }
            }
            2 => {
                self.pmr = value;
                self.pcsr &= !PCSR_PIF;
                self.prescaler = 0;
                if self.pcsr & 0x0010 != 0 {
                    self.counter = value;
                }
            }
            4 => {}
            _ => unreachable!(),
        }
    }

    fn advance_ticks(&mut self, ticks: u64) {
        if self.pcsr & PCSR_EN == 0 || ticks == 0 {
            return;
        }
        let delayed_ticks = ticks.min(self.first_tick_delay_remaining);
        self.first_tick_delay_remaining -= delayed_ticks;
        let ticks = ticks - delayed_ticks;
        if ticks == 0 {
            return;
        }
        let prescaler_bits = u32::from((self.pcsr & PCSR_PRE) >> 8);
        let divisor = 1u32 << prescaler_bits;
        let elapsed = u64::from(self.prescaler) + ticks;
        let timer_ticks = elapsed / u64::from(divisor);
        self.prescaler = (elapsed % u64::from(divisor)) as u32;
        if timer_ticks == 0 {
            return;
        }

        let ticks_until_pif = u64::from(self.counter) + 1;
        if timer_ticks < ticks_until_pif {
            self.counter -= timer_ticks as u16;
            return;
        }

        self.pcsr |= PCSR_PIF;
        let reload = if self.pcsr & PCSR_RLD != 0 {
            self.pmr
        } else {
            u16::MAX
        };
        let ticks_after_pif = timer_ticks - ticks_until_pif;
        let period = u64::from(reload) + 1;
        self.counter = reload - (ticks_after_pif % period) as u16;
    }
}

/// Four MCF5441x 16-bit programmable interrupt timers.
pub struct PitController {
    timers: [Pit; MODULE_COUNT],
    ticks_per_instruction: u32,
}

impl PitController {
    pub fn new() -> Self {
        Self::with_ticks_per_instruction(1)
    }

    pub fn with_ticks_per_instruction(ticks_per_instruction: u32) -> Self {
        assert!(ticks_per_instruction > 0);
        Self {
            timers: std::array::from_fn(|_| Pit::new()),
            ticks_per_instruction,
        }
    }

    pub fn set_first_tick_delay(&mut self, ticks: u64) {
        self.timers[0].set_first_tick_delay(ticks);
    }

    pub fn contains(address: u32) -> bool {
        (BASE..APERTURE_END).contains(&address)
    }

    pub fn advance_instruction(&mut self) {
        for timer in &mut self.timers {
            timer.advance_ticks(u64::from(self.ticks_per_instruction));
        }
    }

    pub fn ticks_until_pif(&self, module: usize) -> Option<u64> {
        let timer = self.timers.get(module)?;
        if timer.pcsr & PCSR_EN == 0 || timer.pcsr & PCSR_PIF != 0 {
            return None;
        }
        let prescaler_bits = u32::from((timer.pcsr & PCSR_PRE) >> 8);
        let divisor = 1u64 << prescaler_bits;
        Some(
            timer.first_tick_delay_remaining + (u64::from(timer.counter) + 1) * divisor
                - u64::from(timer.prescaler),
        )
    }

    pub fn fast_forward_ticks(&mut self, ticks: u64) {
        for timer in &mut self.timers {
            timer.advance_ticks(ticks);
        }
    }

    pub fn ticks_per_instruction(&self) -> u32 {
        self.ticks_per_instruction
    }

    pub fn interrupt_pending(&self, module: usize) -> bool {
        self.timers.get(module).is_some_and(|timer| {
            timer.pcsr & (PCSR_EN | PCSR_PIE | PCSR_PIF) == (PCSR_EN | PCSR_PIE | PCSR_PIF)
        })
    }

    pub fn timer_state(&self, module: usize) -> Option<(u16, u16, u16)> {
        self.timers
            .get(module)
            .map(|timer| (timer.pcsr, timer.pmr, timer.counter))
    }

    fn decode(&self, address: u32, write: bool) -> Result<(usize, u32), BusError> {
        let error = BusError {
            addr: address,
            write,
        };
        if !Self::contains(address) {
            return Err(error);
        }
        let offset = address - BASE;
        let module = (offset / MODULE_STRIDE) as usize;
        let register_offset = offset % MODULE_STRIDE;
        let register = register_offset & !1;
        if register > 4 {
            return Err(error);
        }
        Ok((module, register))
    }

    fn read_register(&self, address: u32, write: bool) -> Result<u16, BusError> {
        if address & 1 != 0 {
            return Err(BusError {
                addr: address,
                write,
            });
        }
        let (module, register) = self.decode(address, write)?;
        Ok(self.timers[module].read(register))
    }

    fn write_register(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        if address & 1 != 0 {
            return Err(BusError {
                addr: address,
                write: true,
            });
        }
        let (module, register) = self.decode(address, true)?;
        self.timers[module].write(register, value);
        Ok(())
    }
}

impl Default for PitController {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for PitController {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        let register_address = address & !1;
        let value = self.read_register(register_address, false)?;
        Ok(if address & 1 == 0 {
            (value >> 8) as u8
        } else {
            value as u8
        })
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        self.read_register(address, false)
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        Err(BusError {
            addr: address,
            write: false,
        })
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        let register_address = address & !1;
        let old = self.read_register(register_address, true)?;
        let next = if address & 1 == 0 {
            (u16::from(value) << 8) | (old & 0x00FF)
        } else {
            (old & 0xFF00) | u16::from(value)
        };
        self.write_register(register_address, next)
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        self.write_register(address, value)
    }

    fn write32(&mut self, address: u32, _value: u32) -> Result<(), BusError> {
        Err(BusError {
            addr: address,
            write: true,
        })
    }

    fn advance_instruction(&mut self) {
        PitController::advance_instruction(self);
    }
}


impl PitController {
    pub fn save_state(&self, w: &mut crate::snapshot::Writer) {
        w.u32(self.ticks_per_instruction);
        for t in &self.timers {
            w.u16(t.pcsr);
            w.u16(t.pmr);
            w.u16(t.counter);
            w.u32(t.prescaler);
            w.u64(t.first_tick_delay);
            w.u64(t.first_tick_delay_remaining);
            w.bool(t.delay_next_enable);
        }
    }

    pub fn load_state(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), String> {
        self.ticks_per_instruction = r.u32()?;
        for t in &mut self.timers {
            t.pcsr = r.u16()?;
            t.pmr = r.u16()?;
            t.counter = r.u16()?;
            t.prescaler = r.u32()?;
            t.first_tick_delay = r.u64()?;
            t.first_tick_delay_remaining = r.u64()?;
            t.delay_next_enable = r.bool()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_timer_sets_pif_after_counter_rollover_and_reloads() {
        let mut pits = PitController::new();
        pits.write16(0xFC08_4002, 0x0083).unwrap();
        pits.write16(0xFC08_4000, 0x0033).unwrap();

        for _ in 0..=u16::MAX {
            pits.advance_instruction();
        }

        assert_eq!(pits.read16(0xFC08_4000).unwrap() & PCSR_PIF, PCSR_PIF);
        assert_eq!(pits.read16(0xFC08_4004).unwrap(), 0x0083);
    }

    #[test]
    fn pmr_write_clears_pif_and_overwrite_loads_counter() {
        let mut pits = PitController::new();
        pits.write16(0xFC08_0000, 0x0011).unwrap();
        pits.write16(0xFC08_0002, 2).unwrap();
        assert_eq!(pits.read16(0xFC08_0004).unwrap(), 2);
        for _ in 0..=2 {
            pits.advance_instruction();
        }
        assert_eq!(pits.read16(0xFC08_0000).unwrap() & PCSR_PIF, PCSR_PIF);

        pits.write16(0xFC08_0002, 4).unwrap();
        assert_eq!(pits.read16(0xFC08_0000).unwrap() & PCSR_PIF, 0);
        assert_eq!(pits.read16(0xFC08_0004).unwrap(), 4);
    }

    #[test]
    fn first_tick_delay_starts_when_pit0_is_enabled_and_applies_once() {
        let mut pits = PitController::new();
        pits.set_first_tick_delay(3);
        pits.write16(0xFC08_0000, PCSR_EN | PCSR_PIE | PCSR_RLD | 0x0010)
            .unwrap();
        pits.write16(0xFC08_0002, 2).unwrap();

        for _ in 0..3 {
            pits.advance_instruction();
        }
        assert!(!pits.interrupt_pending(0));
        assert_eq!(pits.read16(0xFC08_0004).unwrap(), 2);
        assert_eq!(pits.ticks_until_pif(0), Some(3));

        for _ in 0..3 {
            pits.advance_instruction();
        }
        assert!(pits.interrupt_pending(0));
    }

    #[test]
    fn pit_register_width_and_reserved_offsets_are_enforced() {
        let mut pits = PitController::new();
        assert!(pits.read32(0xFC08_0000).is_err());
        assert!(pits.write32(0xFC08_0000, 0).is_err());
        assert!(pits.read16(0xFC08_0001).is_err());
        assert!(pits.read16(0xFC08_0006).is_err());
    }

    #[test]
    fn ticks_until_pif_includes_counter_and_prescaler_remainder() {
        let mut pits = PitController::new();
        pits.write16(0xFC08_4000, 0x0113).unwrap();
        pits.write16(0xFC08_4002, 2).unwrap();
        assert_eq!(pits.ticks_until_pif(1), Some(6));

        pits.fast_forward_ticks(4);
        assert_eq!(pits.ticks_until_pif(1), Some(2));
        pits.fast_forward_ticks(2);
        assert_eq!(pits.ticks_until_pif(1), None);
        assert_ne!(pits.read16(0xFC08_4000).unwrap() & PCSR_PIF, 0);
    }

    #[test]
    fn batched_ticks_match_individual_ticks() {
        for pcsr in [0x0033, 0x0133, 0x0F33, 0x0031] {
            for pmr in [0, 3, 0x0083, u16::MAX] {
                for ticks in [1, 7, 0x1_0203] {
                    let mut individual = PitController::new();
                    let mut batched = PitController::with_ticks_per_instruction(ticks);
                    for pits in [&mut individual, &mut batched] {
                        pits.write16(0xFC08_0002, pmr).unwrap();
                        pits.write16(0xFC08_0000, pcsr).unwrap();
                    }

                    for _ in 0..ticks {
                        individual.advance_instruction();
                    }
                    batched.advance_instruction();

                    assert_eq!(batched.timer_state(0), individual.timer_state(0));
                }
            }
        }
    }
}
