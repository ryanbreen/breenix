//! Interrupted CPU state and the report for an external NMI.
//!
//! The kernel sends NMIs for TLB shootdowns and suite CPU snapshots. Both use
//! `memory::tlb`'s flight flag and are claimed by `shootdown_nmi_handler`. An unclaimed NMI came from outside: a
//! server's BMC (its NMI button, or a Redfish or IPMI NMI, which the metal
//! driver sends to a hung boot before powering it off), a hardware error the
//! chipset signals as NMI, or a watchdog. This report says where the CPU was
//! when it arrived, so a hung boot's last record names the code it was stuck
//! in.
//!
//! It runs in NMI context, possibly interrupting any code, including the code
//! holding the serial lock. So it takes no lock, allocates no memory, uses no
//! `core::fmt`, and writes straight to the log UART's transmit register with a
//! bounded wait per byte. CPUs that take the same broadcast NMI report one at a
//! time, each waiting a bounded time for the one before. At most
//! `MAX_REPORTS` are written, so a stream of NMIs cannot flood the console.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use x86_64::instructions::port::Port;
use x86_64::{registers::rflags::RFlags, structures::idt::InterruptStackFrame};

pub(crate) struct InterruptedContext {
    pub rip: u64,
    pub interrupts_enabled: bool,
    pub tid: u64,
    pub preempt_count: u32,
}

/// Read the original frame and local scheduler state during a diagnostic NMI.
/// The caller identifies the interrupted CPU from its APIC id.
pub(crate) fn interrupted_context(cpu: usize, frame: &InterruptStackFrame) -> InterruptedContext {
    let data = crate::per_cpu::cpu_data(cpu);
    // SAFETY: the current thread and preempt count are owned by this CPU.
    // Its thread cannot switch out during the NMI. Use the local slot without
    // GS, which may still hold the user base in an entry or return stub.
    let (tid, preempt_count) = unsafe {
        let current = (&raw const (*data).current_thread).read_volatile();
        let tid = if current.is_null() {
            0
        } else {
            (&raw const (*current).id).read_volatile()
        };
        (tid, (&raw const (*data).preempt_count).read_volatile())
    };
    InterruptedContext {
        rip: frame.instruction_pointer.as_u64(),
        interrupts_enabled: frame.cpu_flags.contains(RFlags::INTERRUPT_FLAG),
        tid,
        preempt_count,
    }
}

const MAX_REPORTS: u64 = 16;

static REPORTS: AtomicU64 = AtomicU64::new(0);
static REPORTING: AtomicBool = AtomicBool::new(false);

/// A fixed buffer one report is composed in.
struct Report {
    bytes: [u8; 192],
    len: usize,
}

impl Report {
    fn text(&mut self, text: &[u8]) {
        for &byte in text {
            if self.len < self.bytes.len() {
                self.bytes[self.len] = byte;
                self.len += 1;
            }
        }
    }

    fn hex(&mut self, value: u64) {
        self.text(b"0x");
        let digits = (64 - value.leading_zeros()).div_ceil(4).max(1);
        for shift in (0..digits).rev() {
            let nibble = ((value >> (shift * 4)) & 0xf) as u8;
            self.text(&[if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 }]);
        }
    }

    fn decimal(&mut self, value: u64) {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        let mut rest = value;
        loop {
            at -= 1;
            digits[at] = b'0' + (rest % 10) as u8;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        self.text(&digits[at..]);
    }
}

/// Write `bytes` to the log UART, waiting at most about 1 ms (at 115200 baud a
/// byte takes 87 us) for room before each byte.
fn write_raw(bytes: &[u8]) {
    let port = crate::serial::log_port();
    let budget = super::timer::frequency_hz() / 1000;
    // SAFETY: the log UART's line status and transmit registers; writing the
    // transmit register while another writer's bytes are in flight can only
    // interleave output, which is the cost this path accepts.
    unsafe {
        let mut status = Port::<u8>::new(port + 5);
        let mut data = Port::<u8>::new(port);
        for &byte in bytes {
            let deadline = super::timer::rdtsc() + budget;
            while status.read() & 0x20 == 0 && super::timer::rdtsc() < deadline {
                core::hint::spin_loop();
            }
            data.write(byte);
        }
    }
}

/// Report an NMI that no shootdown request claimed, on CPU `cpu` (no value
/// when it could not be told from its APIC id).
pub fn unclaimed(frame: &InterruptStackFrame, cpu: Option<usize>, apic_id: Option<u32>) {
    let number = REPORTS.fetch_add(1, Ordering::Relaxed) + 1;
    if number > MAX_REPORTS {
        return;
    }
    let deadline = super::timer::rdtsc() + super::timer::frequency_hz() / 10;
    while REPORTING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        if super::timer::rdtsc() >= deadline {
            return;
        }
        core::hint::spin_loop();
    }
    // SAFETY: port 0x61's NMI status bits (7: system error, 6: I/O channel
    // check) are read-only status on reads.
    let status = unsafe { Port::<u8>::new(0x61).read() };
    let mut report = Report { bytes: [0; 192], len: 0 };
    report.text(b"\r\n[nmi] external NMI #");
    report.decimal(number);
    report.text(b" cpu=");
    match cpu {
        Some(cpu) => report.decimal(cpu as u64),
        None => report.text(b"?"),
    }
    report.text(b" apic_id=");
    match apic_id {
        Some(id) => report.decimal(u64::from(id)),
        None => report.text(b"?"),
    }
    report.text(b" rip=");
    report.hex(frame.instruction_pointer.as_u64());
    report.text(b" cs=");
    report.hex(frame.code_segment.0.into());
    report.text(b" rflags=");
    report.hex(frame.cpu_flags.bits());
    report.text(b" rsp=");
    report.hex(frame.stack_pointer.as_u64());
    report.text(b" port61=");
    report.hex(u64::from(status));
    report.text(b"\r\n");
    write_raw(&report.bytes[..report.len]);
    REPORTING.store(false, Ordering::Release);
}
