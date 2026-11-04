//! Differential sweep: executes every 16-bit opcode once from several seeded machine states and
//! prints one FNV-1a64 hash of every outcome (registers, SR, PC, error, a RAM window).
//! Run before and after changing `execute`; the hashes must match.
//! `N_SEEDS=n` sets the number of seeds (default 6). `CACHED=1` runs each opcode twice with the
//! decode cache on and hashes the second (cache-hit, handler) run; the hash must be unchanged.
use dt2_core::bus::{Bus, FlatRam};
use dt2_core::coldfire::Cpu;

const BASE: u32 = 0x4000_0000;
const WINDOW: u32 = 0x4000_8000;

struct Fnv(u64);
impl Fnv {
    fn put(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
}

fn lcg(s: &mut u64) -> u32 {
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (*s >> 32) as u32
}

/// Writes the seeded data window and registers; the code page is not touched.
fn seed_state(ram: &mut FlatRam, cpu: &mut Cpu, rng: &mut u64, seed: u64) {
    for i in 0..0x400u32 {
        ram.write8(WINDOW + i, lcg(rng) as u8).unwrap();
    }
    cpu.pc = BASE;
    for i in 0..8 {
        cpu.d[i] = match lcg(rng) % 4 {
            0 => lcg(rng) & 0x1F,
            1 => 0,
            2 => 0x8000_0000 | lcg(rng),
            _ => lcg(rng),
        };
        cpu.a[i] = WINDOW + 0x100 + (lcg(rng) & 0x7F);
    }
    cpu.sr = if seed % 3 == 2 { 0x0000 } else { 0x2700 } | (lcg(rng) as u16 & 0x1F);
}

fn main() {
    let cached = std::env::var("CACHED").is_ok_and(|v| v == "1");
    let double = std::env::var("DOUBLE").is_ok_and(|v| v == "1");
    let per_op = std::env::var("PER_OP").is_ok_and(|v| v == "1");
    let seeds: u64 = std::env::var("N_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    for seed in 0..seeds {
        let mut ram = FlatRam::new(BASE, 0x10000);
        let mut cpu = Cpu::new();
        cpu.set_icache(cached);
        for op in 0..=0xFFFFu32 {
            let mut rng = seed.wrapping_mul(0x9E37_79B9).wrapping_add(op as u64 * 31 + 1);
            // Writing the code page bumps the cache epoch, so stale entries are never replayed.
            ram.write16(BASE, op as u16).unwrap();
            for i in 0..6u32 {
                let w = lcg(&mut rng) as u16;
                ram.write16(BASE + 2 + 2 * i, w).unwrap();
            }
            let state_rng = rng;
            seed_state(&mut ram, &mut cpu, &mut rng, seed);
            if cached || double {
                let saved = ram.clone();
                cpu.run(&mut ram, 1);
                ram = saved;
                // The clone lost the code-page mark; without it the next opcode's write would not
                // bump the epoch and a stale entry would replay.
                ram.cacheable_code(BASE, 12);
                let mut again = state_rng;
                seed_state(&mut ram, &mut cpu, &mut again, seed);
            }
            let report = cpu.run(&mut ram, 1);
            if std::env::var("PRINT_OP").is_ok_and(|v| u32::from_str_radix(&v, 16) == Ok(op)) {
                println!("{op:04x} {:?} {:?}", report.stop, report.cpu);
            }
            let mut one = Fnv(0xcbf2_9ce4_8422_2325);
            std::mem::swap(&mut one, &mut h);
            let chain = one.0;
            h.put(op as u64);
            for b in format!("{:?}", report.stop).bytes() {
                h.put(b as u64);
            }
            for b in format!("{:?}", report.cpu).bytes() {
                h.put(b as u64);
            }
            for i in (0..0x400).step_by(4) {
                h.put(ram.read32(WINDOW + i).unwrap() as u64);
            }
            if per_op {
                println!("{seed} {op:04x} {:016x}", h.0);
            }
            std::mem::swap(&mut one, &mut h);
            h.0 = chain;
            for b in one.0.to_le_bytes() {
                h.put(b as u64);
            }
        }
    }
    println!("opsweep {:016x}", h.0);
}
