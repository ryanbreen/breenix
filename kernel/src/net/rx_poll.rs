//! The e1000's maintenance thread: receive polling on ARM64 (VMware), and the
//! DHCP client on x86-64 where the e1000 is the interface and takes DHCP.
//!
//! ARM64: VMware's ARM64 platform has no GICv2m frame, and Breenix drives
//! neither a GICv3 ITS nor legacy INTx routing, so the e1000 there raises no
//! interrupt; AHCI and xHCI poll on VMware for the same reason. Without one, a
//! frame from outside the VM sat in the receive ring until an unrelated NetRx
//! raise (a loopback send, say) drained it, and a DNS reply was read only after
//! the resolver had given up. ke1000rxd checks the ring once per timer tick and
//! raises NetRx when a frame is waiting, so delivery takes the softirq path an
//! interrupt-driven NIC takes.
//!
//! x86-64: the e1000 interrupts, by MSI or INTx, so its frames need no poller.
//! Without fw_cfg the stack takes its address from DHCP (`net::uses_dhcp`),
//! whose client runs from a thread: the BCM5720 runs it on its own worker,
//! and for an e1000 ke1000dhcp calls `dhcp::maintain`, then sleeps
//! `DHCP_INTERVAL_MS`; the lease state machine keeps its own retry, renew and
//! rebind deadlines.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::drivers::e1000;
use crate::task::kthread;
#[cfg(target_arch = "aarch64")]
use crate::task::scheduler;
#[cfg(target_arch = "aarch64")]
use crate::task::softirqd::{raise_softirq, SoftirqType};

static RX_POLL_TID: AtomicU64 = AtomicU64::new(0);

/// How often the x86 worker gives the DHCP client a turn.
#[cfg(target_arch = "x86_64")]
const DHCP_INTERVAL_MS: u64 = 100;

/// Start the thread when the e1000 is the interface. ARM64 needs it for
/// receive, since the e1000 there takes no interrupt; x86-64 needs it only
/// when the e1000 takes DHCP and no BCM5720, whose worker owns the one DHCP
/// client, is attached.
pub fn init_rx_poll() {
    if !super::active_tx_driver_is_e1000() || RX_POLL_TID.load(Ordering::Acquire) != 0 {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    let (worker, name): (fn(), &str) = {
        if !super::uses_dhcp() || crate::drivers::bcm5720::is_initialized() {
            return;
        }
        (dhcp_worker_fn, "ke1000dhcp")
    };
    #[cfg(target_arch = "aarch64")]
    let (worker, name): (fn(), &str) = (rx_poll_fn, "ke1000rxd");
    match kthread::kthread_run(worker, name) {
        Ok(handle) => RX_POLL_TID.store(handle.tid(), Ordering::Release),
        Err(error) => crate::serial_println!("[net] failed to start {}: {:?}", name, error),
    }
}

#[cfg(target_arch = "x86_64")]
fn dhcp_worker_fn() {
    loop {
        if let Some(mac) = super::get_mac_address() {
            super::dhcp::maintain(mac, e1000::link_up());
        }
        crate::drivers::bcm5720::sleep_ms(DHCP_INTERVAL_MS);
    }
}

#[cfg(target_arch = "aarch64")]
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
#[cfg(target_arch = "aarch64")]
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
    scheduler::with_current(|sched| sched.block_current_for_timer(wake_at));
    scheduler::yield_current();

    loop {
        crate::arch_halt_with_interrupts();
        if scheduler::thread_state(tid) != Some(ThreadState::BlockedOnTimer) {
            break;
        }
    }
}
