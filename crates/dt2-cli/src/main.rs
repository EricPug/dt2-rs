use dt2_core::{
    bus::{Bus, BusError, FlatRam, InterruptRequest, LoopContext, Mcf5441xSram, TstOperand},
    coldfire::{Cpu, CpuError, InterruptInjection, RunStop},
    ddr::DdrController,
    dspi::Dspi,
    dtim1::{DmaTimer1, DTIM3_BASE},
    emmc::{Esdhc, ESDHC_BASE, ESDHC_DATPORT},
    intc0::Intc0,
    intc2::Intc2,
    pit::PitController,
    rgpio::Rgpio,
    spi_nor::W25q128jv,
    uart::Uart8,
};
use dt2_fw::{aplib, container, syx};

use dt2_core::display;

#[cfg_attr(not(feature = "window"), allow(dead_code))]
mod panel_ui;
#[cfg(dt2_unicorn)]
mod unicorn_tier;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "run-bin") {
        run_binary(&args[1..]);
        return;
    }
    if args.first().is_some_and(|arg| arg == "trace-depack") {
        trace_depack(&args[1..]);
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
        eprintln!("       dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc|SBF_RESET_VECTOR> <instruction-limit> [MCF5441X_SRAM|MCF5441X_SBF_BOOT MCF5441X_SRAM_OVERLAY DDR_BOOTSTRAP PIT_BOOTSTRAP PIT_FIRST_TICK_DELAY=n INTC2_BOOTSTRAP DTIM1_BOOTSTRAP DTIM1_TICKS_PER_INSTRUCTION=n DTIM3_BOOTSTRAP DTIM3_TICKS_PER_INSTRUCTION=n DSPI1_BOOTSTRAP DSPI2_BOOTSTRAP ESDHC_EMMC ESDHC_FILE=path ESDHC_TRACE EDMA_TRACE DSP_LINK_REPLY=word PIT0_ONESHOT PIT2_PERIODIC PIT_TICKS_PER_INSTRUCTION=n LOOP_DETECT SCHED_TRACE CONVERT_TRACE STOP_PC=pc INJECT_IRQ=count:level:vector UART8_TX UART8_TX_READY UART8_RX=hex PANEL_STUB SYNTHETIC_MCF_MMIO INSPECT_TRAP0 DUMP_RAM=address:length DUMP_RAM_FILE=address:length:path WATCH_WRITE=address WATCH_READ=address|start:end WRITE_TRACE=start:end TRACE_REGS=pc TRACE_TAIL=n TRACE_PC_RANGE=start:end TRACE_A0_RANGE=start:end RAM_REMAP=from:to:length RAM_SNAPSHOT=watch:src:dst:length:trigger_start:trigger_end:read_start:read_end D0=value A0=value SR=value RAM32@address=value MMIO8@address=value MMIO16@address=value MMIO32@address=value ...]");
        eprintln!("       dt2-cli render-pattern <output.ppm>");
        eprintln!("       dt2-cli build-flash <firmware.syx> <output.bin> [container-offset]");
        eprintln!("       dt2-cli render-uart-records <run.log> <output.ppm>");
        eprintln!("       run-bin option: SSI_TX_RAW=<path> captures channel-50 SSI TX DMA bytes");
        eprintln!("       dt2-cli trace-depack <packed-section.bin>");
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

fn trace_depack(args: &[String]) {
    let Some(path) = args.first() else {
        eprintln!("usage: dt2-cli trace-depack <packed-section.bin>");
        std::process::exit(2)
    };
    let stored = std::fs::read(path).unwrap_or_else(|error| {
        eprintln!("cannot read {path}: {error}");
        std::process::exit(2)
    });
    if stored.len() < 8 {
        eprintln!("packed section is shorter than its length/checksum header");
        std::process::exit(2)
    }
    let payload_length = u32::from_be_bytes(stored[..4].try_into().unwrap()) as usize;
    let payload_end = 8usize.checked_add(payload_length).unwrap_or_else(|| {
        eprintln!("packed section length overflows address space");
        std::process::exit(2)
    });
    let Some(payload) = stored.get(8..payload_end) else {
        eprintln!(
            "packed section header declares {payload_length:#x} input bytes, but file is truncated"
        );
        std::process::exit(2)
    };
    let checkpoints = if args.len() >= 3 {
        let step = parse_address(&args[1]).unwrap_or_else(|error| {
            eprintln!("invalid checkpoint step: {error}");
            std::process::exit(2)
        }) as usize;
        let end = parse_address(&args[2]).unwrap_or_else(|error| {
            eprintln!("invalid checkpoint end: {error}");
            std::process::exit(2)
        }) as usize;
        let start = args
            .get(3)
            .map(|value| {
                parse_address(value).unwrap_or_else(|error| {
                    eprintln!("invalid checkpoint start: {error}");
                    std::process::exit(2)
                }) as usize
            })
            .unwrap_or(0);
        if step == 0 || start > end || end > 0x31FB60 {
            eprintln!("checkpoint step must be nonzero and start/end must fit section 3");
            std::process::exit(2)
        }
        (start..=end).step_by(step).collect::<Vec<_>>()
    } else {
        vec![0x10_0000, 0x20_0000, 0x30_0000]
    };
    let trace = aplib::depack_with_checkpoints(payload, &checkpoints).unwrap_or_else(|error| {
        eprintln!("host aPLib decode failed: {error:?}");
        std::process::exit(1)
    });
    println!(
        "stored={} bytes payload={} bytes decoded={} bytes used={} bytes",
        stored.len(),
        payload_length,
        trace.output.len(),
        trace.used
    );
    for (output_offset, input_offset) in trace.checkpoints {
        println!(
            "host output_offset={output_offset:#x} input_offset={input_offset:#x} input_address={:#010x}",
            0x4020_0008u32.wrapping_add(input_offset as u32)
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

fn parse_uart_rx_bytes(hex: &str) -> Result<Vec<u8>, String> {
    if hex.is_empty() || hex.len() % 2 != 0 {
        return Err("UART8_RX must contain a non-empty even number of hex digits".to_string());
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let byte = std::str::from_utf8(pair).expect("ASCII hex input");
            u8::from_str_radix(byte, 16).map_err(|_| format!("invalid UART8_RX hex byte: {byte}"))
        })
        .collect()
}

fn panel_stub_handshake_reply() -> &'static [u8] {
    &[0x70, 0x0B, 0x00, 0x00, 0x00]
}

const PANEL_STUB_DISCOVERY_QUERY: [u8; 2] = [0x60, 0x01];

fn panel_stub_input_records() -> &'static [u8] {
    &[0x20, 0x01, 0x30, 0x01, 0x20, 0x00]
}

fn panel_stub_discovery_records(func_held: bool) -> &'static [u8] {
    const NEUTRAL: [u8; 14] = [
        0x20, 0x00, 0x21, 0x00, 0x22, 0x00, 0x23, 0x00, 0x24, 0x00, 0x25, 0x00, 0x26, 0x00,
    ];
    const FUNC_HELD: [u8; 14] = [
        0x20, 0x00, 0x21, 0x00, 0x22, 0x01, 0x23, 0x00, 0x24, 0x00, 0x25, 0x00, 0x26, 0x00,
    ];
    if func_held {
        &FUNC_HELD
    } else {
        &NEUTRAL
    }
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
        eprintln!("usage: dt2-cli run-bin <image.bin> <ram-base> <ram-size> <image-address> <entry-pc|SBF_RESET_VECTOR> <instruction-limit> [MCF5441X_SRAM|MCF5441X_SBF_BOOT MCF5441X_SRAM_OVERLAY DDR_BOOTSTRAP PIT_BOOTSTRAP PIT_FIRST_TICK_DELAY=n INTC2_BOOTSTRAP DTIM1_BOOTSTRAP DTIM1_TICKS_PER_INSTRUCTION=n DTIM3_BOOTSTRAP DTIM3_TICKS_PER_INSTRUCTION=n DSPI2_BOOTSTRAP ESDHC_EMMC ESDHC_TRACE EDMA_TRACE DSP_LINK DSP_LINK_REPLY=word PIT2_PERIODIC PIT_TICKS_PER_INSTRUCTION=n LOOP_DETECT SCHED_TRACE CONVERT_TRACE INJECT_IRQ=count:level:vector UART8_TX UART8_TX_READY UART8_RX=hex PANEL_STUB SYNTHETIC_MCF_MMIO INSPECT_TRAP0 DUMP_RAM=address:length D0=value A0=value SR=value RAM32@address=value MMIO8@address=value MMIO16@address=value MMIO32@address=value WRITE_TRACE=start:end TRACE_TAIL=n ...]");
        eprintln!("SSI_TX_RAW=<path> captures channel-50 SSI TX DMA bytes");
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
    let dtim1_bootstrap = args[6..]
        .iter()
        .any(|assignment| assignment == "DTIM1_BOOTSTRAP");
    let dtim3_bootstrap = args[6..]
        .iter()
        .any(|assignment| assignment == "DTIM3_BOOTSTRAP");
    let pit2_periodic = args[6..]
        .iter()
        .any(|assignment| assignment == "PIT2_PERIODIC");
    let dspi1_bootstrap = args[6..]
        .iter()
        .any(|assignment| assignment == "DSPI1_BOOTSTRAP");
    let dspi2_bootstrap = args[6..]
        .iter()
        .any(|assignment| assignment == "DSPI2_BOOTSTRAP");
    let dsp_link_reply = args[6..]
        .iter()
        .find_map(|assignment| assignment.strip_prefix("DSP_LINK_REPLY="))
        .map(|value| {
            let parsed = parse_address(value).unwrap_or_else(|error| {
                eprintln!("invalid DSP_LINK_REPLY value: {error}");
                std::process::exit(2)
            });
            u16::try_from(parsed).unwrap_or_else(|_| {
                eprintln!("invalid DSP_LINK_REPLY value: expected a 16-bit word");
                std::process::exit(2)
            })
        })
        .unwrap_or(0xFFFF);
    let esdhc_file = args[6..]
        .iter()
        .find_map(|assignment| assignment.strip_prefix("ESDHC_FILE="))
        .map(str::to_owned);
    let esdhc_emmc = esdhc_file.is_some()
        || args[6..]
            .iter()
            .any(|assignment| assignment == "ESDHC_EMMC");
    let esdhc_trace = args[6..]
        .iter()
        .any(|assignment| assignment == "ESDHC_TRACE");
    let pit0_oneshot = args[6..]
        .iter()
        .any(|assignment| assignment == "PIT0_ONESHOT");
    let loop_detect = args[6..]
        .iter()
        .any(|assignment| assignment == "LOOP_DETECT");
    let scheduler_trace = args[6..]
        .iter()
        .any(|assignment| assignment == "SCHED_TRACE");
    let conversion_trace = args[6..]
        .iter()
        .any(|assignment| assignment == "CONVERT_TRACE");
    let stop_pc = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("STOP_PC=").map(|pc| {
            parse_address(pc).unwrap_or_else(|error| {
                eprintln!("invalid STOP_PC address: {error}");
                std::process::exit(2)
            })
        })
    });
    let watch_write_range = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("WATCH_WRITE=").map(|spec| {
            let parsed = (|| -> Result<(u32, u32), String> {
                if let Some((start, end)) = spec.split_once(':') {
                    Ok((parse_address(start)?, parse_address(end)?))
                } else {
                    let start = parse_address(spec)?;
                    start
                        .checked_add(1)
                        .map(|end| (start, end))
                        .ok_or_else(|| format!("WATCH_WRITE address overflows: {start:#010x}"))
                }
            })();
            let (start, end) = parsed.unwrap_or_else(|error| {
                eprintln!("invalid WATCH_WRITE range: {error}");
                std::process::exit(2)
            });
            if start >= end {
                eprintln!("invalid WATCH_WRITE range: start must be less than exclusive end");
                std::process::exit(2)
            }
            (start, end)
        })
    });
    let write_trace_range = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("WRITE_TRACE=").map(|spec| {
            let (start, end) = spec.split_once(':').unwrap_or_else(|| {
                eprintln!("invalid WRITE_TRACE range: expected start:end instruction offsets");
                std::process::exit(2)
            });
            let start = start.parse::<usize>().unwrap_or_else(|error| {
                eprintln!("invalid WRITE_TRACE start offset: {error}");
                std::process::exit(2)
            });
            let end = end.parse::<usize>().unwrap_or_else(|error| {
                eprintln!("invalid WRITE_TRACE end offset: {error}");
                std::process::exit(2)
            });
            if start >= end {
                eprintln!("invalid WRITE_TRACE range: start must be less than exclusive end");
                std::process::exit(2)
            }
            (start, end)
        })
    });
    let watch_read_range = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("WATCH_READ=").map(|spec| {
            let parsed = (|| -> Result<(u32, u32), String> {
                if let Some((start, end)) = spec.split_once(':') {
                    Ok((parse_address(start)?, parse_address(end)?))
                } else {
                    let start = parse_address(spec)?;
                    start
                        .checked_add(1)
                        .map(|end| (start, end))
                        .ok_or_else(|| format!("WATCH_READ address overflows: {start:#010x}"))
                }
            })();
            let (start, end) = parsed.unwrap_or_else(|error| {
                eprintln!("invalid WATCH_READ range: {error}");
                std::process::exit(2)
            });
            if start >= end {
                eprintln!("invalid WATCH_READ range: start must be less than exclusive end");
                std::process::exit(2)
            }
            (start, end)
        })
    });
    let sample_access_trace_range = args[6..].iter().find_map(|assignment| {
        assignment.strip_prefix("SAMPLE_ACCESS_TRACE=").map(|spec| {
            let (start, end) = spec.split_once(':').unwrap_or_else(|| {
                eprintln!("invalid SAMPLE_ACCESS_TRACE range: expected start:end");
                std::process::exit(2)
            });
            let start = parse_address(start).unwrap_or_else(|error| {
                eprintln!("invalid SAMPLE_ACCESS_TRACE start: {error}");
                std::process::exit(2)
            });
            let end = parse_address(end).unwrap_or_else(|error| {
                eprintln!("invalid SAMPLE_ACCESS_TRACE end: {error}");
                std::process::exit(2)
            });
            if start >= end {
                eprintln!(
                    "invalid SAMPLE_ACCESS_TRACE range: start must be less than exclusive end"
                );
                std::process::exit(2)
            }
            (start, end)
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
    let trace_tail = args[6..]
        .iter()
        .find_map(|assignment| assignment.strip_prefix("TRACE_TAIL="))
        .map(|value| {
            value
                .parse::<usize>()
                .ok()
                .filter(|length| (1..=1024).contains(length))
                .unwrap_or_else(|| {
                    eprintln!("TRACE_TAIL must be an integer from 1 through 1024");
                    std::process::exit(2)
                })
        })
        .unwrap_or(0);
    let trace_a1_checkpoints: Vec<u32> = args[6..]
        .iter()
        .filter_map(|assignment| assignment.strip_prefix("TRACE_A1="))
        .flat_map(|list| list.split(','))
        .map(|address| {
            parse_address(address).unwrap_or_else(|error| {
                eprintln!("invalid TRACE_A1 address: {error}");
                std::process::exit(2)
            })
        })
        .collect();
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
            if source.is_empty()
                || destination.is_empty()
                || length.is_empty()
                || parts.next().is_some()
            {
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
    let pit_first_tick_delay = args[6..]
        .iter()
        .find_map(|assignment| assignment.strip_prefix("PIT_FIRST_TICK_DELAY="))
        .map(|value| match value.parse::<u64>() {
            Ok(ticks) if ticks > 0 => ticks,
            _ => {
                eprintln!("PIT_FIRST_TICK_DELAY must be a positive integer");
                std::process::exit(2)
            }
        })
        .unwrap_or(0);
    if pit_first_tick_delay > 0 && !pit_bootstrap {
        eprintln!("PIT_FIRST_TICK_DELAY requires PIT_BOOTSTRAP");
        std::process::exit(2);
    }
    let dtim1_tick_setting = args[6..]
        .iter()
        .find_map(|assignment| assignment.strip_prefix("DTIM1_TICKS_PER_INSTRUCTION="));
    let dtim1_ticks_per_instruction = match dtim1_tick_setting {
        Some(value) => match value.parse::<u32>() {
            Ok(ticks) if ticks > 0 => ticks,
            _ => {
                eprintln!("DTIM1_TICKS_PER_INSTRUCTION must be a positive integer");
                std::process::exit(2)
            }
        },
        None => 1,
    };
    if dtim1_tick_setting.is_some() && !dtim1_bootstrap {
        eprintln!("DTIM1_TICKS_PER_INSTRUCTION requires DTIM1_BOOTSTRAP");
        std::process::exit(2);
    }
    // The clickable panel defaults to 4: measured by hand, one upward drag then sweeps an encoder to its
    // maximum (16 gave about half the range, 8 a quarter, 64 almost none); see docs/section3-handoff.md.
    let panel_default_ticks =
        (dtim3_bootstrap && args[6..].iter().any(|a| a == "LIVE_PANEL")).then_some("4");
    let dtim3_tick_setting = args[6..]
        .iter()
        .find_map(|assignment| assignment.strip_prefix("DTIM3_TICKS_PER_INSTRUCTION="))
        .or(panel_default_ticks);
    let dtim3_ticks_per_instruction = match dtim3_tick_setting {
        Some(value) => match value.parse::<u32>() {
            Ok(ticks) if ticks > 0 => ticks,
            _ => {
                eprintln!("DTIM3_TICKS_PER_INSTRUCTION must be a positive integer");
                std::process::exit(2)
            }
        },
        None => 1,
    };
    if dtim3_tick_setting.is_some() && !dtim3_bootstrap {
        eprintln!("DTIM3_TICKS_PER_INSTRUCTION requires DTIM3_BOOTSTRAP");
        std::process::exit(2);
    }
    if pit0_oneshot && (!pit_bootstrap || !intc2_bootstrap) {
        eprintln!("PIT0_ONESHOT requires PIT_BOOTSTRAP and INTC2_BOOTSTRAP");
        std::process::exit(2);
    }
    if pit2_periodic && (!pit_bootstrap || !intc2_bootstrap) {
        eprintln!("PIT2_PERIODIC requires PIT_BOOTSTRAP and INTC2_BOOTSTRAP");
        std::process::exit(2);
    }
    if esdhc_trace && !esdhc_emmc {
        eprintln!("ESDHC_TRACE requires ESDHC_EMMC");
        std::process::exit(2);
    }
    let uart8_tx = args[6..].iter().any(|assignment| assignment == "UART8_TX");
    let uart8_tx_ready = args[6..]
        .iter()
        .any(|assignment| assignment == "UART8_TX_READY");
    let uart8_rx = args[6..]
        .iter()
        .find_map(|assignment| assignment.strip_prefix("UART8_RX="))
        .map(|hex| {
            parse_uart_rx_bytes(hex).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(2)
            })
        });
    let panel_stub = args[6..]
        .iter()
        .any(|assignment| assignment == "PANEL_STUB");
    let panel_stub_func = args[6..]
        .iter()
        .any(|assignment| assignment == "PANEL_STUB_FUNC");
    if panel_stub && uart8_rx.is_some() {
        eprintln!("PANEL_STUB cannot be combined with UART8_RX=hex");
        std::process::exit(2);
    }
    if panel_stub_func && !panel_stub {
        eprintln!("PANEL_STUB_FUNC requires PANEL_STUB");
        std::process::exit(2);
    }
    if panel_stub && !range_fits(ram_base, ram_size, 0x4FE3_A000, 0x400) {
        eprintln!("PANEL_STUB requires mapped RAM covering 0x4fe3a000..0x4fe3a400");
        std::process::exit(2);
    }
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
        if pit_first_tick_delay > 0 {
            bus.pit
                .as_mut()
                .unwrap()
                .set_first_tick_delay(pit_first_tick_delay);
        }
    }
    if let Some(period) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("SSI_AUDIO_IRQ="))
    {
        let Ok(period) = period.parse() else {
            eprintln!("invalid SSI_AUDIO_IRQ period: {period}");
            std::process::exit(2);
        };
        bus.ssi_audio_period = Some(period);
    }
    if args[6..].iter().any(|a| a == "INTC1_BOOTSTRAP") {
        bus.intc1 = Some(Intc0::at(0xFC04_C000, 128));
    }
    if intc2_bootstrap {
        bus.attach_intc2();
    }
    if dtim1_bootstrap {
        bus.attach_dtim1_bootstrap(dtim1_ticks_per_instruction);
    }
    if dtim3_bootstrap {
        bus.attach_dtim3_bootstrap(dtim3_ticks_per_instruction);
    }
    if dspi1_bootstrap {
        bus.attach_dspi1_registers();
    }
    if dspi2_bootstrap {
        bus.attach_dspi2_registers();
    }
    if scheduler_trace {
        bus.enable_scheduler_trace();
    }
    if conversion_trace {
        bus.enable_conversion_trace();
    }
    if pit0_oneshot {
        bus.enable_pit0_oneshot();
    }
    if let Some(range) = watch_write_range {
        bus.watch_write_range(range);
    }
    if let Some(range) = write_trace_range {
        bus.write_trace_range = Some(range);
    }
    if let Some(range) = watch_read_range {
        bus.watch_read_range(range);
    }
    if let Some(range) = sample_access_trace_range {
        bus.sample_access_trace_range = Some(range);
    }
    if let Some(pc) = trace_register_pc {
        bus.trace_registers_at(pc);
    }
    if trace_tail > 0 {
        bus.trace_instruction_tail(trace_tail);
    }
    bus.trace_a1_checkpoints(&trace_a1_checkpoints);
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
    let live_panel = args[6..].iter().any(|a| a == "LIVE_PANEL");
    let panel_image = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("PANEL_IMAGE="))
        .map(str::to_owned);
    if let Some(value) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("PANEL_COUNTS_PER_DEGREE="))
    {
        match value.parse::<f32>() {
            Ok(pixels) if pixels > 0.0 => panel_ui::set_counts_per_degree(pixels),
            _ => {
                eprintln!("PANEL_COUNTS_PER_DEGREE must be a positive number");
                std::process::exit(2)
            }
        }
    }
    let live_window = args[6..].iter().find_map(|a| {
        if a == "LIVE_WINDOW" || a == "LIVE_PANEL" {
            Some(4)
        } else {
            a.strip_prefix("LIVE_WINDOW=")
                .and_then(|s| s.parse::<usize>().ok())
        }
    });
    let live_png = args[6..].iter().find_map(|a| a.strip_prefix("LIVE_PNG="));
    if live_window.is_some() || live_png.is_some() {
        bus.display.get_or_insert_with(display::Display::new);
    }
    if uart8_tx || uart8_tx_ready || uart8_rx.is_some() || panel_stub || bus.display.is_some() {
        bus.attach_uart8_tx();
    }
    if uart8_tx_ready {
        bus.enable_uart8_tx_ready();
    }
    if panel_stub {
        bus.enable_panel_stub();
        bus.panel_stub_func_held = panel_stub_func;
    }
    if let Some(bytes) = &uart8_rx {
        bus.queue_uart8_rx(bytes);
    }
    if esdhc_emmc {
        bus.esdhc = Some(if let Some(path) = &esdhc_file {
            Esdhc::with_backing_file(path).unwrap_or_else(|error| {
                eprintln!("cannot open eSDHC backing image {path}: {error}");
                std::process::exit(2)
            })
        } else {
            Esdhc::new()
        });
    }
    if esdhc_trace {
        bus.enable_esdhc_trace();
    }
    bus.load_ram(image_address, &bytes).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    });
    bus.capture_watch_initial().unwrap_or_else(|error| {
        eprintln!(
            "cannot capture WATCH_WRITE initial bytes at {:#010x}",
            error.addr
        );
        std::process::exit(2)
    });
    let mut cpu = Cpu::new();
    cpu.set_icache(!args[6..].iter().any(|a| a == "DECODE_CACHE=off") && !unicorn_enabled());
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
            || assignment == "INTC1_BOOTSTRAP"
            || assignment.starts_with("SSI_AUDIO_IRQ=")
            || assignment == "DTIM1_BOOTSTRAP"
            || assignment == "DTIM3_BOOTSTRAP"
            || assignment == "DSPI1_BOOTSTRAP"
            || assignment == "DSPI2_BOOTSTRAP"
            || assignment == "DSP_LINK"
            || assignment.starts_with("DSP_LINK_REPLY=")
            || assignment == "EDMA_TRACE"
            || assignment == "NONRAM_BULK_TRACE"
            || assignment.starts_with("SYNTH_VOICE=")
            || assignment == "PIT0_ONESHOT"
            || assignment == "PIT2_PERIODIC"
            || assignment.starts_with("SNAPSHOT_SAVE=")
            || assignment.starts_with("SNAPSHOT_LOAD=")
            || assignment.starts_with("POST_LOAD_RAM32@")
            || assignment.starts_with("INTC0_UNMASK=")
            || assignment.starts_with("UART8_RX_LATE=")
            || assignment.starts_with("RGPIO_INPUT_HIGH=")
            || assignment.starts_with("RGPIO_TRACE=")
            || assignment == "RGPIO_READY_MODEL"
            || assignment == "GPIO_LOOPBACK"
            || assignment == "PIT_EAGER"
            || assignment.starts_with("IRQ_LOG=")
            || assignment == "ESDHC_EMMC"
            || assignment.starts_with("ESDHC_FILE=")
            || assignment.starts_with("ESDHC_DUMP=")
            || assignment == "ESDHC_TRACE"
            || assignment.starts_with("UART8_TX_FILE=")
            || assignment.starts_with("SSI_TX_RAW=")
            || assignment == "LIVE_WINDOW"
            || assignment == "LIVE_PANEL"
            || assignment.starts_with("PANEL_IMAGE=")
            || assignment.starts_with("PC_SAMPLE=")
            || assignment.starts_with("PANEL_COUNTS_PER_DEGREE=")
            || assignment.starts_with("LIVE_WINDOW=")
            || assignment.starts_with("LIVE_PNG=")
            || assignment.starts_with("UART8_RX_PERIODIC=")
            || assignment == "UART8_RX_ONCE"
            || assignment.starts_with("DUMP_CALL=")
            || assignment.starts_with("STOP_PC=")
            || assignment.starts_with("WATCH_WRITE=")
            || assignment.starts_with("WRITE_TRACE=")
            || assignment.starts_with("WATCH_READ=")
            || assignment.starts_with("SAMPLE_ACCESS_TRACE=")
            || assignment.starts_with("TRACE_REGS=")
            || assignment.starts_with("STATE_HASH=")
            || assignment == "DECODE_CACHE=off"
            || assignment.starts_with("TRACE_TAIL=")
            || assignment.starts_with("TRACE_A1=")
            || assignment.starts_with("TRACE_A0_RANGE=")
            || assignment.starts_with("TRACE_PC_RANGE=")
            || assignment.starts_with("RAM_REMAP=")
            || assignment.starts_with("RAM_SNAPSHOT=")
            || assignment.starts_with("PIT_TICKS_PER_INSTRUCTION=")
            || assignment.starts_with("PIT_FIRST_TICK_DELAY=")
            || assignment.starts_with("DTIM1_TICKS_PER_INSTRUCTION=")
            || assignment.starts_with("DTIM3_TICKS_PER_INSTRUCTION=")
            || assignment == "LOOP_DETECT"
            || assignment == "SCHED_TRACE"
            || assignment == "CONVERT_TRACE"
            || assignment.starts_with("INJECT_IRQ=")
            || assignment == "UART8_TX"
            || assignment == "UART8_TX_READY"
            || assignment.starts_with("UART8_RX=")
            || assignment == "PANEL_STUB"
            || assignment == "PANEL_STUB_FUNC"
            || assignment == "INSPECT_TRAP0"
        {
            continue;
        }
        if assignment == "PIT0_PERIODIC" {
            bus.configure_pit0_periodic().unwrap_or_else(|error| {
                eprintln!("cannot configure PIT0_PERIODIC: {error:?}");
                std::process::exit(2)
            });
            continue;
        }
        if assignment == "PIT2_PERIODIC" {
            bus.configure_pit2_periodic().unwrap_or_else(|error| {
                eprintln!("cannot configure PIT2_PERIODIC: {error:?}");
                std::process::exit(2)
            });
            continue;
        }
        if let Some(spec) = assignment.strip_prefix("RAM_LOAD_FILE=") {
            let (address, path) = spec.split_once(':').unwrap_or_else(|| {
                eprintln!("invalid RAM_LOAD_FILE spec: {spec}; expected address:path");
                std::process::exit(2)
            });
            let address = parse_address(address).unwrap_or_else(|error| {
                eprintln!("invalid RAM_LOAD_FILE address: {error}");
                std::process::exit(2)
            });
            let contents = std::fs::read(path).unwrap_or_else(|error| {
                eprintln!("cannot read RAM_LOAD_FILE {path}: {error}");
                std::process::exit(2)
            });
            bus.load_ram(address, &contents).unwrap_or_else(|error| {
                eprintln!("{error}");
                std::process::exit(2)
            });
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
        if let Some(value) = assignment.strip_prefix("DSPI0_CTAR0=") {
            let value = parse_address(value).unwrap_or_else(|error| {
                eprintln!("invalid DSPI0_CTAR0 value: {error}");
                std::process::exit(2)
            });
            bus.write32(DSPI0_BASE + 0x0C, value)
                .unwrap_or_else(|error| {
                    eprintln!("cannot set DSPI0 CTAR0: {error:?}");
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
    if let Some(path) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("SNAPSHOT_LOAD="))
    {
        let data = std::fs::read(path).unwrap_or_else(|error| {
            eprintln!("cannot read SNAPSHOT_LOAD {path}: {error}");
            std::process::exit(2)
        });
        bus.load_snapshot(&mut cpu, &data, ram_base, ram_size as u32)
            .unwrap_or_else(|error| {
                eprintln!("cannot restore snapshot: {error}");
                std::process::exit(2)
            });
        bus.write_trace_base = bus.current_instruction_count;
        bus.sync_esdhc_trace_baseline();
        // An explicit rate overrides the one stored in the snapshot.
        if dtim3_tick_setting.is_some() {
            if let Some(dtim3) = bus.dtim3.as_mut() {
                dtim3.set_ticks_per_instruction(dtim3_ticks_per_instruction);
            }
        }
        println!("restored snapshot {path}: PC={:#010x}", cpu.pc);
    }
    if let Some(spec) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("SYNTH_VOICE="))
    {
        let parsed = spec
            .split_once(':')
            .and_then(|(a, l)| Some((parse_address(a).ok()?, parse_address(l).ok()?)));
        let Some((address, length)) = parsed else {
            eprintln!("invalid SYNTH_VOICE, expected address:length");
            std::process::exit(2)
        };
        bus.synth_voice = Some((address, length));
        println!("diagnostic: SYNTH_VOICE plays {length:#x} bytes of 16-bit BE mono from {address:#010x} into SSI TX on voice start (not firmware behavior)");
    }
    if args[6..].iter().any(|a| a == "DSP_LINK") {
        let Some(dspi2) = bus.dspi2.as_mut() else {
            eprintln!("DSP_LINK needs DSPI2_BOOTSTRAP or a snapshot with DSPI2");
            std::process::exit(2)
        };
        // The firmware only starts a DSP exchange once the previous one reported EOQF.
        dspi2.set_eoq_flag();
        bus.dsp_link = true;
        bus.dsp_link_reply = dsp_link_reply;
        println!(
            "diagnostic: DSP link (DSPI2 + eDMA 28/29), reply={:#06x}",
            bus.dsp_link_reply
        );
    } else if args[6..].iter().any(|a| a.starts_with("DSP_LINK_REPLY=")) {
        eprintln!("DSP_LINK_REPLY requires DSP_LINK");
        std::process::exit(2);
    }
    if args[6..].iter().any(|a| a == "EDMA_TRACE") {
        bus.edma_trace_enabled = true;
        println!("diagnostic: eDMA request trace enabled");
    }
    if args[6..].iter().any(|a| a == "NONRAM_BULK_TRACE") {
        bus.nonram_bulk_trace_enabled = true;
        println!("diagnostic: non-RAM bulk I/O trace enabled (minimum 1024 bytes)");
    }
    for seed in args[6..]
        .iter()
        .filter_map(|a| a.strip_prefix("POST_LOAD_RAM32@"))
    {
        bus.seed_ram32(seed).unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2)
        });
        println!("diagnostic: post-load RAM32 seed {seed}");
    }
    if let Some(list) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("INTC0_UNMASK="))
    {
        let Some(intc0) = bus.intc0.as_mut() else {
            eprintln!("INTC0_UNMASK needs an attached INTC0");
            std::process::exit(2)
        };
        for source in list.split(',') {
            let source: u8 = source.parse().unwrap_or_else(|_| {
                eprintln!("invalid INTC0_UNMASK source {source}");
                std::process::exit(2)
            });
            intc0.write8(0xFC04_801D, source).expect("INTC0 CIMR write");
            println!("diagnostic: INTC0 source {source} unmasked");
        }
    }
    if let Some(path) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("UART8_TX_FILE="))
    {
        let open = |path: &str| {
            std::fs::File::create(path)
                .map(std::io::BufWriter::new)
                .unwrap_or_else(|error| {
                    eprintln!("cannot create {path}: {error}");
                    std::process::exit(2)
                })
        };
        bus.uart8_tx_file = Some((open(path), open(&format!("{path}.times"))));
    }
    let ssi_tx_raw_path = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("SSI_TX_RAW="))
        .map(str::to_owned);
    if ssi_tx_raw_path.as_deref().is_some_and(str::is_empty) {
        eprintln!("SSI_TX_RAW path must not be empty");
        std::process::exit(2);
    }
    if ssi_tx_raw_path.is_some() {
        bus.ssi_tx_raw = Some(Vec::new());
    }
    if let Some(spec) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("UART8_RX_PERIODIC="))
    {
        let (period, messages) = spec.split_once(':').unwrap_or_else(|| {
            eprintln!("UART8_RX_PERIODIC expects period:hex[,hex...]");
            std::process::exit(2)
        });
        let period: usize = period.parse().unwrap_or_else(|_| {
            eprintln!("invalid UART8_RX_PERIODIC period");
            std::process::exit(2)
        });
        let messages: Vec<Vec<u8>> = messages
            .split(',')
            .map(|hex| {
                if hex == "-" {
                    return Vec::new(); // a pause of one period
                }
                parse_uart_rx_bytes(hex).unwrap_or_else(|error| {
                    eprintln!("{error}");
                    std::process::exit(2)
                })
            })
            .collect();
        bus.uart8_rx_periodic = Some((period, messages, 0, 0));
        bus.uart8_rx_once = args[6..].iter().any(|a| a == "UART8_RX_ONCE");
    }
    bus.gpio_loopback = args[6..].iter().any(|a| a == "GPIO_LOOPBACK");
    if args[6..].iter().any(|a| a == "RGPIO_READY_MODEL") {
        bus.rgpio_ready_model = true;
    }
    for spec in args[6..]
        .iter()
        .filter_map(|a| a.strip_prefix("DUMP_CALL="))
    {
        let fields: Vec<&str> = spec.splitn(4, ':').collect();
        let parsed = (fields.len() == 4)
            .then(|| {
                Some(CallDump {
                    pc: parse_address(fields[0]).ok()?,
                    buffer_arg: fields[1].parse().ok().filter(|index| *index < 3)?,
                    length: fields[2].parse().ok()?,
                    prefix: fields[3].to_string(),
                })
            })
            .flatten();
        bus.dump_calls.push(parsed.unwrap_or_else(|| {
            eprintln!("DUMP_CALL expects pc:buffer-arg(0-2):length:path-prefix, got {spec}");
            std::process::exit(2)
        }));
    }
    if let Some(limit) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("RGPIO_TRACE="))
    {
        bus.rgpio_trace_limit = limit.parse().unwrap_or_else(|_| {
            eprintln!("RGPIO_TRACE must be an integer");
            std::process::exit(2)
        });
    }
    if let Some(mask) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("RGPIO_INPUT_HIGH="))
    {
        bus.rgpio_input_high = parse_address(mask).unwrap_or_else(|error| {
            eprintln!("invalid RGPIO_INPUT_HIGH: {error}");
            std::process::exit(2)
        }) as u16;
    }
    if let Some(hex) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("UART8_RX_LATE="))
    {
        let bytes = parse_uart_rx_bytes(hex).unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2)
        });
        bus.deliver_uart8_rx_now(&bytes).unwrap_or_else(|error| {
            eprintln!("cannot deliver UART8_RX_LATE at {:#010x}", error.addr);
            std::process::exit(2)
        });
        println!(
            "UART8_RX_LATE: delivered {} byte(s) through the RX DMA ring",
            bytes.len()
        );
    }
    let run_started = Instant::now();
    if args[6..].iter().any(|a| a == "PIT_EAGER") {
        if let Some(pit) = bus.pit.as_mut() {
            pit.set_eager(true);
        }
        if let Some(dtim1) = bus.dtim1.as_mut() {
            dtim1.set_eager(true);
        }
        if let Some(dtim3) = bus.dtim3.as_mut() {
            dtim3.set_eager(true);
        }
        println!("timers: eager per-instruction path (PIT_EAGER)");
    }
    if let Some(path) = args[6..].iter().find_map(|a| a.strip_prefix("IRQ_LOG=")) {
        let file = std::fs::File::create(path).unwrap_or_else(|error| {
            eprintln!("cannot create IRQ_LOG {path}: {error}");
            std::process::exit(2)
        });
        bus.irq_log = Some(std::io::BufWriter::new(file));
    }
    let fast_path_status = match bus.enable_fast_ram(ram_base, ram_size as u32) {
        None => "fast path: on".to_string(),
        Some(reason) => format!("fast path: off ({reason})"),
    };
    let fast_path_status = format!(
        "{fast_path_status}; instruction hooks: {}",
        bus.configure_instruction_hooks()
    );
    println!("{fast_path_status}");
    let pc_sample = args[6..].iter().find_map(|a| a.strip_prefix("PC_SAMPLE="));
    let state_hash = args[6..].iter().find_map(|a| a.strip_prefix("STATE_HASH="));
    let report = if let Some(spec) = pc_sample {
        run_pc_sampled(&mut cpu, &mut bus, instruction_limit, spec)
    } else if let Some(spec) = state_hash {
        run_state_hashed(
            &mut cpu,
            &mut bus,
            instruction_limit,
            spec,
            ram_base,
            ram_size as u32,
        )
    } else if let Some(scale) = live_window {
        run_live(
            &mut cpu,
            &mut bus,
            instruction_limit,
            interrupt_injection,
            scale,
            live_panel.then_some(panel_image),
        )
    } else {
        run_engine(&mut cpu, &mut bus, instruction_limit, interrupt_injection)
    };
    #[cfg(dt2_unicorn)]
    unicorn_tier::print_stats();
    if let (Some(path), Some(display)) = (live_png, &bus.display) {
        let (width, height, pixels) = display.render_upright(4);
        std::fs::write(path, display::encode_gray_png(width, height, &pixels)).unwrap_or_else(
            |error| {
                eprintln!("cannot write LIVE_PNG {path}: {error}");
                std::process::exit(2)
            },
        );
        println!(
            "LIVE_PNG: wrote {path} after {} display update(s)",
            display.updates
        );
    }
    let elapsed_seconds = run_started.elapsed().as_secs_f64();
    let instructions_per_second = if elapsed_seconds > 0.0 {
        report.instructions as f64 / elapsed_seconds
    } else {
        0.0
    };

    if let Some(log) = &mut bus.irq_log {
        use std::io::Write;
        let _ = log.flush();
    }
    if let Some((bytes, times)) = &mut bus.uart8_tx_file {
        use std::io::Write;
        let _ = bytes.flush();
        let _ = times.flush();
    }
    if let Some(path) = ssi_tx_raw_path {
        let Some(bytes) = bus.ssi_tx_raw.as_deref() else {
            eprintln!("SSI_TX_RAW was requested but the capture buffer was not initialized");
            std::process::exit(2)
        };
        std::fs::write(&path, bytes).unwrap_or_else(|error| {
            eprintln!("cannot write SSI_TX_RAW {path}: {error}");
            std::process::exit(2)
        });
        let nonzero_bytes = bytes.iter().filter(|&&byte| byte != 0).count();
        let first_source = bus
            .ssi_tx_first_source
            .map_or_else(|| "none".to_owned(), |address| format!("0x{address:08x}"));
        println!(
            "SSI TX raw capture: {} DMA block(s), {} byte(s), {nonzero_bytes} nonzero, first source {first_source}, written to {path}; byte order follows the big-endian eDMA source buffer",
            bus.ssi_tx_dma_blocks,
            bytes.len()
        );
    }
    if let Some(path) = args[6..]
        .iter()
        .find_map(|a| a.strip_prefix("SNAPSHOT_SAVE="))
    {
        let data = bus
            .save_snapshot(&cpu, ram_base, ram_size as u32)
            .unwrap_or_else(|error| {
                eprintln!("cannot save snapshot: {error}");
                std::process::exit(2)
            });
        std::fs::write(path, &data).unwrap_or_else(|error| {
            eprintln!("cannot write SNAPSHOT_SAVE {path}: {error}");
            std::process::exit(2)
        });
        println!("saved snapshot {path}: {} bytes", data.len());
    }
    if let Some(path) = args[6..].iter().find_map(|a| a.strip_prefix("ESDHC_DUMP=")) {
        let Some(esdhc) = bus.esdhc.as_ref() else {
            eprintln!("ESDHC_DUMP requires ESDHC_EMMC");
            std::process::exit(2)
        };
        // Sparse raw image of every written block (a hole reads as zeros).
        let result = std::fs::File::create(path).and_then(|mut file| {
            use std::io::{Seek, SeekFrom, Write};
            let mut count = 0usize;
            for (block, data) in esdhc.written_blocks() {
                file.seek(SeekFrom::Start(u64::from(block) * 512))?;
                file.write_all(data)?;
                count += 1;
            }
            Ok(count)
        });
        match result {
            Ok(count) => println!("ESDHC_DUMP {path}: {count} written blocks"),
            Err(error) => {
                eprintln!("cannot write ESDHC_DUMP {path}: {error}");
                std::process::exit(2)
            }
        }
    }
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
    if panel_stub {
        println!("panel stub: answers section-2 discovery and section-3 UART8 handshake; input records are synthetic; eDMA timing is diagnostic");
    }
    if uart8_tx_ready {
        println!("UART8_TX_READY diagnostic: TXRDY/TXEMP asserted from startup; no panel response or RX records are injected");
    }
    if pit0_oneshot {
        println!("PIT0 one-shot diagnostic: expose the first unmasked PIT0 request; suppress later PIT0 requests");
    }
    if pit2_periodic {
        println!("PIT2 periodic diagnostic: approximate 60 Hz timer source 15; not a measured OS1.17 rate");
    }
    if dtim3_bootstrap {
        println!("DTIM3 diagnostic: reference-compare timer 3 with {dtim3_ticks_per_instruction} bus tick(s) per retired instruction; approximate timing; INTC0 source 35 (vector 99) is modeled");
    }
    if dtim1_bootstrap {
        println!("DTIM1 diagnostic: reference-compare timer 1 with {dtim1_ticks_per_instruction} bus tick(s) per retired instruction; approximate timing; INTC0 source 33 is modeled");
    }
    if dspi1_bootstrap {
        println!("DSPI1 model: registers/FIFO only, no attached slave; received data is zero");
    }
    if dspi2_bootstrap {
        println!("DSPI2 model: registers/FIFO only, no attached slave; received data is 0xffff");
    }
    if let Some(esdhc) = &bus.esdhc {
        println!(
            "eSDHC/eMMC synthetic model: {} command(s) executed; EXT_CSD and card content are not hardware evidence",
            esdhc.commands_executed()
        );
    }
    if let Some(path) = &esdhc_file {
        println!("eSDHC backing image: {path} (raw 4 GiB, sparse; synthetic card semantics)");
    }
    bus.print_watched_writes();
    bus.print_watched_reads();
    bus.print_firmware_write_trace();
    bus.print_nonram_bulk_trace();
    bus.print_sample_access_trace();
    bus.print_register_trace();
    bus.print_instruction_trace_tail();
    bus.print_scheduler_trace();
    bus.print_conversion_trace();
    println!("executed {} instructions", report.instructions);
    println!(
        "runner throughput: {elapsed_seconds:.3}s, {instructions_per_second:.0} instructions/s; {fast_path_status}"
    );
    let (icache_hits, icache_misses) = cpu.icache_stats();
    if icache_hits + icache_misses > 0 {
        println!(
            "icache: {icache_hits} hits, {icache_misses} misses, {} flushes",
            bus.code_map.flushes()
        );
    }
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
            "PIT model: {pit_ticks_per_instruction} timer tick(s) per retired instruction;{}approximate timing; INTC delivery requires INTC2_BOOTSTRAP",
            if pit_ticks_per_instruction > 1 {
                " accelerated "
            } else {
                " "
            }
        );
    }
    if let Some(pit) = bus.pit.as_mut() {
        pit.sync();
    }
    if let Some(pit) = &bus.pit {
        for module in 0..4 {
            if let Some((pcsr, pmr, counter)) = pit.timer_state(module) {
                println!("PIT{module} state: PCSR={pcsr:#06x} PMR={pmr:#06x} PCNTR={counter:#06x}");
            }
        }
    }
    if let Some(dtim3) = bus.dtim3.as_mut() {
        let (dtmr, dtxmr, dter, dtrr, dtcn) = dtim3.debug_state();
        println!("DTIM3 state: DTMR={dtmr:#06x} DTXMR={dtxmr:#04x} DTER={dter:#04x} DTRR={dtrr:#x} DTCN={dtcn:#x}");
    }
    if let Some(intc0) = &bus.intc0 {
        let (imrh, imrl, frch, frcl, icr) = intc0.debug_state();
        println!(
            "INTC0 state: IMRH={imrh:#010x} IMRL={imrl:#010x} INTFRCH={frch:#010x} INTFRCL={frcl:#010x} ICR44={} ICR57={}",
            icr[44], icr[57]
        );
    }
    if pit_first_tick_delay > 0 {
        println!("PIT0 first-tick diagnostic delay: {pit_first_tick_delay} modeled bus ticks after the first enable; approximate timing");
    }
    if let Some(uart) = &bus.uart8 {
        println!(
            "UART8 model: TX/RX byte stream, serial timing not modeled; captured {} bytes, {} RX bytes remain",
            uart.transmitted(),
            uart.received_remaining()
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
    bus.print_esdhc_trace();
    bus.print_uart8_trace();
    bus.print_loop_reports();
    bus.print_last_control_flow();
    bus.print_rte_history();
    bus.print_halt_context(report.stop);
    print_stop(report.stop);
    println!(
        "PC={:#010x} SR={:#06x} RAMBAR={:#010x} ACR0={:#010x} ACR1={:#010x} VBR={:#010x} CACR={:#010x} RGPIOBAR={:#010x} DEBUG_CSR={:#010x}",
        report.cpu.pc,
        report.cpu.sr,
        report.cpu.rambar,
        report.cpu.acr0,
        report.cpu.acr1,
        report.cpu.vbr,
        report.cpu.cacr,
        report.cpu.rgpiobar,
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

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn print_scheduler_words(label: &str, address: Option<u32>, words: &[u32]) {
    let rendered = words
        .iter()
        .map(|word| format!("{word:#010x}"))
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(address) = address {
        println!("    {label} at {address:#010x}: {rendered}");
    } else {
        println!("    {label}: {rendered}");
    }
}

fn scheduler_event_kind(pc: u32) -> &'static str {
    match pc {
        0x4000_13EC | 0x4000_145E => "ready-list removal/block transition",
        0x4000_14BA => "ready-list insertion/wakeup transition",
        0x4000_1344 => "task-return trampoline",
        0x4000_141A | 0x4000_13A6 => "semaphore pend",
        0x4000_1608 => "delay call",
        _ => "scheduler event",
    }
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
        return Err(format!(
            "DUMP_RAM_FILE length must be 1-67108864 bytes: {length}"
        ));
    }
    if address.checked_add(length as u32).is_none() {
        return Err(format!(
            "DUMP_RAM_FILE range overflows address space: {spec}"
        ));
    }
    Ok((address, length, path.to_string()))
}

/// `PC_SAMPLE=start:end:interval:path` records the program counter every `interval` instructions between
/// instruction counts `start` and `end` (counted from this run's start) as `instruction pc` lines in `path`.
fn run_pc_sampled(
    cpu: &mut Cpu,
    bus: &mut ProbeBus,
    instruction_limit: usize,
    spec: &str,
) -> dt2_core::coldfire::RunReport {
    let fields: Vec<&str> = spec.splitn(4, ':').collect();
    let number = |index: usize| fields.get(index).and_then(|f| f.parse::<usize>().ok());
    let (Some(start), Some(end), Some(interval), Some(path)) =
        (number(0), number(1), number(2), fields.get(3))
    else {
        eprintln!("PC_SAMPLE expects start:end:interval:path");
        std::process::exit(2)
    };
    let mut report = cpu.run_with_interrupt(bus, start.min(instruction_limit), None);
    let mut done = report.instructions + report.fast_forwarded_instructions;
    let mut out = String::new();
    while report.stop == RunStop::InstructionLimit
        && done < end.min(instruction_limit)
        && interval > 0
    {
        report = cpu.run_with_interrupt(bus, interval, None);
        done += report.instructions + report.fast_forwarded_instructions;
        let sp = cpu.a[7];
        let stack: Vec<String> = (0..12)
            .map(|slot| {
                format!(
                    "{:08x}",
                    bus.peek_ram_u32(sp.wrapping_add(4 * slot)).unwrap_or(0)
                )
            })
            .collect();
        out.push_str(&format!("{done} {:#010x} {}\n", cpu.pc, stack.join(" ")));
    }
    std::fs::write(path, out)
        .unwrap_or_else(|error| eprintln!("cannot write PC_SAMPLE {path}: {error}"));
    report.instructions = done;
    report
}

/// `STATE_HASH=n1,n2,...` prints an FNV-1a hash of the full snapshot (CPU, RAM, devices) after each instruction count
/// (counted from this run's start). It is the regression gate for CPU-core changes: equal hashes mean equal machine state.
fn unicorn_enabled() -> bool {
    #[cfg(dt2_unicorn)]
    return unicorn_tier::enabled();
    #[cfg(not(dt2_unicorn))]
    false
}

/// Runs the CPU on the selected engine: the Rust interpreter, or the Unicorn tier when enabled.
fn run_engine(
    cpu: &mut Cpu,
    bus: &mut ProbeBus,
    limit: usize,
    injection: Option<InterruptInjection>,
) -> dt2_core::coldfire::RunReport {
    #[cfg(dt2_unicorn)]
    if unicorn_tier::enabled() {
        return unicorn_tier::run(cpu, bus, limit, injection);
    }
    cpu.run_with_interrupt(bus, limit, injection)
}

fn run_state_hashed(
    cpu: &mut Cpu,
    bus: &mut ProbeBus,
    instruction_limit: usize,
    spec: &str,
    ram_base: u32,
    ram_size: u32,
) -> dt2_core::coldfire::RunReport {
    let mut points: Vec<usize> = spec
        .split(',')
        .map(|n| {
            n.parse().unwrap_or_else(|_| {
                eprintln!("STATE_HASH expects a comma-separated list of instruction counts");
                std::process::exit(2)
            })
        })
        .collect();
    points.sort_unstable();
    points.push(instruction_limit);
    let mut done = 0usize;
    let mut report = cpu.run_with_interrupt(bus, 0, None);
    for point in points {
        let target = point.min(instruction_limit);
        if target > done {
            report = run_engine(cpu, bus, target - done, None);
            done += report.instructions + report.fast_forwarded_instructions;
        }
        let snapshot = bus
            .save_snapshot(cpu, ram_base, ram_size)
            .unwrap_or_else(|error| {
                eprintln!("STATE_HASH: cannot snapshot: {error}");
                std::process::exit(2)
            });
        let hash = snapshot.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
        });
        println!("STATE_HASH {done} {hash:016x}");
        if report.stop != RunStop::InstructionLimit {
            break;
        }
    }
    report.instructions = done;
    report.fast_forwarded_instructions = 0;
    report
}

const LIVE_CHUNK_INSTRUCTIONS: usize = 400_000;

/// Runs the CPU in slices and presents the decoded panel display between slices.
fn run_live(
    cpu: &mut Cpu,
    bus: &mut ProbeBus,
    instruction_limit: usize,
    mut injection: Option<InterruptInjection>,
    scale: usize,
    panel: Option<Option<String>>,
) -> dt2_core::coldfire::RunReport {
    let mut window = live_window::open(scale, panel);
    if let Some(display) = &bus.display {
        let (width, height, pixels) = display.render_upright(scale);
        window.present(width, height, &pixels, &|id| display.led_rgb(id));
    }
    let mut done = 0usize;
    // PANEL_PERF=1 prints where wall time goes (CPU, input, drawing) every two seconds.
    let perf = std::env::var_os("PANEL_PERF").is_some();
    let (mut t_cpu, mut t_in, mut t_draw) = (
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
    );
    let (mut perf_start, mut perf_done, mut perf_slices) =
        (std::time::Instant::now(), 0usize, 0u32);
    loop {
        let t0 = std::time::Instant::now();
        let chunk = LIVE_CHUNK_INSTRUCTIONS.min(instruction_limit - done);
        let shifted = injection.map(|mut event| {
            event.instruction_count = event.instruction_count.saturating_sub(done);
            event
        });
        let mut report = run_engine(cpu, bus, chunk, shifted);
        if report.injected_interrupt.is_some() {
            injection = None;
        }
        done += report.instructions + report.fast_forwarded_instructions;
        let t1 = std::time::Instant::now();
        for message in window.input_messages() {
            let _ = bus.deliver_uart8_rx_now(&message);
        }
        let t2 = std::time::Instant::now();
        if let Some(display) = &bus.display {
            let (width, height, pixels) = display.render_upright(scale);
            window.present(width, height, &pixels, &|id| display.led_rgb(id));
        }
        if perf {
            let t3 = std::time::Instant::now();
            t_cpu += t1 - t0;
            t_in += t2 - t1;
            t_draw += t3 - t2;
            perf_slices += 1;
            if t3 - perf_start >= std::time::Duration::from_secs(2) {
                let wall = (t3 - perf_start).as_secs_f64();
                eprintln!(
                    "PANEL_PERF: {:.1} M instr/s, {:.1} slices/s, cpu {:.0}% input {:.0}% draw {:.0}%",
                    (done - perf_done) as f64 / wall / 1e6,
                    f64::from(perf_slices) / wall,
                    t_cpu.as_secs_f64() / wall * 100.0,
                    t_in.as_secs_f64() / wall * 100.0,
                    t_draw.as_secs_f64() / wall * 100.0
                );
                (t_cpu, t_in, t_draw) = Default::default();
                (perf_start, perf_done, perf_slices) = (t3, done, 0);
            }
        }
        let finished = report.stop != RunStop::InstructionLimit
            || done >= instruction_limit
            || !window.is_open();
        if finished {
            report.instructions = done;
            return report;
        }
    }
}

/// Wire position (button channel, bit) of a panel key code, from public hardware observations.
fn panel_wire_for_code(code: u8) -> Option<(u8, u8)> {
    match code {
        1..=48 => Some(((code - 1) / 8, (code - 1) % 8)),
        49 => Some((6, 1)),
        50 => Some((6, 0)),
        _ => None,
    }
}

/// Builds UART8 panel packets: `0x20 | channel` carries a channel's whole button mask, `0x30 | encoder`
/// a signed delta (an unverified lead from public notes on the Digitakt II, not confirmed on OS 1.17).
#[derive(Default)]
#[cfg_attr(not(feature = "window"), allow(dead_code))]
struct PanelInput {
    masks: [u8; 7],
    pressed_this_slice: Vec<u8>,
    deferred_releases: Vec<u8>,
    out: Vec<[u8; 2]>,
    /// Encoder counts not yet sent; see `TURN_MAX_DELTA`.
    pending_turns: [i32; 9],
}

/// The firmware accumulates encoder deltas and flushes at most 30 counts per UI tick (digiemu's panelin.py
/// notes the same; here two 0x10 messages in a burst count the same as four). One message of up to 30 counts
/// per encoder per slice therefore uses all it will take, and counts beyond a short backlog are dropped so a
/// released knob stops within a couple of slices instead of draining a queue for seconds.
const TURN_MAX_DELTA: i32 = 30;
const TURN_BACKLOG: i32 = 2 * TURN_MAX_DELTA;

#[cfg_attr(not(feature = "window"), allow(dead_code))]
impl PanelInput {
    /// Releases that arrived in the same slice as their press are sent one slice later, so a quick tap lasts.
    fn begin_slice(&mut self) {
        self.pressed_this_slice.clear();
        for code in std::mem::take(&mut self.deferred_releases) {
            self.set(code, false);
        }
    }

    fn set(&mut self, code: u8, down: bool) {
        let Some((channel, bit)) = panel_wire_for_code(code) else {
            return;
        };
        let old = self.masks[usize::from(channel)];
        let new = if down {
            old | 1 << bit
        } else {
            old & !(1 << bit)
        };
        if new != old {
            self.masks[usize::from(channel)] = new;
            self.out.push([0x20 | channel, new]);
        }
    }

    fn press(&mut self, code: u8) {
        self.pressed_this_slice.push(code);
        self.set(code, true);
    }

    fn release(&mut self, code: u8) {
        if self.pressed_this_slice.contains(&code) {
            self.deferred_releases.push(code);
        } else {
            self.set(code, false);
        }
    }

    fn turn(&mut self, encoder: u8, delta: i8) {
        if encoder < 9 {
            let pending = &mut self.pending_turns[usize::from(encoder)];
            *pending = (*pending + i32::from(delta)).clamp(-TURN_BACKLOG, TURN_BACKLOG);
        }
    }

    fn take(&mut self) -> Vec<[u8; 2]> {
        for (encoder, pending) in self.pending_turns.iter_mut().enumerate() {
            let step = (*pending).clamp(-TURN_MAX_DELTA, TURN_MAX_DELTA);
            if step != 0 {
                *pending -= step;
                self.out.push([0x30 | encoder as u8, step as i8 as u8]);
            }
        }
        std::mem::take(&mut self.out)
    }
}

#[cfg(feature = "window")]
mod live_window {
    pub struct Window(
        minifb::Window,
        super::PanelInput,
        Option<super::panel_ui::PanelUi>,
    );

    /// `panel` opens the clickable faceplate (with an optional panel photo as the backdrop) instead of the bare display.
    pub fn open(scale: usize, panel: Option<Option<String>>) -> Window {
        let (width, height) = match &panel {
            Some(_) => (super::panel_ui::PANEL_WIDTH, super::panel_ui::PANEL_HEIGHT),
            None => (
                super::display::FB_HEIGHT * scale,
                super::display::FB_WIDTH * scale,
            ),
        };
        let ui = panel.map(|image| {
            let backdrop = image.and_then(|path| match super::panel_ui::load_backdrop(&path) {
                Ok(pixels) => Some(pixels),
                Err(error) => {
                    eprintln!("PANEL_IMAGE ignored, drawing the plain panel: {error}");
                    None
                }
            });
            super::panel_ui::PanelUi::new(backdrop)
        });
        Window(
            minifb::Window::new(
                "dt2-rs panel display (synthetic boot)",
                width,
                height,
                minifb::WindowOptions::default(),
            )
            .unwrap_or_else(|error| {
                eprintln!("cannot open LIVE_WINDOW: {error}");
                std::process::exit(2)
            }),
            super::PanelInput::default(),
            ui,
        )
    }

    impl Window {
        pub fn present(
            &mut self,
            width: usize,
            height: usize,
            pixels: &[u32],
            leds: &dyn Fn(u8) -> Option<u32>,
        ) {
            match &self.2 {
                Some(ui) => {
                    let frame = ui.render(Some((width, height, pixels)), leds);
                    let _ = self.0.update_with_buffer(
                        &frame,
                        super::panel_ui::PANEL_WIDTH,
                        super::panel_ui::PANEL_HEIGHT,
                    );
                }
                None => {
                    let _ = self.0.update_with_buffer(pixels, width, height);
                }
            }
        }
        pub fn is_open(&self) -> bool {
            self.0.is_open()
        }

        /// Keyboard layout (my own choice, not the hardware's): F1-F6 page keys, F7-F9 PRESET SETTINGS
        /// TEMPO, Enter/Backspace YES/NO, arrows, Tab TRK, Ctrl FUNC, F10-F12 KEYBOARD RECORD STOP, Space
        /// PLAY, PageUp/Home/End PAGE PTN SONG, 1-8 and Q-I TRIG 1-16, Z-, encoder pushes A-H, / LEVEL
        /// push, Insert SAMPLING; A-K turn encoders A-H and L turns LEVEL (Shift reverses).
        pub fn input_messages(&mut self) -> Vec<[u8; 2]> {
            use minifb::{Key, KeyRepeat};
            let code = |key: Key| -> Option<u8> {
                Some(match key {
                    Key::F1 => 1,
                    Key::F2 => 2,
                    Key::F3 => 3,
                    Key::F4 => 4,
                    Key::F5 => 5,
                    Key::F6 => 6,
                    Key::F7 => 7,
                    Key::F8 => 8,
                    Key::F9 => 9,
                    Key::Enter => 10,
                    Key::Up => 11,
                    Key::Backspace => 12,
                    Key::Left => 13,
                    Key::Down => 14,
                    Key::Right => 15,
                    Key::Tab => 16,
                    Key::LeftCtrl => 17,
                    Key::F10 => 18,
                    Key::F11 => 19,
                    Key::Space => 20,
                    Key::F12 => 21,
                    Key::PageUp => 22,
                    Key::Home => 23,
                    Key::End => 24,
                    Key::Key1 => 25,
                    Key::Key2 => 26,
                    Key::Key3 => 27,
                    Key::Key4 => 28,
                    Key::Key5 => 29,
                    Key::Key6 => 30,
                    Key::Key7 => 31,
                    Key::Key8 => 32,
                    Key::Q => 33,
                    Key::W => 34,
                    Key::E => 35,
                    Key::R => 36,
                    Key::T => 37,
                    Key::Y => 38,
                    Key::U => 39,
                    Key::I => 40,
                    Key::Z => 41,
                    Key::X => 42,
                    Key::C => 43,
                    Key::V => 44,
                    Key::B => 45,
                    Key::N => 46,
                    Key::M => 47,
                    Key::Comma => 48,
                    Key::Slash => 49,
                    Key::Insert => 50,
                    _ => return None,
                })
            };
            let encoder = |key: Key| -> Option<u8> {
                Some(match key {
                    Key::A => 0,
                    Key::S => 1,
                    Key::D => 2,
                    Key::F => 3,
                    Key::G => 4,
                    Key::H => 5,
                    Key::J => 6,
                    Key::K => 7,
                    Key::L => 8,
                    _ => return None,
                })
            };
            let delta = if self.0.is_key_down(Key::LeftShift) || self.0.is_key_down(Key::RightShift)
            {
                -1
            } else {
                1
            };
            self.1.begin_slice();
            if let Some(ui) = self.2.as_mut() {
                use super::panel_ui::Action;
                // While dragging, keep reporting positions outside the window so the turn continues.
                let dragging = self.0.get_mouse_down(minifb::MouseButton::Left);
                let position = self.0.get_mouse_pos(if dragging {
                    minifb::MouseMode::Pass
                } else {
                    minifb::MouseMode::Discard
                });
                let wheel = self.0.get_scroll_wheel().map_or(0.0, |(_, y)| y);
                let mut actions = ui.handle_mouse(
                    position,
                    self.0.get_mouse_down(minifb::MouseButton::Left),
                    self.0.get_mouse_down(minifb::MouseButton::Right),
                    wheel,
                );
                if self.0.is_key_pressed(Key::Escape, KeyRepeat::No) {
                    actions.extend(ui.release_all());
                }
                for action in actions {
                    match action {
                        Action::Press(code) => self.1.press(code),
                        Action::Release(code) => self.1.release(code),
                        Action::Turn(encoder, delta) => self.1.turn(encoder, delta),
                    }
                }
            }
            for key in self.0.get_keys_pressed(KeyRepeat::Yes) {
                if let Some(index) = encoder(key) {
                    self.1.turn(index, delta);
                }
            }
            for key in self.0.get_keys_pressed(KeyRepeat::No) {
                if let Some(code) = code(key) {
                    self.1.press(code);
                }
            }
            for key in self.0.get_keys_released() {
                if let Some(code) = code(key) {
                    self.1.release(code);
                }
            }
            self.1.take()
        }
    }
}

#[cfg(not(feature = "window"))]
mod live_window {
    pub struct Window;

    pub fn open(_scale: usize, _panel: Option<Option<String>>) -> Window {
        eprintln!("LIVE_WINDOW needs the window feature: cargo build --release -p dt2-cli --features window");
        std::process::exit(2)
    }

    impl Window {
        pub fn present(
            &mut self,
            _width: usize,
            _height: usize,
            _pixels: &[u32],
            _leds: &dyn Fn(u8) -> Option<u32>,
        ) {
        }
        pub fn is_open(&self) -> bool {
            false
        }
        pub fn input_messages(&mut self) -> Vec<[u8; 2]> {
            Vec::new()
        }
    }
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
    match register {
        "RAMBAR" => cpu.rambar = value,
        "ACR0" => cpu.acr0 = value,
        "ACR1" => cpu.acr1 = value,
        "VBR" => cpu.vbr = value,
        "CACR" => cpu.cacr = value,
        _ => {}
    }
    if matches!(register, "RAMBAR" | "ACR0" | "ACR1" | "VBR" | "CACR") {
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
    dspi2: Option<Dspi>,
    ddr: Option<DdrController>,
    pit: Option<PitController>,
    dtim1: Option<DmaTimer1>,
    dtim3: Option<DmaTimer1>,
    intc0: Option<Intc0>,
    intc1: Option<Intc0>,
    intc2: Option<Intc2>,
    rgpio: Option<Rgpio>,
    rgpiobar: u32,
    current_sr: u16,
    current_d: [u32; 8],
    current_a: [u32; 8],
    uart8: Option<Uart8>,
    panel_stub: bool,
    panel_stub_func_held: bool,
    panel_stub_discovery_queued: bool,
    panel_stub_handshake_queued: bool,
    panel_stub_tx_tail: VecDeque<u8>,
    uart8_dma_daddr: u32,
    uart8_dma_ring_base: u32,
    uart8_dma_enabled: bool,
    uart8_rx_irq_pending: bool,
    uart8_tx_irq_pending: bool,
    rgpio_input_high: u16,
    rgpio_ready_model: bool,
    gpio_loopback: bool,
    gpio_loopback_level: bool,
    esdhc: Option<Esdhc>,
    esdhc_dma_irq_pending: bool,
    ssi_audio_period: Option<usize>,
    ssi_audio_rearm_at: Option<usize>,
    ssi_audio_irq_pending: bool,
    ssi_tx_raw: Option<Vec<u8>>,
    ssi_tx_dma_blocks: u64,
    ssi_tx_first_source: Option<u32>,
    esdhc_trace_enabled: bool,
    esdhc_commands_seen: u64,
    esdhc_write_blocks_seen: u64,
    esdhc_command_events: Vec<EsdhcCommandTrace>,
    esdhc_write_events: Vec<EsdhcWriteTrace>,
    esdhc_access_summaries: BTreeMap<u32, EsdhcRegisterSummary>,
    esdhc_last_access: Option<EsdhcAccessTrace>,
    esdhc_dma_events: Vec<EsdhcDmaTrace>,
    edma_deferred: std::collections::BTreeSet<u8>,
    uart8_rx_periodic: Option<(usize, Vec<Vec<u8>>, usize, usize)>,
    uart8_rx_once: bool,
    uart8_tx_file: Option<(
        std::io::BufWriter<std::fs::File>,
        std::io::BufWriter<std::fs::File>,
    )>,
    display: Option<display::Display>,
    fast_ram: Option<(u32, u32)>,
    mapped_ram_window: Option<(u32, u64)>,
    /// SRAM overlay accesses within its first 64 KiB go straight to the model.
    sram_fast: bool,
    /// Poll interrupts only when something may have changed (see `poll_due`).
    lazy_poll: bool,
    /// Retirements not yet added to the timers' lag; settled by `flush_timers`.
    timer_pending: u64,
    irq_dirty: bool,
    poll_countdown: u64,
    code_map: dt2_core::icache::CodeMap,
    /// False when no diagnostic needs the per-instruction state report.
    instruction_hooks: bool,
    /// False when no diagnostic reads the per-instruction register copies.
    capture_registers: bool,
    irq_log: Option<std::io::BufWriter<std::fs::File>>,
    dump_calls: Vec<CallDump>,
    dump_call_count: usize,
    edma_tcd: BTreeMap<u32, u8>,
    uart8_trace_events: Vec<Uart8TraceEvent>,
    suppressed_uart8_trace_events: usize,
    dspi1: Option<Dspi>,
    dspi_push_events: Vec<DspiPushEvent>,
    dspi_push_count: usize,
    dspi_pop_events: Vec<DspiPopEvent>,
    dspi_pop_count: usize,
    pit0_oneshot: bool,
    pit0_request_seen: bool,
    watch_write_range: Option<(u32, u32)>,
    watched_write_count: usize,
    watched_writes: VecDeque<WatchedWrite>,
    write_trace_range: Option<(usize, usize)>,
    write_trace_base: usize,
    firmware_write_trace: Vec<FirmwareWriteTrace>,
    watched_initial_bytes: Option<Vec<u8>>,
    watched_completion_tail: Option<(usize, Vec<u8>)>,
    watch_read_range: Option<(u32, u32)>,
    watched_read_count: usize,
    watched_reads: VecDeque<WatchedRead>,
    sample_access_trace_range: Option<(u32, u32)>,
    sample_access_trace: Vec<SampleAccessTrace>,
    trace_register_pc: Option<u32>,
    register_trace_count: usize,
    register_trace: VecDeque<RegisterTrace>,
    register_trace_stack: VecDeque<Vec<u32>>,
    instruction_trace_tail_limit: usize,
    instruction_trace_tail: VecDeque<RegisterTrace>,
    scheduler_trace_enabled: bool,
    scheduler_trace_events: VecDeque<SchedulerTraceEvent>,
    scheduler_task_entries: BTreeMap<u32, u32>,
    scheduler_task_creations: Vec<SchedulerTaskCreation>,
    conversion_trace_enabled: bool,
    conversion_trace_events: VecDeque<ConversionTraceEvent>,
    trace_a1_checkpoints: Vec<(u32, Option<RegisterTrace>)>,
    trace_a1_next_index: usize,
    trace_a0_range: Option<(u32, u32)>,
    first_a0_range_trace: Option<RegisterTrace>,
    trace_pc_range: Option<(u32, u32)>,
    synthetic_mcf_mmio: bool,
    dsp_link: bool,
    dsp_link_reply: u16,
    edma_trace_enabled: bool,
    nonram_bulk_trace_enabled: bool,
    nonram_bulk_transfers: Vec<NonRamBulkTransfer>,
    cpu_nonram_write_run: Option<CpuNonRamWriteRun>,
    synth_voice: Option<(u32, u32)>,
    synth_pos: Option<u32>,
    dsp_link_blocks: u32,
    access_events: Vec<AccessEvent>,
    section3_mmio_events: Vec<(usize, u32, AccessEvent)>,
    section3_mmio_count: usize,
    rgpio_access_events: Vec<(u32, AccessEvent)>,
    rgpio_access_count: usize,
    rgpio_trace_limit: usize,
    rgpio_trace: Vec<RgpioTraceEntry>,
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

#[derive(Clone)]
struct CallDump {
    pc: u32,
    buffer_arg: usize,
    length: usize,
    prefix: String,
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
const DSPI1_BASE: u32 = 0xFC03_C000;
const DSPI2_BASE: u32 = 0xEC03_8000;
const UART8_URB: u32 = 0xEC07_000C;
const UART8_TCD34_DADDR: u32 = 0xFC04_5450;
const UART8_TCD34_REQUEST: u32 = 0xFC04_4018;
const UART8_TCD34_CINT: u32 = 0xFC04_401C;
const UART8_TCD34_REQUEST_VALUE: u8 = 0x22;
const EDMA_SERQ: u32 = 0xFC04_4018;
// Pin driven through 0xEC09401B (set) / 0xEC094027 (clear mask) as seen by bit 3 of 0xEC09401A.
const GPIO_LOOPBACK_SET: u32 = 0xEC09_401B;
const GPIO_LOOPBACK_CLEAR: u32 = 0xEC09_4027;
const GPIO_LOOPBACK_SENSE: u32 = 0xEC09_401A;
const EDMA_TCD_WINDOW: std::ops::Range<u32> = 0xFC04_5000..0xFC04_6000;
const UART8_TX_DMA_CHANNEL: u8 = 0x23;
const ESDHC_DMA_CHANNEL: u8 = 0x3B;
// Audio block DMA (SSI0 TX), vector 170 = INTC1 source 42.
const SSI_AUDIO_DMA_CHANNEL: u8 = 0x32;
const SSI_AUDIO_INTC1_SOURCE: u32 = 42;
const DTIM1_SOURCE: u32 = 33;
const DTIM3_SOURCE: u32 = 35;
const SSI_AUDIO_RX_BUFFER: u32 = 0x4FE5_7100;
const VOICE_RECORD_BUILDER: u32 = 0x4013_9B54;
const SSI_AUDIO_TX_DATA: u32 = 0xFC0B_C000;
const UART8_DMA_RING_MASK: u32 = 0x3FF;
const PANEL_STUB_QUERY: [u8; 4] = [0x60, 0x02, 0x70, 0x00];
const MAX_DSPI_PUSH_EVENTS: usize = 262_144;
const MAX_DSPI_POP_EVENTS: usize = 262_144;
const MAX_UART8_TRACE_EVENTS: usize = 32_768;
const MCF_SRAM_ALIAS_WINDOW: (u32, u32) = (0x8000_0000, 0x9000_0000);
const SRAM_FAST_BASE: u32 = 0x8000_0000;
const SRAM_FAST_SIZE: u32 = 0x1_0000;
const MCF_PERIPHERAL_WINDOWS: [(u32, u32); 2] =
    [(0xEC00_0000, 0xED00_0000), (0xFC00_0000, 0xFD00_0000)];
const DDR_REGISTER_APERTURE: (u32, u32) = (0xFC0B_8000, 0xFC0B_8200);
const MAX_ACCESS_EVENTS: usize = 256;
const MAX_ACCESS_ADDRESSES: usize = 128;
const MAX_LOOP_TRACKED_PCS: usize = 4096;
const MAX_LOOP_HISTORY: usize = 4096;
const MAX_RTE_HISTORY: usize = 16;
const MAX_WATCHED_WRITES: usize = 16_384;
const MAX_WATCHED_READS: usize = 16_384;
const MAX_REGISTER_TRACE: usize = 192;
const REGISTER_TRACE_STACK_WORDS: usize = 12;
const MAX_SCHED_TRACE_EVENTS: usize = 1024;
const MAX_CONVERSION_TRACE_EVENTS: usize = 4096;
const PIT1_PCSR: u32 = 0xFC08_4000;
const NONRAM_BULK_TRACE_MIN_BYTES: u64 = 1024;
const NONRAM_BULK_TRACE_PREFIX_BYTES: usize = 16;

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

// Consecutive identical accesses (a polling loop) are folded into one entry.
struct RgpioTraceEntry {
    instruction: usize,
    pc: u32,
    event: AccessEvent,
    sp: u32,
    repeats: usize,
}

#[derive(Debug, Clone, Copy)]
struct AccessEvent {
    address: u32,
    width: u8,
    write: bool,
    value: u32,
    source: AccessSource,
}

#[derive(Clone, Copy)]
struct DspiPushEvent {
    instruction_count: usize,
    pc: u32,
    module: u8,
    word: u32,
    status_after: u32,
}

#[derive(Clone, Copy)]
struct DspiPopEvent {
    instruction_count: usize,
    pc: u32,
    module: u8,
    width_bits: u8,
    value: u32,
    status_after: u32,
}

#[derive(Clone, Copy)]
struct Uart8TraceEvent {
    instruction_count: usize,
    pc: u32,
    address: u32,
    value: u8,
    write: bool,
    dma: bool,
    dma_destination: u32,
}

#[derive(Clone, Copy)]
struct EsdhcCommandTrace {
    instruction_count: usize,
    pc: u32,
    command: u8,
    argument: u32,
    block_attributes: u32,
    completed_writes: u64,
    write_active: bool,
    write_buffer_bytes: usize,
}

#[derive(Clone, Copy)]
struct EsdhcWriteTrace {
    instruction_count: usize,
    pc: u32,
    first_block: u32,
    block_count: u64,
}

#[derive(Clone, Copy)]
struct EsdhcAccessTrace {
    instruction_count: usize,
    pc: u32,
    address: u32,
    width: u8,
    write: bool,
    value: u32,
}

#[derive(Clone, Copy)]
struct EsdhcRegisterSummary {
    reads: u64,
    writes: u64,
    last_value: u32,
}

#[derive(Clone, Copy)]
struct EsdhcDmaTrace {
    instruction_count: usize,
    pc: u32,
    channel: u8,
    operation: &'static str,
    address: u32,
    value: u32,
    detail: u32,
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
    operand: TstOperand,
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
    instruction_count: usize,
    pc: u32,
    address: u32,
    width_bits: u8,
    old_bytes: Vec<u8>,
    new_bytes: Vec<u8>,
    d: [u32; 8],
    a: [u32; 8],
    sr: u16,
}

struct WatchedRead {
    instruction_count: usize,
    pc: u32,
    address: u32,
    width_bits: u8,
    value: u32,
}

struct FirmwareWriteTrace {
    instruction_count: usize,
    pc: u32,
    actor: FirmwareWriteActor,
    address: u32,
    width_bits: u8,
    value: u32,
}

#[derive(Clone, Copy)]
enum NonRamBulkActor {
    Cpu,
    Dma { channel: u8, synthetic: bool },
}

struct NonRamBulkTransfer {
    instruction_count: usize,
    pc: u32,
    last_pc: u32,
    actor: NonRamBulkActor,
    source: Option<u32>,
    destination: u32,
    size: u64,
    prefix: Vec<u8>,
}

struct CpuNonRamWriteRun {
    instruction_count: usize,
    pc: u32,
    last_pc: u32,
    destination: u32,
    next_address: u32,
    size: u64,
    prefix: Vec<u8>,
}

#[derive(Clone, Copy)]
enum FirmwareWriteActor {
    Cpu,
    Dma(u8),
}

#[derive(Clone, Copy)]
enum SampleAccessActor {
    Cpu,
    Dma { channel: u8, peer: u32 },
}

struct SampleAccessTrace {
    instruction_count: usize,
    pc: u32,
    actor: SampleAccessActor,
    write: bool,
    address: u32,
    width_bits: u8,
    value: u32,
}

struct RegisterTrace {
    instruction_count: usize,
    pc: u32,
    d: [u32; 8],
    a: [u32; 8],
    sr: u16,
}

struct SchedulerTraceEvent {
    instruction_count: usize,
    pc: u32,
    d: [u32; 8],
    a: [u32; 8],
    sr: u16,
    current_tcb: Option<u32>,
    tcb_saved_sp: Option<u32>,
    task_entry: Option<u32>,
    saved_sp_word0: Option<u32>,
    tcb_words: Vec<u32>,
    live_stack_words: Vec<u32>,
    saved_stack_words: Vec<u32>,
}

struct SchedulerTaskCreation {
    instruction_count: usize,
    tcb: u32,
    entry: u32,
    priority: u32,
    stack_base: u32,
    stack_size: u32,
}

struct ConversionTraceEvent {
    instruction_count: usize,
    pc: u32,
    d1: u32,
    loop_index: Option<u32>,
    output_base: u32,
    compare_left: u32,
    compare_right: u32,
    sr: u16,
    branch_taken: Option<bool>,
    branch_target: Option<u32>,
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
            dspi2: None,
            ddr: None,
            pit: None,
            dtim1: None,
            dtim3: None,
            intc0: None,
            intc1: None,
            intc2: None,
            rgpio: None,
            rgpiobar: dt2_core::coldfire::RGPIOBAR_RESET,
            current_sr: 0x2000,
            current_d: [0; 8],
            current_a: [0; 8],
            uart8: None,
            panel_stub: false,
            panel_stub_func_held: false,
            panel_stub_discovery_queued: false,
            panel_stub_handshake_queued: false,
            panel_stub_tx_tail: VecDeque::new(),
            uart8_dma_daddr: 0,
            uart8_dma_ring_base: 0,
            uart8_dma_enabled: false,
            uart8_rx_irq_pending: false,
            uart8_tx_irq_pending: false,
            rgpio_input_high: 0,
            rgpio_ready_model: false,
            gpio_loopback: false,
            gpio_loopback_level: false,
            esdhc: None,
            esdhc_dma_irq_pending: false,
            ssi_audio_period: None,
            ssi_audio_rearm_at: None,
            ssi_audio_irq_pending: false,
            ssi_tx_raw: None,
            ssi_tx_dma_blocks: 0,
            ssi_tx_first_source: None,
            esdhc_trace_enabled: false,
            esdhc_commands_seen: 0,
            esdhc_write_blocks_seen: 0,
            esdhc_command_events: Vec::new(),
            esdhc_write_events: Vec::new(),
            esdhc_access_summaries: BTreeMap::new(),
            esdhc_last_access: None,
            esdhc_dma_events: Vec::new(),
            edma_deferred: std::collections::BTreeSet::new(),
            uart8_rx_periodic: None,
            uart8_rx_once: false,
            uart8_tx_file: None,
            display: None,
            fast_ram: None,
            mapped_ram_window: None,
            sram_fast: false,
            lazy_poll: false,
            timer_pending: 0,
            irq_dirty: true,
            poll_countdown: 0,
            code_map: dt2_core::icache::CodeMap::default(),
            instruction_hooks: true,
            capture_registers: true,
            irq_log: None,
            dump_calls: Vec::new(),
            dump_call_count: 0,
            edma_tcd: BTreeMap::new(),
            uart8_trace_events: Vec::new(),
            suppressed_uart8_trace_events: 0,
            dspi1: None,
            dspi_push_events: Vec::new(),
            dspi_push_count: 0,
            dspi_pop_events: Vec::new(),
            dspi_pop_count: 0,
            pit0_oneshot: false,
            pit0_request_seen: false,
            watch_write_range: None,
            watched_write_count: 0,
            watched_writes: VecDeque::new(),
            write_trace_range: None,
            write_trace_base: 0,
            firmware_write_trace: Vec::new(),
            watched_initial_bytes: None,
            watched_completion_tail: None,
            watch_read_range: None,
            watched_read_count: 0,
            watched_reads: VecDeque::new(),
            sample_access_trace_range: None,
            sample_access_trace: Vec::new(),
            trace_register_pc: None,
            register_trace_count: 0,
            register_trace: VecDeque::new(),
            register_trace_stack: VecDeque::new(),
            instruction_trace_tail_limit: 0,
            instruction_trace_tail: VecDeque::new(),
            scheduler_trace_enabled: false,
            scheduler_trace_events: VecDeque::new(),
            scheduler_task_entries: BTreeMap::new(),
            scheduler_task_creations: Vec::new(),
            conversion_trace_enabled: false,
            conversion_trace_events: VecDeque::new(),
            trace_a1_checkpoints: Vec::new(),
            trace_a1_next_index: 0,
            trace_a0_range: None,
            first_a0_range_trace: None,
            trace_pc_range: None,
            synthetic_mcf_mmio: false,
            dsp_link: false,
            dsp_link_reply: 0xFFFF,
            edma_trace_enabled: false,
            nonram_bulk_trace_enabled: false,
            nonram_bulk_transfers: Vec::new(),
            cpu_nonram_write_run: None,
            synth_voice: None,
            synth_pos: None,
            dsp_link_blocks: 0,
            access_events: Vec::new(),
            section3_mmio_events: Vec::new(),
            section3_mmio_count: 0,
            rgpio_access_events: Vec::new(),
            rgpio_access_count: 0,
            rgpio_trace_limit: 0,
            rgpio_trace: Vec::new(),
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

    fn watch_write_range(&mut self, range: (u32, u32)) {
        self.watch_write_range = Some(range);
    }

    fn watch_read_range(&mut self, range: (u32, u32)) {
        self.watch_read_range = Some(range);
    }

    fn watched_memory_byte(&mut self, address: u32) -> Result<u8, BusError> {
        if let Some(sram) = self.sram_for(address) {
            sram.read8(address)
        } else {
            self.ram.read8(self.remapped_ram_address(address))
        }
    }

    fn capture_watch_initial(&mut self) -> Result<(), BusError> {
        let Some((start, end)) = self.watch_write_range else {
            return Ok(());
        };
        let mut bytes = Vec::with_capacity((end - start) as usize);
        for address in start..end {
            bytes.push(self.watched_memory_byte(address)?);
        }
        self.watched_initial_bytes = Some(bytes);
        Ok(())
    }

    fn watched_old_bytes(
        &mut self,
        address: u32,
        width_bytes: u8,
    ) -> Result<Option<(u32, Vec<u8>)>, BusError> {
        let Some((watch_start, watch_end)) = self.watch_write_range else {
            return Ok(None);
        };
        let write_end = address.saturating_add(u32::from(width_bytes));
        let start = address.max(watch_start);
        let end = write_end.min(watch_end);
        if start >= end {
            return Ok(None);
        }
        let mut bytes = Vec::with_capacity((end - start) as usize);
        for current in start..end {
            bytes.push(self.watched_memory_byte(current)?);
        }
        Ok(Some((start, bytes)))
    }

    fn trace_registers_at(&mut self, pc: u32) {
        self.trace_register_pc = Some(pc);
    }

    fn trace_instruction_tail(&mut self, length: usize) {
        self.instruction_trace_tail_limit = length;
        self.instruction_trace_tail.clear();
    }

    fn enable_scheduler_trace(&mut self) {
        self.scheduler_trace_enabled = true;
        self.scheduler_trace_events.clear();
    }

    fn enable_conversion_trace(&mut self) {
        self.conversion_trace_enabled = true;
        self.conversion_trace_events.clear();
    }

    fn enable_esdhc_trace(&mut self) {
        self.esdhc_trace_enabled = true;
        self.sync_esdhc_trace_baseline();
    }

    fn sync_esdhc_trace_baseline(&mut self) {
        if let Some(esdhc) = &self.esdhc {
            self.esdhc_commands_seen = esdhc.commands_executed();
            self.esdhc_write_blocks_seen = esdhc.write_trace_state().0;
        }
    }

    fn record_conversion_trace(&mut self, mut event: ConversionTraceEvent) {
        if self.conversion_trace_events.len() == MAX_CONVERSION_TRACE_EVENTS {
            self.conversion_trace_events.pop_front();
        }
        event.instruction_count = self.current_instruction_count;
        self.conversion_trace_events.push_back(event);
    }

    fn trace_a1_checkpoints(&mut self, checkpoints: &[u32]) {
        self.trace_a1_checkpoints = checkpoints
            .iter()
            .copied()
            .map(|address| (address, None))
            .collect();
        self.trace_a1_checkpoints
            .sort_by_key(|(address, _)| *address);
        self.trace_a1_next_index = 0;
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

    fn rgpio_enabled(&self, address: u32) -> bool {
        Rgpio::contains(address)
            && self.rgpiobar & 1 != 0
            && (self.rgpiobar & 2 == 0 || self.current_sr & 0x2000 != 0)
    }

    fn rgpio_for(&mut self, address: u32) -> Option<&mut Rgpio> {
        let enabled = self.rgpio_enabled(address);
        let input_high = self.rgpio_input_high;
        let rgpio = self.rgpio.as_mut().filter(|_| enabled)?;
        rgpio.set_input_high(input_high);
        rgpio.set_ready_model(self.rgpio_ready_model);
        Some(rgpio)
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

    fn snapshot_source_on_write(&mut self, address: u32, width_bytes: u32) -> Result<(), BusError> {
        let Some(snapshot) = self.ram_snapshot.filter(|snapshot| {
            !snapshot.active
                && snapshot.watch >= address
                && snapshot.watch - address < width_bytes
                && (snapshot.trigger_pc_start..snapshot.trigger_pc_end).contains(&self.current_pc)
        }) else {
            return Ok(());
        };
        for offset in 0..snapshot.length {
            let value = self.ram.read8(snapshot.source + offset)?;
            self.code_map.note_write(snapshot.destination + offset, 1);
            self.ram.write8(snapshot.destination + offset, value)?;
        }
        if let Some(active) = &mut self.ram_snapshot {
            active.active = true;
        }
        Ok(())
    }

    fn remapped_ram_address(&self, address: u32) -> u32 {
        self.ram_remap
            .filter(|remap| address >= remap.source && address - remap.source < remap.length)
            .map_or(address, |remap| {
                remap.destination + (address - remap.source)
            })
    }

    fn record_watched_write(
        &mut self,
        address: u32,
        width_bytes: u8,
        value: u32,
        old: Option<(u32, Vec<u8>)>,
    ) {
        let Some((watched_address, old_bytes)) = old else {
            return;
        };
        self.watched_write_count += 1;
        if self.watched_writes.len() >= MAX_WATCHED_WRITES {
            self.watched_writes.pop_front();
        }
        let value_bytes = value.to_be_bytes();
        let value_start = 4 - usize::from(width_bytes) + (watched_address - address) as usize;
        let new_bytes = value_bytes[value_start..value_start + old_bytes.len()].to_vec();
        self.watched_writes.push_back(WatchedWrite {
            instruction_count: self.current_instruction_count,
            pc: self.current_pc,
            address: watched_address,
            width_bits: width_bytes * 8,
            old_bytes,
            new_bytes,
            d: self.current_d,
            a: self.current_a,
            sr: self.current_sr,
        });

        let Some((_, watch_end)) = self.watch_write_range else {
            return;
        };
        let write_end = address.saturating_add(u32::from(width_bytes));
        if self.watched_completion_tail.is_none()
            && address < watch_end
            && write_end >= watch_end
            && watch_end >= 0x40
        {
            let tail_start = watch_end - 0x40;
            let mut tail = Vec::with_capacity(0x40);
            for current in tail_start..watch_end {
                let Ok(byte) = self.watched_memory_byte(current) else {
                    return;
                };
                tail.push(byte);
            }
            self.watched_completion_tail = Some((self.current_instruction_count, tail));
        }
    }

    fn record_watched_read(&mut self, address: u32, width_bytes: u8, value: u32) {
        let Some((watch_start, watch_end)) = self.watch_read_range else {
            return;
        };
        let read_end = address.saturating_add(u32::from(width_bytes));
        if address >= watch_end || read_end <= watch_start {
            return;
        }
        self.watched_read_count += 1;
        if self.watched_reads.len() == MAX_WATCHED_READS {
            self.watched_reads.pop_front();
        }
        self.watched_reads.push_back(WatchedRead {
            instruction_count: self.current_instruction_count,
            pc: self.current_pc,
            address,
            width_bits: width_bytes * 8,
            value,
        });
    }

    fn record_firmware_write(
        &mut self,
        actor: FirmwareWriteActor,
        address: u32,
        width_bits: u8,
        value: u32,
    ) {
        let Some((start_offset, end_offset)) = self.write_trace_range else {
            return;
        };
        let start = self.write_trace_base.saturating_add(start_offset);
        let end = self.write_trace_base.saturating_add(end_offset);
        if !(start..end).contains(&self.current_instruction_count) {
            return;
        }
        self.firmware_write_trace.push(FirmwareWriteTrace {
            instruction_count: self.current_instruction_count,
            pc: self.current_pc,
            actor,
            address,
            width_bits,
            value,
        });
    }

    fn is_ram_destination(&self, address: u32, length: u64) -> bool {
        let Some(end) = u64::from(address).checked_add(length) else {
            return false;
        };
        let in_flat_ram = self
            .mapped_ram_window
            .is_some_and(|(start, stop)| address >= start && end <= stop);
        let in_sram_overlay = self.sram_overlay.is_some()
            && ((u64::from(MCF_SRAM_ALIAS_WINDOW.0)..=u64::from(MCF_SRAM_ALIAS_WINDOW.1))
                .contains(&u64::from(address))
                && end <= u64::from(MCF_SRAM_ALIAS_WINDOW.1))
            || (self.sram_overlay_sbf_alias && address < 0x1_0000 && end <= 0x1_0000);
        in_flat_ram || in_sram_overlay
    }

    fn finish_cpu_nonram_write_run(&mut self) {
        let Some(run) = self.cpu_nonram_write_run.take() else {
            return;
        };
        if run.size < NONRAM_BULK_TRACE_MIN_BYTES {
            return;
        }
        self.nonram_bulk_transfers.push(NonRamBulkTransfer {
            instruction_count: run.instruction_count,
            pc: run.pc,
            last_pc: run.last_pc,
            actor: NonRamBulkActor::Cpu,
            source: None,
            destination: run.destination,
            size: run.size,
            prefix: run.prefix,
        });
    }

    fn record_nonram_cpu_write(&mut self, address: u32, width_bytes: u8, value: u32) {
        if !self.nonram_bulk_trace_enabled {
            return;
        }
        if self.is_ram_destination(address, u64::from(width_bytes)) {
            self.finish_cpu_nonram_write_run();
            return;
        }
        let Some(next_address) = address.checked_add(u32::from(width_bytes)) else {
            self.finish_cpu_nonram_write_run();
            return;
        };
        if !self
            .cpu_nonram_write_run
            .as_ref()
            .is_some_and(|run| run.next_address == address || run.destination == address)
        {
            self.finish_cpu_nonram_write_run();
            self.cpu_nonram_write_run = Some(CpuNonRamWriteRun {
                instruction_count: self.current_instruction_count,
                pc: self.current_pc,
                last_pc: self.current_pc,
                destination: address,
                next_address,
                size: u64::from(width_bytes),
                prefix: Vec::with_capacity(NONRAM_BULK_TRACE_PREFIX_BYTES),
            });
        } else if let Some(run) = &mut self.cpu_nonram_write_run {
            run.last_pc = self.current_pc;
            run.next_address = next_address;
            run.size += u64::from(width_bytes);
        }
        if let Some(run) = &mut self.cpu_nonram_write_run {
            let bytes = value.to_be_bytes();
            let start = 4 - usize::from(width_bytes);
            let needed = NONRAM_BULK_TRACE_PREFIX_BYTES.saturating_sub(run.prefix.len());
            run.prefix
                .extend_from_slice(&bytes[start..start + needed.min(usize::from(width_bytes))]);
        }
    }

    fn record_nonram_dma_transfer(
        &mut self,
        channel: u8,
        synthetic: bool,
        source: u32,
        destination: u32,
        size: u64,
        prefix: Vec<u8>,
    ) {
        if !self.nonram_bulk_trace_enabled {
            return;
        }
        self.finish_cpu_nonram_write_run();
        if size < NONRAM_BULK_TRACE_MIN_BYTES || self.is_ram_destination(destination, 1) {
            return;
        }
        self.nonram_bulk_transfers.push(NonRamBulkTransfer {
            instruction_count: self.current_instruction_count,
            pc: self.current_pc,
            last_pc: self.current_pc,
            actor: NonRamBulkActor::Dma { channel, synthetic },
            source: Some(source),
            destination,
            size,
            prefix: prefix
                .into_iter()
                .take(NONRAM_BULK_TRACE_PREFIX_BYTES)
                .collect(),
        });
    }

    fn record_sample_access(
        &mut self,
        actor: SampleAccessActor,
        address: u32,
        width_bytes: u8,
        write: bool,
        value: u32,
    ) {
        let Some((start, end)) = self.sample_access_trace_range else {
            return;
        };
        let access_end = address.saturating_add(u32::from(width_bytes));
        if address >= end || access_end <= start {
            return;
        }
        self.sample_access_trace.push(SampleAccessTrace {
            instruction_count: self.current_instruction_count,
            pc: self.current_pc,
            actor,
            write,
            address,
            width_bits: width_bytes * 8,
            value,
        });
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

    /// Routes plain RAM accesses straight to memory when no diagnostic needs to observe them.
    /// Returns why the fast path stayed off, or None when it is on.
    fn enable_fast_ram(&mut self, base: u32, size: u32) -> Option<&'static str> {
        let end = u64::from(base) + u64::from(size);
        self.mapped_ram_window = Some((base, end));
        let overlaps =
            |start: u32, stop: u32| u64::from(base) < u64::from(stop) && u64::from(start) < end;
        let peripheral = MCF_PERIPHERAL_WINDOWS.iter().any(|&(s, e)| overlaps(s, e))
            || overlaps(MCF_SRAM_ALIAS_WINDOW.0, MCF_SRAM_ALIAS_WINDOW.1)
            || overlaps(
                dt2_core::rgpio::RGPIO_BASE,
                dt2_core::rgpio::RGPIO_BASE + 0x10,
            )
            || overlaps(0, 0x1_0000);
        let reason = if peripheral || end > 1 << 32 {
            Some("RAM window overlaps a peripheral or SRAM window")
        } else if self.ram_remap.is_some() {
            Some("RAM_REMAP active")
        } else if self.ram_snapshot.is_some() {
            Some("RAM_SNAPSHOT active")
        } else if self.watch_write_range.is_some() {
            Some("WATCH_WRITE active")
        } else if self.watch_read_range.is_some() {
            Some("WATCH_READ active")
        } else if self.sample_access_trace_range.is_some() {
            Some("SAMPLE_ACCESS_TRACE active")
        } else if self.synth_voice.is_some() {
            Some("SYNTH_VOICE active")
        } else if self.loop_detection {
            Some("LOOP_DETECT active")
        } else if self.mmio.keys().any(|&a| overlaps(a, a.saturating_add(1))) {
            Some("seeded MMIO inside the RAM window")
        } else {
            None
        };
        if reason.is_none() {
            self.fast_ram = Some((base, (end - 1) as u32));
            self.lazy_poll = !self.pit0_oneshot;
            let sram_range = SRAM_FAST_BASE..SRAM_FAST_BASE + SRAM_FAST_SIZE;
            self.sram_fast = self.sram_overlay.is_some()
                && !self.mmio.keys().any(|a| sram_range.contains(a))
                && !self.edma_tcd.keys().any(|a| sram_range.contains(a))
                && !Rgpio::contains(SRAM_FAST_BASE)
                && !self.rgpio_enabled(SRAM_FAST_BASE);
            self.code_map.enable(base, size);
        }
        reason
    }

    /// Turns off per-instruction diagnostics that nothing requested; returns a status label.
    fn configure_instruction_hooks(&mut self) -> &'static str {
        self.capture_registers = self.watch_write_range.is_some()
            || self.write_trace_range.is_some()
            || self.rgpio_trace_limit > 0
            || self.conversion_trace_enabled;
        self.instruction_hooks = !self.dump_calls.is_empty()
            || self.instruction_trace_tail_limit > 0
            || self.scheduler_trace_enabled
            || self.conversion_trace_enabled
            || self.trace_a0_range.is_some()
            || !self.trace_a1_checkpoints.is_empty()
            || self.trace_register_pc.is_some()
            || self.write_trace_range.is_some()
            || self.synth_voice.is_some();
        if self.instruction_hooks {
            "full"
        } else {
            "minimal"
        }
    }

    fn write_irq_log(&mut self, line: &str) {
        use std::io::Write;
        if let Some(log) = &mut self.irq_log {
            let _ = log.write_all(line.as_bytes());
        }
    }

    /// Retirements until a time-driven event (timer expiry, SSI re-arm) may change what is pending.
    fn instructions_until_poll_event(&self) -> u64 {
        let mut n = u64::MAX;
        if let Some(pit) = &self.pit {
            n = n.min(pit.instructions_until_event());
        }
        if let Some(dtim1) = &self.dtim1 {
            n = n.min(dtim1.instructions_until_event());
        }
        if let Some(dtim3) = &self.dtim3 {
            n = n.min(dtim3.instructions_until_event());
        }
        if let Some(at) = self.ssi_audio_rearm_at {
            n = n.min((at.saturating_sub(self.current_instruction_count) as u64).max(1));
        }
        n
    }

    /// Retirements by an external engine: `count` advances the instruction count, `timers` the
    /// timers' lag and the poll countdown (they differ by the instruction currently executing).
    fn retire_external(&mut self, count: usize, timers: usize) {
        self.current_instruction_count += count;
        self.poll_countdown = self.poll_countdown.saturating_sub(timers as u64);
        self.timer_pending += timers as u64;
    }

    /// Adds batched retirements to every timer.
    fn flush_timers(&mut self) {
        let n = std::mem::take(&mut self.timer_pending);
        if n == 0 {
            return;
        }
        if let Some(pit) = &mut self.pit {
            pit.advance_instructions(n);
        }
        if let Some(dtim1) = &mut self.dtim1 {
            dtim1.advance_instructions(n);
        }
        if let Some(dtim3) = &mut self.dtim3 {
            dtim3.advance_instructions(n);
        }
    }

    fn in_fast_sram(&self, address: u32, width: u32) -> bool {
        self.sram_fast && address.wrapping_sub(SRAM_FAST_BASE) <= SRAM_FAST_SIZE - width
    }

    fn in_fast_ram(&self, address: u32, width: u32) -> bool {
        self.fast_ram.is_some_and(|(start, last)| {
            address >= start && address.checked_add(width - 1).is_some_and(|a| a <= last)
        })
    }

    fn source_for_byte(&self, address: u32) -> AccessSource {
        if self.mmio.contains_key(&address) {
            AccessSource::Seeded
        } else if self
            .dspi0
            .as_ref()
            .is_some_and(|dspi| dspi.contains(address))
            || self
                .dspi1
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
            || self
                .rgpio
                .as_ref()
                .is_some_and(|_| self.rgpio_enabled(address))
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
            && !Rgpio::contains(event.address)
        {
            return;
        }
        if Rgpio::contains(event.address) {
            self.rgpio_access_count = self.rgpio_access_count.saturating_add(1);
            if self.rgpio_access_events.len() < 32 {
                self.rgpio_access_events.push((self.current_pc, event));
            }
            if self.rgpio_trace_limit > 0 {
                let same = self.rgpio_trace.last().is_some_and(|last| {
                    last.pc == self.current_pc
                        && last.event.write == event.write
                        && last.event.address == event.address
                        && last.event.value == event.value
                        && last.event.width == event.width
                });
                if same {
                    self.rgpio_trace.last_mut().unwrap().repeats += 1;
                } else if self.rgpio_trace.len() < self.rgpio_trace_limit {
                    self.rgpio_trace.push(RgpioTraceEntry {
                        instruction: self.current_instruction_count,
                        pc: self.current_pc,
                        event,
                        sp: self.current_a[7],
                        repeats: 1,
                    });
                }
            }
        }
        if (0x4000_0000..0x8000_0000).contains(&self.current_pc) {
            self.section3_mmio_count = self.section3_mmio_count.saturating_add(1);
            if self.section3_mmio_events.len() < 128 {
                self.section3_mmio_events.push((
                    self.current_instruction_count,
                    self.current_pc,
                    event,
                ));
            }
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

    fn record_esdhc_access(&mut self, address: u32, width: u8, write: bool, value: u32) {
        if !self.esdhc_trace_enabled {
            return;
        }
        let register = (address - ESDHC_BASE) & !3;
        let summary = self
            .esdhc_access_summaries
            .entry(register)
            .or_insert(EsdhcRegisterSummary {
                reads: 0,
                writes: 0,
                last_value: value,
            });
        if write {
            summary.writes = summary.writes.saturating_add(1);
        } else {
            summary.reads = summary.reads.saturating_add(1);
        }
        summary.last_value = value;
        self.esdhc_last_access = Some(EsdhcAccessTrace {
            instruction_count: self.current_instruction_count,
            pc: self.current_pc,
            address,
            width,
            write,
            value,
        });

        let Some(esdhc) = self.esdhc.as_ref() else {
            return;
        };
        let commands = esdhc.commands_executed();
        if commands > self.esdhc_commands_seen {
            let (xfertyp, argument, block_attributes) = esdhc.trace_registers();
            let (completed_writes, _, write_active, write_buffer_bytes) = esdhc.write_trace_state();
            self.esdhc_command_events.push(EsdhcCommandTrace {
                instruction_count: self.current_instruction_count,
                pc: self.current_pc,
                command: ((xfertyp >> 24) & 0x3f) as u8,
                argument,
                block_attributes,
                completed_writes,
                write_active,
                write_buffer_bytes,
            });
            self.esdhc_commands_seen = commands;
        }

        let (completed_writes, last_written_block, _, _) = esdhc.write_trace_state();
        if completed_writes > self.esdhc_write_blocks_seen {
            if let Some(last_block) = last_written_block {
                let block_count = completed_writes - self.esdhc_write_blocks_seen;
                self.esdhc_write_events.push(EsdhcWriteTrace {
                    instruction_count: self.current_instruction_count,
                    pc: self.current_pc,
                    first_block: last_block.wrapping_sub((block_count - 1) as u32),
                    block_count,
                });
            }
            self.esdhc_write_blocks_seen = completed_writes;
        }
    }

    fn record_esdhc_dma_event(
        &mut self,
        channel: u8,
        operation: &'static str,
        address: u32,
        value: u32,
        detail: u32,
    ) {
        if self.esdhc_trace_enabled
            && channel == ESDHC_DMA_CHANNEL
            && self.esdhc_dma_events.len() < 256
        {
            self.esdhc_dma_events.push(EsdhcDmaTrace {
                instruction_count: self.current_instruction_count,
                pc: self.current_pc,
                channel,
                operation,
                address,
                value,
                detail,
            });
        }
    }

    fn record_dspi_push(&mut self, address: u32, word: u32) {
        let module = if address == DSPI0_BASE + 0x34 {
            0
        } else if address == DSPI1_BASE + 0x34 {
            1
        } else {
            return;
        };
        self.dspi_push_count = self.dspi_push_count.saturating_add(1);
        if self.dspi_push_events.len() < MAX_DSPI_PUSH_EVENTS {
            let status_after = if module == 0 {
                self.dspi0
                    .as_mut()
                    .and_then(|dspi| dspi.read32(DSPI0_BASE + 0x2C).ok())
            } else {
                self.dspi1
                    .as_mut()
                    .and_then(|dspi| dspi.read32(DSPI1_BASE + 0x2C).ok())
            }
            .unwrap_or(0);
            self.dspi_push_events.push(DspiPushEvent {
                instruction_count: self.current_instruction_count,
                pc: self.current_pc,
                module,
                word,
                status_after,
            });
        }
    }

    fn record_dspi_pop(&mut self, address: u32, width_bits: u8, value: u32) {
        let aligned = address & !3;
        let (module, base) = if aligned == DSPI0_BASE + 0x38 {
            (0, DSPI0_BASE)
        } else if aligned == DSPI1_BASE + 0x38 {
            (1, DSPI1_BASE)
        } else {
            return;
        };
        self.dspi_pop_count = self.dspi_pop_count.saturating_add(1);
        if self.dspi_pop_events.len() < MAX_DSPI_POP_EVENTS {
            let status_after = if module == 0 {
                self.dspi0
                    .as_mut()
                    .and_then(|dspi| dspi.read32(base + 0x2C).ok())
            } else {
                self.dspi1
                    .as_mut()
                    .and_then(|dspi| dspi.read32(base + 0x2C).ok())
            }
            .unwrap_or(0);
            self.dspi_pop_events.push(DspiPopEvent {
                instruction_count: self.current_instruction_count,
                pc: self.current_pc,
                module,
                width_bits,
                value,
                status_after,
            });
        }
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
        let Some((start, end)) = self.watch_write_range else {
            return;
        };
        println!(
            "write watch at {start:#010x}..{end:#010x}: {} matching writes (showing most recent {})",
            self.watched_write_count,
            self.watched_writes.len()
        );
        if let Some(bytes) = &self.watched_initial_bytes {
            println!("  initial={}", hex_bytes(bytes));
        }
        for write in &self.watched_writes {
            println!(
                "  write{} instruction={} PC={:#010x} address={:#010x} old={} new={} SR={:#06x} D={:08x?} A={:08x?}",
                write.width_bits,
                write.instruction_count,
                write.pc,
                write.address,
                hex_bytes(&write.old_bytes),
                hex_bytes(&write.new_bytes),
                write.sr,
                write.d,
                write.a
            );
        }
        if let Some((instruction_count, bytes)) = &self.watched_completion_tail {
            println!(
                "  first write reaching range end at instruction={instruction_count}: tail64={}",
                hex_bytes(bytes)
            );
        }
    }

    fn print_firmware_write_trace(&self) {
        let Some((start_offset, end_offset)) = self.write_trace_range else {
            return;
        };
        let start = self.write_trace_base.saturating_add(start_offset);
        let end = self.write_trace_base.saturating_add(end_offset);
        println!(
            "bus write trace {start}..{end}: {} write(s)",
            self.firmware_write_trace.len()
        );
        for write in &self.firmware_write_trace {
            let actor = match write.actor {
                FirmwareWriteActor::Cpu => "CPU".to_owned(),
                FirmwareWriteActor::Dma(channel) => format!("DMA{channel}"),
            };
            println!(
                "  WRITE actor={actor} instruction={} PC={:#010x} width={} address={:#010x} value={:#010x}",
                write.instruction_count,
                write.pc,
                write.width_bits,
                write.address,
                write.value
            );
        }
    }

    fn print_nonram_bulk_trace(&mut self) {
        if !self.nonram_bulk_trace_enabled {
            return;
        }
        self.finish_cpu_nonram_write_run();
        self.nonram_bulk_transfers
            .sort_by_key(|transfer| transfer.instruction_count);
        println!(
            "non-RAM bulk I/O trace: {} transfer(s) >= {NONRAM_BULK_TRACE_MIN_BYTES} bytes",
            self.nonram_bulk_transfers.len()
        );
        for transfer in &self.nonram_bulk_transfers {
            let actor = match transfer.actor {
                NonRamBulkActor::Cpu => "CPU".to_owned(),
                NonRamBulkActor::Dma {
                    channel: 29,
                    synthetic: true,
                } => "DMA29 (synthetic DSP_LINK)".to_owned(),
                NonRamBulkActor::Dma {
                    channel: SSI_AUDIO_DMA_CHANNEL,
                    synthetic: true,
                } => format!("DMA{SSI_AUDIO_DMA_CHANNEL} (modeled SSI servicing)"),
                NonRamBulkActor::Dma {
                    channel,
                    synthetic: false,
                } => format!("DMA{channel}"),
                NonRamBulkActor::Dma {
                    channel,
                    synthetic: true,
                } => format!("DMA{channel} (synthetic)"),
            };
            let pc = if transfer.pc == transfer.last_pc {
                format!("{:#010x}", transfer.pc)
            } else {
                format!("{:#010x}..{:#010x}", transfer.pc, transfer.last_pc)
            };
            let source = transfer
                .source
                .map_or_else(String::new, |address| format!(" source={address:#010x}"));
            println!(
                "  BULK_IO actor={actor} instruction={} PC={pc} destination={:#010x} size={}{} first16={}",
                transfer.instruction_count,
                transfer.destination,
                transfer.size,
                source,
                hex_bytes(&transfer.prefix)
            );
        }
    }

    fn print_watched_reads(&self) {
        let Some((start, end)) = self.watch_read_range else {
            return;
        };
        println!(
            "read watch at {start:#010x}..{end:#010x}: {} matching reads (showing most recent {})",
            self.watched_read_count,
            self.watched_reads.len()
        );
        for read in &self.watched_reads {
            println!(
                "  read{} instruction={} PC={:#010x} address={:#010x} value={:#010x}",
                read.width_bits, read.instruction_count, read.pc, read.address, read.value
            );
        }
    }

    fn print_sample_access_trace(&self) {
        let Some((start, end)) = self.sample_access_trace_range else {
            return;
        };
        println!(
            "sample access trace {start:#010x}..{end:#010x}: {} access(es)",
            self.sample_access_trace.len()
        );
        for (index, access) in self.sample_access_trace.iter().enumerate() {
            let operation = if access.write { "WRITE" } else { "READ" };
            match access.actor {
                SampleAccessActor::Cpu => println!(
                    "  SAMPLE_ACCESS #{:06} instruction={} PC={:#010x} CPU {operation}{} address={:#010x} value={:#010x}",
                    index + 1,
                    access.instruction_count,
                    access.pc,
                    access.width_bits,
                    access.address,
                    access.value
                ),
                SampleAccessActor::Dma { channel, peer } => println!(
                    "  SAMPLE_ACCESS #{:06} instruction={} PC={:#010x} DMA channel={channel} {operation}{} address={:#010x} value={:#010x} peer={peer:#010x}",
                    index + 1,
                    access.instruction_count,
                    access.pc,
                    access.width_bits,
                    access.address,
                    access.value
                ),
            }
        }
    }

    fn print_register_trace(&self) {
        if let Some(pc) = self.trace_register_pc {
            println!(
                "register trace at PC={pc:#010x}: {} hit(s), {} snapshots retained (showing most recent)",
                self.register_trace_count,
                self.register_trace.len()
            );
            for (trace, stack) in self.register_trace.iter().zip(&self.register_trace_stack) {
                println!(
                    "  PC={:#010x} SR={:#06x} D0={:#010x} D1={:#010x} D2={:#010x} D3={:#010x} A0={:#010x} A1={:#010x} A2={:#010x} A3={:#010x} A4={:#010x} A5={:#010x} A6={:#010x} A7={:#010x}",
                    trace.pc,
                    trace.sr,
                    trace.d[0],
                    trace.d[1],
                    trace.d[2],
                    trace.d[3],
                    trace.a[0],
                    trace.a[1],
                    trace.a[2],
                    trace.a[3],
                    trace.a[4],
                    trace.a[5],
                    trace.a[6],
                    trace.a[7]
                );
                println!(
                    "    instruction={} stack(A7)={:08x?}",
                    trace.instruction_count, stack
                );
            }
        }
        if let (Some((start, end)), Some(trace)) = (self.trace_a0_range, &self.first_a0_range_trace)
        {
            println!(
            "first snapshot with A0 in {start:#010x}..{end:#010x}: PC={:#010x} SR={:#06x} D0={:#010x} D1={:#010x} D2={:#010x} D3={:#010x} A0={:#010x} A1={:#010x} A2={:#010x} A3={:#010x} A4={:#010x} A5={:#010x} A6={:#010x} A7={:#010x}",
                trace.pc,
                trace.sr,
                trace.d[0],
                trace.d[1],
                trace.d[2],
                trace.d[3],
                trace.a[0],
                trace.a[1],
                trace.a[2],
                trace.a[3],
                trace.a[4],
                trace.a[5],
                trace.a[6],
                trace.a[7]
            );
        }
        for (threshold, trace) in &self.trace_a1_checkpoints {
            if let Some(trace) = trace {
                println!(
                    "runtime checkpoint requested_output={threshold:#x} actual_output={:#x} input={:#x} input_offset={:#x} PC={:#010x}",
                    trace.a[1].wrapping_sub(0x4000_0400),
                    trace.a[0],
                    trace.a[0].wrapping_sub(0x4020_0008),
                    trace.pc
                );
            } else {
                println!("runtime checkpoint {threshold:#x} not reached");
            }
        }
    }

    fn print_instruction_trace_tail(&self) {
        if self.instruction_trace_tail_limit == 0 {
            return;
        }
        println!(
            "instruction trace tail: {} snapshots (requested {})",
            self.instruction_trace_tail.len(),
            self.instruction_trace_tail_limit
        );
        for trace in &self.instruction_trace_tail {
            println!(
                "  instruction={} PC={:#010x} SR={:#06x} D0={:#010x} D1={:#010x} D2={:#010x} D3={:#010x} D4={:#010x} D5={:#010x} D6={:#010x} D7={:#010x} A0={:#010x} A1={:#010x} A2={:#010x} A3={:#010x} A4={:#010x} A5={:#010x} A6={:#010x} A7={:#010x}",
                trace.instruction_count,
                trace.pc,
                trace.sr,
                trace.d[0],
                trace.d[1],
                trace.d[2],
                trace.d[3],
                trace.d[4],
                trace.d[5],
                trace.d[6],
                trace.d[7],
                trace.a[0],
                trace.a[1],
                trace.a[2],
                trace.a[3],
                trace.a[4],
                trace.a[5],
                trace.a[6],
                trace.a[7]
            );
        }
    }

    fn print_scheduler_trace(&self) {
        if !self.scheduler_trace_enabled {
            return;
        }
        println!(
            "scheduler/task events: {} event(s) (captured at list PCs, semaphore pends, delay, and task return)",
            self.scheduler_trace_events.len()
        );
        println!(
            "scheduler task contexts: {}",
            self.scheduler_task_creations.len()
        );
        for task in &self.scheduler_task_creations {
            println!(
                "  instruction={} TCB={:#010x} entry={:#010x} priority={} stack={:#010x}..{:#010x}",
                task.instruction_count,
                task.tcb,
                task.entry,
                task.priority,
                task.stack_base,
                task.stack_base.saturating_add(task.stack_size)
            );
        }
        for event in &self.scheduler_trace_events {
            let current_tcb = event
                .current_tcb
                .map_or_else(|| "none".to_string(), |address| format!("{address:#010x}"));
            let saved_sp = event
                .tcb_saved_sp
                .map_or_else(|| "none".to_string(), |address| format!("{address:#010x}"));
            let task_entry = event.task_entry.map_or_else(
                || "unknown".to_string(),
                |address| format!("{address:#010x}"),
            );
            let saved_sp_word0 = event
                .saved_sp_word0
                .map_or_else(|| "unmapped".to_string(), |word| format!("{word:#010x}"));
            println!(
                "  instruction={} PC={:#010x} kind={} SR={:#06x} current_TCB={} entry={} saved_SP={} saved_SP_word0={}",
                event.instruction_count,
                event.pc,
                scheduler_event_kind(event.pc),
                event.sr,
                current_tcb,
                task_entry,
                saved_sp,
                saved_sp_word0
            );
            println!("    D={:08x?} A={:08x?}", event.d, event.a);
            print_scheduler_words("TCB", event.current_tcb, &event.tcb_words);
            print_scheduler_words("live stack", Some(event.a[7]), &event.live_stack_words);
            print_scheduler_words("saved stack", event.tcb_saved_sp, &event.saved_stack_words);
        }
    }

    fn print_conversion_trace(&self) {
        if !self.conversion_trace_enabled {
            return;
        }
        println!(
            "conversion trace: {} event(s); D1/output base and outer CMP.L [A7+0x44],D1/BGT outcomes",
            self.conversion_trace_events.len()
        );
        for event in &self.conversion_trace_events {
            let branch = event.branch_taken.map_or_else(
                || "n/a".to_string(),
                |taken| {
                    format!(
                        "taken={taken} target={:#010x}",
                        event.branch_target.unwrap_or(0)
                    )
                },
            );
            println!(
                "  instruction={} PC={:#010x} D1={:#010x} loop_index={:#010x?} output_base={:#010x} CMP.L D1={:#010x},[A7+0x44]={:#010x} CCR={:#04x} SR={:#06x} BGT {}",
                event.instruction_count,
                event.pc,
                event.d1,
                event.loop_index,
                event.output_base,
                event.compare_left,
                event.compare_right,
                event.sr & 0x1F,
                event.sr,
                branch
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
        println!(
            "DSPI PUSHR transfers: {} total, showing {}{}",
            self.dspi_push_count,
            self.dspi_push_events.len(),
            if self.dspi_push_count > self.dspi_push_events.len() {
                " (log limit reached)"
            } else {
                ""
            }
        );
        for event in &self.dspi_push_events {
            println!(
                "  DSPI{} instruction={} PC={:#010x} PUSHR={:#010x} PCS={:#04x} SR={:#010x} TXCTR={} RXCTR={}",
                event.module,
                event.instruction_count,
                event.pc,
                event.word,
                (event.word >> 16) & 0xFF,
                event.status_after,
                (event.status_after >> 12) & 0xF,
                (event.status_after >> 4) & 0xF
            );
        }
        println!(
            "DSPI POPR reads: {} total, showing {}{}",
            self.dspi_pop_count,
            self.dspi_pop_events.len(),
            if self.dspi_pop_count > self.dspi_pop_events.len() {
                " (log limit reached)"
            } else {
                ""
            }
        );
        for event in &self.dspi_pop_events {
            println!(
                "  DSPI{} instruction={} PC={:#010x} POPR{}={:#010x} SR={:#010x} RXCTR={} POPNXTPTR={}",
                event.module,
                event.instruction_count,
                event.pc,
                event.width_bits,
                event.value,
                event.status_after,
                (event.status_after >> 4) & 0xF,
                event.status_after & 0xF
            );
        }
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
        println!(
            "RGPIO trace: {} events, showing first {}",
            self.rgpio_access_count,
            self.rgpio_access_events.len()
        );
        for (pc, event) in &self.rgpio_access_events {
            let operation = if event.write { "write" } else { "read" };
            println!(
                "  {operation}{} {:#010x} {:#010x} at PC={pc:#010x}",
                event.width, event.address, event.value
            );
        }
        if self.rgpio_trace_limit > 0 {
            println!(
                "RGPIO full trace: {} distinct entries (repeats folded)",
                self.rgpio_trace.len()
            );
            for entry in &self.rgpio_trace {
                let operation = if entry.event.write { "write" } else { "read" };
                println!(
                    "  n={} PC={:#010x} A7={:#010x} {operation}{} {:#010x} = {:#06x} x{}",
                    entry.instruction,
                    entry.pc,
                    entry.sp,
                    entry.event.width,
                    entry.event.address,
                    entry.event.value,
                    entry.repeats
                );
            }
        }
        println!(
            "section3 MMIO: {} events, showing first {}",
            self.section3_mmio_count,
            self.section3_mmio_events.len()
        );
        for (instruction, pc, event) in &self.section3_mmio_events {
            let operation = if event.write { "write" } else { "read" };
            println!(
                "  {operation}{} instruction={instruction} PC={pc:#010x} address={:#010x} value={:#010x} [{}]",
                event.width,
                event.address,
                event.value,
                event.source.label()
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
        if self.gpio_loopback && address == GPIO_LOOPBACK_SENSE {
            return Ok(u8::from(self.gpio_loopback_level) << 3);
        }
        if let Some(rgpio) = self.rgpio_for(address) {
            return rgpio.read8(address);
        }
        if let Some(value) = self.mmio.get(&address) {
            return Ok(*value);
        }
        if let Some(value) = self.edma_tcd.get(&address) {
            return Ok(*value);
        }
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.read8(address);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.read8(address);
        }
        if let Some(esdhc) = self.esdhc_for(address) {
            let result = esdhc.read8(address);
            if let Ok(value) = result {
                self.record_esdhc_access(address, 8, false, u32::from(value));
            }
            return result;
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.read8(address);
        }
        if let Some(dtim1) = self.dtim1_for(address) {
            return dtim1.read8(address);
        }
        if let Some(dtim3) = self.dtim3_for(address) {
            return dtim3.read8(address);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.read8(address);
        }
        if let Some(intc0) = self.intc0_for(address) {
            return intc0.read8(address);
        }
        if let Some(intc1) = self.intc1_for(address) {
            return intc1.read8(address);
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
        if let Some(rgpio) = self.rgpio_for(address) {
            return rgpio.read16(address);
        }
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
        if let Some(esdhc) = self.esdhc_for(address) {
            let result = esdhc.read16(address);
            if let Ok(value) = result {
                self.record_esdhc_access(address, 16, false, u32::from(value));
            }
            return result;
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.read16(address);
        }
        if let Some(dtim1) = self.dtim1_for(address) {
            return dtim1.read16(address);
        }
        if let Some(dtim3) = self.dtim3_for(address) {
            return dtim3.read16(address);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.read16(address);
        }
        if let Some(intc0) = self.intc0_for(address) {
            return intc0.read16(address);
        }
        if let Some(intc1) = self.intc1_for(address) {
            return intc1.read16(address);
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
        if address == UART8_TCD34_DADDR && self.uart8_dma_enabled {
            return Ok(self.uart8_dma_daddr);
        }
        if let Some(rgpio) = self.rgpio_for(address) {
            return rgpio.read32(address);
        }
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
        if let Some(esdhc) = self.esdhc_for(address) {
            let result = esdhc.read32(address);
            if let Ok(value) = result {
                self.record_esdhc_access(address, 32, false, value);
            }
            return result;
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.read32(address);
        }
        if let Some(dtim1) = self.dtim1_for(address) {
            return dtim1.read32(address);
        }
        if let Some(dtim3) = self.dtim3_for(address) {
            return dtim3.read32(address);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.read32(address);
        }
        if let Some(intc0) = self.intc0_for(address) {
            return intc0.read32(address);
        }
        if let Some(intc1) = self.intc1_for(address) {
            return intc1.read32(address);
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
        if self.gpio_loopback {
            if address == GPIO_LOOPBACK_SET && value & 0x10 != 0 {
                self.gpio_loopback_level = true;
            } else if address == GPIO_LOOPBACK_CLEAR && value & 0x10 == 0 {
                self.gpio_loopback_level = false;
            }
        }
        if self.uart8.is_some() && EDMA_TCD_WINDOW.contains(&address) {
            self.edma_tcd.insert(address, value);
            let channel = ((address - EDMA_TCD_WINDOW.start) / 0x20) as u8;
            self.record_esdhc_dma_event(channel, "TCD_WRITE", address, u32::from(value), 0);
            return Ok(());
        }
        if address == EDMA_SERQ && self.edma_trace_enabled {
            self.trace_edma_request(value & 0x3F);
        }
        if address == EDMA_SERQ && self.dsp_link && matches!(value & 0x3F, 28 | 29) {
            self.run_dsp_link_channel(value & 0x3F)?;
            return Ok(());
        }
        if address == EDMA_SERQ && self.uart8.is_some() {
            let channel = value & 0x3F;
            self.record_esdhc_dma_event(channel, "SERQ_START", address, u32::from(channel), 0);
            if channel >= 32 && channel != UART8_TCD34_REQUEST_VALUE {
                self.run_edma_memory_to_peripheral(channel)?;
            }
        }
        if address == UART8_TCD34_CINT && value & 0x3F == UART8_TX_DMA_CHANNEL {
            self.uart8_tx_irq_pending = false;
        }
        if address == UART8_TCD34_CINT && value & 0x3F == SSI_AUDIO_DMA_CHANNEL {
            self.ssi_audio_irq_pending = false;
            // The next audio block completes one period later.
            self.ssi_audio_rearm_at = self
                .ssi_audio_period
                .map(|period| self.current_instruction_count.saturating_add(period));
        }
        if address == UART8_TCD34_CINT && value & 0x3F == ESDHC_DMA_CHANNEL {
            self.esdhc_dma_irq_pending = false;
        }
        if let Some(rgpio) = self.rgpio_for(address) {
            return rgpio.write8(address, value);
        }
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write8(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write8(address, value);
        }
        if self.esdhc_for(address).is_some() {
            let result = self.esdhc.as_mut().unwrap().write8(address, value);
            if result.is_ok() {
                self.record_esdhc_access(address, 8, true, u32::from(value));
            }
            self.kick_deferred_edma()?;
            return result;
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.write8(address, value);
        }
        if let Some(dtim1) = self.dtim1_for(address) {
            return dtim1.write8(address, value);
        }
        if let Some(dtim3) = self.dtim3_for(address) {
            return dtim3.write8(address, value);
        }
        if self
            .uart8
            .as_ref()
            .is_some_and(|_| Uart8::contains(address))
        {
            self.uart8.as_mut().unwrap().write8(address, value)?;
            if address == UART8_URB {
                if let Some(display) = &mut self.display {
                    display.feed(value);
                }
                if let Some((bytes, times)) = &mut self.uart8_tx_file {
                    use std::io::Write;
                    let _ = bytes.write_all(&[value]);
                    let _ = times.write_all(&(self.current_instruction_count as u64).to_le_bytes());
                }
                self.panel_stub_tx_byte(value)?;
            }
            return Ok(());
        }
        if address == UART8_TCD34_REQUEST
            && value == UART8_TCD34_REQUEST_VALUE
            && self.uart8.is_some()
        {
            self.enable_uart8_dma()?;
        }
        if address == UART8_TCD34_CINT && value == UART8_TCD34_REQUEST_VALUE {
            self.uart8_rx_irq_pending = false;
        }
        if let Some(intc2) = self.intc2_for(address) {
            return intc2.write8(address, value);
        }
        if let Some(intc0) = self.intc0_for(address) {
            return intc0.write8(address, value);
        }
        if let Some(intc1) = self.intc1_for(address) {
            return intc1.write8(address, value);
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
            self.code_map.note_write(address, 1);
            self.ram.write8(self.remapped_ram_address(address), value)
        }
    }

    fn write16_inner(&mut self, address: u32, value: u16) -> Result<(), BusError> {
        if let Some(rgpio) = self.rgpio_for(address) {
            return rgpio.write16(address, value);
        }
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write16(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write16(address, value);
        }
        if self.esdhc_for(address).is_some() {
            let result = self.esdhc.as_mut().unwrap().write16(address, value);
            if result.is_ok() {
                self.record_esdhc_access(address, 16, true, u32::from(value));
            }
            self.kick_deferred_edma()?;
            return result;
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.write16(address, value);
        }
        if let Some(dtim1) = self.dtim1_for(address) {
            return dtim1.write16(address, value);
        }
        if let Some(dtim3) = self.dtim3_for(address) {
            return dtim3.write16(address, value);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.write16(address, value);
        }
        if let Some(intc0) = self.intc0_for(address) {
            return intc0.write16(address, value);
        }
        if let Some(intc1) = self.intc1_for(address) {
            return intc1.write16(address, value);
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
        if address == UART8_TCD34_DADDR && self.uart8.is_some() {
            self.uart8_dma_daddr = value;
            self.uart8_dma_ring_base = value;
            self.uart8_dma_enabled = false;
            return Ok(());
        }
        if let Some(rgpio) = self.rgpio_for(address) {
            return rgpio.write32(address, value);
        }
        if let Some(dspi) = self.dspi_for(address) {
            return dspi.write32(address, value);
        }
        if let Some(ddr) = self.ddr_for(address) {
            return ddr.write32(address, value);
        }
        if self.esdhc_for(address).is_some() {
            let result = self.esdhc.as_mut().unwrap().write32(address, value);
            if result.is_ok() {
                self.record_esdhc_access(address, 32, true, value);
            }
            self.kick_deferred_edma()?;
            return result;
        }
        if let Some(pit) = self.pit_for(address) {
            return pit.write32(address, value);
        }
        if let Some(dtim1) = self.dtim1_for(address) {
            return dtim1.write32(address, value);
        }
        if let Some(dtim3) = self.dtim3_for(address) {
            return dtim3.write32(address, value);
        }
        if let Some(uart) = self.uart8_for(address) {
            return uart.write32(address, value);
        }
        if let Some(intc0) = self.intc0_for(address) {
            return intc0.write32(address, value);
        }
        if let Some(intc1) = self.intc1_for(address) {
            return intc1.write32(address, value);
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

    fn attach_dspi1_registers(&mut self) {
        self.dspi1 = Some(Dspi::new(DSPI1_BASE));
    }

    fn attach_dspi2_registers(&mut self) {
        self.dspi2 = Some(Dspi::new(DSPI2_BASE));
    }

    fn dspi_for(&mut self, address: u32) -> Option<&mut Dspi> {
        if self
            .dspi0
            .as_ref()
            .is_some_and(|dspi| dspi.contains(address))
        {
            return self.dspi0.as_mut();
        }
        if self
            .dspi1
            .as_ref()
            .is_some_and(|dspi| dspi.contains(address))
        {
            return self.dspi1.as_mut();
        }
        self.dspi2.as_mut().filter(|dspi| dspi.contains(address))
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

    fn configure_pit0_periodic(&mut self) -> Result<(), BusError> {
        self.write16(0xFC08_0000, 0x053F)?;
        self.write16(0xFC08_0002, 0xA121)?;
        self.write8(0xFC05_004D, 1)?;
        self.write8(0xFC05_001D, 13)
    }

    fn configure_pit2_periodic(&mut self) -> Result<(), BusError> {
        self.write16(0xFC08_8000, 0x053F)?;
        self.write16(0xFC08_8002, 0x8646)?;
        self.write8(0xFC05_004F, 3)?;
        self.write8(0xFC05_001D, 15)
    }

    fn attach_intc2(&mut self) {
        self.intc2 = Some(Intc2::new());
    }

    fn attach_dtim1_bootstrap(&mut self, ticks_per_instruction: u32) {
        self.dtim1 = Some(DmaTimer1::new(ticks_per_instruction));
        self.intc0 = Some(Intc0::new());
    }

    // DTIM3 (INTC0 source 35, vector 99) drives the firmware's UI tick; it shares INTC0 with DTIM1.
    fn attach_dtim3_bootstrap(&mut self, ticks_per_instruction: u32) {
        self.dtim3 = Some(DmaTimer1::at(DTIM3_BASE, ticks_per_instruction));
        self.intc0.get_or_insert_with(Intc0::new);
    }

    fn attach_uart8_tx(&mut self) {
        self.uart8 = Some(Uart8::new());
    }

    fn queue_uart8_rx(&mut self, bytes: &[u8]) {
        if let Some(uart) = &mut self.uart8 {
            uart.queue_receive(bytes);
        }
    }

    fn enable_panel_stub(&mut self) {
        self.panel_stub = true;
        self.enable_uart8_tx_ready();
    }

    fn enable_uart8_tx_ready(&mut self) {
        if let Some(uart) = &mut self.uart8 {
            uart.enable_transmitter();
        }
    }

    fn record_uart8_event(
        &mut self,
        address: u32,
        value: u8,
        write: bool,
        dma: bool,
        dma_destination: u32,
    ) {
        if self.uart8_trace_events.len() >= MAX_UART8_TRACE_EVENTS {
            self.suppressed_uart8_trace_events =
                self.suppressed_uart8_trace_events.saturating_add(1);
            return;
        }
        self.uart8_trace_events.push(Uart8TraceEvent {
            instruction_count: self.current_instruction_count,
            pc: self.current_pc,
            address,
            value,
            write,
            dma,
            dma_destination,
        });
    }

    fn panel_stub_tx_byte(&mut self, value: u8) -> Result<(), BusError> {
        if !self.panel_stub {
            return Ok(());
        }
        self.panel_stub_tx_tail.push_back(value);
        if self.panel_stub_tx_tail.len() > PANEL_STUB_QUERY.len() {
            self.panel_stub_tx_tail.pop_front();
        }
        if !self.panel_stub_discovery_queued
            && self
                .panel_stub_tx_tail
                .iter()
                .rev()
                .take(PANEL_STUB_DISCOVERY_QUERY.len())
                .copied()
                .eq(PANEL_STUB_DISCOVERY_QUERY.into_iter().rev())
        {
            self.panel_stub_discovery_queued = true;
            let ring = 0x8000_8424;
            for (offset, byte) in panel_stub_discovery_records(self.panel_stub_func_held)
                .iter()
                .copied()
                .enumerate()
            {
                self.write8_inner(ring + offset as u32, byte)?;
            }
            self.write32_inner(
                0x8000_8410,
                panel_stub_discovery_records(self.panel_stub_func_held).len() as u32,
            )?;
        }
        if !self.panel_stub_handshake_queued
            && self.panel_stub_tx_tail.iter().copied().eq(PANEL_STUB_QUERY)
        {
            self.panel_stub_handshake_queued = true;
            if let Some(uart) = &mut self.uart8 {
                uart.queue_receive(panel_stub_handshake_reply());
            }
        }
        Ok(())
    }

    fn kick_deferred_edma(&mut self) -> Result<(), BusError> {
        if self.edma_deferred.is_empty() || !self.esdhc.as_ref().is_some_and(Esdhc::data_ready) {
            return Ok(());
        }
        let channels: Vec<u8> = self.edma_deferred.iter().copied().collect();
        self.edma_deferred.clear();
        for channel in channels {
            self.run_edma_memory_to_peripheral(channel)?;
        }
        Ok(())
    }

    fn trace_edma_request(&self, channel: u8) {
        let base = 0xFC04_5000 + 0x20 * u32::from(channel);
        println!(
            "EDMA_TRACE request channel={channel} instruction={} PC={:#010x} SADDR={:#010x} ATTR={:#06x} SOFF={:#06x} NBYTES={:#010x} DADDR={:#010x} DOFF={:#06x} CITER={:#06x} TCD1C={:#06x} TCD1E={:#06x}",
            self.current_instruction_count,
            self.current_pc,
            self.tcd_value(base, 4),
            self.tcd_value(base + 0x04, 2),
            self.tcd_value(base + 0x06, 2),
            self.tcd_value(base + 0x08, 4),
            self.tcd_value(base + 0x10, 4),
            self.tcd_value(base + 0x16, 2),
            self.tcd_value(base + 0x14, 2),
            self.tcd_value(base + 0x1C, 2),
            self.tcd_value(base + 0x1E, 2),
        );
    }

    // Diagnostic DSP link: ch29 feeds DSPI2 PUSHR, ch28 drains POPR. Each block is traced and then
    // completes with EOQF set and the selected placeholder reply (no DSP model).
    fn run_dsp_link_channel(&mut self, channel: u8) -> Result<(), BusError> {
        let base = 0xFC04_5000 + 0x20 * u32::from(channel);
        let source = self.tcd_value(base, 4);
        let destination = self.tcd_value(base + 0x10, 4);
        let total = self.tcd_value(base + 0x14, 2) * self.tcd_value(base + 0x08, 4).max(1);
        if channel == 29 {
            let mut words = Vec::new();
            for offset in (0..total).step_by(2) {
                let address = source.wrapping_add(offset);
                let value = self.read16_inner(address)?;
                self.record_sample_access(
                    SampleAccessActor::Dma {
                        channel,
                        peer: destination,
                    },
                    address,
                    2,
                    false,
                    u32::from(value),
                );
                words.push(value);
            }
            let prefix: Vec<u8> = words
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .take(NONRAM_BULK_TRACE_PREFIX_BYTES)
                .collect();
            self.record_nonram_dma_transfer(
                channel,
                true,
                source,
                destination,
                u64::from(total),
                prefix,
            );
            if self.dsp_link_blocks < 4000 {
                let data: Vec<String> = words.iter().map(|w| format!("{w:04x}")).collect();
                println!(
                    "DSP_LINK block {} len={} {}",
                    self.dsp_link_blocks,
                    total,
                    data.join(" ")
                );
            }
            self.dsp_link_blocks += 1;
            if let Some(dspi) = self.dspi2.as_mut() {
                dspi.set_eoq_flag();
            }
        } else {
            for offset in (0..total).step_by(2) {
                let address = destination.wrapping_add(offset);
                self.write16_inner(address, self.dsp_link_reply)?;
                self.record_firmware_write(
                    FirmwareWriteActor::Dma(channel),
                    address,
                    16,
                    u32::from(self.dsp_link_reply),
                );
                self.record_sample_access(
                    SampleAccessActor::Dma {
                        channel,
                        peer: source,
                    },
                    address,
                    2,
                    true,
                    u32::from(self.dsp_link_reply),
                );
            }
        }
        Ok(())
    }

    // Diagnostic stand-in for the SHARC voice engine: 32 frames of 16 slots (64 bytes each) per block.
    fn synthesize_voice_block(&mut self, block: u32) -> Result<(), BusError> {
        let (Some((address, length)), Some(mut pos)) = (self.synth_voice, self.synth_pos) else {
            return Ok(());
        };
        for frame in 0..32u32 {
            if pos + 2 > length {
                self.synth_pos = None;
                println!("SYNTH_VOICE finished");
                return Ok(());
            }
            let hi = self.read8_inner(address + pos)?;
            let lo = self.read8_inner(address + pos + 1)?;
            let word = u32::from(u16::from_be_bytes([hi, lo])) << 16;
            self.write32_inner(block + frame * 64, word)?;
            self.write32_inner(block + frame * 64 + 4, word)?;
            pos += 2;
        }
        self.synth_pos = Some(pos);
        Ok(())
    }

    fn tcd_value(&self, address: u32, width: u32) -> u32 {
        (0..width).fold(0, |value, offset| {
            value << 8 | u32::from(self.edma_tcd.get(&(address + offset)).copied().unwrap_or(0))
        })
    }

    // Diagnostic eDMA model: a started channel completes immediately, honoring SSIZE/DSIZE and SOFF/DOFF.
    fn run_edma_memory_to_peripheral(&mut self, channel: u8) -> Result<(), BusError> {
        let base = 0xFC04_5000 + 0x20 * u32::from(channel);
        let source = self.tcd_value(base, 4);
        let attr = self.tcd_value(base + 0x04, 2);
        let source_offset = self.tcd_value(base + 0x06, 2) as u16 as i16 as i32;
        let minor_bytes = self.tcd_value(base + 0x08, 4).max(1);
        let destination = self.tcd_value(base + 0x10, 4);
        let iterations = self.tcd_value(base + 0x14, 2);
        let destination_offset = self.tcd_value(base + 0x16, 2) as u16 as i16 as i32;
        let reload = self.tcd_value(base + 0x1C, 2);
        let csr = self.tcd_value(base + 0x1E, 2) as u16;
        self.record_esdhc_dma_event(
            channel,
            "DMA_START",
            source,
            destination,
            (iterations << 16) | (minor_bytes & 0xFFFF),
        );
        if iterations == 0 {
            return Ok(());
        }
        if source == ESDHC_DATPORT && !self.esdhc.as_ref().is_some_and(Esdhc::data_ready) {
            self.edma_deferred.insert(channel);
            self.record_esdhc_dma_event(channel, "DMA_DEFERRED", source, destination, iterations);
            return Ok(());
        }
        let transfer_size = |code: u32| match code & 7 {
            0 => 1,
            1 => 2,
            2 => 4,
            4 => 16,
            5 => 32,
            _ => 1,
        };
        let (source_size, destination_size) = (transfer_size(attr >> 8), transfer_size(attr));
        let total_bytes = iterations.saturating_mul(minor_bytes);
        let capture_ssi_tx = channel == SSI_AUDIO_DMA_CHANNEL
            && destination == SSI_AUDIO_TX_DATA
            && self.ssi_tx_raw.is_some();
        if channel == SSI_AUDIO_DMA_CHANNEL && destination == SSI_AUDIO_TX_DATA {
            self.synthesize_voice_block(source)?;
        }
        if capture_ssi_tx {
            self.ssi_tx_dma_blocks = self.ssi_tx_dma_blocks.saturating_add(1);
            self.ssi_tx_first_source.get_or_insert(source);
        }
        let mut prefix = Vec::with_capacity(NONRAM_BULK_TRACE_PREFIX_BYTES);
        for index in 0..iterations * minor_bytes {
            let from = source
                .wrapping_add(((index / source_size) as i32).wrapping_mul(source_offset) as u32)
                .wrapping_add(index % source_size);
            let to = destination
                .wrapping_add(
                    ((index / destination_size) as i32).wrapping_mul(destination_offset) as u32,
                )
                .wrapping_add(index % destination_size);
            let byte = self.read8_inner(from)?;
            if prefix.len() < NONRAM_BULK_TRACE_PREFIX_BYTES {
                prefix.push(byte);
            }
            self.record_sample_access(
                SampleAccessActor::Dma {
                    channel,
                    peer: destination,
                },
                from,
                1,
                false,
                u32::from(byte),
            );
            self.write8_inner(to, byte)?;
            self.record_firmware_write(FirmwareWriteActor::Dma(channel), to, 8, u32::from(byte));
            self.record_sample_access(
                SampleAccessActor::Dma {
                    channel,
                    peer: source,
                },
                to,
                1,
                true,
                u32::from(byte),
            );
            if capture_ssi_tx {
                if let Some(capture) = &mut self.ssi_tx_raw {
                    capture.push(byte);
                }
            }
        }
        self.record_nonram_dma_transfer(
            channel,
            channel == SSI_AUDIO_DMA_CHANNEL,
            source,
            destination,
            u64::from(total_bytes),
            prefix,
        );
        for (offset, byte) in (reload as u16).to_be_bytes().into_iter().enumerate() {
            self.edma_tcd.insert(base + 0x14 + offset as u32, byte);
        }
        for (offset, byte) in (csr | 0x80).to_be_bytes().into_iter().enumerate() {
            self.edma_tcd.insert(base + 0x1E + offset as u32, byte);
        }
        if channel == UART8_TX_DMA_CHANNEL && csr & 0x02 != 0 {
            self.uart8_tx_irq_pending = true;
        }
        if channel == ESDHC_DMA_CHANNEL && csr & 0x02 != 0 {
            self.esdhc_dma_irq_pending = true;
        }
        if self.ssi_audio_period.is_some() && channel == SSI_AUDIO_DMA_CHANNEL {
            // Major-loop completion: SADDR/DADDR advance, then scatter/gather loads the next TCD.
            let slast = self.tcd_value(base + 0x0C, 4);
            let dlast = self.tcd_value(base + 0x18, 4);
            let total = iterations * minor_bytes;
            let advance = |start: u32, size: u32, offset: i32| {
                start.wrapping_add(((total / size) as i32).wrapping_mul(offset) as u32)
            };
            let saddr = advance(source, source_size, source_offset).wrapping_add(slast);
            let daddr = advance(destination, destination_size, destination_offset);
            for (offset, byte) in saddr.to_be_bytes().into_iter().enumerate() {
                self.edma_tcd.insert(base + offset as u32, byte);
            }
            for (offset, byte) in daddr.to_be_bytes().into_iter().enumerate() {
                self.edma_tcd.insert(base + 0x10 + offset as u32, byte);
            }
            if csr & 0x10 != 0 {
                for offset in 0..0x20u32 {
                    let byte = self.read8_inner(dlast.wrapping_add(offset))?;
                    self.edma_tcd.insert(base + offset, byte);
                }
            }
            // The vector-170 handler looks for a 0x007fffff frame marker at word 0 of the RX half-buffer
            // (64-byte spacing); emulate the codec side that sends it.
            for half in [SSI_AUDIO_RX_BUFFER, SSI_AUDIO_RX_BUFFER + 0x800] {
                for slot in 0..32u32 {
                    let address = half + slot * 64;
                    self.write32_inner(address, 0x007F_FFFF)?;
                    self.record_firmware_write(
                        FirmwareWriteActor::Dma(channel),
                        address,
                        32,
                        0x007F_FFFF,
                    );
                }
            }
            if csr & 0x02 != 0 {
                self.ssi_audio_irq_pending = true;
            }
        }
        self.record_esdhc_dma_event(
            channel,
            "DMA_DONE",
            source,
            destination,
            iterations.saturating_mul(minor_bytes),
        );
        Ok(())
    }

    // Route late panel input through section 2's polled ring until section-3 DMA is active.
    fn deliver_uart8_rx_now(&mut self, bytes: &[u8]) -> Result<(), BusError> {
        if self.panel_stub && !self.uart8_dma_enabled {
            const POLLED_RING: u32 = 0x8000_8424;
            const POLLED_CURSOR: u32 = 0x8000_840C;
            const POLLED_COUNT: u32 = 0x8000_8410;
            const POLLED_CAPACITY: u32 = 0x100;

            let cursor = self.read32_inner(POLLED_CURSOR)?;
            let count = self.read32_inner(POLLED_COUNT)?;
            let byte_count = u32::try_from(bytes.len()).map_err(|_| BusError {
                addr: POLLED_RING,
                write: true,
            })?;
            if cursor > POLLED_CAPACITY
                || count > POLLED_CAPACITY - cursor
                || byte_count > POLLED_CAPACITY - cursor - count
            {
                return Err(BusError {
                    addr: POLLED_RING + cursor.min(POLLED_CAPACITY),
                    write: true,
                });
            }
            let end = count + byte_count;
            for (offset, value) in bytes.iter().copied().enumerate() {
                self.write8_inner(POLLED_RING + cursor + count + offset as u32, value)?;
            }
            self.write32_inner(POLLED_COUNT, end)?;
            return Ok(());
        }
        if let Some(uart) = &mut self.uart8 {
            uart.queue_receive(bytes);
        }
        let bytes = self
            .uart8
            .as_mut()
            .map(Uart8::take_received_for_dma)
            .unwrap_or_default();
        for value in bytes.iter().copied() {
            let destination = self.uart8_dma_daddr;
            self.write8_inner(destination, value)?;
            self.record_firmware_write(
                FirmwareWriteActor::Dma(UART8_TCD34_REQUEST_VALUE),
                destination,
                8,
                u32::from(value),
            );
            self.record_uart8_event(UART8_URB, value, false, true, destination);
            self.uart8_dma_daddr = self.uart8_dma_ring_base.wrapping_add(
                destination
                    .wrapping_sub(self.uart8_dma_ring_base)
                    .wrapping_add(1)
                    & UART8_DMA_RING_MASK,
            );
        }
        if !bytes.is_empty() {
            self.uart8_rx_irq_pending = true;
        }
        Ok(())
    }

    fn enable_uart8_dma(&mut self) -> Result<(), BusError> {
        if self.uart8_dma_enabled {
            return Ok(());
        }
        self.uart8_dma_enabled = true;
        self.uart8_dma_ring_base = self.uart8_dma_daddr;
        if self.panel_stub
            && self
                .uart8
                .as_ref()
                .is_some_and(|uart| uart.received_remaining() == 0)
        {
            if let Some(uart) = &mut self.uart8 {
                uart.queue_receive(if self.panel_stub_handshake_queued {
                    panel_stub_input_records()
                } else {
                    panel_stub_discovery_records(self.panel_stub_func_held)
                });
            }
        }
        let bytes = self
            .uart8
            .as_mut()
            .map(Uart8::take_received_for_dma)
            .unwrap_or_default();
        for value in bytes.iter().copied() {
            let destination = self.uart8_dma_daddr;
            self.write8_inner(destination, value)?;
            self.record_firmware_write(
                FirmwareWriteActor::Dma(UART8_TCD34_REQUEST_VALUE),
                destination,
                8,
                u32::from(value),
            );
            self.record_uart8_event(UART8_URB, value, false, true, destination);
            self.uart8_dma_daddr = self.uart8_dma_ring_base.wrapping_add(
                destination
                    .wrapping_sub(self.uart8_dma_ring_base)
                    .wrapping_add(1)
                    & UART8_DMA_RING_MASK,
            );
        }
        if !bytes.is_empty() {
            self.uart8_rx_irq_pending = true;
        }
        Ok(())
    }

    fn print_esdhc_trace(&self) {
        if !self.esdhc_trace_enabled {
            return;
        }
        let Some(esdhc) = &self.esdhc else {
            return;
        };
        let (completed_writes, last_written_block, write_active, write_buffer_bytes) =
            esdhc.write_trace_state();
        println!(
            "eSDHC trace: {} command(s), {} completed storage block write(s), write_active={} buffered_bytes={}",
            self.esdhc_command_events.len(),
            completed_writes,
            write_active,
            write_buffer_bytes
        );
        let mut command_summary = BTreeMap::<u8, (usize, usize, usize, u64)>::new();
        for event in &self.esdhc_command_events {
            let summary = command_summary.entry(event.command).or_insert((
                0,
                event.instruction_count,
                event.instruction_count,
                0,
            ));
            summary.0 += 1;
            summary.2 = event.instruction_count;
            let requested_blocks = (event.block_attributes >> 16).max(1);
            if matches!(event.command, 17 | 18 | 24 | 25) {
                let blocks = if matches!(event.command, 17 | 24) {
                    1
                } else {
                    requested_blocks
                };
                summary.3 = summary.3.saturating_add(u64::from(blocks));
                println!(
                    "  CMD={:#04x} instruction={}..{} PC={:#010x} argument_block={:#010x} block_range={:#010x}..{:#010x} blocks={} block_size={} completed_writes={} active={} buffered_bytes={}",
                    event.command,
                    event.instruction_count,
                    event.instruction_count,
                    event.pc,
                    event.argument,
                    event.argument,
                    event.argument.wrapping_add(blocks),
                    blocks,
                    event.block_attributes & 0xFFFF,
                    event.completed_writes,
                    event.write_active,
                    event.write_buffer_bytes
                );
            } else {
                println!(
                    "  CMD={:#04x} instruction={} PC={:#010x} argument={:#010x} block_attributes={:#010x} completed_writes={} active={} buffered_bytes={}",
                    event.command,
                    event.instruction_count,
                    event.pc,
                    event.argument,
                    event.block_attributes,
                    event.completed_writes,
                    event.write_active,
                    event.write_buffer_bytes
                );
            }
        }
        println!("eSDHC command summary by type:");
        for (command, (count, first, last, blocks)) in command_summary {
            println!(
                "  CMD={command:#04x} count={count} instruction_range={first}..{last} addressed_blocks={blocks}"
            );
        }
        for event in &self.esdhc_write_events {
            println!(
                "  WRITE_BLOCK instruction={} PC={:#010x} blocks={:#010x}..{:#010x} count={}",
                event.instruction_count,
                event.pc,
                event.first_block,
                event.first_block.wrapping_add(event.block_count as u32),
                event.block_count
            );
        }
        println!("eSDHC register access summary:");
        for (offset, summary) in &self.esdhc_access_summaries {
            let name = match *offset {
                0x04 => "BLKATTR",
                0x08 => "CMDARG",
                0x0C => "XFERTYP",
                0x10 => "CMDRSP0",
                0x20 => "DATPORT",
                0x24 => "PRSSTAT",
                0x2C => "SYSCTL",
                0x30 => "IRQSTAT",
                0x38 => "IRQSIGEN",
                _ => "other",
            };
            println!(
                "  {name} address={:#010x} reads={} writes={} last_value={:#010x}",
                ESDHC_BASE + offset,
                summary.reads,
                summary.writes,
                summary.last_value
            );
        }
        let tcd = 0xFC04_5000 + 0x20 * u32::from(ESDHC_DMA_CHANNEL);
        println!(
            "eSDHC eDMA TCD channel={ESDHC_DMA_CHANNEL} SADDR={:#010x} ATTR={:#06x} SOFF={:#06x} NBYTES={:#010x} DADDR={:#010x} DOFF={:#06x} CITER={:#06x} BITER={:#06x} CSR={:#06x}",
            self.tcd_value(tcd, 4),
            self.tcd_value(tcd + 0x04, 2),
            self.tcd_value(tcd + 0x06, 2),
            self.tcd_value(tcd + 0x08, 4),
            self.tcd_value(tcd + 0x10, 4),
            self.tcd_value(tcd + 0x16, 2),
            self.tcd_value(tcd + 0x14, 2),
            self.tcd_value(tcd + 0x1C, 2),
            self.tcd_value(tcd + 0x1E, 2)
        );
        println!("eSDHC eDMA channel events: {}", self.esdhc_dma_events.len());
        for event in &self.esdhc_dma_events {
            println!(
                "  channel={} {} instruction={} PC={:#010x} address={:#010x} value={:#010x} detail={:#010x}",
                event.channel,
                event.operation,
                event.instruction_count,
                event.pc,
                event.address,
                event.value,
                event.detail
            );
        }
        if let Some(event) = self.esdhc_last_access {
            println!(
                "eSDHC last access: instruction={} PC={:#010x} {}{} address={:#010x} value={:#010x} last_written_block={:?}",
                event.instruction_count,
                event.pc,
                if event.write { "write" } else { "read" },
                event.width,
                event.address,
                event.value,
                last_written_block
            );
        }
    }

    fn print_uart8_trace(&self) {
        println!(
            "UART8 byte events: {} total, showing {}{}",
            self.uart8_trace_events.len() + self.suppressed_uart8_trace_events,
            self.uart8_trace_events.len(),
            if self.suppressed_uart8_trace_events > 0 {
                " (log limit reached)"
            } else {
                ""
            }
        );
        for event in &self.uart8_trace_events {
            if event.dma {
                println!(
                    "  UART8 RX DMA instruction={} PC={:#010x} URB={:#04x} ring={:#010x}",
                    event.instruction_count, event.pc, event.value, event.dma_destination
                );
            } else {
                println!(
                    "  UART8 {} instruction={} PC={:#010x} address={:#010x} value={:#04x}",
                    if event.write { "write" } else { "read" },
                    event.instruction_count,
                    event.pc,
                    event.address,
                    event.value
                );
            }
        }
    }

    fn ddr_for(&mut self, address: u32) -> Option<&mut DdrController> {
        self.ddr
            .as_mut()
            .filter(|_| DdrController::contains(address))
    }

    fn dtim3_for(&mut self, address: u32) -> Option<&mut DmaTimer1> {
        self.dtim3.as_mut().filter(|timer| timer.covers(address))
    }

    fn dtim1_for(&mut self, address: u32) -> Option<&mut DmaTimer1> {
        self.dtim1.as_mut().filter(|_| DmaTimer1::contains(address))
    }

    fn intc0_for(&mut self, address: u32) -> Option<&mut Intc0> {
        self.intc0.as_mut().filter(|_| Intc0::contains(address))
    }

    fn intc1_for(&mut self, address: u32) -> Option<&mut Intc0> {
        self.intc1.as_mut().filter(|intc| intc.covers(address))
    }

    fn esdhc_for(&mut self, address: u32) -> Option<&mut Esdhc> {
        self.esdhc.as_mut().filter(|_| Esdhc::contains(address))
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
        self.dspi0
            .as_ref()
            .and_then(Dspi::unsupported)
            .or_else(|| self.dspi1.as_ref().and_then(Dspi::unsupported))
    }

    fn snapshot_ranges(&self, ram_base: u32, ram_size: u32) -> Vec<(u32, u32)> {
        let mut ranges = vec![(ram_base, ram_size)];
        if self.sram_overlay.is_some() {
            ranges.push((0x8000_0000, 0x1_0000));
        }
        ranges
    }

    fn save_snapshot(
        &mut self,
        cpu: &Cpu,
        ram_base: u32,
        ram_size: u32,
    ) -> Result<Vec<u8>, String> {
        self.flush_timers();
        if let Some(pit) = self.pit.as_mut() {
            pit.sync();
        }
        if let Some(dtim1) = self.dtim1.as_mut() {
            dtim1.sync();
        }
        if let Some(dtim3) = self.dtim3.as_mut() {
            dtim3.sync();
        }
        use dt2_core::snapshot::Writer;
        const PAGE: u32 = 4096;
        let mut w = Writer::new();
        w.bytes(b"DT2SNAP1");
        for v in cpu.d.iter().chain(cpu.a.iter()) {
            w.u32(*v);
        }
        for v in [
            cpu.pc,
            cpu.rambar,
            cpu.acr0,
            cpu.acr1,
            cpu.vbr,
            cpu.cacr,
            cpu.rgpiobar,
            cpu.debug_csr,
        ] {
            w.u32(v);
        }
        w.u16(cpu.sr);
        w.u64(self.current_instruction_count as u64);
        w.u32(self.rgpiobar);
        w.u64(self.mmio.len() as u64);
        for (address, byte) in &self.mmio {
            w.u32(*address);
            w.u8(*byte);
        }
        for flag in [
            self.panel_stub_discovery_queued,
            self.panel_stub_handshake_queued,
            self.uart8_dma_enabled,
            self.uart8_rx_irq_pending,
            self.uart8_tx_irq_pending,
            self.pit0_request_seen,
        ] {
            w.bool(flag);
        }
        w.u64(self.edma_tcd.len() as u64);
        for (address, byte) in &self.edma_tcd {
            w.u32(*address);
            w.u8(*byte);
        }
        w.u32(self.uart8_dma_daddr);
        w.u32(self.uart8_dma_ring_base);
        let tail: Vec<u8> = self.panel_stub_tx_tail.iter().copied().collect();
        w.bytes(&tail);
        macro_rules! device {
            ($field:ident) => {
                w.bool(self.$field.is_some());
                if let Some(device) = &self.$field {
                    device.save_state(&mut w);
                }
            };
        }
        device!(dspi0);
        device!(dspi1);
        device!(dspi2);
        device!(ddr);
        device!(pit);
        device!(dtim1);
        device!(intc0);
        device!(intc2);
        device!(rgpio);
        device!(uart8);
        let ranges = self.snapshot_ranges(ram_base, ram_size);
        w.u32(ranges.len() as u32);
        for (base, size) in ranges {
            w.u32(base);
            w.u32(size);
            let pages = size / PAGE;
            let mut present = Vec::new();
            let mut data = Vec::new();
            for page in 0..pages {
                let bytes = self
                    .dump_ram(base + page * PAGE, PAGE as usize)
                    .map_err(|e| format!("snapshot RAM read failed at {:#010x}", e.addr))?;
                if bytes.iter().any(|b| *b != 0) {
                    present.push(page);
                    data.extend_from_slice(&bytes);
                }
            }
            w.u64(present.len() as u64);
            for page in present {
                w.u32(page);
            }
            w.bytes(&data);
        }
        w.bool(self.esdhc.is_some());
        if let Some(esdhc) = &self.esdhc {
            esdhc.save_state(&mut w);
        }
        w.bool(self.esdhc_dma_irq_pending);
        w.u32(self.edma_deferred.len() as u32);
        for channel in &self.edma_deferred {
            w.u8(*channel);
        }
        // Optional tail written only when a display, INTC1 or EMAC state exists, so older configurations keep their hash.
        let emac_state = !cpu.emac.is_default();
        let dtim3_state = self.dtim3.is_some();
        let led_state = self.display.as_ref().is_some_and(|d| d.has_led_state());
        if self.display.is_some() || self.intc1.is_some() || emac_state || dtim3_state {
            w.bool(self.display.is_some());
            if let Some(display) = &self.display {
                display.save_state(&mut w);
            }
            if self.intc1.is_some() || emac_state || dtim3_state {
                w.bool(self.intc1.is_some());
                if let Some(intc1) = &self.intc1 {
                    intc1.save_state(&mut w);
                }
                if emac_state || dtim3_state || led_state {
                    w.bool(emac_state);
                    if emac_state {
                        cpu.emac.save_state(&mut w);
                    }
                    if let Some(dtim3) = &self.dtim3 {
                        w.bool(true);
                        dtim3.save_state(&mut w);
                    } else if led_state {
                        w.bool(false);
                    }
                    if let (true, Some(display)) = (led_state, &self.display) {
                        w.bool(true);
                        display.save_led_state(&mut w);
                    }
                }
            }
        }
        Ok(w.into_inner())
    }

    fn load_snapshot(
        &mut self,
        cpu: &mut Cpu,
        data: &[u8],
        ram_base: u32,
        ram_size: u32,
    ) -> Result<(), String> {
        use dt2_core::snapshot::Reader;
        const PAGE: u32 = 4096;
        let mut r = Reader::new(data);
        if r.bytes()? != b"DT2SNAP1" {
            return Err("not a DT2 snapshot".into());
        }
        for v in cpu.d.iter_mut().chain(cpu.a.iter_mut()) {
            *v = r.u32()?;
        }
        cpu.pc = r.u32()?;
        cpu.rambar = r.u32()?;
        cpu.acr0 = r.u32()?;
        cpu.acr1 = r.u32()?;
        cpu.vbr = r.u32()?;
        cpu.cacr = r.u32()?;
        cpu.rgpiobar = r.u32()?;
        cpu.debug_csr = r.u32()?;
        cpu.sr = r.u16()?;
        self.current_instruction_count = r.u64()? as usize;
        self.rgpiobar = r.u32()?;
        let n = r.u64()?;
        self.mmio.clear();
        for _ in 0..n {
            let address = r.u32()?;
            self.mmio.insert(address, r.u8()?);
        }
        self.panel_stub_discovery_queued = r.bool()?;
        self.panel_stub_handshake_queued = r.bool()?;
        self.uart8_dma_enabled = r.bool()?;
        self.uart8_rx_irq_pending = r.bool()?;
        self.uart8_tx_irq_pending = r.bool()?;
        self.pit0_request_seen = r.bool()?;
        let n = r.u64()?;
        self.edma_tcd.clear();
        for _ in 0..n {
            let address = r.u32()?;
            self.edma_tcd.insert(address, r.u8()?);
        }
        self.uart8_dma_daddr = r.u32()?;
        self.uart8_dma_ring_base = r.u32()?;
        self.panel_stub_tx_tail = r.bytes()?.iter().copied().collect();
        macro_rules! device {
            ($field:ident) => {
                let present = r.bool()?;
                match (present, self.$field.as_mut()) {
                    (true, Some(device)) => device.load_state(&mut r)?,
                    (false, None) => {}
                    _ => {
                        return Err(format!(
                            "snapshot/config mismatch for {}: re-run with the same device flags",
                            stringify!($field)
                        ))
                    }
                }
            };
        }
        device!(dspi0);
        device!(dspi1);
        device!(dspi2);
        device!(ddr);
        device!(pit);
        device!(dtim1);
        device!(intc0);
        device!(intc2);
        // RGPIO is created lazily when the firmware writes RGPIOBAR.
        let rgpio_present = r.bool()?;
        if rgpio_present {
            self.rgpio
                .get_or_insert_with(Rgpio::new)
                .load_state(&mut r)?;
        } else {
            self.rgpio = None;
        }
        device!(uart8);
        let ranges = self.snapshot_ranges(ram_base, ram_size);
        if r.u32()? as usize != ranges.len() {
            return Err("snapshot RAM range count mismatch".into());
        }
        for (base, size) in ranges {
            if r.u32()? != base || r.u32()? != size {
                return Err("snapshot RAM range mismatch".into());
            }
            let count = r.u64()?;
            let present: Vec<u32> = (0..count).map(|_| r.u32()).collect::<Result<_, _>>()?;
            let bytes = r.bytes()?;
            let zero = vec![0u8; PAGE as usize];
            let mut next = present.iter().copied().enumerate().peekable();
            for page in 0..size / PAGE {
                let address = base + page * PAGE;
                let source = match next.peek() {
                    Some((index, p)) if *p == page => {
                        let start = index * PAGE as usize;
                        let chunk = &bytes[start..start + PAGE as usize];
                        next.next();
                        chunk
                    }
                    _ => {
                        let current = self
                            .dump_ram(address, PAGE as usize)
                            .map_err(|e| format!("snapshot RAM read failed at {:#010x}", e.addr))?;
                        if current.iter().all(|b| *b == 0) {
                            continue;
                        }
                        &zero
                    }
                };
                self.load_ram(address, source)?;
            }
        }
        let intc1_expected = self.intc1.is_some();
        let mut intc1_loaded = false;
        // Optional tail: absent in snapshots written before the eSDHC model existed.
        if r.remaining() > 0 {
            let present = r.bool()?;
            match (present, self.esdhc.as_mut()) {
                (true, Some(esdhc)) => esdhc.load_state(&mut r)?,
                (false, None) => {}
                _ => {
                    return Err(
                        "snapshot/config mismatch for esdhc: re-run with the same device flags"
                            .into(),
                    )
                }
            }
            self.esdhc_dma_irq_pending = r.bool()?;
            self.edma_deferred = (0..r.u32()?).map(|_| r.u8()).collect::<Result<_, _>>()?;
        }
        // Optional tail: the panel framebuffer, present only when saved with a display attached.
        if r.remaining() > 0 {
            if r.bool()? {
                self.display
                    .get_or_insert_with(display::Display::new)
                    .load_state(&mut r)?;
            }
            if r.remaining() > 0 {
                if r.bool()? {
                    self.intc1
                        .get_or_insert_with(|| Intc0::at(0xFC04_C000, 128))
                        .load_state(&mut r)?;
                    intc1_loaded = true;
                }
                if r.remaining() > 0 && r.bool()? {
                    cpu.emac.load_state(&mut r)?;
                }
                if r.remaining() > 0 && r.bool()? {
                    self.dtim3
                        .get_or_insert_with(|| DmaTimer1::at(DTIM3_BASE, 1))
                        .load_state(&mut r)?;
                }
                if r.remaining() > 0 && r.bool()? {
                    self.display
                        .get_or_insert_with(display::Display::new)
                        .load_led_state(&mut r)?;
                }
            }
        }
        if intc1_expected && !intc1_loaded {
            return Err("snapshot/config mismatch for intc1: the snapshot was saved without INTC1_BOOTSTRAP; its INTC1 interrupt levels are lost, so re-save it with the same flags".into());
        }
        Ok(())
    }

    // Logs the first three stack arguments at a function entry and saves the buffer one of them points to.
    fn dump_call_at(&mut self, pc: u32, sp: u32) {
        for index in 0..self.dump_calls.len() {
            if self.dump_calls[index].pc != pc {
                continue;
            }
            let spec = self.dump_calls[index].clone();
            let args: Vec<u32> = (1..=3)
                .map(|slot| self.peek_ram_u32(sp.wrapping_add(4 * slot)).unwrap_or(0))
                .collect();
            let number = self.dump_call_count;
            self.dump_call_count += 1;
            let path = format!("{}-{number:03}.bin", spec.prefix);
            let saved = self
                .dump_ram(args[spec.buffer_arg], spec.length)
                .ok()
                .map(|bytes| std::fs::write(&path, bytes).is_ok())
                .unwrap_or(false);
            println!(
                "call dump #{number}: instruction={} PC={pc:#010x} args=[{:#010x}, {:#010x}, {:#010x}] buffer={:#010x} saved={saved} {path}",
                self.current_instruction_count, args[0], args[1], args[2], args[spec.buffer_arg]
            );
        }
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
            self.code_map.note_write(address, 4);
            self.ram.write32(self.remapped_ram_address(address), value)
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

    fn peek_ram_u32(&mut self, address: u32) -> Option<u32> {
        let bytes = self.dump_ram(address, 4).ok()?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?))
    }

    fn peek_ram_words(&mut self, address: u32, count: usize) -> Vec<u32> {
        (0..count)
            .filter_map(|index| self.peek_ram_u32(address.wrapping_add((index * 4) as u32)))
            .collect()
    }

    fn record_scheduler_task_creation(&mut self, stack_pointer: u32) {
        let Some(tcb) = self.peek_ram_u32(stack_pointer.wrapping_add(4)) else {
            return;
        };
        let Some(entry) = self.peek_ram_u32(stack_pointer.wrapping_add(8)) else {
            return;
        };
        let Some(priority) = self.peek_ram_u32(stack_pointer.wrapping_add(12)) else {
            return;
        };
        let Some(stack_base) = self.peek_ram_u32(stack_pointer.wrapping_add(16)) else {
            return;
        };
        let Some(stack_size) = self.peek_ram_u32(stack_pointer.wrapping_add(20)) else {
            return;
        };
        self.scheduler_task_entries.insert(tcb, entry);
        self.scheduler_task_creations.push(SchedulerTaskCreation {
            instruction_count: self.current_instruction_count,
            tcb,
            entry,
            priority,
            stack_base,
            stack_size,
        });
    }

    fn record_scheduler_transition(&mut self, pc: u32, d: &[u32; 8], a: &[u32; 8], sr: u16) {
        if !matches!(pc, 0x4000_13EC | 0x4000_145E | 0x4000_14BA | 0x4000_1344) {
            return;
        }
        let current_tcb = self.peek_ram_u32(0x47DB_3F64);
        let task_entry = current_tcb.and_then(|tcb| self.scheduler_task_entries.get(&tcb).copied());
        let tcb_words = current_tcb.map_or_else(Vec::new, |tcb| self.peek_ram_words(tcb, 22));
        let tcb_saved_sp = current_tcb.and_then(|tcb| self.peek_ram_u32(tcb.wrapping_add(0x48)));
        let saved_sp_word0 = tcb_saved_sp.and_then(|sp| self.peek_ram_u32(sp));
        let live_stack_words = self.peek_ram_words(a[7], 8);
        let saved_stack_words = tcb_saved_sp.map_or_else(Vec::new, |sp| self.peek_ram_words(sp, 8));
        if self.scheduler_trace_events.len() == MAX_SCHED_TRACE_EVENTS {
            self.scheduler_trace_events.pop_front();
        }
        self.scheduler_trace_events.push_back(SchedulerTraceEvent {
            instruction_count: self.current_instruction_count,
            pc,
            d: *d,
            a: *a,
            sr,
            current_tcb,
            tcb_saved_sp,
            task_entry,
            saved_sp_word0,
            tcb_words,
            live_stack_words,
            saved_stack_words,
        });
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
        self.current_instruction_count = self.current_instruction_count.saturating_add(1);
        if let Some((period, messages, next, cursor)) = self.uart8_rx_periodic.as_mut() {
            if *next == 0 {
                *next = self.current_instruction_count + *period;
            } else if self.current_instruction_count >= *next {
                let message = messages[*cursor % messages.len()].clone();
                *cursor += 1;
                *next += *period;
                let _ = self.deliver_uart8_rx_now(&message);
                self.irq_dirty = true;
            }
        }
        if self.uart8_rx_once
            && self
                .uart8_rx_periodic
                .as_ref()
                .is_some_and(|(_, m, _, cursor)| *cursor >= m.len())
        {
            self.uart8_rx_periodic = None;
        }
    }

    fn stop_before_instruction(&mut self, pc: u32) -> bool {
        self.stop_pc == Some(pc)
    }

    fn set_rgpiobar(&mut self, value: u32) {
        self.irq_dirty = true;
        self.rgpiobar = value;
        self.rgpio.get_or_insert_with(Rgpio::new);
    }

    fn report_instruction_state(&mut self, pc: u32, d: &[u32; 8], a: &[u32; 8], sr: u16) {
        if !self.dump_calls.is_empty() {
            self.dump_call_at(pc, a[7]);
        }
        self.current_sr = sr;
        if self.capture_registers {
            self.current_d = *d;
            self.current_a = *a;
        }
        if self.synth_voice.is_some() && pc == VOICE_RECORD_BUILDER && self.synth_pos.is_none() {
            self.synth_pos = Some(0);
            println!(
                "SYNTH_VOICE start at instruction {}",
                self.current_instruction_count
            );
        }
        if !self.instruction_hooks {
            return;
        }
        let instruction_count = self.current_instruction_count;
        if self.instruction_trace_tail_limit > 0 {
            if self.instruction_trace_tail.len() == self.instruction_trace_tail_limit {
                self.instruction_trace_tail.pop_front();
            }
            self.instruction_trace_tail.push_back(RegisterTrace {
                instruction_count,
                pc,
                d: *d,
                a: *a,
                sr,
            });
        }
        if self.scheduler_trace_enabled {
            if pc == 0x4000_12C8 {
                self.record_scheduler_task_creation(a[7]);
            }
            self.record_scheduler_transition(pc, d, a, sr);
        }
        if self.conversion_trace_enabled && pc == 0x4018_1218 {
            self.record_conversion_trace(ConversionTraceEvent {
                instruction_count: 0,
                pc,
                d1: d[1],
                loop_index: None,
                output_base: a[3],
                compare_left: 0,
                compare_right: 0,
                sr,
                branch_taken: None,
                branch_target: None,
            });
        }
        let trace_pc_matches = self
            .trace_pc_range
            .map_or(true, |(start, end)| (start..end).contains(&pc));
        if trace_pc_matches
            && self.first_a0_range_trace.is_none()
            && self
                .trace_a0_range
                .is_some_and(|(start, end)| (start..end).contains(&a[0]))
        {
            self.first_a0_range_trace = Some(RegisterTrace {
                instruction_count,
                pc,
                d: *d,
                a: *a,
                sr,
            });
        }
        while trace_pc_matches && self.trace_a1_next_index < self.trace_a1_checkpoints.len() {
            let (threshold, trace) = &mut self.trace_a1_checkpoints[self.trace_a1_next_index];
            let reached = (0x4000_0400..0x4031_ff60).contains(&a[1])
                && a[1].saturating_sub(0x4000_0400) >= *threshold;
            if !reached {
                break;
            }
            *trace = Some(RegisterTrace {
                instruction_count,
                pc,
                d: *d,
                a: *a,
                sr,
            });
            self.trace_a1_next_index += 1;
        }
        if self.trace_register_pc != Some(pc) {
            return;
        }
        self.register_trace_count = self.register_trace_count.saturating_add(1);
        if self.register_trace.len() == MAX_REGISTER_TRACE {
            self.register_trace.pop_front();
            self.register_trace_stack.pop_front();
        }
        let stack = self.peek_ram_words(a[7], REGISTER_TRACE_STACK_WORDS);
        self.register_trace_stack.push_back(stack);
        self.register_trace.push_back(RegisterTrace {
            instruction_count,
            pc,
            d: *d,
            a: *a,
            sr,
        });
    }

    fn code_epoch(&self) -> u32 {
        self.code_map.epoch()
    }

    fn cacheable_code(&mut self, pc: u32, bytes: u32) -> bool {
        self.code_map.mark(pc, bytes)
    }

    fn begin_instruction_fetch(&mut self) {
        self.instruction_fetch = true;
    }

    fn end_instruction_fetch(&mut self) {
        self.instruction_fetch = false;
    }

    fn report_tst(&mut self, pc: u32, operand: TstOperand, value: u32, size_bytes: u8, sr: u16) {
        self.last_tst = Some(TstDiagnostic {
            pc,
            operand,
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
        if self.conversion_trace_enabled && pc == 0x4018_11D2 {
            let loop_index = self.current_a[7]
                .checked_add(0x44)
                .and_then(|address| self.peek_ram_u32(address));
            self.record_conversion_trace(ConversionTraceEvent {
                instruction_count: 0,
                pc,
                d1: self.current_d[1],
                loop_index,
                output_base: 0,
                compare_left: self.current_d[1],
                compare_right: loop_index.unwrap_or(0),
                sr,
                branch_taken: Some(taken),
                branch_target: Some(target),
            });
        }
    }

    fn report_rte(
        &mut self,
        pc: u32,
        frame_sp: u32,
        format_vector: u16,
        restored_sr: u16,
        restored_pc: u32,
    ) {
        if self.irq_log.is_some() {
            let line = format!(
                "RTE n={} pc={pc:#010x} restored_pc={restored_pc:#010x} sr={restored_sr:#06x}\n",
                self.current_instruction_count
            );
            self.write_irq_log(&line);
        }
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

    fn interrupt_taken(&mut self, level: u8, vector: u8) {
        self.irq_dirty = true;
        self.flush_timers();
        if self.irq_log.is_none() {
            return;
        }
        let mut timers = String::new();
        if let Some(pit) = self.pit.as_mut() {
            pit.sync();
            for module in 0..4 {
                if let Some((pcsr, _, counter)) = pit.timer_state(module) {
                    timers.push_str(&format!(" pit{module}={pcsr:#06x}:{counter:#06x}"));
                }
            }
        }
        let line = format!(
            "IRQ n={} level={level} vector={vector}{timers}\n",
            self.current_instruction_count
        );
        self.write_irq_log(&line);
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
        self.flush_timers();
        let Some(pit) = self.pit.as_mut() else {
            return 0;
        };
        pit.sync();
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
        if self.in_fast_ram(address, 1) {
            return self.ram.read8(address);
        }
        if self.in_fast_sram(address, 1) {
            return self.sram_overlay.as_mut().unwrap().read8(address);
        }
        self.irq_dirty = true;
        self.flush_timers();
        let source = self.source_for_access(address, 1);
        match self.read8_inner(address) {
            Ok(value) => {
                self.record_sample_access(
                    SampleAccessActor::Cpu,
                    address,
                    1,
                    false,
                    u32::from(value),
                );
                self.record_watched_read(address, 1, u32::from(value));
                if Uart8::contains(address) {
                    self.record_uart8_event(address, value, false, false, 0);
                }
                self.record_dspi_pop(address, 8, u32::from(value));
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
        if self.in_fast_ram(address, 2) {
            return self.ram.read16(address);
        }
        if self.in_fast_sram(address, 2) {
            return self.sram_overlay.as_mut().unwrap().read16(address);
        }
        self.irq_dirty = true;
        self.flush_timers();
        let source = self.source_for_access(address, 2);
        match self.read16_inner(address) {
            Ok(value) => {
                self.record_sample_access(
                    SampleAccessActor::Cpu,
                    address,
                    2,
                    false,
                    u32::from(value),
                );
                self.record_watched_read(address, 2, u32::from(value));
                self.record_dspi_pop(address, 16, u32::from(value));
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
        if self.in_fast_ram(address, 4) {
            return self.ram.read32(address);
        }
        if self.in_fast_sram(address, 4) {
            return self.sram_overlay.as_mut().unwrap().read32(address);
        }
        self.irq_dirty = true;
        self.flush_timers();
        let source = self.source_for_access(address, 4);
        match self.read32_inner(address) {
            Ok(value) => {
                self.record_sample_access(SampleAccessActor::Cpu, address, 4, false, value);
                self.record_watched_read(address, 4, value);
                self.record_dspi_pop(address, 32, value);
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
        if self.in_fast_ram(address, 1) {
            self.code_map.note_write(address, 1);
            self.ram.write8(address, value)?;
            self.record_firmware_write(FirmwareWriteActor::Cpu, address, 8, u32::from(value));
            self.record_nonram_cpu_write(address, 1, u32::from(value));
            return Ok(());
        }
        if self.in_fast_sram(address, 1) {
            self.sram_overlay.as_mut().unwrap().write8(address, value)?;
            self.record_firmware_write(FirmwareWriteActor::Cpu, address, 8, u32::from(value));
            self.record_nonram_cpu_write(address, 1, u32::from(value));
            return Ok(());
        }
        self.irq_dirty = true;
        self.flush_timers();
        let source = self.source_for_access(address, 1);
        let old = self.watched_old_bytes(address, 1)?;
        self.write8_inner(address, value)?;
        self.record_firmware_write(FirmwareWriteActor::Cpu, address, 8, u32::from(value));
        self.record_nonram_cpu_write(address, 1, u32::from(value));
        self.record_sample_access(SampleAccessActor::Cpu, address, 1, true, u32::from(value));
        if Uart8::contains(address) {
            self.record_uart8_event(address, value, true, false, 0);
        }
        self.snapshot_source_on_write(address, 1)?;
        self.record_watched_write(address, 1, u32::from(value), old);
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
        if self.in_fast_ram(address, 2) {
            self.code_map.note_write(address, 2);
            self.ram.write16(address, value)?;
            self.record_firmware_write(FirmwareWriteActor::Cpu, address, 16, u32::from(value));
            self.record_nonram_cpu_write(address, 2, u32::from(value));
            return Ok(());
        }
        if self.in_fast_sram(address, 2) {
            self.sram_overlay
                .as_mut()
                .unwrap()
                .write16(address, value)?;
            self.record_firmware_write(FirmwareWriteActor::Cpu, address, 16, u32::from(value));
            self.record_nonram_cpu_write(address, 2, u32::from(value));
            return Ok(());
        }
        self.irq_dirty = true;
        self.flush_timers();
        let source = self.source_for_access(address, 2);
        let old = self.watched_old_bytes(address, 2)?;
        self.write16_inner(address, value)?;
        self.record_firmware_write(FirmwareWriteActor::Cpu, address, 16, u32::from(value));
        self.record_nonram_cpu_write(address, 2, u32::from(value));
        self.record_sample_access(SampleAccessActor::Cpu, address, 2, true, u32::from(value));
        self.snapshot_source_on_write(address, 2)?;
        self.record_watched_write(address, 2, u32::from(value), old);
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
        if self.in_fast_ram(address, 4) {
            self.code_map.note_write(address, 4);
            self.ram.write32(address, value)?;
            self.record_firmware_write(FirmwareWriteActor::Cpu, address, 32, value);
            self.record_nonram_cpu_write(address, 4, value);
            return Ok(());
        }
        if self.in_fast_sram(address, 4) {
            self.sram_overlay
                .as_mut()
                .unwrap()
                .write32(address, value)?;
            self.record_firmware_write(FirmwareWriteActor::Cpu, address, 32, value);
            self.record_nonram_cpu_write(address, 4, value);
            return Ok(());
        }
        self.irq_dirty = true;
        self.flush_timers();
        let source = self.source_for_access(address, 4);
        let old = self.watched_old_bytes(address, 4)?;
        self.write32_inner(address, value)?;
        self.record_firmware_write(FirmwareWriteActor::Cpu, address, 32, value);
        self.record_nonram_cpu_write(address, 4, value);
        self.record_sample_access(SampleAccessActor::Cpu, address, 4, true, value);
        self.record_dspi_push(address, value);
        self.snapshot_source_on_write(address, 4)?;
        self.record_watched_write(address, 4, value, old);
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
        if let Some(period) = self.ssi_audio_period {
            // After a snapshot restore the channel was started earlier: arm on the first poll.
            if self.ssi_audio_rearm_at.is_none()
                && !self.ssi_audio_irq_pending
                && self.tcd_value(
                    0xFC04_5000 + 0x20 * u32::from(SSI_AUDIO_DMA_CHANNEL) + 0x14,
                    2,
                ) != 0
            {
                self.ssi_audio_rearm_at =
                    Some(self.current_instruction_count.saturating_add(period));
            }
        }
        if self
            .ssi_audio_rearm_at
            .is_some_and(|at| self.current_instruction_count >= at)
        {
            self.ssi_audio_rearm_at = None;
            let _ = self.run_edma_memory_to_peripheral(SSI_AUDIO_DMA_CHANNEL);
        }
        self.flush_timers();
        self.irq_dirty = false;
        self.poll_countdown = self.instructions_until_poll_event();
        if self.uart8_rx_irq_pending {
            return Some(InterruptRequest {
                source: 26,
                level: 3,
                vector: 154,
            });
        }
        if self.uart8_tx_irq_pending {
            return Some(InterruptRequest {
                source: 27,
                level: 3,
                vector: 155,
            });
        }
        if self.esdhc.as_ref().is_some_and(Esdhc::irq_pending) {
            return Some(InterruptRequest {
                source: 31,
                level: 3,
                vector: 223,
            });
        }
        if self.esdhc_dma_irq_pending {
            return Some(InterruptRequest {
                source: 0,
                level: 3,
                vector: 192,
            });
        }
        let pending_sources = self
            .pit
            .as_ref()
            .map_or(0, PitController::pending_source_bits);
        let timer1_pending = self
            .dtim1
            .as_ref()
            .is_some_and(DmaTimer1::interrupt_pending);
        let timer3_pending = self
            .dtim3
            .as_ref()
            .is_some_and(DmaTimer1::interrupt_pending);
        // Nothing asserted and nothing forced: skip the per-source INTC scans.
        if pending_sources == 0
            && !timer1_pending
            && !timer3_pending
            && !self.ssi_audio_irq_pending
            && !self.intc0.as_ref().is_some_and(Intc0::has_forced)
            && !self.intc1.as_ref().is_some_and(Intc0::has_forced)
            && !self.intc2.as_ref().is_some_and(Intc2::has_forced)
        {
            return None;
        }
        let intc0_request = self.intc0.as_ref().and_then(|intc0| {
            intc0.pending_interrupt_with(
                (u64::from(timer1_pending) << DTIM1_SOURCE)
                    | (u64::from(timer3_pending) << DTIM3_SOURCE),
            )
        });
        let intc2_request = self
            .intc2
            .as_ref()
            .and_then(|intc2| intc2.pending_interrupt(pending_sources));
        let intc1_request = self.intc1.as_ref().and_then(|intc1| {
            intc1.pending_interrupt_with(if self.ssi_audio_irq_pending {
                1u64 << SSI_AUDIO_INTC1_SOURCE
            } else {
                0
            })
        });
        // Highest level wins; ties go to INTC0, then INTC1, then INTC2 (an approximation).
        let request = [intc0_request, intc1_request, intc2_request]
            .into_iter()
            .flatten()
            .reduce(|best, next| if next.level > best.level { next } else { best });
        let request = request?;
        if request.source == 13 && self.pit0_oneshot {
            if self.pit0_request_seen {
                return None;
            }
            self.pit0_request_seen = true;
        }
        Some(request)
    }

    fn idle_skip(&mut self, max: usize) -> usize {
        // Executing idle spins for real gives the instruction-exact baseline the Unicorn tier
        // (which cannot fast-forward) is compared against.
        static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *DISABLED.get_or_init(|| std::env::var_os("DT2_NO_IDLE_SKIP").is_some()) {
            return 0;
        }
        if self.instruction_hooks || !self.dump_calls.is_empty() || self.loop_detection {
            return 0;
        }
        self.flush_timers();
        let count = self.current_instruction_count;
        let mut cap = max;
        if let Some((_, _, next, _)) = self.uart8_rx_periodic.as_ref() {
            if *next == 0 {
                return 0;
            }
            cap = cap.min(next.saturating_sub(count).saturating_sub(1));
        }
        if let Some(at) = self.ssi_audio_rearm_at {
            cap = cap.min(at.saturating_sub(count).saturating_sub(1));
        }
        let mut headroom = cap as u64;
        if let Some(pit) = &self.pit {
            headroom = headroom.min(pit.idle_headroom());
        }
        if let Some(dtim1) = &self.dtim1 {
            headroom = headroom.min(dtim1.idle_headroom());
        }
        if let Some(dtim3) = &self.dtim3 {
            headroom = headroom.min(dtim3.idle_headroom());
        }
        if headroom == 0 {
            return 0;
        }
        if let Some(pit) = &mut self.pit {
            pit.skip_idle(headroom);
        }
        if let Some(dtim1) = &mut self.dtim1 {
            dtim1.skip_idle(headroom);
        }
        if let Some(dtim3) = &mut self.dtim3 {
            dtim3.skip_idle(headroom);
        }
        self.current_instruction_count = count.saturating_add(headroom as usize);
        self.poll_countdown = self.poll_countdown.saturating_sub(headroom);
        headroom as usize
    }

    fn quiet_horizon(&self) -> usize {
        if !self.lazy_poll
            || self.instruction_hooks
            || self.capture_registers
            || self.loop_detection
            || self.stop_pc.is_some()
            || self.irq_dirty
        {
            return 0;
        }
        match self.uart8_rx_periodic.as_ref() {
            Some((_, _, 0, _)) => 0,
            Some((_, _, next, _)) => next.saturating_sub(self.current_instruction_count + 1),
            None => usize::MAX,
        }
    }

    fn quiet_step(&mut self, pc: u32, sr: u16) {
        self.current_pc = pc;
        self.current_instruction_count += 1;
        self.current_sr = sr;
    }

    fn quiet_retire(&mut self) {
        self.poll_countdown = self.poll_countdown.saturating_sub(1);
        self.timer_pending += 1;
    }

    fn poll_due(&self) -> bool {
        !self.lazy_poll || self.irq_dirty || self.poll_countdown == 0
    }

    fn end_run(&mut self) {
        self.flush_timers();
    }

    fn host_memory(&mut self) -> Vec<(u32, *mut u8, usize)> {
        let mut regions = Vec::new();
        if self.fast_ram.is_some() {
            regions.extend(self.ram.host_memory());
        }
        if self.sram_fast {
            if let Some(sram) = self.sram_overlay.as_mut() {
                regions.extend(sram.host_memory());
            }
        }
        regions
    }

    fn track_host_writes(&mut self) {
        self.ram.track_host_writes();
        if let Some(sram) = self.sram_overlay.as_mut() {
            sram.track_host_writes();
        }
    }

    fn take_host_writes(&mut self, out: &mut Vec<(u32, u32)>) {
        self.ram.take_host_writes(out);
        if let Some(sram) = self.sram_overlay.as_mut() {
            sram.take_host_writes(out);
        }
    }

    fn retire_batch(&mut self, n: usize) {
        self.retire_external(n, n);
    }

    fn advance_instruction(&mut self) {
        self.poll_countdown = self.poll_countdown.saturating_sub(1);
        if self.lazy_poll {
            self.timer_pending += 1;
            return;
        }
        if let Some(pit) = &mut self.pit {
            pit.advance_instruction();
        }
        if let Some(dtim1) = &mut self.dtim1 {
            dtim1.advance_instruction();
        }
        if let Some(dtim3) = &mut self.dtim3 {
            dtim3.advance_instruction();
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
    #[test]
    fn ssi_tx_raw_capture_records_channel_50_source_bytes() {
        let mut bus = ProbeBus::new(FlatRam::new(0x1000, 0x100));
        bus.ssi_tx_raw = Some(Vec::new());
        bus.synthetic_mcf_mmio = true;
        bus.write32(0x1000, 0x1234_5678).unwrap();

        let tcd = 0xFC04_5000 + 0x20 * u32::from(SSI_AUDIO_DMA_CHANNEL);
        for (offset, value, width) in [
            (0x00, 0x1000, 4),
            (0x04, 0x0202, 2),
            (0x06, 4, 2),
            (0x08, 4, 4),
            (0x10, SSI_AUDIO_TX_DATA, 4),
            (0x14, 1, 2),
        ] {
            let bytes = value.to_be_bytes();
            for index in 0..width {
                bus.edma_tcd
                    .insert(tcd + offset + index, bytes[(4 - width + index) as usize]);
            }
        }

        bus.run_edma_memory_to_peripheral(SSI_AUDIO_DMA_CHANNEL)
            .unwrap();

        assert_eq!(bus.ssi_tx_dma_blocks, 1);
        assert_eq!(bus.ssi_tx_first_source, Some(0x1000));
        assert_eq!(
            bus.ssi_tx_raw.as_deref(),
            Some(&[0x12, 0x34, 0x56, 0x78][..])
        );
    }

    #[test]
    fn panel_codes_map_to_channel_and_bit() {
        assert_eq!(super::panel_wire_for_code(1), Some((0, 0)));
        assert_eq!(super::panel_wire_for_code(25), Some((3, 0)));
        assert_eq!(super::panel_wire_for_code(48), Some((5, 7)));
        assert_eq!(super::panel_wire_for_code(49), Some((6, 1)));
        assert_eq!(super::panel_wire_for_code(50), Some((6, 0)));
        assert_eq!(super::panel_wire_for_code(0), None);
        assert_eq!(super::panel_wire_for_code(51), None);
    }

    #[test]
    fn panel_input_sends_whole_masks_and_defers_a_same_slice_release() {
        let mut input = super::PanelInput::default();
        input.begin_slice();
        input.press(25);
        input.press(26);
        input.release(25);
        assert_eq!(input.take(), vec![[0x23, 0x01], [0x23, 0x03]]);
        input.begin_slice();
        assert_eq!(input.take(), vec![[0x23, 0x02]]);
        input.begin_slice();
        input.release(26);
        assert_eq!(input.take(), vec![[0x23, 0x00]]);
        input.turn(3, -1);
        input.turn(9, 1);
        assert_eq!(input.take(), vec![[0x33, 0xFF]]);
    }

    #[test]
    fn panel_input_caps_turns_and_backlog() {
        let mut input = super::PanelInput::default();
        for _ in 0..50 {
            input.turn(4, 127);
        }
        assert_eq!(input.take(), vec![[0x34, 30]]);
        assert_eq!(input.take(), vec![[0x34, 30]]);
        assert!(input.take().is_empty());
        input.turn(4, -5);
        assert_eq!(input.take(), vec![[0x34, 0xFB]]);
    }

    use super::*;
    use dt2_core::uart::UART8_BASE;

    #[test]
    fn sample_access_trace_orders_cpu_and_dma_accesses() {
        let mut bus = ProbeBus::new(FlatRam::new(0x10000, 0x100));
        bus.sample_access_trace_range = Some((0x10000, 0x10020));
        bus.ram.write16(0x10000, 0xA1B2).unwrap();
        bus.begin_instruction(0x2000);
        assert_eq!(bus.read8(0x10000).unwrap(), 0xA1);

        let tcd = 0xFC04_5000 + 0x20 * 2;
        for (offset, value, width) in [
            (0x00_u32, 0x10000_u32, 4_u32),
            (0x04, 0, 2),
            (0x06, 1, 2),
            (0x08, 2, 4),
            (0x10, 0x10010, 4),
            (0x14, 1, 2),
            (0x16, 1, 2),
        ] {
            let bytes = value.to_be_bytes();
            for index in 0..width {
                bus.edma_tcd
                    .insert(tcd + offset + index, bytes[(4 - width + index) as usize]);
            }
        }
        bus.begin_instruction(0x2010);
        bus.run_edma_memory_to_peripheral(2).unwrap();

        assert_eq!(bus.sample_access_trace.len(), 5);
        assert!(matches!(
            bus.sample_access_trace[0].actor,
            SampleAccessActor::Cpu
        ));
        assert_eq!(bus.sample_access_trace[0].pc, 0x2000);
        assert_eq!(bus.sample_access_trace[0].address, 0x10000);
        for (index, address, write, value) in [
            (1, 0x10000, false, 0xA1),
            (2, 0x10010, true, 0xA1),
            (3, 0x10001, false, 0xB2),
            (4, 0x10011, true, 0xB2),
        ] {
            let event = &bus.sample_access_trace[index];
            assert!(matches!(
                event.actor,
                SampleAccessActor::Dma {
                    channel: 2,
                    peer: 0x10000 | 0x10010
                }
            ));
            assert_eq!(event.pc, 0x2010);
            assert_eq!(event.address, address);
            assert_eq!(event.write, write);
            assert_eq!(event.value, value);
        }
    }

    #[test]
    fn firmware_write_trace_captures_only_the_selected_instruction_window() {
        let mut bus = ProbeBus::new(FlatRam::new(0x1000, 0x100));
        bus.write_trace_range = Some((3, 5));
        bus.write_trace_base = 100;
        bus.current_pc = 0x2000;

        bus.current_instruction_count = 103;
        bus.write16(0x1010, 0x1234).unwrap();
        bus.current_instruction_count = 105;
        bus.write8(0x1012, 0x56).unwrap();

        assert_eq!(bus.firmware_write_trace.len(), 1);
        let write = &bus.firmware_write_trace[0];
        assert_eq!(write.instruction_count, 103);
        assert_eq!(write.pc, 0x2000);
        assert!(matches!(write.actor, FirmwareWriteActor::Cpu));
        assert_eq!(write.address, 0x1010);
        assert_eq!(write.width_bits, 16);
        assert_eq!(write.value, 0x1234);
    }

    #[test]
    fn dsp_link_reply_value_is_written_to_the_modeled_rx_buffer() {
        let mut bus = ProbeBus::new(FlatRam::new(0x8000_1000, 0x100));
        bus.dsp_link_reply = 0;
        let tcd = 0xFC04_5000 + 0x20 * 28;
        for (offset, value, width) in [
            (0x08_u32, 2_u32, 4_u32),
            (0x10, 0x8000_1000, 4),
            (0x14, 1, 2),
        ] {
            let bytes = value.to_be_bytes();
            for index in 0..width {
                bus.edma_tcd
                    .insert(tcd + offset + index, bytes[(4 - width + index) as usize]);
            }
        }

        bus.run_dsp_link_channel(28).unwrap();

        assert_eq!(bus.ram.read16(0x8000_1000).unwrap(), 0);
    }

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
        set_register(&mut cpu, "RAMBAR=0x80000235").unwrap();
        set_register(&mut cpu, "ACR0=0x4007e020").unwrap();
        set_register(&mut cpu, "VBR=0x40000000").unwrap();
        set_register(&mut cpu, "CACR=0xa50ce100").unwrap();
        assert_eq!(cpu.d[3], 0x1234_5678);
        assert_eq!(cpu.a[7], 0x8001_FFFC);
        assert_eq!(cpu.sr, 0x2000);
        assert_eq!(cpu.rambar, 0x8000_0235);
        assert_eq!(cpu.acr0, 0x4007_E020);
        assert_eq!(cpu.vbr, 0x4000_0000);
        assert_eq!(cpu.cacr, 0xA50C_E100);
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
    fn rgpio_valid_bit_routes_the_fixed_window_before_sram() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.attach_mcf_sram_overlay();

        bus.write16(0x8C00_0002, 0x1234).unwrap();
        bus.set_rgpiobar(dt2_core::coldfire::RGPIOBAR_RESET | 1);
        assert_eq!(bus.read16(0x8C00_0002).unwrap(), 0);
        bus.write16(0x8C00_0002, 0x0001).unwrap();
        assert_eq!(bus.read16(0x8C00_0002).unwrap(), 1);

        bus.set_rgpiobar(dt2_core::coldfire::RGPIOBAR_RESET);
        assert_eq!(bus.read16(0x8C00_0002).unwrap(), 0x1234);
    }

    #[test]
    fn rgpio_user_mask_disables_window_only_in_user_mode() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.rgpio = Some(Rgpio::new());
        bus.set_rgpiobar(dt2_core::coldfire::RGPIOBAR_RESET | 3);

        bus.current_sr = 0;
        assert_eq!(bus.source_for_byte(0x8C00_0000), AccessSource::Ram);
        bus.current_sr = 0x2000;
        assert_eq!(bus.source_for_byte(0x8C00_0000), AccessSource::Modeled);
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
    fn uart8_tx_ready_option_does_not_consume_queued_rx_bytes() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.enable_synthetic_mcf_mmio();
        bus.attach_uart8_tx();
        bus.queue_uart8_rx(&[0xAA, 0x55]);

        bus.enable_uart8_tx_ready();

        assert_eq!(bus.read8(UART8_BASE + 4).unwrap(), 0x0D);
        assert_eq!(bus.uart8.as_ref().unwrap().received_remaining(), 2);
        assert_eq!(bus.read8(UART8_URB).unwrap(), 0xAA);
        assert_eq!(bus.read8(UART8_URB).unwrap(), 0x55);
    }

    #[test]
    fn uart8_rx_hex_spec_requires_complete_byte_pairs() {
        assert_eq!(parse_uart_rx_bytes("700b00").unwrap(), [0x70, 0x0B, 0x00]);
        assert!(parse_uart_rx_bytes("").is_err());
        assert!(parse_uart_rx_bytes("700").is_err());
        assert!(parse_uart_rx_bytes("70xz").is_err());
    }

    #[test]
    fn panel_stub_replies_then_dma_delivers_input_and_requests_vector_154() {
        let ring = 0x4FE3_A000;
        let mut bus = ProbeBus::new(FlatRam::new(ring, 0x400));
        bus.enable_synthetic_mcf_mmio();
        bus.attach_uart8_tx();
        bus.enable_panel_stub();
        bus.begin_instruction(0x4000_1000);
        assert_eq!(bus.read8(UART8_BASE + 4).unwrap() & 0x04, 0x04);

        for byte in PANEL_STUB_QUERY {
            bus.begin_instruction(0x4000_1010);
            bus.write8(UART8_URB, byte).unwrap();
        }
        assert!(bus.panel_stub_handshake_queued);
        for expected in panel_stub_handshake_reply() {
            bus.begin_instruction(0x4000_1020);
            assert_eq!(bus.read8(UART8_URB).unwrap(), *expected);
        }

        bus.begin_instruction(0x4000_1030);
        bus.write32(UART8_TCD34_DADDR, ring).unwrap();
        bus.begin_instruction(0x4000_1040);
        bus.write8(UART8_TCD34_REQUEST, UART8_TCD34_REQUEST_VALUE)
            .unwrap();

        assert_eq!(
            bus.dump_ram(ring, panel_stub_input_records().len())
                .unwrap(),
            panel_stub_input_records()
        );
        assert_eq!(
            bus.pending_interrupt(),
            Some(InterruptRequest {
                source: 26,
                level: 3,
                vector: 154,
            })
        );
        assert_eq!(
            bus.uart8_trace_events
                .iter()
                .filter(|event| event.dma)
                .count(),
            panel_stub_input_records().len()
        );
        assert!(bus
            .uart8_trace_events
            .iter()
            .all(|event| event.instruction_count > 0));

        bus.write8(UART8_TCD34_CINT, UART8_TCD34_REQUEST_VALUE)
            .unwrap();
        assert_eq!(bus.pending_interrupt(), None);
    }

    #[test]
    fn panel_stub_dma_delivers_all_section2_discovery_channels() {
        let ring = 0x4FE3_A000;
        let mut bus = ProbeBus::new(FlatRam::new(ring, 0x400));
        bus.enable_synthetic_mcf_mmio();
        bus.attach_uart8_tx();
        bus.enable_panel_stub();
        bus.begin_instruction(0x4000_2000);
        bus.write32(UART8_TCD34_DADDR, ring).unwrap();

        bus.begin_instruction(0x4000_2010);
        bus.write8(UART8_TCD34_REQUEST, UART8_TCD34_REQUEST_VALUE)
            .unwrap();

        assert_eq!(
            bus.dump_ram(ring, panel_stub_discovery_records(false).len())
                .unwrap(),
            panel_stub_discovery_records(false)
        );
        assert_eq!(
            bus.pending_interrupt(),
            Some(InterruptRequest {
                source: 26,
                level: 3,
                vector: 154,
            })
        );
        assert!(!bus.panel_stub_handshake_queued);
    }

    #[test]
    fn panel_stub_answers_section2_discovery_query_in_polled_ring() {
        let ring = 0x8000_8424;
        let mut bus = ProbeBus::new(FlatRam::new(0x8000_0000, 0x1_0000));
        bus.attach_uart8_tx();
        bus.enable_panel_stub();

        for byte in PANEL_STUB_DISCOVERY_QUERY {
            bus.write8(UART8_URB, byte).unwrap();
        }

        assert_eq!(
            bus.dump_ram(ring, panel_stub_discovery_records(false).len())
                .unwrap(),
            panel_stub_discovery_records(false)
        );
        assert_eq!(bus.read32(0x8000_8410).unwrap(), 14);
        assert!(bus.panel_stub_discovery_queued);
        assert!(!bus.panel_stub_handshake_queued);
    }

    #[test]
    fn panel_stub_can_report_func_held_during_discovery() {
        assert_eq!(&panel_stub_discovery_records(true)[4..6], &[0x22, 0x01]);
        assert_eq!(&panel_stub_discovery_records(false)[4..6], &[0x22, 0x00]);
    }

    #[test]
    fn panel_stub_late_input_queues_in_section2_polled_ring_before_dma() {
        let ring = 0x8000_8424;
        let mut bus = ProbeBus::new(FlatRam::new(0x8000_0000, 0x1_0000));
        bus.attach_uart8_tx();
        bus.enable_panel_stub();
        bus.write32(0x8000_840C, 14).unwrap();

        bus.deliver_uart8_rx_now(&[0x23, 0x04]).unwrap();

        assert_eq!(bus.read32(0x8000_8410).unwrap(), 2);
        assert_eq!(bus.dump_ram(ring, 16).unwrap()[14..], [0x23, 0x04]);
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
        bus.write16(0xFC08_0000, 0x001B).unwrap();
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
    fn dtim1_reference_match_delivers_intc0_vector_97_when_unmasked() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 0x3000));
        bus.attach_dtim1_bootstrap(1);
        bus.write8(dt2_core::intc0::INTC0_BASE + 0x61, 2).unwrap();
        bus.write8(dt2_core::intc0::INTC0_BASE + 0x1D, 33).unwrap();
        bus.write32(dt2_core::dtim1::DTIM1_BASE + 0x04, 2).unwrap();
        bus.write16(dt2_core::dtim1::DTIM1_BASE, 0x841B).unwrap();

        for _ in 0..(2 * 133 - 1) {
            bus.advance_instruction();
        }
        assert_eq!(bus.pending_interrupt(), None);

        bus.advance_instruction();
        assert_eq!(
            bus.pending_interrupt(),
            Some(InterruptRequest {
                source: 33,
                level: 2,
                vector: 97,
            })
        );

        bus.write8(dt2_core::dtim1::DTIM1_BASE + 0x03, 0x02)
            .unwrap();
        assert_eq!(bus.pending_interrupt(), None);
    }

    #[test]
    fn dtim3_reference_match_delivers_intc0_vector_99_with_firmware_setup() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 0x3000));
        bus.attach_dtim3_bootstrap(1);
        // Same register writes as the firmware's timer start at 0x400c0b84..0x400c0ba6.
        bus.write8(dt2_core::intc0::INTC0_BASE + 0x63, 2).unwrap();
        bus.write8(dt2_core::intc0::INTC0_BASE + 0x1D, 35).unwrap();
        bus.write32(DTIM3_BASE + 0x0C, 0).unwrap();
        bus.write32(DTIM3_BASE + 0x04, 3).unwrap();
        bus.write16(DTIM3_BASE, 0x1D).unwrap();

        for _ in 0..(3 * 16 - 1) {
            bus.advance_instruction();
        }
        assert_eq!(bus.pending_interrupt(), None);

        bus.advance_instruction();
        assert_eq!(
            bus.pending_interrupt(),
            Some(InterruptRequest {
                source: 35,
                level: 2,
                vector: 99,
            })
        );

        bus.write8(DTIM3_BASE + 0x03, 0x02).unwrap();
        assert_eq!(bus.pending_interrupt(), None);
    }

    #[test]
    fn restored_periodic_pit0_configuration_delivers_vector_205() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 0x3000));
        bus.attach_pit_bootstrap_with_ticks(1024);
        bus.attach_intc2();
        bus.configure_pit0_periodic().unwrap();

        for _ in 0..1400 {
            bus.advance_instruction();
        }

        assert_eq!(
            bus.pending_interrupt(),
            Some(InterruptRequest {
                source: 13,
                level: 1,
                vector: 205,
            })
        );
    }

    #[test]
    fn restored_periodic_pit2_configuration_delivers_vector_207() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 0x3000));
        bus.attach_pit_bootstrap_with_ticks(1024);
        bus.attach_intc2();
        bus.configure_pit2_periodic().unwrap();

        for _ in 0..2200 {
            bus.advance_instruction();
        }

        assert_eq!(
            bus.pending_interrupt(),
            Some(InterruptRequest {
                source: 15,
                level: 3,
                vector: 207,
            })
        );
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
        bus.watch_write_range((0x1002, 0x1003));
        bus.capture_watch_initial().unwrap();
        let mut data_registers = [0; 8];
        let mut address_registers = [0; 8];
        bus.begin_instruction(0x2000);
        data_registers[3] = 0x1234_5678;
        address_registers[1] = 0x1000;
        bus.report_instruction_state(0x2000, &data_registers, &address_registers, 0x2708);
        bus.write32(0x1000, 0xAABB_CCDD).unwrap();
        bus.begin_instruction(0x2004);
        data_registers[3] = 0x8765_4321;
        bus.report_instruction_state(0x2004, &data_registers, &address_registers, 0x2004);
        bus.write8(0x1002, 0x11).unwrap();

        assert_eq!(bus.watched_write_count, 2);
        assert_eq!(bus.watched_writes.len(), 2);
        assert_eq!(bus.watched_writes[0].pc, 0x2000);
        assert_eq!(bus.watched_writes[0].instruction_count, 1);
        assert_eq!(bus.watched_writes[0].address, 0x1002);
        assert_eq!(bus.watched_writes[0].width_bits, 32);
        assert_eq!(bus.watched_writes[0].old_bytes, [0]);
        assert_eq!(bus.watched_writes[0].new_bytes, [0xCC]);
        assert_eq!(bus.watched_writes[0].d[3], 0x1234_5678);
        assert_eq!(bus.watched_writes[0].a[1], 0x1000);
        assert_eq!(bus.watched_writes[0].sr, 0x2708);
        assert_eq!(bus.watched_writes[1].old_bytes, [0xCC]);
        assert_eq!(bus.watched_writes[1].new_bytes, [0x11]);
        assert_eq!(bus.watched_writes[1].instruction_count, 2);
        assert_eq!(bus.watched_writes[1].d[3], 0x8765_4321);
    }

    #[test]
    fn write_watch_range_records_every_overlapping_byte_with_old_and_new_values() {
        let mut bus = ProbeBus::new(FlatRam::new(0x1000, 0x100));
        bus.watch_write_range((0x1002, 0x1005));
        bus.capture_watch_initial().unwrap();
        bus.begin_instruction(0x2000);
        bus.write32(0x1000, 0xAABB_CCDD).unwrap();
        bus.begin_instruction(0x2004);
        bus.write32(0x1004, 0x1122_3344).unwrap();

        assert_eq!(bus.watched_write_count, 2);
        assert_eq!(bus.watched_writes[0].address, 0x1002);
        assert_eq!(bus.watched_writes[0].old_bytes, [0, 0]);
        assert_eq!(bus.watched_writes[0].new_bytes, [0xCC, 0xDD]);
        assert_eq!(bus.watched_writes[0].instruction_count, 1);
        assert_eq!(bus.watched_writes[1].address, 0x1004);
        assert_eq!(bus.watched_writes[1].old_bytes, [0]);
        assert_eq!(bus.watched_writes[1].new_bytes, [0x11]);
        assert_eq!(bus.watched_writes[1].instruction_count, 2);
    }

    #[test]
    fn write_watch_captures_tail_when_the_range_end_is_first_written() {
        let mut bus = ProbeBus::new(FlatRam::new(0x1000, 0x100));
        bus.watch_write_range((0x1000, 0x1040));
        bus.capture_watch_initial().unwrap();
        bus.begin_instruction(0x2000);
        bus.write8(0x103F, 0xA5).unwrap();

        let (instruction_count, tail) = bus.watched_completion_tail.unwrap();
        assert_eq!(instruction_count, 1);
        assert_eq!(tail.len(), 0x40);
        assert_eq!(tail[0x3F], 0xA5);
    }

    #[test]
    fn read_watch_records_overlapping_cpu_reads_and_disables_fast_ram() {
        let mut bus = ProbeBus::new(FlatRam::new(0x10000, 0x100));
        bus.ram.write32(0x10000, 0xAABB_CCDD).unwrap();
        bus.watch_read_range((0x10002, 0x10005));
        assert_eq!(
            bus.enable_fast_ram(0x10000, 0x100),
            Some("WATCH_READ active")
        );

        bus.begin_instruction(0x2000);
        assert_eq!(bus.read32(0x10000).unwrap(), 0xAABB_CCDD);
        bus.begin_instruction(0x2004);
        assert_eq!(bus.read16(0x10004).unwrap(), 0);

        assert_eq!(bus.watched_read_count, 2);
        assert_eq!(bus.watched_reads.len(), 2);
        assert_eq!(bus.watched_reads[0].pc, 0x2000);
        assert_eq!(bus.watched_reads[0].instruction_count, 1);
        assert_eq!(bus.watched_reads[0].address, 0x10000);
        assert_eq!(bus.watched_reads[0].width_bits, 32);
        assert_eq!(bus.watched_reads[0].value, 0xAABB_CCDD);
        assert_eq!(bus.watched_reads[1].address, 0x10004);
        assert_eq!(bus.watched_reads[1].width_bits, 16);
    }

    #[test]
    fn register_trace_keeps_most_recent_selected_pc_snapshots() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 48));
        bus.trace_registers_at(0x1000);
        let mut data_registers = [0; 8];
        let address_registers = [0; 8];
        for value in 0..=MAX_REGISTER_TRACE as u32 {
            data_registers[0] = value;
            bus.report_instruction_state(0x1000, &data_registers, &address_registers, 0);
        }

        assert_eq!(bus.register_trace.len(), MAX_REGISTER_TRACE);
        assert_eq!(bus.register_trace_count, MAX_REGISTER_TRACE + 1);
        assert_eq!(bus.register_trace.front().unwrap().d[0], 1);
        assert_eq!(
            bus.register_trace.back().unwrap().d[0],
            MAX_REGISTER_TRACE as u32
        );
        assert_eq!(
            bus.register_trace_stack.front().unwrap().len(),
            REGISTER_TRACE_STACK_WORDS
        );
    }

    #[test]
    fn instruction_trace_tail_keeps_last_snapshots_in_order() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.trace_instruction_tail(3);
        let mut data_registers = [0; 8];
        let mut address_registers = [0; 8];

        for value in 0..5 {
            data_registers[0] = value;
            address_registers[7] = 0x1000 + value;
            bus.begin_instruction(0x2000 + value * 2);
            bus.report_instruction_state(
                0x2000 + value * 2,
                &data_registers,
                &address_registers,
                0x2000 | value as u16,
            );
        }

        assert_eq!(bus.instruction_trace_tail.len(), 3);
        assert_eq!(
            bus.instruction_trace_tail
                .front()
                .unwrap()
                .instruction_count,
            3
        );
        assert_eq!(bus.instruction_trace_tail.front().unwrap().d[0], 2);
        assert_eq!(bus.instruction_trace_tail.back().unwrap().pc, 0x2008);
        assert_eq!(bus.instruction_trace_tail.back().unwrap().a[7], 0x1004);
    }

    #[test]
    fn scheduler_trace_captures_tcb_and_live_and_saved_stacks() {
        let mut bus = ProbeBus::new(FlatRam::new(0x47DB_0000, 0x10_000));
        bus.enable_scheduler_trace();
        bus.write32(0x47DB_3F64, 0x47DB_4000).unwrap();
        bus.write32(0x47DB_4048, 0x47DB_5000).unwrap();
        bus.write32(0x47DB_5000, 0x400D_1A12).unwrap();
        bus.write32(0x47DB_5100, 0xDEAD_BEEF).unwrap();
        bus.write32(0x47DB_6004, 0x47DB_4000).unwrap();
        bus.write32(0x47DB_6008, 0x400D_1A12).unwrap();
        bus.write32(0x47DB_600C, 7).unwrap();
        bus.write32(0x47DB_6010, 0x47DB_3000).unwrap();
        bus.write32(0x47DB_6014, 0x1000).unwrap();
        let data_registers = [0x1234_5678; 8];
        let mut address_registers = [0; 8];
        address_registers[7] = 0x47DB_6000;
        bus.begin_instruction(0x4000_12C8);
        bus.report_instruction_state(0x4000_12C8, &data_registers, &address_registers, 0x2700);

        address_registers[7] = 0x47DB_5100;
        bus.begin_instruction(0x4000_13EC);
        bus.report_instruction_state(0x4000_13EC, &data_registers, &address_registers, 0x2700);

        let event = bus.scheduler_trace_events.front().unwrap();
        assert_eq!(event.current_tcb, Some(0x47DB_4000));
        assert_eq!(event.tcb_saved_sp, Some(0x47DB_5000));
        assert_eq!(event.saved_sp_word0, Some(0x400D_1A12));
        assert_eq!(event.task_entry, Some(0x400D_1A12));
        assert_eq!(event.live_stack_words[0], 0xDEAD_BEEF);
        assert_eq!(event.d[0], 0x1234_5678);
        assert_eq!(bus.scheduler_task_creations[0].priority, 7);
        assert_eq!(bus.scheduler_task_creations[0].stack_base, 0x47DB_3000);
        assert_eq!(bus.scheduler_task_creations[0].stack_size, 0x1000);
    }

    #[test]
    fn conversion_trace_records_index_compare_flags_and_branch_result() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 0x100));
        bus.enable_conversion_trace();
        bus.write32(0x64, 100).unwrap();
        let mut data_registers = [0; 8];
        let mut address_registers = [0; 8];
        data_registers[1] = 100;
        address_registers[3] = 0x4000_1000;
        bus.begin_instruction(0x4018_1218);
        bus.report_instruction_state(0x4018_1218, &data_registers, &address_registers, 0x2000);

        data_registers[1] = 101;
        address_registers[7] = 0x20;
        bus.begin_instruction(0x4018_11D2);
        bus.report_instruction_state(0x4018_11D2, &data_registers, &address_registers, 0x2009);
        bus.report_branch(0x4018_11D2, 13, true, 0x4018_0FFC, 0x2009);

        assert_eq!(bus.conversion_trace_events.len(), 2);
        let index = &bus.conversion_trace_events[0];
        assert_eq!(index.d1, 100);
        assert_eq!(index.output_base, 0x4000_1000);
        assert_eq!(index.branch_taken, None);
        let branch = &bus.conversion_trace_events[1];
        assert_eq!(branch.loop_index, Some(100));
        assert_eq!(branch.d1, 101);
        assert_eq!(branch.compare_left, 101);
        assert_eq!(branch.compare_right, 100);
        assert_eq!(branch.sr & 0x1F, 0x09);
        assert_eq!(branch.branch_taken, Some(true));
        assert_eq!(branch.branch_target, Some(0x4018_0FFC));
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
    fn a1_checkpoints_capture_only_after_output_offset_is_reached() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.trace_pc_range((0x2000, 0x2010));
        bus.trace_a1_checkpoints(&[0x100, 0x200]);
        let data = [0; 8];
        let mut address = [0; 8];

        address[1] = 0x4000_04FF;
        bus.report_instruction_state(0x2000, &data, &address, 0);
        assert!(bus
            .trace_a1_checkpoints
            .iter()
            .all(|(_, trace)| trace.is_none()));

        address[0] = 0x4020_1234;
        address[1] = 0x4000_0500;
        bus.report_instruction_state(0x2002, &data, &address, 0);
        assert_eq!(
            bus.trace_a1_checkpoints[0].1.as_ref().unwrap().a[0],
            0x4020_1234
        );
        assert!(bus.trace_a1_checkpoints[1].1.is_none());

        address[0] = 0x4020_5678;
        address[1] = 0x4000_0600;
        bus.report_instruction_state(0x2004, &data, &address, 0);
        assert_eq!(
            bus.trace_a1_checkpoints[1].1.as_ref().unwrap().a[0],
            0x4020_5678
        );
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
        assert_eq!(
            bus.dump_ram(0x4020_0010, 4).unwrap(),
            [0xAA, 0xBB, 0xCC, 0xDD]
        );
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
            bus.write8(0x4020_0000 + offset, offset as u8 + 1).unwrap();
        }

        bus.begin_instruction(0x8000_5800);
        assert_eq!(bus.read8(0x4020_0000).unwrap(), 1);
        bus.begin_instruction(0x8000_5800);
        bus.write8(0x4020_0000, 0xFF).unwrap();

        assert_eq!(bus.dump_ram(0x4020_0000, 1).unwrap(), [0xFF]);
        assert_eq!(
            bus.dump_ram(0x4400_0000, 16).unwrap(),
            (1..=16).collect::<Vec<_>>()
        );
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
        assert_eq!(bus.dspi_push_count, 4);
        assert!(bus.dspi_push_events.iter().all(|event| event.module == 0));
        assert_eq!(bus.dspi_push_events[0].word, 0x8402_009F);
        assert_eq!((bus.dspi_push_events[0].word >> 16) & 0xFF, 0x02);
        assert_eq!(bus.dspi_pop_count, 4);
        assert!(bus.dspi_pop_events.iter().all(|event| event.module == 0));
        assert_eq!(
            bus.dspi_pop_events
                .iter()
                .map(|event| event.value)
                .collect::<Vec<_>>(),
            [0, 0xEF, 0x40, 0x18]
        );
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
    fn dspi1_eoq_transfer_sets_status_without_an_attached_slave() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.attach_dspi1_registers();
        bus.write32(DSPI1_BASE, 0x8000_0000).unwrap();
        bus.write32(DSPI1_BASE + 0x0C, 0x3800_0000).unwrap();
        bus.begin_instruction(0x400C_C858);
        bus.write32(DSPI1_BASE + 0x34, 0x0802_0055).unwrap();

        assert_eq!(bus.read32(DSPI1_BASE + 0x38).unwrap(), 0xFFFF);
        let status = bus.read32(DSPI1_BASE + 0x2C).unwrap();
        assert_ne!(status & (1 << 28), 0);
        assert_eq!(status >> 12 & 0xF, 0);
        assert_eq!(status >> 8 & 0xF, 1);
        assert_eq!(status >> 4 & 0xF, 0);
        assert_eq!(status & 0xF, 1);
        assert_eq!(bus.dspi_push_count, 1);
        assert_eq!(bus.dspi_push_events[0].module, 1);
        assert_eq!(bus.dspi_push_events[0].pc, 0x400C_C858);
        assert_eq!(bus.dspi_push_events[0].word, 0x0802_0055);
        assert_eq!((bus.dspi_push_events[0].word >> 16) & 0xFF, 0x02);
        assert_eq!(bus.dspi_pop_events.len(), 1);
        assert_eq!(bus.dspi_pop_events[0].module, 1);
        assert_eq!(bus.dspi_pop_events[0].value, 0xFFFF);
        assert_eq!(bus.dspi_pop_events[0].pc, 0x400C_C858);
    }

    #[test]
    fn esdhc_trace_records_command_and_completed_storage_write() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 0x1_0000));
        bus.esdhc = Some(Esdhc::new());
        bus.enable_esdhc_trace();
        bus.current_pc = 0x1234;
        bus.current_instruction_count = 10;

        bus.write32(ESDHC_BASE + 0x08, 7).unwrap();
        bus.write32(ESDHC_BASE + 0x04, (1 << 16) | 512).unwrap();
        bus.write32(ESDHC_BASE + 0x0C, 24 << 24).unwrap();
        for _ in 0..128 {
            bus.write32(ESDHC_DATPORT, 0x0102_0304).unwrap();
        }

        assert_eq!(bus.esdhc_command_events.len(), 1);
        let command = bus.esdhc_command_events[0];
        assert_eq!((command.command, command.argument), (24, 7));
        assert_eq!(command.block_attributes, (1 << 16) | 512);
        assert_eq!(bus.esdhc_write_events.len(), 1);
        assert_eq!(bus.esdhc_write_events[0].first_block, 7);
        assert_eq!(bus.esdhc_write_events[0].block_count, 1);
        assert_eq!(bus.esdhc_last_access.unwrap().address, ESDHC_DATPORT);
    }

    #[test]
    fn dspi2_bootstrap_exposes_rxctr_and_no_slave_fill() {
        let mut bus = ProbeBus::new(FlatRam::new(0, 16));
        bus.attach_dspi2_registers();
        bus.write32(DSPI2_BASE, 0x8000_0000).unwrap();
        bus.write32(DSPI2_BASE + 0x0C, 0x7800_0000).unwrap();
        bus.write32(DSPI2_BASE + 0x34, 0x0002_0055).unwrap();

        let status = bus.read32(DSPI2_BASE + 0x2C).unwrap();
        assert_eq!(status >> 4 & 0xF, 1);
        assert_eq!(bus.read32(DSPI2_BASE + 0x38).unwrap(), 0xFFFF);
        assert_eq!(bus.read32(DSPI2_BASE + 0x2C).unwrap() >> 4 & 0xF, 0);
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
