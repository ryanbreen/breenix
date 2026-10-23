//! IPMI 2.0 sections 9 and 27: polled KCS, SMS/OS watchdog, hard reset.
//! Only this kthread owns the interface. No interrupts feed the timer. A round
//! must return from uncached root metadata I/O and validate the root inode
//! before Reset Watchdog Timer is sent; a blocked round lets hardware expire.
//! The transport accepts only S19's four application commands.
use core::sync::atomic::{AtomicU16, Ordering};
use x86_64::instructions::port::Port;

static PORT: AtomicU16 = AtomicU16::new(0);
const TIMEOUT_MS: u64 = 5_000;
const PERIOD_MS: u64 = 20_000;
const TIMER: [u8; 6] = [4, 1, 0, 0x10, 0xb0, 4]; // SMS/OS, reset, no pretimeout, 1200 deciseconds

pub fn discover(rsdp: Option<u64>, offset: u64) {
    if let Some(port) = crate::arch_impl::x86_64::acpi::read_kcs_port(rsdp, offset) {
        PORT.store(port, Ordering::Release);
    }
}

struct Kcs { data: Port<u8>, status: Port<u8> }
impl Kcs {
    fn new(port: u16) -> Self { Self { data: Port::new(port), status: Port::new(port + 1) } }
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
        if !matches!((command, data.len()), (1, 0) | (0x22, 0) | (0x24, 6) | (0x25, 0)) {
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
fn wait(ms: u64) {
    let until = now().saturating_add(ms);
    while now() < until { crate::arch_halt_with_interrupts(); }
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
fn run(port: u16) -> Result<(), &'static str> {
    let mut kcs = Kcs::new(port);
    let (_, length) = kcs.request(1, &[])?;
    if length < 14 { return Err("device ID length"); }
    let previous = kcs.get()?;
    crate::serial_println!("[watchdog] previous-expired={}", u8::from(previous[3] & 0x10 != 0));
    health()?;
    kcs.request(0x24, &TIMER)?;
    kcs.request(0x22, &[])?;
    let timer = kcs.get()?;
    if timer[0] & 0x47 != 0x44 || timer[1] != 1 || timer[2] != 0 || timer[4..6] != [0xb0, 4] {
        return Err("watchdog settings readback");
    }
    crate::serial_println!("[watchdog] armed seconds=120 action=hard-reset health=root-io,root-inode");
    #[cfg(feature = "watchdog_fault_inject")]
    fault::start()?;
    let mut rounds = 0u64;
    loop {
        wait(PERIOD_MS);
        health()?;
        kcs.request(0x22, &[])?;
        rounds += 1;
        // The fault cases require this receipt after six feed rounds.
        if rounds % 6 == 0 { crate::serial_println!("[watchdog] healthy feeds={}", rounds); }
        #[cfg(feature = "watchdog_fault_inject")]
        fault::feeder();
    }
}
pub fn start() {
    let port = PORT.load(Ordering::Acquire);
    if port == 0 { return; }
    if crate::task::kthread::kthread_run_on_cpu(move || {
        if let Err(reason) = run(port) {
            // No retry feeding a failed health round. An already armed timer must expire.
            crate::serial_println!("[watchdog] stopped: {}", reason);
        }
    }, "watchdog", 0).is_err() {
        crate::serial_println!("[watchdog] feeder could not start");
    }
}

#[cfg(feature = "watchdog_fault_inject")]
mod fault {
    use super::{now, wait};
    use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
    static MODE: AtomicU8 = AtomicU8::new(0);
    static AT: AtomicU64 = AtomicU64::new(0);
    pub fn start() -> Result<(), &'static str> {
        let guard = crate::fs::ext2::root_fs_read()?;
        let fs = guard.as_ref().ok_or("fault root absent")?;
        let Ok(number) = fs.resolve_path("/etc/breenix/watchdog-fault") else { return Ok(()); };
        let inode = fs.read_inode(number)?;
        if inode.size() > 32 { return Err("fault intent too long"); }
        let bytes = fs.read_file_content_coherent(number, &inode)?;
        let mode = match bytes.as_slice() { b"feeder-hang\n" => 1, b"health-hang\n" => 2, b"" => return Ok(()), _ => return Err("fault intent invalid") };
        MODE.store(mode, Ordering::Release);
        AT.store(now() + 150_000, Ordering::Release);
        if mode == 2 {
            crate::task::kthread::kthread_run(|| {
                wait(150_000);
                // A real filesystem wedge: hold its writer forever. The feeder's
                // next ordinary read health round blocks on this same lock.
                let _guard = crate::fs::ext2::root_fs_write().expect("fault root lock");
                crate::serial_println!("[watchdog-fault] trigger health-hang");
                loop { crate::arch_halt_with_interrupts(); }
            }, "watchdog-fault").map_err(|_| "fault thread could not start")?;
        }
        Ok(())
    }
    pub fn feeder() {
        if MODE.load(Ordering::Acquire) == 1 && now() >= AT.load(Ordering::Acquire) {
            crate::serial_println!("[watchdog-fault] trigger feeder-hang");
            x86_64::instructions::interrupts::disable();
            loop { x86_64::instructions::hlt(); }
        }
    }
}
