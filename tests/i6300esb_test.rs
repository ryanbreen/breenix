//! Register model from Intel 300641-004US, section 16, and QEMU 11.0.3's
//! hw/watchdog/wdt_i6300esb.c. No guest or watchdog timer is started here.
#[path = "../kernel/src/drivers/i6300esb.rs"]
mod i6300esb;
#[path = "../kernel/src/drivers/watchdog_round.rs"]
mod watchdog_round;

use i6300esb::{Bus, Watchdog};
use std::{cell::RefCell, rc::Rc};
use watchdog_round::Transport;

#[derive(Debug, PartialEq)]
enum Write {
    Config16(u16, u16),
    Config8(u16, u8),
    Mmio8(u16, u8),
    Mmio16(u16, u16),
    Mmio32(u16, u32),
}

struct Device {
    config: u16,
    lock: u8,
    preload: [u32; 2],
    unlock: u8,
    expired: bool,
    reloads: usize,
    status_reads: usize,
    writes: Vec<Write>,
}

impl Device {
    fn new(expired: bool) -> Self {
        Self {
            config: 0,
            lock: 0,
            preload: [0xfffff; 2],
            unlock: 0,
            expired,
            reloads: 0,
            status_reads: 0,
            writes: Vec::new(),
        }
    }

    fn protected_write(&mut self) {
        assert_eq!(self.unlock, 2, "every protected write needs 0x80, 0x86");
        self.unlock = 0;
        let n = self.writes.len();
        assert_eq!(
            &self.writes[n - 2..],
            &[Write::Mmio8(0x0c, 0x80), Write::Mmio8(0x0c, 0x86)]
        );
    }

    fn timeout_ns(&self) -> u64 {
        let shift = if self.config & 4 == 0 { 15 } else { 5 };
        // QEMU uses preload * 2^shift * 30 ns, without the datasheet's +1.
        (u64::from(self.preload[0]) + u64::from(self.preload[1])) * (1 << shift) * 30
    }
}

#[derive(Clone)]
struct FakeBus(Rc<RefCell<Device>>);

impl Bus for FakeBus {
    fn config_read16(&mut self, offset: u16) -> u16 {
        assert_eq!(offset, 0x60);
        self.0.borrow().config
    }
    fn config_write16(&mut self, offset: u16, value: u16) {
        assert_eq!(offset, 0x60);
        let mut d = self.0.borrow_mut();
        d.config = value & 0x27;
        d.writes.push(Write::Config16(offset, value));
    }
    fn config_read8(&mut self, offset: u16) -> u8 {
        assert_eq!(offset, 0x68);
        self.0.borrow().lock
    }
    fn config_write8(&mut self, offset: u16, value: u8) {
        assert_eq!(offset, 0x68);
        let mut d = self.0.borrow_mut();
        if d.lock & 1 == 0 {
            d.lock = value & 7;
        }
        d.writes.push(Write::Config8(offset, value));
    }
    fn mmio_read16(&mut self, offset: u16) -> u16 {
        assert_eq!(offset, 0x0c);
        let mut d = self.0.borrow_mut();
        d.status_reads += 1;
        if d.expired {
            0x1200
        } else {
            0
        }
    }
    fn mmio_write8(&mut self, offset: u16, value: u8) {
        assert_eq!(offset, 0x0c);
        let mut d = self.0.borrow_mut();
        d.unlock = match (d.unlock, value) {
            (_, 0x80) => 1,
            (1, 0x86) => 2,
            _ => panic!("invalid unlock sequence"),
        };
        d.writes.push(Write::Mmio8(offset, value));
    }
    fn mmio_write16(&mut self, offset: u16, value: u16) {
        assert_eq!(offset, 0x0c);
        let mut d = self.0.borrow_mut();
        d.protected_write();
        if value & 0x100 != 0 {
            assert_eq!(d.lock & 6, 2, "reload only in enabled watchdog mode");
            d.reloads += 1;
        }
        if value & 0x200 != 0 {
            d.expired = false;
        }
        d.writes.push(Write::Mmio16(offset, value));
    }
    fn mmio_write32(&mut self, offset: u16, value: u32) {
        assert!(offset == 0 || offset == 4);
        assert!(value <= 0xfffff);
        let mut d = self.0.borrow_mut();
        d.protected_write();
        d.preload[usize::from(offset / 4)] = value;
        d.writes.push(Write::Mmio32(offset, value));
    }
}

fn device(expired: bool) -> (Watchdog<FakeBus>, Rc<RefCell<Device>>) {
    let d = Rc::new(RefCell::new(Device::new(expired)));
    (Watchdog::new(FakeBus(d.clone())), d)
}

#[test]
fn health_gated_two_stage_timer_is_enabled_then_locked_and_reloaded() {
    let (mut watchdog, d) = device(false);
    assert_eq!(
        watchdog_round::feed(&mut watchdog, || Err("EIO")),
        Err("EIO")
    );
    assert!(d.borrow().writes.is_empty());
    assert_eq!(watchdog_round::feed(&mut watchdog, || Ok(())), Ok(true));
    {
        let d = d.borrow();
        assert_eq!(d.config & 3, 3, "stage-1 interrupts disabled");
        assert_eq!(d.config & 0x20, 0, "timeout reset enabled (active low)");
        assert_eq!(d.config & 4, 0, "long-period prescaler");
        assert_eq!(d.lock, 3, "enabled, locked, two-stage watchdog mode");
        assert!(d.preload.iter().all(|p| *p > 0 && *p <= 0xfffff));
        let quantum_ns = (1_u64 << 15) * 30;
        assert!(d.timeout_ns() >= 120_000_000_000);
        assert!(d.timeout_ns() < 120_000_000_000 + quantum_ns);
        let enable = d
            .writes
            .iter()
            .position(|w| *w == Write::Config8(0x68, 2))
            .unwrap();
        let lock = d
            .writes
            .iter()
            .position(|w| *w == Write::Config8(0x68, 3))
            .unwrap();
        let last_preload = d
            .writes
            .iter()
            .rposition(|w| matches!(w, Write::Mmio32(..)))
            .unwrap();
        assert!(last_preload < enable && enable < lock);
        assert_eq!(d.reloads, 1);
    }
    assert_eq!(watchdog_round::feed(&mut watchdog, || Ok(())), Ok(false));
    assert_eq!(d.borrow().reloads, 2);
    assert_eq!(
        d.borrow()
            .writes
            .iter()
            .filter(|w| matches!(w, Write::Mmio32(..)))
            .count(),
        2
    );
}

#[test]
fn previous_timeout_is_captured_and_cleared_without_feeding() {
    let (watchdog, d) = device(true);
    assert!(watchdog.previous_expired());
    let d = d.borrow();
    assert!(d.status_reads > 0);
    assert!(!d.expired);
    assert_eq!(d.reloads, 0);
    assert_eq!(d.lock, 0);
    assert_eq!(d.writes.last(), Some(&Write::Mmio16(0x0c, 0x200)));
    let (watchdog, d) = device(false);
    assert!(!watchdog.previous_expired());
    assert!(d.borrow().writes.is_empty());
}

#[test]
fn configuration_readback_detects_reset_and_repairs_settings() {
    let (mut watchdog, d) = device(false);
    watchdog_round::feed(&mut watchdog, || Ok(())).unwrap();
    d.borrow_mut().config = 0x24; // reset disabled, fast clock, IRQ enabled
    assert_eq!(watchdog_round::feed(&mut watchdog, || Ok(())), Ok(true));
    assert_eq!(d.borrow().config, 3);
    // A locked disabled device cannot be made safe by issuing a reload.
    d.borrow_mut().lock = 1;
    let reloads = d.borrow().reloads;
    assert!(watchdog_round::feed(&mut watchdog, || Ok(())).is_err());
    assert_eq!(d.borrow().reloads, reloads);
}
