use dt2_fw::{container, syx};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: dt2-cli <firmware.syx> [outdir to write unpacked sections]");
        std::process::exit(2)
    });
    let file = std::fs::read(&path).expect("cannot read file");
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
        if let (Some(dir), Ok(v)) = (std::env::args().nth(2), &res) {
            std::fs::create_dir_all(&dir).expect("cannot create output folder");
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
