//! ColdFire V4e core. Written from the ColdFire Family Programmer's Reference Manual.
//!
//! Only a subset of the instruction set is decoded so far; anything else returns
//! [`CpuError::Illegal`], which at this stage means "not implemented" as much as "illegal".

use crate::bus::{Bus, BusError, LoopContext};
use crate::emac::Emac;

pub const CCR_C: u16 = 0x01;
pub const CCR_V: u16 = 0x02;
pub const CCR_Z: u16 = 0x04;
pub const CCR_N: u16 = 0x08;
pub const CCR_X: u16 = 0x10;
pub const RGPIOBAR_RESET: u32 = 0x8C00_0034;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuError {
    Bus(BusError),
    Halt { pc: u32 },
    Illegal { pc: u32, op: u16 },
    Privilege { pc: u32, op: u16 },
    UnsupportedExceptionState { pc: u32, op: u16 },
    UnsupportedDebugRegister { pc: u32, register: u8 },
}

impl From<BusError> for CpuError {
    fn from(e: BusError) -> Self {
        CpuError::Bus(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuSnapshot {
    pub d: [u32; 8],
    pub a: [u32; 8],
    pub pc: u32,
    pub sr: u16,
    pub rambar: u32,
    pub acr0: u32,
    pub acr1: u32,
    pub vbr: u32,
    pub cacr: u32,
    pub rgpiobar: u32,
    pub debug_csr: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStop {
    InstructionLimit,
    Breakpoint { pc: u32 },
    Halt { pc: u32 },
    Error(CpuError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptInjection {
    pub instruction_count: usize,
    pub level: u8,
    pub vector: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptDelivery {
    pub instruction_count: usize,
    pub source: u8,
    pub level: u8,
    pub vector: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunReport {
    pub instructions: usize,
    pub fast_forwarded_instructions: usize,
    pub injected_interrupt: Option<InterruptInjection>,
    pub natural_interrupt: Option<InterruptDelivery>,
    pub cpu: CpuSnapshot,
    pub stop: RunStop,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Size {
    B,
    W,
    L,
}

impl Size {
    fn mask(self) -> u32 {
        match self {
            Size::B => 0xFF,
            Size::W => 0xFFFF,
            Size::L => 0xFFFF_FFFF,
        }
    }
    fn msb(self) -> u32 {
        match self {
            Size::B => 0x80,
            Size::W => 0x8000,
            Size::L => 0x8000_0000,
        }
    }
    fn bytes(self) -> u32 {
        match self {
            Size::B => 1,
            Size::W => 2,
            Size::L => 4,
        }
    }
}

#[derive(Clone, Copy)]
enum Operand {
    Dn(usize),
    An(usize),
    Mem(u32),
    Imm(u32),
}

#[derive(Clone)]
pub struct Cpu {
    pub d: [u32; 8],
    pub a: [u32; 8],
    pub pc: u32,
    pub sr: u16,
    pub rambar: u32,
    pub acr0: u32,
    pub acr1: u32,
    pub vbr: u32,
    pub cacr: u32,
    pub rgpiobar: u32,
    pub debug_csr: u32,
    pub emac: Emac,
    fast_forwarded_in_step: usize,
}

impl Default for Cpu {
    fn default() -> Self {
        Self {
            d: [0; 8],
            a: [0; 8],
            pc: 0,
            sr: 0,
            rambar: 0,
            acr0: 0,
            acr1: 0,
            vbr: 0,
            cacr: 0,
            rgpiobar: RGPIOBAR_RESET,
            debug_csr: 0,
            emac: Emac::default(),
            fast_forwarded_in_step: 0,
        }
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    fn fetch16<B: Bus>(&mut self, bus: &mut B) -> Result<u16, CpuError> {
        bus.begin_instruction_fetch();
        let value = bus.read16(self.pc);
        bus.end_instruction_fetch();
        let v = value?;
        self.pc = self.pc.wrapping_add(2);
        Ok(v)
    }

    fn fetch32<B: Bus>(&mut self, bus: &mut B) -> Result<u32, CpuError> {
        bus.begin_instruction_fetch();
        let value = bus.read32(self.pc);
        bus.end_instruction_fetch();
        let v = value?;
        self.pc = self.pc.wrapping_add(4);
        Ok(v)
    }

    fn index_ea<B: Bus>(&mut self, bus: &mut B, base: u32) -> Result<u32, CpuError> {
        let at = self.pc;
        let ext = self.fetch16(bus)?;
        // Only the brief format; the full format (bit 8 set) is not decoded yet.
        if ext & 0x0100 != 0 {
            return Err(CpuError::Illegal { pc: at, op: ext });
        }
        let r = ((ext >> 12) & 7) as usize;
        let x = if ext & 0x8000 != 0 {
            self.a[r]
        } else {
            self.d[r]
        };
        let x = if ext & 0x0800 != 0 {
            x
        } else {
            x as u16 as i16 as i32 as u32
        };
        let x = x << ((ext >> 9) & 3);
        Ok(base
            .wrapping_add(x)
            .wrapping_add(ext as u8 as i8 as i32 as u32))
    }

    fn ea<B: Bus>(
        &mut self,
        bus: &mut B,
        mode: u16,
        reg: usize,
        size: Size,
        pc0: u32,
        op: u16,
    ) -> Result<Operand, CpuError> {
        // ColdFire adjusts A7 by 1 for byte operands; the 68000 rounds up to 2.
        let step = size.bytes();
        Ok(match mode {
            0 => Operand::Dn(reg),
            1 => Operand::An(reg),
            2 => Operand::Mem(self.a[reg]),
            3 => {
                let a = self.a[reg];
                self.a[reg] = a.wrapping_add(step);
                Operand::Mem(a)
            }
            4 => {
                self.a[reg] = self.a[reg].wrapping_sub(step);
                Operand::Mem(self.a[reg])
            }
            5 => {
                let d = self.fetch16(bus)? as i16 as i32 as u32;
                Operand::Mem(self.a[reg].wrapping_add(d))
            }
            6 => {
                let base = self.a[reg];
                Operand::Mem(self.index_ea(bus, base)?)
            }
            7 => match reg {
                0 => Operand::Mem(self.fetch16(bus)? as i16 as i32 as u32),
                1 => Operand::Mem(self.fetch32(bus)?),
                2 => {
                    let base = self.pc;
                    let d = self.fetch16(bus)? as i16 as i32 as u32;
                    Operand::Mem(base.wrapping_add(d))
                }
                3 => {
                    let base = self.pc;
                    Operand::Mem(self.index_ea(bus, base)?)
                }
                4 => Operand::Imm(match size {
                    Size::L => self.fetch32(bus)?,
                    Size::W => self.fetch16(bus)? as u32,
                    Size::B => self.fetch16(bus)? as u32 & 0xFF,
                }),
                _ => return Err(CpuError::Illegal { pc: pc0, op }),
            },
            _ => return Err(CpuError::Illegal { pc: pc0, op }),
        })
    }

    fn control_ea<B: Bus>(&mut self, bus: &mut B, op: u16, pc0: u32) -> Result<u32, CpuError> {
        let mode = (op >> 3) & 7;
        if !matches!(mode, 2 | 5 | 6 | 7) {
            return Err(CpuError::Illegal { pc: pc0, op });
        }
        match self.ea(bus, mode, (op & 7) as usize, Size::L, pc0, op)? {
            Operand::Mem(a) => Ok(a),
            _ => Err(CpuError::Illegal { pc: pc0, op }),
        }
    }

    fn read<B: Bus>(&self, bus: &mut B, o: Operand, size: Size) -> Result<u32, CpuError> {
        Ok(match o {
            Operand::Dn(r) => self.d[r] & size.mask(),
            Operand::An(r) => self.a[r] & size.mask(),
            Operand::Imm(v) => v,
            Operand::Mem(a) => match size {
                Size::B => bus.read8(a)? as u32,
                Size::W => bus.read16(a)? as u32,
                Size::L => bus.read32(a)?,
            },
        })
    }

    fn write<B: Bus>(
        &mut self,
        bus: &mut B,
        o: Operand,
        size: Size,
        v: u32,
        pc0: u32,
        op: u16,
    ) -> Result<(), CpuError> {
        match o {
            Operand::Dn(r) => {
                self.d[r] = (self.d[r] & !size.mask()) | (v & size.mask());
            }
            Operand::Mem(a) => match size {
                Size::B => bus.write8(a, v as u8)?,
                Size::W => bus.write16(a, v as u16)?,
                Size::L => bus.write32(a, v)?,
            },
            _ => return Err(CpuError::Illegal { pc: pc0, op }),
        }
        Ok(())
    }

    fn set_ccr(&mut self, mask: u16, bits: u16) {
        self.sr = (self.sr & !mask) | (bits & mask);
    }

    fn flags_move(&mut self, v: u32, size: Size) {
        let mut f = 0;
        if v & size.msb() != 0 {
            f |= CCR_N;
        }
        if v & size.mask() == 0 {
            f |= CCR_Z;
        }
        self.set_ccr(CCR_N | CCR_Z | CCR_V | CCR_C, f);
    }

    fn add(&mut self, dst: u32, src: u32, size: Size, set_x: bool) -> u32 {
        let wide = (dst & size.mask()) as u64 + (src & size.mask()) as u64;
        let res = wide as u32 & size.mask();
        let mut f = 0;
        if res & size.msb() != 0 {
            f |= CCR_N;
        }
        if res == 0 {
            f |= CCR_Z;
        }
        if (dst ^ res) & (src ^ res) & size.msb() != 0 {
            f |= CCR_V;
        }
        if wide > size.mask() as u64 {
            f |= CCR_C;
            if set_x {
                f |= CCR_X;
            }
        }
        self.set_ccr(
            CCR_N | CCR_Z | CCR_V | CCR_C | if set_x { CCR_X } else { 0 },
            f,
        );
        res
    }

    fn sub(&mut self, dst: u32, src: u32, size: Size, set_x: bool) -> u32 {
        let (d, s) = (dst & size.mask(), src & size.mask());
        let res = d.wrapping_sub(s) & size.mask();
        let mut f = 0;
        if res & size.msb() != 0 {
            f |= CCR_N;
        }
        if res == 0 {
            f |= CCR_Z;
        }
        if (d ^ s) & (d ^ res) & size.msb() != 0 {
            f |= CCR_V;
        }
        if s > d {
            f |= CCR_C;
            if set_x {
                f |= CCR_X;
            }
        }
        self.set_ccr(
            CCR_N | CCR_Z | CCR_V | CCR_C | if set_x { CCR_X } else { 0 },
            f,
        );
        res
    }

    fn addx(&mut self, dst: u32, src: u32) -> u32 {
        let extend = u64::from(self.sr & CCR_X != 0);
        let wide = u64::from(dst) + u64::from(src) + extend;
        let result = wide as u32;
        let signed_wide = i64::from(dst as i32) + i64::from(src as i32) + extend as i64;
        let mut flags = 0;
        if result & 0x8000_0000 != 0 {
            flags |= CCR_N;
        }
        if result == 0 && self.sr & CCR_Z != 0 {
            flags |= CCR_Z;
        }
        if !(i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&signed_wide) {
            flags |= CCR_V;
        }
        if wide > u64::from(u32::MAX) {
            flags |= CCR_C | CCR_X;
        }
        self.set_ccr(CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X, flags);
        result
    }

    fn subx(&mut self, dst: u32, src: u32) -> u32 {
        let extend = u64::from(self.sr & CCR_X != 0);
        let result = dst.wrapping_sub(src).wrapping_sub(extend as u32);
        let signed_wide = i64::from(dst as i32) - i64::from(src as i32) - extend as i64;
        let mut flags = 0;
        if result & 0x8000_0000 != 0 {
            flags |= CCR_N;
        }
        if result == 0 && self.sr & CCR_Z != 0 {
            flags |= CCR_Z;
        }
        if !(i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&signed_wide) {
            flags |= CCR_V;
        }
        if u64::from(dst) < u64::from(src) + extend {
            flags |= CCR_C | CCR_X;
        }
        self.set_ccr(CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X, flags);
        result
    }

    fn cond(&self, c: u16) -> bool {
        let (n, z, v, cy) = (
            self.sr & CCR_N != 0,
            self.sr & CCR_Z != 0,
            self.sr & CCR_V != 0,
            self.sr & CCR_C != 0,
        );
        match c {
            0 => true,
            1 => false,
            2 => !cy && !z,
            3 => cy || z,
            4 => !cy,
            5 => cy,
            6 => !z,
            7 => z,
            8 => !v,
            9 => v,
            10 => !n,
            11 => n,
            12 => n == v,
            13 => n != v,
            14 => !z && n == v,
            _ => z || n != v,
        }
    }

    fn push32<B: Bus>(&mut self, bus: &mut B, v: u32) -> Result<(), CpuError> {
        self.a[7] = self.a[7].wrapping_sub(4);
        bus.write32(self.a[7], v)?;
        Ok(())
    }

    fn take_trap<B: Bus>(
        &mut self,
        bus: &mut B,
        pc0: u32,
        op: u16,
        trap_number: u32,
    ) -> Result<(), CpuError> {
        if self.sr & 0x2000 == 0 {
            return Err(CpuError::UnsupportedExceptionState { pc: pc0, op });
        }

        let saved_sr = self.sr;
        let original_sp = self.a[7];
        let format = 4 + (original_sp & 3);
        let frame_sp = original_sp.wrapping_sub(4 + format);
        let vector_number = 32 + trap_number;
        bus.write16(frame_sp, ((format as u16) << 12) | vector_number as u16)?;
        bus.write16(frame_sp.wrapping_add(2), saved_sr)?;
        bus.write32(frame_sp.wrapping_add(4), self.pc)?;
        self.a[7] = frame_sp;
        self.sr = (saved_sr & !0x8000) | 0x2000;

        let vector_address = self
            .vbr
            .wrapping_add(0x80)
            .wrapping_add(trap_number.wrapping_mul(4));
        self.pc = bus.read32(vector_address)?;
        Ok(())
    }

    fn take_interrupt<B: Bus>(
        &mut self,
        bus: &mut B,
        level: u8,
        vector: u8,
    ) -> Result<(), CpuError> {
        if self.sr & 0x2000 == 0 {
            return Err(CpuError::UnsupportedExceptionState {
                pc: self.pc,
                op: 0x4E73,
            });
        }
        let saved_sr = self.sr;
        let original_sp = self.a[7];
        let format = 4 + (original_sp & 3);
        let frame_sp = original_sp.wrapping_sub(4 + format);
        bus.write16(frame_sp, ((format as u16) << 12) | u16::from(vector))?;
        bus.write16(frame_sp.wrapping_add(2), saved_sr)?;
        bus.write32(frame_sp.wrapping_add(4), self.pc)?;
        self.a[7] = frame_sp;
        self.sr = (saved_sr & !(0x8000 | 0x0700)) | 0x2000 | (u16::from(level) << 8);
        self.pc = bus.read32(self.vbr.wrapping_add(u32::from(vector) * 4))?;
        Ok(())
    }

    /// Execute one instruction.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> Result<(), CpuError> {
        self.step_with_loop_budget(bus, None)
    }

    fn step_with_loop_budget<B: Bus>(
        &mut self,
        bus: &mut B,
        loop_budget: Option<(usize, usize)>,
    ) -> Result<(), CpuError> {
        self.fast_forwarded_in_step = 0;
        let pc0 = self.pc;
        bus.begin_instruction(pc0);
        bus.report_instruction_state(pc0, &self.d, &self.a, self.sr);
        let op = self.fetch16(bus)?;
        let ill = CpuError::Illegal { pc: pc0, op };
        let (mode, reg) = ((op >> 3) & 7, (op & 7) as usize);
        match op >> 12 {
            0 => {
                if op & 0xFFF8 == 0x04C0 {
                    let source = self.d[reg];
                    let result = source.leading_zeros();
                    self.d[reg] = result;
                    let mut flags = 0;
                    if source & 0x8000_0000 != 0 {
                        flags |= CCR_N;
                    }
                    if source == 0 {
                        flags |= CCR_Z;
                    }
                    self.set_ccr(CCR_N | CCR_Z | CCR_V | CCR_C, flags);
                } else if op & 0x0100 != 0 && mode != 1 {
                    let bit_register = ((op >> 9) & 7) as usize;
                    let size = if mode == 0 { Size::L } else { Size::B };
                    let bit = self.d[bit_register] & if size == Size::L { 31 } else { 7 };
                    let operand = self.ea(bus, mode, reg, size, pc0, op)?;
                    let value = self.read(bus, operand, size)?;
                    let mask = 1 << bit;
                    self.set_ccr(CCR_Z, if value & mask == 0 { CCR_Z } else { 0 });
                    match op & 0x00C0 {
                        0x0000 => {}
                        0x0040 => self.write(bus, operand, size, value ^ mask, pc0, op)?,
                        0x0080 => self.write(bus, operand, size, value & !mask, pc0, op)?,
                        0x00C0 => self.write(bus, operand, size, value | mask, pc0, op)?,
                        _ => unreachable!(),
                    }
                } else if op & 0xFF00 == 0x0800 {
                    if mode == 1 || (mode == 7 && reg > 1) {
                        return Err(ill);
                    }
                    let size = if mode == 0 { Size::L } else { Size::B };
                    let bit = (self.fetch16(bus)? & if mode == 0 { 31 } else { 7 }) as u32;
                    let operand = self.ea(bus, mode, reg, size, pc0, op)?;
                    let mask = 1 << bit;
                    let old = self.read(bus, operand, size)?;
                    self.set_ccr(CCR_Z, if old & mask == 0 { CCR_Z } else { 0 });
                    match (op >> 6) & 3 {
                        0 => {}
                        1 => self.write(bus, operand, size, old ^ mask, pc0, op)?,
                        2 => self.write(bus, operand, size, old & !mask, pc0, op)?,
                        3 => self.write(bus, operand, size, old | mask, pc0, op)?,
                        _ => unreachable!(),
                    }
                } else if op & 0xFFF8 == 0x0680 {
                    let immediate = self.fetch32(bus)?;
                    self.d[reg] = self.add(self.d[reg], immediate, Size::L, false);
                } else if op & 0xFFF8 == 0x0480 {
                    let immediate = self.fetch32(bus)?;
                    self.d[reg] = self.sub(self.d[reg], immediate, Size::L, true);
                } else if op & 0xFFF8 == 0x0C80 {
                    let immediate = self.fetch32(bus)?;
                    self.sub(self.d[reg], immediate, Size::L, false);
                } else if matches!(op & 0xFFF8, 0x0280 | 0x0080 | 0x0A80) {
                    let immediate = self.fetch32(bus)?;
                    let result = match op & 0xFFF8 {
                        0x0280 => self.d[reg] & immediate,
                        0x0080 => self.d[reg] | immediate,
                        _ => self.d[reg] ^ immediate,
                    };
                    self.d[reg] = result;
                    self.flags_move(result, Size::L);
                } else {
                    return Err(ill);
                }
            }
            1..=3 => {
                let size = match op >> 12 {
                    1 => Size::B,
                    3 => Size::W,
                    _ => Size::L,
                };
                let src = self.ea(bus, mode, reg, size, pc0, op)?;
                let mut v = self.read(bus, src, size)?;
                let (dmode, dreg) = ((op >> 6) & 7, ((op >> 9) & 7) as usize);
                if dmode == 1 {
                    // MOVEA: word source is sign-extended, flags untouched.
                    match size {
                        Size::B => return Err(ill),
                        Size::W => v = v as u16 as i16 as i32 as u32,
                        Size::L => {}
                    }
                    self.a[dreg] = v;
                } else {
                    let dst = self.ea(bus, dmode, dreg, size, pc0, op)?;
                    self.write(bus, dst, size, v, pc0, op)?;
                    self.flags_move(v, size);
                }
            }
            4 => {
                if op & 0xFFF8 == 0x4080 {
                    let value = self.d[reg];
                    let extend = u32::from(self.sr & CCR_X != 0);
                    let result = 0u32.wrapping_sub(value).wrapping_sub(extend);
                    let mut flags = 0;
                    if result & 0x8000_0000 != 0 {
                        flags |= CCR_N;
                    }
                    if result == 0 && self.sr & CCR_Z != 0 {
                        flags |= CCR_Z;
                    }
                    if value == 0x8000_0000 && extend == 0 {
                        flags |= CCR_V;
                    }
                    if value != 0 || extend != 0 {
                        flags |= CCR_C | CCR_X;
                    }
                    self.d[reg] = result;
                    self.set_ccr(CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X, flags);
                } else if op & 0xFFF8 == 0x4480 {
                    self.d[reg] = self.sub(0, self.d[reg], Size::L, true);
                } else if op & 0xFFF0 == 0x4E40 {
                    self.take_trap(bus, pc0, op, u32::from(op & 0xF))?;
                } else if op == 0x4E73 {
                    if self.sr & 0x2000 == 0 {
                        return Err(CpuError::Privilege { pc: pc0, op });
                    }
                    let frame_sp = self.a[7];
                    let format_vector = bus.read16(frame_sp)?;
                    let format = (format_vector >> 12) & 0xF;
                    if !(4..=7).contains(&format) {
                        return Err(CpuError::Illegal { pc: pc0, op });
                    }
                    let restored_sr = bus.read16(frame_sp.wrapping_add(2))?;
                    let restored_pc = bus.read32(frame_sp.wrapping_add(4))?;
                    bus.report_rte(pc0, frame_sp, format_vector, restored_sr, restored_pc);
                    self.a[7] = frame_sp.wrapping_add(4 + u32::from(format));
                    self.sr = restored_sr;
                    self.pc = restored_pc;
                } else if op & 0xFFC0 == 0x4C40 {
                    // REM (Dr != Dq) writing only Dr is inferred, not confirmed against the manual.
                    // DIVS/DIVU.L <ea>,Dq and REMS/REMU.L <ea>,Dr:Dq (32-bit only).
                    if !matches!(mode, 0 | 2 | 3 | 4 | 5) {
                        return Err(ill);
                    }
                    let extension = self.fetch16(bus)?;
                    if extension & 0x87F8 != 0 {
                        return Err(CpuError::Illegal {
                            pc: pc0,
                            op: extension,
                        });
                    }
                    let signed = extension & 0x0800 != 0;
                    let dq = ((extension >> 12) & 7) as usize;
                    let dr = (extension & 7) as usize;
                    let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let divisor = self.read(bus, source, Size::L)?;
                    let dividend = self.d[dq];
                    let division = if signed {
                        (dividend as i32)
                            .checked_div(divisor as i32)
                            .zip((dividend as i32).checked_rem(divisor as i32))
                            .map(|(q, r)| (q as u32, r as u32))
                    } else {
                        dividend.checked_div(divisor).zip(dividend.checked_rem(divisor))
                    };
                    let Some((quotient, remainder)) = division else {
                        return Err(CpuError::UnsupportedExceptionState { pc: pc0, op });
                    };
                    let result = if dr == dq {
                        self.d[dq] = quotient;
                        quotient
                    } else {
                        self.d[dr] = remainder;
                        remainder
                    };
                    self.flags_move(result, Size::L);
                                } else if op & 0xFFF8 == 0x4C80 {
                    // SATS.L: with CCR[V] set, saturate in the direction opposite to the wrapped sign.
                    let value = self.d[reg];
                    let result = if self.sr & CCR_V != 0 {
                        if value & 0x8000_0000 != 0 { 0x7FFF_FFFF } else { 0x8000_0000 }
                    } else {
                        value
                    };
                    self.d[reg] = result;
                    self.flags_move(result, Size::L);
                } else if op & 0xFFC0 == 0x4C00 {
                    if !matches!(mode, 0 | 2 | 3 | 4 | 5) {
                        return Err(ill);
                    }
                    let extension = self.fetch16(bus)?;
                    if extension & 0x87FF != 0 {
                        return Err(CpuError::Illegal {
                            pc: pc0,
                            op: extension,
                        });
                    }
                    let destination = ((extension >> 12) & 7) as usize;
                    let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let source = self.read(bus, source, Size::L)?;
                    let result = if extension & 0x0800 != 0 {
                        ((source as i32 as i64) * (self.d[destination] as i32 as i64)) as u32
                    } else {
                        (u64::from(source) * u64::from(self.d[destination])) as u32
                    };
                    self.d[destination] = result;
                    let mut flags = 0;
                    if result & 0x8000_0000 != 0 {
                        flags |= CCR_N;
                    }
                    if result == 0 {
                        flags |= CCR_Z;
                    }
                    self.set_ccr(CCR_N | CCR_Z | CCR_V | CCR_C, flags);
                } else if op & 0xFFC0 == 0x40C0 {
                    let destination = self.ea(bus, mode, reg, Size::W, pc0, op)?;
                    self.write(bus, destination, Size::W, u32::from(self.sr), pc0, op)?;
                } else if op & 0xFFC0 == 0x44C0 {
                    let source = self.ea(bus, mode, reg, Size::B, pc0, op)?;
                    let value = self.read(bus, source, Size::B)?;
                    self.set_ccr(0x1F, value as u16);
                } else if op & 0xFF00 == 0x4600 && mode == 0 && op & 0x00C0 != 0x00C0 {
                    let size = match op & 0x00C0 {
                        0x0000 => Size::B,
                        0x0040 => Size::W,
                        0x0080 => Size::L,
                        _ => return Err(ill),
                    };
                    let mask = size.mask();
                    let result = !self.d[reg] & mask;
                    self.d[reg] = (self.d[reg] & !mask) | result;
                    self.flags_move(result, size);
                } else if op == 0x46FC {
                    if self.sr & 0x2000 == 0 {
                        return Err(CpuError::Privilege { pc: pc0, op });
                    }
                    self.sr = self.fetch16(bus)?;
                } else if op & 0xFFC0 == 0x46C0 {
                    if self.sr & 0x2000 == 0 {
                        return Err(CpuError::Privilege { pc: pc0, op });
                    }
                    let source = self.ea(bus, mode, reg, Size::W, pc0, op)?;
                    self.sr = self.read(bus, source, Size::W)? as u16;
                } else if op & 0xFFF8 == 0x4840 {
                    let swapped = self.d[reg].rotate_left(16);
                    self.d[reg] = swapped;
                    self.flags_move(swapped, Size::L);
                } else if op & 0xFFC0 == 0x4840 {
                    let address = self.control_ea(bus, op, pc0)?;
                    self.push32(bus, address)?;
                } else if op & 0xFFF8 == 0x4E50 {
                    let displacement = self.fetch16(bus)? as i16 as i32 as u32;
                    self.push32(bus, self.a[reg])?;
                    self.a[reg] = self.a[7];
                    self.a[7] = self.a[7].wrapping_add(displacement);
                } else if op & 0xFFF8 == 0x4E58 {
                    self.a[7] = self.a[reg];
                    let restored = bus.read32(self.a[7])?;
                    self.a[7] = self.a[7].wrapping_add(4);
                    self.a[reg] = restored;
                } else if op & 0xFFF8 == 0x4880 {
                    let value = self.d[reg] as u8 as i8 as i16 as u16;
                    self.d[reg] = (self.d[reg] & 0xFFFF_0000) | u32::from(value);
                    self.flags_move(u32::from(value), Size::W);
                } else if op & 0xFFF8 == 0x48C0 {
                    let value = self.d[reg] as u16 as i16 as i32 as u32;
                    self.d[reg] = value;
                    self.flags_move(value, Size::L);
                } else if op & 0xFFC0 == 0x48C0 && matches!(mode, 2 | 5) {
                    let mask = self.fetch16(bus)?;
                    let destination = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let mut address = match destination {
                        Operand::Mem(address) => address,
                        _ => return Err(ill),
                    };
                    for register in 0..16 {
                        if mask & (1 << register) != 0 {
                            let value = if register < 8 {
                                self.d[register]
                            } else {
                                self.a[register - 8]
                            };
                            bus.write32(address, value)?;
                            address = address.wrapping_add(4);
                        }
                    }
                } else if op & 0xFFC0 == 0x4CC0 && matches!(mode, 2 | 5) {
                    let mask = self.fetch16(bus)?;
                    let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let mut address = match source {
                        Operand::Mem(address) => address,
                        _ => return Err(ill),
                    };
                    for register in 0..16 {
                        if mask & (1 << register) != 0 {
                            let value = bus.read32(address)?;
                            if register < 8 {
                                self.d[register] = value;
                            } else {
                                self.a[register - 8] = value;
                            }
                            address = address.wrapping_add(4);
                        }
                    }
                } else if op == 0x4E7B {
                    let ext = self.fetch16(bus)?;
                    let register = ((ext >> 12) & 0xF) as usize;
                    let value = if register < 8 {
                        self.d[register]
                    } else {
                        self.a[register - 8]
                    };
                    match ext & 0x0FFF {
                        0x0002 => self.cacr = value,
                        0x0004 => self.acr0 = value,
                        0x0005 => self.acr1 = value,
                        0x0801 => self.vbr = value,
                        0x0C05 => self.rambar = value,
                        0x0009 => {
                            self.rgpiobar = RGPIOBAR_RESET | (value & 3);
                            bus.set_rgpiobar(self.rgpiobar);
                        }
                        _ => return Err(CpuError::Illegal { pc: pc0, op: ext }),
                    }
                } else if op == 0x4E71 {
                    // NOP
                } else if op == 0x4E75 {
                    self.pc = bus.read32(self.a[7])?;
                    self.a[7] = self.a[7].wrapping_add(4);
                } else if op & 0xFFC0 == 0x4EC0 {
                    self.pc = self.control_ea(bus, op, pc0)?;
                } else if op & 0xFFC0 == 0x4E80 {
                    let target = self.control_ea(bus, op, pc0)?;
                    self.push32(bus, self.pc)?;
                    self.pc = target;
                } else if op & 0xF1C0 == 0x41C0 {
                    let a = self.control_ea(bus, op, pc0)?;
                    self.a[((op >> 9) & 7) as usize] = a;
                } else if op == 0x4AC8 {
                    if self.sr & 0x2000 == 0 {
                        return Err(CpuError::Privilege { pc: pc0, op });
                    }
                    return Err(CpuError::Halt { pc: pc0 });
                } else if op & 0xFF00 == 0x4200 || op & 0xFF00 == 0x4A00 {
                    let size = match (op >> 6) & 3 {
                        0 => Size::B,
                        1 => Size::W,
                        2 => Size::L,
                        _ => return Err(ill),
                    };
                    let o = self.ea(bus, mode, reg, size, pc0, op)?;
                    if op & 0xFF00 == 0x4200 {
                        self.write(bus, o, size, 0, pc0, op)?;
                        self.flags_move(0, size);
                    } else {
                        let v = self.read(bus, o, size)?;
                        self.flags_move(v, size);
                        let operand = match o {
                            Operand::Dn(register) => format!("D{register}"),
                            Operand::An(register) => format!("A{register}"),
                            Operand::Mem(address) => format!("memory[{address:#010x}]"),
                            Operand::Imm(value) => format!("immediate[{value:#010x}]"),
                        };
                        bus.report_tst(pc0, &operand, v, size.bytes() as u8, self.sr);
                    }
                } else {
                    return Err(ill);
                }
            }
            5 => {
                if op == 0x51FC {
                    // TRAPF has a false condition and therefore never traps.
                    return Ok(());
                }
                if (op >> 6) & 3 == 3 {
                    // Scc.B Dn: the only form ColdFire provides.
                    if mode != 0 {
                        return Err(ill);
                    }
                    let byte = if self.cond((op >> 8) & 0xF) { 0xFF } else { 0 };
                    self.d[reg] = (self.d[reg] & !0xFF) | byte;
                    return Ok(());
                }
                // ColdFire ADDQ/SUBQ are long only.
                if (op >> 6) & 7 != 2 && (op >> 6) & 7 != 6 {
                    return Err(ill);
                }
                let sub = op & 0x0100 != 0;
                let q = match (op >> 9) & 7 {
                    0 => 8,
                    n => n as u32,
                };
                if mode == 1 {
                    let a = self.a[reg];
                    self.a[reg] = if sub {
                        a.wrapping_sub(q)
                    } else {
                        a.wrapping_add(q)
                    };
                } else {
                    let o = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let v = self.read(bus, o, Size::L)?;
                    let r = if sub {
                        self.sub(v, q, Size::L, true)
                    } else {
                        self.add(v, q, Size::L, true)
                    };
                    self.write(bus, o, Size::L, r, pc0, op)?;
                }
            }
            6 => {
                let c = (op >> 8) & 0xF;
                let base = pc0.wrapping_add(2);
                let disp = match op & 0xFF {
                    0x00 => self.fetch16(bus)? as i16 as i32 as u32,
                    0xFF => self.fetch32(bus)?,
                    d => d as u8 as i8 as i32 as u32,
                };
                if c == 1 {
                    let target = base.wrapping_add(disp);
                    self.push32(bus, self.pc)?;
                    self.pc = target;
                    bus.report_branch(pc0, c as u8, true, target, self.sr);
                    if target <= pc0 {
                        if let Some((instructions, remaining)) = loop_budget {
                            self.fast_forwarded_in_step = bus.fast_forward_loop(LoopContext {
                                from_pc: pc0,
                                pc: target,
                                d: &self.d,
                                a: &self.a,
                                sr: self.sr,
                                instructions,
                                remaining,
                            });
                        }
                    }
                } else {
                    let taken = self.cond(c);
                    let target = base.wrapping_add(disp);
                    if taken {
                        self.pc = target;
                        if target <= pc0 {
                            if let Some((instructions, remaining)) = loop_budget {
                                self.fast_forwarded_in_step = bus.fast_forward_loop(LoopContext {
                                    from_pc: pc0,
                                    pc: target,
                                    d: &self.d,
                                    a: &self.a,
                                    sr: self.sr,
                                    instructions,
                                    remaining,
                                });
                            }
                        }
                    }
                    bus.report_branch(pc0, c as u8, taken, target, self.sr);
                }
            }
            0xE => {
                // Long shifts: ASR/ASL (type 00), LSR/LSL (type 01).
                let left = op & 0x0100 != 0;
                let shift_type = (op >> 3) & 3;
                let supported = op & 0x00C0 == 0x0080 && shift_type <= 1;
                if !supported {
                    return Err(ill);
                }
                let register_count = op & 0x0020 != 0;
                let encoded_count = ((op >> 9) & 7) as u32;
                let count = if register_count {
                    self.d[((op >> 9) & 7) as usize] & 0x3F
                } else if encoded_count == 0 {
                    8
                } else {
                    encoded_count
                };
                let original = self.d[reg];
                let (result, carry) = match (shift_type, left) {
                    (_, _) if count == 0 => (original, false),
                    (0, true) if count < 32 => (
                        original << count,
                        original & (1 << (32 - count)) != 0,
                    ),
                    (0, true) if count == 32 => (0, original & 1 != 0),
                    (0, true) => (0, false),
                    (0, false) if count < 32 => (
                        ((original as i32) >> count) as u32,
                        original & (1 << (count - 1)) != 0,
                    ),
                    (0, false) => (
                        if original & 0x8000_0000 != 0 {
                            u32::MAX
                        } else {
                            0
                        },
                        original & 0x8000_0000 != 0,
                    ),
                    (_, false) if count < 32 => {
                        (original >> count, original & (1 << (count - 1)) != 0)
                    }
                    (_, false) if count == 32 => (0, original & 0x8000_0000 != 0),
                    (_, false) => (0, false),
                    (_, true) if count < 32 => {
                        (original << count, original & (1 << (32 - count)) != 0)
                    }
                    (_, true) if count == 32 => (0, original & 1 != 0),
                    (_, true) => (0, false),
                };
                self.d[reg] = result;
                let mut flags = 0;
                if result & 0x8000_0000 != 0 {
                    flags |= CCR_N;
                }
                if result == 0 {
                    flags |= CCR_Z;
                }
                if carry {
                    flags |= CCR_C | CCR_X;
                }
                if shift_type == 0 && left && count != 0 {
                    // V: the sign bit changed at any point during the shift.
                    let overflow = if count >= 32 {
                        original != 0
                    } else {
                        let top = u64::from(original) >> (31 - count);
                        top != 0 && top != (1u64 << (count + 1)) - 1
                    };
                    if overflow {
                        flags |= CCR_V;
                    }
                }
                self.set_ccr(
                    CCR_N | CCR_Z | CCR_V | CCR_C | if count == 0 { 0 } else { CCR_X },
                    flags,
                );
            }
            0xF => {
                // CPUSHL dc/ic/bc,(An): there is no cache model, so the push is a no-op.
                if matches!(op & 0xFFF8, 0xF468 | 0xF4A8 | 0xF4E8) {
                    if self.sr & 0x2000 == 0 {
                        return Err(CpuError::Privilege { pc: pc0, op });
                    }
                    return Ok(());
                }
                if op & 0xFFC0 != 0xFBC0 {
                    return Err(ill);
                }
                if self.sr & 0x2000 == 0 {
                    return Err(CpuError::Privilege { pc: pc0, op });
                }
                if self.fetch16(bus)? != 0x0003 {
                    return Err(ill);
                }
                let address = self.control_ea(bus, op, pc0)?;
                let command = bus.read32(address)?;
                let data_tail = bus.read32(address.wrapping_add(4))?;
                let command_word = (command >> 16) as u16;
                if command_word & 0xFFF0 != 0x2C80 {
                    return Err(CpuError::Illegal {
                        pc: pc0,
                        op: command_word,
                    });
                }
                let register = (command_word & 0xF) as u8;
                if register != 0 {
                    return Err(CpuError::UnsupportedDebugRegister { pc: pc0, register });
                }
                self.debug_csr = ((command & 0xFFFF) << 16) | (data_tail >> 16);
            }
            7 => {
                if op & 0x0100 != 0 {
                    // MVS (bit 7 clear) and MVZ (bit 7 set), both ISA_B.
                    let size = if op & 0x0040 != 0 { Size::W } else { Size::B };
                    let source = self.ea(bus, mode, reg, size, pc0, op)?;
                    let v = self.read(bus, source, size)?;
                    let extended = if op & 0x0080 != 0 {
                        v & size.mask()
                    } else if size == Size::B {
                        v as u8 as i8 as i32 as u32
                    } else {
                        v as u16 as i16 as i32 as u32
                    };
                    self.d[((op >> 9) & 7) as usize] = extended;
                    self.flags_move(extended, Size::L);
                    return Ok(());
                }
                let v = op as u8 as i8 as i32 as u32;
                self.d[((op >> 9) & 7) as usize] = v;
                self.flags_move(v, Size::L);
            }
            8 => match (op >> 6) & 7 {
                2 if mode != 1 => {
                    let dst = ((op >> 9) & 7) as usize;
                    let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let source_value = self.read(bus, source, Size::L)?;
                    let result = self.d[dst] | source_value;
                    self.d[dst] = result;
                    self.flags_move(result, Size::L);
                }
                6 if mode >= 2 => {
                    let src = ((op >> 9) & 7) as usize;
                    let destination = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let destination_value = self.read(bus, destination, Size::L)?;
                    let result = destination_value | self.d[src];
                    self.write(bus, destination, Size::L, result, pc0, op)?;
                    self.flags_move(result, Size::L);
                }
                _ => return Err(ill),
            },
            0xA => {
                // EMAC register transfers; MAC and MSAC (and the move-with-load forms) are not modelled.
                let acc = ((op >> 9) & 3) as usize;
                let (mode, reg) = ((op >> 3) & 7, (op & 7) as usize);
                let write_rx = |cpu: &mut Self, value: u32| {
                    if op & 8 != 0 {
                        cpu.a[reg] = value;
                    } else {
                        cpu.d[reg] = value;
                    }
                };
                if op & 0xF9B0 == 0xA180 {
                    let value = if op & 0x40 != 0 {
                        self.emac.store_and_clear(acc)
                    } else {
                        self.emac.store(acc)
                    };
                    write_rx(self, value);
                } else if op & 0xF9FC == 0xA110 {
                    self.emac.copy((op & 3) as usize, acc);
                } else if op & 0xF9C0 == 0xA100 {
                    let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let value = self.read(bus, source, Size::L)?;
                    self.emac.load(acc, value);
                } else if op == 0xA9C0 {
                    let bits = self.emac.macsr_to_ccr();
                    self.set_ccr(CCR_N | CCR_Z | CCR_V | CCR_C, bits);
                } else if op & 0xFFC0 == 0xA900 {
                    let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let value = self.read(bus, source, Size::L)?;
                    self.emac.write_macsr(value);
                } else if op & 0xFFF0 == 0xA980 {
                    write_rx(self, self.emac.macsr);
                } else if op & 0xFFC0 == 0xAD00 {
                    let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let value = self.read(bus, source, Size::L)?;
                    self.emac.write_mask(value);
                } else if op & 0xFFF0 == 0xAD80 {
                    write_rx(self, self.emac.read_mask());
                } else if op & 0xFBC0 == 0xAB00 {
                    let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let value = self.read(bus, source, Size::L)?;
                    self.emac.set_ext_pair(op & 0x400 != 0, value);
                } else if op & 0xFBF0 == 0xAB80 {
                    write_rx(self, self.emac.ext_pair(op & 0x400 != 0));
                } else if op & 0xF100 == 0xA000 {
                    // MAC/MSAC (extension bit 8 subtracts) with an optional load; the dual-accumulate forms
                    // are absent from the MCF54418 manual.
                    let ext = self.fetch16(bus)?;
                    let load = op & 0x30 != 0;
                    let register = |cpu: &Self, address_flag: bool, number: u16| {
                        if address_flag {
                            cpu.a[usize::from(number & 7)]
                        } else {
                            cpu.d[usize::from(number & 7)]
                        }
                    };
                    let (x, y, accumulator) = if load {
                        (
                            register(self, ext & 0x8000 != 0, ext >> 12),
                            register(self, ext & 8 != 0, ext),
                            (((op >> 7) & 1) ^ 1) | ((ext >> 3) & 2),
                        )
                    } else {
                        (
                            register(self, op & 0x40 != 0, op >> 9),
                            register(self, op & 8 != 0, op),
                            ((op >> 7) & 1) | ((ext >> 3) & 2),
                        )
                    };
                    let loaded = if load {
                        let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                        let Operand::Mem(address) = source else {
                            return Err(ill);
                        };
                        Some(bus.read32(address & self.emac.read_mask())?)
                    } else {
                        None
                    };
                    let word = (ext & 0x800 == 0).then_some((ext & 0x80 != 0, ext & 0x40 != 0));
                    self.emac.mac(usize::from(accumulator), x, y, word, ((ext >> 9) & 3) as u8, ext & 0x100 != 0);
                    if let Some(value) = loaded {
                        let destination = ((op >> 9) & 7) as usize;
                        if op & 0x40 != 0 {
                            self.a[destination] = value;
                        } else {
                            self.d[destination] = value;
                        }
                    }
                } else {
                    return Err(ill);
                }
            }
            0xC if op & 0xF1C0 == 0xC1C0 => {
                let dst = ((op >> 9) & 7) as usize;
                let (mode, reg) = ((op >> 3) & 7, (op & 7) as usize);
                if mode == 1 {
                    return Err(ill);
                }
                let source = self.ea(bus, mode, reg, Size::W, pc0, op)?;
                let source_value = self.read(bus, source, Size::W)? as u16 as i16 as i32;
                let destination_value = self.d[dst] as u16 as i16 as i32;
                let result = destination_value.wrapping_mul(source_value) as u32;
                self.d[dst] = result;
                self.flags_move(result, Size::L);
            }
            0xC if op & 0xF1C0 == 0xC0C0 => {
                let dst = ((op >> 9) & 7) as usize;
                let (mode, reg) = ((op >> 3) & 7, (op & 7) as usize);
                if mode == 1 {
                    return Err(ill);
                }
                let source = self.ea(bus, mode, reg, Size::W, pc0, op)?;
                let source_value = self.read(bus, source, Size::W)? & 0xFFFF;
                let result = (self.d[dst] & 0xFFFF).wrapping_mul(source_value);
                self.d[dst] = result;
                self.flags_move(result, Size::L);
            }
            0xC => match (op >> 6) & 7 {
                2 if mode != 1 => {
                    let dst = ((op >> 9) & 7) as usize;
                    let source = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let source_value = self.read(bus, source, Size::L)?;
                    let result = self.d[dst] & source_value;
                    self.d[dst] = result;
                    self.flags_move(result, Size::L);
                }
                6 if mode >= 2 => {
                    let src = ((op >> 9) & 7) as usize;
                    let destination = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                    let destination_value = self.read(bus, destination, Size::L)?;
                    let result = destination_value & self.d[src];
                    self.write(bus, destination, Size::L, result, pc0, op)?;
                    self.flags_move(result, Size::L);
                }
                _ => return Err(ill),
            },
            9 if op & 0xF1F8 == 0x9180 && op & 0x00C0 == 0x0080 => {
                let dst = ((op >> 9) & 7) as usize;
                let src = (op & 7) as usize;
                self.d[dst] = self.subx(self.d[dst], self.d[src]);
            }
            0xD if op & 0xF1F8 == 0xD180 && op & 0x00C0 == 0x0080 => {
                let dst = ((op >> 9) & 7) as usize;
                let src = (op & 7) as usize;
                self.d[dst] = self.addx(self.d[dst], self.d[src]);
            }
            nib @ (9 | 0xB | 0xD) => {
                let r = ((op >> 9) & 7) as usize;
                let opmode = (op >> 6) & 7;
                let (size, to_ea) = match (nib, opmode) {
                    (0xB, 0) => (Size::B, false),
                    (0xB, 1) => (Size::W, false),
                    (_, 2) => (Size::L, false),
                    (0xB, 6) if mode != 1 => (Size::L, true),
                    (9 | 0xD, 6) if mode >= 2 => (Size::L, true),
                    (_, 7) => {
                        // ADDA/SUBA/CMPA.L
                        let o = self.ea(bus, mode, reg, Size::L, pc0, op)?;
                        let v = self.read(bus, o, Size::L)?;
                        match nib {
                            0xD => self.a[r] = self.a[r].wrapping_add(v),
                            9 => self.a[r] = self.a[r].wrapping_sub(v),
                            _ => {
                                let a = self.a[r];
                                self.sub(a, v, Size::L, false);
                            }
                        }
                        return Ok(());
                    }
                    _ => return Err(ill),
                };
                let o = self.ea(bus, mode, reg, size, pc0, op)?;
                let ev = self.read(bus, o, size)?;
                let dv = self.d[r];
                if to_ea {
                    let res = if nib == 0xB {
                        ev ^ dv
                    } else if nib == 0xD {
                        self.add(ev, dv, size, true)
                    } else {
                        self.sub(ev, dv, size, true)
                    };
                    self.write(bus, o, size, res, pc0, op)?;
                    if nib == 0xB {
                        self.flags_move(res, size);
                    }
                } else if nib == 0xB {
                    self.sub(dv, ev, size, false);
                } else {
                    let res = if nib == 0xD {
                        self.add(dv, ev, size, true)
                    } else {
                        self.sub(dv, ev, size, true)
                    };
                    self.d[r] = res;
                }
            }
            _ => return Err(ill),
        }
        Ok(())
    }

    pub fn run<B: Bus>(&mut self, bus: &mut B, instruction_limit: usize) -> RunReport {
        self.run_with_interrupt(bus, instruction_limit, None)
    }

    pub fn run_with_interrupt<B: Bus>(
        &mut self,
        bus: &mut B,
        instruction_limit: usize,
        mut injection: Option<InterruptInjection>,
    ) -> RunReport {
        let mut instructions = 0;
        let mut budget_used = 0;
        let mut fast_forwarded_instructions = 0;
        let mut injected_interrupt = None;
        let mut natural_interrupt = None;
        let stop = loop {
            if budget_used >= instruction_limit {
                break RunStop::InstructionLimit;
            }
            if bus.stop_before_instruction(self.pc) {
                break RunStop::Breakpoint { pc: self.pc };
            }
            if let Some(event) = injection {
                let interrupt_mask = ((self.sr >> 8) & 7) as u8;
                if budget_used >= event.instruction_count && event.level > interrupt_mask {
                    match self.take_interrupt(bus, event.level, event.vector) {
                        Ok(()) => {
                            bus.interrupt_taken(event.level, event.vector);
                            injection = None;
                            injected_interrupt = Some(event);
                            continue;
                        }
                        Err(error) => break RunStop::Error(error),
                    }
                }
            }
            if let Some(request) = bus.pending_interrupt() {
                let interrupt_mask = ((self.sr >> 8) & 7) as u8;
                if request.level > interrupt_mask {
                    if let Err(error) = self.take_interrupt(bus, request.level, request.vector) {
                        break RunStop::Error(error);
                    }
                    bus.interrupt_taken(request.level, request.vector);
                    if natural_interrupt.is_none() {
                        natural_interrupt = Some(InterruptDelivery {
                            instruction_count: budget_used,
                            source: request.source,
                            level: request.level,
                            vector: request.vector,
                        });
                    }
                    continue;
                }
            }
            let logical_instruction_count = budget_used + 1;
            let remaining = instruction_limit - logical_instruction_count;
            match self.step_with_loop_budget(bus, Some((logical_instruction_count, remaining))) {
                Ok(()) => {
                    bus.advance_instruction();
                    instructions += 1;
                    budget_used += 1;
                    let skipped = self.fast_forwarded_in_step.min(remaining);
                    budget_used += skipped;
                    fast_forwarded_instructions += skipped;
                }
                Err(CpuError::Halt { pc }) => {
                    instructions += 1;
                    break RunStop::Halt { pc };
                }
                Err(error) => break RunStop::Error(error),
            }
        };
        RunReport {
            instructions,
            fast_forwarded_instructions,
            injected_interrupt,
            natural_interrupt,
            cpu: CpuSnapshot {
                d: self.d,
                a: self.a,
                pc: self.pc,
                sr: self.sr,
                rambar: self.rambar,
                acr0: self.acr0,
                acr1: self.acr1,
                vbr: self.vbr,
                cacr: self.cacr,
                rgpiobar: self.rgpiobar,
                debug_csr: self.debug_csr,
            },
            stop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::FlatRam;

    fn machine(code: &[u16]) -> (Cpu, FlatRam) {
        let mut ram = FlatRam::new(0, 0x3000);
        let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_be_bytes()).collect();
        ram.load(0x1000, &bytes);
        let mut cpu = Cpu::new();
        cpu.pc = 0x1000;
        cpu.a[7] = 0x2000;
        (cpu, ram)
    }

    fn run(cpu: &mut Cpu, ram: &mut FlatRam, n: usize) {
        for _ in 0..n {
            cpu.step(ram).unwrap();
        }
    }

    #[test]
    fn moveq_sign_extends_and_sets_flags() {
        let (mut c, mut r) = machine(&[0x7005, 0x72FF]);
        run(&mut c, &mut r, 2);
        assert_eq!(c.d[0], 5);
        assert_eq!(c.d[1], 0xFFFF_FFFF);
        assert_eq!(c.sr & (CCR_N | CCR_Z), CCR_N);
    }

    #[test]
    fn move_long_immediate() {
        let (mut c, mut r) = machine(&[0x243C, 0x1234, 0x5678]);
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[2], 0x1234_5678);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn subq_bne_loop_counts_down() {
        let (mut c, mut r) = machine(&[0x7003, 0x5380, 0x66FC, 0x4E71]);
        run(&mut c, &mut r, 7);
        assert_eq!(c.d[0], 0);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn trapf_is_a_no_op() {
        let (mut c, mut r) = machine(&[0x51FC, 0x4E71]);
        c.sr = 0x2004;

        run(&mut c, &mut r, 1);

        assert_eq!(c.pc, 0x1002);
        assert_eq!(c.sr, 0x2004);
        assert_eq!(c.a[7], 0x2000);
    }

    #[test]
    fn jsr_rts_use_the_stack() {
        let (mut c, mut r) = machine(&[0x4EB9, 0x0000, 0x1100]);
        r.load(0x1100, &[0x76, 0x07, 0x4E, 0x75]);
        run(&mut c, &mut r, 3);
        assert_eq!(c.d[3], 7);
        assert_eq!(c.pc, 0x1006);
        assert_eq!(c.a[7], 0x2000);
    }

    #[test]
    fn push_and_pop() {
        let (mut c, mut r) = machine(&[0x2F00, 0x221F]);
        c.d[0] = 0xCAFE_F00D;
        run(&mut c, &mut r, 2);
        assert_eq!(c.d[1], 0xCAFE_F00D);
        assert_eq!(c.a[7], 0x2000);
    }

    #[test]
    fn add_carry_and_zero() {
        let (mut c, mut r) = machine(&[0xD081]);
        c.d[0] = 0xFFFF_FFFF;
        c.d[1] = 1;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0);
        assert_eq!(c.sr & 0x1F, CCR_Z | CCR_C | CCR_X);
    }

    #[test]
    fn cmp_borrow_leaves_operand() {
        let (mut c, mut r) = machine(&[0xB081]);
        c.d[0] = 5;
        c.d[1] = 7;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 5);
        assert_eq!(c.sr & 0x1F, CCR_N | CCR_C);
    }

    #[test]
    fn bra_word_displacement() {
        let (mut c, mut r) = machine(&[0x6000, 0x0010]);
        run(&mut c, &mut r, 1);
        assert_eq!(c.pc, 0x1012);
    }

    #[test]
    fn unknown_opcode_is_reported() {
        let (mut c, mut r) = machine(&[0xFFFF]);
        assert_eq!(
            c.step(&mut r),
            Err(CpuError::Illegal {
                pc: 0x1000,
                op: 0xFFFF
            })
        );
    }

    #[test]
    fn halt_is_a_clean_supervisor_stop() {
        let (mut c, mut r) = machine(&[0x4AC8]);
        c.sr = 0x2000;

        let report = c.run(&mut r, 10);

        assert_eq!(report.instructions, 1);
        assert_eq!(report.cpu.pc, 0x1002);
        assert_eq!(report.stop, RunStop::Halt { pc: 0x1000 });
    }

    #[test]
    fn halt_is_privileged() {
        let (mut c, mut r) = machine(&[0x4AC8]);

        let report = c.run(&mut r, 10);

        assert_eq!(
            report.stop,
            RunStop::Error(CpuError::Privilege {
                pc: 0x1000,
                op: 0x4AC8,
            })
        );
    }

    #[test]
    fn injected_interrupt_enters_vector_and_returns_to_loop() {
        let (mut cpu, mut ram) = machine(&[0x60FE]);
        cpu.sr = 0x2000;
        cpu.vbr = 0x1000;
        ram.write32(0x1000 + 206 * 4, 0x1100).unwrap();
        ram.write16(0x1100, 0x4E73).unwrap();

        let injection = InterruptInjection {
            instruction_count: 0,
            level: 6,
            vector: 206,
        };
        let report = cpu.run_with_interrupt(&mut ram, 2, Some(injection));

        assert_eq!(report.injected_interrupt, Some(injection));
        assert_eq!(report.instructions, 2);
        assert_eq!(report.cpu.pc, 0x1000);
        assert_eq!(report.cpu.sr, 0x2000);
        assert_eq!(report.cpu.a[7], 0x2000);
    }

    #[test]
    fn injected_interrupt_waits_until_level_exceeds_sr_mask() {
        let (mut cpu, mut ram) = machine(&[0x60FE]);
        cpu.sr = 0x2600;

        let report = cpu.run_with_interrupt(
            &mut ram,
            1,
            Some(InterruptInjection {
                instruction_count: 0,
                level: 6,
                vector: 206,
            }),
        );

        assert_eq!(report.injected_interrupt, None);
        assert_eq!(report.cpu.pc, 0x1000);
    }

    #[test]
    fn run_reports_instruction_limit_and_cpu_state() {
        let (mut c, mut r) = machine(&[0x4E71]);
        let report = c.run(&mut r, 1);
        assert_eq!(report.instructions, 1);
        assert_eq!(report.cpu.pc, 0x1002);
        assert_eq!(report.cpu.a[7], 0x2000);
        assert_eq!(report.stop, RunStop::InstructionLimit);
    }

    #[test]
    fn run_reports_first_unsupported_opcode() {
        let (mut c, mut r) = machine(&[0x4E71, 0xFFFF]);
        let report = c.run(&mut r, 10);
        assert_eq!(report.instructions, 1);
        assert_eq!(report.cpu.pc, 0x1004);
        assert_eq!(
            report.stop,
            RunStop::Error(CpuError::Illegal {
                pc: 0x1002,
                op: 0xFFFF,
            })
        );
    }

    #[test]
    fn run_reports_first_unmapped_bus_access() {
        let (mut c, mut r) = machine(&[]);
        c.pc = 0x4000;
        let report = c.run(&mut r, 10);
        assert_eq!(report.instructions, 0);
        assert_eq!(report.cpu.pc, 0x4000);
        assert_eq!(
            report.stop,
            RunStop::Error(CpuError::Bus(BusError {
                addr: 0x4000,
                write: false,
            }))
        );
    }

    #[test]
    fn bset_immediate_sets_wrapped_data_register_bit_and_zero_flag() {
        let (mut c, mut r) = machine(&[0x08C0, 0x002C]);
        c.d[0] = 0x0000_087D;
        c.sr = CCR_N | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_187D);
        assert_eq!(c.sr, CCR_N | CCR_C | CCR_Z);
    }

    #[test]
    fn bset_immediate_clears_zero_when_bit_was_already_set() {
        let (mut c, mut r) = machine(&[0x08C0, 0x000C]);
        c.d[0] = 0x0000_187D;
        c.sr = CCR_N | CCR_C | CCR_Z;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_187D);
        assert_eq!(c.sr, CCR_N | CCR_C);
    }

    #[test]
    fn bclr_immediate_clears_wrapped_data_register_bit_and_zero_flag() {
        let (mut c, mut r) = machine(&[0x0880, 0x002C]);
        c.d[0] = 0x0000_187D;
        c.sr = CCR_N | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_087D);
        assert_eq!(c.sr, CCR_N | CCR_C);
    }

    #[test]
    fn bclr_immediate_sets_zero_when_bit_was_already_clear() {
        let (mut c, mut r) = machine(&[0x0880, 0x000C]);
        c.d[0] = 0x0000_087D;
        c.sr = CCR_N | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_087D);
        assert_eq!(c.sr, CCR_N | CCR_C | CCR_Z);
    }

    #[test]
    fn bchg_immediate_displacement_memory_toggles_wrapped_byte_bit() {
        let (mut cpu, mut ram) = machine(&[0x086F, 0x001F, 0x0008]);
        cpu.a[7] = 0x1010;
        ram.write8(0x1018, 0x80).unwrap();
        cpu.sr = CCR_X | CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(ram.read8(0x1018).unwrap(), 0x00);
        assert_eq!(cpu.sr, CCR_X | CCR_V | CCR_C);
        assert_eq!(cpu.pc, 0x1006);
    }

    #[test]
    fn dynamic_bclr_absolute_memory_clears_data_bit_and_updates_zero() {
        let (mut c, mut r) = machine(&[0x01B9, 0x0000, 0x2014]);
        c.d[0] = 3;
        c.sr = CCR_X | CCR_N | CCR_V | CCR_C;
        r.write8(0x2014, 0xFF).unwrap();

        run(&mut c, &mut r, 1);

        assert_eq!(r.read8(0x2014).unwrap(), 0xF7);
        assert_eq!(c.sr, CCR_X | CCR_N | CCR_V | CCR_C);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn dynamic_bset_absolute_memory_sets_data_bit_and_zero_flag() {
        let (mut c, mut r) = machine(&[0x01F9, 0x0000, 0x2014]);
        c.d[0] = 4;
        c.sr = CCR_X | CCR_N | CCR_V | CCR_C;
        r.write8(0x2014, 0).unwrap();

        run(&mut c, &mut r, 1);

        assert_eq!(r.read8(0x2014).unwrap(), 0x10);
        assert_eq!(c.sr, CCR_X | CCR_N | CCR_Z | CCR_V | CCR_C);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn btst_immediate_tests_wrapped_data_register_bit_without_modifying_it() {
        let (mut c, mut r) = machine(&[0x0800, 0x0024]);
        c.d[0] = 0x0000_0010;
        c.sr = CCR_X | CCR_N | CCR_Z | CCR_V | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_0010);
        assert_eq!(c.sr, CCR_X | CCR_N | CCR_V | CCR_C);
    }

    #[test]
    fn btst_immediate_sets_zero_when_data_register_bit_is_clear() {
        let (mut c, mut r) = machine(&[0x0800, 0x0004]);
        c.d[0] = 0;
        c.sr = CCR_X | CCR_N | CCR_V | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0);
        assert_eq!(c.sr, CCR_X | CCR_N | CCR_Z | CCR_V | CCR_C);
    }

    #[test]
    fn andi_long_immediate_updates_data_register_and_flags() {
        let (mut c, mut r) = machine(&[0x0280, 0xFFFF, 0x83FF]);
        c.d[0] = 0xFFFF_87FF;
        c.sr = CCR_X | CCR_V | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0xFFFF_83FF);
        assert_eq!(c.sr, CCR_X | CCR_N);
    }

    #[test]
    fn addi_long_immediate_updates_data_register_and_flags() {
        let (mut c, mut r) = machine(&[0x0680, 0x0000, 0x0001]);
        c.d[0] = 0xFFFF_FFFF;
        c.sr = CCR_X | CCR_V;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[0], 0);
        assert_eq!(c.sr, CCR_X | CCR_Z | CCR_C);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn neg_long_data_register_sets_negative_carry_and_extend() {
        let (mut c, mut r) = machine(&[0x4480]);
        c.d[0] = 1;
        c.sr = CCR_V;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[0], u32::MAX);
        assert_eq!(c.sr, CCR_N | CCR_C | CCR_X);
        assert_eq!(c.pc, 0x1002);
    }

    #[test]
    fn subi_long_immediate_updates_destination_and_extend_flags() {
        let (mut cpu, mut ram) = machine(&[0x0481, 0x0000, 0x0001]);
        cpu.d[1] = 0;
        cpu.sr = CCR_V;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[1], u32::MAX);
        assert_eq!(cpu.sr, CCR_N | CCR_C | CCR_X);
        assert_eq!(cpu.pc, 0x1006);
    }

    #[test]
    fn subx_long_register_uses_destination_and_source_order() {
        let (mut cpu, mut ram) = machine(&[0x9182]);
        cpu.d[0] = 0x0192_1FB6;
        cpu.d[2] = 0x0064_87ED;
        cpu.sr = CCR_N | CCR_Z | CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[0], 0x012D_97C9);
        assert_eq!(cpu.d[2], 0x0064_87ED);
        assert_eq!(cpu.sr, 0);
    }

    #[test]
    fn subx_long_consumes_extend_and_cumulates_zero() {
        let (mut cpu, mut ram) = machine(&[0x9182]);
        cpu.d[0] = 1;
        cpu.d[2] = 0;
        cpu.sr = CCR_X | CCR_Z | CCR_V;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[0], 0);
        assert_eq!(cpu.sr, CCR_Z);
    }

    #[test]
    fn addx_long_register_consumes_extend_and_cumulates_zero() {
        let (mut cpu, mut ram) = machine(&[0xD182]);
        cpu.d[0] = u32::MAX;
        cpu.d[2] = 0;
        cpu.sr = CCR_X | CCR_Z | CCR_V;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[0], 0);
        assert_eq!(cpu.d[2], 0);
        assert_eq!(cpu.sr, CCR_X | CCR_Z | CCR_C);
    }

    #[test]
    fn and_long_displacement_memory_to_data_register_updates_flags() {
        let (mut c, mut r) = machine(&[0xC2AF, 0x0014]);
        c.d[1] = 0xF00F_F00F;
        c.sr = CCR_X | CCR_V | CCR_C;
        r.write32(0x2014, 0x00FF_00FF).unwrap();

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[1], 0x000F_000F);
        assert_eq!(c.sr, CCR_X);
        assert_eq!(c.pc, 0x1004);
    }

    #[test]
    fn and_long_data_register_to_absolute_memory_updates_flags() {
        let (mut c, mut r) = machine(&[0xC1B9, 0x0000, 0x2014]);
        c.d[0] = 0x00FF_00FF;
        c.sr = CCR_X | CCR_N | CCR_V | CCR_C;
        r.write32(0x2014, 0xF00F_F00F).unwrap();

        run(&mut c, &mut r, 1);

        assert_eq!(r.read32(0x2014).unwrap(), 0x000F_000F);
        assert_eq!(c.sr, CCR_X);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn or_long_data_register_to_absolute_memory_updates_flags() {
        let mut r = FlatRam::new(0x8000_0000, 0x1_0000);
        r.load(0x8000_1000, &[0x81, 0xB9, 0x80, 0x00, 0x84, 0x04]);
        let mut c = Cpu::new();
        c.pc = 0x8000_1000;
        c.d[0] = 1;
        c.sr = CCR_X | CCR_V | CCR_C;
        r.write32(0x8000_8404, 0x8000_0000).unwrap();

        run(&mut c, &mut r, 1);

        assert_eq!(r.read32(0x8000_8404).unwrap(), 0x8000_0001);
        assert_eq!(c.sr, CCR_X | CCR_N);
        assert_eq!(c.pc, 0x8000_1006);
    }

    #[test]
    fn or_long_absolute_memory_to_data_register_updates_flags() {
        let (mut c, mut r) = machine(&[0x80B9, 0x0000, 0x2014]);
        c.d[0] = 1;
        c.sr = CCR_X | CCR_V | CCR_C;
        r.write32(0x2014, 0x8000_0000).unwrap();

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[0], 0x8000_0001);
        assert_eq!(c.sr, CCR_X | CCR_N);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn eor_long_data_register_to_data_register_updates_flags() {
        let (mut c, mut r) = machine(&[0xB782]);
        c.d[2] = 0x8000_0001;
        c.d[3] = 1;
        c.sr = CCR_X | CCR_N | CCR_V | CCR_C;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[2], 0x8000_0000);
        assert_eq!(c.d[3], 1);
        assert_eq!(c.sr, CCR_X | CCR_N);
    }

    #[test]
    fn negx_long_data_register_propagates_extend_and_cumulative_zero() {
        let (mut cpu, mut ram) = machine(&[0x4080]);
        cpu.d[0] = 0;
        cpu.sr = CCR_X | CCR_Z | CCR_V;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[0], u32::MAX);
        assert_eq!(cpu.sr, CCR_X | CCR_N | CCR_C);
        assert_eq!(cpu.pc, 0x1002);

        let (mut cpu, mut ram) = machine(&[0x4080]);
        cpu.d[0] = 0;
        cpu.sr = CCR_Z | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[0], 0);
        assert_eq!(cpu.sr, CCR_Z);
    }

    #[test]
    fn addx_subx_clear_extend_ignores_x_and_sets_zero_only_if_previously_set() {
        for (op, a, b, expected) in [(0xD182u16, 5u32, 3u32, 8u32), (0x9182, 5, 3, 2)] {
            let (mut cpu, mut ram) = machine(&[op]);
            cpu.d[0] = a;
            cpu.d[2] = b;
            cpu.sr = 0;
            run(&mut cpu, &mut ram, 1);
            assert_eq!((cpu.d[0], cpu.sr), (expected, 0));

            let (mut cpu, mut ram) = machine(&[op]);
            cpu.d[0] = a;
            cpu.d[2] = b;
            cpu.sr = CCR_X;
            run(&mut cpu, &mut ram, 1);
            assert_eq!(cpu.d[0], if op == 0xD182 { 9 } else { 1 });
            assert_eq!(cpu.sr & CCR_X, 0);
        }
    }

    #[test]
    fn addx_subx_overflow_uses_extend_input() {
        for (op, extend, destination, source, expected, flags) in [
            (0xD181u16, 0, 0x7FFF_FFFF, 0, 0x7FFF_FFFF, 0),
            (0xD181, CCR_X, 0x7FFF_FFFF, 0, 0x8000_0000, CCR_N | CCR_V),
            (0x9181, 0, 0x8000_0000, 0, 0x8000_0000, CCR_N),
            (0x9181, CCR_X, 0x8000_0000, 0, 0x7FFF_FFFF, CCR_V),
        ] {
            let (mut cpu, mut ram) = machine(&[op]);
            cpu.d[0] = destination;
            cpu.d[1] = source;
            cpu.sr = extend;

            run(&mut cpu, &mut ram, 1);

            assert_eq!(cpu.d[0], expected, "op={op:#06x} X={extend:#04x}");
            assert_eq!(cpu.sr, flags, "op={op:#06x} X={extend:#04x}");
        }
    }

    #[test]
    fn negx_clear_extend_negates_and_set_extend_borrows() {
        let (mut cpu, mut ram) = machine(&[0x4080]);
        cpu.d[0] = 1;
        cpu.sr = 0;
        run(&mut cpu, &mut ram, 1);
        assert_eq!(cpu.d[0], u32::MAX);
        assert_eq!(cpu.sr, CCR_N | CCR_C | CCR_X);

        let (mut cpu, mut ram) = machine(&[0x4080]);
        cpu.d[0] = 1;
        cpu.sr = CCR_X;
        run(&mut cpu, &mut ram, 1);
        assert_eq!(cpu.d[0], u32::MAX - 1);
        assert_eq!(cpu.sr, CCR_N | CCR_C | CCR_X);
    }

    #[test]
    fn unsupported_extend_sizes_memory_forms_and_rotates_are_illegal() {
        // Only ADDX/SUBX.L Dn,Dm and NEGX.L Dn are supported by this core.
        for op in [
            0xD102u16, 0xD142, 0x9102, 0x9142, 0xD188, 0x9188, 0x4000, 0x4040, 0x4090, 0xC180,
            0x8180, 0xE190, 0xE090, 0xE198, 0xE098, 0xE1B0, 0xE0B0,
        ] {
            for x in [0, CCR_X] {
                let (mut cpu, mut ram) = machine(&[op]);
                cpu.sr = x;
                assert_eq!(
                    cpu.step(&mut ram),
                    Err(CpuError::Illegal { pc: 0x1000, op }),
                    "op {op:#06x} x={x}"
                );
            }
        }
    }

    #[test]
    fn byte_a7_predecrement_and_postincrement_step_by_one() {
        let (mut c, mut r) = machine(&[0x4227, 0x101F]);
        r.write8(0x1FFF, 0x5A).unwrap();
        run(&mut c, &mut r, 1);
        assert_eq!(c.a[7], 0x1FFF);
        assert_eq!(r.read8(0x1FFF).unwrap(), 0);
        r.write8(0x1FFF, 0x5A).unwrap();
        run(&mut c, &mut r, 1);
        assert_eq!(c.a[7], 0x2000);
        assert_eq!(c.d[0] & 0xFF, 0x5A);
    }

    #[test]
    fn ori_long_immediate_updates_data_register_and_flags() {
        let (mut c, mut r) = machine(&[0x0080, 0x0000, 0x1800]);
        c.d[0] = 0x0000_C000;
        c.sr = CCR_X | CCR_N | CCR_V | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_D800);
        assert_eq!(c.sr, CCR_X);
    }

    #[test]
    fn movec_data_register_to_rambar() {
        let (mut c, mut r) = machine(&[0x4E7B, 0x0C05]);
        c.d[0] = 0x8000_0235;
        run(&mut c, &mut r, 1);
        assert_eq!(c.rambar, 0x8000_0235);
        assert_eq!(c.pc, 0x1004);
    }

    #[test]
    fn movec_data_register_to_rgpiobar_keeps_fixed_base_and_access_bits() {
        let (mut c, mut r) = machine(&[0x4E7B, 0x0009]);
        c.d[0] = 0x8C00_0035;
        c.sr = 0x2000;

        run(&mut c, &mut r, 1);

        assert_eq!(c.rgpiobar, RGPIOBAR_RESET | 1);
        assert_eq!(c.pc, 0x1004);
    }

    #[test]
    fn ff1_long_counts_leading_zeroes_and_sets_documented_flags() {
        let (mut cpu, mut ram) = machine(&[0x04C1]);
        cpu.d[1] = 0x0010_0000;
        cpu.sr = CCR_X | CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[1], 11);
        assert_eq!(cpu.sr, CCR_X);
        assert_eq!(cpu.pc, 0x1002);

        let (mut cpu, mut ram) = machine(&[0x04C1]);
        cpu.d[1] = 0;
        cpu.sr = CCR_X | CCR_N | CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[1], 32);
        assert_eq!(cpu.sr, CCR_X | CCR_Z);

        let (mut cpu, mut ram) = machine(&[0x04C1]);
        cpu.d[1] = 0x8000_0000;
        cpu.sr = CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[1], 0);
        assert_eq!(cpu.sr, CCR_N);
    }

    #[test]
    fn long_multiply_signed_and_unsigned_forms_update_result_and_flags() {
        let (mut cpu, mut ram) = machine(&[0x4C00, 0x2800]);
        cpu.d[0] = 3;
        cpu.d[2] = (-7i32) as u32;
        cpu.sr = CCR_X | CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[2], (-21i32) as u32);
        assert_eq!(cpu.sr, CCR_X | CCR_N);
        assert_eq!(cpu.pc, 0x1004);

        let (mut cpu, mut ram) = machine(&[0x4C00, 0x2000]);
        cpu.d[0] = 3;
        cpu.d[2] = u32::MAX;
        cpu.sr = CCR_X | CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[2], 0xFFFF_FFFD);
        assert_eq!(cpu.sr, CCR_X | CCR_N);
        assert_eq!(cpu.pc, 0x1004);
    }

    #[test]
    fn muls_word_immediate_sign_extends_both_operands() {
        let (mut cpu, mut ram) = machine(&[0xC3FC, 0x2AAB]);
        cpu.d[1] = 0x1234_FFFD;
        cpu.sr = CCR_X | CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[1], 0xFFFF_7FFF);
        assert_eq!(cpu.sr, CCR_X | CCR_N);
        assert_eq!(cpu.pc, 0x1004);
    }

    #[test]
    fn mulu_word_immediate_zero_extends_both_operands() {
        let (mut cpu, mut ram) = machine(&[0xC2FC, 0x1556]);
        cpu.d[1] = 0x1234_FFFD;
        cpu.sr = CCR_X | CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[1], 0xFFFD * 0x1556);
        assert_eq!(cpu.sr, CCR_X);
        assert_eq!(cpu.pc, 0x1004);
    }

    #[test]
    fn emac_register_moves_save_restore_and_clear() {
        // move.l d1,acc1; movclr.l acc1,d2; movclr.l acc1,d3; move.l macsr,a0; move.l #0x20,macsr; move.l macsr,d4
        let (mut cpu, mut ram) = machine(&[0xA301, 0xA3C2, 0xA3C3, 0xA988, 0xA93C, 0x0000, 0x0020, 0xA984]);
        cpu.d[1] = 0x8000_0001;
        cpu.d[3] = 0xFFFF_FFFF;

        run(&mut cpu, &mut ram, 6);

        assert_eq!(cpu.d[2], 0x8000_0001);
        assert_eq!(cpu.d[3], 0, "the accumulator was cleared by the first MOVCLR");
        assert_eq!(cpu.a[0], 0x8, "N from the load, PAV and the other flags clear");
        assert_eq!(cpu.d[4], 0x20);
        assert_eq!(cpu.pc, 0x1010);
    }

    #[test]
    fn sats_long_saturates_only_when_v_is_set() {
        let (mut cpu, mut ram) = machine(&[0x4C84, 0x4C85]);
        cpu.d[4] = 0x8000_0001;
        cpu.d[5] = 0x7FFF_FFFF;
        cpu.sr = CCR_V;
        run(&mut cpu, &mut ram, 1);
        assert_eq!(cpu.d[4], 0x7FFF_FFFF);
        assert_eq!(cpu.sr, 0);
        cpu.sr = 0;
        run(&mut cpu, &mut ram, 1);
        assert_eq!(cpu.d[5], 0x7FFF_FFFF, "unchanged without V");
    }

    #[test]
    fn eori_long_immediate_toggles_bits_and_sets_nz() {
        let (mut cpu, mut ram) = machine(&[0x0A81, 0xFFFF, 0xFFFF]);
        cpu.d[1] = 0x8000_0000;
        cpu.sr = CCR_V | CCR_C;
        run(&mut cpu, &mut ram, 1);
        assert_eq!(cpu.d[1], 0x7FFF_FFFF);
        assert_eq!(cpu.sr, 0);
        assert_eq!(cpu.pc, 0x1006);
    }

    #[test]
    fn divs_long_register_form_updates_quotient_and_flags() {
        let (mut cpu, mut ram) = machine(&[0x4C40, 0x2802]);
        cpu.d[0] = 0x0002_625A;
        cpu.d[2] = 0x07DE_2900;
        cpu.sr = CCR_X | CCR_V | CCR_C;

        run(&mut cpu, &mut ram, 1);

        assert_eq!(cpu.d[2], 0x0000_034C);
        assert_eq!(cpu.sr, CCR_X);
        assert_eq!(cpu.pc, 0x1004);
    }

    #[test]
    fn div_and_rem_long_forms_select_signedness_and_destination() {
        // (ext, dividend D4, divisor D0, expect D4, expect D1)
        let cases: [(u16, u32, u32, u32, u32); 5] = [
            (0x4804, (-7i32) as u32, 2, (-3i32) as u32, 0x1111_1111),
            (0x4004, 0xFFFF_FFF9, 2, 0x7FFF_FFFC, 0x1111_1111),
            (0x4801, (-7i32) as u32, 2, (-7i32) as u32, (-1i32) as u32),
            (0x4001, 0xFFFF_FFF9, 4, 0xFFFF_FFF9, 1),
            (0x4801, 7, (-2i32) as u32, 7, 1),
        ];
        for (ext, dividend, divisor, d4, d1) in cases {
            let (mut c, mut r) = machine(&[0x4C40, ext]);
            c.d[4] = dividend;
            c.d[0] = divisor;
            c.d[1] = 0x1111_1111;
            run(&mut c, &mut r, 1);
            assert_eq!((c.d[4], c.d[1]), (d4, d1), "ext {ext:#06x}");
            assert_eq!(c.pc, 0x1004);
        }
    }

    #[test]
    fn div_long_with_same_register_keeps_quotient_and_ignores_remainder() {
        for (dividend, divisor, quotient) in [(7i32, 2i32, 3i32), (-7, 2, -3), (7, -2, -3)] {
            let (mut c, mut r) = machine(&[0x4C40, 0x4804]);
            c.d[4] = dividend as u32;
            c.d[0] = divisor as u32;
            c.d[1] = 0x1111_1111;
            run(&mut c, &mut r, 1);
            assert_eq!((c.d[4], c.d[1]), (quotient as u32, 0x1111_1111));
        }
    }

    #[test]
    fn rem_long_sets_flags_from_remainder() {
        let (mut c, mut r) = machine(&[0x4C40, 0x4801]);
        c.d[4] = (-7i32) as u32;
        c.d[0] = 2;
        c.sr = CCR_X | CCR_V | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.sr, CCR_X | CCR_N);
    }

    #[test]
    fn div_long_rejects_64_bit_form_and_zero_divisor_stops() {
        let (mut c, mut r) = machine(&[0x4C40, 0x4C04]);
        assert!(matches!(c.step(&mut r), Err(CpuError::Illegal { .. })));
        let (mut c, mut r) = machine(&[0x4C40, 0x4804]);
        c.d[4] = 5;
        assert!(matches!(
            c.step(&mut r),
            Err(CpuError::UnsupportedExceptionState { .. })
        ));
    }

    #[test]
    fn cpushl_is_a_privileged_no_op_for_each_cache_selection() {
        for op in [0xF468u16, 0xF4A9, 0xF4EF] {
            let (mut c, mut r) = machine(&[op]);
            c.sr = 0x2000;
            let before = c.d;
            run(&mut c, &mut r, 1);
            assert_eq!((c.pc, c.d), (0x1002, before));
            let (mut c, mut r) = machine(&[op]);
            c.sr = 0;
            assert_eq!(c.step(&mut r), Err(CpuError::Privilege { pc: 0x1000, op }));
        }
    }

    #[test]
    fn movec_data_register_to_vbr() {
        let (mut c, mut r) = machine(&[0x4E7B, 0x0801]);
        c.d[0] = 0x8000_0000;
        run(&mut c, &mut r, 1);
        assert_eq!(c.vbr, 0x8000_0000);
    }

    #[test]
    fn movec_data_register_to_cacr() {
        let (mut c, mut r) = machine(&[0x4E7B, 0x0002]);
        c.d[0] = 0x000C_C100;
        run(&mut c, &mut r, 1);
        assert_eq!(c.cacr, 0x000C_C100);
    }

    #[test]
    fn movec_data_register_to_acr0() {
        let (mut c, mut r) = machine(&[0x4E7B, 0x0004]);
        c.d[0] = 0x4007_E020;
        run(&mut c, &mut r, 1);
        assert_eq!(c.acr0, 0x4007_E020);
    }

    #[test]
    fn movec_data_register_to_acr1() {
        let (mut c, mut r) = machine(&[0x4E7B, 0x0005]);
        c.d[0] = 0x0000_E020;
        run(&mut c, &mut r, 1);
        assert_eq!(c.acr1, 0x0000_E020);
    }

    #[test]
    fn ext_long_sign_extends_word_and_updates_flags() {
        let (mut c, mut r) = machine(&[0x48C0]);
        c.d[0] = 0xABCD_8001;
        c.sr = CCR_X | CCR_V | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0xFFFF_8001);
        assert_eq!(c.sr, CCR_X | CCR_N);
    }

    #[test]
    fn ext_word_sign_extends_byte_and_preserves_upper_word() {
        let (mut c, mut r) = machine(&[0x4883]);
        c.d[3] = 0x1234_0080;
        c.sr = CCR_X | CCR_V | CCR_C;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[3], 0x1234_FF80);
        assert_eq!(c.sr, CCR_X | CCR_N);
    }

    #[test]
    fn not_long_data_register_inverts_value_and_updates_flags() {
        let (mut c, mut r) = machine(&[0x4680]);
        c.d[0] = 0x1234_5678;
        c.sr = CCR_X | CCR_N | CCR_V | CCR_C;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[0], 0xEDCB_A987);
        assert_eq!(c.sr, CCR_X | CCR_N);
    }

    #[test]
    fn or_long_data_register_updates_destination_and_flags() {
        let (mut c, mut r) = machine(&[0x8081]);
        c.d[0] = 0x0000_0100;
        c.d[1] = 0x0000_0008;
        c.sr = CCR_X | CCR_N | CCR_V | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_0108);
        assert_eq!(c.d[1], 0x0000_0008);
        assert_eq!(c.sr, CCR_X);
    }

    #[test]
    fn and_long_data_register_updates_destination_and_flags() {
        let (mut c, mut r) = machine(&[0xC081]);
        c.d[0] = 0x0000_0055;
        c.d[1] = 0xFFFF_FFF3;
        c.sr = CCR_X | CCR_N | CCR_V | CCR_C;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_0051);
        assert_eq!(c.d[1], 0xFFFF_FFF3);
        assert_eq!(c.sr, CCR_X);
    }

    #[test]
    fn link_word_saves_frame_pointer_and_allocates_stack() {
        let (mut c, mut r) = machine(&[0x4E56, 0xFFF0]);
        c.a[6] = 0x1234_5678;
        c.sr = CCR_X | CCR_Z;
        run(&mut c, &mut r, 1);
        assert_eq!(c.a[6], 0x1FFC);
        assert_eq!(c.a[7], 0x1FEC);
        assert_eq!(r.read32(0x1FFC).unwrap(), 0x1234_5678);
        assert_eq!(c.sr, CCR_X | CCR_Z);
    }

    #[test]
    fn unlk_restores_stack_and_frame_pointer_without_touching_flags() {
        let (mut c, mut r) = machine(&[0x4E56, 0xFFF0, 0x4E5E]);
        c.a[6] = 0x1234_5678;
        c.sr = CCR_X | CCR_Z;
        run(&mut c, &mut r, 2);
        assert_eq!(c.a[6], 0x1234_5678);
        assert_eq!(c.a[7], 0x2000);
        assert_eq!(c.sr, CCR_X | CCR_Z);
    }

    #[test]
    fn unlk_a7_loads_the_stack_value_into_a7() {
        let (mut c, mut r) = machine(&[0x4E5F]);
        r.load(0x2000, &0x1234u32.to_be_bytes());
        run(&mut c, &mut r, 1);
        assert_eq!(c.a[7], 0x1234);
    }

    #[test]
    fn pea_pc_relative_pushes_effective_address() {
        let (mut c, mut r) = machine(&[0x487A, 0x0010]);
        run(&mut c, &mut r, 1);
        assert_eq!(c.a[7], 0x1FFC);
        assert_eq!(r.read32(0x1FFC).unwrap(), 0x1012);
    }

    #[test]
    fn move_immediate_to_sr_loads_status_word() {
        let (mut c, mut r) = machine(&[0x46FC, 0x2500]);
        c.sr = 0x2700;
        run(&mut c, &mut r, 1);
        assert_eq!(c.sr, 0x2500);
        assert_eq!(c.pc, 0x1004);
    }

    #[test]
    fn move_byte_to_ccr_replaces_condition_codes_only() {
        let (mut c, mut r) = machine(&[0x44C0]);
        c.d[0] = 0xABCD_001B;
        c.sr = 0x2500 | CCR_Z;
        run(&mut c, &mut r, 1);
        assert_eq!(c.sr, 0x251B);
    }

    #[test]
    fn move_sr_to_data_register_preserves_upper_word_and_status() {
        let (mut c, mut r) = machine(&[0x40C2]);
        c.d[2] = 0x1234_5678;
        c.sr = 0x2704;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[2], 0x1234_2704);
        assert_eq!(c.sr, 0x2704);
    }

    #[test]
    fn move_data_register_to_sr_loads_status_word() {
        let (mut c, mut r) = machine(&[0x46C2]);
        c.sr = 0x2700;
        c.d[2] = 0x1234_2004;

        run(&mut c, &mut r, 1);

        assert_eq!(c.sr, 0x2004);
    }

    #[test]
    fn move_data_register_to_sr_is_privileged() {
        let (mut c, mut r) = machine(&[0x46C2]);
        c.d[2] = 0x2004;
        assert_eq!(
            c.step(&mut r),
            Err(CpuError::Privilege {
                pc: 0x1000,
                op: 0x46C2
            })
        );
    }

    #[test]
    fn trap_dispatches_with_aligned_format_vector_frame_and_rte_restores_state() {
        let (mut c, mut r) = machine(&[0x4E40]);
        c.vbr = 0x1200;
        c.a[7] = 0x2003;
        c.sr = 0xA004;
        r.write32(0x1280, 0x1500).unwrap();
        r.load(0x1500, &[0x4E, 0x73]);

        run(&mut c, &mut r, 1);

        assert_eq!(c.pc, 0x1500);
        assert_eq!(c.a[7], 0x1FF8);
        assert_eq!(c.sr, 0x2004);
        assert_eq!(r.read16(0x1FF8).unwrap(), 0x7020);
        assert_eq!(r.read16(0x1FFA).unwrap(), 0xA004);
        assert_eq!(r.read32(0x1FFC).unwrap(), 0x1002);

        run(&mut c, &mut r, 1);

        assert_eq!(c.pc, 0x1002);
        assert_eq!(c.a[7], 0x2003);
        assert_eq!(c.sr, 0xA004);
    }

    #[test]
    fn user_mode_trap_reports_missing_supervisor_stack_model() {
        let (mut c, mut r) = machine(&[0x4E40]);
        c.sr = 0;
        assert_eq!(
            c.step(&mut r),
            Err(CpuError::UnsupportedExceptionState {
                pc: 0x1000,
                op: 0x4E40
            })
        );
    }

    #[test]
    fn wdebug_executes_wdmreg_command_to_csr() {
        let (mut c, mut r) = machine(&[0xFBEF, 0x0003, 0xFFF8]);
        c.sr = 0x2700;
        r.load(0x1FF8, &[0x2C, 0x80, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00]);
        run(&mut c, &mut r, 1);
        assert_eq!(c.debug_csr, 0x0002_0000);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn wdebug_requires_supervisor_state() {
        let (mut c, mut r) = machine(&[0xFBEF, 0x0003, 0xFFF8]);
        assert_eq!(
            c.step(&mut r),
            Err(CpuError::Privilege {
                pc: 0x1000,
                op: 0xFBEF,
            })
        );
    }

    #[test]
    fn wdebug_reports_unimplemented_debug_register() {
        let (mut c, mut r) = machine(&[0xFBEF, 0x0003, 0xFFF8]);
        c.sr = 0x2700;
        r.load(0x1FF8, &[0x2C, 0x88, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(
            c.step(&mut r),
            Err(CpuError::UnsupportedDebugRegister {
                pc: 0x1000,
                register: 8,
            })
        );
    }

    #[test]
    fn movem_long_register_list_stores_in_register_order() {
        let (mut c, mut r) = machine(&[0x48D0, 0x00F0]);
        c.a[0] = 0x1100;
        c.d[4..8].copy_from_slice(&[0x4444_4444, 0x5555_5555, 0x6666_6666, 0x7777_7777]);
        run(&mut c, &mut r, 1);
        for (offset, expected) in [0x4444_4444, 0x5555_5555, 0x6666_6666, 0x7777_7777]
            .into_iter()
            .enumerate()
        {
            assert_eq!(r.read32(0x1100 + offset as u32 * 4).unwrap(), expected);
        }
        assert_eq!(c.a[0], 0x1100);
    }

    #[test]
    fn movem_long_displacement_restores_registers_including_base_register() {
        let (mut c, mut r) = machine(&[0x4CE8, 0xFFFF, 0x000C]);
        let expected: Vec<u32> = (0..16).map(|index| 0xA000_0000 + index).collect();
        for (index, value) in expected.iter().copied().enumerate() {
            r.write32(0x110C + index as u32 * 4, value).unwrap();
        }
        c.a[0] = 0x1100;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d.as_slice(), &expected[..8]);
        assert_eq!(c.a.as_slice(), &expected[8..]);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn movem_long_register_list_stores_to_displacement_address() {
        let (mut c, mut r) = machine(&[0x48E8, 0xFFFF, 0x000C]);
        c.d.copy_from_slice(&[
            0xD000_0000,
            0xD111_1111,
            0xD222_2222,
            0xD333_3333,
            0xD444_4444,
            0xD555_5555,
            0xD666_6666,
            0xD777_7777,
        ]);
        c.a.copy_from_slice(&[
            0x1100,
            0xA111_1111,
            0xA222_2222,
            0xA333_3333,
            0xA444_4444,
            0xA555_5555,
            0xA666_6666,
            0xA777_7777,
        ]);

        run(&mut c, &mut r, 1);

        for (index, expected) in c.d.iter().chain(c.a.iter()).copied().enumerate() {
            assert_eq!(r.read32(0x110C + index as u32 * 4).unwrap(), expected);
        }
        assert_eq!(c.a[0], 0x1100);
        assert_eq!(c.pc, 0x1006);
    }

    #[test]
    fn movem_long_indirect_restores_registers_without_changing_base() {
        let (mut c, mut r) = machine(&[0x4CD7, 0x00F0]);
        c.a[7] = 0x2000;
        r.load(
            0x2000,
            &[
                0x44, 0x44, 0x44, 0x44, 0x55, 0x55, 0x55, 0x55, 0x66, 0x66, 0x66, 0x66, 0x77, 0x77,
                0x77, 0x77,
            ],
        );
        run(&mut c, &mut r, 1);
        assert_eq!(
            &c.d[4..8],
            &[0x4444_4444, 0x5555_5555, 0x6666_6666, 0x7777_7777]
        );
        assert_eq!(c.a[7], 0x2000);
    }

    #[test]
    fn cmpi_long_immediate_leaves_destination_and_sets_zero() {
        let (mut c, mut r) = machine(&[0x0C80, 0x0000, 0x0400]);
        c.d[0] = 0x400;
        c.sr = CCR_X | CCR_N;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x400);
        assert_eq!(c.sr, CCR_X | CCR_Z);
    }

    #[test]
    fn cmpi_long_immediate_sets_borrow_without_changing_extend() {
        let (mut c, mut r) = machine(&[0x0C80, 0x0000, 0x0400]);
        c.d[0] = 0x3FC;
        c.sr = CCR_X;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x3FC);
        assert_eq!(c.sr, CCR_N | CCR_C | CCR_X);
    }

    #[test]
    fn scc_sets_only_the_low_byte_from_the_condition_and_keeps_flags() {
        let (mut c, mut r) = machine(&[0x57C0]);
        c.d[0] = 0x1234_5600;
        c.sr = CCR_Z | CCR_X;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x1234_56FF);
        assert_eq!(c.sr & 0x1F, CCR_Z | CCR_X);

        let (mut c, mut r) = machine(&[0x57C1]);
        c.d[1] = 0xFFFF_FFFF;
        c.sr = CCR_N;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[1], 0xFFFF_FF00);
        assert_eq!(c.sr & 0x1F, CCR_N);
    }

    #[test]
    fn scc_true_and_false_conditions_are_unconditional() {
        let (mut c, mut r) = machine(&[0x50C2, 0x51C3]);
        c.d[3] = 0xFF;
        run(&mut c, &mut r, 2);
        assert_eq!(c.d[2] & 0xFF, 0xFF);
        assert_eq!(c.d[3] & 0xFF, 0);
    }

    #[test]
    fn scc_other_destinations_are_not_decoded() {
        let (mut c, mut r) = machine(&[0x57D0]);
        assert!(matches!(
            c.run(&mut r, 1).stop,
            RunStop::Error(CpuError::Illegal { .. })
        ));
    }

    #[test]
    fn swap_exchanges_register_halves_and_sets_flags_from_the_result() {
        let (mut c, mut r) = machine(&[0x4841]);
        c.d[1] = 0x1234_8000;
        c.sr = CCR_V | CCR_C | CCR_X;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[1], 0x8000_1234);
        assert_eq!(
            c.sr & (CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X),
            CCR_N | CCR_X
        );

        let (mut c, mut r) = machine(&[0x4840]);
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0);
        assert_eq!(c.sr & (CCR_N | CCR_Z), CCR_Z);
    }

    #[test]
    fn pea_still_decodes_beside_swap() {
        let (mut c, mut r) = machine(&[0x4850]);
        c.a[0] = 0x1234;
        run(&mut c, &mut r, 1);
        assert_eq!(c.a[7], 0x2000 - 4);
        assert_eq!(r.read32(c.a[7]).unwrap(), 0x1234);
    }

    #[test]
    fn mvz_byte_from_displacement_zero_fills_and_clears_flags() {
        let (mut c, mut r) = machine(&[0x71AE, 0x0004]);
        c.a[6] = 0x1000;
        c.d[0] = 0xFFFF_FFFF;
        c.sr = CCR_N | CCR_V | CCR_C | CCR_X;
        r.load(0x1004, &[0x9C]);
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_009C);
        assert_eq!(c.sr & (CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X), CCR_X);
    }

    #[test]
    fn mvz_word_from_data_register_zero_fills_and_keeps_extend() {
        let (mut c, mut r) = machine(&[0x71C1]);
        c.d[1] = 0xABCD_0000;
        c.d[0] = 0xFFFF_FFFF;
        c.sr = CCR_X;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0);
        assert_eq!(
            c.sr & (CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X),
            CCR_Z | CCR_X
        );

        let (mut c, mut r) = machine(&[0x71C1]);
        c.d[1] = 0xABCD_8001;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_8001);
        assert_eq!(c.sr & (CCR_N | CCR_Z), 0);
    }

    #[test]
    fn mvz_byte_zero_result_sets_z() {
        let (mut c, mut r) = machine(&[0x7000 | 0x0100 | 0x0080 | 0x0001]);
        c.d[1] = 0xFFFF_FF00;
        c.d[0] = 0x1234_5678;
        c.sr = CCR_X;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0);
        assert_eq!(
            c.sr & (CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X),
            CCR_Z | CCR_X
        );
    }

    #[test]
    fn mvs_byte_sign_extends_and_sets_negative() {
        let (mut c, mut r) = machine(&[0x7100]);
        c.d[0] = 0x1234_5680;
        c.sr = CCR_V | CCR_C | CCR_X;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0xFFFF_FF80);
        assert_eq!(
            c.sr & (CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X),
            CCR_N | CCR_X
        );
    }

    #[test]
    fn mvs_word_from_memory_sign_extends_or_keeps_positive_values() {
        let (mut c, mut r) = machine(&[0x7358, 0x7358]);
        c.a[0] = 0x1800;
        c.d[1] = 0xFFFF_FFFF;
        r.load(0x1800, &[0x7F, 0xFF, 0x80, 0x00]);
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[1], 0x0000_7FFF);
        assert_eq!(c.a[0], 0x1802);
        assert_eq!(c.sr & (CCR_N | CCR_Z), 0);
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[1], 0xFFFF_8000);
        assert_eq!(c.sr & CCR_N, CCR_N);
    }

    #[test]
    fn mvs_zero_result_sets_z() {
        let (mut c, mut r) = machine(&[0x7101]);
        c.d[0] = 0x55;
        c.d[1] = 0xFFFF_FF00;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0);
        assert_eq!(c.sr & (CCR_N | CCR_Z), CCR_Z);
    }

    #[test]
    fn lsr_long_immediate_shifts_in_zeros_and_sets_carry_and_extend() {
        let (mut c, mut r) = machine(&[0xE888]);
        c.d[0] = 0xC202_0220;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0C20_2022);
        assert_eq!(c.sr & (CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X), 0);

        let (mut c, mut r) = machine(&[0xE889]);
        c.d[1] = 0x8000_0018;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[1], 0x0800_0001);
        assert_eq!(c.sr & (CCR_N | CCR_C | CCR_X), CCR_C | CCR_X);
    }

    #[test]
    fn lsr_long_immediate_sets_zero_and_encodes_eight_as_zero() {
        let (mut c, mut r) = machine(&[0xE088]);
        c.d[0] = 0x0000_00FF;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0);
        assert_eq!(c.sr & (CCR_Z | CCR_C | CCR_X), CCR_Z | CCR_C | CCR_X);
    }

    #[test]
    fn lsl_long_immediate_shifts_left_and_reports_the_last_bit_out() {
        let (mut c, mut r) = machine(&[0xE988]);
        c.d[0] = 0x1000_0001;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_0010);
        assert_eq!(c.sr & (CCR_N | CCR_Z | CCR_C | CCR_X), CCR_C | CCR_X);

        let (mut c, mut r) = machine(&[0xE188]);
        c.d[0] = 0x0100_0001;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0x0000_0100);
        assert_eq!(c.sr & (CCR_N | CCR_Z | CCR_C | CCR_X), CCR_C | CCR_X);
    }

    #[test]
    fn lsl_long_register_count_uses_low_six_bits_of_count_register() {
        let (mut c, mut r) = machine(&[0xE3A8]);
        c.d[0] = 1;
        c.d[1] = 17;
        c.sr = CCR_X | CCR_V | CCR_C;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[0], 0x0002_0000);
        assert_eq!(c.sr & (CCR_X | CCR_N | CCR_Z | CCR_V | CCR_C), 0);
    }

    #[test]
    fn lsr_long_register_count_sets_carry_and_extend() {
        let (mut c, mut r) = machine(&[0xE8A8]);
        c.d[0] = 0x8000_0001;
        c.d[4] = 1;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[0], 0x4000_0000);
        assert_eq!(
            c.sr & (CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X),
            CCR_C | CCR_X
        );
    }

    #[test]
    fn register_shift_count_zero_preserves_extend_and_clears_carry() {
        let (mut c, mut r) = machine(&[0xE3A8]);
        c.d[0] = 0x8000_0001;
        c.d[1] = 0;
        c.sr = CCR_X | CCR_V | CCR_C;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[0], 0x8000_0001);
        assert_eq!(
            c.sr & (CCR_X | CCR_N | CCR_Z | CCR_V | CCR_C),
            CCR_X | CCR_N
        );
    }

    #[test]
    fn register_lsl_count_32_reports_last_original_bit() {
        let (mut c, mut r) = machine(&[0xE3A8]);
        c.d[0] = 0x8000_0001;
        c.d[1] = 32;

        run(&mut c, &mut r, 1);

        assert_eq!(c.d[0], 0);
        assert_eq!(c.sr & (CCR_X | CCR_Z | CCR_C), CCR_X | CCR_Z | CCR_C);
    }

    #[test]
    fn shifts_other_than_immediate_long_asr_lsr_lsl_are_not_decoded() {
        for op in [0xE898u16, 0xE848] {
            let (mut c, mut r) = machine(&[op]);
            assert!(matches!(
                c.run(&mut r, 1).stop,
                RunStop::Error(CpuError::Illegal { .. })
            ));
        }
    }

    #[test]
    fn asl_long_flags_x_c_v_n_z() {
        const ALL: u16 = CCR_X | CCR_N | CCR_Z | CCR_V | CCR_C;
        // (opcode, D0, D1 count, initial SR, result, expected CCR)
        let cases: [(u16, u32, u32, u16, u32, u16); 9] = [
            (0xE380, 0x1000_0001, 0, ALL, 0x2000_0002, 0),
            (0xE380, 0x4000_0000, 0, 0, 0x8000_0000, CCR_N | CCR_V),
            (0xE380, 0x8000_0000, 0, 0, 0, CCR_X | CCR_C | CCR_Z | CCR_V),
            (0xE380, 0xC000_0000, 0, 0, 0x8000_0000, CCR_X | CCR_C | CCR_N),
            (0xE180, 0x00FF_0000, 0, 0, 0xFF00_0000, CCR_N | CCR_V),
            (0xE180, 0xFF00_0000, 0, 0, 0, CCR_X | CCR_C | CCR_Z | CCR_V),
            (0xE3A0, 0x4000_0000, 0, CCR_X | CCR_V | CCR_C, 0x4000_0000, CCR_X),
            (0xE3A0, 0x0000_0001, 32, 0, 0, CCR_X | CCR_C | CCR_Z | CCR_V),
            (0xE3A0, 0xFFFF_FFFF, 40, CCR_X, 0, CCR_Z | CCR_V),
        ];
        for (op, d0, d1, sr, result, ccr) in cases {
            let (mut c, mut r) = machine(&[op]);
            c.d[0] = d0;
            c.d[1] = d1;
            c.sr = sr;
            run(&mut c, &mut r, 1);
            assert_eq!(c.d[0], result, "op {op:#06x} d0={d0:#x} count={d1}");
            assert_eq!(c.sr & ALL, ccr, "op {op:#06x} d0={d0:#x} count={d1}");
        }
    }

    #[test]
    fn asl_long_zero_result_without_sign_change_has_no_overflow() {
        let (mut c, mut r) = machine(&[0xE380]);
        c.d[0] = 0;
        c.sr = CCR_X | CCR_V;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0);
        assert_eq!(c.sr & (CCR_X | CCR_Z | CCR_V | CCR_C), CCR_Z);
    }

    #[test]
    fn asr_long_immediate_scales_copy_length() {
        let (mut c, mut r) = machine(&[0xE881]);
        c.d[1] = 0x1A70;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[1], 0x01A7);
        assert_eq!(c.sr & (CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X), 0);
    }

    #[test]
    fn asr_long_immediate_sign_extends_and_sets_carry_and_extend() {
        let (mut c, mut r) = machine(&[0xE880]);
        c.d[0] = 0x8000_0008;
        run(&mut c, &mut r, 1);
        assert_eq!(c.d[0], 0xF800_0000);
        assert_eq!(
            c.sr & (CCR_N | CCR_Z | CCR_V | CCR_C | CCR_X),
            CCR_N | CCR_C | CCR_X
        );
    }
}
