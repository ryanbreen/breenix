//! G2's minimal scanout binding (`docs/gui/gpu.md` section 1.7, `PRESENT` in
//! `docs/gui/gpu-abi.md`). No mode policy or ownership stack (G7).
//!
//! A `PRESENT` is a fenced operation of the context it names: SET_SCANOUT and a fenced
//! RESOURCE_FLUSH, numbered in that context's fence sequence with its `SUBMIT`s, so its
//! fence is a context fence `WAIT` accepts and record offset 184 carries.
use super::*;
use abi::errno::EPERM;

#[derive(Clone, Copy)]
pub(super) struct Binding {
    pub id: u32,
    /// The context fence of the `PRESENT` whose flush completed this binding.
    pub fence: u64,
    pub answer: u32,
}

pub(super) struct State {
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
        Self { binding: None, holds: BTreeSet::new(), known: true, sent: 0, outstanding: 0, latest_ok: true }
    }
}

/// What a `PRESENT` sub carries: the target it binds and its place among sent presents.
#[derive(Clone, Copy)]
pub(super) struct Presenting {
    pub id: u32,
    pub seq: u64,
}

/// Whether thread group `tgid` owns the display now. Ownership is recorded per thread
/// group with the take-over, so this reads one word and takes no lock.
pub(super) fn owner(tgid: u64) -> bool {
    crate::syscall::graphics::DISPLAY_OWNER_GROUP.load(Ordering::Acquire) == tgid
}

/// Whether `tgid` may observe target `id`: a possible scanout is the display owner's, and
/// only while its binding is known. EIO carries reason 10, binding-unknown.
pub(super) fn readable(c: &Core, tgid: u64, id: u32) -> Result<(), i64> {
    if c.display.holds.contains(&id) {
        if !owner(tgid) { return Err(EPERM); }
        if !c.display.known { return Err(EIO); }
    }
    Ok(())
}

/// The completed binding of `id`, for record v2's flags bit 0 and offsets 184 and 192.
pub(super) fn binding(c: &Core, tgid: u64, id: u32) -> Result<Option<Binding>, i64> {
    readable(c, tgid, id)?;
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
        if !(latest && ok) {
            return;
        }
        d.binding = Some(Binding { id: p.id, fence, answer });
        let old: Vec<u32> = d.holds.iter().copied().filter(|&rid| rid != p.id).collect();
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
    if !owner(tgid) { return Err(EPERM); }
    // Wait for the target's writers and the context's room, then send the bind and the
    // flush in the same critical section that checks ownership: SET_SCANOUT is queued only
    // while the caller owns the display.
    let sent = wait_for(None, true, |c| {
        if !owner(tgid) { return Some(Err(EPERM)); }
        let Some(ctx_id) = c.ctx_of(tgid, a.context) else { return Some(Err(EINVAL)) };
        let Some(id) = c.res_of(tgid, a.target) else { return Some(Err(EINVAL)) };
        let Some(r) = c.resources.get(&id) else { return Some(Err(EINVAL)) };
        if r.kind != kind::TARGET || !r.scanout { return Some(Err(EINVAL)); }
        if r.lost || r.dropped { return Some(Err(EIO)); }
        let (width, height, writers) = (r.width, r.height, r.writers);
        let Some(ctx) = c.contexts.get(&ctx_id) else { return Some(Err(EINVAL)) };
        if ctx.state != CtxState::Live || !ctx.device_alive || ctx.dropped { return Some(Err(EIO)); }
        if writers > 0 { return None; }
        if ctx.inflight.len() >= limits::IN_FLIGHT {
            return None;
        }
        if ctx.busy { return None; }
        let r = c.resources.get(&id)?;
        if r.invalid { return Some(Err(EIO)); }
        // A target without a completed write has no contents to show.
        if r.last.is_none() { return Some(Err(EINVAL)); }
        if c.queue_size < 2 { return Some(Err(ENOMEM)); }
        let serial = c.serial();
        let fence = c.device_fence();
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
        if let Some(r) = c.resources.get_mut(&id) { r.holds += 1; }
        if c.display.holds.insert(id) {
            if let Some(r) = c.resources.get_mut(&id) { r.holds += 1; }
        }
        c.subs.insert(serial, Sub {
            ctx: ctx_id,
            n,
            fence,
            sent_ns: None,
            error: None,
            mismatch: false,
            requests: 2,
            published: false,
            kept: false,
            result: None,
            written: Vec::new(),
            whole: Vec::new(),
            held: alloc::vec![id],
            present: Some(Presenting { id, seq }),
        });
        let epoch = c.epoch;
        c.backlog.push_back((epoch, virgl::set_scanout(Tag::Submit { ctx: ctx_id, serial, last: false }, ctx_id, id, width, height)));
        c.backlog.push_back((epoch, virgl::flush_resource(Tag::Submit { ctx: ctx_id, serial, last: true }, ctx_id, fence, id, width, height)));
        c.flush();
        Some(Ok((ctx_id, n, bgl_fence)))
    });
    let (ctx_id, n, bgl_fence) = match sent {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(EINTR),
    };
    // The fence is the caller's from here on, even if the wait below is interrupted.
    a.fence = bgl_fence;
    let forget = || {
        with_core(|c| {
            if let Some(ctx) = c.contexts.get_mut(&ctx_id) { ctx.flushes.take(n); }
        });
    };
    if let Err(e) = write_arg(arg, &a) { forget(); return Err(e); }
    let outcome = wait_for(None, true, |c| {
        let Some(ctx) = c.contexts.get_mut(&ctx_id) else { return Some((fence_status::DEVICE_RESET, 0)) };
        if n > ctx.done_upto { return None; }
        let answer = ctx.flushes.take(n);
        Some(ctx.outcomes.get(&n).copied().unwrap_or_else(|| match answer {
            Some(answer) => (fence_status::OK, answer),
            None => (fence_status::DEVICE_ERROR, 0),
        }))
    });
    let outcome = match outcome {
        Ok(o) => o,
        Err(_) => { forget(); return Err(EINTR); }
    };
    a.answer = outcome.1;
    write_arg(arg, &a)?;
    if outcome.0 == fence_status::OK { Ok(0) } else { Err(EIO) }
}
