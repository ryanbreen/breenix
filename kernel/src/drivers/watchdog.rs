//! IPMI 2.0 sections 9 and 27: polled KCS, SMS/OS watchdog, hard reset.
//! Only this kthread owns the interface. Each feed requires root-storage health.
//! The transport accepts only S19's four application commands.
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use x86_64::instructions::port::Port;
use super::watchdog_round as policy;
use policy::wait;

static PORT: AtomicU32 = AtomicU32::new(0);
const TIMEOUT_MS: u64 = 5_000;
const PERIOD_MS: u64 = 20_000;
const TIMER: [u8; 6] = [4, 1, 0, 0x10, 0xb0, 4];

#[path = "watchdog_progress.rs"]
mod progress;
use progress::{Overdue, Phase, Progress};

static PROGRESS: Progress = Progress::new();
static FEEDER_TID: AtomicU64 = AtomicU64::new(0);
static PROBE_PORT: AtomicU32 = AtomicU32::new(u32::MAX);

pub(crate) fn root_probe_waiting(disk: Option<crate::block::HardwareDisk>) {
    if !is_feeder() {
        return;
    }
    let port = match disk {
        Some(crate::block::HardwareDisk::Ahci(port)) => port as u32,
        _ => u32::MAX,
    };
    PROBE_PORT.store(port, Ordering::Relaxed);
    PROGRESS.enter(Phase::RootMount);
}

pub(crate) fn root_probe_reading(inode: bool) {
    if is_feeder() {
        PROGRESS.enter(if inode { Phase::RootInode } else { Phase::RootSuperblock });
    }
}

fn is_feeder() -> bool {
    let feeder = FEEDER_TID.load(Ordering::Relaxed);
    feeder != 0 && crate::arch_without_interrupts(|| {
        crate::per_cpu::current_thread_id_lock_free() == Some(feeder)
    })
}

/// Sample from another CPU so a blocked feeder can still be observed.
fn report_stall() {
    let Some(Overdue { phase, age_ms, has_fed }) = PROGRESS.overdue(now()) else { return; };
    let tid = FEEDER_TID.load(Ordering::Relaxed);
    let running = crate::task::scheduler::try_liveness_snapshot(0)
        .map(|s| s.per_cpu_current);
    crate::serial_println!(
        "[watchdog] feeder overdue: phase={:?} tid={} age_ms={} has_fed={} running={:?}",
        phase, tid, age_ms, has_fed, running,
    );
    let port = PROBE_PORT.load(Ordering::Relaxed);
    if matches!(phase, Phase::RootMount | Phase::RootSuperblock | Phase::RootInode)
        && port != u32::MAX
    {
        super::ahci::report_watchdog_port(port as usize);
    }
}


pub fn discover(rsdp: Option<u64>, offset: u64) {
    if let Some((data, status)) = crate::arch_impl::x86_64::acpi::read_kcs_ports(rsdp, offset) {
        PORT.store(u32::from(data) | (u32::from(status) << 16), Ordering::Release);
    }
}

struct Kcs { data: Port<u8>, status: Port<u8> }
impl Kcs {
    fn new(ports: u32) -> Self { Self { data: Port::new(ports as u16), status: Port::new((ports >> 16) as u16) } }
    fn status(&mut self) -> u8 { unsafe { self.status.read() } }
    fn ready(&mut self, deadline: u64) -> Result<u8, &'static str> {
        loop {
            let status = self.status();
            if status & 2 == 0 { return Ok(status); }
            if now() >= deadline { return Err("KCS input timeout"); }
            core::hint::spin_loop();
        }
    }
    fn output(&mut self, deadline: u64) -> Result<u8, &'static str> {
        loop {
            if self.status() & 1 != 0 { return Ok(unsafe { self.data.read() }); }
            if now() >= deadline { return Err("KCS output timeout"); }
            core::hint::spin_loop();
        }
    }
    fn clear_output(&mut self, status: u8) {
        if status & 1 != 0 { unsafe { self.data.read(); } }
    }
    fn request(&mut self, command: u8, data: &[u8]) -> Result<([u8; 32], usize), &'static str> {
        if !matches!((command, data.len()), (1, 0) | (0x22, 0) | (0x25, 0))
            && !(command == 0x24 && data == TIMER) {
            return Err("command outside watchdog scope");
        }
        match command {
            0x25 => PROGRESS.enter(Phase::KcsGet),
            0x24 => PROGRESS.enter(Phase::KcsSet),
            0x22 => PROGRESS.enter(Phase::KcsReset),
            _ => {}
        }
        let deadline = now().saturating_add(TIMEOUT_MS);
        let status = self.ready(deadline)?;
        self.clear_output(status);
        // WRITE_START starts a new transfer, abandoning an incomplete one (IPMI 2.0, 9.19).
        unsafe { self.status.write(0x61); }
        let mut packet = [0u8; 8];
        packet[0] = 6 << 2; // Application request NetFn, LUN 0
        packet[1] = command;
        packet[2..2 + data.len()].copy_from_slice(data);
        for (index, byte) in packet[..2 + data.len()].iter().enumerate() {
            let status = self.ready(deadline)?;
            if status >> 6 != 2 { return Err("KCS not in write state"); }
            self.clear_output(status);
            if index == 1 + data.len() {
                unsafe { self.status.write(0x62); } // WRITE_END precedes last data byte
                let status = self.ready(deadline)?;
                if status >> 6 != 2 { return Err("KCS write-end state"); }
                self.clear_output(status);
            }
            unsafe { self.data.write(*byte); }
        }
        let mut reply = [0u8; 32];
        let mut length = 0;
        loop {
            let status = self.ready(deadline)?;
            match status >> 6 {
                1 => {
                    let byte = self.output(deadline)?;
                    if length == reply.len() { return Err("KCS reply too long"); }
                    reply[length] = byte;
                    length += 1;
                    unsafe { self.data.write(0x68); } // READ_BYTE
                }
                0 => { self.output(deadline)?; break; } // final dummy byte
                _ => return Err("KCS read state"),
            }
        }
        if length < 3 || reply[0] != 7 << 2 || reply[1] != command || reply[2] != 0 {
            return Err("IPMI response refused");
        }
        Ok((reply, length))
    }
    fn get(&mut self) -> Result<[u8; 8], &'static str> {
        let (reply, length) = self.request(0x25, &[])?;
        if length != 11 { return Err("watchdog response length"); }
        let mut timer = [0; 8];
        timer.copy_from_slice(&reply[3..11]);
        Ok(timer)
    }
}
fn now() -> u64 {
    let (seconds, ns) = crate::time::get_monotonic_time_ns();
    seconds.saturating_mul(1000).saturating_add(ns / 1_000_000)
}
impl policy::Transport for Kcs {
    type State = [u8; 8];
    fn configured(timer: &Self::State) -> bool {
        timer[0] & 0x47 == 0x44 && timer[1] == 1 && timer[2] == 0 && timer[4..6] == TIMER[4..6]
    }
    fn get(&mut self) -> Result<[u8; 8], &'static str> { Kcs::get(self) }
    fn before_rearm(&mut self, timer: &Self::State) {
        let flags = timer[3];
        let running = u8::from(timer[0] & 0x40 != 0);
        let countdown = u16::from_le_bytes([timer[6], timer[7]]);
        // Set clears the SMS/OS expiry flag, so keep the raw Get state first.
        let event = if flags & TIMER[3] != 0 { "expired-before-rearm" } else { "state-before-rearm" };
        crate::serial_println!(
            "[watchdog] {} flags={:#04x} running={} countdown-ticks={}",
            event, flags, running, countdown,
        );
    }
    fn set(&mut self) -> Result<(), &'static str> { self.request(0x24, &TIMER).map(|_| ()) }
    fn reset(&mut self) -> Result<(), &'static str> {
        self.request(0x22, &[])?;
        #[cfg(not(test))]
        PROGRESS.fed(now());
        Ok(())
    }
}
fn run(port: u32) -> Result<(), &'static str> {
    #[cfg(not(test))]
    {
        FEEDER_TID.store(crate::task::scheduler::current_thread_id().unwrap_or(0), Ordering::Relaxed);
        PROGRESS.start(now());
    }
    let mut kcs = Kcs::new(port);
    let mut identified = false;
    let mut announced = false;
    let mut rounds = 0u64;
    loop {
        let result = (|| {
            if !identified {
                let (_, length) = kcs.request(1, &[])?;
                if length < 14 { return Err("device ID length"); }
                let previous = kcs.get()?;
                crate::serial_println!("[watchdog] previous-expired={}", u8::from(previous[3] & 0x10 != 0));
                identified = true;
            }
            #[cfg(not(test))]
            PROGRESS.enter(Phase::RootLookup);
            policy::feed(&mut kcs, policy::health)
        })();
        match result {
            Ok(rearmed) => {
                if announced {
                    rounds += 1;
                    if rounds % 3 == 0 { crate::serial_println!("[watchdog] healthy feeds={}", rounds); }
                }
                if rearmed || !announced {
                    crate::serial_println!("[watchdog] armed seconds=120 action=hard-reset health=root-io,root-inode");
                    announced = true;
                }
            }
            Err(reason) => crate::serial_println!("[watchdog] round failed; retrying: {}", reason),
        }
        // Retry after the scheduler wait. A stalled feeder cannot keep an
        // armed timer alive.
        #[cfg(not(test))]
        PROGRESS.enter(Phase::Waiting);
        wait(PERIOD_MS)?;
    }
}
pub fn start() {
    // Supervised test boots retain the driver's silence/NMI capture path.
    // Only the persistent target and explicit watchdog fault builds arm KCS.
    #[cfg(not(feature = "watchdog_fault_inject"))]
    if !crate::boot::intent::get().is_some_and(|bytes| bytes == b"persistent\n") {
        crate::serial_println!("[watchdog] disabled: supervised boot");
        return;
    }
    #[cfg(feature = "watchdog_fault_inject")]
    if crate::task::kthread::kthread_run_on_cpu(|| {
        if let Err(reason) = fault::run() {
            crate::serial_println!("[watchdog-fault] request failed: {}", reason);
        }
    }, "watchdog-fault", 0).is_err() {
        crate::serial_println!("[watchdog-fault] thread could not start");
    }
    let port = PORT.load(Ordering::Acquire);
    if port == 0 { return; }
    let observer_cpu = usize::from(crate::arch_impl::x86_64::smp::is_cpu_online(1));
    if crate::task::kthread::kthread_run_on_cpu(|| {
        while wait(5_000).is_ok() {
            report_stall();
        }
    }, "watchdog-observer", observer_cpu).is_err() {
        crate::serial_println!("[watchdog] observer could not start");
    }
    if crate::task::kthread::kthread_run_on_cpu(move || {
        if let Err(reason) = run(port) {
            // Losing the thread context prevents further rounds.
            crate::serial_println!("[watchdog] stopped: {}", reason);
        }
    }, "watchdog", 0).is_err() {
        crate::serial_println!("[watchdog] feeder could not start");
    }
}

#[cfg(feature = "watchdog_fault_inject")]
mod fault {
    use super::wait;
    pub fn run() -> Result<(), &'static str> {
        let mode = {
            let bytes = crate::fs::vfs::read_config("/etc/breenix/watchdog-fault", 32)
                .map_err(|_| "fault intent unreadable")?;
            match bytes.as_slice() { b"feeder-hang\n" => 1, b"health-hang\n" => 2, b"" => return Ok(()), _ => return Err("fault intent invalid") }
        };
        // Run independently of KCS initialization: a missing watchdog must
        // still face a real hang. Leave time for six ordinary feed rounds.
        wait(180_000)?;
        if mode == 2 {
            // A real filesystem wedge: hold its writer forever while timer
            // interrupts and scheduling continue. The ordinary health read blocks.
            return crate::fs::vfs::root_filesystem().map_err(|_| "fault root absent")?
                .hold_writer(&|| {
                    crate::serial_println!("\n[watchdog-fault] trigger health-hang");
                    loop { crate::arch_halt_with_interrupts(); }
                }).map_err(|_| "fault writer unavailable");
        }
        // Both this injector and the feeder are pinned to CPU 0; halting
        // this CPU prevents the feeder from running, independent of its state.
        crate::serial_println!("\n[watchdog-fault] trigger feeder-hang");
        x86_64::instructions::interrupts::disable();
        loop { x86_64::instructions::hlt(); }
    }
}
