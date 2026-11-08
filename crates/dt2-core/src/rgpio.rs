//! MCF5441x rapid GPIO programming model (MCF54418RM chapter 16).

use crate::bus::{Bus, BusError};

pub const RGPIO_BASE: u32 = 0x8C00_0000;
const RGPIO_SIZE: u32 = 0x10;

#[derive(Default)]
pub struct Rgpio {
    direction: u16,
    data: u16,
    enable: u16,
    input_high: u16,
    ready_model: bool,
    busy_polls: std::cell::Cell<u8>,
}

impl Rgpio {
    pub fn new() -> Self {
        Self::default()
    }

    /// Diagnostic: pins forced high when read, standing in for an unmodeled external driver.
    pub fn set_input_high(&mut self, mask: u16) {
        self.input_high = mask;
    }

    /// Diagnostic: pin 0 reads low for one poll after pin 7 rises, then high (a synthetic ready line).
    pub fn set_ready_model(&mut self, enabled: bool) {
        self.ready_model = enabled;
    }

    pub fn contains(address: u32) -> bool {
        (RGPIO_BASE..RGPIO_BASE + RGPIO_SIZE).contains(&address)
    }

    fn error(address: u32, write: bool) -> BusError {
        BusError {
            addr: address,
            write,
        }
    }

    fn read_word(&self, address: u32) -> Result<u16, BusError> {
        if address & 1 != 0 || !Self::contains(address) {
            return Err(Self::error(address, false));
        }
        Ok(match address - RGPIO_BASE {
            0x0 | 0x8 | 0xC => self.direction,
            0x2 | 0x6 | 0xA | 0xE => {
                let mut value = self.data | self.input_high;
                if self.ready_model && address - RGPIO_BASE == 0x2 {
                    let busy = self.busy_polls.get();
                    value = if busy > 0 { value & !1 } else { value | 1 };
                    self.busy_polls.set(busy.saturating_sub(1));
                }
                value
            }
            0x4 => self.enable,
            _ => return Err(Self::error(address, false)),
        })
    }

    fn write_word(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        if address & 1 != 0 || !Self::contains(address) {
            return Err(Self::error(address, true));
        }
        let rising_strobe = value & 0x80 != 0 && self.data & 0x80 == 0;
        if rising_strobe && matches!(address - RGPIO_BASE, 0x2 | 0xA) {
            self.busy_polls.set(1);
        }
        match address - RGPIO_BASE {
            0x0 => self.direction = value,
            0x2 => self.data = value,
            0x4 => self.enable = value,
            0x6 => self.data &= !value,
            0x8 | 0xC => {}
            0xA => self.data |= value,
            0xE => self.data ^= value,
            _ => return Err(Self::error(address, true)),
        }
        Ok(())
    }
}

impl Bus for Rgpio {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        let word_address = address & !1;
        let word = self.read_word(word_address)?;
        Ok(if address & 1 == 0 {
            (word >> 8) as u8
        } else {
            word as u8
        })
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        self.read_word(address)
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        if address & 3 != 0 {
            return Err(Self::error(address, false));
        }
        let high = self.read_word(address)?;
        let low = self.read_word(address.wrapping_add(2))?;
        Ok(u32::from_be_bytes([
            (high >> 8) as u8,
            high as u8,
            (low >> 8) as u8,
            low as u8,
        ]))
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        if !Self::contains(address) {
            return Err(Self::error(address, true));
        }
        let offset = address - RGPIO_BASE;
        let lane = if address & 1 == 0 {
            u16::from(value) << 8
        } else {
            u16::from(value)
        };
        match offset & !1 {
            0x0 => {
                self.direction =
                    (self.direction & !if address & 1 == 0 { 0xFF00 } else { 0x00FF }) | lane;
            }
            0x2 => {
                self.data = (self.data & !if address & 1 == 0 { 0xFF00 } else { 0x00FF }) | lane;
            }
            0x4 => {
                self.enable =
                    (self.enable & !if address & 1 == 0 { 0xFF00 } else { 0x00FF }) | lane;
            }
            0x6 => self.data &= !lane,
            0x8 | 0xC => {}
            0xA => self.data |= lane,
            0xE => self.data ^= lane,
            _ => return Err(Self::error(address, true)),
        }
        Ok(())
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        self.write_word(address, value)
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        if address & 3 != 0 {
            return Err(Self::error(address, true));
        }
        let bytes = value.to_be_bytes();
        self.write_word(address, u16::from_be_bytes([bytes[0], bytes[1]]))?;
        self.write_word(
            address.wrapping_add(2),
            u16::from_be_bytes([bytes[2], bytes[3]]),
        )
    }
}

impl Rgpio {
    pub fn save_state(&self, w: &mut crate::snapshot::Writer) {
        w.u16(self.direction);
        w.u16(self.data);
        w.u16(self.enable);
    }

    pub fn load_state(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), String> {
        self.direction = r.u16()?;
        self.data = r.u16()?;
        self.enable = r.u16()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Bus, Rgpio, RGPIO_BASE};

    #[test]
    fn aliases_read_data_and_apply_write_set_clear_toggle() {
        let mut gpio = Rgpio::new();
        gpio.write16(RGPIO_BASE, 0x0080).unwrap();
        gpio.write16(RGPIO_BASE + 4, 0x0081).unwrap();
        gpio.write16(RGPIO_BASE + 0xA, 0x0003).unwrap();
        assert_eq!(gpio.read16(RGPIO_BASE + 2).unwrap(), 0x0003);
        assert_eq!(gpio.read16(RGPIO_BASE + 6).unwrap(), 0x0003);
        gpio.write16(RGPIO_BASE + 6, 0x0001).unwrap();
        assert_eq!(gpio.read16(RGPIO_BASE + 0xA).unwrap(), 0x0002);
        gpio.write16(RGPIO_BASE + 0xE, 0x0003).unwrap();
        assert_eq!(gpio.read16(RGPIO_BASE + 0xE).unwrap(), 0x0001);
        assert_eq!(gpio.read16(RGPIO_BASE + 8).unwrap(), 0x0080);
    }

    #[test]
    fn byte_writes_apply_only_the_selected_alias_lane() {
        let mut gpio = Rgpio::new();
        gpio.write16(RGPIO_BASE + 2, 0xFFFF).unwrap();

        gpio.write8(RGPIO_BASE + 7, 0x01).unwrap();
        assert_eq!(gpio.read16(RGPIO_BASE + 2).unwrap(), 0xFFFE);
        gpio.write8(RGPIO_BASE + 0xE, 0x01).unwrap();
        assert_eq!(gpio.read16(RGPIO_BASE + 2).unwrap(), 0xFEFE);
        gpio.write8(RGPIO_BASE + 0xB, 0x01).unwrap();
        assert_eq!(gpio.read16(RGPIO_BASE + 2).unwrap(), 0xFEFF);
    }
}
