//! Minimal little-endian state serialization for machine snapshots.

pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    pub fn bytes(&mut self, v: &[u8]) {
        self.u64(v.len() as u64);
        self.buf.extend_from_slice(v);
    }
    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or("snapshot truncated")?;
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn bool(&mut self) -> Result<bool, String> {
        Ok(self.u8()? != 0)
    }
    pub fn bytes(&mut self) -> Result<&'a [u8], String> {
        let len = self.u64()? as usize;
        self.take(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pit::PitController;

    #[test]
    fn writer_reader_round_trip_and_truncation_is_an_error() {
        let mut w = Writer::new();
        w.u8(7);
        w.u16(0xBEEF);
        w.u32(0xDEAD_BEEF);
        w.u64(1 << 40);
        w.bool(true);
        w.bytes(&[1, 2, 3]);
        let data = w.into_inner();
        let mut r = Reader::new(&data);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u16().unwrap(), 0xBEEF);
        assert_eq!(r.u32().unwrap(), 0xDEAD_BEEF);
        assert_eq!(r.u64().unwrap(), 1 << 40);
        assert!(r.bool().unwrap());
        assert_eq!(r.bytes().unwrap(), [1, 2, 3]);
        assert!(r.u8().is_err());
    }

    #[test]
    fn pit_state_survives_save_and_load() {
        use crate::bus::Bus;
        let mut pit = PitController::new();
        pit.write16(0xFC08_0002, 0x1234).unwrap();
        pit.write16(0xFC08_0000, 0x0013).unwrap();
        for _ in 0..100 {
            pit.advance_instruction();
        }
        let mut w = Writer::new();
        pit.save_state(&mut w);
        let data = w.into_inner();
        let mut restored = PitController::new();
        restored.load_state(&mut Reader::new(&data)).unwrap();
        let mut again = Writer::new();
        restored.save_state(&mut again);
        assert_eq!(again.into_inner(), data);
        assert_eq!(restored.timer_state(0), pit.timer_state(0));
    }
}
