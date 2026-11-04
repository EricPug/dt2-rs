//! Synthetic eSDHC host with a healthy, blank eMMC behind it (diagnostic model, not hardware evidence).
//!
//! Commands complete instantly; the EXT_CSD contents are chosen so the OS 1.17 storage checks pass.

use crate::bus::{Bus, BusError};
use crate::snapshot::{Reader, Writer};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const ESDHC_BASE: u32 = 0xFC0C_C000;
pub const ESDHC_DATPORT: u32 = ESDHC_BASE + 0x20;
const ESDHC_SIZE: u32 = 0x100;

const CMDARG: usize = 0x08 / 4;
const XFERTYP: usize = 0x0C / 4;
const RSP0: usize = 0x10 / 4;
const SYSCTL: usize = 0x2C / 4;
const IRQSTAT: usize = 0x30 / 4;
const IRQSIGEN: usize = 0x38 / 4;
const BLKATTR: usize = 0x04 / 4;

const IRQ_CC: u32 = 1;
const IRQ_TC: u32 = 2;
const BLOCK: usize = 512;
const CARD_SECTORS: u64 = 0x0080_0000;
const CARD_BYTES: u64 = CARD_SECTORS * BLOCK as u64;
const OCR: u32 = 0xC0FF_8080;
const CARD_STATUS_TRAN: u32 = (4 << 9) | (1 << 8);

pub struct Esdhc {
    regs: [u32; 64],
    data_out: VecDeque<u8>,
    write_buf: Vec<u8>,
    precommand_write_data: Vec<u8>,
    bus_test: Vec<u8>,
    ext_csd: [u8; BLOCK],
    storage: BTreeMap<u32, [u8; BLOCK]>,
    write_block: Option<u32>,
    write_blocks_remaining: u32,
    pending_block_count: Option<u32>,
    bus_test_write_active: bool,
    backing_file: Option<File>,
    commands: u64,
    completed_write_blocks: u64,
    last_written_block: Option<u32>,
}

impl Default for Esdhc {
    fn default() -> Self {
        Self::new()
    }
}

impl Esdhc {
    pub fn new() -> Self {
        let mut ext_csd = [0u8; BLOCK];
        ext_csd[192] = 5; // EXT_CSD_REV
        ext_csd[196] = 3; // DEVICE_TYPE
        let sec_count_be = (CARD_SECTORS as u32).to_be_bytes();
        ext_csd[212..216].copy_from_slice(&sec_count_be);
        ext_csd[152] = 1; // the only field the "MMC NOT IN SLC MODE" check reads
        ext_csd[155] = 1; // PARTITION_SETTING_COMPLETED
        ext_csd[156] = 1; // PARTITIONS_ATTRIBUTE: enhanced user area
        ext_csd[160] = 3; // PARTITIONING_SUPPORT
        Self {
            regs: [0; 64],
            data_out: VecDeque::new(),
            write_buf: Vec::new(),
            precommand_write_data: Vec::new(),
            bus_test: Vec::new(),
            ext_csd,
            storage: BTreeMap::new(),
            write_block: None,
            write_blocks_remaining: 0,
            pending_block_count: None,
            bus_test_write_active: false,
            backing_file: None,
            commands: 0,
            completed_write_blocks: 0,
            last_written_block: None,
        }
    }

    pub fn with_backing_file(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let length = file.metadata()?.len();
        if length > CARD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("eMMC image is larger than {CARD_BYTES} bytes"),
            ));
        }
        if length < CARD_BYTES {
            file.set_len(CARD_BYTES)?;
        }
        let mut host = Self::new();
        host.backing_file = Some(file);
        Ok(host)
    }

    /// Blocks written so far, in block order (data that is not in the backing file).
    pub fn written_blocks(&self) -> impl Iterator<Item = (u32, &[u8; BLOCK])> {
        self.storage.iter().map(|(block, data)| (*block, data))
    }

    pub fn contains(address: u32) -> bool {
        (ESDHC_BASE..ESDHC_BASE + ESDHC_SIZE).contains(&address)
    }

    pub fn irq_pending(&self) -> bool {
        self.regs[IRQSTAT] & self.regs[IRQSIGEN] != 0
    }

    pub fn data_ready(&self) -> bool {
        !self.data_out.is_empty()
    }

    pub fn commands_executed(&self) -> u64 {
        self.commands
    }

    pub fn trace_registers(&self) -> (u32, u32, u32) {
        (self.regs[XFERTYP], self.regs[CMDARG], self.regs[BLKATTR])
    }

    pub fn write_trace_state(&self) -> (u64, Option<u32>, bool, usize) {
        (
            self.completed_write_blocks,
            self.last_written_block,
            self.write_block.is_some(),
            self.write_buf.len(),
        )
    }

    fn respond_r1(&mut self) {
        self.regs[RSP0] = CARD_STATUS_TRAN;
    }

    fn respond_r2(&mut self, seed: u32) {
        for index in 0..4 {
            self.regs[RSP0 + index] = seed.wrapping_add(index as u32 * 0x0101_0101);
        }
    }

    fn queue_block(&mut self, block: u32) -> Result<(), BusError> {
        let data = if let Some(data) = self.storage.get(&block).copied() {
            data
        } else if let Some(file) = &mut self.backing_file {
            let mut data = [0; BLOCK];
            file
                .seek(SeekFrom::Start(u64::from(block) * BLOCK as u64))
                .and_then(|_| file.read_exact(&mut data))
                .map_err(|_| BusError {
                    addr: ESDHC_DATPORT,
                    write: false,
                })?;
            data
        } else {
            [0; BLOCK]
        };
        self.data_out.extend(data);
        Ok(())
    }

    fn take_block_count(&mut self, fallback: u32) -> u32 {
        self.pending_block_count
            .take()
            .filter(|count| *count > 0)
            .unwrap_or_else(|| fallback.max(1))
    }

    fn execute(&mut self, xfertyp: u32) -> Result<(), BusError> {
        let command = (xfertyp >> 24) & 0x3F;
        let argument = self.regs[CMDARG];
        let blocks = (self.regs[BLKATTR] >> 16).max(1);
        self.commands += 1;
        if self.write_block.is_some() {
            self.write_block = None;
            self.write_blocks_remaining = 0;
            self.write_buf.clear();
        }
        let mut transfer = false;
        match command {
            0 => {}
            1 => self.regs[RSP0] = OCR,
            2 => self.respond_r2(0x4453_3254),
            9 => self.respond_r2(0x00D0_0F00),
            10 => self.respond_r2(0x4453_3254),
            6 => {
                self.respond_r1();
                if (argument >> 24) & 3 == 3 {
                    self.ext_csd[((argument >> 16) & 0xFF) as usize] = (argument >> 8) as u8;
                }
                transfer = true;
            }
            12 => {
                self.respond_r1();
                self.write_block = None;
                self.write_blocks_remaining = 0;
                self.pending_block_count = None;
                self.bus_test_write_active = false;
                self.write_buf.clear();
                self.precommand_write_data.clear();
                transfer = true;
            }
            23 => {
                self.respond_r1();
                self.pending_block_count = Some(argument & 0xFFFF);
            }
            8 => {
                self.respond_r1();
                self.data_out.extend(self.ext_csd);
                transfer = true;
            }
            14 => {
                self.respond_r1();
                self.bus_test_write_active = false;
                self.data_out.extend(self.bus_test.iter().map(|byte| !byte));
                transfer = true;
            }
            17 | 18 => {
                self.respond_r1();
                let count = self.take_block_count(if command == 17 { 1 } else { blocks });
                for offset in 0..count {
                    self.queue_block(argument.wrapping_add(offset))?;
                }
                transfer = true;
            }
            19 => {
                self.respond_r1();
                self.write_buf.clear();
                self.precommand_write_data.clear();
                self.write_block = None;
                self.write_blocks_remaining = 0;
                self.pending_block_count = None;
                self.bus_test.clear();
                self.bus_test_write_active = true;
            }
            24 | 25 => {
                self.respond_r1();
                self.write_buf.clear();
                self.bus_test_write_active = false;
                self.write_blocks_remaining = if command == 24 {
                    self.pending_block_count.take();
                    1
                } else {
                    self.take_block_count(blocks)
                };
                self.write_block = (self.write_blocks_remaining > 0).then_some(argument);
                let prefetched = std::mem::take(&mut self.precommand_write_data);
                self.data_write(&prefetched)?;
                transfer = true;
            }
            7 | 38 => {
                self.respond_r1();
                transfer = true;
            }
            _ => self.respond_r1(),
        }
        self.regs[IRQSTAT] |= IRQ_CC | if transfer { IRQ_TC } else { 0 };
        Ok(())
    }

    fn data_write(&mut self, bytes: &[u8]) -> Result<(), BusError> {
        if self.bus_test_write_active {
            self.bus_test.extend_from_slice(bytes);
            self.regs[IRQSTAT] |= IRQ_TC;
            return Ok(());
        }
        if self.write_block.is_none() {
            self.precommand_write_data.extend_from_slice(bytes);
            return Ok(());
        }
        self.write_buf.extend_from_slice(bytes);
        while self.write_buf.len() >= BLOCK && self.write_blocks_remaining > 0 {
            let block = self.write_block.unwrap();
            let mut data = [0u8; BLOCK];
            data.copy_from_slice(&self.write_buf[..BLOCK]);
            if let Some(file) = &mut self.backing_file {
                file.seek(SeekFrom::Start(u64::from(block) * BLOCK as u64))
                    .and_then(|_| file.write_all(&data))
                    .and_then(|_| file.flush())
                    .map_err(|_| BusError {
                        addr: ESDHC_DATPORT,
                        write: true,
                    })?;
            }
            self.storage.insert(block, data);
            self.completed_write_blocks = self.completed_write_blocks.saturating_add(1);
            self.last_written_block = Some(block);
            self.write_buf.drain(..BLOCK);
            self.write_blocks_remaining -= 1;
            self.write_block = (self.write_blocks_remaining > 0)
                .then_some(block.wrapping_add(1));
            self.regs[IRQSTAT] |= IRQ_TC;
        }
        if self.write_blocks_remaining == 0 {
            self.write_buf.clear();
            self.write_block = None;
        }
        Ok(())
    }

    fn read_register(&mut self, offset: u32) -> u32 {
        match offset {
            0x20 => {
                let mut word = 0u32;
                for _ in 0..4 {
                    word = word << 8 | u32::from(self.data_out.pop_front().unwrap_or(0));
                }
                word
            }
            0x24 => {
                (1 << 3)
                    | (1 << 10)
                    | if self.data_out.is_empty() { 0 } else { 1 << 11 }
                    | (1 << 16)
                    | (1 << 23)
                    | (0xFF << 24)
            }
            _ => self.regs[(offset / 4) as usize & 63],
        }
    }

    fn write_register(&mut self, offset: u32, value: u32) -> Result<(), BusError> {
        let index = (offset / 4) as usize & 63;
        match offset {
            0x20 => self.data_write(&value.to_be_bytes())?,
            0x2C => self.regs[SYSCTL] = value & !((1 << 24) | (1 << 25) | (1 << 26) | (1 << 27)) | (1 << 3),
            0x30 => self.regs[IRQSTAT] &= !value,
            0x0C => {
                self.regs[XFERTYP] = value;
                self.execute(value)?;
            }
            0x10..=0x1C | 0x24 => {}
            _ => self.regs[index] = value,
        }
        Ok(())
    }

    pub fn save_state(&self, w: &mut Writer) {
        for value in self.regs {
            w.u32(value);
        }
        let queued: Vec<u8> = self.data_out.iter().copied().collect();
        w.bytes(&queued);
        w.bytes(&self.write_buf);
        w.bytes(&self.precommand_write_data);
        w.bytes(&self.bus_test);
        w.bytes(&self.ext_csd);
        w.u64(self.storage.len() as u64);
        for (block, data) in &self.storage {
            w.u32(*block);
            w.bytes(data);
        }
        w.u64(self.write_block.map_or(u64::MAX, u64::from));
        w.u32(self.write_blocks_remaining);
        w.u32(self.pending_block_count.unwrap_or(u32::MAX));
        w.bool(self.bus_test_write_active);
        w.u64(self.commands);
        w.u64(self.completed_write_blocks);
        w.u32(self.last_written_block.unwrap_or(u32::MAX));
    }

    pub fn load_state(&mut self, r: &mut Reader) -> Result<(), String> {
        for value in &mut self.regs {
            *value = r.u32()?;
        }
        self.data_out = r.bytes()?.iter().copied().collect();
        self.write_buf = r.bytes()?.to_vec();
        self.precommand_write_data = r.bytes()?.to_vec();
        self.bus_test = r.bytes()?.to_vec();
        self.ext_csd.copy_from_slice(r.bytes()?);
        self.storage.clear();
        for _ in 0..r.u64()? {
            let block = r.u32()?;
            let mut data = [0u8; BLOCK];
            data.copy_from_slice(r.bytes()?);
            self.storage.insert(block, data);
        }
        self.write_block = match r.u64()? {
            u64::MAX => None,
            block => Some(block as u32),
        };
        self.write_blocks_remaining = r.u32()?;
        self.pending_block_count = match r.u32()? {
            u32::MAX => None,
            count => Some(count),
        };
        self.bus_test_write_active = r.bool()?;
        self.commands = r.u64()?;
        self.completed_write_blocks = r.u64()?;
        self.last_written_block = match r.u32()? {
            u32::MAX => None,
            block => Some(block),
        };
        Ok(())
    }
}

impl Bus for Esdhc {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        let offset = address - ESDHC_BASE;
        if offset & !3 == 0x20 {
            return Ok(self.data_out.pop_front().unwrap_or(0));
        }
        let word = self.read_register(offset & !3);
        Ok(word.to_be_bytes()[(offset & 3) as usize])
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        let offset = address - ESDHC_BASE;
        let word = self.read_register(offset & !3);
        Ok(if offset & 2 == 0 { (word >> 16) as u16 } else { word as u16 })
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        Ok(self.read_register(address - ESDHC_BASE))
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        let offset = address - ESDHC_BASE;
        if offset & !3 == 0x20 {
            // eDMA writes the FIFO one byte at a time; each byte is one byte of data.
            return self.data_write(&[value]);
        }
        let shift = 8 * (3 - (offset & 3));
        let current = self.regs[(offset / 4) as usize & 63];
        self.write_register(offset & !3, (current & !(0xFF << shift)) | u32::from(value) << shift)?;
        Ok(())
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        let offset = address - ESDHC_BASE;
        if offset & !3 == 0x20 {
            return self.data_write(&value.to_be_bytes());
        }
        let shift = if offset & 2 == 0 { 16 } else { 0 };
        let current = self.regs[(offset / 4) as usize & 63];
        self.write_register(offset & !3, (current & !(0xFFFF << shift)) | u32::from(value) << shift)?;
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        self.write_register(address - ESDHC_BASE, value)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd8_returns_ext_csd_and_raises_completion() {
        let mut host = Esdhc::new();
        host.write32(ESDHC_BASE + 0x38, IRQ_CC | IRQ_TC).unwrap();
        host.write32(ESDHC_BASE + 0x0C, (8 << 24) | (1 << 21) | (1 << 4)).unwrap();
        assert!(host.irq_pending());
        assert_eq!(host.read32(ESDHC_BASE + 0x30).unwrap() & 3, 3);
        host.write32(ESDHC_BASE + 0x30, 3).unwrap();
        assert!(!host.irq_pending());
        assert!(host.data_ready());
        let mut block = Vec::new();
        for _ in 0..BLOCK / 4 {
            block.extend(host.read32(ESDHC_DATPORT).unwrap().to_be_bytes());
        }
        assert_eq!(block[152], 1);
        assert_eq!(block[192], 5);
        assert_eq!(
            u32::from_be_bytes(block[212..216].try_into().unwrap()),
            CARD_SECTORS as u32
        );
        assert!(!host.data_ready());
    }

    #[test]
    fn bus_test_returns_complement_and_blocks_round_trip() {
        let mut host = Esdhc::new();
        host.write32(ESDHC_BASE + 0x0C, 19 << 24).unwrap();
        host.write32(ESDHC_DATPORT, 0x5A).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 14 << 24).unwrap();
        assert_eq!(host.read32(ESDHC_DATPORT).unwrap() & 0xFF, 0xA5);
        host.write32(ESDHC_BASE + 0x08, 7).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 24 << 24).unwrap();
        for _ in 0..BLOCK / 4 {
            host.write32(ESDHC_DATPORT, 0x0102_0304).unwrap();
        }
        host.write32(ESDHC_BASE + 0x08, 7).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 17 << 24).unwrap();
        assert_eq!(host.read32(ESDHC_DATPORT).unwrap(), 0x0102_0304);
    }

    #[test]
    fn observed_cmd23_cmd25_one_block_ignores_dma_overrun_and_closes_write() {
        let mut host = Esdhc::new();
        host.write32(ESDHC_BASE + 0x04, (1 << 16) | BLOCK as u32)
            .unwrap();
        host.write32(ESDHC_BASE + 0x08, 0).unwrap();
        host.write32(ESDHC_BASE + 0x0C, (23 << 24) | (1 << 31) | 1)
            .unwrap();
        for value in [0x1111_1111, 0x2222_2222, 0x3333_3333, 0x4444_4444] {
            for _ in 0..BLOCK / 4 {
                host.write32(ESDHC_DATPORT, value).unwrap();
            }
        }
        host.write32(ESDHC_BASE + 0x08, 0).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 25 << 24).unwrap();

        assert_eq!(host.write_trace_state(), (1, Some(0), false, 0));
        host.write32(ESDHC_BASE + 0x08, 0).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 17 << 24).unwrap();
        assert_eq!(host.read32(ESDHC_DATPORT).unwrap(), 0x1111_1111);
        for _ in 1..BLOCK / 4 {
            host.read32(ESDHC_DATPORT).unwrap();
        }
        host.write32(ESDHC_BASE + 0x08, 1).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 17 << 24).unwrap();
        assert_eq!(host.read32(ESDHC_DATPORT).unwrap(), 0);
    }

    #[test]
    fn byte_and_halfword_fifo_writes_store_the_bytes_written() {
        let mut host = Esdhc::new();
        host.write32(ESDHC_BASE + 0x04, (1 << 16) | BLOCK as u32).unwrap();
        host.write32(ESDHC_BASE + 0x08, 1).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 23 << 24).unwrap();
        for index in 0..BLOCK / 2 {
            if index % 2 == 0 {
                host.write8(ESDHC_DATPORT + (index as u32 * 2 & 3), index as u8).unwrap();
                host.write8(ESDHC_DATPORT + (index as u32 * 2 & 3) + 1, 0x5A).unwrap();
            } else {
                host.write16(ESDHC_DATPORT + 2, (index as u16) << 8 | 0x3C).unwrap();
            }
        }
        host.write32(ESDHC_BASE + 0x08, 0x900).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 25 << 24).unwrap();
        let block = host.storage.get(&0x900).expect("block written");
        assert_eq!(&block[..6], &[0, 0x5A, 1, 0x3C, 2, 0x5A]);
    }

    #[test]
    fn cmd23_four_block_cmd25_completes_at_exact_count() {
        let mut host = Esdhc::new();
        host.write32(ESDHC_BASE + 0x04, (1 << 16) | BLOCK as u32)
            .unwrap();
        host.write32(ESDHC_BASE + 0x08, 4).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 23 << 24).unwrap();
        for value in [0xAAAA_AAAA, 0xBBBB_BBBB, 0xCCCC_CCCC, 0xDDDD_DDDD] {
            for _ in 0..BLOCK / 4 {
                host.write32(ESDHC_DATPORT, value).unwrap();
            }
        }
        host.write32(ESDHC_BASE + 0x08, 0x800).unwrap();
        host.write32(ESDHC_BASE + 0x0C, 25 << 24).unwrap();
        assert_eq!(host.write_trace_state(), (4, Some(0x803), false, 0));
    }

    #[test]
    fn backing_file_preserves_cmd25_block_across_reopen_and_ignores_dma_overrun() {
        let path = std::env::temp_dir().join(format!("dt2-emmc-{}.img", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut host = Esdhc::with_backing_file(&path).unwrap();
            host.write32(ESDHC_BASE + 0x08, 1).unwrap();
            host.write32(ESDHC_BASE + 0x0C, (23 << 24) | (1 << 31) | 1)
                .unwrap();
            for value in [0xA1A1_A1A1, 0xB2B2_B2B2, 0xC3C3_C3C3, 0xD4D4_D4D4] {
                for _ in 0..BLOCK / 4 {
                    host.write32(ESDHC_DATPORT, value).unwrap();
                }
            }
            host.write32(ESDHC_BASE + 0x08, 7).unwrap();
            host.write32(ESDHC_BASE + 0x0C, 25 << 24).unwrap();
            assert_eq!(host.write_trace_state(), (1, Some(7), false, 0));
        }

        {
            let mut host = Esdhc::with_backing_file(&path).unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().len(), CARD_BYTES);
            host.write32(ESDHC_BASE + 0x08, 7).unwrap();
            host.write32(ESDHC_BASE + 0x0C, 17 << 24).unwrap();
            assert_eq!(host.read32(ESDHC_DATPORT).unwrap(), 0xA1A1_A1A1);
            for _ in 1..BLOCK / 4 {
                host.read32(ESDHC_DATPORT).unwrap();
            }
            host.write32(ESDHC_BASE + 0x08, 8).unwrap();
            host.write32(ESDHC_BASE + 0x0C, 17 << 24).unwrap();
            assert_eq!(host.read32(ESDHC_DATPORT).unwrap(), 0);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn trace_state_reports_command_registers_and_completed_storage_writes() {
        let mut host = Esdhc::new();
        host.write32(ESDHC_BASE + 0x08, 7).unwrap();
        host.write32(ESDHC_BASE + 0x04, (1 << 16) | BLOCK as u32)
            .unwrap();
        host.write32(ESDHC_BASE + 0x0C, 24 << 24).unwrap();
        assert_eq!(host.trace_registers(), (24 << 24, 7, (1 << 16) | 512));

        for _ in 0..BLOCK / 4 {
            host.write32(ESDHC_DATPORT, 0x0102_0304).unwrap();
        }

        assert_eq!(host.write_trace_state(), (1, Some(7), false, 0));
    }

    #[test]
    fn state_round_trips() {
        let mut host = Esdhc::new();
        host.write32(ESDHC_BASE + 0x0C, 8 << 24).unwrap();
        let mut w = Writer::new();
        host.save_state(&mut w);
        let data = w.into_inner();
        let mut again = Esdhc::new();
        again.load_state(&mut Reader::new(&data)).unwrap();
        let mut w2 = Writer::new();
        again.save_state(&mut w2);
        assert_eq!(w2.into_inner(), data);
    }
}
