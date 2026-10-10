/// A memory access that no device answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusError {
    pub addr: u32,
    pub write: bool,
}

/// Big-endian system bus seen by a CPU core.
pub trait Bus {
    fn read8(&mut self, addr: u32) -> Result<u8, BusError>;
    fn read16(&mut self, addr: u32) -> Result<u16, BusError>;
    fn read32(&mut self, addr: u32) -> Result<u32, BusError>;
    fn write8(&mut self, addr: u32, v: u8) -> Result<(), BusError>;
    fn write16(&mut self, addr: u32, v: u16) -> Result<(), BusError>;
    fn write32(&mut self, addr: u32, v: u32) -> Result<(), BusError>;
}

/// One flat RAM block, for tests and tools.
pub struct FlatRam {
    base: u32,
    data: Vec<u8>,
}

impl FlatRam {
    pub fn new(base: u32, size: usize) -> Self {
        Self {
            base,
            data: vec![0; size],
        }
    }

    pub fn load(&mut self, addr: u32, bytes: &[u8]) {
        let o = (addr - self.base) as usize;
        self.data[o..o + bytes.len()].copy_from_slice(bytes);
    }

    fn range(&self, addr: u32, n: usize, write: bool) -> Result<usize, BusError> {
        let o = addr.wrapping_sub(self.base) as usize;
        if addr >= self.base && o + n <= self.data.len() {
            Ok(o)
        } else {
            Err(BusError { addr, write })
        }
    }
}

impl Bus for FlatRam {
    fn read8(&mut self, a: u32) -> Result<u8, BusError> {
        Ok(self.data[self.range(a, 1, false)?])
    }
    fn read16(&mut self, a: u32) -> Result<u16, BusError> {
        let o = self.range(a, 2, false)?;
        Ok(u16::from_be_bytes([self.data[o], self.data[o + 1]]))
    }
    fn read32(&mut self, a: u32) -> Result<u32, BusError> {
        let o = self.range(a, 4, false)?;
        Ok(u32::from_be_bytes(self.data[o..o + 4].try_into().unwrap()))
    }
    fn write8(&mut self, a: u32, v: u8) -> Result<(), BusError> {
        let o = self.range(a, 1, true)?;
        self.data[o] = v;
        Ok(())
    }
    fn write16(&mut self, a: u32, v: u16) -> Result<(), BusError> {
        let o = self.range(a, 2, true)?;
        self.data[o..o + 2].copy_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn write32(&mut self, a: u32, v: u32) -> Result<(), BusError> {
        let o = self.range(a, 4, true)?;
        self.data[o..o + 4].copy_from_slice(&v.to_be_bytes());
        Ok(())
    }
}

/// MCF5441x 64 KiB on-chip SRAM mirrored across its 256 MiB address window.
pub struct Mcf5441xSram {
    data: Vec<u8>,
}

impl Mcf5441xSram {
    const BASE: u32 = 0x8000_0000;
    const WINDOW_SIZE: u32 = 0x1000_0000;
    const SIZE: usize = 0x1_0000;

    pub fn new() -> Self {
        Self {
            data: vec![0; Self::SIZE],
        }
    }

    fn offset(&self, address: u32, write: bool) -> Result<usize, BusError> {
        if (Self::BASE..Self::BASE + Self::WINDOW_SIZE).contains(&address) {
            Ok((address.wrapping_sub(Self::BASE) as usize) & (Self::SIZE - 1))
        } else {
            Err(BusError {
                addr: address,
                write,
            })
        }
    }
}

impl Default for Mcf5441xSram {
    fn default() -> Self {
        Self::new()
    }
}

impl Bus for Mcf5441xSram {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        Ok(self.data[self.offset(address, false)?])
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        Ok(u16::from_be_bytes([
            self.read8(address)?,
            self.read8(address.wrapping_add(1))?,
        ]))
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        Ok(u32::from_be_bytes([
            self.read8(address)?,
            self.read8(address.wrapping_add(1))?,
            self.read8(address.wrapping_add(2))?,
            self.read8(address.wrapping_add(3))?,
        ]))
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        let offset = self.offset(address, true)?;
        self.data[offset] = value;
        Ok(())
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write8(address.wrapping_add(offset as u32), byte)?;
        }
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write8(address.wrapping_add(offset as u32), byte)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Bus, Mcf5441xSram};

    #[test]
    fn mcf5441x_sram_aliases_every_64_kib() {
        let mut sram = Mcf5441xSram::new();
        sram.write32(0x8000_0004, 0x1234_5678).unwrap();
        assert_eq!(sram.read32(0x8001_0004).unwrap(), 0x1234_5678);

        sram.write16(0x8000_FFFF, 0xA55A).unwrap();
        assert_eq!(sram.read8(0x8000_FFFF).unwrap(), 0xA5);
        assert_eq!(sram.read8(0x8001_0000).unwrap(), 0x5A);
    }

    #[test]
    fn mcf5441x_sram_rejects_addresses_outside_the_alias_window() {
        let mut sram = Mcf5441xSram::new();
        assert!(sram.read8(0x7FFF_FFFF).is_err());
        assert!(sram.read8(0x9000_0000).is_err());
    }
}
