//! MCF5441x EMAC register file and its register-transfer semantics (MCF54418RM chapter 5).
//! MAC and MSAC follow the pseudocode in section 5.3.5; the dual-accumulate forms are not modelled.

const OMC: u32 = 0x80;
const SU: u32 = 0x40;
const FI: u32 = 0x20;
const RT: u32 = 0x10;
const FLAG_N: u32 = 0x8;
const FLAG_Z: u32 = 0x4;
const FLAG_V: u32 = 0x2;
const FLAG_EV: u32 = 0x1;
const PAV0: u32 = 0x100;
const MACSR_WRITABLE: u32 = 0xFFF;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emac {
    pub macsr: u32,
    mask: u32,
    acc: [u32; 4],
    /// Extension halfword of each accumulator; its use depends on the fractional/integer mode.
    ext: [u16; 4],
}

impl Default for Emac {
    fn default() -> Self {
        Self {
            macsr: 0,
            mask: 0xFFFF_FFFF,
            acc: [0; 4],
            ext: [0; 4],
        }
    }
}

impl Emac {
    /// Raw register file for an engine that executes MAC instructions itself:
    /// (ACC0..3, ACCext01, ACCext23, MACSR, MASK).
    pub fn raw_registers(&self) -> ([u32; 4], u32, u32, u32, u32) {
        (self.acc, self.ext_pair(false), self.ext_pair(true), self.macsr, self.read_mask())
    }

    /// Replaces the register file from an engine's raw state (see [`Emac::raw48`]).
    pub fn load_raw(&mut self, macc: [u64; 4], macsr: u32, mask: u32) {
        self.macsr = macsr;
        self.mask = mask & 0xFFFF;
        for (n, bits) in macc.into_iter().enumerate() {
            self.store_raw48(n, bits & 0xFFFF_FFFF_FFFF);
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    fn fractional(&self) -> bool {
        self.macsr & FI != 0
    }

    fn unsigned_integer(&self) -> bool {
        self.macsr & (SU | FI) == SU
    }

    /// The 48-bit accumulator as the manual defines it for the current mode.
    pub fn raw48(&self, n: usize) -> u64 {
        let acc = u64::from(self.acc[n]);
        let ext = u64::from(self.ext[n]);
        if self.fractional() {
            ((ext >> 8) << 40) | (acc << 8) | (ext & 0xFF)
        } else {
            (ext << 32) | acc
        }
    }

    fn signed48(&self, n: usize) -> i64 {
        ((self.raw48(n) << 16) as i64) >> 16
    }

    /// MOVE.L <ea>,ACCx: loads the accumulator, clears its PAV flag and sets N/Z.
    pub fn load(&mut self, n: usize, value: u32) {
        let negative = value & 0x8000_0000 != 0 && !self.unsigned_integer();
        self.acc[n] = value;
        self.ext[n] = match (self.fractional(), negative) {
            (true, true) => 0xFF00,
            (false, true) => 0xFFFF,
            _ => 0,
        };
        self.macsr &= !(PAV0 << n);
        self.macsr &= !(FLAG_N | FLAG_Z | FLAG_V | FLAG_EV);
        let raw = self.raw48(n);
        if raw == 0 {
            self.macsr |= FLAG_Z;
        } else if raw >> 47 & 1 != 0 {
            self.macsr |= FLAG_N;
        }
    }

    /// MOVE.L ACCy,ACCx copies the 48-bit value and its overflow flag.
    pub fn copy(&mut self, from: usize, to: usize) {
        self.acc[to] = self.acc[from];
        self.ext[to] = self.ext[from];
        let pav = self.macsr & (PAV0 << from) != 0;
        self.macsr &= !(PAV0 << to);
        if pav {
            self.macsr |= PAV0 << to;
        }
    }

    /// MOVE.L ACCx,Rx including the saturation and rounding the current mode requires.
    pub fn store(&self, n: usize) -> u32 {
        let saturate = self.macsr & OMC != 0;
        if self.fractional() {
            let acc = self.acc[n];
            let ext = self.ext[n];
            let guard_ok = (ext >> 8) as u8 == if acc & 0x8000_0000 != 0 { 0xFF } else { 0 };
            let negative = (ext >> 8) as u8 & 0x80 != 0;
            let saturated = |wide: bool| match (wide, negative) {
                (true, false) => 0x7FFF_FFFF,
                (true, true) => 0x8000_0000,
                (false, false) => 0x7FFF,
                (false, true) => 0x8000,
            };
            let low8 = u32::from(ext & 0xFF);
            if self.macsr & SU != 0 {
                // Round to a 16-bit fraction using the low 24 bits.
                let tail = ((acc & 0xFFFF) << 8) | low8;
                let upper = (acc >> 16) as i16 as i32;
                let up = tail > 0x80_0000 || (tail == 0x80_0000 && upper & 1 == 1);
                let rounded = i64::from(upper) + i64::from(up);
                if !guard_ok || rounded > i64::from(i16::MAX) {
                    return if saturate { saturated(false) } else { (rounded as u32) & 0xFFFF };
                }
                return (rounded as u32) & 0xFFFF;
            }
            if self.macsr & RT != 0 {
                let up = low8 > 0x80 || (low8 == 0x80 && acc & 1 == 1);
                let rounded = i64::from(acc as i32) + i64::from(up);
                if !guard_ok || rounded > i64::from(i32::MAX) {
                    return if saturate { saturated(true) } else { rounded as u32 };
                }
                return rounded as u32;
            }
            return if !guard_ok && saturate { saturated(true) } else { acc };
        }
        if self.unsigned_integer() {
            let raw = self.raw48(n);
            return if saturate && raw > u64::from(u32::MAX) { u32::MAX } else { raw as u32 };
        }
        let value = self.signed48(n);
        if saturate && (value > i64::from(i32::MAX) || value < i64::from(i32::MIN)) {
            return if value < 0 { 0x8000_0000 } else { 0x7FFF_FFFF };
        }
        value as u32
    }

    /// MOVCLR.L ACCx,Rx: stores like MOVE.L, then clears the accumulator and its PAV flag.
    pub fn store_and_clear(&mut self, n: usize) -> u32 {
        let value = self.store(n);
        self.acc[n] = 0;
        self.ext[n] = 0;
        self.macsr &= !(PAV0 << n);
        value
    }


    /// One MAC or MSAC. `word` is `Some((x_upper, y_upper))` for word operands and `None` for longwords;
    /// `scale` is the 2-bit SF field (integer modes only).
    pub fn mac(&mut self, n: usize, x: u32, y: u32, word: Option<(bool, bool)>, scale: u8, subtract: bool) {
        let omc = self.macsr & OMC != 0;
        let pav_bit = PAV0 << n;
        if omc && self.macsr & pav_bit != 0 {
            self.set_mac_flags(n);
            return;
        }
        self.macsr &= !pav_bit;
        let fractional = self.fractional();
        let unsigned = self.unsigned_integer();
        let pick = |value: u32, upper: bool| -> i64 {
            let half = if upper { value >> 16 } else { value & 0xFFFF };
            if fractional {
                i64::from((half << 16) as i32)
            } else if unsigned {
                i64::from(half)
            } else {
                i64::from(half as u16 as i16)
            }
        };
        let (ox, oy) = match word {
            Some((xu, yu)) => (pick(x, xu), pick(y, yu)),
            None if unsigned => (i64::from(x), i64::from(y)),
            None => (i64::from(x as i32), i64::from(y as i32)),
        };
        let acc48 = if unsigned { self.raw48(n) as i64 } else { self.signed48(n) };
        let (min48, max48) = if unsigned { (0i128, (1i128 << 48) - 1) } else { (-(1i128 << 47), (1i128 << 47) - 1) };
        let mut product_overflow = false;
        let product: i128 = if fractional {
            let mut p = (i128::from(ox) * i128::from(oy)) << 1;
            if self.macsr & RT != 0 {
                let low = (p & 0xFF_FFFF) as u32;
                if low > 0x80_0000 || (low == 0x80_0000 && (p >> 24) & 1 == 1) {
                    p += 1 << 24;
                }
            }
            p >> 24
        } else {
            let raw = i128::from(ox) * i128::from(oy);
            let (low, high) = if unsigned { (0i128, (1i128 << 40) - 1) } else { (-(1i128 << 39), (1i128 << 39) - 1) };
            product_overflow = raw < low || raw > high;
            let low40 = raw & ((1i128 << 40) - 1);
            let mut p = if !unsigned && low40 >= 1i128 << 39 { low40 - (1i128 << 40) } else { low40 };
            match scale & 3 {
                1 => p <<= 1,
                3 => p >>= 1,
                _ => {}
            }
            p
        };
        let mut result: i128;
        if product_overflow {
            self.macsr |= pav_bit | FLAG_V;
        }
        if product_overflow && omc {
            result = if unsigned {
                if subtract { 0 } else { (1i128 << 48) - 1 }
            } else {
                let negative = i128::from(ox) * i128::from(oy) < 0;
                match (subtract, negative) {
                    (true, true) => 0x7FFF_FFFF,
                    (true, false) => -0x8000_0000,
                    (false, true) => -0x8000_0000,
                    (false, false) => 0x7FFF_FFFF,
                }
            };
        } else {
            let base = i128::from(acc48);
            result = if subtract { base - product } else { base + product };
            if result < min48 || result > max48 {
                self.macsr |= pav_bit | FLAG_V;
                if omc {
                    result = if unsigned {
                        if subtract { 0 } else { (1i128 << 48) - 1 }
                    } else if fractional {
                        if result > 0 { 0x007F_FFFF_FF00 } else { -0x0080_0000_0000 }
                    } else if result > 0 {
                        0x7FFF_FFFF
                    } else {
                        -0x8000_0000
                    };
                }
            }
        }
        let bits = (result as u64) & 0xFFFF_FFFF_FFFF;
        self.store_raw48(n, bits);
        self.set_mac_flags(n);
    }

    fn store_raw48(&mut self, n: usize, bits: u64) {
        if self.fractional() {
            self.ext[n] = ((((bits >> 40) & 0xFF) << 8) | (bits & 0xFF)) as u16;
            self.acc[n] = (bits >> 8) as u32;
        } else {
            self.ext[n] = (bits >> 32) as u16;
            self.acc[n] = bits as u32;
        }
    }

    /// N, Z, EV from the accumulator, V from its sticky PAV flag (section 5.3.5).
    fn set_mac_flags(&mut self, n: usize) {
        let raw = self.raw48(n);
        self.macsr &= !(FLAG_N | FLAG_Z | FLAG_V | FLAG_EV);
        if self.macsr & (PAV0 << n) != 0 {
            self.macsr |= FLAG_V;
        }
        if raw >> 47 & 1 != 0 {
            self.macsr |= FLAG_N;
        }
        if raw == 0 {
            self.macsr |= FLAG_Z;
        }
        let fits = if self.fractional() {
            matches!(raw >> 39, 0 | 0x1FF)
        } else if self.unsigned_integer() {
            raw >> 32 == 0
        } else {
            matches!(raw >> 31, 0 | 0x1_FFFF)
        };
        if !fits {
            self.macsr |= FLAG_EV;
        }
    }

    pub fn ext_pair(&self, upper: bool) -> u32 {
        let base = if upper { 2 } else { 0 };
        u32::from(self.ext[base]) | (u32::from(self.ext[base + 1]) << 16)
    }

    pub fn set_ext_pair(&mut self, upper: bool, value: u32) {
        let base = if upper { 2 } else { 0 };
        self.ext[base] = value as u16;
        self.ext[base + 1] = (value >> 16) as u16;
    }

    pub fn read_mask(&self) -> u32 {
        0xFFFF_0000 | (self.mask & 0xFFFF)
    }

    pub fn write_mask(&mut self, value: u32) {
        self.mask = value & 0xFFFF;
    }

    pub fn write_macsr(&mut self, value: u32) {
        self.macsr = value & MACSR_WRITABLE;
    }

    /// MOVE.L MACSR,CCR: N, Z, V and EV land in CCR bits 3..0.
    pub fn macsr_to_ccr(&self) -> u16 {
        (self.macsr & 0xF) as u16
    }

    pub fn save_state(&self, w: &mut crate::snapshot::Writer) {
        w.u32(self.macsr);
        w.u32(self.mask);
        for value in self.acc {
            w.u32(value);
        }
        for value in self.ext {
            w.u32(u32::from(value));
        }
    }

    pub fn load_state(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), String> {
        self.macsr = r.u32()?;
        self.mask = r.u32()?;
        for value in &mut self.acc {
            *value = r.u32()?;
        }
        for value in &mut self.ext {
            *value = r.u32()? as u16;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_save_restore_round_trips_every_accumulator_bit() {
        let mut emac = Emac::default();
        emac.set_ext_pair(false, 0x1234_ABCD);
        emac.load(0, 0x8000_0001);
        emac.set_ext_pair(false, 0x1234_ABCD);
        assert_eq!(emac.store(0), 0x8000_0001);
        assert_eq!(emac.ext_pair(false), 0x1234_ABCD);
        // Loading sign-extends the 32-bit value into the extension in signed integer mode.
        emac.load(1, 0xFFFF_FFFE);
        assert_eq!(emac.signed48(1), -2);
    }

    #[test]
    fn movclr_returns_the_value_and_clears_accumulator_and_pav() {
        let mut emac = Emac::default();
        emac.load(2, 7);
        emac.macsr |= PAV0 << 2;
        assert_eq!(emac.store_and_clear(2), 7);
        assert_eq!(emac.store(2), 0);
        assert_eq!(emac.macsr & (PAV0 << 2), 0);
    }

    #[test]
    fn load_sets_n_and_z_and_macsr_write_keeps_only_defined_bits() {
        let mut emac = Emac::default();
        emac.load(0, 0);
        assert_eq!(emac.macsr & 0xF, FLAG_Z);
        emac.load(0, 0x8000_0000);
        assert_eq!(emac.macsr & 0xF, FLAG_N);
        emac.write_macsr(0xFFFF_FFFF);
        assert_eq!(emac.macsr, 0xFFF);
        assert_eq!(emac.macsr_to_ccr(), 0xF);
    }

    #[test]
    fn mask_reads_back_with_upper_halfword_ones() {
        let mut emac = Emac::default();
        assert_eq!(emac.read_mask(), 0xFFFF_FFFF);
        emac.write_mask(0x1234_00FF);
        assert_eq!(emac.read_mask(), 0xFFFF_00FF);
    }

    #[test]
    fn integer_store_saturates_only_when_omc_is_set() {
        let mut emac = Emac::default();
        emac.set_ext_pair(false, 0x0001);
        emac.acc[0] = 0x1234;
        assert_eq!(emac.store(0), 0x1234, "wraps without OMC");
        emac.macsr = OMC;
        assert_eq!(emac.store(0), 0x7FFF_FFFF);
        emac.ext[0] = 0xFFFF;
        emac.acc[0] = 0x0000_0001;
        assert_eq!(emac.store(0), 0x8000_0000);
    }

    #[test]
    fn fractional_store_rounds_to_nearest_even_when_requested() {
        let mut emac = Emac::default();
        emac.macsr = FI | RT;
        emac.acc[0] = 0x0000_0001;
        emac.ext[0] = 0x0080; // exactly half: odd lsb rounds up
        assert_eq!(emac.store(0), 2);
        emac.acc[0] = 0x0000_0002;
        assert_eq!(emac.store(0), 2, "even lsb rounds down at the halfway point");
        emac.macsr = FI;
        emac.acc[0] = 0x0000_0001;
        assert_eq!(emac.store(0), 1, "truncate mode returns the raw fraction");
    }

    #[test]
    fn snapshot_state_round_trips() {
        let mut emac = Emac::default();
        emac.load(3, 0xDEAD_BEEF);
        emac.write_mask(0xF0F0);
        emac.set_ext_pair(true, 0x0001_0002);
        let mut w = crate::snapshot::Writer::new();
        emac.save_state(&mut w);
        let data = w.into_inner();
        let mut copy = Emac::default();
        copy.load_state(&mut crate::snapshot::Reader::new(&data)).unwrap();
        assert_eq!(copy, emac);
    }
}
