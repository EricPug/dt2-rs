//! MCF5441x DSPI in SPI master mode (MCF54418RM chapter 40).
//!
//! Frames complete instantly when started, so SCK timing, delays and DMA are not modelled.

use crate::bus::{Bus, BusError};
use std::collections::VecDeque;

/// A device on a DSPI chip-select line, driven one byte at a time.
pub trait SpiDevice {
    fn select(&mut self);
    fn deselect(&mut self);
    fn transfer_byte(&mut self, mosi: u8) -> u8;
    fn unsupported_command(&self) -> Option<u8> {
        None
    }
    fn save_state(&self, _w: &mut crate::snapshot::Writer) {}
    fn load_state(&mut self, _r: &mut crate::snapshot::Reader) -> Result<(), String> {
        Ok(())
    }
}

const FIFO_DEPTH: usize = 16;
const BLOCK_SIZE: u32 = 0xC0;

const MCR_MSTR: u32 = 1 << 31;
const MCR_MDIS: u32 = 1 << 14;
const MCR_CLR_TXF: u32 = 1 << 11;
const MCR_CLR_RXF: u32 = 1 << 10;
const MCR_HALT: u32 = 1;
const MCR_RESET: u32 = MCR_MDIS | MCR_HALT;
const CTAR_RESET: u32 = 0x7800_0000;

const SR_TCF: u32 = 1 << 31;
const SR_TXRXS: u32 = 1 << 30;
const SR_EOQF: u32 = 1 << 28;
const SR_TFFF: u32 = 1 << 25;
const SR_RFOF: u32 = 1 << 19;
const SR_RFDF: u32 = 1 << 17;
const SR_W1C: u32 = SR_TCF | SR_EOQF | SR_RFOF | SR_RFDF;

const REG_MCR: u32 = 0x00;
const REG_TCR: u32 = 0x08;
const REG_CTAR0: u32 = 0x0C;
const REG_CTAR7: u32 = 0x28;
const REG_SR: u32 = 0x2C;
const REG_RSER: u32 = 0x30;
const REG_PUSHR: u32 = 0x34;
const REG_POPR: u32 = 0x38;

pub struct Dspi {
    base: u32,
    mcr: u32,
    tcr: u32,
    ctar: [u32; 8],
    rser: u32,
    flags: u32,
    tfff_enabled: bool,
    tx: VecDeque<u32>,
    rx: VecDeque<u16>,
    tx_next: u32,
    pop_next: u32,
    asserted: u8,
    devices: [Option<Box<dyn SpiDevice>>; 8],
    unsupported: Option<&'static str>,
}

impl Dspi {
    pub fn new(base: u32) -> Self {
        Self {
            base,
            mcr: MCR_RESET,
            tcr: 0,
            ctar: [CTAR_RESET; 8],
            rser: 0,
            flags: 0,
            tfff_enabled: false,
            tx: VecDeque::new(),
            rx: VecDeque::new(),
            tx_next: 0,
            pop_next: 0,
            asserted: 0,
            devices: Default::default(),
            unsupported: None,
        }
    }

    pub fn attach(&mut self, pcs: usize, device: Box<dyn SpiDevice>) {
        self.devices[pcs] = Some(device);
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < BLOCK_SIZE
    }

    /// First behaviour the model could not reproduce, if any.
    pub fn unsupported(&self) -> Option<String> {
        if let Some(reason) = self.unsupported {
            return Some(reason.to_string());
        }
        self.devices
            .iter()
            .flatten()
            .find_map(|device| device.unsupported_command())
            .map(|command| format!("SPI device command {command:#04x}"))
    }

    fn running(&self) -> bool {
        self.mcr & MCR_MSTR != 0
            && self.mcr & (MCR_MDIS | MCR_HALT) == 0
            && self.flags & SR_EOQF == 0
    }

    // TFFF follows "TX FIFO not full" because frames complete before software can clear it.
    fn status(&self) -> u32 {
        let mut sr = self.flags & SR_W1C;
        if self.running() {
            sr |= SR_TXRXS;
        }
        if self.tfff_enabled && self.tx.len() < FIFO_DEPTH {
            sr |= SR_TFFF;
        }
        sr | (self.tx.len() as u32 & 0xF) << 12
            | (self.tx_next & 0xF) << 8
            | (self.rx.len() as u32 & 0xF) << 4
            | (self.pop_next & 0xF)
    }

    fn pump(&mut self) {
        // Diagnostic instant-clock mode: queued PUSHR entries complete on write, even while
        // MCR.HALT/MDIS is set. This models an external clocking peer, not the DSPI clock gate.
        while self.flags & SR_EOQF == 0 {
            let Some(command) = self.tx.pop_front() else {
                break;
            };
            self.tx_next = self.tx_next.wrapping_add(1);
            self.execute(command);
        }
    }

    fn execute(&mut self, command: u32) {
        let continuous = command & (1 << 31) != 0;
        let ctar = self.ctar[(command >> 28 & 7) as usize];
        let pcs = (command >> 16) as u8;
        if command & (1 << 26) != 0 {
            self.tcr = 0;
        }

        let leaving = self.asserted & !pcs;
        let entering = pcs & !self.asserted;
        for line in 0..8 {
            if let Some(device) = &mut self.devices[line] {
                if leaving >> line & 1 != 0 {
                    device.deselect();
                }
                if entering >> line & 1 != 0 {
                    device.select();
                }
            }
        }
        self.asserted = pcs;

        let bits = (ctar >> 27 & 0xF) + 1;
        let received = if ctar & (1 << 24) != 0 {
            self.unsupported = Some("LSB-first DSPI frame");
            0
        } else if bits % 8 != 0 {
            self.unsupported = Some("DSPI frame size that is not a whole number of bytes");
            0
        } else {
            self.shift_frame(command as u16, bits)
        };

        if !continuous {
            for device in self.devices.iter_mut().flatten() {
                device.deselect();
            }
            self.asserted = 0;
        }

        self.tcr = self.tcr.wrapping_add(1 << 16);
        self.flags |= SR_TCF;
        if self.rx.len() < FIFO_DEPTH {
            self.rx.push_back(received);
            self.flags |= SR_RFDF;
        } else {
            self.flags |= SR_RFOF;
        }
        if command & (1 << 27) != 0 {
            self.flags |= SR_EOQF;
        }
    }

    fn shift_frame(&mut self, data: u16, bits: u32) -> u16 {
        let has_selected_device =
            (0..8).any(|line| self.asserted & (1 << line) != 0 && self.devices[line].is_some());
        if !has_selected_device {
            return u16::MAX;
        }
        let mut received = 0u16;
        for byte in (0..bits / 8).rev() {
            let mosi = (data >> (byte * 8)) as u8;
            let mut miso = 0;
            for line in 0..8 {
                if self.asserted >> line & 1 != 0 {
                    if let Some(device) = &mut self.devices[line] {
                        miso |= device.transfer_byte(mosi);
                    }
                }
            }
            received = received << 8 | u16::from(miso);
        }
        received
    }

    fn read_register(&mut self, offset: u32) -> Option<u32> {
        match offset {
            REG_MCR => Some(self.mcr),
            REG_TCR => Some(self.tcr),
            REG_CTAR0..=REG_CTAR7 => Some(self.ctar[((offset - REG_CTAR0) / 4) as usize]),
            REG_SR => Some(self.status()),
            REG_RSER => Some(self.rser),
            // Documented as write-only data; the value read back is not specified.
            REG_PUSHR => Some(0),
            REG_POPR => Some(match self.rx.pop_front() {
                Some(data) => {
                    self.pop_next = self.pop_next.wrapping_add(1);
                    u32::from(data)
                }
                None => 0,
            }),
            _ => None,
        }
    }

    fn write_register(&mut self, offset: u32, value: u32) -> Option<()> {
        match offset {
            REG_MCR => {
                if self.mcr & MCR_MDIS != 0 && value & MCR_MDIS == 0 {
                    self.tfff_enabled = true;
                }
                if value & MCR_CLR_TXF != 0 {
                    self.tx.clear();
                    self.tx_next = 0;
                }
                if value & MCR_CLR_RXF != 0 {
                    self.rx.clear();
                    self.pop_next = 0;
                }
                self.mcr = value & !(MCR_CLR_TXF | MCR_CLR_RXF);
                self.pump();
            }
            REG_TCR => self.tcr = value & 0xFFFF_0000,
            REG_CTAR0..=REG_CTAR7 => self.ctar[((offset - REG_CTAR0) / 4) as usize] = value,
            REG_SR => {
                self.flags &= !(value & SR_W1C);
                self.pump();
            }
            REG_RSER => self.rser = value,
            REG_PUSHR => {
                if self.tx.len() < FIFO_DEPTH {
                    self.tx.push_back(value);
                }
                self.pump();
            }
            REG_POPR => {}
            _ => return None,
        }
        Some(())
    }

    fn read_word(&mut self, address: u32) -> Result<u32, BusError> {
        let offset = address.wrapping_sub(self.base) & !3;
        self.read_register(offset).ok_or(BusError {
            addr: address,
            write: false,
        })
    }

    fn write_lane(
        &mut self,
        address: u32,
        shift: u32,
        mask: u32,
        value: u32,
    ) -> Result<(), BusError> {
        let error = BusError {
            addr: address,
            write: true,
        };
        let offset = address.wrapping_sub(self.base) & !3;
        let merged = match offset {
            REG_PUSHR => return Err(error),
            REG_POPR => return Ok(()),
            // Write-1-to-clear: bits outside the written lane must not be written as 1.
            REG_SR => value << shift,
            _ => {
                let old = self.read_register(offset).ok_or(error)?;
                old & !(mask << shift) | value << shift
            }
        };
        self.write_register(offset, merged).ok_or(error)
    }
}

impl Bus for Dspi {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        Ok((self.read_word(address)? >> (24 - 8 * (address & 3))) as u8)
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        if address & 3 == 3 {
            return Err(BusError {
                addr: address,
                write: false,
            });
        }
        Ok((self.read_word(address)? >> (16 - 8 * (address & 3))) as u16)
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        self.read_word(address)
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        self.write_lane(address, 24 - 8 * (address & 3), 0xFF, u32::from(value))
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        if address & 3 == 3 {
            return Err(BusError {
                addr: address,
                write: true,
            });
        }
        self.write_lane(address, 16 - 8 * (address & 3), 0xFFFF, u32::from(value))
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        let offset = address.wrapping_sub(self.base) & !3;
        self.write_register(offset, value).ok_or(BusError {
            addr: address,
            write: true,
        })
    }
}


impl Dspi {
    pub fn save_state(&self, w: &mut crate::snapshot::Writer) {
        w.u32(self.mcr);
        w.u32(self.tcr);
        for v in self.ctar {
            w.u32(v);
        }
        w.u32(self.rser);
        w.u32(self.flags);
        w.bool(self.tfff_enabled);
        w.u64(self.tx.len() as u64);
        for v in &self.tx {
            w.u32(*v);
        }
        w.u64(self.rx.len() as u64);
        for v in &self.rx {
            w.u16(*v);
        }
        w.u32(self.tx_next);
        w.u32(self.pop_next);
        w.u8(self.asserted);
        for device in &self.devices {
            w.bool(device.is_some());
            if let Some(device) = device {
                device.save_state(w);
            }
        }
    }

    pub fn load_state(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), String> {
        self.mcr = r.u32()?;
        self.tcr = r.u32()?;
        for v in &mut self.ctar {
            *v = r.u32()?;
        }
        self.rser = r.u32()?;
        self.flags = r.u32()?;
        self.tfff_enabled = r.bool()?;
        let n = r.u64()?;
        self.tx = (0..n).map(|_| r.u32()).collect::<Result<_, _>>()?;
        let n = r.u64()?;
        self.rx = (0..n).map(|_| r.u16()).collect::<Result<_, _>>()?;
        self.tx_next = r.u32()?;
        self.pop_next = r.u32()?;
        self.asserted = r.u8()?;
        for device in &mut self.devices {
            let present = r.bool()?;
            match (present, device.as_mut()) {
                (true, Some(device)) => device.load_state(r)?,
                (false, None) => {}
                _ => return Err("DSPI attached-device mismatch".into()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spi_nor::W25q128jv;

    const BASE: u32 = 0xFC05_C000;
    const MCR_RUN: u32 = MCR_MSTR;

    fn flash_dspi() -> Dspi {
        let mut image = vec![0xFF; 16];
        image[..4].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let mut dspi = Dspi::new(BASE);
        dspi.attach(1, Box::new(W25q128jv::with_image(&image).unwrap()));
        dspi.write32(BASE + REG_CTAR0, 0x3800_0000).unwrap();
        dspi
    }

    fn pop(dspi: &mut Dspi) -> u32 {
        dspi.read32(BASE + REG_POPR).unwrap()
    }

    #[test]
    fn reset_state_is_stopped_with_documented_defaults() {
        let mut dspi = Dspi::new(BASE);
        assert_eq!(dspi.read32(BASE + REG_MCR).unwrap(), 0x0000_4001);
        assert_eq!(dspi.read32(BASE + REG_CTAR0).unwrap(), 0x7800_0000);
        assert_eq!(dspi.read32(BASE + REG_SR).unwrap() & SR_TXRXS, 0);
    }

    #[test]
    fn tfff_is_set_once_the_module_is_enabled() {
        let mut dspi = Dspi::new(BASE);
        assert_eq!(dspi.read32(BASE + REG_SR).unwrap() & SR_TFFF, 0);
        dspi.write32(BASE + REG_MCR, MCR_RUN | MCR_HALT).unwrap();
        assert_ne!(dspi.read32(BASE + REG_SR).unwrap() & SR_TFFF, 0);
    }

    #[test]
    fn jedec_id_is_read_through_continuous_chip_select() {
        let mut dspi = flash_dspi();
        dspi.write32(BASE + REG_MCR, MCR_RUN).unwrap();
        dspi.write32(BASE + REG_PUSHR, 0x8402_009F).unwrap();
        for _ in 0..2 {
            dspi.write32(BASE + REG_PUSHR, 0x8002_0000).unwrap();
        }
        dspi.write32(BASE + REG_PUSHR, 0x0002_0000).unwrap();
        assert_eq!(pop(&mut dspi), 0);
        assert_eq!(pop(&mut dspi), 0xEF);
        assert_eq!(pop(&mut dspi), 0x40);
        assert_eq!(pop(&mut dspi), 0x18);
        assert_eq!(dspi.read32(BASE + REG_TCR).unwrap(), 4 << 16);
    }

    #[test]
    fn sixteen_bit_frame_reads_flash_data_big_endian() {
        let mut dspi = flash_dspi();
        dspi.write32(BASE + REG_CTAR0, 0x7800_0000).unwrap();
        dspi.write32(BASE + REG_MCR, MCR_RUN).unwrap();
        for word in [0x8002_0300, 0x8002_0000, 0x8002_0000, 0x0002_0000] {
            dspi.write32(BASE + REG_PUSHR, word).unwrap();
        }
        assert_eq!(pop(&mut dspi), 0);
        assert_eq!(pop(&mut dspi), 0);
        assert_eq!(pop(&mut dspi), 0xDEAD);
        assert_eq!(pop(&mut dspi), 0xBEEF);
    }

    #[test]
    fn pushr_completes_immediately_even_when_dspi_is_halted() {
        let mut dspi = flash_dspi();
        dspi.write32(BASE + REG_MCR, MCR_RUN | MCR_HALT).unwrap();
        dspi.write32(BASE + REG_PUSHR, 0x0002_009F).unwrap();
        let sr = dspi.read32(BASE + REG_SR).unwrap();
        assert_eq!(sr >> 12 & 0xF, 0);
        assert_ne!(sr & SR_TCF, 0);
        assert_ne!(sr & SR_RFDF, 0);
        assert_eq!(sr >> 4 & 0xF, 1);
        assert_eq!(pop(&mut dspi), 0);
        let sr = dspi.read32(BASE + REG_SR).unwrap();
        assert_eq!(sr >> 4 & 0xF, 0);
        assert_eq!(sr & 0xF, 1);
    }

    #[test]
    fn eoq_stops_the_queue_until_the_flag_is_cleared() {
        let mut dspi = Dspi::new(BASE);
        dspi.write32(BASE + REG_MCR, MCR_RUN).unwrap();
        dspi.write32(BASE + REG_PUSHR, 0x0802_0000).unwrap();
        dspi.write32(BASE + REG_PUSHR, 0x0002_0000).unwrap();
        let sr = dspi.read32(BASE + REG_SR).unwrap();
        assert_ne!(sr & SR_EOQF, 0);
        assert_eq!(sr & SR_TXRXS, 0);
        assert_eq!(sr >> 12 & 0xF, 1);
        assert_eq!(sr >> 8 & 0xF, 1);
        assert_eq!(sr >> 4 & 0xF, 1);
        assert_eq!(sr & 0xF, 0);
        assert_eq!(pop(&mut dspi), u32::from(u16::MAX));
        let sr = dspi.read32(BASE + REG_SR).unwrap();
        assert_eq!(sr >> 4 & 0xF, 0);
        assert_eq!(sr & 0xF, 1);

        dspi.write32(BASE + REG_SR, SR_EOQF).unwrap();
        let sr = dspi.read32(BASE + REG_SR).unwrap();
        assert_eq!(sr & SR_EOQF, 0);
        assert_eq!(sr >> 12 & 0xF, 0);
    }

    #[test]
    fn ctcnt_clears_the_transfer_counter() {
        let mut dspi = flash_dspi();
        dspi.write32(BASE + REG_MCR, MCR_RUN).unwrap();
        dspi.write32(BASE + REG_PUSHR, 0x0002_0000).unwrap();
        dspi.write32(BASE + REG_PUSHR, 0x0402_0000).unwrap();
        assert_eq!(dspi.read32(BASE + REG_TCR).unwrap(), 1 << 16);
    }

    #[test]
    fn full_rx_fifo_sets_overflow_and_drops_data() {
        let mut dspi = flash_dspi();
        dspi.write32(BASE + REG_MCR, MCR_RUN).unwrap();
        for _ in 0..17 {
            dspi.write32(BASE + REG_PUSHR, 0x0002_0000).unwrap();
        }
        let sr = dspi.read32(BASE + REG_SR).unwrap();
        assert_ne!(sr & SR_RFOF, 0);
        assert_eq!(sr >> 4 & 0xF, 0);
    }

    #[test]
    fn sub_word_status_write_only_clears_the_written_lane() {
        let mut dspi = flash_dspi();
        dspi.write32(BASE + REG_MCR, MCR_RUN).unwrap();
        dspi.write32(BASE + REG_PUSHR, 0x0002_0000).unwrap();
        assert_ne!(dspi.read32(BASE + REG_SR).unwrap() & SR_TCF, 0);

        dspi.write8(BASE + REG_SR + 1, 0x02).unwrap();
        let sr = dspi.read32(BASE + REG_SR).unwrap();
        assert_eq!(sr & SR_RFDF, 0);
        assert_ne!(sr & SR_TCF, 0);
    }

    #[test]
    fn pushr_rejects_sub_word_writes() {
        let mut dspi = flash_dspi();
        assert!(dspi.write16(BASE + REG_PUSHR, 0x9F).is_err());
    }

    #[test]
    fn unmodelled_registers_report_a_bus_error() {
        let mut dspi = flash_dspi();
        assert!(dspi.read32(BASE + 0x3C).is_err());
        assert!(dspi.read32(BASE + 0x04).is_err());
    }

    #[test]
    fn unsupported_flash_command_is_reported() {
        let mut dspi = flash_dspi();
        dspi.write32(BASE + REG_MCR, MCR_RUN).unwrap();
        dspi.write32(BASE + REG_PUSHR, 0x0002_00C7).unwrap();
        assert_eq!(
            dspi.unsupported().as_deref(),
            Some("SPI device command 0xc7")
        );
    }

    #[test]
    fn lsb_first_frames_are_reported_unsupported() {
        let mut dspi = flash_dspi();
        dspi.write32(BASE + REG_CTAR0, 0x3900_0000).unwrap();
        dspi.write32(BASE + REG_MCR, MCR_RUN).unwrap();
        dspi.write32(BASE + REG_PUSHR, 0x0002_009F).unwrap();
        assert!(dspi.unsupported().is_some());
    }
}
