//! Failure-only delivery evidence for the AHCI MSI vector.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use x86_64::structures::idt::InterruptStackFrame;

use crate::arch_impl::x86_64::{apic, msi, nmi_report, smp};
use crate::log_notice::Record;
use crate::task::scheduler::MAX_CPUS;

use super::{TimeoutRegisters, AHCI_IRQ, AHCI_IRQ_EDGE, MAX_PORTS};

#[path = "../../task/cpu_sample.rs"]
mod cpu_sample;
mod receipt;

use cpu_sample::Sample;
use receipt::{Receipt, SampleSource, Snapshot};

static ENTRIES: AtomicU32 = AtomicU32::new(0);
static NOTICES: [Record; MAX_PORTS] = [const { Record::new() }; MAX_PORTS];
static NEXT_SAMPLE: AtomicU32 = AtomicU32::new(0);
static REQUESTS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static REPLIES: [Sample<15>; MAX_CPUS] = [const { Sample::new() }; MAX_CPUS];

pub(super) fn interrupt_entry() {
    ENTRIES.fetch_add(1, Ordering::Relaxed);
    super::handle_interrupt();
}

pub(super) struct Request {
    vector: u8,
    destination: u32,
    entries: u32,
    sample: Option<PendingSample>,
}

struct PendingSample {
    cpu: usize,
    key: u64,
    source: SampleSource,
}

/// Start the sample before the timeout path acknowledges the port. A reply
/// is asynchronous, so its LAPIC read can straddle the port snapshot.
pub(super) fn request(token: u32) -> Option<Request> {
    let irq = AHCI_IRQ.load(Ordering::Acquire);
    if irq == 0 || !AHCI_IRQ_EDGE.load(Ordering::Relaxed) {
        return None;
    }
    let vector = irq as u8;
    let destination = msi::destination()?;
    let entries = ENTRIES.load(Ordering::Relaxed);
    let sample = crate::arch_without_interrupts(|| {
        let cpu = smp::cpu_of_apic_id(destination)?;
        // A self-NMI describes the sampling CPU at the point of capture.
        let source = if apic::id() == destination {
            SampleSource::Local
        } else {
            SampleSource::Remote
        };
        let key = (u64::from(NEXT_SAMPLE.fetch_add(1, Ordering::Relaxed)) << 32) | u64::from(token);
        let mut posted = false;
        crate::memory::tlb::request_ahci_lapic_sample(cpu, || {
            posted = REQUESTS[cpu]
                .compare_exchange(0, key, Ordering::Release, Ordering::Relaxed)
                .is_ok();
            posted
        });
        posted.then_some(PendingSample { cpu, key, source })
    });
    Some(Request {
        vector,
        destination,
        entries,
        sample,
    })
}

/// Runs on the destination CPU even with maskable IRQs disabled. The
/// occupied request serializes this CPU's reply writer until publication.
pub(crate) fn record_lapic_sample(cpu: usize, frame: &InterruptStackFrame) {
    let key = REQUESTS[cpu].load(Ordering::Acquire);
    if key == 0 {
        return;
    }
    let priority = apic::priority_snapshot();
    let context = nmi_report::interrupted_context(cpu, frame);
    let [isr0, isr1, isr2, isr3] = priority.isr;
    let [irr0, irr1, irr2, irr3] = priority.irr;
    REPLIES[cpu].store([
        key,
        context.rip,
        u64::from(context.interrupts_enabled),
        u64::from(priority.cr8),
        u64::from(priority.ppr),
        context.tid,
        u64::from(context.preempt_count),
        isr0,
        isr1,
        isr2,
        isr3,
        irr0,
        irr1,
        irr2,
        irr3,
    ]);
    REQUESTS[cpu].store(0, Ordering::Release);
}

impl PendingSample {
    fn read(self) -> Option<(SampleSource, Snapshot)> {
        let (start, freq) = super::read_cntpct_and_freq();
        // Wait for at most 1 ms, only to report a timed-out command.
        // The driver has already made its decision from the registers.
        loop {
            if let Some(
                [key, rip, interrupts, cr8, ppr, tid, preempt, isr0, isr1, isr2, isr3, irr0, irr1, irr2, irr3],
            ) = REPLIES[self.cpu].load()
            {
                if key == self.key {
                    return Some((
                        self.source,
                        Snapshot {
                            rip,
                            interrupts_enabled: interrupts != 0,
                            cr8: cr8 as u8,
                            ppr: ppr as u8,
                            tid,
                            preempt_count: preempt as u32,
                            isr: [isr0, isr1, isr2, isr3],
                            irr: [irr0, irr1, irr2, irr3],
                        },
                    ));
                }
            }
            if super::read_cntpct().wrapping_sub(start) >= freq / 1000 {
                return None;
            }
            core::hint::spin_loop();
        }
    }
}

pub(super) fn report(port: usize, token: u32, request: Option<Request>, regs: TimeoutRegisters) {
    if let Some(Request {
        vector,
        destination,
        entries,
        sample,
    }) = request
    {
        let receipt = Receipt {
            port,
            token,
            vector,
            destination,
            entries,
            hba_is: regs.hba_is,
            port_is: regs.port_is,
            sample: sample.and_then(PendingSample::read),
        };
        NOTICES[port].post_again(format_args!("{receipt}"));
    }
}
