//! Receive polling for the ARM64 e1000 (VMware).
//!
//! VMware's ARM64 platform has no GICv2m frame, and Breenix drives neither a
//! GICv3 ITS nor legacy INTx routing, so the e1000 there raises no interrupt;
//! AHCI and xHCI poll on VMware for the same reason. Without one, a frame from
//! outside the VM sat in the receive ring until an unrelated NetRx raise (a
//! loopback send, say) drained it, and a DNS reply was read only after the
//! resolver had given up. ke1000rxd checks the ring once per timer tick and
//! raises NetRx when a frame is waiting, so delivery takes the softirq path an
//! interrupt-driven NIC takes.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::drivers::e1000;
use crate::task::softirqd::{raise_softirq, SoftirqType};
use crate::task::{kthread, scheduler};

static RX_POLL_TID: AtomicU64 = AtomicU64::new(0);

/// Start the poller when the e1000 is the interface (VMware); other ARM64
/// network devices interrupt.
pub fn init_rx_poll() {
    if !super::active_tx_driver_is_e1000() || RX_POLL_TID.load(Ordering::Acquire) != 0 {
        return;
    }
    match kthread::kthread_run(rx_poll_fn, "ke1000rxd") {
        Ok(handle) => RX_POLL_TID.store(handle.tid(), Ordering::Release),
        Err(error) => crate::serial_println!("[net] failed to start ke1000rxd: {:?}", error),
    }
}

fn rx_poll_fn() {
    loop {
        if e1000::can_receive() {
            raise_softirq(SoftirqType::NetRx);
        }
        if let Some(mac) = super::get_mac_address() {
            super::dhcp::maintain(mac, e1000::link_up());
        }
        sleep_one_tick();
    }
}

/// Block on the timer for one tick: the block/yield/halt sequence of
/// `sys_nanosleep` and the dispatch census thread's `sleep_one_interval`.
fn sleep_one_tick() {
    use crate::task::thread::ThreadState;

    let Some(tid) = scheduler::current_thread_id() else {
        crate::arch_halt_with_interrupts();
        return;
    };
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    let wake_at = seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(nanos)
        .saturating_add(crate::time::timer::MS_PER_TICK * 1_000_000);
    scheduler::with_scheduler(|sched| {
        if sched.get_thread(tid).is_some() {
            sched.block_current_for_timer(wake_at);
        }
    });
    scheduler::yield_current();

    loop {
        crate::arch_halt_with_interrupts();
        let still_blocked = scheduler::with_scheduler(|sched| {
            sched
                .get_thread(tid)
                .is_some_and(|thread| thread.state == ThreadState::BlockedOnTimer)
        })
        .unwrap_or(false);
        if !still_blocked {
            break;
        }
    }
}
