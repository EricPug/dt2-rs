use dt2_core::bus::{Bus, FlatRam};
use dt2_core::coldfire::Cpu;
fn main() {
    // loop: moveq #1,d0; add.l d0,d1; addq.l #1,d2; move.l d1,(a0); move.l (a0),d3; subq.l #1,d4; bne loop
    let code: [u16; 8] = [
        0x7001, 0xD280, 0x5282, 0x2081, 0x2610, 0x5384, 0x66F2, 0x4E71,
    ];
    for cache in std::env::var("C")
        .map(|c| vec![c == "1"])
        .unwrap_or(vec![false, true])
    {
        let mut ram = FlatRam::new(0x4000_0000, 0x10000);
        for (i, w) in code.iter().enumerate() {
            ram.write16(0x4000_0000 + 2 * i as u32, *w).unwrap();
        }
        let mut cpu = Cpu::new();
        cpu.set_icache(cache);
        cpu.pc = 0x4000_0000;
        cpu.a[0] = 0x4000_8000;
        cpu.d[4] = u32::MAX;
        cpu.sr = 0x2700;
        let n: usize = std::env::var("N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20_000_000);
        let t = std::time::Instant::now();
        let r = cpu.run(&mut ram, n);
        let e = t.elapsed().as_secs_f64();
        println!(
            "cache={cache} {:?} {:.1} M instr/s",
            r.stop,
            n as f64 / e / 1e6
        );
    }
}
