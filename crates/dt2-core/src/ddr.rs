//! MCF5441x DDR controller registers used by the bootstrap (MCF54418RM chapter 21).

use crate::bus::{Bus, BusError};

const RCR: u32 = 0xFC0B_8180;
const PADCR: u32 = 0xFC0B_81AC;
const CR00: u32 = 0xFC0B_8000;
const CR01: u32 = 0xFC0B_8004;
const CR02: u32 = 0xFC0B_8008;
const CR03: u32 = 0xFC0B_800C;
const CR04: u32 = 0xFC0B_8010;
const CR05: u32 = 0xFC0B_8014;
const CR06: u32 = 0xFC0B_8018;
const CR07: u32 = 0xFC0B_801C;
const CR08: u32 = 0xFC0B_8020;
const CR09: u32 = 0xFC0B_8024;
const CR10: u32 = 0xFC0B_8028;
const CR11: u32 = 0xFC0B_802C;
const CR12: u32 = 0xFC0B_8030;
const CR13: u32 = 0xFC0B_8034;
const CR14: u32 = 0xFC0B_8038;
const CR15: u32 = 0xFC0B_803C;
const CR16: u32 = 0xFC0B_8040;
const CR17: u32 = 0xFC0B_8044;
const CR18: u32 = 0xFC0B_8048;
const CR19: u32 = 0xFC0B_804C;
const CR20: u32 = 0xFC0B_8050;
const CR21: u32 = 0xFC0B_8054;
const CR22: u32 = 0xFC0B_8058;
const CR23: u32 = 0xFC0B_805C;
const CR24: u32 = 0xFC0B_8060;
const CR25: u32 = 0xFC0B_8064;
const CR26: u32 = 0xFC0B_8068;
const CR27: u32 = 0xFC0B_806C;
const CR28: u32 = 0xFC0B_8070;
const CR29: u32 = 0xFC0B_8074;
const CR31: u32 = 0xFC0B_807C;
const CR40: u32 = 0xFC0B_80A0;
const CR41: u32 = 0xFC0B_80A4;
const CR42: u32 = 0xFC0B_80A8;
const CR43: u32 = 0xFC0B_80AC;
const CR45: u32 = 0xFC0B_80B4;
const CR56: u32 = 0xFC0B_80E0;
const CR57: u32 = 0xFC0B_80E4;
const CR58: u32 = 0xFC0B_80E8;
const CR59: u32 = 0xFC0B_80EC;
const CR60: u32 = 0xFC0B_80F0;
const RCR_RESET: u32 = 1 << 30;
const PADCR_RESET: u32 = 1 << 9;
const PADCR_FIELDS: u32 = 0x0303_0203;
const CR00_FIELDS: u32 = (1 << 24) | (1 << 16) | (1 << 8) | 1;
const CR01_FIELDS: u32 = (1 << 16) | (1 << 8) | 1;
const CR02_FIELDS: u32 = (1 << 24) | (1 << 16) | (1 << 8) | 1;
const CR03_FIELDS: u32 = (1 << 24) | (1 << 16);
const CR04_FIELDS: u32 = (1 << 16) | (1 << 8) | 1;
const CR05_FIELDS: u32 = (1 << 24) | (1 << 16) | (1 << 8) | 1;
const CR06_FIELDS: u32 = (1 << 16) | (1 << 8);
const CR07_FIELDS: u32 = (1 << 24) | (1 << 8) | 1;
const CR08_FIELDS: u32 = (1 << 24) | (1 << 16) | (1 << 8) | 1;
const CR09_FIELDS: u32 = (1 << 24) | (1 << 16) | (1 << 8) | 1;
const CR10_FIELDS: u32 = (1 << 24) | (1 << 16) | (1 << 8) | 1;
const CR11_FIELDS: u32 = (0x3 << 24) | (0x3 << 16) | (0x3 << 8);
const CR12_FIELDS: u32 = (1 << 24) | (0x3 << 16) | (0x3 << 8) | 0x3;
const CR13_RESET: u32 = 1 << 16;
const CR13_FIELDS: u32 = (1 << 24) | (1 << 8) | 1;
const CR14_FIELDS: u32 = 1 << 8;
const CR15_FIELDS: u32 = (0x7 << 24) | (0x3 << 8) | 0x3;
const CR16_FIELDS: u32 = (0x7 << 24) | (0x7 << 16) | (0x7 << 8) | 0x7;
const CR17_FIELDS: u32 = (0xF << 24) | (0x7 << 16) | 0x7;
const CR18_FIELDS: u32 = (0x7 << 24) | (0xF << 8) | 0x7;
const CR19_FIELDS: u32 = (0xF << 24) | (0xF << 16) | (0xF << 8) | 0x7;
const CR20_READ_ONLY: u32 = 0x0C00_0000;
const CR20_FIELDS: u32 = (0xF << 16) | (0xF << 8) | 0x7;
const CR21_READ_ONLY: u32 = 1 << 10;
const CR21_FIELDS: u32 = (0xF << 16) | 0x1F;
const CR22_FIELDS: u32 = (0xF << 24) | (0xF << 16) | 0xF;
const CR23_READ_ONLY: u32 = 0x1000_0000;
const CR23_FIELDS: u32 = (0x1F << 16) | (0x1F << 8) | 0x1F;
const CR24_FIELDS: u32 = (0x1F << 24) | (0x1F << 16) | (0x1F << 8) | 0x1F;
const CR25_FIELDS: u32 = 0xFF00_001F;
const CR26_FIELDS: u32 = (0x1FF << 16) | (0xFF << 8) | 0xFF;
const CR28_FIELDS: u32 = (0x7FF << 16) | 0x7FF;
const CR31_FIELDS: u32 = 0x3FFF_0000;
const CR45_FIELDS: u32 = 0x00FF_FFFF;
const CR56_FIELDS: u32 = (0xF << 24) | (0x7 << 16);
const CR57_FIELDS: u32 = 0xFF00_0F0F;
const CR58_FIELDS: u32 = 0xFFFF_0000;
const CR40_FIELDS: u32 = 0xFFFF_0000;
const CR41_FIELDS: u32 = u32::MAX;

pub struct DdrController {
    padcr: u32,
    cr00: u32,
    cr01: u32,
    cr02: u32,
    cr03: u32,
    cr04: u32,
    cr05: u32,
    cr06: u32,
    cr07: u32,
    cr08: u32,
    cr09: u32,
    cr10: u32,
    cr11: u32,
    cr12: u32,
    cr13: u32,
    cr14: u32,
    cr15: u32,
    cr16: u32,
    cr17: u32,
    cr18: u32,
    cr19: u32,
    cr20: u32,
    cr21: u32,
    cr22: u32,
    cr23: u32,
    cr24: u32,
    cr25: u32,
    cr26: u32,
    cr28: u32,
    cr29: u32,
    cr31: u32,
    cr40: u32,
    cr41: u32,
    cr42: u32,
    cr43: u32,
    cr45: u32,
    cr56: u32,
    cr57: u32,
    cr58: u32,
    cr59: u32,
    cr60: u32,
}

impl DdrController {
    pub fn new() -> Self {
        Self {
            padcr: PADCR_RESET,
            cr00: 0,
            cr01: 0,
            cr02: 0,
            cr03: 0,
            cr04: 0,
            cr05: 0,
            cr06: 0,
            cr07: 0,
            cr08: 0,
            cr09: 0,
            cr10: 0,
            cr11: 0,
            cr12: 0,
            cr13: CR13_RESET,
            cr14: 0,
            cr15: 0,
            cr16: 0,
            cr17: 0,
            cr18: 0,
            cr19: 0,
            cr20: CR20_READ_ONLY,
            cr21: CR21_READ_ONLY,
            cr22: 0,
            cr23: CR23_READ_ONLY,
            cr24: 0,
            cr25: 0,
            cr26: 0,
            cr28: 0,
            cr29: 0,
            cr31: 0,
            cr40: 0,
            cr41: 0,
            cr42: 0,
            cr43: 0,
            cr45: 0,
            cr56: 0,
            cr57: 0,
            cr58: 0,
            cr59: 0,
            cr60: 0,
        }
    }

    pub fn contains(address: u32) -> bool {
        (RCR..RCR + 4).contains(&address)
            || (PADCR..PADCR + 4).contains(&address)
            || (CR00..CR00 + 4).contains(&address)
            || (CR01..CR01 + 4).contains(&address)
            || (CR02..CR02 + 4).contains(&address)
            || (CR03..CR03 + 4).contains(&address)
            || (CR04..CR04 + 4).contains(&address)
            || (CR05..CR05 + 4).contains(&address)
            || (CR06..CR06 + 4).contains(&address)
            || (CR07..CR07 + 4).contains(&address)
            || (CR08..CR08 + 4).contains(&address)
            || (CR09..CR09 + 4).contains(&address)
            || (CR10..CR10 + 4).contains(&address)
            || (CR11..CR11 + 4).contains(&address)
            || (CR12..CR12 + 4).contains(&address)
            || (CR13..CR13 + 4).contains(&address)
            || (CR14..CR14 + 4).contains(&address)
            || (CR15..CR15 + 4).contains(&address)
            || (CR16..CR16 + 4).contains(&address)
            || (CR17..CR17 + 4).contains(&address)
            || (CR18..CR18 + 4).contains(&address)
            || (CR19..CR19 + 4).contains(&address)
            || (CR20..CR20 + 4).contains(&address)
            || (CR21..CR21 + 4).contains(&address)
            || (CR22..CR22 + 4).contains(&address)
            || (CR23..CR23 + 4).contains(&address)
            || (CR24..CR24 + 4).contains(&address)
            || (CR25..CR25 + 4).contains(&address)
            || (CR26..CR26 + 4).contains(&address)
            || (CR27..CR27 + 4).contains(&address)
            || (CR28..CR28 + 4).contains(&address)
            || (CR29..CR29 + 4).contains(&address)
            || (CR31..CR31 + 4).contains(&address)
            || (CR40..CR40 + 4).contains(&address)
            || (CR41..CR41 + 4).contains(&address)
            || (CR42..CR42 + 4).contains(&address)
            || (CR43..CR43 + 4).contains(&address)
            || (CR45..CR45 + 4).contains(&address)
            || (CR56..CR56 + 4).contains(&address)
            || (CR57..CR57 + 4).contains(&address)
            || (CR58..CR58 + 4).contains(&address)
            || (CR59..CR59 + 4).contains(&address)
            || (CR60..CR60 + 4).contains(&address)
    }

    fn check(address: u32, write: bool) -> Result<(), BusError> {
        if Self::contains(address) {
            Ok(())
        } else {
            Err(BusError {
                addr: address,
                write,
            })
        }
    }

    fn read_register(&self, address: u32) -> Result<u32, BusError> {
        match address {
            CR00 => Ok(self.cr00),
            CR01 => Ok(self.cr01),
            CR02 => Ok(self.cr02),
            CR03 => Ok(self.cr03),
            CR04 => Ok(self.cr04),
            CR05 => Ok(self.cr05),
            CR06 => Ok(self.cr06),
            CR07 => Ok(self.cr07),
            CR08 => Ok(self.cr08),
            CR09 => Ok(self.cr09),
            CR10 => Ok(self.cr10),
            CR11 => Ok(self.cr11),
            CR12 => Ok(self.cr12),
            CR13 => Ok(self.cr13),
            CR14 => Ok(self.cr14),
            CR15 => Ok(self.cr15),
            CR16 => Ok(self.cr16),
            CR17 => Ok(self.cr17),
            CR18 => Ok(self.cr18),
            CR19 => Ok(self.cr19),
            CR20 => Ok(self.cr20),
            CR21 => Ok(self.cr21),
            CR22 => Ok(self.cr22),
            CR23 => Ok(self.cr23),
            CR24 => Ok(self.cr24),
            CR25 => Ok(self.cr25),
            CR26 => Ok(self.cr26),
            CR27 => Ok(0),
            CR28 => Ok(self.cr28),
            CR29 => Ok(self.cr29),
            CR31 => Ok(self.cr31),
            CR40 => Ok(self.cr40),
            CR41 => Ok(self.cr41),
            CR42 => Ok(self.cr42),
            CR43 => Ok(self.cr43),
            CR45 => Ok(self.cr45),
            CR56 => Ok(self.cr56),
            CR57 => Ok(self.cr57),
            CR58 => Ok(self.cr58),
            CR59 => Ok(self.cr59),
            CR60 => Ok(self.cr60),
            RCR => Ok(0),
            PADCR => Ok(self.padcr),
            _ => Err(BusError {
                addr: address,
                write: false,
            }),
        }
    }

    fn write_register(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        match address {
            CR00 => {
                self.cr00 = value & CR00_FIELDS;
                Ok(())
            }
            CR01 => {
                self.cr01 = value & CR01_FIELDS;
                Ok(())
            }
            CR02 => {
                self.cr02 = value & CR02_FIELDS;
                Ok(())
            }
            CR03 => {
                self.cr03 = value & CR03_FIELDS;
                Ok(())
            }
            CR04 => {
                self.cr04 = value & CR04_FIELDS;
                Ok(())
            }
            CR05 => {
                self.cr05 = value & CR05_FIELDS;
                Ok(())
            }
            CR06 => {
                self.cr06 = value & CR06_FIELDS;
                Ok(())
            }
            CR07 => {
                self.cr07 = value & CR07_FIELDS;
                Ok(())
            }
            CR08 => {
                self.cr08 = value & CR08_FIELDS;
                Ok(())
            }
            CR09 => {
                self.cr09 = value & CR09_FIELDS;
                Ok(())
            }
            CR10 => {
                self.cr10 = value & CR10_FIELDS;
                Ok(())
            }
            CR11 => {
                self.cr11 = value & CR11_FIELDS;
                Ok(())
            }
            CR12 => {
                self.cr12 = value & CR12_FIELDS;
                Ok(())
            }
            CR13 => {
                self.cr13 = (value & CR13_FIELDS) | CR13_RESET;
                Ok(())
            }
            CR14 => {
                self.cr14 = value & CR14_FIELDS;
                Ok(())
            }
            CR15 => {
                self.cr15 = value & CR15_FIELDS;
                Ok(())
            }
            CR16 => {
                self.cr16 = value & CR16_FIELDS;
                Ok(())
            }
            CR17 => {
                self.cr17 = value & CR17_FIELDS;
                Ok(())
            }
            CR18 => {
                self.cr18 = value & CR18_FIELDS;
                Ok(())
            }
            CR20 => {
                self.cr20 = (value & CR20_FIELDS) | CR20_READ_ONLY;
                Ok(())
            }
            CR21 => {
                self.cr21 = (value & CR21_FIELDS) | CR21_READ_ONLY;
                Ok(())
            }
            CR22 => {
                self.cr22 = value & CR22_FIELDS;
                Ok(())
            }
            CR23 => {
                self.cr23 = (value & CR23_FIELDS) | CR23_READ_ONLY;
                Ok(())
            }
            CR24 => {
                self.cr24 = value & CR24_FIELDS;
                Ok(())
            }
            CR25 => {
                self.cr25 = value & CR25_FIELDS;
                Ok(())
            }
            CR26 => {
                self.cr26 = value & CR26_FIELDS;
                Ok(())
            }
            CR27 => Ok(()),
            CR28 => {
                self.cr28 = value & CR28_FIELDS;
                Ok(())
            }
            CR29 => {
                self.cr29 = value & CR28_FIELDS;
                Ok(())
            }
            CR31 => {
                self.cr31 = value & CR31_FIELDS;
                Ok(())
            }
            CR40 => {
                self.cr40 = value & CR40_FIELDS;
                Ok(())
            }
            CR41 => {
                self.cr41 = value & CR41_FIELDS;
                Ok(())
            }
            CR42 => {
                self.cr42 = value;
                Ok(())
            }
            CR43 => {
                self.cr43 = value;
                Ok(())
            }
            CR45 => {
                self.cr45 = value & CR45_FIELDS;
                Ok(())
            }
            CR56 => {
                self.cr56 = value & CR56_FIELDS;
                Ok(())
            }
            CR57 => {
                self.cr57 = value & CR57_FIELDS;
                Ok(())
            }
            CR58 => {
                self.cr58 = value & CR58_FIELDS;
                Ok(())
            }
            CR59 => {
                self.cr59 = value;
                Ok(())
            }
            CR60 => {
                self.cr60 = value;
                Ok(())
            }
            CR19 => {
                self.cr19 = value & CR19_FIELDS;
                Ok(())
            }
            RCR => {
                let _reset_requested = value & RCR_RESET != 0;
                Ok(())
            }
            PADCR => {
                self.padcr = (value & PADCR_FIELDS) | PADCR_RESET;
                Ok(())
            }
            _ => Err(BusError {
                addr: address,
                write: true,
            }),
        }
    }

    fn read_byte(&self, address: u32) -> Result<u8, BusError> {
        Self::check(address, false)?;
        let register_address = address & !3;
        Ok((self.read_register(register_address)? >> (24 - 8 * (address & 3))) as u8)
    }

    fn write_byte(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        Self::check(address, true)?;
        let register_address = address & !3;
        let shift = 24 - 8 * (address & 3);
        let old = self.read_register(register_address)?;
        let merged = (old & !(0xFF << shift)) | (u32::from(value) << shift);
        self.write_register(register_address, merged)
    }
}

impl Default for DdrController {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for DdrController {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        self.read_byte(address)
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        Ok(u16::from_be_bytes([
            self.read_byte(address)?,
            self.read_byte(address.wrapping_add(1))?,
        ]))
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        let address = address & !3;
        self.read_register(address)
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        self.write_byte(address, value)
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write_byte(address.wrapping_add(offset as u32), byte)?;
        }
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        self.write_register(address & !3, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u32 = 0xFC0B_8180;

    #[test]
    fn rcr_reset_strobe_is_accepted_and_always_reads_zero() {
        let mut ddr = DdrController::new();
        ddr.write32(BASE, 1 << 30).unwrap();
        assert_eq!(ddr.read32(BASE).unwrap(), 0);
        ddr.write32(BASE, 0xFFFF_FFFF).unwrap();
        assert_eq!(ddr.read32(BASE).unwrap(), 0);
    }

    #[test]
    fn padcr_starts_at_documented_reset_value_and_stores_defined_fields() {
        let mut ddr = DdrController::new();
        assert_eq!(ddr.read32(PADCR).unwrap(), PADCR_RESET);
        ddr.write32(PADCR, 0xFFFF_FFFF).unwrap();
        assert_eq!(ddr.read32(PADCR).unwrap(), PADCR_FIELDS | PADCR_RESET);
    }

    #[test]
    fn cr00_stores_only_its_documented_configuration_fields() {
        let mut ddr = DdrController::new();
        assert_eq!(ddr.read32(CR00).unwrap(), 0);
        ddr.write32(CR00, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR00).unwrap(), CR00_FIELDS);
    }

    #[test]
    fn cr01_stores_configuration_fields_but_not_auto_refresh_strobe() {
        let mut ddr = DdrController::new();
        ddr.write32(CR01, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR01).unwrap(), CR01_FIELDS);
    }

    #[test]
    fn cr02_stores_only_its_documented_configuration_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR02, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR02).unwrap(), CR02_FIELDS);
    }

    #[test]
    fn cr03_stores_dqs_mode_and_dll_bypass_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR03, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR03).unwrap(), CR03_FIELDS);
    }

    #[test]
    fn cr04_stores_self_refresh_bank_and_idle_output_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR04, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR04).unwrap(), CR04_FIELDS);
    }

    #[test]
    fn cr05_stores_its_four_write_interruption_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR05, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR05).unwrap(), CR05_FIELDS);
    }

    #[test]
    fn cr06_stores_only_odten_and_turnaround_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR06, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR06).unwrap(), CR06_FIELDS);
    }

    #[test]
    fn cr07_stores_power_protection_and_placement_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR07, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR07).unwrap(), CR07_FIELDS);
    }

    #[test]
    fn cr08_stores_only_documented_datapath_and_queue_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR08, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR08).unwrap(), CR08_FIELDS);
    }

    #[test]
    fn cr09_stores_start_and_controller_mode_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR09, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR09).unwrap(), CR09_FIELDS);
    }

    #[test]
    fn cr10_stores_refresh_and_command_queue_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR10, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR10).unwrap(), CR10_FIELDS);
    }

    #[test]
    fn cr11_stores_configuration_fields_but_not_mode_register_strobe() {
        let mut ddr = DdrController::new();
        ddr.write32(CR11, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR11).unwrap(), CR11_FIELDS);
    }

    #[test]
    fn cr12_stores_chip_select_and_port_one_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR12, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR12).unwrap(), CR12_FIELDS);
    }

    #[test]
    fn cr13_preserves_read_only_chip_select_count_and_stores_controls() {
        let mut ddr = DdrController::new();
        assert_eq!(ddr.read32(CR13).unwrap(), CR13_RESET);
        ddr.write32(CR13, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR13).unwrap(), CR13_RESET | CR13_FIELDS);
    }

    #[test]
    fn cr14_stores_only_write_odt_chip_select_control() {
        let mut ddr = DdrController::new();
        ddr.write32(CR14, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR14).unwrap(), CR14_FIELDS);
    }

    #[test]
    fn cr15_stores_address_pin_odt_and_queue_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR15, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR15).unwrap(), CR15_FIELDS);
    }

    #[test]
    fn cr16_stores_geometry_and_timing_fields_only() {
        let mut ddr = DdrController::new();
        ddr.write32(CR16, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR16).unwrap(), CR16_FIELDS);
    }

    #[test]
    fn cr17_stores_timing_fields_and_leaves_read_only_error_status_clear() {
        let mut ddr = DdrController::new();
        ddr.write32(CR17, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR17).unwrap(), CR17_FIELDS);
    }

    #[test]
    fn cr18_stores_timing_fields_only() {
        let mut ddr = DdrController::new();
        ddr.write32(CR18, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR18).unwrap(), CR18_FIELDS);
    }

    #[test]
    fn cr19_stores_all_four_timing_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR19, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR19).unwrap(), CR19_FIELDS);
    }

    #[test]
    fn cr20_preserves_read_only_max_column_and_stores_latency_fields() {
        let mut ddr = DdrController::new();
        assert_eq!(ddr.read32(CR20).unwrap(), CR20_READ_ONLY);
        ddr.write32(CR20, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR20).unwrap(), CR20_READ_ONLY | CR20_FIELDS);
    }

    #[test]
    fn cr21_preserves_read_only_status_and_stores_latency_fields() {
        let mut ddr = DdrController::new();
        assert_eq!(ddr.read32(CR21).unwrap(), CR21_READ_ONLY);
        ddr.write32(CR21, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR21).unwrap(), CR21_READ_ONLY | CR21_FIELDS);
    }

    #[test]
    fn cr22_stores_base_timing_fields_but_not_calculated_status() {
        let mut ddr = DdrController::new();
        ddr.write32(CR22, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR22).unwrap(), CR22_FIELDS);
    }

    #[test]
    fn cr23_preserves_max_row_status_and_stores_power_and_write_recovery() {
        let mut ddr = DdrController::new();
        assert_eq!(ddr.read32(CR23).unwrap(), CR23_READ_ONLY);
        ddr.write32(CR23, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR23).unwrap(), CR23_READ_ONLY | CR23_FIELDS);
    }

    #[test]
    fn cr24_stores_timing_and_ocd_adjustment_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR24, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR24).unwrap(), CR24_FIELDS);
    }

    #[test]
    fn cr25_stores_timing_fields_but_not_interrupt_clear_mask() {
        let mut ddr = DdrController::new();
        ddr.write32(CR25, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR25).unwrap(), CR25_FIELDS);
    }

    #[test]
    fn cr26_stores_interrupt_mask_and_timing_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR26, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR26).unwrap(), CR26_FIELDS);
    }

    #[test]
    fn cr27_initialization_status_is_read_only_and_clear_at_reset() {
        let mut ddr = DdrController::new();
        assert_eq!(ddr.read32(CR27).unwrap(), 0);
        ddr.write32(CR27, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR27).unwrap(), 0);
    }

    #[test]
    fn cr28_stores_port_zero_burst_counts() {
        let mut ddr = DdrController::new();
        ddr.write32(CR28, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR28).unwrap(), CR28_FIELDS);
    }

    #[test]
    fn cr29_stores_port_one_burst_counts() {
        let mut ddr = DdrController::new();
        ddr.write32(CR29, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR29).unwrap(), CR28_FIELDS);
    }

    #[test]
    fn cr31_stores_refresh_interval_without_phy_status() {
        let mut ddr = DdrController::new();
        ddr.write32(CR31, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR31).unwrap(), CR31_FIELDS);
    }

    #[test]
    fn cr40_stores_only_the_self_refresh_countdown_field() {
        let mut ddr = DdrController::new();
        ddr.write32(CR40, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR40).unwrap(), CR40_FIELDS);
    }

    #[test]
    fn cr41_stores_the_two_dram_timing_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR41, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR41).unwrap(), CR41_FIELDS);
    }

    #[test]
    fn cr42_stores_both_sixteen_bit_timing_fields() {
        let mut ddr = DdrController::new();
        ddr.write32(CR42, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR42).unwrap(), u32::MAX);
    }

    #[test]
    fn cr43_stores_both_self_refresh_exit_timings() {
        let mut ddr = DdrController::new();
        ddr.write32(CR43, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR43).unwrap(), u32::MAX);
    }

    #[test]
    fn cr45_stores_the_24_bit_initialization_delay() {
        let mut ddr = DdrController::new();
        ddr.write32(CR45, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR45).unwrap(), CR45_FIELDS);
    }

    #[test]
    fn cr56_stores_ddr_class_and_turnaround_fields_without_status() {
        let mut ddr = DdrController::new();
        ddr.write32(CR56, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR56).unwrap(), CR56_FIELDS);
    }

    #[test]
    fn cr57_stores_mode_and_phy_latency_adjustments() {
        let mut ddr = DdrController::new();
        ddr.write32(CR57, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR57).unwrap(), CR57_FIELDS);
    }

    #[test]
    fn cr58_stores_emrs1_data_in_the_upper_half() {
        let mut ddr = DdrController::new();
        ddr.write32(CR58, u32::MAX).unwrap();
        assert_eq!(ddr.read32(CR58).unwrap(), CR58_FIELDS);
    }

    #[test]
    fn cr59_stores_both_extended_mode_register_words() {
        let mut ddr = DdrController::new();
        ddr.write32(CR59, 0xC000_4044).unwrap();
        assert_eq!(ddr.read32(CR59).unwrap(), 0xC000_4044);
    }

    #[test]
    fn cr60_stores_mrs_data_and_extended_mode_register_copy() {
        let mut ddr = DdrController::new();
        ddr.write32(CR60, 0x0642_C000).unwrap();
        assert_eq!(ddr.read32(CR60).unwrap(), 0x0642_C000);
    }

    #[test]
    fn padcr_subword_accesses_preserve_other_fields_and_reserved_defaults() {
        let mut ddr = DdrController::new();
        ddr.write8(PADCR + 1, 0x01).unwrap();
        assert_eq!(ddr.read32(PADCR).unwrap(), PADCR_RESET | (1 << 16));
        assert_eq!(ddr.read16(PADCR + 2).unwrap(), 0x0200);
    }

    #[test]
    fn only_the_observed_registers_are_mapped() {
        let mut ddr = DdrController::new();
        assert!(ddr.read32(BASE - 4).is_err());
        assert!(ddr.read32(BASE + 4).is_err());
        assert!(ddr.read32(CR00 - 4).is_err());
        assert!(ddr.read32(CR60 + 4).is_err());
        assert!(ddr.read32(PADCR - 4).is_err());
        assert!(ddr.read32(PADCR + 4).is_err());
        assert!(ddr.write32(PADCR + 4, 0).is_err());
    }

    #[test]
    fn byte_and_word_accesses_inside_rcr_are_zero_and_accepted() {
        let mut ddr = DdrController::new();
        assert_eq!(ddr.read8(BASE).unwrap(), 0);
        assert_eq!(ddr.read16(BASE + 2).unwrap(), 0);
        ddr.write8(BASE, 0x40).unwrap();
        ddr.write16(BASE + 2, 0xFFFF).unwrap();
        assert_eq!(ddr.read32(BASE).unwrap(), 0);
        assert!(ddr.read16(BASE + 3).is_err());
    }
}
