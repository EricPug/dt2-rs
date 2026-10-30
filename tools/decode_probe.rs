use dt2_core::bus::FlatRam;
use dt2_core::coldfire::{Cpu, CpuError};
use std::io::BufRead;

fn main() {
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut it = line.split_whitespace();
        let (Some(tag), Some(hex)) = (it.next(), it.next()) else { continue };
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        let mut ram = FlatRam::new(0, 0x10_0000);
        ram.load(0x1000, &bytes);
        let mut cpu = Cpu::new();
        cpu.pc = 0x1000;
        cpu.sr = 0x2000;
        cpu.a = [0x8_0000; 8];
        cpu.a[7] = 0xC_0000;
        let r = cpu.step(&mut ram);
        let status = match r {
            Ok(()) => format!("OK {}", cpu.pc.wrapping_sub(0x1000) as i32),
            Err(CpuError::Illegal { .. }) => "ILL 0".into(),
            Err(e) => format!("ERR {e:?}").replace(' ', ""),
        };
        println!("{tag} {status}");
    }
}
