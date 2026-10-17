//! G2's minimal scanout binding. No mode policy or ownership stack (G7).
use super::*;
use abi::errno::EPERM;

#[derive(Clone, Copy)]
pub(super) struct Binding {
    pub id: u32,
    pub fence: u64,
    pub answer: u32,
}

pub(super) struct State {
    pub binding: Option<Binding>,
    /// All resources a sent bind could have installed, until a later completed flush.
    pub holds: BTreeSet<u32>,
    pub known: bool,
    pub busy: bool,
    pub sync: Option<u64>,
    pub abandoned: bool,
}
impl State {
    pub const fn new() -> Self {
        Self { binding: None, holds: BTreeSet::new(), known: true, busy: false, sync: None, abandoned: false }
    }
}
fn owner(tgid: u64) -> bool {
    crate::syscall::graphics::DISPLAY_OWNER_PID.load(Ordering::Acquire) == tgid
}

pub(super) fn readable(tgid: u64, id: u32) -> Result<(), i64> {
    with_core(|c| {
        if c.display.holds.contains(&id) {
            if !owner(tgid) { return Err(EPERM); }
            if !c.display.known { return Err(EIO); }
        }
        Ok(())
    }).ok_or(ENODEV)?
}
pub(super) fn binding(tgid: u64, id: u32) -> Result<Option<(u64, u32)>, i64> {
    readable(tgid, id)?;
    with_core(|c| c.display.binding.filter(|b| b.id == id).map(|b| (b.fence, b.answer))).ok_or(ENODEV)
}

struct Guard(u32);
impl Drop for Guard {
    fn drop(&mut self) {
        with_core(|c| {
            c.display.abandoned = true;
            if c.display.sync.is_none() { c.display.busy = false; }
            if !c.syncs.values().any(|s| s.reading == Some(self.0)) {
                if let Some(r) = c.resources.get_mut(&self.0) { r.reading = false; }
                c.free_if_unused(self.0);
            }
        });
        notify();
    }
}

pub(super) fn present(arg: u64, len: u64) -> Result<u64, i64> {
    let mut a: abi::Present = read_arg(arg, len)?;
    if a.fence != 0 || a.answer != 0 || a.reserved0 != 0 || a.reserved != 0 { return Err(EINVAL); }
    let tgid = caller()?;
    if !owner(tgid) { return Err(EPERM); }
    probe_output(arg, core::mem::size_of::<abi::Present>())?;
    let (id, ctx, width, height) = wait_for(None, true, |c| {
        if !owner(tgid) { return Some(Err(EPERM)); }
        let Some(id) = c.res_of(tgid, a.target) else { return Some(Err(EINVAL)) };
        let Some(r) = c.resources.get(&id) else { return Some(Err(EINVAL)) };
        if r.kind != kind::TARGET { return Some(Err(EINVAL)); }
        if r.lost || r.dropped { return Some(Err(EIO)); }
        if r.writers > 0 || r.reading || c.display.busy { return None; }
        if r.invalid { return Some(Err(EIO)); }
        let live = |id: &u32| c.contexts.get(id).is_some_and(|x| x.owner == tgid && x.state == CtxState::Live && x.device_alive);
        let ctx = r.last.map(|l| l.ctx).filter(live).or_else(||
            c.procs.get(&tgid).and_then(|p| p.contexts.values().copied().find(live)));
        let Some(ctx) = ctx else { return Some(Err(EIO)); };
        let result = (id, ctx, r.width, r.height);
        c.display.busy = true;
        c.display.abandoned = false;
        c.resources.get_mut(&id).unwrap().reading = true;
        Some(Ok(result))
    }).map_err(|_| EINTR)??;
    let _guard = Guard(id);
    with_core(|c| {
        // A possible scanout binding is a device-access hold, independent of whether
        // its waiter lives to see the answer. It outlives handle/context destruction.
        if c.display.holds.insert(id) {
            if let Some(r) = c.resources.get_mut(&id) { r.holds += 1; }
        }
        c.display.known = false;
    });
    for flush in [false, true] {
        let (serial, fence) = with_core(|c| {
            let serial = c.serial();
            c.display.sync = Some(serial);
            (serial, c.device_fence())
        }).ok_or(ENODEV)?;
        let req = if flush { virgl::flush_resource(Tag::Sync(serial), ctx, fence, id, width, height) }
            else { virgl::set_scanout(Tag::Sync(serial), ctx, fence, id, width, height) };
        let result = exec_req(serial, req, ctx, &[id], Some(id));
        // exec_req may fail before it enqueues anything (for example a dead context).
        with_core(|c| { if !c.syncs.contains_key(&serial) { c.display.sync = None; } });
        let d = result?;
        if !virgl::ok(d.answer) || d.echoed != fence || d.resp.u32_at(wire::HDR_FLAGS) & wire::FLAG_FENCE == 0 {
            return Err(EIO);
        }
        if flush {
            a.fence = fence;
            a.answer = d.answer;
            with_core(|c| {
                c.display.binding = Some(Binding { id, fence, answer: d.answer });
                c.display.known = true;
                let old: Vec<u32> = c.display.holds.iter().copied().filter(|&rid| rid != id).collect();
                for rid in old { c.display.holds.remove(&rid); c.unhold(rid); }
            });
        }
    }
    write_arg(arg, &a)?;
    Ok(0)
}
