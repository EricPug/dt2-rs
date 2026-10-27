use dt2_core::{
    bus::{Bus, BusError, FlatRam, InterruptRequest, LoopContext, Mcf5441xSram},
    coldfire::{Cpu, CpuError, InterruptInjection, RunStop},
    ddr::DdrController,
    dspi::Dspi,
    intc2::Intc2,
    pit::PitController,
    spi_nor::W25q128jv,
    uart::Uart8,
};
use dt2_fw::{container, syx};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;

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
    if args.first().is_some_and(|arg| arg == "build-flash") {
        build_flash(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "render-uart-records") {
        render_uart_records(&args[1..]);
        return;
    }

    let path = args.first().unwrap_or_else(|| {
        eprintln!("usage: dt2-cli <firmware.syx> [outdir]");
        eprintln!("       dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc|SBF_RESET_VECTOR> <instruction-limit> [MCF5441X_SRAM|MCF5441X_SBF_BOOT MCF5441X_SRAM_OVERLAY DDR_BOOTSTRAP PIT_BOOTSTRAP INTC2_BOOTSTRAP PIT0_ONESHOT PIT_TICKS_PER_INSTRUCTION=n LOOP_DETECT STOP_PC=pc INJECT_IRQ=count:level:vector UART8_TX SYNTHETIC_MCF_MMIO INSPECT_TRAP0 DUMP_RAM=address:length DUMP_RAM_FILE=address:length:path WATCH_WRITE=address TRACE_REGS=pc TRACE_PC_RANGE=start:end TRACE_A0_RANGE=start:end RAM_REMAP=from:to:length RAM_SNAPSHOT=watch:src:dst:length:trigger_start:trigger_end:read_start:read_end D0=value A0=value SR=value RAM32@address=value MMIO8@address=value MMIO16@address=value MMIO32@address=value ...]");
        eprintln!("       dt2-cli render-pattern <output.ppm>");
        eprintln!("       dt2-cli build-flash <firmware.syx> <output.bin> [container-offset]");
        eprintln!("       dt2-cli render-uart-records <run.log> <output.ppm>");
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

const W25Q128_SIZE: usize = 16 * 1024 * 1024;
const DEFAULT_NOR_CONTAINER_OFFSET: u32 = 0x80000;
const SYSEX_PREAMBLE_SIZE: usize = 8;

fn build_flash(args: &[String]) {
    if !(2..=3).contains(&args.len()) {
        eprintln!("usage: dt2-cli build-flash <firmware.syx> <output.bin> [container-offset]");
        std::process::exit(2);
    }
    let input = std::fs::read(&args[0]).unwrap_or_else(|error| {
        eprintln!("cannot read {}: {error}", args[0]);
        std::process::exit(2)
    });
    let sysex = syx::decode(&input).unwrap_or_else(|error| {
        eprintln!("cannot decode firmware SysEx: {error:?}");
        std::process::exit(2)
    });
    container::parse(&sysex.stream).unwrap_or_else(|error| {
        eprintln!("cannot parse firmware container: {error:?}");
        std::process::exit(2)
    });
    let offset = args
        .get(2)
        .map(|value| parse_address(value))
        .transpose()
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2)
        })
        .unwrap_or(DEFAULT_NOR_CONTAINER_OFFSET);
    let container_bytes = &sysex.stream[SYSEX_PREAMBLE_SIZE..];
    let image = nor_image_with_container(container_bytes, offset).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    std::fs::write(&args[1], image).unwrap_or_else(|error| {
        eprintln!("cannot write {}: {error}", args[1]);
        std::process::exit(2)
    });
    println!(
        "wrote {}-byte W25Q128JV image with {} container bytes at {offset:#x}",
        W25Q128_SIZE,
        container_bytes.len()
    );
}

fn nor_image_with_container(container: &[u8], offset: u32) -> Result<Vec<u8>, String> {
    let start = usize::try_from(offset).map_err(|_| "NOR offset exceeds address space")?;
    let end = start
        .checked_add(container.len())
        .filter(|end| *end <= W25Q128_SIZE)
        .ok_or_else(|| "firmware container does not fit in W25Q128JV image".to_string())?;
    let mut image = vec![0xFF; W25Q128_SIZE];
    image[start..end].copy_from_slice(container);
    Ok(image)
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

const UART_RECORD_HEADER_BYTES: usize = 40;
const UART_RECORD_BYTES: usize = 10;
const UART_RECORD_COUNT: usize = 128;
const UART_TILE_SIZE: usize = 8;
const UART_TILE_COLUMNS: usize = 8;
const UART_TILE_ROWS: usize = 16;
const UART_BITMAP_SCALE: usize = 4;

fn render_uart_records(args: &[String]) {
    if args.len() != 2 {
        eprintln!("usage: dt2-cli render-uart-records <run.log> <output.ppm>");
        std::process::exit(2);
    }
    let log = std::fs::read_to_string(&args[0]).unwrap_or_else(|error| {
        eprintln!("cannot read {}: {error}", args[0]);
        std::process::exit(2)
    });
    let bytes = parse_uart_hex_log(&log).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    let image = uart_records_bitmap_ppm(&bytes).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    std::fs::write(&args[1], image).unwrap_or_else(|error| {
        eprintln!("cannot write {}: {error}", args[1]);
        std::process::exit(2)
    });
    println!(
        "rendered {} UART bytes as a {}x{} tiled diagnostic bitmap at {} (bit 7 leftmost)",
        bytes.len(),
        UART_TILE_COLUMNS * UART_TILE_SIZE * UART_BITMAP_SCALE,
        UART_TILE_ROWS * UART_TILE_SIZE * UART_BITMAP_SCALE,
        args[1]
    );
}

fn parse_uart_hex_log(log: &str) -> Result<Vec<u8>, String> {
    let hex = log
        .lines()
        .find_map(|line| line.strip_prefix("UART8 TX hex: "))
        .ok_or_else(|| "run log has no UART8 TX hex line".to_string())?;
    hex.split_whitespace()
        .map(|byte| {
            u8::from_str_radix(byte, 16)
                .map_err(|_| format!("invalid UART8 hex byte in run log: {byte}"))
        })
        .collect()
}

fn uart_records_bitmap_ppm(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let expected = UART_RECORD_HEADER_BYTES + UART_RECORD_BYTES * UART_RECORD_COUNT + 1;
    if bytes.len() != expected {
        return Err(format!(
            "UART record stream must be {expected} bytes, got {}",
            bytes.len()
        ));
    }
    if bytes[expected - 1] != 0xB8 {
        return Err("UART record stream is missing the 0xB8 terminator".to_string());
    }

    let width = UART_TILE_COLUMNS * UART_TILE_SIZE;
    let height = UART_TILE_ROWS * UART_TILE_SIZE;
    let mut pixels = vec![0u8; width * height * 3];
    let records = &bytes[UART_RECORD_HEADER_BYTES..expected - 1];
    for record in records.chunks_exact(UART_RECORD_BYTES) {
        let tag = record[0];
        let y = usize::from(record[1]);
        if !(0x10..=0x17).contains(&tag) || y > 0x78 || y % UART_TILE_SIZE != 0 {
            return Err(format!(
                "invalid UART tile record header: tag={tag:#04x} y={y:#04x}"
            ));
        }
        let tile_x = usize::from(tag - 0x10) * UART_TILE_SIZE;
        for row in 0..UART_TILE_SIZE {
            let bits = record[2 + row];
            for column in 0..UART_TILE_SIZE {
                let color = if bits & (1 << (7 - column)) != 0 {
                    255
                } else {
                    0
                };
                let pixel = ((y + row) * width + tile_x + column) * 3;
                pixels[pixel..pixel + 3].fill(color);
            }
        }
    }

    let scaled_width = width * UART_BITMAP_SCALE;
    let scaled_height = height * UART_BITMAP_SCALE;
    let header = format!("P6\n{scaled_width} {scaled_height}\n255\n");
    let mut image = Vec::with_capacity(header.len() + scaled_width * scaled_height * 3);
    image.extend_from_slice(header.as_bytes());
    for y in 0..scaled_height {
        for x in 0..scaled_width {
            let source = ((y / UART_BITMAP_SCALE) * width + x / UART_BITMAP_SCALE) * 3;
            image.extend_from_slice(&pixels[source..source + 3]);
        }
    }
    Ok(image)
}

fn run_binary(args: &[String]) {
    if args.len() < 6 {
        eprintln!("usage: dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc|SBF_RESET_VECTOR> <instruction-limit> [MCF5441X_SRAM|MCF5441X_SBF_BOOT MCF5441X_SRAM_OVERLAY DDR_BOOTSTRAP PIT_BOOTSTRAP INTC2_BOOTSTRAP PIT_TICKS_PER_INSTRUCTION=n LOOP_DETECT INJECT_IRQ=count:level:vector UART8_TX SYNTHETIC_MCF_MMIO INSPECT_TRAP0 DUMP_RAM=address:length D0=value A0=value SR=value RAM32@address=value MMIO8@address=value MMIO16@address=value MMIO32@address=value ...]");
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
    let mcf_sram_overlay = args[6..]
        .iter()
        .any(|assignment| assignment == "MCF5441X_SRAM_OVERLAY");
    let bytes = std::fs::read(&args[0]).unwrap_or_else(|error| {
        eprintln!("cannot read {}: {error}", args[0]);
        std::process::exit(2)
    });
    let image_fits_ram = range_fits(ram_base, ram_size, image_address, bytes.len());
    let image_fits_sram_overlay = mcf_sram_overlay
        && range_fits(
            MCF_SRAM_ALIAS_WINDOW.0,
            0x1_0000,
            image_address,
            bytes.len(),
        );
    if !image_fits_ram && !image_fits_sram_overlay {
        eprintln!(
            "image range {image_address:#010x}..+{:#x} does not fit configured memory",
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
    let synthetic_mcf_mmio = args[6..]
        .iter()
        .any(|assignment| assignment == "SYNTHETIC_MCF_MMIO");
    let pit_bootstrap = args[6..]
        .iter()
        .any(|assignment| assignment == "PIT_BOOTSTRAP");
    let intc2_bootstrap = args[6..]
        .iter()
        .any(|assignment| assignment == "INTC2_BOOTSTRAP");
    let pit0_oneshot = args[6..]
        .iter()
        .any(|assignment| assignment == "PIT0_ONESHOT");
    let loop_detect = args[6..]
        .iter()
        .any(|assignment| assignment == "LOOP_DETECT");
    let stop_pc = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("STOP_PC=").map(|pc| {
            parse_address(pc).unwrap_or_else(|error| {
                eprintln!("invalid STOP_PC address: {error}");
                std::process::exit(2)
            })
        })
    });
    let watch_write_address = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("WATCH_WRITE=").map(|address| {
            parse_address(address).unwrap_or_else(|error| {
                eprintln!("invalid WATCH_WRITE address: {error}");
                std::process::exit(2)
            })
        })
    });
    let trace_register_pc = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("TRACE_REGS=").map(|pc| {
            parse_address(pc).unwrap_or_else(|error| {
                eprintln!("invalid TRACE_REGS address: {error}");
                std::process::exit(2)
            })
        })
    });
    let trace_a0_range = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("TRACE_A0_RANGE=").map(|range| {
            let (start, end) = range.split_once(':').unwrap_or_else(|| {
                eprintln!("invalid TRACE_A0_RANGE spec: {range}; expected start:end");
                std::process::exit(2)
            });
            let start = parse_address(start).unwrap_or_else(|error| {
                eprintln!("invalid TRACE_A0_RANGE start: {error}");
                std::process::exit(2)
            });
            let end = parse_address(end).unwrap_or_else(|error| {
                eprintln!("invalid TRACE_A0_RANGE end: {error}");
                std::process::exit(2)
            });
            if start >= end {
                eprintln!("TRACE_A0_RANGE start must be less than end");
                std::process::exit(2)
            }
            (start, end)
        })
    });
    let trace_pc_range = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("TRACE_PC_RANGE=").map(|range| {
            let (start, end) = range.split_once(':').unwrap_or_else(|| {
                eprintln!("invalid TRACE_PC_RANGE spec: {range}; expected start:end");
                std::process::exit(2)
            });
            let start = parse_address(start).unwrap_or_else(|error| {
                eprintln!("invalid TRACE_PC_RANGE start: {error}");
                std::process::exit(2)
            });
            let end = parse_address(end).unwrap_or_else(|error| {
                eprintln!("invalid TRACE_PC_RANGE end: {error}");
                std::process::exit(2)
            });
            if start >= end {
                eprintln!("TRACE_PC_RANGE start must be less than end");
                std::process::exit(2)
            }
            (start, end)
        })
    });
    let ram_remap = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("RAM_REMAP=").map(|spec| {
            let mut parts = spec.split(':');
            let source = parts.next().unwrap_or_default();
            let destination = parts.next().unwrap_or_default();
            let length = parts.next().unwrap_or_default();
            if source.is_empty() || destination.is_empty() || length.is_empty() || parts.next().is_some() {
                eprintln!("invalid RAM_REMAP spec: {spec}; expected from:to:length");
                std::process::exit(2)
            }
            let source = parse_address(source).unwrap_or_else(|error| {
                eprintln!("invalid RAM_REMAP source: {error}");
                std::process::exit(2)
            });
            let destination = parse_address(destination).unwrap_or_else(|error| {
                eprintln!("invalid RAM_REMAP destination: {error}");
                std::process::exit(2)
            });
            let length = parse_address(length).unwrap_or_else(|error| {
                eprintln!("invalid RAM_REMAP length: {error}");
                std::process::exit(2)
            });
            if length == 0
                || source.checked_add(length).is_none()
                || destination.checked_add(length).is_none()
                || !range_fits(ram_base, ram_size, source, length as usize)
                || !range_fits(ram_base, ram_size, destination, length as usize)
            {
                eprintln!("RAM_REMAP range must be nonempty and fit inside configured RAM");
                std::process::exit(2)
            }
            RamRemap {
                source,
                destination,
                length,
            }
        })
    });
    let ram_snapshot = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("RAM_SNAPSHOT=").map(|spec| {
            let values: Vec<_> = spec.split(':').collect();
            if values.len() != 8 {
                eprintln!("invalid RAM_SNAPSHOT spec: {spec}; expected watch:src:dst:length:trigger_start:trigger_end:read_start:read_end");
                std::process::exit(2)
            }
            let mut parsed = [0u32; 8];
            for (index, value) in values.iter().enumerate() {
                parsed[index] = parse_address(value).unwrap_or_else(|error| {
                    eprintln!("invalid RAM_SNAPSHOT value: {error}");
                    std::process::exit(2)
                });
            }
            let [watch, source, destination, length, trigger_pc_start, trigger_pc_end, read_pc_start, read_pc_end] = parsed;
            let source_end = source.checked_add(length);
            let destination_end = destination.checked_add(length);
            if length == 0
                || trigger_pc_start >= trigger_pc_end
                || read_pc_start >= read_pc_end
                || !range_fits(ram_base, ram_size, source, length as usize)
                || !range_fits(ram_base, ram_size, destination, length as usize)
                || source_end.is_none()
                || destination_end.is_none()
                || watch < source
                || watch >= source_end.unwrap()
                || (source < destination_end.unwrap() && destination < source_end.unwrap())
            {
                eprintln!("RAM_SNAPSHOT ranges must fit RAM, be disjoint, and contain the watch address");
                std::process::exit(2)
            }
            RamSnapshot {
                watch,
                source,
                destination,
                length,
                trigger_pc_start,
                trigger_pc_end,
                read_pc_start,
                read_pc_end,
                active: false,
            }
        })
    });
    if ram_snapshot.is_some() && ram_remap.is_some() {
        eprintln!("RAM_SNAPSHOT cannot be combined with RAM_REMAP");
        std::process::exit(2);
    }
    let interrupt_injection = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("INJECT_IRQ=").map(|spec| {
            parse_interrupt_injection(spec).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(2)
            })
        })
    });
    let pit_tick_setting = args[6..]
        .iter()
        .find_map(|assignment| assignment.strip_prefix("PIT_TICKS_PER_INSTRUCTION="));
    let pit_ticks_per_instruction = match pit_tick_setting {
        Some(value) => match value.parse::<u32>() {
            Ok(ticks) if ticks > 0 => ticks,
            _ => {
                eprintln!("PIT_TICKS_PER_INSTRUCTION must be a positive integer");
                std::process::exit(2)
            }
        },
        None => 1,
    };
    if pit_tick_setting.is_some() && !pit_bootstrap {
        eprintln!("PIT_TICKS_PER_INSTRUCTION requires PIT_BOOTSTRAP");
        std::process::exit(2);
    }
    if pit0_oneshot && (!pit_bootstrap || !intc2_bootstrap) {
        eprintln!("PIT0_ONESHOT requires PIT_BOOTSTRAP and INTC2_BOOTSTRAP");
        std::process::exit(2);
    }
    let uart8_tx = args[6..].iter().any(|assignment| assignment == "UART8_TX");
    let inspect_trap0 = args[6..]
        .iter()
        .any(|assignment| assignment == "INSPECT_TRAP0");
    if reset_from_sbf_vector && !mcf_sbf_boot {
        eprintln!("SBF_RESET_VECTOR requires MCF5441X_SBF_BOOT");
        std::process::exit(2);
    }
    if mcf_sram_overlay && mcf_sram_alias && !mcf_sbf_boot {
        eprintln!("MCF5441X_SRAM_OVERLAY cannot be combined with MCF5441X_SRAM");
        std::process::exit(2);
    }
    let mut bus = if mcf_sram_overlay {
        ProbeBus::new(FlatRam::new(ram_base, ram_size))
    } else if mcf_sram_alias {
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
        if mcf_sbf_boot {
            bus.attach_mcf_sram_overlay_with_sbf_alias(true);
        } else {
            bus.attach_mcf_sram_overlay();
        }
    }
    if synthetic_mcf_mmio {
        bus.enable_synthetic_mcf_mmio();
    }
    if pit_bootstrap {
        if pit_ticks_per_instruction == 1 {
            bus.attach_pit_bootstrap();
        } else {
            bus.attach_pit_bootstrap_with_ticks(pit_ticks_per_instruction);
        }
    }
    if intc2_bootstrap {
        bus.attach_intc2();
    }
    if pit0_oneshot {
        bus.enable_pit0_oneshot();
    }
    if let Some(address) = watch_write_address {
        bus.watch_write(address);
    }
    if let Some(pc) = trace_register_pc {
        bus.trace_registers_at(pc);
    }
    if let Some(range) = trace_a0_range {
        bus.trace_first_a0_in_range(range);
    }
    if let Some(range) = trace_pc_range {
        bus.trace_pc_range(range);
    }
    if let Some(remap) = ram_remap {
        bus.set_ram_remap(remap);
    }
    if let Some(snapshot) = ram_snapshot {
        bus.set_ram_snapshot(snapshot);
    }
    if let Some(pc) = stop_pc {
        bus.set_stop_pc(pc);
    }
    if loop_detect {
        bus.enable_loop_detection();
    }
    if uart8_tx {
        bus.attach_uart8_tx();
    }
    bus.load_ram(image_address, &bytes).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    let mut cpu = Cpu::new();
    let mut cpu_overrides = Vec::new();
    let mut ram_dumps = Vec::new();
    let mut ram_file_dumps = Vec::new();
    for assignment in &args[6..] {
        if assignment == "MCF5441X_SRAM"
            || assignment == "MCF5441X_SBF_BOOT"
            || assignment == "MCF5441X_SRAM_OVERLAY"
            || assignment == "SYNTHETIC_MCF_MMIO"
            || assignment == "PIT_BOOTSTRAP"
            || assignment == "INTC2_BOOTSTRAP"
            || assignment == "PIT0_ONESHOT"
            || assignment.starts_with("STOP_PC=")
            || assignment.starts_with("WATCH_WRITE=")
            || assignment.starts_with("TRACE_REGS=")
            || assignment.starts_with("TRACE_A0_RANGE=")
            || assignment.starts_with("TRACE_PC_RANGE=")
            || assignment.starts_with("RAM_REMAP=")
            || assignment.starts_with("RAM_SNAPSHOT=")
            || assignment.starts_with("PIT_TICKS_PER_INSTRUCTION=")
            || assignment == "LOOP_DETECT"
            || assignment.starts_with("INJECT_IRQ=")
            || assignment == "UART8_TX"
            || assignment == "INSPECT_TRAP0"
        {
            continue;
        }
        if let Some(spec) = assignment.strip_prefix("DUMP_RAM=") {
            ram_dumps.push(parse_ram_dump_spec(spec).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(2)
            }));
            continue;
        }
        if let Some(spec) = assignment.strip_prefix("DUMP_RAM_FILE=") {
            ram_file_dumps.push(parse_ram_file_dump_spec(spec).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(2)
            }));
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
    let run_started = Instant::now();
    let report = cpu.run_with_interrupt(&mut bus, instruction_limit, interrupt_injection);
    let elapsed_seconds = run_started.elapsed().as_secs_f64();
    let instructions_per_second = if elapsed_seconds > 0.0 {
        report.instructions as f64 / elapsed_seconds
    } else {
        0.0
    };

    for (address, length) in ram_dumps {
        match bus.dump_ram(address, length) {
            Ok(bytes) => print_ram_dump(address, &bytes),
            Err(error) => println!("RAM dump unmapped at {:#010x}", error.addr),
        }
    }
    for (address, length, path) in ram_file_dumps {
        match bus.dump_ram(address, length) {
            Ok(bytes) => {
                if let Err(error) = std::fs::write(&path, bytes) {
                    eprintln!("cannot write RAM dump {path}: {error}");
                    std::process::exit(2)
                }
                println!("wrote {length} RAM bytes from {address:#010x} to {path}");
            }
            Err(error) => {
                eprintln!("RAM file dump unmapped at {:#010x}", error.addr);
                std::process::exit(2)
            }
        }
    }

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
    if let Some(remap) = ram_remap {
        println!(
            "RAM remap: {:#010x}..{:#010x} -> {:#010x}..{:#010x}",
            remap.source,
            remap.source + remap.length,
            remap.destination,
            remap.destination + remap.length
        );
    }
    if let Some(snapshot) = ram_snapshot {
        println!(
            "RAM snapshot on write {:#010x} at PCs {:#010x}..{:#010x}: {:#010x}..{:#010x} -> {:#010x}; read PCs {:#010x}..{:#010x}",
            snapshot.watch,
            snapshot.trigger_pc_start,
            snapshot.trigger_pc_end,
            snapshot.source,
            snapshot.source + snapshot.length,
            snapshot.destination,
            snapshot.read_pc_start,
            snapshot.read_pc_end
        );
    }
    if let Some(snapshot) = &bus.ram_snapshot {
        println!("RAM snapshot captured: {}", snapshot.active);
    }
    if let Some(pc) = stop_pc {
        println!("stop breakpoint configured at PC={pc:#010x}");
    }
    if pit0_oneshot {
        println!("PIT0 one-shot diagnostic: expose the first unmasked PIT0 request; suppress later PIT0 requests");
    }
    bus.print_watched_writes();
    bus.print_register_trace();
    println!("executed {} instructions", report.instructions);
    println!(
        "runner throughput: {elapsed_seconds:.3}s, {instructions_per_second:.0} instructions/s"
    );
    if report.fast_forwarded_instructions > 0 {
        println!(
            "fast-forwarded {} stable-loop instructions",
            report.fast_forwarded_instructions
        );
    }
    if let Some(injection) = report.injected_interrupt {
        println!(
            "synthetic interrupt injected at instruction {}: level {} vector {:#04x}; bypassed INTC source mask and acknowledge",
            injection.instruction_count, injection.level, injection.vector
        );
    }
    if let Some(interrupt) = report.natural_interrupt {
        println!(
            "natural interrupt delivered after instruction {}: source {} level {} vector {:#04x}",
            interrupt.instruction_count, interrupt.source, interrupt.level, interrupt.vector
        );
    }
    if let Some(behavior) = bus.unsupported_device_behavior() {
        println!("device model: unsupported {behavior}");
    }
    if bus.ddr.is_some() {
        println!("DDR model: bootstrap-observed registers only");
    }
    if bus.pit.is_some() {
        println!(
            "PIT model: {pit_ticks_per_instruction} timer tick(s) per retired instruction;{}approximate timing, no INTC delivery",
            if pit_ticks_per_instruction > 1 {
                " accelerated "
            } else {
                " "
            }
        );
    }
    if let Some(pit) = &bus.pit {
        if let Some((pcsr, pmr, counter)) = pit.timer_state(1) {
            println!("PIT1 state: PCSR={pcsr:#06x} PMR={pmr:#06x} PCNTR={counter:#06x}");
        }
    }
    if let Some(uart) = &bus.uart8 {
        println!(
            "UART8 model: TX-only sink, no RX or serial timing; captured {} bytes",
            uart.transmitted()
        );
        if !uart.captured().is_empty() {
            let hex = uart
                .captured()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!("UART8 TX hex: {hex}");
            let text = uart
                .captured()
                .iter()
                .flat_map(|byte| std::ascii::escape_default(*byte))
                .map(char::from)
                .collect::<String>();
            println!("UART8 TX text (escaped): {text}");
            let tail = &uart.captured()[uart.captured().len().saturating_sub(32)..];
            let tail_hex = tail
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            let tail_text = tail
                .iter()
                .flat_map(|byte| std::ascii::escape_default(*byte))
                .map(char::from)
                .collect::<String>();
            println!("UART8 TX tail hex ({} bytes): {tail_hex}", tail.len());
            println!("UART8 TX tail text (escaped): {tail_text}");
        }
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
    bus.print_loop_reports();
    bus.print_last_control_flow();
    bus.print_rte_history();
    bus.print_halt_context(report.stop);
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

fn parse_ram_dump_spec(spec: &str) -> Result<(u32, usize), String> {
    let (address, length) = spec
        .split_once(':')
        .ok_or_else(|| format!("invalid DUMP_RAM spec: {spec}"))?;
    let address = parse_address(address)?;
    let length = usize::try_from(parse_address(length)?)
        .map_err(|_| format!("invalid DUMP_RAM length: {length}"))?;
    if !(1..=256).contains(&length) {
        return Err(format!("DUMP_RAM length must be 1-256 bytes: {length}"));
    }
    if address.checked_add(length as u32).is_none() {
        return Err(format!("DUMP_RAM range overflows address space: {spec}"));
    }
    Ok((address, length))
}

fn parse_ram_file_dump_spec(spec: &str) -> Result<(u32, usize, String), String> {
    let mut fields = spec.splitn(3, ':');
    let address_text = fields
        .next()
        .ok_or_else(|| format!("invalid DUMP_RAM_FILE spec: {spec}"))?;
    let length_text = fields
        .next()
        .ok_or_else(|| format!("invalid DUMP_RAM_FILE spec: {spec}"))?;
    let path = fields
        .next()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| format!("invalid DUMP_RAM_FILE path: {spec}"))?;
    let address = parse_address(address_text)?;
    let length = usize::try_from(parse_address(length_text)?)
        .map_err(|_| format!("invalid DUMP_RAM_FILE length: {length_text}"))?;
    if length == 0 || length > 64 * 1024 * 1024 {
        return Err(format!("DUMP_RAM_FILE length must be 1-67108864 bytes: {length}"));
    }
    if address.checked_add(length as u32).is_none() {
        return Err(format!("DUMP_RAM_FILE range overflows address space: {spec}"));
    }
    Ok((address, length, path.to_string()))
}

fn parse_interrupt_injection(spec: &str) -> Result<InterruptInjection, String> {
    let mut fields = spec.split(':');
    let instruction_count = fields
        .next()
        .ok_or_else(|| format!("invalid INJECT_IRQ spec: {spec}"))?
        .parse::<usize>()
        .map_err(|_| format!("invalid INJECT_IRQ instruction count: {spec}"))?;
    let level = fields
        .next()
        .ok_or_else(|| format!("invalid INJECT_IRQ spec: {spec}"))?
        .parse::<u8>()
        .ok()
        .filter(|level| (1..=7).contains(level))
        .ok_or_else(|| format!("INJECT_IRQ level must be 1-7: {spec}"))?;
    let vector = fields
        .next()
        .ok_or_else(|| format!("invalid INJECT_IRQ spec: {spec}"))
        .and_then(parse_address)
        .and_then(|value| {
            u8::try_from(value).map_err(|_| format!("INJECT_IRQ vector exceeds 8 bits: {spec}"))
        })?;
    if fields.next().is_some() {
        return Err(format!("invalid INJECT_IRQ spec: {spec}"));
    }
    Ok(InterruptInjection {
        instruction_count,
        level,
        vector,
    })
}

fn print_ram_dump(address: u32, bytes: &[u8]) {
    println!("RAM dump {address:#010x} ({} bytes):", bytes.len());
    for (line, chunk) in bytes.chunks(16).enumerate() {
        print!("  {:#010x}:", address.wrapping_add((line * 16) as u32));
        for byte in chunk {
            print!(" {byte:02x}");
        }
        println!();
    }
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
    ram_remap: Option<RamRemap>,
    ram_snapshot: Option<RamSnapshot>,
    stop_pc: Option<u32>,
    sram_overlay: Option<Mcf5441xSram>,
    sram_overlay_sbf_alias: bool,
    mmio: BTreeMap<u32, u8>,
    dspi0: Option<Dspi>,
    ddr: Option<DdrController>,
    pit: Option<PitController>,
    intc2: Option<Intc2>,
    uart8: Option<Uart8>,
    pit0_oneshot: bool,
    pit0_request_seen: bool,
    watch_write_address: Option<u32>,
    watched_write_count: usize,
    watched_writes: VecDeque<WatchedWrite>,
    trace_register_pc: Option<u32>,
    register_trace: VecDeque<RegisterTrace>,
    trace_a0_range: Option<(u32, u32)>,
    first_a0_range_trace: Option<RegisterTrace>,
    trace_pc_range: Option<(u32, u32)>,
    synthetic_mcf_mmio: bool,
    access_events: Vec<AccessEvent>,
    access_addresses: BTreeSet<u32>,
    access_summaries: BTreeMap<u32, AccessSummary>,
    current_pc: u32,
    suppressed_access_events: usize,
    loop_detection: bool,
    current_instruction_count: usize,
    loop_observations: BTreeMap<u32, LoopObservation>,
    loop_access_history: VecDeque<LoopAccess>,
    instruction_fetch: bool,
    loop_reports: Vec<LoopReport>,
    loop_reported_pcs: BTreeSet<u32>,
    last_tst: Option<TstDiagnostic>,
    last_branch: Option<BranchDiagnostic>,
    rte_count: usize,
    rte_history: VecDeque<RteDiagnostic>,
    recent_mmio_accesses: VecDeque<(u32, AccessEvent)>,
}

#[derive(Clone, Copy)]
struct RamRemap {
    source: u32,
    destination: u32,
    length: u32,
}

#[derive(Clone, Copy)]
struct RamSnapshot {
    watch: u32,
    source: u32,
    destination: u32,
    length: u32,
    trigger_pc_start: u32,
    trigger_pc_end: u32,
    read_pc_start: u32,
    read_pc_end: u32,
    active: bool,
}

const DSPI0_BASE: u32 = 0xFC05_C000;
const MCF_SRAM_ALIAS_WINDOW: (u32, u32) = (0x8000_0000, 0x9000_0000);
const MCF_PERIPHERAL_WINDOWS: [(u32, u32); 2] =
    [(0xEC00_0000, 0xED00_0000), (0xFC00_0000, 0xFD00_0000)];
const DDR_REGISTER_APERTURE: (u32, u32) = (0xFC0B_8000, 0xFC0B_8200);
const MAX_ACCESS_EVENTS: usize = 256;
const MAX_ACCESS_ADDRESSES: usize = 128;
const MAX_LOOP_TRACKED_PCS: usize = 4096;
const MAX_LOOP_HISTORY: usize = 4096;
const MAX_RTE_HISTORY: usize = 16;
const MAX_WATCHED_WRITES: usize = 16;
const MAX_REGISTER_TRACE: usize = 16;
const PIT1_PCSR: u32 = 0xFC08_4000;

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

#[derive(Debug, Clone, Copy)]
struct AccessSummary {
    first_pc: u32,
    reads: u64,
    writes: u64,
    last_value: u32,
    source: AccessSource,
    unmodeled: bool,
}

#[derive(Clone, Copy)]
struct LoopObservation {
    d: [u32; 8],
    a: [u32; 8],
    sr: u16,
    instruction_count: usize,
    repeats: u32,
    stable_repeats: u32,
}

#[derive(Clone, Copy)]
struct LoopAccess {
    instruction_count: usize,
    address: u32,
    write: bool,
}

struct LoopReport {
    pc: u32,
    start_pc: u32,
    end_pc: u32,
    sr: u16,
    repeat_period: usize,
    modified_registers: Vec<String>,
    read_addresses: Vec<u32>,
}

struct TstDiagnostic {
    pc: u32,
    operand: String,
    value: u32,
    size_bytes: u8,
    sr: u16,
}

struct RteDiagnostic {
    pc: u32,
    frame_sp: u32,
    format_vector: u16,
    restored_sr: u16,
    restored_pc: u32,
}

struct WatchedWrite {
    pc: u32,
    address: u32,
    width_bits: u8,
    value: u32,
}

struct RegisterTrace {
    pc: u32,
    d: [u32; 8],
    a: [u32; 8],
    sr: u16,
}

struct BranchDiagnostic {
    pc: u32,
    condition: u8,
    taken: bool,
    target: u32,
    sr: u16,
}

impl ProbeBus {
    fn new(ram: impl Bus + 'static) -> Self {
        Self {
            ram: Box::new(ram),
            ram_remap: None,
            ram_snapshot: None,
            stop_pc: None,
            sram_overlay: None,
            sram_overlay_sbf_alias: false,
            mmio: BTreeMap::new(),
            dspi0: None,
            ddr: None,
            pit: None,
            intc2: None,
            uart8: None,
            pit0_oneshot: false,
            pit0_request_seen: false,
            watch_write_address: None,
            watched_write_count: 0,
            watched_writes: VecDeque::new(),
            trace_register_pc: None,
            register_trace: VecDeque::new(),
            trace_a0_range: None,
            first_a0_range_trace: None,
            trace_pc_range: None,
            synthetic_mcf_mmio: false,
            access_events: Vec::new(),
            access_addresses: BTreeSet::new(),
            access_summaries: BTreeMap::new(),
            current_pc: 0,
            suppressed_access_events: 0,
            loop_detection: false,
            current_instruction_count: 0,
            loop_observations: BTreeMap::new(),
            loop_access_history: VecDeque::new(),
            instruction_fetch: false,
            loop_reports: Vec::new(),
            loop_reported_pcs: BTreeSet::new(),
            last_tst: None,
            last_branch: None,
            rte_count: 0,
            rte_history: VecDeque::new(),
            recent_mmio_accesses: VecDeque::new(),
        }
    }

    fn enable_loop_detection(&mut self) {
        self.loop_detection = true;
    }

    fn enable_pit0_oneshot(&mut self) {
        self.pit0_oneshot = true;
    }

    fn watch_write(&mut self, address: u32) {
        self.watch_write_address = Some(address);
    }

    fn trace_registers_at(&mut self, pc: u32) {
        self.trace_register_pc = Some(pc);
    }

    fn trace_first_a0_in_range(&mut self, range: (u32, u32)) {
        self.trace_a0_range = Some(range);
    }

    fn trace_pc_range(&mut self, range: (u32, u32)) {
        self.trace_pc_range = Some(range);
    }

    fn set_ram_remap(&mut self, remap: RamRemap) {
        self.ram_remap = Some(remap);
    }

    fn set_ram_snapshot(&mut self, snapshot: RamSnapshot) {
        self.ram_snapshot = Some(snapshot);
    }

    fn set_stop_pc(&mut self, pc: u32) {
        self.stop_pc = Some(pc);
    }

    fn snapshot_read_address(&self, address: u32) -> u32 {
        self.ram_snapshot
            .filter(|snapshot| {
                snapshot.active
                    && (snapshot.read_pc_start..snapshot.read_pc_end).contains(&self.current_pc)
                    && address >= snapshot.source
                    && address - snapshot.source < snapshot.length
            })
            .map_or(address, |snapshot| {
                snapshot.destination + (address - snapshot.source)
            })
    }

    fn snapshot_source_on_write(
        &mut self,
        address: u32,
        width_bytes: u32,
    ) -> Result<(), BusError> {
        let Some(snapshot) = self.ram_snapshot.filter(|snapshot| {
            !snapshot.active
                && snapshot.watch >= address
                && snapshot.watch - address < width_bytes
                && (snapshot.trigger_pc_start..snapshot.trigger_pc_end)
                    .contains(&self.current_pc)
        }) else {
            return Ok(());
        };
        for offset in 0..snapshot.length {
            let value = self.ram.read8(snapshot.source + offset)?;
            self.ram.write8(snapshot.destination + offset, value)?;
        }
        if let Some(active) = &mut self.ram_snapshot {
            active.active = true;
        }
        Ok(())
    }

    fn remapped_ram_address(&self, address: u32) -> u32 {
        self.ram_remap
            .filter(|remap| {
                address >= remap.source && address - remap.source < remap.length
            })
            .map_or(address, |remap| {
                remap.destination + (address - remap.source)
            })
    }

    fn record_watched_write(&mut self, address: u32, width: u8, value: u32) {
        let Some(watched_address) = self.watch_write_address else {
            return;
        };
        if watched_address < address || watched_address - address >= u32::from(width) {
            return;
        }
        self.watched_write_count += 1;
        if self.watched_writes.len() < MAX_WATCHED_WRITES {
            self.watched_writes.push_back(WatchedWrite {
                pc: self.current_pc,
                address,
                width_bits: width * 8,
                value,
            });
        }
    }

    fn enable_synthetic_mcf_mmio(&mut self) {
        self.synthetic_mcf_mmio = true;
    }

    fn attach_mcf_sram_overlay(&mut self) {
        self.attach_mcf_sram_overlay_with_sbf_alias(false);
    }

    fn attach_mcf_sram_overlay_with_sbf_alias(&mut self, sbf_reset_alias: bool) {
        let sram = Mcf5441xSram::new();
        self.sram_overlay = Some(if sbf_reset_alias {
            sram.with_sbf_reset_alias()
        } else {
            sram
        });
        self.sram_overlay_sbf_alias = sbf_reset_alias;
    }

    fn sram_for(&mut self, address: u32) -> Option<&mut Mcf5441xSram> {
        self.sram_overlay.as_mut().filter(|_| {
            (MCF_SRAM_ALIAS_WINDOW.0..MCF_SRAM_ALIAS_WINDOW.1).contains(&address)
                || (self.sram_overlay_sbf_alias && address < 0x1_0000)
        })
    }

    fn synthetic_access(&self, address: u32) -> bool {
        self.synthetic_mcf_mmio
            && MCF_PERIPHERAL_WINDOWS
                .iter()
                .any(|&(start, end)| (start..end).contains(&address))
            && !(DDR_REGISTER_APERTURE.0..DDR_REGISTER_APERTURE.1).contains(&address)
    }

    fn source_for_byte(&self, address: u32) -> AccessSource {
        if self.mmio.contains_key(&address) {
            AccessSource::Seeded
        } else if self
            .dspi0
            .as_ref()
            .is_some_and(|dspi| dspi.contains(address))
            || self
                .ddr
                .as_ref()
                .is_some_and(|_| DdrController::contains(address))
            || self
                .pit
                .as_ref()
                .is_some_and(|_| PitController::contains(address))
            || self
                .uart8
                .as_ref()
                .is_some_and(|_| Uart8::contains(address))
            || self
                .intc2
                .as_ref()
                .is_some_and(|_| Intc2::contains(address))
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
        if !MCF_PERIPHERAL_WINDOWS
            .iter()
            .any(|&(start, end)| (start..end).contains(&event.address))
        {
            return;
        }
        if !self.access_summaries.contains_key(&event.address)
            && self.access_summaries.len() >= MAX_ACCESS_ADDRESSES
        {
            self.suppressed_access_events = self.suppressed_access_events.saturating_add(1);
            return;
        }
        let summary = self
            .access_summaries
            .entry(event.address)
            .or_insert(AccessSummary {
                first_pc: self.current_pc,
                reads: 0,
                writes: 0,
                last_value: event.value,
                source: event.source,
                unmodeled: false,
            });
        if event.write {
            summary.writes = summary.writes.saturating_add(1);
        } else {
            summary.reads = summary.reads.saturating_add(1);
        }
        summary.last_value = event.value;
        summary.unmodeled |= matches!(event.source, AccessSource::Synthetic | AccessSource::Mixed);
        self.recent_mmio_accesses
            .push_back((self.current_pc, event));
        if self.recent_mmio_accesses.len() > 32 {
            self.recent_mmio_accesses.pop_front();
        }

        if self.access_events.len() >= MAX_ACCESS_EVENTS {
            self.suppressed_access_events = self.suppressed_access_events.saturating_add(1);
            return;
        }
        let is_new_address = !self.access_addresses.contains(&event.address);
        if is_new_address && self.access_addresses.len() >= MAX_ACCESS_ADDRESSES {
            self.suppressed_access_events = self.suppressed_access_events.saturating_add(1);
            return;
        }
        self.access_addresses.insert(event.address);
        self.access_events.push(event);
    }

    fn record_loop_access(&mut self, address: u32, write: bool) {
        if !self.loop_detection || self.instruction_fetch {
            return;
        }
        self.loop_access_history.push_back(LoopAccess {
            instruction_count: self.current_instruction_count,
            address,
            write,
        });
        if self.loop_access_history.len() > MAX_LOOP_HISTORY {
            self.loop_access_history.pop_front();
        }
    }

    fn print_loop_reports(&self) {
        if !self.loop_detection {
            return;
        }
        println!("loop detection: {} candidate(s)", self.loop_reports.len());
        for report in &self.loop_reports {
            let modified = if report.modified_registers.is_empty() {
                "none observed".to_string()
            } else {
                report.modified_registers.join(",")
            };
            let reads = if report.read_addresses.is_empty() {
                "none".to_string()
            } else {
                report
                    .read_addresses
                    .iter()
                    .map(|address| format!("{address:#010x}"))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            println!(
                "  loop {:#010x}..{:#010x}, recurring pc={:#010x}, period={} instructions; SR={:#06x} interrupt_mask={}; modified={modified}; reads={reads}",
                report.start_pc,
                report.end_pc,
                report.pc,
                report.repeat_period,
                report.sr,
                (report.sr >> 8) & 7
            );
        }
    }

    fn print_rte_history(&self) {
        if self.rte_count > 0 {
            println!(
                "RTE events: {} (showing most recent {})",
                self.rte_count,
                self.rte_history.len()
            );
            let first_index = self.rte_count - self.rte_history.len() + 1;
            for (offset, rte) in self.rte_history.iter().enumerate() {
                println!(
                    "  RTE #{} at {:#010x}: frame={:#010x} format={} vector={:#05x} restored_SR={:#06x} restored_PC={:#010x}",
                    first_index + offset,
                    rte.pc,
                    rte.frame_sp,
                    rte.format_vector >> 12,
                    rte.format_vector & 0x0FFF,
                    rte.restored_sr,
                    rte.restored_pc
                );
            }
        }
    }

    fn print_watched_writes(&self) {
        let Some(address) = self.watch_write_address else {
            return;
        };
        println!(
            "write watch at {address:#010x}: {} matching writes (showing first {})",
            self.watched_write_count,
            self.watched_writes.len()
        );
        for write in &self.watched_writes {
            println!(
                "  write{} at PC={:#010x} address={:#010x} value={:#010x}",
                write.width_bits, write.pc, write.address, write.value
            );
        }
    }

    fn print_register_trace(&self) {
        if let Some(pc) = self.trace_register_pc {
            println!(
                "register trace at PC={pc:#010x}: {} snapshots (showing most recent)",
                self.register_trace.len()
            );
            for trace in &self.register_trace {
                println!(
                    "  PC={:#010x} SR={:#06x} D0={:#010x} D1={:#010x} D2={:#010x} D3={:#010x} A0={:#010x} A1={:#010x} A2={:#010x} A3={:#010x}",
                    trace.pc,
                    trace.sr,
                    trace.d[0],
                    trace.d[1],
                    trace.d[2],
                    trace.d[3],
                    trace.a[0],
                    trace.a[1],
                    trace.a[2],
                    trace.a[3]
                );
            }
        }
        if let (Some((start, end)), Some(trace)) = (self.trace_a0_range, &self.first_a0_range_trace) {
            println!(
            "first snapshot with A0 in {start:#010x}..{end:#010x}: PC={:#010x} SR={:#06x} D0={:#010x} D1={:#010x} D2={:#010x} D3={:#010x} A0={:#010x} A1={:#010x} A2={:#010x} A3={:#010x}",
                trace.pc,
                trace.sr,
                trace.d[0],
                trace.d[1],
                trace.d[2],
                trace.d[3],
                trace.a[0],
                trace.a[1],
                trace.a[2],
                trace.a[3]
            );
        }
    }

    fn print_halt_context(&self, stop: RunStop) {
        let RunStop::Halt { pc } = stop else {
            return;
        };
        println!("HALT context at {pc:#010x}:");
        if let Some(tst) = &self.last_tst {
            println!(
                "  TST at {:#010x}: {} value={:#010x} width={} SR={:#06x}",
                tst.pc,
                tst.operand,
                tst.value,
                tst.size_bytes * 8,
                tst.sr
            );
        }
        if let Some(branch) = &self.last_branch {
            println!(
                "  branch at {:#010x}: condition={} taken={} target={:#010x} SR={:#06x}",
                branch.pc,
                branch_condition_name(branch.condition),
                branch.taken,
                branch.target,
                branch.sr
            );
        }
        println!(
            "  recent MMIO before HALT ({} accesses):",
            self.recent_mmio_accesses.len()
        );
        let mut summarized = BTreeSet::new();
        for (pc, event) in &self.recent_mmio_accesses {
            let operation = if event.write { "write" } else { "read" };
            println!(
                "    {operation}{} at PC={pc:#010x} address={:#010x} value={:#010x} [{}]",
                event.width,
                event.address,
                event.value,
                event.source.label()
            );
            summarized.insert(event.address);
        }
        for address in summarized {
            if let Some(summary) = self.access_summaries.get(&address) {
                println!(
                    "    summary {address:#010x}: first_pc={:#010x} reads={} writes={} last={:#010x} [{}{}]",
                    summary.first_pc,
                    summary.reads,
                    summary.writes,
                    summary.last_value,
                    summary.source.label(),
                    if summary.unmodeled {
                        ", synthetic/unmodeled"
                    } else {
                        ""
                    }
                );
            }
        }
    }

    fn print_access_trace(&self) {
        if self.synthetic_mcf_mmio {
            println!(
                "MMIO diagnostic: synthetic responses are approximations, not hardware behavior"
            );
        }
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
        println!(
            "MMIO access summary: {} addresses",
            self.access_summaries.len()
        );
        for (address, summary) in &self.access_summaries {
            println!(
                "  {address:#010x}: first_pc={:#010x} reads={} writes={} last={:#010x} [{}{}]",
                summary.first_pc,
                summary.reads,
                summary.writes,
                summary.last_value,
                summary.source.label(),
                if summary.unmodeled {
                    ", synthetic/unmodeled"
                } else {
                    ""
                }
            );
        }
    }

    fn print_last_control_flow(&self) {
        if let Some(tst) = &self.last_tst {
            println!(
                "last TST at {:#010x}: {} value={:#010x} width={} SR={:#06x}",
                tst.pc,
                tst.operand,
                tst.value,
                tst.size_bytes * 8,
                tst.sr
            );
        }
        if let Some(branch) = &self.last_branch {
            println!(
                "last branch at {:#010x}: condition={} taken={} target={:#010x} SR={:#06x}",
                branch.pc,
                branch_condition_name(branch.condition),
                branch.taken,
                branch.target,
                branch.sr
            );
        }
    }

    fn read8_inner(&mut self, address: u32) -> Result<u8, BusError> {
        if let Some(value) = self.mmio.get(&address) {
            return Ok(*value);
        }
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read8(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read8(address);
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.read8(address);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.read8(address);
        }
        if let Some(intc2) = self.intc2_for(address) {
            return intc2.read8(address);
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
            let address = self.snapshot_read_address(address);
            self.ram.read8(self.remapped_ram_address(address))
        }
    }

    fn read16_inner(&mut self, address: u32) -> Result<u16, BusError> {
        if self.mmio.contains_key(&address) || self.mmio.contains_key(&address.wrapping_add(1)) {
            let bytes = [
                self.read8_inner(address)?,
                self.read8_inner(address.wrapping_add(1))?,
            ];
            return Ok(u16::from_be_bytes(bytes));
        }
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read16(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read16(address);
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.read16(address);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.read16(address);
        }
        if let Some(intc2) = self.intc2_for(address) {
            return intc2.read16(address);
        }
        let bytes = [
            self.read8_inner(address)?,
            self.read8_inner(address.wrapping_add(1))?,
        ];
        Ok(u16::from_be_bytes(bytes))
    }

    fn read32_inner(&mut self, address: u32) -> Result<u32, BusError> {
        if (0..4).any(|offset| self.mmio.contains_key(&address.wrapping_add(offset))) {
            let bytes = [
                self.read8_inner(address)?,
                self.read8_inner(address.wrapping_add(1))?,
                self.read8_inner(address.wrapping_add(2))?,
                self.read8_inner(address.wrapping_add(3))?,
            ];
            return Ok(u32::from_be_bytes(bytes));
        }
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read32(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read32(address);
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.read32(address);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.read32(address);
        }
        if let Some(intc2) = self.intc2_for(address) {
            return intc2.read32(address);
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
        if let Some(pit) = self.pit_for(address) {
            return pit.write8(address, value);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.write8(address, value);
        }
        if let Some(intc2) = self.intc2_for(address) {
            return intc2.write8(address, value);
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
            self.ram.write8(self.remapped_ram_address(address), value)
        }
    }

    fn write16_inner(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write16(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write16(address, value);
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.write16(address, value);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.write16(address, value);
        }
        if let Some(intc2) = self.intc2_for(address) {
            return intc2.write16(address, value);
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
        if let Some(pit) = self.pit_for(address) {
            return pit.write32(address, value);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.write32(address, value);
        }
        if let Some(intc2) = self.intc2_for(address) {
            return intc2.write32(address, value);
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

    fn attach_pit_bootstrap(&mut self) {
        self.attach_pit_bootstrap_with_ticks(1);
    }

    fn attach_pit_bootstrap_with_ticks(&mut self, ticks_per_instruction: u32) {
        self.pit = Some(PitController::with_ticks_per_instruction(
            ticks_per_instruction,
        ));
    }

    fn attach_intc2(&mut self) {
        self.intc2 = Some(Intc2::new());
    }

    fn attach_uart8_tx(&mut self) {
        self.uart8 = Some(Uart8::new());
    }

    fn ddr_for(&mut self, address: u32) -> Option<&mut DdrController> {
        self.ddr
            .as_mut()
            .filter(|_| DdrController::contains(address))
    }

    fn pit_for(&mut self, address: u32) -> Option<&mut PitController> {
        self.pit
            .as_mut()
            .filter(|_| PitController::contains(address))
    }

    fn intc2_for(&mut self, address: u32) -> Option<&mut Intc2> {
        self.intc2.as_mut().filter(|_| Intc2::contains(address))
    }

    fn uart8_for(&mut self, address: u32) -> Option<&mut Uart8> {
        self.uart8.as_mut().filter(|_| Uart8::contains(address))
    }

    fn unsupported_device_behavior(&self) -> Option<String> {
        self.dspi0.as_ref().and_then(Dspi::unsupported)
    }

    fn load_ram(&mut self, address: u32, bytes: &[u8]) -> Result<(), String> {
        for (offset, byte) in bytes.iter().copied().enumerate() {
            self.write8_inner(address.wrapping_add(offset as u32), byte)
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
            self.ram
                .write32(self.remapped_ram_address(address), value)
        };
        result.map_err(|error| format!("RAM seed outside mapped memory at {:#010x}", error.addr))
    }

    fn dump_ram(&mut self, address: u32, length: usize) -> Result<Vec<u8>, BusError> {
        (0..length)
            .map(|offset| {
                let current = address.wrapping_add(offset as u32);
                if let Some(sram) = self.sram_for(current) {
                    sram.read8(current)
                } else {
                    self.ram.read8(self.remapped_ram_address(current))
                }
            })
            .collect()
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
    fn begin_instruction(&mut self, pc: u32) {
        self.current_pc = pc;
    }

    fn stop_before_instruction(&mut self, pc: u32) -> bool {
        self.stop_pc == Some(pc)
    }

    fn report_instruction_state(&mut self, pc: u32, d: &[u32; 8], a: &[u32; 8], sr: u16) {
        let trace_pc_matches = self
            .trace_pc_range
            .is_none_or(|(start, end)| (start..end).contains(&pc));
        if trace_pc_matches
            && self.first_a0_range_trace.is_none()
            && self
                .trace_a0_range
                .is_some_and(|(start, end)| (start..end).contains(&a[0]))
        {
            self.first_a0_range_trace = Some(RegisterTrace {
                pc,
                d: *d,
                a: *a,
                sr,
            });
        }
        if self.trace_register_pc != Some(pc) {
            return;
        }
        if self.register_trace.len() == MAX_REGISTER_TRACE {
            self.register_trace.pop_front();
        }
        self.register_trace.push_back(RegisterTrace {
            pc,
            d: *d,
            a: *a,
            sr,
        });
    }

    fn begin_instruction_fetch(&mut self) {
        self.instruction_fetch = true;
    }

    fn end_instruction_fetch(&mut self) {
        self.instruction_fetch = false;
    }

    fn report_tst(&mut self, pc: u32, operand: &str, value: u32, size_bytes: u8, sr: u16) {
        self.last_tst = Some(TstDiagnostic {
            pc,
            operand: operand.to_string(),
            value,
            size_bytes,
            sr,
        });
    }

    fn report_branch(&mut self, pc: u32, condition: u8, taken: bool, target: u32, sr: u16) {
        self.last_branch = Some(BranchDiagnostic {
            pc,
            condition,
            taken,
            target,
            sr,
        });
    }

    fn report_rte(
        &mut self,
        pc: u32,
        frame_sp: u32,
        format_vector: u16,
        restored_sr: u16,
        restored_pc: u32,
    ) {
        if self.rte_history.len() == MAX_RTE_HISTORY {
            self.rte_history.pop_front();
        }
        self.rte_count += 1;
        self.rte_history.push_back(RteDiagnostic {
            pc,
            frame_sp,
            format_vector,
            restored_sr,
            restored_pc,
        });
    }

    fn fast_forward_loop(&mut self, context: LoopContext<'_>) -> usize {
        if !self.loop_detection {
            return 0;
        }
        let LoopContext {
            from_pc,
            pc,
            d,
            a,
            sr,
            instructions: instruction_count,
            remaining,
        } = context;
        self.current_instruction_count = instruction_count;
        let Some(previous) = self.loop_observations.get(&pc).copied() else {
            if self.loop_observations.len() < MAX_LOOP_TRACKED_PCS {
                self.loop_observations.insert(
                    pc,
                    LoopObservation {
                        d: *d,
                        a: *a,
                        sr,
                        instruction_count,
                        repeats: 0,
                        stable_repeats: 0,
                    },
                );
            }
            return 0;
        };
        let period = instruction_count.saturating_sub(previous.instruction_count);
        if period == 0 {
            return 0;
        }
        let same_state = previous.d == *d && previous.a == *a && previous.sr == sr;
        let stable_repeats = if same_state {
            previous.stable_repeats.saturating_add(1)
        } else {
            0
        };
        let repeats = previous.repeats.saturating_add(1);
        let start_pc = pc;
        let end_pc = from_pc;
        let accesses: Vec<_> = self
            .loop_access_history
            .iter()
            .filter(|access| {
                access.instruction_count >= previous.instruction_count
                    && access.instruction_count < instruction_count
            })
            .copied()
            .collect();
        let read_addresses: BTreeSet<_> = accesses
            .iter()
            .filter(|access| !access.write)
            .map(|access| access.address)
            .collect();
        let has_pit_read = read_addresses.contains(&PIT1_PCSR);
        let has_other_peripheral_read = read_addresses.iter().any(|address| {
            *address != PIT1_PCSR
                && MCF_PERIPHERAL_WINDOWS
                    .iter()
                    .any(|&(start, end)| (start..end).contains(address))
        });
        let has_writes = accesses.iter().any(|access| access.write);
        let mut modified_registers = Vec::new();
        for index in 0..8 {
            if previous.d[index] != d[index] {
                modified_registers.push(format!("D{index}"));
            }
            if previous.a[index] != a[index] {
                modified_registers.push(format!("A{index}"));
            }
        }
        if previous.sr != sr {
            modified_registers.push("SR".to_string());
        }
        if !self.loop_reported_pcs.contains(&pc)
            && (stable_repeats >= 2 || repeats >= 8)
            && self.loop_reports.len() < 32
        {
            self.loop_reported_pcs.insert(pc);
            self.loop_reports.push(LoopReport {
                pc,
                start_pc,
                end_pc,
                sr,
                repeat_period: period,
                modified_registers,
                read_addresses: read_addresses.into_iter().collect(),
            });
        }
        self.loop_observations.insert(
            pc,
            LoopObservation {
                d: *d,
                a: *a,
                sr,
                instruction_count,
                repeats,
                stable_repeats,
            },
        );

        if !same_state
            || stable_repeats < 2
            || period > 64
            || has_writes
            || !has_pit_read
            || has_other_peripheral_read
        {
            return 0;
        }
        let Some(pit) = self.pit.as_mut() else {
            return 0;
        };
        let Some(ticks_until_pif) = pit.ticks_until_pif(1) else {
            return 0;
        };
        let ticks_per_loop = (period as u64) * u64::from(pit.ticks_per_instruction());
        if ticks_per_loop == 0 {
            return 0;
        }
        let loops_to_skip =
            ((ticks_until_pif - 1) / ticks_per_loop).min((remaining / period) as u64);
        let skipped = (loops_to_skip as usize) * period;
        if skipped == 0 {
            return 0;
        }
        pit.fast_forward_ticks((skipped as u64) * u64::from(pit.ticks_per_instruction()));
        self.loop_observations
            .get_mut(&pc)
            .unwrap()
            .instruction_count = instruction_count.saturating_add(skipped);
        self.current_instruction_count = instruction_count.saturating_add(skipped);
        skipped
    }

    fn read8(&mut self, address: u32) -> Result<u8, BusError> {
        let source = self.source_for_access(address, 1);
        match self.read8_inner(address) {
            Ok(value) => {
                self.record_loop_access(address, false);
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
                self.record_loop_access(address, false);
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
                self.record_loop_access(address, false);
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
        self.snapshot_source_on_write(address, 1)?;
        self.record_watched_write(address, 1, u32::from(value));
        self.record_loop_access(address, true);
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
        self.snapshot_source_on_write(address, 2)?;
        self.record_watched_write(address, 2, u32::from(value));
        self.record_loop_access(address, true);
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
        self.snapshot_source_on_write(address, 4)?;
        self.record_watched_write(address, 4, value);
        self.record_loop_access(address, true);
        self.record_access(AccessEvent {
            address,
            width: 32,
            write: true,
            value,
            source,
        });
        Ok(())
    }

    fn pending_interrupt(&mut self) -> Option<InterruptRequest> {
        let pit0_pending = self
            .pit
            .as_ref()
            .is_some_and(|pit| pit.interrupt_pending(0));
        let pending_sources = if pit0_pending { 1u64 << 13 } else { 0 };
        let request = self.intc2.as_ref()?.pending_interrupt(pending_sources)?;
        if request.source == 13 && self.pit0_oneshot {
            if self.pit0_request_seen {
                return None;
            }
            self.pit0_request_seen = true;
        }
        Some(request)
    }

    fn advance_instruction(&mut self) {
        if let Some(pit) = &mut self.pit {
            pit.advance_instruction();
        }
    }
}

fn print_stop(stop: RunStop) {
    match stop {
        RunStop::InstructionLimit => println!("stop: instruction limit"),
        RunStop::Breakpoint { pc } => println!("stop: breakpoint before instruction at {pc:#010x}"),
        RunStop::Halt { pc } => println!("HALT executed at PC={pc:#010x}"),
        RunStop::Error(CpuError::Illegal { pc, op }) => {
            println!("stop: unsupported opcode {op:#06x} at {pc:#010x}");
        }
        RunStop::Error(CpuError::Halt { pc }) => {
            println!("HALT executed at PC={pc:#010x}");
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

fn branch_condition_name(condition: u8) -> &'static str {
    match condition & 0xF {
        0 => "T",
        1 => "BSR",
        2 => "HI",
        3 => "LS",
        4 => "CC",
        5 => "CS",
        6 => "NE",
        7 => "EQ",
        8 => "VC",
        9 => "VS",
        10 => "PL",
        11 => "MI",
        12 => "GE",
        13 => "LT",
        14 => "GT",
        _ => "LE",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dt2_core::uart::UART8_BASE;

    #[test]
    fn nor_image_places_container_in_erased_flash_and_checks_bounds() {
        let payload = [0x45, 0x4C, 0x45, 0x33];
        let image = nor_image_with_container(&payload, 0x20).unwrap();

        assert_eq!(image.len(), W25Q128_SIZE);
        assert!(image[..0x20].iter().all(|byte| *byte == 0xFF));
        assert_eq!(&image[0x20..0x24], &payload);
        assert_eq!(image[0x24], 0xFF);
        assert!(nor_image_with_container(&payload, (W25Q128_SIZE - 2) as u32).is_err());
    }

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
    fn uart_records_render_as_tagged_tiles_with_msb_left_pixels() {
        let mut bytes =
            vec![0; UART_RECORD_HEADER_BYTES + UART_RECORD_BYTES * UART_RECORD_COUNT + 1];
        let mut offset = UART_RECORD_HEADER_BYTES;
        for group in 0..2 {
            for y in 0..UART_TILE_ROWS {
                for column in 0..4 {
                    let tag = 0x10 + group * 4 + column;
                    bytes[offset] = tag as u8;
                    bytes[offset + 1] = (y * UART_TILE_SIZE) as u8;
                    if group == 0 && y == 0 && column == 0 {
                        bytes[offset + 2] = 0x80;
                    }
                    if group == 1 && y == 0 && column == 0 {
                        bytes[offset + 2] = 0x01;
                    }
                    offset += UART_RECORD_BYTES;
                }
            }
        }
        *bytes.last_mut().unwrap() = 0xB8;

        let image = uart_records_bitmap_ppm(&bytes).unwrap();
        let header = format!(
            "P6\n{} {}\n255\n",
            UART_TILE_COLUMNS * UART_TILE_SIZE * UART_BITMAP_SCALE,
            UART_TILE_ROWS * UART_TILE_SIZE * UART_BITMAP_SCALE
        );
        let pixels = &image[header.len()..];
        let pixel_at = |x: usize, y: usize| {
            let offset = (y * UART_TILE_COLUMNS * UART_TILE_SIZE * UART_BITMAP_SCALE + x) * 3;
            &pixels[offset..offset + 3]
        };

        assert!(image.starts_with(header.as_bytes()));
        assert_eq!(pixel_at(0, 0), [255, 255, 255]);
        assert_eq!(pixel_at(4, 0), [0, 0, 0]);
        assert_eq!(pixel_at((32 + 7) * UART_BITMAP_SCALE, 0), [255, 255, 255]);
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
    fn interrupt_injection_spec_validates_count_level_and_vector() {
        assert_eq!(
            parse_interrupt_injection("39000000:1:205").unwrap(),
            InterruptInjection {
                instruction_count: 39_000_000,
                level: 1,
                vector: 205,
            }
        );
        assert!(parse_interrupt_injection("10:0:205").is_err());
        assert!(parse_interrupt_injection("10:8:205").is_err());
        assert!(parse_interrupt_injection("10:1:205:extra").is_err());
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
    fn seeded_mmio_overrides_modeled_ddr_status_on_reads() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.attach_ddr_bootstrap();
        assert_eq!(bus.read32(0xFC0B_806C).unwrap(), 0);

        bus.seed_mmio32("0xFC0B806C=0x00000008").unwrap();

        assert_eq!(bus.read32(0xFC0B_806C).unwrap(), 0x8);
        assert_eq!(
            bus.access_events.last().unwrap().source,
            AccessSource::Seeded
        );
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
    fn pit_bootstrap_model_releases_pif_poll_without_default_synthesis() {
        let mut synthetic = ProbeBus::new(FlatRam::new(0, 16));
        synthetic.enable_synthetic_mcf_mmio();
        synthetic.write16(0xFC08_4000, 0x0033).unwrap();
        assert_eq!(synthetic.read16(0xFC08_4000).unwrap(), 0);

        let mut modeled = ProbeBus::new(FlatRam::new(0, 16));
        modeled.enable_synthetic_mcf_mmio();
        modeled.attach_pit_bootstrap();
        modeled.write16(0xFC08_4002, 0x0083).unwrap();
        modeled.write16(0xFC08_4000, 0x0033).unwrap();
        for _ in 0..=u16::MAX {
            modeled.advance_instruction();
        }

        assert_ne!(modeled.read16(0xFC08_4000).unwrap() & 0x0004, 0);
        assert_eq!(modeled.access_events[0].source, AccessSource::Modeled);
    }

    #[test]
    fn uart8_tx_model_sets_txready_and_captures_output_bytes() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.enable_synthetic_mcf_mmio();
        bus.attach_uart8_tx();

        bus.write8(UART8_BASE + 8, 0x04).unwrap();
        assert_eq!(bus.read8(UART8_BASE + 4).unwrap() & 0x04, 0x04);
        bus.write8(UART8_BASE + 0x0C, b'A').unwrap();

        assert_eq!(bus.uart8.as_ref().unwrap().transmitted(), 1);
        assert_eq!(bus.uart8.as_ref().unwrap().captured(), b"A");
        assert!(bus
            .access_events
            .iter()
            .all(|event| event.source == AccessSource::Modeled));
    }

    #[test]
    fn cpu_retirement_ticks_pit_and_exits_pif_poll() {
        let mut bus = ProbeBus::new(FlatRam::new(0x1000, 0x100));
        bus.attach_pit_bootstrap();
        bus.write16(0xFC08_0000, 0x0013).unwrap();
        bus.write16(0xFC08_0002, 2).unwrap();
        bus.load_ram(
            0x1000,
            &[0x30, 0x10, 0x08, 0x00, 0x00, 0x02, 0x67, 0x00, 0xFF, 0xF8],
        )
        .unwrap();
        let mut cpu = Cpu::new();
        cpu.pc = 0x1000;
        cpu.a[0] = 0xFC08_0000;

        let report = cpu.run(&mut bus, 10);

        assert_eq!(report.cpu.pc, 0x100C);
        assert_eq!(
            report.stop,
            RunStop::Error(CpuError::Illegal { pc: 0x100A, op: 0 })
        );
        assert_eq!(bus.read16(0xFC08_0000).unwrap() & 0x0004, 0x0004);
    }

    #[test]
    fn stop_pc_breakpoint_stops_before_instruction_execution() {
        let mut bus = ProbeBus::new(FlatRam::new(0x1000, 0x100));
        bus.load_ram(0x1000, &[0x4E, 0x71]).unwrap();
        bus.set_stop_pc(0x1000);
        let mut cpu = Cpu::new();
        cpu.pc = 0x1000;

        let report = cpu.run(&mut bus, 10);

        assert_eq!(report.instructions, 0);
        assert_eq!(report.cpu.pc, 0x1000);
        assert_eq!(report.stop, RunStop::Breakpoint { pc: 0x1000 });
    }

    #[test]
    fn natural_pit0_interrupt_uses_intc2_vector_and_returns_with_rte() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 0x3000));
        bus.attach_pit_bootstrap();
        bus.attach_intc2();
        bus.write16(0xFC08_0000, 0x0019).unwrap();
        bus.write16(0xFC08_0002, 0).unwrap();
        bus.write8(0xFC05_004D, 1).unwrap();
        bus.write8(0xFC05_001D, 13).unwrap();
        bus.write32(0xCD * 4, 0x1100).unwrap();
        bus.load_ram(0x1000, &[0x4E, 0x71, 0x60, 0xFE]).unwrap();
        bus.load_ram(
            0x1100,
            &[0x33, 0xFC, 0x00, 0x04, 0xFC, 0x08, 0x00, 0x00, 0x4E, 0x73],
        )
        .unwrap();
        let mut cpu = Cpu::new();
        cpu.pc = 0x1000;
        cpu.a[7] = 0x2000;
        cpu.sr = 0x2000;

        let report = cpu.run(&mut bus, 6);

        assert_eq!(
            report.natural_interrupt,
            Some(dt2_core::coldfire::InterruptDelivery {
                instruction_count: 1,
                source: 13,
                level: 1,
                vector: 205,
            })
        );
        assert_eq!(report.cpu.pc, 0x1002);
        assert_eq!(report.cpu.a[7], 0x2000);
        assert_eq!(report.cpu.sr, 0x2000);
        assert_eq!(bus.read16(0xFC08_0000).unwrap() & 0x0004, 0);
    }

    #[test]
    fn pit0_oneshot_suppresses_only_repeated_pit0_requests() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 0x100));
        bus.attach_pit_bootstrap();
        bus.attach_intc2();
        bus.enable_pit0_oneshot();
        bus.write16(0xFC08_0000, 0x0019).unwrap();
        bus.write16(0xFC08_0002, 0).unwrap();
        bus.write8(0xFC05_004D, 1).unwrap();
        bus.write8(0xFC05_001D, 13).unwrap();
        bus.advance_instruction();

        assert_eq!(
            bus.pending_interrupt(),
            Some(InterruptRequest {
                source: 13,
                level: 1,
                vector: 205,
            })
        );

        bus.write16(0xFC08_0002, 0).unwrap();
        bus.advance_instruction();
        assert_eq!(bus.pending_interrupt(), None);
    }

    #[test]
    fn write_watch_records_first_cpu_write_overlapping_address() {
        let mut bus = ProbeBus::new(FlatRam::new(0x1000, 0x100));
        bus.watch_write(0x1002);
        bus.begin_instruction(0x2000);
        bus.write32(0x1000, 0xAABB_CCDD).unwrap();
        bus.begin_instruction(0x2004);
        bus.write8(0x1002, 0x11).unwrap();

        assert_eq!(bus.watched_write_count, 2);
        assert_eq!(bus.watched_writes.len(), 2);
        assert_eq!(bus.watched_writes[0].pc, 0x2000);
        assert_eq!(bus.watched_writes[0].address, 0x1000);
        assert_eq!(bus.watched_writes[0].width_bits, 32);
        assert_eq!(bus.watched_writes[0].value, 0xAABB_CCDD);
    }

    #[test]
    fn register_trace_keeps_most_recent_selected_pc_snapshots() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.trace_registers_at(0x1000);
        let mut data_registers = [0; 8];
        let address_registers = [0; 8];
        for value in 0..=MAX_REGISTER_TRACE as u32 {
            data_registers[0] = value;
            bus.report_instruction_state(0x1000, &data_registers, &address_registers, 0);
        }

        assert_eq!(bus.register_trace.len(), MAX_REGISTER_TRACE);
        assert_eq!(bus.register_trace.front().unwrap().d[0], 1);
        assert_eq!(bus.register_trace.back().unwrap().d[0], 16);
    }

    #[test]
    fn register_trace_captures_first_a0_threshold_crossing() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.trace_registers_at(0x1000);
        bus.trace_first_a0_in_range((0x2000, 0x2002));
        let data_registers = [0xAABB_CCDD; 8];
        let mut address_registers = [0; 8];
        bus.trace_pc_range((0x1000, 0x1001));
        bus.report_instruction_state(0x1001, &data_registers, &address_registers, 0x2020);
        address_registers[0] = 0x1FFF;
        bus.report_instruction_state(0x1000, &data_registers, &address_registers, 0x2000);
        address_registers[0] = 0x2000;
        bus.report_instruction_state(0x1000, &data_registers, &address_registers, 0x2010);
        address_registers[0] = 0x2001;
        bus.report_instruction_state(0x1000, &data_registers, &address_registers, 0x2011);

        let crossing = bus.first_a0_range_trace.as_ref().unwrap();
        assert_eq!(crossing.a[0], 0x2000);
        assert_eq!(crossing.sr, 0x2010);
        assert_eq!(crossing.d[0], 0xAABB_CCDD);
    }

    #[test]
    fn ram_remap_aliases_only_the_configured_window() {
        let mut bus = ProbeBus::new(FlatRam::new(0x4000_0000, 0x0800_0000));
        bus.set_ram_remap(RamRemap {
            source: 0x4020_0000,
            destination: 0x4400_0000,
            length: 0x0020_0000,
        });
        bus.write32(0x4020_0010, 0xAABB_CCDD).unwrap();
        bus.write32(0x403F_FFFC, 0x1234_5678).unwrap();

        assert_eq!(bus.read32(0x4400_0010).unwrap(), 0xAABB_CCDD);
        assert_eq!(bus.read32(0x403F_FFFC).unwrap(), 0x1234_5678);
        assert_eq!(bus.dump_ram(0x4020_0010, 4).unwrap(), [0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(bus.read32(0x4020_0000).unwrap(), 0);
        assert_eq!(bus.read32(0x4040_0000).unwrap(), 0);
    }

    #[test]
    fn ram_snapshot_remaps_decoder_reads_after_final_source_write() {
        let mut bus = ProbeBus::new(FlatRam::new(0x4000_0000, 0x0800_0000));
        bus.set_ram_snapshot(RamSnapshot {
            watch: 0x4020_000F,
            source: 0x4020_0000,
            destination: 0x4400_0000,
            length: 16,
            trigger_pc_start: 0x8000_4980,
            trigger_pc_end: 0x8000_4984,
            read_pc_start: 0x8000_5800,
            read_pc_end: 0x8000_5880,
            active: false,
        });
        for offset in 0..16 {
            bus.begin_instruction(0x8000_4982);
            bus.write8(0x4020_0000 + offset, offset as u8 + 1)
                .unwrap();
        }

        bus.begin_instruction(0x8000_5800);
        assert_eq!(bus.read8(0x4020_0000).unwrap(), 1);
        bus.begin_instruction(0x8000_5800);
        bus.write8(0x4020_0000, 0xFF).unwrap();

        assert_eq!(bus.dump_ram(0x4020_0000, 1).unwrap(), [0xFF]);
        assert_eq!(bus.dump_ram(0x4400_0000, 16).unwrap(), (1..=16).collect::<Vec<_>>());
        bus.begin_instruction(0x8000_5890);
        assert_eq!(bus.read8(0x4020_0000).unwrap(), 0xFF);
    }

    #[test]
    fn natural_pit0_interrupt_respects_intc2_source_mask() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 0x3000));
        bus.attach_pit_bootstrap();
        bus.attach_intc2();
        bus.write16(0xFC08_0000, 0x0019).unwrap();
        bus.write16(0xFC08_0002, 0).unwrap();
        bus.write8(0xFC05_004D, 1).unwrap();
        bus.load_ram(0x1000, &[0x4E, 0x71, 0x60, 0xFE]).unwrap();
        let mut cpu = Cpu::new();
        cpu.pc = 0x1000;
        cpu.a[7] = 0x2000;
        cpu.sr = 0x2000;

        let report = cpu.run(&mut bus, 3);

        assert_eq!(report.natural_interrupt, None);
        assert_eq!(report.cpu.pc, 0x1002);
        assert_ne!(bus.read16(0xFC08_0000).unwrap() & 0x0004, 0);
    }

    #[test]
    fn loop_detector_fast_forwards_stable_pit_poll() {
        let mut bus = ProbeBus::new(FlatRam::new(0x1000, 0x100));
        bus.attach_pit_bootstrap();
        bus.enable_loop_detection();
        bus.write16(0xFC08_4000, 0x0013).unwrap();
        bus.write16(0xFC08_4002, 0x0083).unwrap();
        bus.load_ram(
            0x1000,
            &[
                0x32, 0x39, 0xFC, 0x08, 0x40, 0x00, 0x74, 0x04, 0xC2, 0x82, 0x4A, 0x41, 0x67, 0xF2,
                0x4E, 0x71, 0x60, 0xFE,
            ],
        )
        .unwrap();
        let mut cpu = Cpu::new();
        cpu.pc = 0x1000;

        let report = cpu.run(&mut bus, 200);

        assert!(report.fast_forwarded_instructions > 0);
        assert!(report.instructions + report.fast_forwarded_instructions <= 200);
        assert!(bus
            .loop_reports
            .iter()
            .any(|report| { report.pc == 0x1000 && report.read_addresses == [PIT1_PCSR] }));
        assert_ne!(bus.read16(0xFC08_4000).unwrap() & 0x0004, 0);
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
    fn mmio_access_summary_tracks_first_pc_counts_and_synthetic_source() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.enable_synthetic_mcf_mmio();
        bus.begin_instruction(0x1000);
        bus.write8(0xFC00_0000, 0x12).unwrap();
        bus.begin_instruction(0x1004);
        let last_value = bus.read8(0xFC00_0000).unwrap();

        let summary = bus.access_summaries.get(&0xFC00_0000).unwrap();
        assert_eq!(summary.first_pc, 0x1000);
        assert_eq!(summary.reads, 1);
        assert_eq!(summary.writes, 1);
        assert_eq!(summary.last_value, u32::from(last_value));
        assert!(summary.unmodeled);
    }

    #[test]
    fn image_range_may_start_inside_ram_but_must_fit() {
        assert!(range_fits(0x8000_0000, 0x20000, 0x8000_03FC, 0x765E));
        assert!(!range_fits(0x8000_0000, 0x1000, 0x8000_0FFC, 8));
        assert!(!range_fits(0x8000_1000, 0x1000, 0x8000_0FFC, 4));
    }

    #[test]
    fn ram_dump_spec_enforces_bounds_and_address_overflow() {
        assert_eq!(
            parse_ram_dump_spec("0x40001000:32").unwrap(),
            (0x4000_1000, 32)
        );
        assert!(parse_ram_dump_spec("0x40001000:0").is_err());
        assert!(parse_ram_dump_spec("0x40001000:257").is_err());
        assert!(parse_ram_dump_spec("0xfffffff0:32").is_err());
    }

    #[test]
    fn ram_file_dump_spec_enforces_size_path_and_address_bounds() {
        assert_eq!(
            parse_ram_file_dump_spec("0x40001000:0x31fb60:out/section3.bin").unwrap(),
            (0x4000_1000, 0x31fb60, "out/section3.bin".to_string())
        );
        assert_eq!(
            parse_ram_file_dump_spec("0x40001000:16:/tmp/ram:section3.bin")
                .unwrap()
                .2,
            "/tmp/ram:section3.bin"
        );
        assert!(parse_ram_file_dump_spec("0x40001000:0:out.bin").is_err());
        assert!(parse_ram_file_dump_spec("0x40001000:0x4000001:out.bin").is_err());
        assert!(parse_ram_file_dump_spec("0xfffffff0:32:out.bin").is_err());
        assert!(parse_ram_file_dump_spec("0x40001000:32:").is_err());
    }

    #[test]
    fn ram_dump_reads_flat_and_sram_memory_but_not_mmio() {
        let mut bus = ProbeBus::new(FlatRam::new(0x4000_0000, 0x1000));
        bus.attach_mcf_sram_overlay();
        bus.seed_ram32("0x40000004=0x12345678").unwrap();
        bus.seed_ram32("0x80000000=0xAABBCCDD").unwrap();

        assert_eq!(
            bus.dump_ram(0x4000_0004, 4).unwrap(),
            [0x12, 0x34, 0x56, 0x78]
        );
        assert_eq!(
            bus.dump_ram(0x8001_0000, 4).unwrap(),
            [0xAA, 0xBB, 0xCC, 0xDD]
        );
        assert!(bus.dump_ram(0xFC04_8000, 4).is_err());
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

        bus.load_ram(0x8000_03FC, &[0x4E, 0x71]).unwrap();
        bus.seed_ram32("0x80000000=0x12345678").unwrap();
        bus.write32(0x4000_0000, 0xAABB_CCDD).unwrap();

        assert_eq!(bus.read32(0x8001_0000).unwrap(), 0x1234_5678);
        assert_eq!(bus.read16(0x8000_03FC).unwrap(), 0x4E71);
        assert_eq!(bus.read32(0x4000_0000).unwrap(), 0xAABB_CCDD);
        assert!(bus.seed_ram32("0x90000000=1").is_err());
    }

    #[test]
    fn mcf_sram_overlay_can_map_the_sbf_reset_alias() {
        let mut bus = ProbeBus::new(FlatRam::new(0x4000_0000, 0x0800_0000));
        bus.attach_mcf_sram_overlay_with_sbf_alias(true);
        bus.write32(0, 0x8001_0000).unwrap();
        bus.write32(4, 0x8000_0DE8).unwrap();

        assert_eq!(bus.read32(0x8000_0000).unwrap(), 0x8001_0000);
        assert_eq!(bus.read32(0x8000_0004).unwrap(), 0x8000_0DE8);
        assert_eq!(bus.read32(0x4000_0000).unwrap(), 0);
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
