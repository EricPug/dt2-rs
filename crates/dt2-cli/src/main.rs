use dt2_core::{
    bus::{Bus, BusError, FlatRam},
    coldfire::{Cpu, CpuError, RunStop},
};
use dt2_fw::{container, syx};
use std::collections::BTreeMap;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "run-bin") {
        run_binary(&args[1..]);
        return;
    }

    let path = args.first().unwrap_or_else(|| {
        eprintln!("usage: dt2-cli <firmware.syx> [outdir]");
        eprintln!("       dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc> <instruction-limit> [D0=value A0=value SR=value MMIO16@address=value MMIO32@address=value ...]");
        std::process::exit(2)
    });
    let file = std::fs::read(path).expect("cannot read file");
    let s = syx::decode(&file).expect("not a firmware SysEx file");
    println!(
        "{} messages, counter starts at {}, stream {} bytes",
        s.messages,
        s.first_counter,
        s.stream.len()
    );
    let c = container::parse(&s.stream).expect("not an ELE3 container");
    println!(
        "container version {:?}, {} sections",
        c.version,
        c.sections.len()
    );
    for sec in &c.sections {
        let res = c.unpack(sec);
        if let (Some(dir), Ok(v)) = (args.get(1), &res) {
            std::fs::create_dir_all(dir).expect("cannot create output folder");
            std::fs::write(format!("{dir}/section_{}.bin", sec.id), v)
                .expect("cannot write section");
        }
        println!(
            "  id {} dest {:#010x} stored {:>8} bytes {} -> {}",
            sec.id,
            sec.dest,
            sec.stored.len(),
            if sec.packed { "packed" } else { "raw   " },
            match &res {
                Ok(v) => format!("{} bytes", v.len()),
                Err(e) => format!("ERROR {e:?}"),
            }
        );
    }
}

fn run_binary(args: &[String]) {
    if args.len() < 6 {
        eprintln!("usage: dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc> <instruction-limit> [D0=value A0=value SR=value MMIO16@address=value MMIO32@address=value ...]");
        std::process::exit(2);
    }
    let ram_base = parse_address(&args[1]).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    let ram_size = parse_address(&args[2]).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    }) as usize;
    let image_address = parse_address(&args[3]).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    let entry_pc = parse_address(&args[4]).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    let instruction_limit = args[5].parse::<usize>().unwrap_or_else(|_| {
        eprintln!("invalid instruction limit: {}", args[5]);
        std::process::exit(2)
    });
    let bytes = std::fs::read(&args[0]).unwrap_or_else(|error| {
        eprintln!("cannot read {}: {error}", args[0]);
        std::process::exit(2)
    });
    if !range_fits(ram_base, ram_size, image_address, bytes.len()) {
        eprintln!(
            "image range {image_address:#010x}..+{:#x} does not fit RAM {ram_base:#010x}..+{ram_size:#x}",
            bytes.len()
        );
        std::process::exit(2);
    }
    let mut ram = FlatRam::new(ram_base, ram_size);
    ram.load(image_address, &bytes);
    let mut bus = ProbeBus::new(ram);
    let mut cpu = Cpu::new();
    cpu.pc = entry_pc;
    for assignment in &args[6..] {
        let result = if let Some(mmio) = assignment.strip_prefix("MMIO16@") {
            bus.seed_mmio16(mmio)
        } else if let Some(mmio) = assignment.strip_prefix("MMIO32@") {
            bus.seed_mmio32(mmio)
        } else {
            set_register(&mut cpu, assignment)
        };
        result.unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2)
        });
    }
    let report = cpu.run(&mut bus, instruction_limit);

    println!("RAM base={ram_base:#010x} size={ram_size:#x}; image loaded at {image_address:#010x}");
    println!("unprovided initial registers are zero");
    if args.len() > 6 {
        println!("initial overrides: {}", args[6..].join(" "));
    }
    println!("executed {} instructions", report.instructions);
    print_stop(report.stop);
    println!(
        "PC={:#010x} SR={:#06x} RAMBAR={:#010x} VBR={:#010x}",
        report.cpu.pc, report.cpu.sr, report.cpu.rambar, report.cpu.vbr
    );
    for register in 0..8 {
        println!(
            "D{register}={:#010x} A{register}={:#010x}",
            report.cpu.d[register], report.cpu.a[register]
        );
    }
    if matches!(report.stop, RunStop::Error(_)) {
        std::process::exit(1);
    }
}

fn parse_address(value: &str) -> Result<u32, String> {
    let parsed = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16)
    } else {
        value.parse()
    };
    parsed.map_err(|_| format!("invalid 32-bit address: {value}"))
}

fn range_fits(base: u32, size: usize, address: u32, length: usize) -> bool {
    let base = u64::from(base);
    let end = base.checked_add(size as u64);
    let address = u64::from(address);
    end.is_some_and(|end| {
        address >= base
            && address
                .checked_add(length as u64)
                .is_some_and(|image_end| image_end <= end)
    })
}

fn set_register(cpu: &mut Cpu, assignment: &str) -> Result<(), String> {
    let (register, raw_value) = assignment
        .split_once('=')
        .ok_or_else(|| format!("invalid register assignment: {assignment}"))?;
    let value = parse_address(raw_value)?;
    if register == "SR" {
        cpu.sr = u16::try_from(value).map_err(|_| "SR value exceeds 16 bits".to_string())?;
        return Ok(());
    }
    if let Some(index) = register.strip_prefix('D') {
        let index = parse_register_index(index, register)?;
        cpu.d[index] = value;
        return Ok(());
    }
    if let Some(index) = register.strip_prefix('A') {
        let index = parse_register_index(index, register)?;
        cpu.a[index] = value;
        return Ok(());
    }
    Err(format!("unknown register: {register}"))
}

fn parse_register_index(index: &str, register: &str) -> Result<usize, String> {
    let index = index
        .parse::<usize>()
        .map_err(|_| format!("invalid register: {register}"))?;
    if index < 8 {
        Ok(index)
    } else {
        Err(format!("register index out of range: {register}"))
    }
}

struct ProbeBus {
    ram: FlatRam,
    mmio: BTreeMap<u32, u8>,
}

impl ProbeBus {
    fn new(ram: FlatRam) -> Self {
        Self {
            ram,
            mmio: BTreeMap::new(),
        }
    }

    fn seed_mmio16(&mut self, assignment: &str) -> Result<(), String> {
        let (address, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("invalid MMIO seed: MMIO16@{assignment}"))?;
        let address = parse_address(address)?;
        if address & 1 != 0 {
            return Err(format!("16-bit MMIO address must be even: {address:#010x}"));
        }
        let value = u16::try_from(parse_address(value)?)
            .map_err(|_| format!("16-bit MMIO value exceeds 16 bits: {value}"))?;
        let [high, low] = value.to_be_bytes();
        self.mmio.insert(address, high);
        self.mmio.insert(address + 1, low);
        Ok(())
    }

    fn seed_mmio32(&mut self, assignment: &str) -> Result<(), String> {
        let (address, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("invalid MMIO seed: MMIO32@{assignment}"))?;
        let address = parse_address(address)?;
        if address & 3 != 0 {
            return Err(format!(
                "32-bit MMIO address must be aligned: {address:#010x}"
            ));
        }
        for (offset, byte) in parse_address(value)?.to_be_bytes().into_iter().enumerate() {
            self.mmio.insert(address + offset as u32, byte);
        }
        Ok(())
    }
}

impl Bus for ProbeBus {
    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        if let Some(value) = self.mmio.get(&address) {
            Ok(*value)
        } else {
            self.ram.read8(address)
        }
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        let bytes = [self.read8(address)?, self.read8(address + 1)?];
        Ok(u16::from_be_bytes(bytes))
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        let bytes = [
            self.read8(address)?,
            self.read8(address + 1)?,
            self.read8(address + 2)?,
            self.read8(address + 3)?,
        ];
        Ok(u32::from_be_bytes(bytes))
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        if let Some(mapped) = self.mmio.get_mut(&address) {
            *mapped = value;
            Ok(())
        } else {
            self.ram.write8(address, value)
        }
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write8(address + offset as u32, byte)?;
        }
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write8(address + offset as u32, byte)?;
        }
        Ok(())
    }
}

fn print_stop(stop: RunStop) {
    match stop {
        RunStop::InstructionLimit => println!("stop: instruction limit"),
        RunStop::Error(CpuError::Illegal { pc, op }) => {
            println!("stop: unsupported opcode {op:#06x} at {pc:#010x}");
        }
        RunStop::Error(CpuError::Bus(BusError { addr, write })) => {
            let access = if write { "write" } else { "read" };
            println!("stop: unmapped bus {access} at {addr:#010x}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_overrides_set_cpu_state() {
        let mut cpu = Cpu::new();
        set_register(&mut cpu, "D3=0x12345678").unwrap();
        set_register(&mut cpu, "A7=0x8001fffc").unwrap();
        set_register(&mut cpu, "SR=0x2000").unwrap();
        assert_eq!(cpu.d[3], 0x1234_5678);
        assert_eq!(cpu.a[7], 0x8001_FFFC);
        assert_eq!(cpu.sr, 0x2000);
    }

    #[test]
    fn register_overrides_reject_invalid_values() {
        let mut cpu = Cpu::new();
        assert!(set_register(&mut cpu, "A8=0").is_err());
        assert!(set_register(&mut cpu, "SR=0x10000").is_err());
        assert!(set_register(&mut cpu, "PC=0").is_err());
    }

    #[test]
    fn mmio_seed_uses_big_endian_16_bit_value() {
        let ram = FlatRam::new(0, 16);
        let mut bus = ProbeBus::new(ram);
        bus.seed_mmio16("0x1000=0x187D").unwrap();
        assert_eq!(bus.read16(0x1000).unwrap(), 0x187D);
        bus.write16(0x1000, 0xA55A).unwrap();
        assert_eq!(bus.read16(0x1000).unwrap(), 0xA55A);
    }

    #[test]
    fn mmio_seed_rejects_unaligned_address_and_wide_value() {
        let ram = FlatRam::new(0, 16);
        let mut bus = ProbeBus::new(ram);
        assert!(bus.seed_mmio16("0x1001=0x1234").is_err());
        assert!(bus.seed_mmio16("0x1000=0x123456").is_err());
    }

    #[test]
    fn mmio32_seed_uses_big_endian_value_and_accepts_writes() {
        let ram = FlatRam::new(0, 16);
        let mut bus = ProbeBus::new(ram);
        bus.seed_mmio32("0x1000=0x12345678").unwrap();
        assert_eq!(bus.read32(0x1000).unwrap(), 0x1234_5678);
        bus.write32(0x1000, 0xAABB_CCDD).unwrap();
        assert_eq!(bus.read32(0x1000).unwrap(), 0xAABB_CCDD);
    }

    #[test]
    fn mmio32_seed_rejects_unaligned_address() {
        let ram = FlatRam::new(0, 16);
        let mut bus = ProbeBus::new(ram);
        assert!(bus.seed_mmio32("0x1002=0").is_err());
    }

    #[test]
    fn image_range_may_start_inside_ram_but_must_fit() {
        assert!(range_fits(0x8000_0000, 0x20000, 0x8000_03FC, 0x765E));
        assert!(!range_fits(0x8000_0000, 0x1000, 0x8000_0FFC, 8));
        assert!(!range_fits(0x8000_1000, 0x1000, 0x8000_0FFC, 4));
    }
}
