# Patches a scratch copy of the repo (argument: its root) to record each distinct executed opcode word; OPCODE_DUMP=path writes "op pc" lines at exit.
import sys
root = sys.argv[1]
p = root + "/crates/dt2-core/src/coldfire.rs"
s = open(p).read()
old = "        let op = self.fetch16(bus)?;\n"
assert s.count(old) == 1
s = s.replace(old, old + "        opprobe::note(pc0, op);\n", 1)
s += '''
pub mod opprobe {
    use std::cell::RefCell;
    thread_local! {
        static SEEN: RefCell<(Vec<u64>, Vec<(u16, u32)>)> = RefCell::new((vec![0; 1024], Vec::new()));
    }
    pub fn note(pc: u32, op: u16) {
        SEEN.with(|c| {
            let mut c = c.borrow_mut();
            let (w, b) = ((op >> 6) as usize, op & 63);
            if c.0[w] >> b & 1 == 0 {
                c.0[w] |= 1 << b;
                c.1.push((op, pc));
            }
        });
    }
    pub fn dump(path: &str) {
        SEEN.with(|c| {
            let mut v = c.borrow().1.clone();
            v.sort();
            let text: String = v.iter().map(|(o, p)| format!("{o:04x} {p:08x}\\n")).collect();
            std::fs::write(path, text).unwrap();
        });
    }
}
'''
open(p, "w").write(s)
p = root + "/crates/dt2-cli/src/main.rs"
s = open(p).read()
old = "    bus.print_instruction_trace_tail();\n"
assert s.count(old) == 1
s = s.replace(old, old + '    if let Ok(path) = std::env::var("OPCODE_DUMP") {\n        dt2_core::coldfire::opprobe::dump(&path);\n    }\n')
open(p, "w").write(s)
