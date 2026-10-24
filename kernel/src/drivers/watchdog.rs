//! IPMI 2.0 sections 9 and 27: polled KCS, SMS/OS watchdog, hard reset.
//! Only this kthread owns the interface. No interrupts feed the timer. A round
//! must return from uncached root metadata I/O and validate the root inode
//! before Reset Watchdog Timer is sent; a blocked round lets hardware expire.
//! The transport accepts only S19's four application commands.
use core::sync::atomic::{AtomicU32, Ordering};
use x86_64::instructions::port::Port;
#[path = "watchdog_round.rs"]
mod policy;

static PORT: AtomicU32 = AtomicU32::new(0);
const TIMEOUT_MS: u64 = 5_000;
const PERIOD_MS: u64 = 20_000;
use policy::TIMER;

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
fn wait(ms: u64) -> Result<(), &'static str> {
    use crate::task::{scheduler, thread::ThreadState};
    let tid = scheduler::current_thread_id().ok_or("watchdog has no thread context")?;
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    let until = seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
        .saturating_add(ms.saturating_mul(1_000_000));
    scheduler::with_current(|scheduler| scheduler.block_current_for_timer(until));
    scheduler::yield_current();
    loop {
        crate::arch_halt_with_interrupts();
        match scheduler::thread_state(tid) {
            Some(ThreadState::BlockedOnTimer) => {},
            Some(_) => return Ok(()),
            None => return Err("watchdog thread disappeared"),
        }
    }
}
fn health() -> Result<(), &'static str> {
    let guard = crate::fs::ext2::root_fs_read()?;
    let fs = guard.as_ref().ok_or("root absent")?;
    let (superblock, groups) = fs.persisted_metadata()?;
    if groups.is_empty() || superblock.block_size() == 0 || !fs.read_inode(crate::fs::ext2::root_inode())?.is_dir() {
        return Err("root metadata invalid");
    }
    Ok(())
}
impl policy::Transport for Kcs {
    fn get(&mut self) -> Result<[u8; 8], &'static str> { Kcs::get(self) }
    fn set(&mut self) -> Result<(), &'static str> { self.request(0x24, &TIMER).map(|_| ()) }
    fn reset(&mut self) -> Result<(), &'static str> { self.request(0x22, &[]).map(|_| ()) }
}
fn run(port: u32) -> Result<(), &'static str> {
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
            policy::feed(&mut kcs, health)
        })();
        match result {
            Ok(rearmed) => {
                if rearmed || !announced {
                    crate::serial_println!("[watchdog] armed seconds=120 action=hard-reset health=root-io,root-inode");
                    announced = true;
                }
                rounds += 1;
                if rounds % 3 == 0 { crate::serial_println!("[watchdog] healthy feeds={}", rounds); }
            }
            Err(reason) => crate::serial_println!("[watchdog] round failed; retrying: {}", reason),
        }
        // A transient failure gets another health round; a blocked or persistently
        // failing round still cannot keep an armed timer alive.
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
            let guard = crate::fs::ext2::root_fs_read()?;
            let fs = guard.as_ref().ok_or("fault root absent")?;
            let number = fs.resolve_path("/etc/breenix/watchdog-fault")?;
            let inode = fs.read_inode(number)?;
            if inode.size() > 32 { return Err("fault intent too long"); }
            let bytes = fs.read_file_content_coherent(number, &inode)?;
            match bytes.as_slice() { b"feeder-hang\n" => 1, b"health-hang\n" => 2, b"" => return Ok(()), _ => return Err("fault intent invalid") }
        };
        // Run independently of KCS initialization: a missing watchdog must
        // still face a real hang. Leave time for six ordinary feed rounds.
        wait(180_000)?;
        if mode == 2 {
            // A real filesystem wedge: hold its writer forever while timer
            // interrupts and scheduling continue. The ordinary health read blocks.
            let _guard = crate::fs::ext2::root_fs_write()?;
            crate::serial_println!("\n[watchdog-fault] trigger health-hang");
            loop { crate::arch_halt_with_interrupts(); }
        }
        // Both this injector and the feeder are pinned to CPU 0; halting
        // this CPU prevents the feeder from running, independent of its state.
        crate::serial_println!("\n[watchdog-fault] trigger feeder-hang");
        x86_64::instructions::interrupts::disable();
        loop { x86_64::instructions::hlt(); }
    }
}
