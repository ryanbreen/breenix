//! The port's integrated copper PHY, reached over the MAC's MDIO interface.
//!
//! Register access follows the guide's "Reading a PHY Register" and "Writing a
//! PHY Register" (Section 10, pp. 205-206) and "MDI Register Access"
//! (pp. 211-212): one transaction at a time through the MI Communication
//! register, polled until Start/Busy clears. The bring-up sequence is the
//! guide's "PHY Configuration Auto-Negotiation" (pp. 207-211). Register meanings
//! are IEEE 802.3 clause 22 (control and status), clause 28 (advertisement)
//! and clause 40 (1000BASE-T control), plus the guide's Auxiliary Status
//! Summary (Section 14, p. 525).

use super::regs::*;
use super::{delay_us, Function};

/// Polls of Start/Busy before a transaction is declared failed (the guide's
/// example loops 5000 times).
const MDIO_POLLS: u32 = 5000;

/// The speed and duplex autonegotiation resolved.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub mbps: u32,
    pub full_duplex: bool,
}

impl Function {
    fn mdio(&self, command: u32) -> Result<u32, &'static str> {
        self.write32(MI_COMMUNICATION, command | MI_COM_START_BUSY);
        for _ in 0..MDIO_POLLS {
            let value = self.read32(MI_COMMUNICATION);
            if value & MI_COM_START_BUSY == 0 {
                return Ok(value);
            }
            delay_us(10);
        }
        Err("MDIO transaction did not complete")
    }

    pub(super) fn phy_read(&self, register: u8) -> Result<u16, &'static str> {
        let value = self.mdio(
            (u32::from(self.phy) << MI_COM_PHY_SHIFT)
                | (u32::from(register) << MI_COM_REG_SHIFT)
                | MI_COM_CMD_READ,
        )?;
        if value & MI_COM_READ_FAILED != 0 {
            return Err("the PHY did not answer an MDIO read");
        }
        Ok(value as u16)
    }

    pub(super) fn phy_write(&self, register: u8, value: u16) -> Result<(), &'static str> {
        self.mdio(
            (u32::from(self.phy) << MI_COM_PHY_SHIFT)
                | (u32::from(register) << MI_COM_REG_SHIFT)
                | MI_COM_CMD_WRITE
                | u32::from(value),
        )
        .map(|_| ())
    }

    /// The link bit of the clause 22 status register. It latches low, so the
    /// first read reports a drop since the last read and the second the
    /// current state.
    pub(super) fn phy_link(&self) -> Result<bool, &'static str> {
        self.phy_read(MII_BMSR)?;
        Ok(self.phy_read(MII_BMSR)? & BMSR_LINK != 0)
    }

    /// The PHY identifier, registers 2 and 3.
    pub(super) fn phy_id(&self) -> Result<u32, &'static str> {
        let high = self.phy_read(MII_PHY_ID_HIGH)?;
        let low = self.phy_read(MII_PHY_ID_LOW)?;
        Ok((u32::from(high) << 16) | u32::from(low))
    }

    /// Keep an operational autonegotiated firmware link. The reset mailbox
    /// deliberately preserves the PHY (Section 10, "Firmware Mailbox"); HTTP
    /// boot has already negotiated it. Resetting it again destroys that handoff.
    /// Otherwise use the guide's PHY setup sequence (Section 10, pp. 207-211).
    pub(super) fn phy_configure(&self) -> Result<(), &'static str> {
        let control = self.phy_read(MII_BMCR)?;
        let inactive = BMCR_RESET | BMCR_LOOPBACK | BMCR_POWER_DOWN | BMCR_ISOLATE
            | BMCR_RESTART_AUTONEG;
        if control & (inactive | BMCR_AUTONEG_ENABLE) == BMCR_AUTONEG_ENABLE
            && self.phy_resolved()?.is_some()
        {
            return Ok(());
        }

        self.phy_write(MII_BMCR, BMCR_RESET)?;
        let mut reset_done = false;
        for _ in 0..100 {
            if self.phy_read(MII_BMCR)? & BMCR_RESET == 0 {
                reset_done = true;
                break;
            }
            delay_us(1000);
        }
        if !reset_done {
            return Err("the PHY reset did not complete within 100 ms");
        }

        self.phy_write(MII_AUX_CONTROL, AUX_SHADOW_MISC_READ)?;
        let misc = self.phy_read(MII_AUX_CONTROL)? | AUX_MISC_FORCE_AUTO_MDIX;
        self.phy_write(MII_AUX_CONTROL, misc | AUX_SHADOW_WRITE)?;
        let extended =
            self.phy_read(MII_PHY_EXTENDED_CONTROL)? & !EXTENDED_CONTROL_DISABLE_AUTO_MDIX;
        self.phy_write(MII_PHY_EXTENDED_CONTROL, extended)?;

        self.phy_write(MII_BMCR, BMCR_LOOPBACK)?;
        let mut link_down = false;
        for _ in 0..1500 {
            // Reset may have latched a previous link drop. Only the second
            // status read tells us whether loopback has dropped it *now*.
            if !self.phy_link()? {
                link_down = true;
                break;
            }
            delay_us(10);
        }
        if !link_down {
            return Err("the PHY link did not drop within 15 ms of loopback");
        }

        self.phy_write(MII_ADVERTISE, ADVERTISE_ALL)?;
        self.phy_write(MII_1000BASE_T_CONTROL, GIGABIT_ADVERTISE_FULL)?;
        self.phy_write(MII_BMCR, BMCR_AUTONEG_ENABLE | BMCR_RESTART_AUTONEG)
    }

    /// What autonegotiation resolved, once the link is up and negotiation has
    /// completed; an empty result while either is still pending.
    pub(super) fn phy_resolved(&self) -> Result<Option<Resolved>, &'static str> {
        self.phy_read(MII_BMSR)?;
        let status = self.phy_read(MII_BMSR)?;
        if status & BMSR_LINK == 0 || status & BMSR_AUTONEG_COMPLETE == 0 {
            return Ok(None);
        }
        let hcd = (self.phy_read(MII_AUX_STATUS)? >> AUX_STATUS_HCD_SHIFT) & AUX_STATUS_HCD_MASK;
        Ok(match hcd {
            HCD_1000_FULL => Some(Resolved {
                mbps: 1000,
                full_duplex: true,
            }),
            HCD_1000_HALF => Some(Resolved {
                mbps: 1000,
                full_duplex: false,
            }),
            HCD_100_FULL => Some(Resolved {
                mbps: 100,
                full_duplex: true,
            }),
            HCD_100_HALF => Some(Resolved {
                mbps: 100,
                full_duplex: false,
            }),
            HCD_10_FULL => Some(Resolved {
                mbps: 10,
                full_duplex: true,
            }),
            HCD_10_HALF => Some(Resolved {
                mbps: 10,
                full_duplex: false,
            }),
            _ => None,
        })
    }
}
