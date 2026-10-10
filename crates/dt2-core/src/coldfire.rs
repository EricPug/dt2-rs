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
                if op == 0x4E71 {
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
}
