use dt2_core::{
    bus::{Bus, BusError, FlatRam, Mcf5441xSram},
    coldfire::{Cpu, CpuError, RunStop},
    ddr::DdrController,
    dspi::Dspi,
    spi_nor::W25q128jv,
};
use dt2_fw::{container, syx};
use std::collections::{BTreeMap, BTreeSet};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "run-bin") {
        run_binary(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "render-pattern") {
        render_pattern(&args[1..]);
        return;
    }

    let path = args.first().unwrap_or_else(|| {
        eprintln!("usage: dt2-cli <firmware.syx> [outdir]");
        eprintln!("       dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc|SBF_RESET_VECTOR> <instruction-limit> [MCF5441X_SRAM|MCF5441X_SBF_BOOT MCF5441X_SRAM_OVERLAY DDR_BOOTSTRAP SYNTHETIC_MCF_MMIO INSPECT_TRAP0 D0=value A0=value SR=value RAM32@address=value MMIO8@address=value MMIO16@address=value MMIO32@address=value ...]");
        eprintln!("       dt2-cli render-pattern <output.ppm>");
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

const PATTERN_WIDTH: usize = 320;
const PATTERN_HEIGHT: usize = 240;
const COLOR_BARS: [[u8; 3]; 8] = [
    [255, 255, 255],
    [255, 255, 0],
    [0, 255, 255],
    [0, 255, 0],
    [255, 0, 255],
    [255, 0, 0],
    [0, 0, 255],
    [0, 0, 0],
];

fn render_pattern(args: &[String]) {
    if args.len() != 1 {
        eprintln!("usage: dt2-cli render-pattern <output.ppm>");
        std::process::exit(2);
    }
    let image = diagnostic_pattern_ppm();
    std::fs::write(&args[0], image).unwrap_or_else(|error| {
        eprintln!("cannot write {}: {error}", args[0]);
        std::process::exit(2)
    });
    println!(
        "wrote diagnostic PPM {}x{} to {}",
        PATTERN_WIDTH, PATTERN_HEIGHT, args[0]
    );
}

fn diagnostic_pattern_ppm() -> Vec<u8> {
    let header = format!("P6\n{PATTERN_WIDTH} {PATTERN_HEIGHT}\n255\n");
    let mut image = Vec::with_capacity(header.len() + PATTERN_WIDTH * PATTERN_HEIGHT * 3);
    image.extend_from_slice(header.as_bytes());
    let bars_height = PATTERN_HEIGHT * 2 / 3;
    for y in 0..PATTERN_HEIGHT {
        for x in 0..PATTERN_WIDTH {
            let color = if y < bars_height {
                COLOR_BARS[x * COLOR_BARS.len() / PATTERN_WIDTH]
            } else {
                let gray = (x * 255 / (PATTERN_WIDTH - 1)) as u8;
                [gray, gray, gray]
            };
            image.extend_from_slice(&color);
        }
    }
    image
}

fn run_binary(args: &[String]) {
    if args.len() < 6 {
        eprintln!("usage: dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc|SBF_RESET_VECTOR> <instruction-limit> [MCF5441X_SRAM|MCF5441X_SBF_BOOT MCF5441X_SRAM_OVERLAY DDR_BOOTSTRAP SYNTHETIC_MCF_MMIO INSPECT_TRAP0 D0=value A0=value SR=value RAM32@address=value MMIO8@address=value MMIO16@address=value MMIO32@address=value ...]");
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
    let mcf_sram_overlay = args[6..]
        .iter()
        .any(|assignment| assignment == "MCF5441X_SRAM_OVERLAY");
    let synthetic_mcf_mmio = args[6..]
        .iter()
        .any(|assignment| assignment == "SYNTHETIC_MCF_MMIO");
    let inspect_trap0 = args[6..]
        .iter()
        .any(|assignment| assignment == "INSPECT_TRAP0");
    if reset_from_sbf_vector && !mcf_sbf_boot {
        eprintln!("SBF_RESET_VECTOR requires MCF5441X_SBF_BOOT");
        std::process::exit(2);
    }
    if mcf_sram_overlay && mcf_sram_alias {
        eprintln!(
            "MCF5441X_SRAM_OVERLAY cannot be combined with MCF5441X_SRAM or MCF5441X_SBF_BOOT"
        );
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
    if mcf_sram_overlay {
        bus.attach_mcf_sram_overlay();
    }
    if synthetic_mcf_mmio {
        bus.enable_synthetic_mcf_mmio();
    }
    bus.load_ram(image_address, &bytes).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    let mut cpu = Cpu::new();
    let mut cpu_overrides = Vec::new();
    for assignment in &args[6..] {
        if assignment == "MCF5441X_SRAM"
            || assignment == "MCF5441X_SBF_BOOT"
            || assignment == "MCF5441X_SRAM_OVERLAY"
            || assignment == "SYNTHETIC_MCF_MMIO"
            || assignment == "INSPECT_TRAP0"
        {
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
    if mcf_sram_overlay {
        println!("SRAM overlay: MCF5441x 64 KiB aliased window at 0x80000000..0x90000000");
    }
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
    if inspect_trap0 {
        let vector_address = trap_vector_address(report.cpu.vbr, 0);
        match bus.read32(vector_address) {
            Ok(handler) => println!(
                "TRAP #0 inspection: VBR={:#010x} vector={vector_address:#010x} handler={handler:#010x} post-run A7={:#010x}",
                report.cpu.vbr, report.cpu.a[7]
            ),
            Err(error) => println!(
                "TRAP #0 inspection: VBR={:#010x} vector={vector_address:#010x} unmapped read at {:#010x}; post-run A7={:#010x}",
                report.cpu.vbr, error.addr, report.cpu.a[7]
            ),
        }
    }
    bus.print_access_trace();
    print_stop(report.stop);
    println!(
        "PC={:#010x} SR={:#06x} RAMBAR={:#010x} ACR0={:#010x} VBR={:#010x} CACR={:#010x} DEBUG_CSR={:#010x}",
        report.cpu.pc,
        report.cpu.sr,
        report.cpu.rambar,
        report.cpu.acr0,
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

fn trap_vector_address(vbr: u32, trap_number: u32) -> u32 {
    vbr.wrapping_add(0x80)
        .wrapping_add(trap_number.wrapping_mul(4))
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
    sram_overlay: Option<Mcf5441xSram>,
    mmio: BTreeMap<u32, u8>,
    dspi0: Option<Dspi>,
    ddr: Option<DdrController>,
    synthetic_mcf_mmio: bool,
    access_events: Vec<AccessEvent>,
    access_addresses: BTreeSet<u32>,
    suppressed_access_events: usize,
}

const DSPI0_BASE: u32 = 0xFC05_C000;
const MCF_SRAM_ALIAS_WINDOW: (u32, u32) = (0x8000_0000, 0x9000_0000);
const MCF_PERIPHERAL_WINDOWS: [(u32, u32); 2] =
    [(0xEC00_0000, 0xED00_0000), (0xFC00_0000, 0xFD00_0000)];
const DDR_REGISTER_APERTURE: (u32, u32) = (0xFC0B_8000, 0xFC0B_8200);
const MAX_ACCESS_EVENTS: usize = 256;
const MAX_ACCESS_ADDRESSES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessSource {
    Modeled,
    Seeded,
    Synthetic,
    Ram,
    Mixed,
}

impl AccessSource {
    fn label(self) -> &'static str {
        match self {
            Self::Modeled => "modeled",
            Self::Seeded => "seeded",
            Self::Synthetic => "synthetic",
            Self::Ram => "RAM",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AccessEvent {
    address: u32,
    width: u8,
    write: bool,
    value: u32,
    source: AccessSource,
}

impl ProbeBus {
    fn new(ram: impl Bus + 'static) -> Self {
        Self {
            ram: Box::new(ram),
            sram_overlay: None,
            mmio: BTreeMap::new(),
            dspi0: None,
            ddr: None,
            synthetic_mcf_mmio: false,
            access_events: Vec::new(),
            access_addresses: BTreeSet::new(),
            suppressed_access_events: 0,
        }
    }

    fn enable_synthetic_mcf_mmio(&mut self) {
        self.synthetic_mcf_mmio = true;
    }

    fn attach_mcf_sram_overlay(&mut self) {
        self.sram_overlay = Some(Mcf5441xSram::new());
    }

    fn sram_for(&mut self, address: u32) -> Option<&mut Mcf5441xSram> {
        self.sram_overlay
            .as_mut()
            .filter(|_| (MCF_SRAM_ALIAS_WINDOW.0..MCF_SRAM_ALIAS_WINDOW.1).contains(&address))
    }

    fn synthetic_access(&self, address: u32) -> bool {
        self.synthetic_mcf_mmio
            && MCF_PERIPHERAL_WINDOWS
                .iter()
                .any(|&(start, end)| (start..end).contains(&address))
            && !(DDR_REGISTER_APERTURE.0..DDR_REGISTER_APERTURE.1).contains(&address)
    }

    fn source_for_byte(&self, address: u32) -> AccessSource {
        if self
            .dspi0
            .as_ref()
            .is_some_and(|dspi| dspi.contains(address))
            || self
                .ddr
                .as_ref()
                .is_some_and(|_| DdrController::contains(address))
        {
            AccessSource::Modeled
        } else if self.mmio.contains_key(&address) {
            AccessSource::Seeded
        } else if self.synthetic_access(address) {
            AccessSource::Synthetic
        } else {
            AccessSource::Ram
        }
    }

    fn source_for_access(&self, address: u32, width_bytes: u8) -> AccessSource {
        let mut source = None;
        for offset in 0..u32::from(width_bytes) {
            let byte_source = self.source_for_byte(address.wrapping_add(offset));
            if source.is_some_and(|first| first != byte_source) {
                return AccessSource::Mixed;
            }
            source = Some(byte_source);
        }
        source.unwrap_or(AccessSource::Ram)
    }

    fn record_access(&mut self, event: AccessEvent) {
        if !self.synthetic_mcf_mmio
            || !MCF_PERIPHERAL_WINDOWS
                .iter()
                .any(|&(start, end)| (start..end).contains(&event.address))
        {
            return;
        }
        let is_new_address = !self.access_addresses.contains(&event.address);
        if self.access_events.len() >= MAX_ACCESS_EVENTS
            || (is_new_address && self.access_addresses.len() >= MAX_ACCESS_ADDRESSES)
        {
            self.suppressed_access_events = self.suppressed_access_events.saturating_add(1);
            return;
        }
        self.access_addresses.insert(event.address);
        self.access_events.push(event);
    }

    fn print_access_trace(&self) {
        if !self.synthetic_mcf_mmio {
            return;
        }
        println!("MMIO diagnostic: synthetic responses are approximations, not hardware behavior");
        for event in &self.access_events {
            let operation = if event.write { "write" } else { "read" };
            let arrow = if event.write { "<-" } else { "->" };
            println!(
                "  {operation}{} {:#010x} {arrow} {:#010x} [{}]",
                event.width,
                event.address,
                event.value,
                event.source.label()
            );
        }
        println!(
            "MMIO trace: {} events, {} addresses, {} suppressed",
            self.access_events.len(),
            self.access_addresses.len(),
            self.suppressed_access_events
        );
    }

    fn read8_inner(&mut self, address: u32) -> Result<u8, BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read8(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read8(address);
        }
        if let Some(value) = self.mmio.get(&address) {
            return Ok(*value);
        }
        if let Some(sram) = self.sram_for(address) {
            return sram.read8(address);
        }
        if self.synthetic_mcf_mmio
            && (DDR_REGISTER_APERTURE.0..DDR_REGISTER_APERTURE.1).contains(&address)
        {
            return Err(BusError {
                addr: address,
                write: false,
            });
        }
        if self.synthetic_access(address) {
            Ok(0)
        } else {
            self.ram.read8(address)
        }
    }

    fn read16_inner(&mut self, address: u32) -> Result<u16, BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read16(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read16(address);
        }
        let bytes = [
            self.read8_inner(address)?,
            self.read8_inner(address.wrapping_add(1))?,
        ];
        Ok(u16::from_be_bytes(bytes))
    }

    fn read32_inner(&mut self, address: u32) -> Result<u32, BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read32(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read32(address);
        }
        let bytes = [
            self.read8_inner(address)?,
            self.read8_inner(address.wrapping_add(1))?,
            self.read8_inner(address.wrapping_add(2))?,
            self.read8_inner(address.wrapping_add(3))?,
        ];
        Ok(u32::from_be_bytes(bytes))
    }

    fn write8_inner(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write8(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write8(address, value);
        }
        if let Some(mapped) = self.mmio.get_mut(&address) {
            *mapped = value;
            return Ok(());
        }
        if let Some(sram) = self.sram_for(address) {
            return sram.write8(address, value);
        }
        if self.synthetic_mcf_mmio
            && (DDR_REGISTER_APERTURE.0..DDR_REGISTER_APERTURE.1).contains(&address)
        {
            return Err(BusError {
                addr: address,
                write: true,
            });
        }
        if self.synthetic_access(address) {
            Ok(())
        } else {
            self.ram.write8(address, value)
        }
    }

    fn write16_inner(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write16(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write16(address, value);
        }
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write8_inner(address.wrapping_add(offset as u32), byte)?;
        }
        Ok(())
    }

    fn write32_inner(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write32(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write32(address, value);
        }
        for (offset, byte) in value.to_be_bytes().into_iter().enumerate() {
            self.write8_inner(address.wrapping_add(offset as u32), byte)?;
        }
        Ok(())
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
        let value = parse_address(value)?;
        let result = if let Some(sram) = self.sram_for(address) {
            sram.write32(address, value)
        } else {
            self.ram.write32(address, value)
        };
        result.map_err(|error| format!("RAM seed outside mapped memory at {:#010x}", error.addr))
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
        let source = self.source_for_access(address, 1);
        match self.read8_inner(address) {
            Ok(value) => {
                self.record_access(AccessEvent {
                    address,
                    width: 8,
                    write: false,
                    value: u32::from(value),
                    source,
                });
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }

    fn read16(&mut self, address: u32) -> Result<u16, BusError> {
        let source = self.source_for_access(address, 2);
        match self.read16_inner(address) {
            Ok(value) => {
                self.record_access(AccessEvent {
                    address,
                    width: 16,
                    write: false,
                    value: u32::from(value),
                    source,
                });
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }

    fn read32(&mut self, address: u32) -> Result<u32, BusError> {
        let source = self.source_for_access(address, 4);
        match self.read32_inner(address) {
            Ok(value) => {
                self.record_access(AccessEvent {
                    address,
                    width: 32,
                    write: false,
                    value,
                    source,
                });
                Ok(value)
            }
            Err(error) => Err(error),
        }
    }

    fn write8(&mut self, address: u32, value: u8) -> Result<(), BusError> {
        let source = self.source_for_access(address, 1);
        self.write8_inner(address, value)?;
        self.record_access(AccessEvent {
            address,
            width: 8,
            write: true,
            value: u32::from(value),
            source,
        });
        Ok(())
    }

    fn write16(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        let source = self.source_for_access(address, 2);
        self.write16_inner(address, value)?;
        self.record_access(AccessEvent {
            address,
            width: 16,
            write: true,
            value: u32::from(value),
            source,
        });
        Ok(())
    }

    fn write32(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        let source = self.source_for_access(address, 4);
        self.write32_inner(address, value)?;
        self.record_access(AccessEvent {
            address,
            width: 32,
            write: true,
            value,
            source,
        });
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
        RunStop::Error(CpuError::UnsupportedExceptionState { pc, op }) => {
            println!("stop: unsupported exception stack state at {pc:#010x} (opcode {op:#06x})");
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
    fn diagnostic_pattern_is_a_well_formed_ppm_with_expected_bars() {
        let image = diagnostic_pattern_ppm();
        let header = format!("P6\n{PATTERN_WIDTH} {PATTERN_HEIGHT}\n255\n");
        let pixel_data = &image[header.len()..];

        assert!(image.starts_with(header.as_bytes()));
        assert_eq!(pixel_data.len(), PATTERN_WIDTH * PATTERN_HEIGHT * 3);
        for (index, expected) in COLOR_BARS.iter().enumerate() {
            let x = index * PATTERN_WIDTH / COLOR_BARS.len();
            let offset = (PATTERN_WIDTH + x) * 3;
            assert_eq!(&pixel_data[offset..offset + 3], expected);
        }
        let gray_row = PATTERN_HEIGHT * 2 / 3;
        let first_gray = (gray_row * PATTERN_WIDTH) * 3;
        let last_gray = (gray_row * PATTERN_WIDTH + PATTERN_WIDTH - 1) * 3;
        assert_eq!(&pixel_data[first_gray..first_gray + 3], &[0, 0, 0]);
        assert_eq!(&pixel_data[last_gray..last_gray + 3], &[255, 255, 255]);
    }

    #[test]
    fn trap_vector_address_uses_coldfire_trap_table_offset() {
        assert_eq!(trap_vector_address(0x4000_0000, 0), 0x4000_0080);
        assert_eq!(trap_vector_address(0x4000_0000, 15), 0x4000_00BC);
    }

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
    fn strict_bus_keeps_unmapped_peripheral_accesses_as_errors() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        assert_eq!(
            bus.write32(0xFC04_002D, 0x25),
            Err(BusError {
                addr: 0xFC04_002D,
                write: true
            })
        );
        assert!(bus.access_events.is_empty());
    }

    #[test]
    fn synthetic_peripheral_accesses_are_logged_once_at_original_width() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.enable_synthetic_mcf_mmio();

        bus.write8(0xFC04_002D, 0x11).unwrap();
        assert_eq!(bus.read16(0xFC04_002E).unwrap(), 0);
        bus.write32(0xFC04_002D, 0x25).unwrap();
        assert_eq!(bus.read32(0xFC04_002D).unwrap(), 0);

        assert_eq!(bus.access_events.len(), 4);
        assert_eq!(
            bus.access_events
                .iter()
                .map(|event| event.width)
                .collect::<Vec<_>>(),
            [8, 16, 32, 32]
        );
        assert_eq!(
            bus.access_events
                .iter()
                .map(|event| event.write)
                .collect::<Vec<_>>(),
            [true, false, true, false]
        );
        assert_eq!(bus.access_events[0].value, 0x11);
        assert_eq!(bus.access_events[1].value, 0);
        assert_eq!(bus.access_events[2].value, 0x25);
        assert_eq!(bus.access_events[3].value, 0);
        assert!(bus
            .access_events
            .iter()
            .all(|event| event.source == AccessSource::Synthetic));
    }

    #[test]
    fn seeded_mmio_takes_precedence_over_synthetic_fallback() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.enable_synthetic_mcf_mmio();
        bus.seed_mmio32("0xFC04002C=0x12345678").unwrap();

        assert_eq!(bus.read32(0xFC04_002C).unwrap(), 0x1234_5678);
        bus.write32(0xFC04_002C, 0xAABB_CCDD).unwrap();
        assert_eq!(bus.read32(0xFC04_002C).unwrap(), 0xAABB_CCDD);
        assert!(bus
            .access_events
            .iter()
            .all(|event| event.source == AccessSource::Seeded));
    }

    #[test]
    fn synthetic_fallback_keeps_ddr_model_and_unmapped_ddr_offsets_strict() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.enable_synthetic_mcf_mmio();
        bus.attach_ddr_bootstrap();

        assert_eq!(bus.read32(0xFC0B_8000).unwrap(), 0);
        assert_eq!(bus.access_events[0].source, AccessSource::Modeled);
        assert_eq!(
            bus.read32(0xFC0B_8184),
            Err(BusError {
                addr: 0xFC0B_8184,
                write: false
            })
        );
        assert_eq!(bus.access_events.len(), 1);
        assert!(bus.read8(0xED00_0000).is_err());
    }

    #[test]
    fn access_trace_caps_events_and_unique_addresses() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.enable_synthetic_mcf_mmio();

        for offset in 0..=MAX_ACCESS_ADDRESSES {
            bus.write8(0xFC00_0000 + offset as u32, offset as u8)
                .unwrap();
        }
        for value in 0..=MAX_ACCESS_ADDRESSES {
            bus.write8(0xFC00_0000, value as u8).unwrap();
        }

        assert_eq!(bus.access_events.len(), MAX_ACCESS_EVENTS);
        assert_eq!(bus.access_addresses.len(), MAX_ACCESS_ADDRESSES);
        assert_eq!(bus.suppressed_access_events, 2);
        assert!(bus.write8(0xE000_0000, 0).is_err());
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
    fn mcf_sram_overlay_coexists_with_flat_ddr_and_aliases_seeded_data() {
        let mut bus = ProbeBus::new(FlatRam::new(0x4000_0000, 0x0800_0000));
        bus.attach_mcf_sram_overlay();

        bus.seed_ram32("0x80000000=0x12345678").unwrap();
        bus.write32(0x4000_0000, 0xAABB_CCDD).unwrap();

        assert_eq!(bus.read32(0x8001_0000).unwrap(), 0x1234_5678);
        assert_eq!(bus.read32(0x4000_0000).unwrap(), 0xAABB_CCDD);
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
        bus.enable_synthetic_mcf_mmio();
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
        assert_eq!(bus.access_events.len(), 10);
        assert!(bus
            .access_events
            .iter()
            .all(|event| event.source == AccessSource::Modeled));
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
