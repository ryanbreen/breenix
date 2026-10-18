//! EP0 orchestration, xHCI 1.2 4.6.8 and 4.10.1.1; USB 2.0 9.2.6.
//! The hardware adapter and host tests execute this same sequence.
use super::spec::{self, Trb};
use alloc::vec::Vec;

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
        let event = control.wait(&pointers)?;
        identity(event, control.slot())?;
        if event.code() == 13 {
            // A short Data TD does not halt EP0. Retire Status even on the
            // final attempt before returning or reusing the ring (4.10.1.1).
            let status = *pointers.last().ok_or("EP0 transfer has no Status TD")?;
            if event.parameter & !15 != status {
                let done = control.wait(&[status])?;
                identity(done, control.slot())?;
                if done.code() != 1 {
                    if spec::halts_endpoint(done.code()) { control.recover()?; }
                    return Ok(done);
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
