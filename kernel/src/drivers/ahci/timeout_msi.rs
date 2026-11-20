//! Failure-only delivery evidence for the AHCI MSI vector.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::arch_impl::x86_64::{apic, msi, smp};
use crate::log_notice::Record;
use crate::task::scheduler::MAX_CPUS;

use super::{TimeoutRegisters, AHCI_IRQ, AHCI_IRQ_EDGE, MAX_PORTS};

static ENTRIES: AtomicU32 = AtomicU32::new(0);
static NOTICES: [Record; MAX_PORTS] = [const { Record::new() }; MAX_PORTS];
static REQUESTS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static REPLIES: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

const IRR: u64 = 1 << 8;
const ISR: u64 = 1 << 9;

pub(super) fn interrupt_entry() {
    ENTRIES.fetch_add(1, Ordering::Relaxed);
    super::handle_interrupt();
}

pub(super) struct Request {
    vector: u8,
    destination: u32,
    entries: u32,
    sample: Sample,
}

enum Sample {
    Local(bool, bool),
    Remote { cpu: usize, key: u64 },
    Unavailable,
}

/// Start the sample before the timeout path acknowledges the port. A remote
/// reply is asynchronous, so its LAPIC read can straddle the port snapshot.
pub(super) fn request(token: u32) -> Option<Request> {
    let irq = AHCI_IRQ.load(Ordering::Acquire);
    if irq == 0 || !AHCI_IRQ_EDGE.load(Ordering::Relaxed) {
        return None;
    }
    let vector = irq as u8;
    let destination = msi::destination()?;
    let entries = ENTRIES.load(Ordering::Relaxed);
    let sample = crate::arch_without_interrupts(|| {
        if apic::id() == destination {
            let (irr, isr) = apic::vector_pending_state(vector);
            return Sample::Local(irr, isr);
        }
        let Some(cpu) = smp::cpu_of_apic_id(destination) else {
            return Sample::Unavailable;
        };
        let key = (u64::from(token) << 16) | u64::from(vector);
        let mut posted = false;
        crate::memory::tlb::request_ahci_lapic_sample(cpu, || {
            posted = REQUESTS[cpu].compare_exchange(
                0, key, Ordering::Release, Ordering::Relaxed,
            ).is_ok();
            posted
        });
        if posted { Sample::Remote { cpu, key } } else { Sample::Unavailable }
    });
    Some(Request { vector, destination, entries, sample })
}

/// Runs on the destination CPU, including when its maskable IRQs are disabled.
/// Read only local registers and publish atomics. NMI entry may precede SWAPGS.
pub(crate) fn record_lapic_sample(cpu: usize) {
    let key = REQUESTS[cpu].load(Ordering::Acquire);
    if key != 0 {
        let (irr, isr) = apic::vector_pending_state(key as u8);
        let flags = (if irr { IRR } else { 0 }) | (if isr { ISR } else { 0 });
        REPLIES[cpu].store(key | flags, Ordering::Release);
        REQUESTS[cpu].store(0, Ordering::Release);
    }
}

impl Sample {
    fn read(self) -> Option<(bool, bool)> {
        match self {
            Self::Local(irr, isr) => Some((irr, isr)),
            Self::Unavailable => None,
            Self::Remote { cpu, key } => {
                let (start, freq) = super::read_cntpct_and_freq();
                // Wait for at most 1 ms, only to report a timed-out command.
                // The driver has already made its decision from the registers.
                loop {
                    let reply = REPLIES[cpu].load(Ordering::Acquire);
                    if reply & !(IRR | ISR) == key {
                        return Some((reply & IRR != 0, reply & ISR != 0));
                    }
                    if super::read_cntpct().wrapping_sub(start) >= freq / 1000 {
                        return None;
                    }
                    core::hint::spin_loop();
                }
            }
        }
    }
}

pub(super) fn report(port: usize, token: u32, request: Option<Request>, regs: TimeoutRegisters) {
    if let Some(Request { vector, destination, entries, sample }) = request {
        let (irr, isr) = match sample.read() {
            Some((irr, isr)) => (if irr { "1" } else { "0" }, if isr { "1" } else { "0" }),
            None => ("unavailable", "unavailable"),
        };
        NOTICES[port].post_again(format_args!(
            "AHCI_MSI_TIMEOUT port={port} token={token} vector={vector:#x} destination={destination} entries={entries} IRR={irr} ISR={isr} HBA_IS={:#x} IS={:#x}",
            regs.hba_is, regs.port_is,
        ));
    }
}
