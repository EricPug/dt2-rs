//! Winbond W25Q128JV-IQ 128 Mb SPI NOR flash, read-only subset (Winbond W25Q128JV datasheet, 8.1-8.2).

use crate::dspi::SpiDevice;

const SIZE: usize = 16 * 1024 * 1024;
const MANUFACTURER_ID: u8 = 0xEF;
const DEVICE_ID: u8 = 0x17;
const JEDEC_ID: [u8; 3] = [MANUFACTURER_ID, 0x40, 0x18];

pub struct W25q128jv {
    data: Vec<u8>,
    command: Option<u8>,
    received: usize,
    address: u32,
    write_enabled: bool,
    unsupported: Option<u8>,
}

impl W25q128jv {
    pub fn blank() -> Self {
        Self {
            data: vec![0xFF; SIZE],
            command: None,
            received: 0,
            address: 0,
            write_enabled: false,
            unsupported: None,
        }
    }

    /// Image bytes load at address 0; the rest of the array stays erased (0xFF).
    pub fn with_image(image: &[u8]) -> Result<Self, String> {
        if image.len() > SIZE {
            return Err(format!(
                "flash image is {} bytes; the W25Q128JV holds {SIZE}",
                image.len()
            ));
        }
        let mut flash = Self::blank();
        flash.data[..image.len()].copy_from_slice(image);
        Ok(flash)
    }

    fn read_array(&mut self) -> u8 {
        let byte = self.data[self.address as usize];
        self.address = (self.address + 1) & (SIZE as u32 - 1);
        byte
    }

    fn accumulate_address(&mut self, byte: u8) {
        self.address = (self.address << 8 | u32::from(byte)) & (SIZE as u32 - 1);
    }
}

impl SpiDevice for W25q128jv {
    fn save_state(&self, w: &mut crate::snapshot::Writer) {
        w.u16(self.command.map_or(0, |c| 0x100 | u16::from(c)));
        w.u64(self.received as u64);
        w.u32(self.address);
        w.bool(self.write_enabled);
        w.u16(self.unsupported.map_or(0, |c| 0x100 | u16::from(c)));
    }

    fn load_state(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), String> {
        let opt = |v: u16| (v & 0x100 != 0).then_some(v as u8);
        self.command = opt(r.u16()?);
        self.received = r.u64()? as usize;
        self.address = r.u32()?;
        self.write_enabled = r.bool()?;
        self.unsupported = opt(r.u16()?);
        Ok(())
    }

    fn select(&mut self) {
        self.command = None;
        self.received = 0;
        self.address = 0;
    }

    fn deselect(&mut self) {
        self.command = None;
    }

    fn transfer_byte(&mut self, mosi: u8) -> u8 {
        let Some(command) = self.command else {
            self.command = Some(mosi);
            self.received = 0;
            self.address = 0;
            match mosi {
                0x06 => self.write_enabled = true,
                0x04 => self.write_enabled = false,
                0x9F | 0x90 | 0xAB | 0x05 | 0x03 | 0x0B => {}
                other => self.unsupported = self.unsupported.or(Some(other)),
            }
            return 0;
        };
        self.received += 1;
        let n = self.received;
        match command {
            0x9F => n
                .checked_sub(1)
                .and_then(|index| JEDEC_ID.get(index))
                .copied()
                .unwrap_or(0),
            0xAB => {
                if n >= 4 {
                    DEVICE_ID
                } else {
                    0
                }
            }
            0x90 => {
                if n <= 3 {
                    self.accumulate_address(mosi);
                    0
                } else if (n - 4 + (self.address as usize & 1)) % 2 == 0 {
                    MANUFACTURER_ID
                } else {
                    DEVICE_ID
                }
            }
            // BUSY (S0) stays 0 because programming and erasing are not modelled.
            0x05 => u8::from(self.write_enabled) << 1,
            0x03 | 0x0B => {
                let data_start = if command == 0x0B { 5 } else { 4 };
                if n <= 3 {
                    self.accumulate_address(mosi);
                    0
                } else if n < data_start {
                    0
                } else {
                    self.read_array()
                }
            }
            _ => 0,
        }
    }

    fn unsupported_command(&self) -> Option<u8> {
        self.unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(flash: &mut W25q128jv, tx: &[u8]) -> Vec<u8> {
        flash.select();
        let out = tx.iter().map(|&byte| flash.transfer_byte(byte)).collect();
        flash.deselect();
        out
    }

    fn flash() -> W25q128jv {
        let mut image = vec![0u8; 0x200];
        image[0x100..0x104].copy_from_slice(&[1, 2, 3, 4]);
        W25q128jv::with_image(&image).unwrap()
    }

    #[test]
    fn jedec_id_matches_the_datasheet() {
        assert_eq!(run(&mut flash(), &[0x9F, 0, 0, 0]), [0, 0xEF, 0x40, 0x18]);
    }

    #[test]
    fn release_power_down_returns_the_device_id() {
        assert_eq!(
            run(&mut flash(), &[0xAB, 0, 0, 0, 0, 0]),
            [0, 0, 0, 0, 0x17, 0x17]
        );
    }

    #[test]
    fn manufacturer_and_device_id_follow_address_bit_zero() {
        assert_eq!(
            run(&mut flash(), &[0x90, 0, 0, 0, 0, 0]),
            [0, 0, 0, 0, 0xEF, 0x17]
        );
        assert_eq!(
            run(&mut flash(), &[0x90, 0, 0, 1, 0, 0]),
            [0, 0, 0, 0, 0x17, 0xEF]
        );
    }

    #[test]
    fn read_data_returns_array_bytes_from_the_address() {
        assert_eq!(
            run(&mut flash(), &[0x03, 0, 1, 0, 0, 0, 0, 0]),
            [0, 0, 0, 0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn fast_read_has_one_dummy_byte() {
        assert_eq!(
            run(&mut flash(), &[0x0B, 0, 1, 0, 0, 0, 0]),
            [0, 0, 0, 0, 0, 1, 2]
        );
    }

    #[test]
    fn reads_wrap_at_the_end_of_the_array() {
        let mut flash = W25q128jv::blank();
        flash.data[SIZE - 1] = 0xAA;
        flash.data[0] = 0xBB;
        assert_eq!(
            run(&mut flash, &[0x03, 0xFF, 0xFF, 0xFF, 0, 0]),
            [0, 0, 0, 0, 0xAA, 0xBB]
        );
    }

    #[test]
    fn chip_select_restarts_the_command_phase() {
        let mut flash = flash();
        flash.select();
        flash.transfer_byte(0x9F);
        flash.deselect();
        flash.select();
        assert_eq!(flash.transfer_byte(0x05), 0);
        assert_eq!(flash.transfer_byte(0), 0);
    }

    #[test]
    fn write_enable_sets_and_clears_the_latch_in_status_register_1() {
        let mut flash = flash();
        run(&mut flash, &[0x06]);
        assert_eq!(run(&mut flash, &[0x05, 0]), [0, 0x02]);
        run(&mut flash, &[0x04]);
        assert_eq!(run(&mut flash, &[0x05, 0]), [0, 0x00]);
    }

    #[test]
    fn erased_space_reads_as_ones_and_oversized_images_are_rejected() {
        assert_eq!(run(&mut flash(), &[0x03, 0, 0, 0x10, 0, 0])[4..], [0, 0]);
        assert_eq!(
            run(&mut flash(), &[0x03, 0, 0x10, 0, 0, 0])[4..],
            [0xFF, 0xFF]
        );
        assert!(W25q128jv::with_image(&vec![0; SIZE + 1]).is_err());
    }

    #[test]
    fn unknown_commands_are_recorded() {
        let mut flash = flash();
        run(&mut flash, &[0xC7]);
        assert_eq!(flash.unsupported_command(), Some(0xC7));
    }
}
