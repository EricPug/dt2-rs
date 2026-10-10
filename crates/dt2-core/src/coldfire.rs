//! ColdFire V4e core. Written from the ColdFire Family Programmer's Reference Manual.
//!
//! Only a subset of the instruction set is decoded so far; anything else returns
//! [`CpuError::Illegal`], which at this stage means "not implemented" as much as "illegal".

use crate::bus::{Bus, BusError};

pub const CCR_C: u16 = 0x01;
pub const CCR_V: u16 = 0x02;
pub const CCR_Z: u16 = 0x04;
pub const CCR_N: u16 = 0x08;
pub const CCR_X: u16 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuError {
    Bus(BusError),
    Illegal { pc: u32, op: u16 },
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
    pub vbr: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStop {
    InstructionLimit,
    Error(CpuError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunReport {
    pub instructions: usize,
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

#[derive(Clone, Default)]
pub struct Cpu {
    pub d: [u32; 8],
    pub a: [u32; 8],
    pub pc: u32,
    pub sr: u16,
    pub rambar: u32,
    pub vbr: u32,
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    fn fetch16<B: Bus>(&mut self, bus: &mut B) -> Result<u16, CpuError> {
        let v = bus.read16(self.pc)?;
        self.pc = self.pc.wrapping_add(2);
        Ok(v)
    }

    fn fetch32<B: Bus>(&mut self, bus: &mut B) -> Result<u32, CpuError> {
        let v = bus.read32(self.pc)?;
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
        let step = if size == Size::B && reg == 7 {
            2
        } else {
            size.bytes()
        };
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

    /// Execute one instruction.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> Result<(), CpuError> {
        let pc0 = self.pc;
        let op = self.fetch16(bus)?;
        let ill = CpuError::Illegal { pc: pc0, op };
        let (mode, reg) = ((op >> 3) & 7, (op & 7) as usize);
        match op >> 12 {
            0 => {
                if op & 0xFFC0 == 0x08C0 && mode == 0 {
                    let bit = (self.fetch16(bus)? & 31) as u32;
                    let mask = 1 << bit;
                    let old = self.d[reg];
                    self.d[reg] = old | mask;
                    self.set_ccr(CCR_Z, if old & mask == 0 { CCR_Z } else { 0 });
                } else if op & 0xFFC0 == 0x0800 && mode == 0 {
                    let bit = (self.fetch16(bus)? & 31) as u32;
                    let mask = 1 << bit;
                    self.set_ccr(CCR_Z, if self.d[reg] & mask == 0 { CCR_Z } else { 0 });
                } else if op & 0xFFC0 == 0x0880 && mode == 0 {
                    let bit = (self.fetch16(bus)? & 31) as u32;
                    let mask = 1 << bit;
                    let old = self.d[reg];
                    self.d[reg] = old & !mask;
                    self.set_ccr(CCR_Z, if old & mask == 0 { CCR_Z } else { 0 });
                } else if op & 0xFFF8 == 0x0C80 {
                    let immediate = self.fetch32(bus)?;
                    self.sub(self.d[reg], immediate, Size::L, false);
                } else if op & 0xFFF8 == 0x0280 || op & 0xFFF8 == 0x0080 {
                    let immediate = self.fetch32(bus)?;
                    let result = if op & 0xFFF8 == 0x0280 {
                        self.d[reg] & immediate
                    } else {
                        self.d[reg] | immediate
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
                if op & 0xFFC0 == 0x48C0 && mode == 2 {
                    let mask = self.fetch16(bus)?;
                    let mut address = self.a[reg];
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
                } else if op & 0xFFC0 == 0x4CC0 && mode == 2 {
                    let mask = self.fetch16(bus)?;
                    let mut address = self.a[reg];
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
                        0x0801 => self.vbr = value,
                        0x0C05 => self.rambar = value,
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
                    }
                } else {
                    return Err(ill);
                }
            }
            5 => {
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
                    self.push32(bus, self.pc)?;
                    self.pc = base.wrapping_add(disp);
                } else if self.cond(c) {
                    self.pc = base.wrapping_add(disp);
                }
            }
            0xE => {
                if op & 0xF1F8 != 0xE080 {
                    return Err(ill);
                }
                let encoded_count = ((op >> 9) & 7) as u32;
                let count = if encoded_count == 0 { 8 } else { encoded_count };
                let original = self.d[reg];
                let result = ((original as i32) >> count) as u32;
                let carry = original & (1 << (count - 1)) != 0;
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
                self.set_ccr(CCR_X | CCR_N | CCR_Z | CCR_V | CCR_C, flags);
            }
            7 => {
                if op & 0x0100 != 0 {
                    return Err(ill);
                }
                let v = op as u8 as i8 as i32 as u32;
                self.d[((op >> 9) & 7) as usize] = v;
                self.flags_move(v, Size::L);
            }
            nib @ (9 | 0xB | 0xD) => {
                let r = ((op >> 9) & 7) as usize;
                let opmode = (op >> 6) & 7;
                let (size, to_ea) = match (nib, opmode) {
                    (0xB, 0) => (Size::B, false),
                    (0xB, 1) => (Size::W, false),
                    (_, 2) => (Size::L, false),
                    (9 | 0xD, 6) => (Size::L, true),
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
                    let res = if nib == 0xD {
                        self.add(ev, dv, size, true)
                    } else {
                        self.sub(ev, dv, size, true)
                    };
                    self.write(bus, o, size, res, pc0, op)?;
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
        let mut instructions = 0;
        let stop = loop {
            if instructions == instruction_limit {
                break RunStop::InstructionLimit;
            }
            match self.step(bus) {
                Ok(()) => instructions += 1,
                Err(error) => break RunStop::Error(error),
            }
        };
        RunReport {
            instructions,
            cpu: CpuSnapshot {
                d: self.d,
                a: self.a,
                pc: self.pc,
                sr: self.sr,
                rambar: self.rambar,
                vbr: self.vbr,
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
    fn movec_data_register_to_vbr() {
        let (mut c, mut r) = machine(&[0x4E7B, 0x0801]);
        c.d[0] = 0x8000_0000;
        run(&mut c, &mut r, 1);
        assert_eq!(c.vbr, 0x8000_0000);
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
