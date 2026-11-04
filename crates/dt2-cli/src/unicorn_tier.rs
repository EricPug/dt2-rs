//! Spike: run guest code in patched Unicorn (QEMU TCG) while `ProbeBus` keeps every device.
//!
//! RAM and SRAM are the bus's own buffers, mapped into Unicorn by pointer, so loads and stores
//! never leave the JIT. Every other address is an MMIO region whose callbacks call the bus.
//! Execution is sliced to the bus's next poll event; between slices the registers are copied
//! to the Rust `Cpu`, which delivers interrupts and executes anything Unicorn rejects.
//! Built when the patched libunicorn is found and on by default (`DT2_UNICORN=0` opts out); the Rust interpreter is the fallback.

use super::ProbeBus;
use dt2_core::bus::{Bus, BusError};
use dt2_core::coldfire::{
    Cpu, CpuError, CpuSnapshot, InterruptDelivery, InterruptInjection, RunReport, RunStop,
};
use std::cell::RefCell;
use std::ffi::c_void;

type Uc = *mut c_void;
type ReadCb = extern "C" fn(Uc, u64, u32, *mut c_void) -> u64;
type WriteCb = extern "C" fn(Uc, u64, u32, u64, *mut c_void);
type IntrCb = extern "C" fn(Uc, u32, *mut c_void);

extern "C" {
    fn uc_open(arch: i32, mode: i32, uc: *mut Uc) -> i32;
    fn uc_ctl(uc: Uc, control: i32, ...) -> i32;
    fn uc_mem_map_ptr(uc: Uc, address: u64, size: usize, perms: u32, ptr: *mut u8) -> i32;
    fn uc_mmio_map(
        uc: Uc,
        address: u64,
        size: usize,
        read: ReadCb,
        read_data: *mut c_void,
        write: WriteCb,
        write_data: *mut c_void,
    ) -> i32;
    fn uc_hook_add(uc: Uc, hook: *mut usize, kind: i32, cb: IntrCb, data: *mut c_void, begin: u64, end: u64, ...) -> i32;
    fn uc_emu_start(uc: Uc, begin: u64, until: u64, timeout: u64, count: usize) -> i32;
    fn uc_reg_read(uc: Uc, reg: i32, value: *mut c_void) -> i32;
    fn uc_reg_write(uc: Uc, reg: i32, value: *const c_void) -> i32;
    fn uc_digikit_set_options(uc: Uc, bits: u32) -> i32;
    fn uc_digikit_set_irq_level(uc: Uc, level: u32);
    fn uc_digikit_stop_at_boundary(uc: Uc);
    fn uc_digikit_emac_set(uc: Uc, macc: *const u64, macsr: u32, mask: u32);
    fn uc_digikit_emac_get(uc: Uc, macc: *mut u64, macsr: *mut u32, mask: *mut u32);
    fn uc_digikit_counter(uc: Uc) -> u64;
    fn uc_digikit_count_limit(uc: Uc) -> u64;
}

const UC_ARCH_M68K: i32 = 7;
const UC_MODE_BIG_ENDIAN: i32 = 1 << 30;
const UC_CTL_WRITE_CPU_MODEL: i32 = 1_140_850_695;
const UC_CPU_M68K_CFV4E: i32 = 7;
const UC_HOOK_INTR: i32 = 1;
const REG_A0: i32 = 1;
const REG_D0: i32 = 9;
const REG_SR: i32 = 17;
const REG_PC: i32 = 18;
const NATIVE_RTE: u32 = 1;
const NO_MEM_EXIT: u32 = 2;
const NO_HOOK_PC_SYNC: u32 = 4;
const ERR_INSN_INVALID: i32 = 10;

/// State the callbacks share with the run loop.
struct Shared {
    bus: *mut ProbeBus,
    uc: Uc,
    begun: bool,
    /// Instructions of the current slice already credited to the bus.
    credited: usize,
    first_error: Option<(u32, BusError)>,
    stop_on_access: bool,
    intr: Option<u32>,
    intr_counter: usize,
}

impl Shared {
    /// Settles time before the instruction now touching the bus, as the Rust loop would have:
    /// earlier instructions have retired, and this one has begun (the count includes it, the
    /// timers' lag does not).
    fn credit(&mut self) {
        let counter = unsafe { uc_digikit_counter(self.uc) } as usize;
        let before = counter.saturating_sub(1);
        let bus = unsafe { &mut *self.bus };
        if before > self.credited {
            bus.retire_batch(before - self.credited);
            self.credited = before;
        }
        if !self.begun {
            self.begun = true;
            bus.current_instruction_count += 1;
        }
    }

    /// Ends the slice at the next instruction boundary when the access may have changed what
    /// is pending.
    fn finish_access(&mut self) {
        if self.stop_on_access && unsafe { &*self.bus }.irq_dirty {
            unsafe { uc_digikit_stop_at_boundary(self.uc) };
        }
    }
}

struct Region {
    shared: *mut Shared,
    base: u32,
}

extern "C" fn mmio_read(_uc: Uc, offset: u64, size: u32, user: *mut c_void) -> u64 {
    let region = unsafe { &*(user as *const Region) };
    let shared = unsafe { &mut *region.shared };
    shared.credit();
    let bus = unsafe { &mut *shared.bus };
    let address = region.base.wrapping_add(offset as u32);
    let result = match size {
        1 => bus.read8(address).map(u64::from),
        2 => bus.read16(address).map(u64::from),
        _ => bus.read32(address).map(u64::from),
    };
    let value = result.unwrap_or_else(|error| {
        shared.first_error.get_or_insert((address, error));
        0
    });
    shared.finish_access();
    value
}

extern "C" fn mmio_write(_uc: Uc, offset: u64, size: u32, value: u64, user: *mut c_void) {
    let region = unsafe { &*(user as *const Region) };
    let shared = unsafe { &mut *region.shared };
    shared.credit();
    let bus = unsafe { &mut *shared.bus };
    let address = region.base.wrapping_add(offset as u32);
    let result = match size {
        1 => bus.write8(address, value as u8),
        2 => bus.write16(address, value as u16),
        _ => bus.write32(address, value as u32),
    };
    if let Err(error) = result {
        shared.first_error.get_or_insert((address, error));
    }
    shared.finish_access();
}

extern "C" fn on_intr(uc: Uc, number: u32, user: *mut c_void) {
    let shared = unsafe { &mut *(user as *mut Shared) };
    if shared.intr.is_none() {
        shared.intr = Some(number);
        // The faulting instruction has begun but not retired. Returning from the hook makes the
        // engine retry it, so stop at that retry and remember which instruction it was.
        shared.intr_counter = unsafe { uc_digikit_counter(uc) } as usize;
        unsafe { uc_digikit_stop_at_boundary(uc) };
    }
}

pub struct UnicornTier {
    uc: Uc,
    shared: Box<Shared>,
    _regions: Vec<Box<Region>>,
    max_slice: usize,
    pub slices: u64,
    pub fallback_steps: u64,
    pub irqs: std::collections::BTreeMap<u8, u64>,
}

impl UnicornTier {
    fn new(bus: &mut ProbeBus) -> Self {
        let max_slice = std::env::var("DT2_UNICORN_SLICE").ok().and_then(|v| v.parse().ok()).unwrap_or(4096);
        let mut uc: Uc = std::ptr::null_mut();
        unsafe {
            assert_eq!(uc_open(UC_ARCH_M68K, UC_MODE_BIG_ENDIAN, &mut uc), 0);
            assert_eq!(uc_ctl(uc, UC_CTL_WRITE_CPU_MODEL, UC_CPU_M68K_CFV4E), 0);
        }
        let mut shared = Box::new(Shared {
            bus: bus as *mut ProbeBus,
            uc,
            begun: false,
            credited: 0,
            first_error: None,
            stop_on_access: std::env::var_os("DT2_UNICORN_NOSTOP").is_none(),
            intr: None,
            intr_counter: 0,
        });
        let mut windows = bus.host_memory();
        windows.sort_by_key(|w| w.0);
        let mut cursor: u64 = 0;
        let mut regions = Vec::new();
        let map_mmio = |start: u64, end: u64, regions: &mut Vec<Box<Region>>, shared: &mut Shared| {
            if end > start {
                let region = Box::new(Region { shared, base: start as u32 });
                let user = &*region as *const Region as *mut c_void;
                let status = unsafe {
                    uc_mmio_map(uc, start, (end - start) as usize, mmio_read, user, mmio_write, user)
                };
                assert_eq!(status, 0, "uc_mmio_map {start:#x}..{end:#x}");
                regions.push(region);
            }
        };
        for &(base, ptr, len) in &windows {
            map_mmio(cursor, u64::from(base), &mut regions, &mut shared);
            let status = unsafe { uc_mem_map_ptr(uc, u64::from(base), len, 7, ptr) };
            assert_eq!(status, 0, "uc_mem_map_ptr {base:#x}");
            cursor = u64::from(base) + len as u64;
        }
        map_mmio(cursor, 1 << 32, &mut regions, &mut shared);
        unsafe {
            let mut hook = 0usize;
            let data = &mut *shared as *mut Shared as *mut c_void;
            assert_eq!(uc_hook_add(uc, &mut hook, UC_HOOK_INTR, on_intr, data, 1, 0), 0);
            let options = NATIVE_RTE | NO_MEM_EXIT | NO_HOOK_PC_SYNC;
            assert_eq!(uc_digikit_set_options(uc, options), 0, "patched Unicorn required");
        }
        Self { uc, shared, _regions: regions, max_slice, slices: 0, fallback_steps: 0, irqs: Default::default() }
    }

    fn start(bus: &mut ProbeBus, cpu: &Cpu) -> Self {
        let mut tier = Self::new(bus);
        tier.load(cpu);
        tier.shared.bus = bus as *mut ProbeBus;
        tier
    }

    fn load(&mut self, cpu: &Cpu) {
        self.load_emac(cpu);
        // SR first: writing it can swap stack pointers.
        unsafe {
            let sr = u32::from(cpu.sr);
            uc_reg_write(self.uc, REG_SR, &sr as *const u32 as *const c_void);
            for i in 0..8 {
                uc_reg_write(self.uc, REG_A0 + i as i32, &cpu.a[i] as *const u32 as *const c_void);
                uc_reg_write(self.uc, REG_D0 + i as i32, &cpu.d[i] as *const u32 as *const c_void);
            }
            uc_reg_write(self.uc, REG_PC, &cpu.pc as *const u32 as *const c_void);
        }
    }

    fn load_emac(&mut self, cpu: &Cpu) {
        let macc = [0, 1, 2, 3].map(|n| cpu.emac.raw48(n));
        let (_, _, _, macsr, mask) = cpu.emac.raw_registers();
        unsafe { uc_digikit_emac_set(self.uc, macc.as_ptr(), macsr, mask) };
    }

    /// Reads the engine's raw EMAC state back into `cpu`.
    fn extract_emac(&mut self, cpu: &mut Cpu) {
        let (mut macc, mut macsr, mut mask) = ([0u64; 4], 0u32, 0u32);
        unsafe { uc_digikit_emac_get(self.uc, macc.as_mut_ptr(), &mut macsr, &mut mask) };
        cpu.emac.load_raw(macc, macsr, mask);
    }

    fn store(&mut self, cpu: &mut Cpu) {
        self.extract_emac(cpu);
        unsafe {
            let mut sr = 0u32;
            uc_reg_read(self.uc, REG_SR, &mut sr as *mut u32 as *mut c_void);
            cpu.sr = sr as u16;
            for i in 0..8 {
                uc_reg_read(self.uc, REG_A0 + i as i32, &mut cpu.a[i] as *mut u32 as *mut c_void);
                uc_reg_read(self.uc, REG_D0 + i as i32, &mut cpu.d[i] as *mut u32 as *mut c_void);
            }
            uc_reg_read(self.uc, REG_PC, &mut cpu.pc as *mut u32 as *mut c_void);
        }
    }

    fn set_irq_level(&mut self, level: u32) {
        unsafe { uc_digikit_set_irq_level(self.uc, level) };
    }

    /// Runs up to `n` instructions from `pc`; returns (executed, engine status).
    fn slice(&mut self, bus: &mut ProbeBus, pc: u32, n: usize) -> (usize, i32) {
        self.slices += 1;
        self.shared.bus = bus as *mut ProbeBus;
        self.shared.credited = 0;
        self.shared.begun = false;
        self.shared.intr = None;
        let status = unsafe { uc_emu_start(self.uc, u64::from(pc), u64::MAX, 0, n) };
        let counter = unsafe { uc_digikit_counter(self.uc) } as usize;
        let limit = unsafe { uc_digikit_count_limit(self.uc) } as usize;
        // An exception leaves the faulting instruction begun but not retired.
        let executed = if let Some(at) = self.shared.intr.and(Some(self.shared.intr_counter)) {
            at.saturating_sub(1)
        } else if status != 0 {
            counter.saturating_sub(1)
        } else {
            limit.min(counter)
        };
        let begun = usize::from(self.shared.begun);
        bus.retire_external(executed.saturating_sub(self.shared.credited + begun), executed.saturating_sub(self.shared.credited));
        if std::env::var_os("DT2_UNICORN_TRACE").is_some() {
            let mut pc_after = 0u32;
            unsafe { uc_reg_read(self.uc, REG_PC, &mut pc_after as *mut u32 as *mut c_void) };
            eprintln!("slice pc={pc:#010x} n={n} executed={executed} status={status} counter={counter} limit={limit} intr={:?} pc_after={pc_after:#010x} count={}", self.shared.intr, bus.current_instruction_count);
        }
        (executed, status)
    }
}

thread_local! {
    static TIER: RefCell<Option<Box<UnicornTier>>> = const { RefCell::new(None) };
}

/// On by default when built; `DT2_UNICORN=0` selects the Rust interpreter.
pub fn enabled() -> bool {
    std::env::var("DT2_UNICORN").map_or(true, |v| v != "0")
}

pub fn run(
    cpu: &mut Cpu,
    bus: &mut ProbeBus,
    limit: usize,
    mut injection: Option<InterruptInjection>,
) -> RunReport {
    TIER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let tier = slot.get_or_insert_with(|| Box::new(UnicornTier::start(bus, cpu)));
        run_with(tier, cpu, bus, limit, &mut injection)
    })
}

fn run_with(
    tier: &mut UnicornTier,
    cpu: &mut Cpu,
    bus: &mut ProbeBus,
    limit: usize,
    injection: &mut Option<InterruptInjection>,
) -> RunReport {
    let mut done = 0usize;
    let mut instructions = 0usize;
    let mut natural_interrupt = None;
    let mut injected_interrupt = None;
    let mut force_poll = true;
    tier.load(cpu);
    let stop = 'run: loop {
        if done >= limit {
            break RunStop::InstructionLimit;
        }
        let mask = ((cpu.sr >> 8) & 7) as u8;
        if let Some(event) = *injection {
            if done >= event.instruction_count && event.level > mask {
                if let Err(error) = cpu.take_interrupt(bus, event.level, event.vector) {
                    break RunStop::Error(error);
                }
                bus.interrupt_taken(event.level, event.vector);
                tier.load(cpu);
                force_poll = true;
                *injection = None;
                injected_interrupt = Some(event);
                continue;
            }
        }
        if force_poll || bus.poll_due() {
            force_poll = false;
            let pending = bus.pending_interrupt();
            // Tell the engine which request is held back, so that an SR change (RTE, MOVE to
            // SR) that unmasks it ends the slice there.
            tier.set_irq_level(pending.filter(|r| r.level <= mask).map_or(0, |r| u32::from(r.level)));
            if let Some(request) = pending {
                if request.level > mask {
                    if let Err(error) = cpu.take_interrupt(bus, request.level, request.vector) {
                        break RunStop::Error(error);
                    }
                    bus.interrupt_taken(request.level, request.vector);
                    *tier.irqs.entry(request.vector).or_default() += 1;
                    tier.load(cpu);
                    force_poll = true;
                    natural_interrupt.get_or_insert(InterruptDelivery {
                        instruction_count: done,
                        source: request.source,
                        level: request.level,
                        vector: request.vector,
                    });
                    continue;
                }
            }
        }
        let mut room = bus.quiet_horizon();
        if room > 1 {
            room = room.min(limit - done).min(tier.max_slice);
            if let Some(event) = *injection {
                room = room.min(event.instruction_count.saturating_sub(done));
            }
            room = room.min(bus.poll_countdown.max(1) as usize);
        }
        let mut status = 0;
        if room > 1 {
            let (executed, st) = tier.slice(bus, cpu.pc, room);
            done += executed;
            instructions += executed;
            status = st;
            tier.store(cpu);
            force_poll = true;
            if let Some((address, error)) = tier.shared.first_error.take() {
                eprintln!("unicorn tier: bus error {error:?} at {address:#010x}");
                break RunStop::Error(CpuError::Bus(error));
            }
            // At the budget the pending TRAP / rejected opcode stays unexecuted for the next run.
            if (tier.shared.intr.is_none() && status == 0) || done >= limit {
                continue;
            }
        }
        // One instruction through the Rust interpreter: hooks are on, an injection is due, or
        // Unicorn raised an exception / rejected an opcode.
        if status != 0 && status != ERR_INSN_INVALID && tier.shared.intr.is_none() {
            eprintln!("unicorn tier: engine status {status} at pc {:#010x}", cpu.pc);
        }
        if std::env::var_os("DT2_UNICORN_DEBUG").is_some() {
            eprintln!("fallback pc={:#010x} op={:04x} status={status} intr={:?}", cpu.pc, bus.read16(cpu.pc).unwrap_or(0), tier.shared.intr);
        }
        tier.shared.intr = None;
        tier.fallback_steps += 1;
        match cpu.step(bus) {
            Ok(()) => {
                bus.advance_instruction();
                done += 1;
                instructions += 1;
                tier.load(cpu);
            }
            Err(CpuError::Halt { pc }) => {
                instructions += 1;
                break 'run RunStop::Halt { pc };
            }
            Err(error) => break RunStop::Error(error),
        }
    };
    if std::env::var_os("DT2_UNICORN_TRACE").is_some() { eprintln!("run_with end: limit={limit} done={done} instructions={instructions} count={}", bus.current_instruction_count); }
    bus.end_run();
    RunReport {
        instructions,
        fast_forwarded_instructions: 0,
        injected_interrupt,
        natural_interrupt,
        cpu: CpuSnapshot {
            d: cpu.d,
            a: cpu.a,
            pc: cpu.pc,
            sr: cpu.sr,
            rambar: cpu.rambar,
            acr0: cpu.acr0,
            acr1: cpu.acr1,
            vbr: cpu.vbr,
            cacr: cpu.cacr,
            rgpiobar: cpu.rgpiobar,
            debug_csr: cpu.debug_csr,
        },
        stop,
    }
}

pub fn print_stats() {
    TIER.with(|cell| {
        if let Some(tier) = cell.borrow().as_ref() {
            println!(
                "unicorn tier: {} slices, {} Rust fallback steps, interrupts by vector {:?}",
                tier.slices, tier.fallback_steps, tier.irqs
            );
        }
    });
}
