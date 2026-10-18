//! EP0 and device-reset orchestration, xHCI 1.2 4.6.8, 4.10.1.1 and
//! 4.10.2.1; USB 2.0 9.2.6. The hardware adapters and host tests execute
//! these same sequences.
use super::spec::{self, Trb};
use alloc::vec::Vec;

/// Device resets one attach may use when configuration is still refused
/// after EP0 recovery (4.10.2.1.2).
pub(super) const DEVICE_RESETS: u32 = 2;

pub(super) trait Control {
    fn slot(&self) -> u8;
    fn submit(&mut self) -> Vec<u64>;
    fn wait(&mut self, pointers: &[u64]) -> Result<Trb, &'static str>;
    fn recover(&mut self) -> Result<(), &'static str>;
    fn retry(&mut self, event: Trb, attempt: u32, after_ns: u64);
}

fn identity(event: Trb, slot: u8) -> Result<(), &'static str> {
    if event.kind() != 32 || event.slot() != slot || (event.control >> 16) & 31 != 1 {
        return Err("EP0 transfer identity mismatch");
    }
    Ok(())
}

pub(super) fn transfer(control: &mut impl Control) -> Result<Trb, &'static str> {
    for attempt in 1..=spec::EP0_ATTEMPTS {
        let pointers = control.submit();
        let mut event = control.wait(&pointers)?;
        identity(event, control.slot())?;
        if event.code() == 13 {
            // A short Data TD does not halt EP0; the xHC goes on to the Status
            // TD (4.10.1.1). Retire Status even on the final attempt before
            // returning or reusing the ring. A failed Status is this attempt's
            // outcome: recovered and sent again like any other completion
            // (4.10.2.1), and the one reported when no attempt is left.
            let status = *pointers.last().ok_or("EP0 transfer has no Status TD")?;
            if event.parameter & !15 != status {
                let done = control.wait(&[status])?;
                identity(done, control.slot())?;
                if done.code() != 1 {
                    event = done;
                }
            }
        }
        let after_ns = match spec::ep0_next(event.code(), attempt) {
            spec::Ep0Next::Done | spec::Ep0Next::Fail => return Ok(event),
            spec::Ep0Next::RecoverAndFail => {
                control.recover()?;
                return Ok(event);
            }
            spec::Ep0Next::Retry { after_ns } => {
                control.recover()?;
                after_ns
            }
            spec::Ep0Next::Resend { after_ns } => after_ns,
        };
        control.retry(event, attempt, after_ns);
    }
    Err("EP0 attempt limit exceeded")
}

/// How one enumerate-and-configure attempt on a port ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Attach {
    /// Configured, not a boot keyboard, or not resettable (its slot stayed
    /// with the controller).
    Settled,
    /// Enumeration or configuration failed and the slot was given back.
    Refused,
}

pub(super) trait Port {
    fn connected(&self) -> bool;
    /// Reset the device and enumerate and configure it again; `reset`
    /// counts from 1.
    fn reset(&mut self, reset: u32) -> Attach;
}

/// After a refused attach, reset the device and attach it again while it is
/// connected, at most `DEVICE_RESETS` times. Returns the resets used.
pub(super) fn reset_while_refused(port: &mut impl Port, first: Attach) -> u32 {
    let (mut outcome, mut resets) = (first, 0);
    while outcome == Attach::Refused && resets < DEVICE_RESETS && port.connected() {
        resets += 1;
        outcome = port.reset(resets);
    }
    resets
}
