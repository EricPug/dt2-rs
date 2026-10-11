use dt2_core::{
    bus::{Bus, BusError, FlatRam, Mcf5441xSram},
    coldfire::{Cpu, CpuError, RunStop},
    ddr::DdrController,
    dspi::Dspi,
    spi_nor::W25q128jv,
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
        eprintln!("       dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc|SBF_RESET_VECTOR> <instruction-limit> [MCF5441X_SRAM|MCF5441X_SBF_BOOT DDR_BOOTSTRAP D0=value A0=value SR=value RAM32@address=value MMIO8@address=value MMIO16@address=value MMIO32@address=value ...]");
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
        eprintln!("usage: dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc|SBF_RESET_VECTOR> <instruction-limit> [MCF5441X_SRAM|MCF5441X_SBF_BOOT DDR_BOOTSTRAP D0=value A0=value SR=value RAM32@address=value MMIO8@address=value MMIO16@address=value MMIO32@address=value ...]");
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
    let reset_from_sbf_vector = args[4] == "SBF_RESET_VECTOR";
    let entry_pc = if reset_from_sbf_vector {
        None
    } else {
        Some(parse_address(&args[4]).unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2)
        }))
    };
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
    let mcf_sbf_boot = args[6..]
        .iter()
        .any(|assignment| assignment == "MCF5441X_SBF_BOOT");
    let mcf_sram_alias = mcf_sbf_boot
        || args[6..]
            .iter()
            .any(|assignment| assignment == "MCF5441X_SRAM");
    if reset_from_sbf_vector && !mcf_sbf_boot {
        eprintln!("SBF_RESET_VECTOR requires MCF5441X_SBF_BOOT");
        std::process::exit(2);
    }
    let mut bus = if mcf_sram_alias {
        if ram_base != 0x8000_0000 || ram_size != 0x1_0000 {
            eprintln!("MCF5441X_SRAM requires RAM base 0x80000000 and size 0x10000");
            std::process::exit(2);
        }
        let sram = Mcf5441xSram::new();
        if mcf_sbf_boot {
            ProbeBus::new(sram.with_sbf_reset_alias())
        } else {
            ProbeBus::new(sram)
        }
    } else {
        ProbeBus::new(FlatRam::new(ram_base, ram_size))
    };
    bus.load_ram(image_address, &bytes).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    let mut cpu = Cpu::new();
    let mut cpu_overrides = Vec::new();
    for assignment in &args[6..] {
        if assignment == "MCF5441X_SRAM" || assignment == "MCF5441X_SBF_BOOT" {
            continue;
        }
        if assignment == "DDR_BOOTSTRAP" || assignment == "DDR_RCR" {
            bus.attach_ddr_bootstrap();
            continue;
        }
        if let Some(flash) = assignment.strip_prefix("DSPI0_FLASH=") {
            bus.attach_dspi0_flash(flash).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(2)
            });
            continue;
        }
        let result = if let Some(ram_seed) = assignment.strip_prefix("RAM32@") {
            bus.seed_ram32(ram_seed)
        } else if let Some(mmio) = assignment.strip_prefix("MMIO8@") {
            bus.seed_mmio8(mmio)
        } else if let Some(mmio) = assignment.strip_prefix("MMIO16@") {
            bus.seed_mmio16(mmio)
        } else if let Some(mmio) = assignment.strip_prefix("MMIO32@") {
            bus.seed_mmio32(mmio)
        } else {
            cpu_overrides.push(assignment.as_str());
            continue;
        };
        result.unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2)
        });
    }
    if reset_from_sbf_vector {
        reset_cpu_from_sbf_vector(&mut cpu, &mut bus).unwrap_or_else(|error| {
            eprintln!("unmapped SBF reset-vector read at {:#010x}", error.addr);
            std::process::exit(2)
        });
    } else {
        cpu.pc = entry_pc.unwrap();
    }
    for assignment in cpu_overrides {
        set_register(&mut cpu, assignment).unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2)
        });
    }
    let report = cpu.run(&mut bus, instruction_limit);

    let ram_map = if mcf_sbf_boot {
        "MCF5441x SRAM with SBF address-zero reset alias"
    } else if mcf_sram_alias {
        "MCF5441x 64 KiB SRAM with aliasing"
    } else {
        "flat RAM"
    };
    println!("RAM map={ram_map}; base={ram_base:#010x} size={ram_size:#x}; image loaded at {image_address:#010x}");
    if reset_from_sbf_vector {
        println!("initial state: A7/PC from SBF reset vector, SR=0x2700; other registers default to zero");
    } else {
        println!("unprovided initial registers are zero");
    }
    if args.len() > 6 {
        println!("initial overrides: {}", args[6..].join(" "));
    }
    println!("executed {} instructions", report.instructions);
    if let Some(behavior) = bus.unsupported_device_behavior() {
        println!("device model: unsupported {behavior}");
    }
    if bus.ddr.is_some() {
        println!("DDR model: bootstrap-observed registers only");
    }
    print_stop(report.stop);
    println!(
        "PC={:#010x} SR={:#06x} RAMBAR={:#010x} VBR={:#010x} CACR={:#010x} DEBUG_CSR={:#010x}",
        report.cpu.pc,
        report.cpu.sr,
        report.cpu.rambar,
        report.cpu.vbr,
        report.cpu.cacr,
        report.cpu.debug_csr
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

fn reset_cpu_from_sbf_vector<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> Result<(), BusError> {
    cpu.a[7] = bus.read32(0)?;
    cpu.pc = bus.read32(4)?;
    cpu.sr = 0x2700;
    Ok(())
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
    ram: Box<dyn Bus>,
    mmio: BTreeMap<u32, u8>,
    dspi0: Option<Dspi>,
    ddr: Option<DdrController>,
}

const DSPI0_BASE: u32 = 0xFC05_C000;

impl ProbeBus {
    fn new(ram: impl Bus + 'static) -> Self {
        Self {
            ram: Box::new(ram),
            mmio: BTreeMap::new(),
            dspi0: None,
            ddr: None,
        }
    }

    /// Spec is `<pcs>:BLANK` or `<pcs>:<image path>`; the chip is a W25Q128JV.
    fn attach_dspi0_flash(&mut self, spec: &str) -> Result<(), String> {
        let (pcs, source) = spec
            .split_once(':')
            .ok_or_else(|| format!("invalid DSPI0_FLASH spec: {spec}"))?;
        let pcs = pcs
            .parse::<usize>()
            .ok()
            .filter(|pcs| *pcs < 4)
            .ok_or_else(|| format!("DSPI0 chip select must be 0-3: {pcs}"))?;
        let flash = if source == "BLANK" {
            W25q128jv::blank()
        } else {
            let image =
                std::fs::read(source).map_err(|error| format!("cannot read {source}: {error}"))?;
            W25q128jv::with_image(&image)?
        };
        self.dspi0
            .get_or_insert_with(|| Dspi::new(DSPI0_BASE))
            .attach(pcs, Box::new(flash));
        Ok(())
    }

    fn dspi_for(&mut self, address: u32) -> Option<&mut Dspi> {
        self.dspi0.as_mut().filter(|dspi| dspi.contains(address))
    }

    fn attach_ddr_bootstrap(&mut self) {
        self.ddr = Some(DdrController::new());
    }

    fn ddr_for(&mut self, address: u32) -> Option<&mut DdrController> {
        self.ddr
            .as_mut()
            .filter(|_| DdrController::contains(address))
    }

    fn unsupported_device_behavior(&self) -> Option<String> {
        self.dspi0.as_ref().and_then(Dspi::unsupported)
    }

    fn load_ram(&mut self, address: u32, bytes: &[u8]) -> Result<(), String> {
        for (offset, byte) in bytes.iter().copied().enumerate() {
            self.ram
                .write8(address.wrapping_add(offset as u32), byte)
                .map_err(|error| format!("image load hit unmapped RAM at {:#010x}", error.addr))?;
        }
        Ok(())
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

    fn seed_mmio8(&mut self, assignment: &str) -> Result<(), String> {
        let (address, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("invalid MMIO seed: MMIO8@{assignment}"))?;
        let address = parse_address(address)?;
        let value = u8::try_from(parse_address(value)?)
            .map_err(|_| format!("8-bit MMIO value exceeds 8 bits: {value}"))?;
        self.mmio.insert(address, value);
        Ok(())
    }

    fn seed_ram32(&mut self, assignment: &str) -> Result<(), String> {
        let (address, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("invalid RAM seed: RAM32@{assignment}"))?;
        let address = parse_address(address)?;
        if address & 3 != 0 {
            return Err(format!(
                "32-bit RAM address must be aligned: {address:#010x}"
            ));
        }
        self.ram
            .write32(address, parse_address(value)?)
            .map_err(|error| format!("RAM seed outside mapped memory at {:#010x}", error.addr))
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
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read8(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read8(address);
        }
        if let Some(value) = self.mmio.get(&address) {
            Ok(*value)
        } else {
            self.ram.read8(address)
        }
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read16(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read16(address);
        }
        let bytes = [self.read8(address)?, self.read8(address + 1)?];
        Ok(u16::from_be_bytes(bytes))
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read32(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read32(address);
        }
        let bytes = [
            self.read8(address)?,
            self.read8(address + 1)?,
            self.read8(address + 2)?,
            self.read8(address + 3)?,
        ];
        Ok(u32::from_be_bytes(bytes))
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write8(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write8(address, value);
        }
        if let Some(mapped) = self.mmio.get_mut(&address) {
            *mapped = value;
            Ok(())
        } else {
            self.ram.write8(address, value)
        }
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write16(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write16(address, value);
        }
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write8(address + offset as u32, byte)?;
        }
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write32(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write32(address, value);
        }
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
        RunStop::Error(CpuError::Privilege { pc, op }) => {
            println!("stop: privilege violation at {pc:#010x} (opcode {op:#06x})");
        }
        RunStop::Error(CpuError::UnsupportedDebugRegister { pc, register }) => {
            println!("stop: unsupported debug register {register:#x} at {pc:#010x}");
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
    fn mmio8_seed_reads_and_writes_one_byte() {
        let ram = FlatRam::new(0, 16);
        let mut bus = ProbeBus::new(ram);
        bus.seed_mmio8("0x1001=0x03").unwrap();
        assert_eq!(bus.read8(0x1001).unwrap(), 0x03);
        bus.write8(0x1001, 0xA5).unwrap();
        assert_eq!(bus.read8(0x1001).unwrap(), 0xA5);
    }

    #[test]
    fn mmio8_seed_rejects_wide_value() {
        let ram = FlatRam::new(0, 16);
        let mut bus = ProbeBus::new(ram);
        assert!(bus.seed_mmio8("0x1000=0x100").is_err());
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

    #[test]
    fn ram_seed_writes_initial_stack_memory() {
        let ram = FlatRam::new(0x8000_0000, 0x20000);
        let mut bus = ProbeBus::new(ram);
        bus.seed_ram32("0x80010004=1").unwrap();
        assert_eq!(bus.read32(0x8001_0004).unwrap(), 1);
        assert!(bus.seed_ram32("0x90000000=1").is_err());
    }

    #[test]
    fn mcf_sram_seed_is_visible_through_its_64k_alias() {
        let mut bus = ProbeBus::new(Mcf5441xSram::new());
        bus.seed_ram32("0x80010004=0x80000de8").unwrap();
        assert_eq!(bus.read32(0x80000004).unwrap(), 0x8000_0DE8);
    }

    #[test]
    fn sbf_reset_vector_seed_is_visible_in_high_sram_alias() {
        let mut bus = ProbeBus::new(Mcf5441xSram::new().with_sbf_reset_alias());
        bus.seed_ram32("0=0x80010000").unwrap();
        bus.seed_ram32("4=0x80000de8").unwrap();
        assert_eq!(bus.read32(0x80000000).unwrap(), 0x8001_0000);
        assert_eq!(bus.read32(0x80010004).unwrap(), 0x8000_0DE8);
    }

    #[test]
    fn dspi0_flash_answers_jedec_id_through_the_bus() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.attach_dspi0_flash("1:BLANK").unwrap();
        bus.write32(DSPI0_BASE, 0x8000_0000).unwrap();
        bus.write32(DSPI0_BASE + 0x0C, 0x3800_0000).unwrap();
        for word in [0x8402_009F, 0x8002_0000, 0x8002_0000, 0x0002_0000] {
            bus.write32(DSPI0_BASE + 0x34, word).unwrap();
        }
        let id: Vec<u32> = (0..4)
            .map(|_| bus.read32(DSPI0_BASE + 0x38).unwrap())
            .collect();
        assert_eq!(id, [0, 0xEF, 0x40, 0x18]);
    }

    #[test]
    fn dspi0_flash_spec_is_validated() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        assert!(bus.attach_dspi0_flash("BLANK").is_err());
        assert!(bus.attach_dspi0_flash("4:BLANK").is_err());
        assert!(bus.attach_dspi0_flash("1:/nonexistent/flash.bin").is_err());
    }

    #[test]
    fn dspi0_registers_fall_back_to_seeds_without_a_flash() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.seed_mmio32("0xFC05C000=0x4001").unwrap();
        assert_eq!(bus.read32(DSPI0_BASE).unwrap(), 0x4001);
    }

    #[test]
    fn ddr_bootstrap_option_handles_only_mapped_registers() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.attach_ddr_bootstrap();
        bus.write32(0xFC0B_8180, 1 << 30).unwrap();
        assert_eq!(bus.read32(0xFC0B_8180).unwrap(), 0);
        assert!(bus.read32(0xFC0B_8184).is_err());
    }

    #[test]
    fn sbf_reset_initializes_stack_pc_and_status_from_reset_vector() {
        let mut bus = ProbeBus::new(Mcf5441xSram::new().with_sbf_reset_alias());
        bus.seed_ram32("0=0x80010000").unwrap();
        bus.seed_ram32("4=0x80000de8").unwrap();
        let mut cpu = Cpu::new();

        reset_cpu_from_sbf_vector(&mut cpu, &mut bus).unwrap();

        assert_eq!(cpu.a[7], 0x8001_0000);
        assert_eq!(cpu.pc, 0x8000_0DE8);
        assert_eq!(cpu.sr, 0x2700);
    }
}
