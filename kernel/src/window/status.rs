//! Compositor progress publication (BreenGL 17.1, E18 W16).
use breengel_abi::{errno::{EINVAL, ENOENT, EPERM}, CompositorAck, CompositorStatus};
use super::{Caller, REGISTRY};

pub(super) fn completed() -> u64 {
    crate::tracing::providers::window::WINDOW_COMPOSITES.aggregate()
}

pub fn ack(caller: &Caller, ack: &CompositorAck) -> Result<u64, i32> {
    if ack.flags & !1 != 0 || ack.reserved != 0 { return Err(EINVAL); }
    let mut reg = REGISTRY.lock();
    let claim = reg.claim.filter(|c| c.holder == caller.tgid && c.epoch == ack.claim_epoch)
        .ok_or(EPERM)?;
    if ack.epoch < claim.ack.epoch || ack.handled_serial < claim.ack.handled_serial {
        return Err(EINVAL);
    }
    if ack.presented != 0 && (ack.presented <= claim.first_serial ||
        !reg.present.records.iter().any(|r| r.serial == ack.presented && r.flags == 0)) {
        return Err(EINVAL);
    }
    reg.claim.as_mut().ok_or(EPERM)?.ack = *ack;
    Ok(0)
}

pub fn read(caller: &Caller) -> Result<CompositorStatus, i32> {
    let reg = REGISTRY.lock();
    let claim = reg.claim.ok_or(ENOENT)?;
    if !caller.is_root() && claim.holder != caller.tgid &&
        !reg.windows.values().any(|w| w.owner == caller.tgid && w.registered) {
        return Err(EPERM);
    }
    Ok(CompositorStatus {
        claim_epoch: claim.epoch,
        epoch: claim.ack.epoch,
        handled_serial: claim.ack.handled_serial,
        presented: claim.ack.presented,
        scene_version: reg.scene_version,
        composites: completed().saturating_sub(claim.composites_base),
        next_deadline_ns: claim.ack.next_deadline_ns,
        owner: claim.holder as u32,
        flags: claim.ack.flags,
        reserved: [0; 2],
    })
}

/// The proc text view uses the same authority and atomic snapshot as op 31.
pub fn text(caller: &Caller) -> Result<alloc::string::String, i32> {
    let s = read(caller)?;
    Ok(alloc::format!("claim_epoch={} epoch={} handled_serial={} presented={} scene_version={} composites={} next_deadline_ns={} owner={} flags={}\n",
        s.claim_epoch, s.epoch, s.handled_serial, s.presented, s.scene_version,
        s.composites, s.next_deadline_ns, s.owner, s.flags))
}
