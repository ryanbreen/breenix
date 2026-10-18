//! Execute the production PHY sequence against a timed MDIO register model.
use std::cell::{Cell, RefCell};

#[path = "../kernel/src/drivers/bcm5720/phy.rs"]
mod phy;
// The register map also exports the MAC/DMA API, unused by this PHY-only harness.
#[allow(dead_code)]
#[path = "../kernel/src/drivers/bcm5720/regs.rs"]
mod regs;
use regs::*;

thread_local! { static CLOCK_US: Cell<u64> = const { Cell::new(0) }; }
fn delay_us(us: u64) {
    CLOCK_US.with(|clock| clock.set(clock.get() + us));
}
fn now_us() -> u64 {
    CLOCK_US.with(Cell::get)
}

struct Model {
    registers: [u16; 32],
    response: u32,
    writes: Vec<(u8, u16)>,
    reset_at: Option<u64>,
    reset_delay: Option<u64>,
    loopback_at: Option<u64>,
    drop_delay: Option<u64>,
    latched_low: bool,
}

struct Function {
    phy: u8,
    model: RefCell<Model>,
}
impl Function {
    fn new(control: u16) -> Self {
        CLOCK_US.with(|clock| clock.set(0));
        let mut registers = [0; 32];
        registers[MII_BMCR as usize] = control;
        registers[MII_BMSR as usize] = BMSR_LINK | BMSR_AUTONEG_COMPLETE;
        registers[MII_AUX_STATUS as usize] = HCD_1000_FULL << AUX_STATUS_HCD_SHIFT;
        registers[MII_PHY_ID_HIGH as usize] = 0x0362;
        registers[MII_PHY_ID_LOW as usize] = 0x5f60;
        Self {
            phy: 2,
            model: RefCell::new(Model {
                registers,
                response: 0,
                writes: Vec::new(),
                reset_at: None,
                reset_delay: Some(2000),
                loopback_at: None,
                drop_delay: Some(3000),
                latched_low: true,
            }),
        }
    }

    fn write32(&self, offset: usize, command: u32) {
        assert_eq!(offset, MI_COMMUNICATION);
        assert_eq!((command >> MI_COM_PHY_SHIFT) & 31, 2);
        let register = ((command >> MI_COM_REG_SHIFT) & 31) as u8;
        let mut m = self.model.borrow_mut();
        let value = if command & MI_COM_CMD_READ != 0 {
            if register == MII_BMCR
                && m.reset_at
                    .zip(m.reset_delay)
                    .is_some_and(|(at, delay)| now_us() - at >= delay)
            {
                m.registers[register as usize] &= !BMCR_RESET;
            }
            let mut value = m.registers[register as usize];
            if register == MII_BMSR {
                if m.latched_low
                    || m.loopback_at
                        .zip(m.drop_delay)
                        .is_some_and(|(at, delay)| now_us() - at >= delay)
                {
                    value &= !BMSR_LINK;
                }
                m.latched_low = false;
            }
            value
        } else {
            assert_ne!(command & MI_COM_CMD_WRITE, 0);
            let value = command as u16;
            m.writes.push((register, value));
            m.registers[register as usize] = value;
            if register == MII_BMCR && value & BMCR_RESET != 0 {
                m.reset_at = Some(now_us());
                m.latched_low = true;
            }
            if register == MII_BMCR && value & BMCR_LOOPBACK != 0 {
                m.loopback_at = Some(now_us());
            }
            if register == MII_BMCR && value & BMCR_RESTART_AUTONEG != 0 {
                assert!(
                    m.loopback_at
                        .zip(m.drop_delay)
                        .is_some_and(|(at, delay)| now_us() - at >= delay),
                    "autonegotiation restarted before current link went down"
                );
            }
            value
        };
        m.response = u32::from(value);
    }

    fn read32(&self, offset: usize) -> u32 {
        assert_eq!(offset, MI_COMMUNICATION);
        self.model.borrow().response
    }
}

#[test]
fn keep_negotiated_firmware_link_even_with_a_latched_drop() {
    let f = Function::new(BMCR_AUTONEG_ENABLE);
    assert_eq!(f.phy_id().unwrap(), 0x0362_5f60);
    assert!(f.phy_link().unwrap());
    f.model.borrow_mut().latched_low = true;
    f.phy_configure().unwrap();
    assert!(
        f.model.borrow().writes.is_empty(),
        "must preserve the live PHY"
    );
    let link = f.phy_resolved().unwrap().unwrap();
    assert_eq!(link.mbps, 1000);
    assert!(link.full_duplex);
}

#[test]
fn forced_or_inactive_phy_restarts_only_after_current_link_drops() {
    for control in [
        0,
        BMCR_AUTONEG_ENABLE | BMCR_LOOPBACK,
        BMCR_AUTONEG_ENABLE | BMCR_POWER_DOWN,
        BMCR_AUTONEG_ENABLE | BMCR_ISOLATE,
        BMCR_AUTONEG_ENABLE | BMCR_RESTART_AUTONEG,
        BMCR_AUTONEG_ENABLE | BMCR_RESET,
    ] {
        let f = Function::new(control);
        f.phy_configure().unwrap();
        let m = f.model.borrow();
        assert_eq!(m.writes.first(), Some(&(MII_BMCR, BMCR_RESET)));
        assert_eq!(m.registers[MII_ADVERTISE as usize], ADVERTISE_ALL);
        assert_eq!(
            m.registers[MII_1000BASE_T_CONTROL as usize],
            GIGABIT_ADVERTISE_FULL
        );
        assert_eq!(
            m.writes.last(),
            Some(&(MII_BMCR, BMCR_AUTONEG_ENABLE | BMCR_RESTART_AUTONEG))
        );
    }
}

#[test]
fn loopback_timeout_does_not_restart_or_advertise() {
    let f = Function::new(0);
    f.model.borrow_mut().drop_delay = None;
    assert_eq!(
        f.phy_configure(),
        Err("the PHY link did not drop within 15 ms of loopback")
    );
    let m = f.model.borrow();
    assert!(m.writes.iter().all(|&(reg, _)| reg != MII_ADVERTISE));
    assert!(now_us() >= m.loopback_at.unwrap() + 15_000);
}

#[test]
fn reset_timeout_does_not_program_the_phy() {
    let f = Function::new(0);
    f.model.borrow_mut().reset_delay = None;
    assert_eq!(
        f.phy_configure(),
        Err("the PHY reset did not complete within 100 ms")
    );
    assert_eq!(f.model.borrow().writes, [(MII_BMCR, BMCR_RESET)]);
}

#[test]
fn carrier_without_completed_negotiation_is_not_ready() {
    let f = Function::new(BMCR_AUTONEG_ENABLE);
    f.model.borrow_mut().registers[MII_BMSR as usize] = BMSR_LINK;
    assert!(f.phy_resolved().unwrap().is_none());
}
