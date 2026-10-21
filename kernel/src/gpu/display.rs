//! GPU scanout binding, modes and the ownership stack (gpu.md section 1.7).
//!
//! A `PRESENT` is a fenced operation of the context it names: SET_SCANOUT and a fenced
//! RESOURCE_FLUSH, numbered in that context's fence sequence with its `SUBMIT`s, so its
//! fence is a context fence `WAIT` accepts and record offset 184 carries.
use super::*;
use abi::errno::EPERM;

const EBUSY: i64 = 16;

/// The display's modes (section 1.7): 1280x800 at scale 1 and 2560x1600 at scale 2.
pub(super) const MODES: [abi::Mode; 2] = [
    abi::Mode { width: 1280, height: 800, scale: 1, reserved: 0 },
    abi::Mode { width: 2560, height: 1600, scale: 2, reserved: 0 },
];
/// The mode the display returns to when no process owns it (`publish_owner`), which the
/// kernel's screen is drawn at.
pub(super) const KERNEL_MODE: usize = 1;

#[derive(Clone, Copy)]
pub(super) struct Binding {
    pub id: u32,
    /// The context fence of the `PRESENT` whose flush completed this binding.
    pub fence: u64,
    pub answer: u32,
}

pub(super) struct Owner {
    pub group: u64,
    pub pid: u64,
    pub mode: u32,
    pub last: Option<Binding>,
    /// Private CPU framebuffer target, retained across nested owners.
    pub pane: Option<u32>,
}

pub(super) struct State {
    pub stack: Vec<Owner>,
    pub mode: u32,
    pub changing: bool,
    pub owner_sequence: u64,
    /// The binding the last completed `PRESENT` made, while no later one is outstanding.
    pub binding: Option<Binding>,
    /// Resources a sent bind could have installed, held until a later sent
    /// bind's flush completes or a confirmed reset ends device access.
    pub holds: BTreeSet<u32>,
    /// Whether the kernel knows what the scanout shows: no bind is outstanding and the
    /// last one completed OK, or a confirmed reset cleared it.
    pub known: bool,
    /// `PRESENT`s sent, in order; the latest one decides the binding once it completes.
    pub sent: u64,
    /// `PRESENT`s sent whose flush has not completed.
    pub outstanding: u32,
    /// The latest sent `PRESENT` completed OK (initially true before a send).
    pub latest_ok: bool,
}
impl State {
    pub const fn new() -> Self {
        Self {
            stack: Vec::new(),
            mode: KERNEL_MODE as u32,
            changing: false,
            owner_sequence: 0,
            binding: None,
            holds: BTreeSet::new(),
            known: true,
            sent: 0,
            outstanding: 0,
            latest_ok: true,
        }
    }
}

/// What a `PRESENT` sub carries: the target it binds and its place among sent presents.
#[derive(Clone, Copy)]
pub(super) struct Presenting {
    pub id: u32,
    pub seq: u64,
    pub owner: u64,
}

/// Whether thread group `tgid` owns the display now. Ownership is recorded per thread
/// group with the take-over, so this reads one word and takes no lock.
pub(super) fn owner(tgid: u64) -> bool {
    crate::syscall::graphics::DISPLAY_OWNER_GROUP.load(Ordering::Acquire) == tgid
}

/// Whether `tgid` may observe target `id`: a possible scanout is the display owner's, or a
/// `window` observer's (one the window layer authorized for SCANOUT_READBACK), and only
/// while its binding is known. EIO carries reason 10, binding-unknown.
pub(super) fn readable(c: &Core, tgid: u64, id: u32, window: bool) -> Result<(), i64> {
    if c.display.holds.contains(&id)
        || c.display
            .stack
            .iter()
            .any(|o| o.last.is_some_and(|b| b.id == id))
    {
        if !window && !owner(tgid) {
            return Err(EPERM);
        }
        if !c.display.known {
            return Err(EIO);
        }
    }
    Ok(())
}

/// The completed binding of `id`, for record v2's flags bit 0 and offsets 184 and 192.
pub(super) fn binding(c: &Core, tgid: u64, id: u32, window: bool) -> Result<Option<Binding>, i64> {
    readable(c, tgid, id, window)?;
    Ok(c.display.binding.filter(|b| b.id == id))
}

impl Core {
    /// A `PRESENT`'s flush came back: the latest one, completed OK, makes its binding
    /// known and ends the holds of resources only an earlier bind could have shown.
    /// The binding is known again once the outstanding count is 0 and the latest completed OK.
    pub(super) fn presented(&mut self, p: Presenting, ok: bool, fence: u64, answer: u32) {
        let d = &mut self.display;
        d.outstanding = d.outstanding.saturating_sub(1);
        let latest = p.seq == d.sent;
        if latest {
            d.latest_ok = ok;
        }
        d.known = d.outstanding == 0 && d.latest_ok;
        if ok {
            // The process's record already exists: it holds the presented target.
            if let Some(owner) = self.procs.get_mut(&p.owner) {
                owner.presents += 1;
            }
        }
        let d = &mut self.display;
        if !(latest && ok) {
            return;
        }
        d.binding = Some(Binding {
            id: p.id,
            fence,
            answer,
        });
        let saved = d.stack.last_mut().filter(|o| o.group == p.owner).map(|o| {
            o.last.replace(Binding {
                id: p.id,
                fence,
                answer,
            })
        });
        let old: Vec<u32> = d.holds.iter().copied().filter(|&rid| rid != p.id).collect();
        if let Some(previous) = saved {
            if previous.map(|b| b.id) != Some(p.id) {
                if let Some(r) = self.resources.get_mut(&p.id) {
                    r.holds += 1;
                }
                if let Some(b) = previous {
                    self.unhold(b.id);
                }
            }
        }
        for rid in old {
            self.display.holds.remove(&rid);
            self.unhold(rid);
        }
    }

    /// A confirmed reset ended device access: the binding is cleared and marked known.
    pub(super) fn display_reset(&mut self) {
        let held = core::mem::take(&mut self.display.holds);
        self.display.binding = None;
        self.display.known = true;
        self.display.outstanding = 0;
        self.display.latest_ok = true;
        let saved: Vec<u32> = self
            .display
            .stack
            .iter_mut()
            .filter_map(|o| o.last.take().map(|b| b.id))
            .collect();
        for id in saved {
            self.unhold(id);
        }
        for id in held {
            self.unhold(id);
        }
    }
}

pub(super) fn present(arg: u64, len: u64) -> Result<u64, i64> {
    let mut a: abi::Present = read_arg(arg, len)?;
    if a.fence != 0 || a.answer != 0 || a.reserved != 0 {
        return Err(EINVAL);
    }
    let tgid = caller()?;
    // The fence is the caller's from the send on, even if the wait is interrupted.
    let (fence, status, answer) = present_as(tgid, a.context, a.target, &|fence| write_arg(arg, &abi::Present { fence, ..a }))?;
    a.fence = fence;
    a.answer = answer;
    write_arg(arg, &a)?;
    if status == fence_status::OK {
        Ok(0)
    } else {
        Err(EIO)
    }
}

/// Bind `target` of `tgid` to the scanout in `context` and wait for its flush: the fence,
/// its status and the device's answer. `on_sent` receives the fence once the bind is queued;
/// its error, or an interrupted wait, leaves the queued present to complete on its own.
pub(super) fn present_as(tgid: u64, context: u64, target: u64, on_sent: &dyn Fn(u64) -> Result<(), i64>) -> Result<(u64, u32, u32), i64> {
    let a = abi::Present { context, target, ..abi::Present::ZERO };
    if !owner(tgid) {
        return Err(EPERM);
    }
    // Wait for the target's writers and the context's room, then send the bind and the
    // flush in the same critical section that checks ownership: SET_SCANOUT is queued only
    // while the caller owns the display.
    let sent = wait_for(None, true, |c| {
        if !owner(tgid) {
            return Some(Err(EPERM));
        }
        if c.display.changing {
            return None;
        }
        let Some(ctx_id) = c.ctx_of(tgid, a.context) else {
            return Some(Err(EINVAL));
        };
        let Some(id) = c.res_of(tgid, a.target) else {
            return Some(Err(EINVAL));
        };
        let Some(r) = c.resources.get(&id) else {
            return Some(Err(EINVAL));
        };
        if r.kind != kind::TARGET || !r.scanout {
            return Some(Err(EINVAL));
        }
        if r.lost || r.dropped {
            return Some(Err(EIO));
        }
        let (width, height, writers) = (r.width, r.height, r.writers);
        let Some(ctx) = c.contexts.get(&ctx_id) else {
            return Some(Err(EINVAL));
        };
        if ctx.state != CtxState::Live || !ctx.device_alive || ctx.dropped {
            return Some(Err(EIO));
        }
        if writers > 0 {
            return None;
        }
        if ctx.inflight.len() >= limits::IN_FLIGHT {
            return None;
        }
        if ctx.busy {
            return None;
        }
        let r = c.resources.get(&id)?;
        if r.invalid {
            return Some(Err(EIO));
        }
        // A target without a completed write has no contents to show.
        if r.last.is_none() {
            return Some(Err(EINVAL));
        }
        if c.queue_size < 2 {
            return Some(Err(ENOMEM));
        }
        let other = c.procs.get(&tgid).and_then(|p| {
            p.resources
                .values()
                .copied()
                .filter(|&rid| rid != id)
                .filter(|rid| {
                    c.resources
                        .get(rid)
                        .is_some_and(|r| r.scanout && !r.lost && !r.dropped)
                })
                .min()
        });
        let wrong =
            other.and_then(|_| crate::test_control::reserve(test_point::WRONG_SCANOUT, a.target));
        let id = if wrong.is_some() {
            other.unwrap_or(id)
        } else {
            id
        };
        if c.resources.get(&id).is_some_and(|r| r.writers != 0) {
            return None;
        }
        if let Some(wrong) = wrong {
            wrong.fire();
        }
        let (width, height) = c
            .resources
            .get(&id)
            .map(|r| (r.width, r.height))
            .unwrap_or((width, height));
        let skip = crate::test_control::take(test_point::SKIP_PRESENT_FLUSH, a.target).is_some();
        if skip {
            if c.display.holds.insert(id) {
                if let Some(r) = c.resources.get_mut(&id) {
                    r.holds += 1;
                }
            }
            c.display.known = false;
            c.display.latest_ok = false;
            if let Some(req) = dev::set_scanout(Tag::Background, ctx_id, id, width, height) {
                c.background(req);
            }
            return Some(Err(EIO));
        }
        let serial = c.serial();
        let fence = c.device_fence();
        let bind = dev::set_scanout(Tag::Submit { ctx: ctx_id, serial, last: false }, ctx_id, id, width, height);
        let flush = dev::flush_resource(Tag::Submit { ctx: ctx_id, serial, last: true }, ctx_id, fence, id, width, height);
        let (Some(bind), Some(flush)) = (bind, flush) else {
            return Some(Err(EIO));
        };
        c.display.sent += 1;
        c.display.outstanding += 1;
        c.display.known = false;
        let seq = c.display.sent;
        let ctx = c.contexts.get_mut(&ctx_id)?;
        ctx.issued += 1;
        let n = ctx.issued;
        let bgl_fence = (ctx.ns as u64) << 32 | n as u64;
        ctx.inflight.push_back(serial);
        ctx.flushes.register(n);
        // The sub holds the target until the device returns both requests; the binding
        // holds it until a later bind's flush completes or a confirmed reset.
        if let Some(r) = c.resources.get_mut(&id) {
            r.holds += 1;
        }
        if c.display.holds.insert(id) {
            if let Some(r) = c.resources.get_mut(&id) {
                r.holds += 1;
            }
        }
        c.subs.insert(
            serial,
            Sub {
                writer: None,
                ctx: ctx_id,
                n,
                fence,
                native_fence: 0,
                sent_ns: None,
                error: None,
                mismatch: false,
                requests: 2,
                published: false,
                result: None,
                written: Vec::new(),
                whole: Vec::new(),
                held: alloc::vec![id],
                present: Some(Presenting {
                    id,
                    seq,
                    owner: tgid,
                }),
                _charge: None,
            },
        );
        let epoch = c.epoch;
        c.backlog.push_back((epoch, bind));
        c.backlog.push_back((epoch, flush));
        c.flush();
        Some(Ok((ctx_id, n, bgl_fence)))
    });
    let (ctx_id, n, bgl_fence) = match sent {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(EINTR),
    };
    let forget = || {
        with_core(|c| {
            if let Some(ctx) = c.contexts.get_mut(&ctx_id) {
                ctx.flushes.take(n);
            }
        });
    };
    if let Err(e) = on_sent(bgl_fence) {
        forget();
        return Err(e);
    }
    let outcome = wait_for(None, true, |c| {
        let Some(ctx) = c.contexts.get_mut(&ctx_id) else {
            return Some((fence_status::DEVICE_RESET, 0));
        };
        if n > ctx.done_upto {
            return None;
        }
        let answer = ctx.flushes.take(n);
        Some(
            ctx.outcomes
                .get(&n)
                .copied()
                .unwrap_or_else(|| match answer {
                    Some(answer) => (fence_status::OK, answer),
                    None => (fence_status::DEVICE_ERROR, 0),
                }),
        )
    });
    let outcome = match outcome {
        Ok(o) => o,
        Err(_) => {
            forget();
            return Err(EINTR);
        }
    };
    Ok((bgl_fence, outcome.0, outcome.1))
}

pub(super) fn modes(arg: u64, len: u64) -> Result<u64, i64> {
    let input: abi::Modes = read_arg(arg, len)?;
    if input != abi::Modes::ZERO {
        return Err(EINVAL);
    }
    let mut a = with_core(|c| {
        let mut a = abi::Modes::ZERO;
        a.version = 1;
        a.size = 256;
        a.count = 2;
        a.current = c.display.mode;
        a.max_width = c.caps.max_width;
        a.max_height = c.caps.max_height;
        a.owner_depth = c.display.stack.len() as u32;
        a.binding_known = c.display.known as u32;
        a.modes = MODES;
        a.scanout_resource = c
            .display
            .binding
            .filter(|_| c.display.known)
            .and_then(|b| dev::resource_id(b.id))
            .map_or(0, u64::from);
        a
    })
    .ok_or(ENODEV)?;
    super::console::describe(&mut a);
    write_arg(arg, &a)?;
    Ok(0)
}
pub(super) fn set_mode(arg: u64, len: u64) -> Result<u64, i64> {
    let tgid = caller()?;
    if !owner(tgid) {
        return Err(EPERM);
    }
    let a: abi::SetMode = read_arg(arg, len)?;
    if a.index > 1 || a.reserved != [0; 3] {
        return Err(EINVAL);
    }
    with_core(|c| {
        if !owner(tgid) {
            return Err(EPERM);
        }
        c.display.mode = a.index;
        if let Some(o) = c.display.stack.last_mut() {
            o.mode = a.index;
        }
        Ok(0)
    })
    .ok_or(ENODEV)?
}

pub(super) fn active() -> bool {
    with_core(|c| c.status == abi::status::ADMITTED).unwrap_or(false)
}
fn publish_owner(c: &mut Core) {
    c.display.owner_sequence += 1;
    let (group, pid, mode) = c
        .display
        .stack
        .last()
        .map_or((u64::MAX, u64::MAX, KERNEL_MODE as u32), |o| (o.group, o.pid, o.mode));
    c.display.mode = mode;
    crate::syscall::graphics::DISPLAY_OWNER_GROUP.store(group, Ordering::Release);
    crate::syscall::graphics::DISPLAY_OWNER_PID.store(pid, Ordering::Release);
}

pub(super) struct Transition;
impl Transition {
    pub(super) fn for_observation(group: u64) -> Result<Self, i64> {
        let guard = Self::acquire()?;
        if !owner(group) {
            return Err(EPERM);
        }
        Ok(guard)
    }
    /// A transition for SCANOUT_READBACK: the window layer authorized the observer, which
    /// need not own the display; the binding cannot change while it is held.
    pub(super) fn for_window_observer() -> Result<Self, i64> {
        Self::acquire()
    }
    /// A transition for the kernel's screen: while no process owns the display, or while
    /// the owner's terminal is released and the kernel draws the foreground one (contract
    /// section 4.2).
    pub(super) fn for_console() -> Option<Self> {
        with_core(|c| {
            if c.display.changing
                || c.display.outstanding != 0
                || !(c.display.stack.is_empty() || crate::console::vt::presentation_suspended())
            {
                return None;
            }
            c.display.changing = true;
            Some(Self)
        })
        .flatten()
    }
    fn acquire() -> Result<Self, i64> {
        wait_for(None, true, |c| {
            if c.display.changing || c.display.outstanding > 0 {
                return None;
            }
            c.display.changing = true;
            Some(())
        })
        .map_err(|_| EINTR)?;
        Ok(Self)
    }
}
impl Drop for Transition {
    fn drop(&mut self) {
        with_core(|c| c.display.changing = false);
        notify();
    }
}

pub(super) fn take(group: u64, pid: u64, vt: usize) -> Result<(), i64> {
    // Refuse before changing VT custody. Hold a transition while installing both
    // owners. Take the console lock after releasing the core lock.
    let acquired = wait_for(None, false, |c| {
        if c.display.stack.last().is_some_and(|o| o.group == group) {
            return Some(Ok(false));
        }
        if c.display.stack.len() >= 4 || c.display.stack.iter().any(|o| o.group == group) {
            return Some(Err(EBUSY));
        }
        if c.display.changing || c.display.outstanding != 0 {
            return None;
        }
        c.display.changing = true;
        Some(Ok(true))
    }).unwrap_or(Err(ENODEV))?;
    if !acquired {
        // Repeated takeover still owes the foreground-VT check.
        return crate::console::vt::take_display(group, vt).map_err(|e| e as i64);
    }
    let _transition = Transition;
    crate::console::vt::take_display_nested(group, vt).map_err(|e| e as i64)?;
    with_core(|c| {
        c.display.stack.push(Owner {
            group,
            pid,
            mode: c.display.mode,
            last: None,
            pane: None,
        });
        publish_owner(c);
    });
    Ok(())
}

/// `GIVE_DISPLAY` (414 op 27, breengel.md section 18.3): the current owner `giver` hands the
/// display to process `pid`, which becomes owner directly above it. The giver's mode,
/// binding and backing stay on the stack; no compositor claim moves with the display. The
/// target is resolved once to its live thread group, so a later process given the same
/// number is not the owner.
pub(super) fn give(giver: u64, pid: u32) -> Result<(), i64> {
    const ESRCH: i64 = 3;
    let target = crate::process::with_process_manager(|m| {
        let p = m.get_process(crate::process::ProcessId::new(pid as u64))?;
        (!p.is_terminated()).then(|| p.thread_group_id.unwrap_or(pid as u64))
    })
    .flatten()
    .ok_or(ESRCH)?;
    // Refuse before changing VT custody; hold a transition while installing the owner.
    wait_for(None, true, |c| {
        if !owner(giver) {
            return Some(Err(EPERM));
        }
        if target == giver || c.display.stack.iter().any(|o| o.group == target) || c.display.stack.len() >= 4 {
            return Some(Err(EBUSY));
        }
        if c.display.changing || c.display.outstanding != 0 {
            return None;
        }
        if c.display.stack.try_reserve(1).is_err() {
            return Some(Err(ENOMEM));
        }
        c.display.changing = true;
        Some(Ok(()))
    })
    .map_err(|_| EINTR)??;
    let _transition = Transition;
    // The display goes with the giver's terminal.
    let vt = crate::console::vt::display_owner().map_or(1, |(_, vt)| vt);
    crate::console::vt::take_display_nested(target, vt).map_err(|e| e as i64)?;
    with_core(|c| {
        let mode = c.display.mode;
        c.display.stack.push(Owner { group: target, pid: pid as u64, mode, last: None, pane: None });
        publish_owner(c);
    });
    crate::process::with_process_manager(|m| {
        if let Some(p) = m.get_process_mut(crate::process::ProcessId::new(pid as u64)) {
            p.has_display_ownership = true;
        }
    });
    Ok(())
}

pub(super) fn back(group: u64) -> Result<(), i64> {
    let _transition = Transition::acquire()?;
    let removed = with_core(|c| {
        if !owner(group) {
            return Err(EPERM);
        }
        let o = c.display.stack.pop().ok_or(EPERM)?;
        Ok(o)
    })
    .ok_or(ENODEV)??;
    match restore() {
        Ok(()) => {
            if with_core(|c| !c.display.stack.is_empty()).unwrap_or(false) {
                crate::console::vt::give_back_display();
            }
            with_core(|c| {
                publish_owner(c);
                if let Some(id) = removed.pane {
                    c.unhold(id);
                }
                if let Some(b) = removed.last {
                    c.unhold(b.id);
                }
            });
            Ok(())
        }
        Err(e) => {
            with_core(|c| {
                c.display.stack.push(removed);
                c.display.known = false;
                super::console::request_restore(c.display.owner_sequence);
            });
            Err(e)
        }
    }
}

/// Remove an ended owner and ask the render thread to restore a displaced frame.
pub(super) fn ended(c: &mut Core, group: u64) {
    let was_top = c.display.stack.last().is_some_and(|o| o.group == group);
    if let Some(i) = c.display.stack.iter().position(|o| o.group == group) {
        let o = c.display.stack.remove(i);
        if let Some(id) = o.pane {
            c.unhold(id);
        }
        if let Some(b) = o.last {
            c.unhold(b.id);
        }
        if was_top {
            publish_owner(c);
            c.display.known = false;
            super::console::request_restore(c.display.owner_sequence);
        }
    }
}

pub(super) fn restore() -> Result<(), i64> {
    let (saved, current, owned) = with_core(|c| {
        let current = c.display.binding.filter(|_| c.display.known);
        (c.display.stack.last().and_then(|o| o.last), current, !c.display.stack.is_empty())
    })
    .unwrap_or((None, None, false));
    match saved {
        // The top owner presented after the exit that asked for this restore, and that
        // frame is already bound and complete. Binding it again would only make the
        // binding unknown meanwhile, refusing the owner's readbacks with reason 10.
        Some(b) if current.is_some_and(|c| c.id == b.id) => Ok(()),
        Some(b) => super::screen::bind_saved(b),
        // An owner that has not presented yet sees the kernel's screen as it was left.
        None if owned => super::screen::show(),
        // No owner: the screen console has the display again (ruling F25).
        None => super::screen::restore(),
    }
}

/// Publish a synchronous restore only after its flush echoed the requested fence.
pub(super) fn restored(b: Binding) {
    with_core(|c| {
        let old: Vec<u32> = c
            .display
            .holds
            .iter()
            .copied()
            .filter(|&id| id != b.id)
            .collect();
        if c.display.holds.insert(b.id) {
            if let Some(r) = c.resources.get_mut(&b.id) {
                r.holds += 1;
            }
        }
        c.display.binding = Some(b);
        c.display.known = true;
        c.display.latest_ok = true;
        for id in old {
            c.display.holds.remove(&id);
            c.unhold(id);
        }
    });
}

pub(super) fn restore_pending() -> Result<(), i64> {
    let _transition = Transition::acquire()?;
    // Owner exit publishes its request under the core lock too: deciding that a request
    // is obsolete and clearing it must not straddle publication of the next request.
    let seq = with_core(|c| {
        let seq = c.display.owner_sequence;
        if super::console::restoring(seq) {
            Some(seq)
        } else {
            super::console::restored_pending();
            None
        }
    }).ok_or(ENODEV)?;
    let Some(seq) = seq else { return Ok(()) };
    let outcome = restore();
    with_core(|c| {
        if outcome.is_ok() && c.display.owner_sequence == seq {
            super::console::restored_pending();
        }
    });
    outcome
}
