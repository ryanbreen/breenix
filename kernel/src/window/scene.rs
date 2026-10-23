//! B13 metadata reservations and the window side of composition admission.
//! No reservation retains a pixel/tree reference. Core attachment and submission belong
//! to the display backend, which must retain admitted sampling pins until DMA completion.
use super::damage_union::normalized;
use super::{
    mem,
    present::{self, Composed, Pin, Record},
    Caller, REGISTRY,
};
use crate::gpu::window_targets::{self, MetadataCharge, SceneEpoch};
use crate::task::waitqueue::WaitQueueHead;
use alloc::{sync::Arc, vec::Vec};
use breengel_abi::errno::{EAGAIN, EBUSY, EINVAL, EIO, ENOENT, ENOMEM, EPERM};
use breengel_abi::{Composite, CompositeEntry, Cursor, Plain, Rect, SceneSnapshot, SceneWindow};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    id: u64,
    uid: u64,
    incarnation: u64,
    pixel_gen: u64,
    rect: Rect,
    z: u32,
}
struct Source {
    key: Key,
    resource_epoch: u64,
    wire: SceneWindow,
}
pub struct Reservation {
    owner: u64,
    context: u64,
    token: u64,
    claim: u64,
    epoch: SceneEpoch,
    cursor: Cursor,
    sources: Vec<Source>,
    damage: Vec<Rect>,
    full: bool,
    uncovered: usize,
    /// The whole-screen requests made before this snapshot.
    full_requests: u64,
    _charge: MetadataCharge,
}

/// The queue the compositor's scene descriptor sleeps on.
static CHANGES: spin::Once<Arc<WaitQueueHead>> = spin::Once::new();

/// Note that the scene may have changed (a window op, a publication or an owner's exit) and
/// wake the compositor's scene descriptor. Called without the registry lock.
pub fn changed() {
    {
        let mut reg = REGISTRY.lock();
        reg.present.changes = reg.present.changes.wrapping_add(1);
    }
    if let Some(queue) = CHANGES.get() {
        queue.wake_up();
    }
}

/// The scene descriptor's queue, for `EVENT_FD` of window 0 by the compositor claimant;
/// ENOENT for anyone else, as for any id that names no window.
pub fn changes_queue(caller: &Caller) -> Result<Arc<WaitQueueHead>, i32> {
    if !REGISTRY.lock().is_compositor(caller) {
        return Err(ENOENT);
    }
    if let Some(queue) = CHANGES.get() {
        return Ok(queue.clone());
    }
    let queue = Arc::try_new(WaitQueueHead::new()).map_err(|_| ENOMEM)?;
    Ok(CHANGES.call_once(|| queue).clone())
}

fn authority(caller: &Caller) -> Result<u64, i32> {
    if !crate::gpu::window_targets::display_owner(caller.tgid) {
        return Err(EPERM);
    }
    REGISTRY
        .lock()
        .claim
        .filter(|c| c.holder == caller.tgid)
        .map(|c| c.epoch)
        .ok_or(EPERM)
}

fn keys(reg: &super::Registry) -> Result<Vec<Key>, i32> {
    let mut out = Vec::new();
    out.try_reserve_exact(reg.windows.len())
        .map_err(|_| ENOMEM)?;
    for w in reg.windows.values().filter(|w| w.presentation_required()) {
        if w.pending.is_some() {
            return Err(EAGAIN);
        }
        let b = w.front.map(|f| &w.buffers[f]);
        out.push(Key {
            id: w.id,
            uid: b.map_or(0, |b| b.uid),
            incarnation: b.map_or(0, |b| b.write_incarnation),
            pixel_gen: w.pixel_gen,
            rect: w.screen_rect().ok_or(EAGAIN)?,
            z: w.z,
        });
    }
    out.sort_unstable_by_key(|k| (k.z, k.id));
    Ok(out)
}

/// Drop reservations outside the registry lock: charges can acquire the process manager.
pub fn invalidate(owner: u64) {
    let old = {
        let mut reg = REGISTRY.lock();
        if reg.snapshot.as_ref().is_some_and(|s| s.owner == owner) {
            reg.snapshot.take()
        } else {
            None
        }
    };
    drop(old);
}

/// Reclaim metadata after a display/claim/context/reset transition, outside core locks.
pub fn invalidate_stale() {
    let saved = {
        let reg = REGISTRY.lock();
        reg.snapshot
            .as_ref()
            .map(|s| (s.owner, s.context, s.token, s.claim, s.epoch))
    };
    let Some((owner, context, token, claim, epoch)) = saved else {
        return;
    };
    let valid_claim = REGISTRY
        .lock()
        .claim
        .is_some_and(|c| c.holder == owner && c.epoch == claim);
    if valid_claim && window_targets::scene_epoch(owner, context) == Ok(epoch) {
        return;
    }
    let old = {
        let mut reg = REGISTRY.lock();
        if reg.snapshot.as_ref().is_some_and(|s| s.token == token) {
            reg.snapshot.take()
        } else {
            None
        }
    };
    drop(old);
}

pub fn snapshot(caller: &Caller, a: &mut SceneSnapshot, out: u64) -> Result<u64, i32> {
    let claim = authority(caller)?;
    if a.window_count != 0
        || a.damage_count != 0
        || a.snapshot != 0
        || a.width != 0
        || a.height != 0
        || a.scale != 0
        || a.flags != 0
        || a.cursor != Cursor::ZERO
        || a.claim_epoch != 0
        || a.owner_sequence != 0
    {
        return Err(EINVAL);
    }
    super::ops::publish_ready();
    let epoch = window_targets::scene_epoch(caller.tgid, a.context)?;
    let stale = {
        let reg = REGISTRY.lock();
        reg.snapshot
            .as_ref()
            .is_some_and(|s| s.owner != caller.tgid || s.claim != claim || s.epoch != epoch)
    };
    if stale {
        let old = REGISTRY.lock().snapshot.take();
        drop(old);
    }
    let sizing =
        a.window_capacity == 0 && a.damage_capacity == 0 && a.windows == 0 && a.damage == 0;
    // Test point 1, `ignore-damage`: a reservation takes the whole of an armed window
    // with new pixels as its damage, so its composite does the excess work.
    let mut ignored = Vec::new();
    let (scene_keys, targets, cursor, full, damage, uncovered, full_requests) = {
        let reg = REGISTRY.lock();
        if reg.snapshot.is_some() || reg.present.busy {
            return Err(EBUSY);
        }
        let scene_keys = keys(&reg)?;
        let mut targets = Vec::new();
        targets
            .try_reserve_exact(scene_keys.len())
            .map_err(|_| ENOMEM)?;
        for k in &scene_keys {
            let w = &reg.windows[&k.id];
            targets.push((
                w.front.map(|f| w.buffers[f].mem.clone()),
                w.commit_id,
                w.stride(),
            ));
        }
        let full = !reg.present.composed_once || reg.present.force_full;
        let mut damage = Vec::new();
        let capacity = reg
            .windows
            .len()
            .checked_mul(18)
            .and_then(|n| n.checked_add(reg.present.uncovered.len()))
            .ok_or(ENOMEM)?;
        damage.try_reserve_exact(capacity).map_err(|_| ENOMEM)?;
        damage.extend_from_slice(&reg.present.uncovered);
        for w in reg.windows.values() {
            let now = w.screen_rect();
            if w.present.shown != now || (now.is_some() && w.present.shown_z != w.z) {
                damage.extend(w.present.shown);
                damage.extend(now);
            } else if let Some(r) = now {
                let point = (!sizing && !w.present.pending.is_empty() && ignored.try_reserve(1).is_ok())
                    .then(|| crate::test_control::reserve(breengel_abi::tc_point::IGNORE_DAMAGE, w.id))
                    .flatten();
                if let Some(point) = point {
                    ignored.push(point);
                    damage.push(r);
                    continue;
                }
                match &w.present.pending {
                    super::Damage::Full => damage.push(r),
                    super::Damage::Rects(rects) => damage.extend(
                        rects
                            .iter()
                            .map(|d| Rect::new(r.x + d.x, r.y + d.y, d.w, d.h)),
                    ),
                }
            }
        }
        (
            scene_keys,
            targets,
            reg.cursor,
            full,
            damage,
            reg.present.uncovered.len(),
            reg.present.full_requests,
        )
    };
    let screen = Rect::new(0, 0, epoch.width as i32, epoch.height as i32);
    let damage = if full {
        let mut damage = damage;
        damage.clear();
        damage.try_reserve(1).map_err(|_| ENOMEM)?;
        damage.push(screen);
        damage
    } else {
        normalized(&damage, screen)?
    };
    let mut sources = Vec::new();
    sources
        .try_reserve_exact(scene_keys.len())
        .map_err(|_| ENOMEM)?;
    if !sizing
        && ((a.window_capacity as usize) < scene_keys.len()
            || (a.damage_capacity as usize) < damage.len())
    {
        return Err(EINVAL);
    }
    if !sizing {
        mem::probe_writable(
            a.windows,
            (a.window_capacity as u64).checked_mul(80).ok_or(EINVAL)?,
        )?;
        mem::probe_writable(
            a.damage,
            (a.damage_capacity as u64).checked_mul(16).ok_or(EINVAL)?,
        )?;
    }
    for (key, (target, commit_id, stride)) in scene_keys.iter().cloned().zip(targets) {
        let (resource_epoch, generation, fence) = match &target {
            Some(t) => t.identity()?,
            None => (epoch.reset, 0, 0),
        };
        let sample = if !sizing && target.is_some() {
            window_targets::scene_handle(caller.tgid)?
        } else {
            0
        };
        sources.push(Source {
            resource_epoch,
            wire: SceneWindow {
                id: key.id,
                sample,
                commit_id,
                pixel_gen: key.pixel_gen,
                render_fence: fence,
                target_generation: generation,
                x: key.rect.x,
                y: key.rect.y,
                width: key.rect.w as u32,
                height: key.rect.h as u32,
                z: key.z,
                stride,
                format: 0,
                reserved: 0,
            },
            key,
        });
    }
    a.window_count = sources.len().try_into().map_err(|_| ENOMEM)?;
    a.damage_count = damage.len().try_into().map_err(|_| ENOMEM)?;
    a.width = epoch.width;
    a.height = epoch.height;
    a.scale = epoch.scale;
    a.flags = full as u32;
    a.cursor = cursor;
    a.claim_epoch = claim;
    a.owner_sequence = epoch.owner_sequence;
    if sizing {
        crate::syscall::window::copy_out(out, a)?;
        return Ok(0);
    }
    let bytes = core::mem::size_of::<Reservation>()
        .checked_add(
            sources
                .capacity()
                .checked_mul(core::mem::size_of::<Source>())
                .ok_or(ENOMEM)?,
        )
        .and_then(|n| n.checked_add(damage.capacity() * core::mem::size_of::<Rect>()))
        .ok_or(ENOMEM)?;
    let charge = MetadataCharge::new(caller.tgid, bytes)?;
    let token = window_targets::scene_handle(caller.tgid)?;
    for (i, s) in sources.iter().enumerate() {
        crate::syscall::window::copy_out(a.windows + i as u64 * 80, &s.wire)?;
    }
    for (i, r) in damage.iter().enumerate() {
        crate::syscall::window::copy_out(a.damage + i as u64 * 16, r)?;
    }
    a.snapshot = token;
    crate::syscall::window::copy_out(out, a)?;
    if authority(caller)? != claim || window_targets::scene_epoch(caller.tgid, a.context)? != epoch
    {
        return Err(EAGAIN);
    }
    let reservation = Reservation {
        owner: caller.tgid,
        context: a.context,
        token,
        claim,
        epoch,
        cursor,
        sources,
        damage,
        full,
        uncovered,
        full_requests,
        _charge: charge,
    };
    let mut reg = REGISTRY.lock();
    if reg.snapshot.is_some() || reg.present.busy {
        drop(reg);
        drop(reservation);
        return Err(EBUSY);
    }
    if keys(&reg)? != scene_keys || reg.cursor != cursor {
        drop(reg);
        drop(reservation);
        return Err(EAGAIN);
    }
    reg.snapshot = Some(reservation);
    drop(reg);
    for point in ignored {
        point.fire();
    }
    Ok(0)
}

/// Call-local exact fronts. A display submission clones only sampling pins into its
/// completion holds; retaining this metadata after return grants no new authority.
pub struct AdmittedSource {
    pub window: SceneWindow,
    pub pin: Option<Arc<Pin>>,
}
pub struct Admission {
    reservation: Reservation,
    pub sources: Vec<AdmittedSource>,
    pub serial: u64,
    entries: Vec<CompositeEntry>,
}
impl Admission {
    pub fn context(&self) -> u64 {
        self.reservation.context
    }
    pub fn epoch(&self) -> SceneEpoch {
        self.reservation.epoch
    }
    pub fn damage(&self) -> &[Rect] {
        &self.reservation.damage
    }
    pub fn full(&self) -> bool {
        self.reservation.full
    }
    /// Call only after core stream validation, before any attach or device send.
    pub fn begin(&mut self) -> Result<(), i32> {
        let caller = Caller::current()?;
        if authority(&caller)? != self.reservation.claim
            || window_targets::scene_epoch(caller.tgid, self.context())? != self.epoch()
        {
            return Err(EPERM);
        }
        let mut reg = REGISTRY.lock();
        if self.serial != 0 {
            return Err(EBUSY);
        }
        reg.present.serial += 1;
        self.serial = reg.present.serial;
        Ok(())
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        let mut reg = REGISTRY.lock();
        reg.present.busy = false;
        if self.serial != 0 {
            reg.present.request_full();
        }
    }
}

pub fn submit(caller: &Caller, a: &mut Composite, out: u64) -> Result<u64, i32> {
    let claim = authority(caller)?;
    if a.reserved != 0
        || a.flags > 1
        || a.serial != 0
        || a.pixels != 0
        || a.render_fence != 0
        || a.present_fence != 0
        || a.estimate != 0
        || a.error_index != 0
        || a.error != 0
        || a.reason != 0
        || a.fence_status != 0
        || a.answer != 0
        || a.phase != 0
    {
        return Err(EINVAL);
    }
    if a.flags == 1 && (a.target != 0 || a.stream != 0 || a.length != 0 || a.scene_target != 0) {
        return Err(EINVAL);
    }
    let reservation = {
        let mut reg = REGISTRY.lock();
        if !reg
            .snapshot
            .as_ref()
            .is_some_and(|s| s.owner == caller.tgid && s.token == a.snapshot)
        {
            return Err(ENOENT);
        }
        reg.snapshot.take().ok_or(ENOENT)?
    };
    let epoch = window_targets::scene_epoch(caller.tgid, reservation.context)?;
    if reservation.claim != claim || reservation.epoch.owner_sequence != epoch.owner_sequence {
        return Err(ENOENT);
    }
    if reservation.epoch.reset != epoch.reset {
        a.reason = gpu_abi::readback_reason::DEVICE_RESET;
        crate::syscall::window::copy_out(out, a)?;
        return Err(EIO);
    }
    if reservation.epoch != epoch {
        return Err(EAGAIN);
    }
    if a.flags == 1 {
        return Ok(0);
    }
    super::ops::publish_ready();
    // Check resource metadata before taking the registry; no registry/core lock nesting.
    let mut identities = Vec::new();
    identities
        .try_reserve_exact(reservation.sources.len())
        .map_err(|_| ENOMEM)?;
    {
        let reg = REGISTRY.lock();
        for s in &reservation.sources {
            let w = reg.windows.get(&s.key.id).ok_or(ENOENT)?;
            let t = w.front.map(|f| w.buffers[f].mem.clone());
            identities.push(t);
        }
    }
    for (s, t) in reservation.sources.iter().zip(&identities) {
        if let Some(t) = t {
            let (epoch, generation, _) = t.identity()?;
            if epoch != s.resource_epoch || generation != s.wire.target_generation {
                return Err(EAGAIN);
            }
        }
    }
    drop(identities);
    let mut sources = Vec::new();
    let mut pinned = Vec::new();
    pinned
        .try_reserve_exact(reservation.sources.len())
        .map_err(|_| ENOMEM)?;
    sources
        .try_reserve_exact(reservation.sources.len())
        .map_err(|_| ENOMEM)?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(reservation.sources.len())
        .map_err(|_| ENOMEM)?;
    {
        let mut reg = REGISTRY.lock();
        if reg.present.busy {
            return Err(EBUSY);
        }
        let current = keys(&reg)?;
        if current.len() != reservation.sources.len()
            || !current
                .iter()
                .zip(&reservation.sources)
                .all(|(k, s)| k == &s.key)
            || reg.cursor != reservation.cursor
        {
            return Err(EAGAIN);
        }
        reg.present.records.try_reserve(1).map_err(|_| ENOMEM)?;
        for s in &reservation.sources {
            let w = reg.windows.get_mut(&s.key.id).ok_or(ENOENT)?;
            let pin = w.front.map(|f| {
                let b = &mut w.buffers[f];
                b.pins += 1;
                Pin::new(w.id, b.uid, b.mem.clone())
            });
            pinned.push((s.wire, pin));
            entries.push(CompositeEntry {
                id: s.wire.id,
                commit_id: s.wire.commit_id,
                pixel_gen: s.wire.pixel_gen,
                pixels: 0,
            });
        }
        reg.present.busy = true;
    }
    for (window, pin) in pinned {
        let pin = match pin {
            Some(pin) => match Arc::try_new(pin) {
                Ok(pin) => Some(pin),
                Err(_) => {
                    REGISTRY.lock().present.busy = false;
                    return Err(ENOMEM);
                }
            },
            None => None,
        };
        sources.push(AdmittedSource { window, pin });
    }
    let mut frame = Admission {
        reservation,
        sources,
        serial: 0,
        entries,
    };
    // Test point 16, `composite-stall`: a composite that includes the armed window pauses
    // here, with the sources' fronts pinned, before the backend composes.
    if frame.sources.iter().any(|s| crate::test_control::take(breengel_abi::tc_point::COMPOSITE_STALL, s.window.id).is_some()) {
        super::ops::stall();
    }
    let result = present::backend()
        .ok_or(EIO)
        .and_then(|b| b.submit_scene(caller, &mut frame, a));
    a.serial = frame.serial;
    let copied = crate::syscall::window::copy_out(out, a);
    if copied.is_err() {
        finish(&mut frame, Err(breengel_abi::errno::EFAULT), 0)?;
    }
    finish(&mut frame, result, a.reason)?;
    Ok(0)
}
fn finish(frame: &mut Admission, result: Result<Composed, i32>, reason: u32) -> Result<(), i32> {
    let result = result.and_then(|done| {
        if done.per_window.len() == frame.entries.len() {
            Ok(done)
        } else {
            Err(EIO)
        }
    });
    let mut reg = REGISTRY.lock();
    if frame.serial == 0 {
        return result.and(Err(EIO));
    }
    let (pixels, flags) = match &result {
        Ok(c) if c.per_window.len() == frame.entries.len() => {
            for (e, &pixels) in frame.entries.iter_mut().zip(&c.per_window) {
                e.pixels = pixels;
            }
            for s in &frame.reservation.sources {
                if let Some(w) = reg.windows.get_mut(&s.key.id) {
                    if w.pixel_gen == s.key.pixel_gen {
                        w.present.pending = super::Damage::none();
                    }
                    w.present.shown = Some(s.key.rect);
                    w.present.shown_z = s.key.z;
                }
            }
            let n = frame.reservation.uncovered.min(reg.present.uncovered.len());
            reg.present.uncovered.drain(..n);
            reg.present.composed_once = true;
            // A cursor change during the frame left its footprints in `uncovered`, unless they
            // overflowed it: that asked for a whole screen this frame did not compose.
            if reg.present.full_requests == frame.reservation.full_requests {
                reg.present.force_full = false;
            }
            crate::tracing::providers::window::count_composite(c.pixels);
            (c.pixels, 0)
        }
        _ => {
            frame.entries.clear();
            reg.present.request_full();
            (
                0,
                breengel_abi::record_flags::FAILED
                    | ((reason & 255) << breengel_abi::record_flags::REASON_SHIFT),
            )
        }
    };
    reg.present.records.push_back(Record {
        serial: frame.serial,
        pixels,
        flags,
        entries: core::mem::take(&mut frame.entries),
    });
    if reg.present.records.len() > present::RECORDS {
        reg.present.records.pop_front();
    }
    frame.serial = 0;
    result.map(|_| ())
}
