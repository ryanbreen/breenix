// Compiled by ahci_completion_timeout_test.py with the actual driver bodies.
#![allow(dead_code)]

use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

const MAX_PORTS: usize = 2;
const AHCI_MAX_CONCURRENT: u32 = 1;
const FIRST: u32 = 51849;

struct Device {
    port_is: [u32; MAX_PORTS],
    ci: [u32; MAX_PORTS],
    tfd: [u32; MAX_PORTS],
    cmd: u32,
    prdbc: u32,
    stop_writes: usize,
    force_timeout: bool,
    interrupts_enabled: bool,
    reset_started: Option<u64>,
    reset_finished: bool,
    counter: u64,
    hba_is: u32,
    irr: bool,
    isr: bool,
    step: usize,
    events: Vec<(usize, usize, HardwareStep)>,
    receipts: Vec<(usize, u32)>,
    issued_after_receipt: bool,
    timeouts: usize,
    recovered: usize,
    notices: Vec<String>,
}

#[derive(Clone, Copy)]
enum HardwareStep {
    ClearCi,
    PortStatus,
    GlobalStatus,
}

impl Device {
    fn set_global(&mut self, new: u32) {
        // AHCI 1.3.1, 10.7.2.1: a change to a nonzero IS sends a
        // message. SDM vol. 3A, 12.8.4: messages can share one IRR bit.
        if self.hba_is != new && new != 0 {
            self.irr = true;
        }
        self.hba_is = new;
    }

    fn checkpoint(&mut self) {
        self.step += 1;
        while self.events.first().is_some_and(|event| event.0 == self.step) {
            let (_, port, event) = self.events.remove(0);
            match event {
                HardwareStep::ClearCi => self.ci[port] = 0,
                HardwareStep::PortStatus => self.port_is[port] |= PORT_IRQ_D2H_REG_FIS,
                HardwareStep::GlobalStatus => self.set_global(self.hba_is | (1 << port)),
            }
        }
    }
}

thread_local! {
    static MODEL: RefCell<Device> = const { RefCell::new(Device {
        port_is: [0; MAX_PORTS], ci: [1, 0], tfd: [0x50; MAX_PORTS],
        cmd: PORT_CMD_ST | PORT_CMD_CR, prdbc: 512, stop_writes: 0,
        force_timeout: false, interrupts_enabled: false, reset_started: None,
        reset_finished: false, counter: 0,
        hba_is: 0, irr: false, isr: false, step: 0, events: Vec::new(),
        receipts: Vec::new(), issued_after_receipt: false, timeouts: 0, recovered: 0,
        notices: Vec::new(),
    }) };
}

// Serial test execution is enforced by the harness invocation.
static AHCI_ABAR: AtomicU64 = AtomicU64::new(1);
static AHCI_IRQ: AtomicU32 = AtomicU32::new(0x50);
static AHCI_PORTS_IMPLEMENTED: AtomicU32 = AtomicU32::new(3);
static AHCI_ISR_COUNT: AtomicU32 = AtomicU32::new(0);
static AHCI_ISR_PORT_HIT: [AtomicU32; MAX_PORTS] = atomics();
static AHCI_ISR_STALE_ACK: [AtomicU32; MAX_PORTS] = atomics();
static AHCI_ISR_COMPLETE_HIT: [AtomicU32; MAX_PORTS] = atomics();
static AHCI_LAST_ISR_PORT1_IS: AtomicU32 = AtomicU32::new(0);
static AHCI_LAST_ISR_PORT1_CMD_NUM: AtomicU32 = AtomicU32::new(0);
static AHCI_LAST_ISR_COMPLETE_PORT: AtomicU32 = AtomicU32::new(0);
static AHCI_LAST_ISR_COMPLETE_CMD: AtomicU32 = AtomicU32::new(0);
static AHCI_LAST_ISR_COMPLETE_WAITER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static AHCI_POLLED_POST_REGISTRATION_COUNT: AtomicU32 = AtomicU32::new(0);
static AHCI_POLLED_POST_REG_SCHEDULER_RUNNING: AtomicU32 = AtomicU32::new(0);
static AHCI_POLLED_POST_REG_PRE_SCHEDULER: AtomicU32 = AtomicU32::new(0);
static AHCI_POLLED_COMPLETION_COUNT: AtomicU32 = AtomicU32::new(0);
static PORT_IO_COMPLETED: [AtomicU64; MAX_PORTS] = [const { AtomicU64::new(0) }; MAX_PORTS];
static PORT_ACTIVE_MASK: [AtomicU32; MAX_PORTS] = atomics();
static PORT_ACTIVE_CMD_NUM: [AtomicU32; MAX_PORTS] = atomics();
static PORT_ACKED_ERRORS: [AtomicU32; MAX_PORTS] = atomics();
static PORT_ACKED_TFD: [AtomicU32; MAX_PORTS] = atomics();
static PORT_RECOVERY: [AtomicU8; MAX_PORTS] = [const { AtomicU8::new(PORT_READY) }; MAX_PORTS];
static PORT_POLL: [Lock; MAX_PORTS] = [const { Lock(Mutex::new(())) }; MAX_PORTS];
static PORT_IO_LOCK: [Lock; MAX_PORTS] = [const { Lock(Mutex::new(())) }; MAX_PORTS];
static LOST_NOTIFICATIONS: [Notice; MAX_PORTS] = [const { Notice }; MAX_PORTS];

struct Notice;
impl Notice {
    fn post_again(&self, args: std::fmt::Arguments<'_>) -> bool {
        MODEL.with_borrow_mut(|device| device.notices.push(args.to_string()));
        true
    }
}

const fn atomics() -> [AtomicU32; MAX_PORTS] {
    [const { AtomicU32::new(0) }; MAX_PORTS]
}

struct Lock(Mutex<()>);
impl Lock {
    fn lock(&self) -> MutexGuard<'_, ()> { self.0.lock().unwrap() }
    fn try_lock(&self) -> Option<MutexGuard<'_, ()>> { self.0.try_lock().ok() }
}

struct Completion {
    port: usize,
    done: AtomicU32,
}
static AHCI_COMPLETIONS: [[Completion; 1]; MAX_PORTS] =
    [[Completion::new(0)], [Completion::new(1)]];

impl Completion {
    const fn new(port: usize) -> Self {
        Self { port, done: AtomicU32::new(0) }
    }

    fn waiter_tid(&self) -> u64 { 21 }

    fn wait_timeout_uninterruptible_observed(
        &self, _: u32, _: u64, _: impl FnMut(WaitStage),
    ) -> Result<bool, ()> {
        Ok(self.done.load(Ordering::Acquire) != 0 && MODEL.with_borrow(|device| !device.force_timeout))
    }

    fn complete(&self, token: u32) {
        self.done.store(token, Ordering::Release);
        MODEL.with_borrow_mut(|device| {
            assert_eq!(device.port_is[self.port] & PORT_IRQ_D2H_REG_FIS, 0);
            device.receipts.push((self.port, token));
            if device.issued_after_receipt && token == FIRST {
                // A successor can issue and finish before the prior LAPIC EOI
                // once the waiter receives its token after the ISR's last MMIO.
                assert!(device.isr);
                PORT_ACTIVE_MASK[0].store(1, Ordering::Release);
                PORT_ACTIVE_CMD_NUM[0].store(FIRST + 1, Ordering::Release);
                device.port_is[0] = PORT_IRQ_D2H_REG_FIS;
                device.set_global(device.hba_is | 1);
                assert!(device.irr);
            }
        });
    }
}

struct CmdToken {
    port: usize,
    cmd_num: u32,
    dma_index: usize,
    expected_bytes: u32,
    has_irq: bool,
    scheduler_running: bool,
    polled: bool,
}
enum WaitStage {}
enum EventKind { IsrEntry, ReceiptDeferred, ReceiptRecorded, Timeout }

fn trace_command_event(_: usize, _: u32, _: u8, kind: EventKind, _: u32, _: u32, _: u32, _: u32) {
    if matches!(kind, EventKind::Timeout) {
        MODEL.with_borrow_mut(|device| device.recovered += 1);
    }
}
fn trace_retirement(_: usize, _: u32, _: u8, _: u32, _: u32, _: u32, _: bool) {}
fn trace_wait_event(_: usize, _: u32, _: WaitStage) {}
fn scheduler_sleep_ready(_: bool) -> bool { true }
fn kick_poller() {}
fn read_cntpct() -> u64 {
    MODEL.with_borrow_mut(|device| {
        assert!(device.interrupts_enabled, "recovery waited with interrupts masked");
        device.counter += 1;
        device.counter
    })
}
fn read_cntpct_and_freq() -> (u64, u64) { (read_cntpct(), 1000) }
fn arch_interrupts_enabled() -> bool { MODEL.with_borrow(|device| device.interrupts_enabled) }
fn read_prdbc(_: usize) -> Result<u32, &'static str> {
    Ok(MODEL.with_borrow(|device| device.prdbc))
}
fn hba_read(_: u64, register: usize) -> u32 {
    assert_eq!(register, HBA_IS);
    MODEL.with_borrow_mut(|device| {
        device.checkpoint();
        device.hba_is
    })
}

fn hba_write(_: u64, register: usize, mask: u32) {
    assert_eq!(register, HBA_IS);
    MODEL.with_borrow_mut(|device| {
        device.checkpoint();
        device.set_global(device.hba_is & !mask);
        let wires = device.port_is.iter().enumerate().fold(0, |bits, (port, &status)| {
            // Both D2H and descriptor status are enabled on this modeled HBA.
            bits | (u32::from(status & (PORT_IRQ_D2H_REG_FIS | PORT_IRQ_SG_DONE) != 0) << port)
        });
        device.set_global(device.hba_is | wires);
    });
}

fn port_read(_: u64, port: usize, register: usize) -> u32 {
    MODEL.with_borrow_mut(|device| {
        device.checkpoint();
        match register {
            PORT_IS => device.port_is[port],
            PORT_CI => device.ci[port],
            PORT_TFD => device.tfd[port],
            PORT_CMD => device.cmd,
            PORT_SACT | PORT_SERR => 0,
            PORT_SCTL => u32::from(device.reset_started.is_some() && !device.reset_finished),
            PORT_SSTS => 3,
            _ => panic!("unexpected port register {register:#x}"),
        }
    })
}

fn port_write(_: u64, port: usize, register: usize, mask: u32) {
    MODEL.with_borrow_mut(|device| {
        device.checkpoint();
        match register {
            PORT_IS => device.port_is[port] &= !mask,
            PORT_CMD => {
                if device.cmd & PORT_CMD_ST != 0 && mask & PORT_CMD_ST == 0 {
                    device.stop_writes += 1;
                }
                device.cmd = mask;
            }
            PORT_IE | PORT_SERR => (),
            PORT_SCTL if mask & 0xf == 1 => device.reset_started = Some(device.counter),
            PORT_SCTL => {
                assert!(device.counter > device.reset_started.unwrap());
                device.reset_finished = true;
            }
            _ => panic!("unexpected port register {register:#x}"),
        }
    });
}

include!("driver.rs");

mod timeout_msi {
    pub fn request(_: u32) {}
    pub fn report<T>(_: usize, _: u32, _: (), _: T) {}
}

fn reset() {
    MODEL.with_borrow_mut(|device| {
        *device = Device {
            port_is: [0; MAX_PORTS], ci: [1, 0], tfd: [0x50; MAX_PORTS],
            cmd: PORT_CMD_ST | PORT_CMD_CR, prdbc: 512, stop_writes: 0,
        force_timeout: false, interrupts_enabled: false, reset_started: None,
        reset_finished: false, counter: 0,
        hba_is: 0, irr: false, isr: false, step: 0, events: Vec::new(),
            receipts: Vec::new(), issued_after_receipt: false, timeouts: 0, recovered: 0,
            notices: Vec::new(),
        };
    });
    for port in 0..MAX_PORTS {
        PORT_ACTIVE_CMD_NUM[port].store(0, Ordering::Relaxed);
        PORT_ACTIVE_MASK[port].store(0, Ordering::Relaxed);
        PORT_ACKED_ERRORS[port].store(0, Ordering::Relaxed);
        PORT_ACKED_TFD[port].store(0, Ordering::Relaxed);
        PORT_RECOVERY[port].store(PORT_READY, Ordering::Relaxed);
        AHCI_COMPLETIONS[port][0].done.store(0, Ordering::Relaxed);
    }
    PORT_ACTIVE_CMD_NUM[0].store(FIRST, Ordering::Relaxed);
    PORT_ACTIVE_MASK[0].store(1, Ordering::Relaxed);
}

fn drain_vectors() {
    for _ in 0..16 {
        let deliver = MODEL.with_borrow_mut(|device| {
            device.checkpoint();
            if device.irr {
                device.irr = false;
                device.isr = true;
            }
            device.isr
        });
        if deliver {
            handle_interrupt();
            // LAPIC EOI lowers ISR. Any fixed interrupt in IRR remains pending.
            MODEL.with_borrow_mut(|device| device.isr = false);
        }
    }
}

#[test]
fn single_msi_acknowledgement_interleavings() {
    // Vary completion across the read/filter/drain, between both W1Cs, and
    // after return. Stale pulses can reach the early filter with IS already 0.
    for (old_status, old_global) in [(0, 0), (0, 1), (PORT_IRQ_SG_DONE, 1), (0, 2)] {
        for at in 1..=24 {
            for gap in 0..=3 {
                reset();
                MODEL.with_borrow_mut(|device| {
                    device.port_is[0] = old_status;
                    device.hba_is = old_global;
                    device.irr = true;
                    device.events = vec![
                        (at, 0, HardwareStep::ClearCi),
                        (at + gap, 0, HardwareStep::PortStatus),
                        (at + 2 * gap, 0, HardwareStep::GlobalStatus),
                    ];
                });
                // Drive remaining asynchronous hardware steps even when the CPU
                // temporarily has no pending interrupt to handle.
                for _ in 0..3 {
                    drain_vectors();
                }
                MODEL.with_borrow(|device| {
                    assert!(device.events.is_empty());
                    assert_eq!(
                        (device.port_is[0], device.hba_is, device.irr, &device.receipts[..]),
                        (0, 0, false, &[(0, FIRST)][..]),
                        "starting PxIS={old_status:#x} HBA.IS={old_global:#x}, at={at}, gap={gap}",
                    );
                });
            }
        }
    }
}

#[test]
fn same_vector_can_be_pending_before_previous_eoi() {
    reset();
    MODEL.with_borrow_mut(|device| {
        device.ci[0] = 0;
        device.port_is[0] = PORT_IRQ_D2H_REG_FIS;
        device.set_global(1);
        device.issued_after_receipt = true;
    });
    drain_vectors();
    MODEL.with_borrow(|device| assert_eq!(device.receipts, [(0, FIRST), (0, FIRST + 1)]));
}

fn wait_for_first() -> Result<(), &'static str> {
    wait_cmd_slot0(CmdToken {
        port: 0, cmd_num: FIRST, dma_index: 0, expected_bytes: 512,
        has_irq: true, scheduler_running: true, polled: false,
    })
}

#[test]
fn timeout_retires_the_completed_token_and_records_the_loss() {
    reset();
    MODEL.with_borrow_mut(|device| {
        device.ci[0] = 0;
        device.port_is[0] = PORT_IRQ_D2H_REG_FIS | PORT_IRQ_SG_DONE;
        device.hba_is = 1;
    });
    assert_eq!(wait_for_first(), Ok(()));
    MODEL.with_borrow(|device| {
        assert_eq!(device.receipts, [(0, FIRST)]);
        assert_eq!(device.recovered, 1);
        assert_eq!(device.notices, [
            "AHCI_LOST_NOTIFICATION port=0 token=51849 CI=0x0 IS=0x21 TFD=0x50 HBA_IS=0x1",
        ]);
        assert_eq!((device.port_is[0], device.hba_is), (0, 0));
    });
    assert_eq!(PORT_ACTIVE_CMD_NUM[0].load(Ordering::Relaxed), 0);
    assert_eq!(PORT_ACTIVE_MASK[0].load(Ordering::Relaxed), 0);
}

#[test]
fn timeout_rejects_active_ci_or_error_even_if_the_isr_already_acked_it() {
    for (ci, status, tfd, ack) in [
        (1, PORT_IRQ_D2H_REG_FIS, 0x50, false),
        (0, PORT_IRQ_TF_ERR, 0x50, false),
        (0, PORT_IRQ_HBUS_DATA_ERR, 0x50, true),
        (0, 0, 0x51, false),
        (0, 0, 0x70, false),
    ] {
        reset();
        MODEL.with_borrow_mut(|device| {
            device.ci[0] = ci;
            device.port_is[0] = status;
            device.tfd[0] = tfd;
            device.hba_is = 1;
            device.irr = ack;
            device.force_timeout = true;
        });
        if ack {
            drain_vectors();
        }
        assert!(wait_for_first().is_err(), "CI={ci:#x} PxIS={status:#x} TFD={tfd:#x} ack={ack}");
        MODEL.with_borrow(|device| assert_eq!(device.recovered, 0));
    }
}

#[test]
fn interrupt_error_is_preserved_after_status_acknowledgement() {
    for (status, task_file) in [
        (PORT_IRQ_TF_ERR, 0x50),
        (PORT_IRQ_HBUS_ERR, 0x50),
        (PORT_IRQ_HBUS_DATA_ERR, 0x50),
        (PORT_IRQ_IF_ERR, 0x50),
        (PORT_IRQ_D2H_REG_FIS, 0x0451),
    ] {
        reset();
        MODEL.with_borrow_mut(|device| {
            device.ci[0] = 0;
            device.port_is[0] = status;
            device.tfd[0] = task_file;
            device.set_global(1);
        });
        drain_vectors();
        MODEL.with_borrow_mut(|device| {
            assert_eq!(device.port_is[0], 0);
            device.tfd[0] = 0x50;
        });
        assert!(wait_for_first().is_err(), "PxIS={status:#x} PxTFD={task_file:#x}");
    }
}

#[test]
fn short_transfer_fails_before_the_waiter_can_copy() {
    reset();
    MODEL.with_borrow_mut(|device| {
        device.ci[0] = 0;
        device.port_is[0] = PORT_IRQ_D2H_REG_FIS;
        device.prdbc = 256;
        device.set_global(1);
    });
    drain_vectors();
    assert!(wait_for_first().is_err());
}

#[test]
fn active_timeout_stops_the_engine_and_blocks_dma_reuse() {
    reset();
    assert!(wait_for_first().is_err());
    MODEL.with_borrow(|device| assert_eq!(device.stop_writes, 1));
    assert!(ensure_port_ready(1, 0).is_err(), "CI and CR still set");
    MODEL.with_borrow_mut(|device| device.cmd &= !PORT_CMD_CR);
    assert!(ensure_port_ready(1, 0).is_err(), "CI still set");
    MODEL.with_borrow_mut(|device| {
        device.ci[0] = 0;
        device.cmd |= PORT_CMD_CR;
    });
    assert!(ensure_port_ready(1, 0).is_err(), "CR still set");
    MODEL.with_borrow_mut(|device| {
        device.cmd &= !PORT_CMD_CR;
        device.interrupts_enabled = true;
    });
    assert_eq!(ensure_port_ready(1, 0), Ok(()));
    MODEL.with_borrow(|device| {
        assert!(device.reset_finished);
        assert_ne!(device.cmd & PORT_CMD_ST, 0);
    });
}
