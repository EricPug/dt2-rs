//! MCF5441x UART8 transmit sink (MCF54418RM chapter 41).
//!
//! Characters are accepted immediately when TXRDY is set. Serial bit timing,
//! receive data, and interrupt generation are not modeled.

use crate::bus::{Bus, BusError};

pub const UART8_BASE: u32 = 0xEC07_0000;
const UART8_END: u32 = UART8_BASE + 0x40;
const UMR: u32 = 0x00;
const USR_UCSR: u32 = 0x04;
const UCR: u32 = 0x08;
const URB_UTB: u32 = 0x0C;
const UIPCR_UACR: u32 = 0x10;
const UISR_UIMR: u32 = 0x14;
const UBG1: u32 = 0x18;
const UBG2: u32 = 0x1C;
const UIP: u32 = 0x34;
const UOP1: u32 = 0x38;
const UOP0: u32 = 0x3C;
const USR_TXEMP: u8 = 1 << 3;
const USR_TXRDY: u8 = 1 << 2;
const UCR_TX_ENABLE: u8 = 0x04;
const UCR_TX_DISABLE: u8 = 0x08;
const UCR_RESET_MODE_POINTER: u8 = 0x10;
const UCR_RESET_RECEIVER: u8 = 0x20;
const UCR_RESET_TRANSMITTER: u8 = 0x30;
const MAX_CAPTURED_BYTES: usize = 4096;

pub struct Uart8 {
    mode_registers: [u8; 2],
    mode_register_pointer: usize,
    clock_select: u8,
    transmitter_enabled: bool,
    receiver_enabled: bool,
    uacr: u8,
    uimr: u8,
    baud: [u8; 2],
    output_port: u8,
    transmitted: u64,
    captured: Vec<u8>,
}

impl Uart8 {
    pub fn new() -> Self {
        Self {
            mode_registers: [0; 2],
            mode_register_pointer: 0,
            clock_select: 0,
            transmitter_enabled: false,
            receiver_enabled: false,
            uacr: 0,
            uimr: 0,
            baud: [0; 2],
            output_port: 0,
            transmitted: 0,
            captured: Vec::new(),
        }
    }

    pub fn contains(address: u32) -> bool {
        (UART8_BASE..UART8_END).contains(&address)
    }

    pub fn transmitted(&self) -> u64 {
        self.transmitted
    }

    pub fn captured(&self) -> &[u8] {
        &self.captured
    }

    fn error(address: u32, write: bool) -> BusError {
        BusError {
            addr: address,
            write,
        }
    }

    fn register(address: u32, write: bool) -> Result<u32, BusError> {
        if !Self::contains(address) {
            return Err(Self::error(address, write));
        }
        Ok(address - UART8_BASE)
    }

    fn read_byte(&mut self, address: u32) -> Result<u8, BusError> {
        let offset = Self::register(address, false)?;
        match offset {
            UMR => {
                let value = self.mode_registers[self.mode_register_pointer];
                self.mode_register_pointer = (self.mode_register_pointer + 1) & 1;
                Ok(value)
            }
            USR_UCSR => Ok(if self.transmitter_enabled {
                USR_TXRDY | USR_TXEMP
            } else {
                0
            }),
            URB_UTB => Ok(0xFF),
            UIPCR_UACR => Ok(0),
            UISR_UIMR => Ok(0),
            UIP => Ok(0xFF),
            _ => Err(Self::error(address, false)),
        }
    }

    fn write_byte(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        let offset = Self::register(address, true)?;
        match offset {
            UMR => {
                self.mode_registers[self.mode_register_pointer] = value;
                self.mode_register_pointer = (self.mode_register_pointer + 1) & 1;
            }
            USR_UCSR => self.clock_select = value,
            UCR => self.write_command(value),
            URB_UTB => {
                if self.transmitter_enabled {
                    self.transmitted = self.transmitted.saturating_add(1);
                    if self.captured.len() < MAX_CAPTURED_BYTES {
                        self.captured.push(value);
                    }
                }
            }
            UIPCR_UACR => self.uacr = value,
            UISR_UIMR => self.uimr = value,
            UBG1 => self.baud[0] = value,
            UBG2 => self.baud[1] = value,
            UOP1 => self.output_port |= value,
            UOP0 => self.output_port &= !value,
            _ => return Err(Self::error(address, true)),
        }
        Ok(())
    }

    fn write_command(&mut self, value: u8) {
        match value & 0x70 {
            UCR_RESET_MODE_POINTER => self.mode_register_pointer = 0,
            UCR_RESET_RECEIVER => self.receiver_enabled = false,
            UCR_RESET_TRANSMITTER => self.transmitter_enabled = false,
            _ => {}
        }
        match value & 0x0C {
            UCR_TX_ENABLE => self.transmitter_enabled = true,
            UCR_TX_DISABLE => self.transmitter_enabled = false,
            _ => {}
        }
        match value & 0x03 {
            1 => self.receiver_enabled = true,
            2 => self.receiver_enabled = false,
            _ => {}
        }
    }
}

impl Default for Uart8 {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for Uart8 {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        self.read_byte(address)
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        Err(Self::error(address, false))
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        Err(Self::error(address, false))
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        self.write_byte(address, value)
    }

    fn write16(&mut self, address: u32, _value: u16) -> Result<(), BusError> {
        Err(Self::error(address, true))
    }

    fn write32(&mut self, address: u32, _value: u32) -> Result<(), BusError> {
        Err(Self::error(address, true))
    }

    fn advance_instruction(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_enable_sets_ready_and_transmit_buffer_captures_bytes() {
        let mut uart = Uart8::new();
        assert_eq!(uart.read8(UART8_BASE + USR_UCSR).unwrap() & USR_TXRDY, 0);

        uart.write8(UART8_BASE + UCR, UCR_TX_ENABLE).unwrap();
        assert_eq!(
            uart.read8(UART8_BASE + USR_UCSR).unwrap(),
            USR_TXRDY | USR_TXEMP
        );
        for byte in b"BOOT" {
            uart.write8(UART8_BASE + URB_UTB, *byte).unwrap();
        }

        assert_eq!(uart.transmitted(), 4);
        assert_eq!(uart.captured(), b"BOOT");
        assert_eq!(
            uart.read8(UART8_BASE + USR_UCSR).unwrap() & USR_TXRDY,
            USR_TXRDY
        );
    }

    #[test]
    fn transmitter_reset_clears_ready_and_receive_is_not_fabricated() {
        let mut uart = Uart8::new();
        uart.write8(UART8_BASE + UCR, UCR_TX_ENABLE).unwrap();
        uart.write8(UART8_BASE + UCR, UCR_RESET_TRANSMITTER)
            .unwrap();

        assert_eq!(uart.read8(UART8_BASE + USR_UCSR).unwrap(), 0);
        assert_eq!(uart.read8(UART8_BASE + URB_UTB).unwrap(), 0xFF);
    }

    #[test]
    fn uart8_rejects_wide_accesses_and_caps_captured_prefix() {
        let mut uart = Uart8::new();
        uart.write8(UART8_BASE + UCR, UCR_TX_ENABLE).unwrap();
        assert!(uart.read16(UART8_BASE + USR_UCSR).is_err());
        assert!(uart.write32(UART8_BASE + URB_UTB, 0).is_err());
        for byte in 0..=MAX_CAPTURED_BYTES {
            uart.write8(UART8_BASE + URB_UTB, byte as u8).unwrap();
        }
        assert_eq!(uart.transmitted(), (MAX_CAPTURED_BYTES + 1) as u64);
        assert_eq!(uart.captured().len(), MAX_CAPTURED_BYTES);
    }

    #[test]
    fn uart8_retains_the_observed_bootstrap_stream() {
        let mut uart = Uart8::new();
        uart.write8(UART8_BASE + UCR, UCR_TX_ENABLE).unwrap();
        for byte in 0..1321 {
            uart.write8(UART8_BASE + URB_UTB, byte as u8).unwrap();
        }

        assert_eq!(uart.transmitted(), 1321);
        assert_eq!(uart.captured().len(), 1321);
    }
}
