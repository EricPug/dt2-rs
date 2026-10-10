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
