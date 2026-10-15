//! The window operations of system call 414 (sections 17 to 19), past argument copying.
//!
//! Each takes its argument structure, already copied in, and fills its output fields; the
//! system call copies them out. User memory an op reads or writes itself (a title, the LIST
//! array, events, capture destinations) is probed before any state changes, so an EFAULT has
//! no side effect.

use alloc::{sync::Arc, vec::Vec};

use breengel_abi::errno::{
    EAGAIN, EBUSY, EFAULT, EINTR, EINVAL, EMFILE, ENOENT, ENOMEM, EPERM, ETIMEDOUT,
};
use breengel_abi::{
    event_kind, limits, Acquire, Capture, Claim, Commit, Create, Geometry, List, MapBuffers,
    MapTree, Meta, NoChange, Plain, ReadEvents, Rect, Resize, Title, WinInfo, WindowEvent,
    WriteEvents, READ_EVENTS_BLOCK,
};

use crate::ipc::wait::{wait_on, IpcDeadline, WaitEnd};
use crate::task::waitqueue::WaitQueueHead;

use super::events::{kernel_event, now_ns, writable, Ring};
use super::mem::{self, Mem};
use super::present::{Pin, WindowPresent};
use super::{
    who, BufState, Buffer, Caller, Claim as Held, Damage, Remnant, Waits, Window, REGISTRY,
};

fn deadline(timeout_ns: u64) -> IpcDeadline {
    if timeout_ns == u64::MAX {
        IpcDeadline::Never
    } else {
        IpcDeadline::Monotonic(now_ns().saturating_add(timeout_ns))
    }
}

/// Wait on `queue` while `pending` holds, until `deadline`. EINTR for a signal (whether or not
/// its handler restarts calls), ETIMEDOUT at the deadline.
fn wait(queue: &WaitQueueHead, deadline: &IpcDeadline, pending: impl FnOnce() -> bool) -> Result<(), i32> {
    match wait_on(queue, deadline, pending) {
        WaitEnd::Woken => Ok(()),
        WaitEnd::TimedOut => Err(ETIMEDOUT),
        WaitEnd::Interrupted(_) => Err(EINTR),
    }
}

/// Release a pin on buffer `uid` of window `window`: a retired buffer with no pin left returns
/// to its owner, and the window's acquirers are woken.
pub fn unpin(window: u64, uid: u64) {
    let waits = {
        let mut reg = REGISTRY.lock();
        let Some(w) = reg.windows.get_mut(&window) else {
            return;
        };
        let Some(b) = w.buffer_mut(uid) else {
            return;
        };
        b.pins = b.pins.saturating_sub(1);
        if b.pins == 0 && b.state == BufState::Retired {
            b.state = BufState::Free;
            Some(w.waits.clone())
        } else {
            None
        }
    };
    if let Some(waits) = waits {
        waits.acquire.wake_up();
    }
}

fn valid_size(width: u32, height: u32) -> bool {
    (1..=limits::MAX_DIMENSION).contains(&width) && (1..=limits::MAX_DIMENSION).contains(&height)
}

/// Bytes of one buffer of a `width` x `height` window.
fn buffer_bytes(width: u32, height: u32) -> u64 {
    width as u64 * height as u64 * 4
}

/// One buffer, allocated and mapped read-write into the caller. ENOMEM when the mapping would
/// pass `RLIMIT_AS` or memory runs out.
fn new_buffer(reg_uid: u64, width: u32, height: u32) -> Result<Buffer, i32> {
    room_for(buffer_bytes(width, height))?;
    let mem = Mem::new(buffer_bytes(width, height))?;
    let addr = mem::map(&mem, true)?;
    crate::tracing::providers::window::count_buffer_alloc();
    Ok(Buffer {
        uid: reg_uid,
        mem,
        addr,
        state: BufState::Free,
        acquired: false,
        committing: false,
        base: 0,
        pins: 0,
    })
}

/// ENOMEM when mapping `bytes` more would take the caller past its `RLIMIT_AS`, checked
/// before the memory is allocated. Mapping checks it again.
fn room_for(bytes: u64) -> Result<(), i32> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ENOMEM)?;
    let bytes = bytes.div_ceil(4096) * 4096;
    let fits = crate::process::with_process_manager(|m| {
        m.find_process_by_thread(tid).is_some_and(|(_, p)| {
            p.mapped_bytes().saturating_add(bytes) <= p.limits.get(crate::process::limits::AS).soft
        })
    })
    .unwrap_or(false);
    if fits {
        Ok(())
    } else {
        Err(ENOMEM)
    }
}

fn uids(n: usize) -> Vec<u64> {
    let mut reg = REGISTRY.lock();
    (0..n).map(|_| reg.uid()).collect()
}

/// Undo mappings made for a window that will not exist.
fn unmap_all(buffers: &[Buffer], tree: Option<(&Mem, u64)>) {
    for b in buffers {
        mem::unmap(&b.mem, b.addr);
    }
    if let Some((t, addr)) = tree {
        mem::unmap(t, addr);
    }
}

/// `CREATE`.
pub fn create(caller: &Caller, a: &mut Create) -> Result<u64, i32> {
    if a.reserved != 0 || !valid_size(a.width, a.height) {
        return Err(EINVAL);
    }
    if !(limits::MIN_BUFFERS..=limits::MAX_BUFFERS).contains(&a.buffers) {
        return Err(EINVAL);
    }
    let tree_kib = if a.tree_kib == 0 { limits::DEFAULT_TREE_KIB } else { a.tree_kib };
    if tree_kib > limits::MAX_TREE_KIB {
        return Err(EINVAL);
    }
    sweep_dead_owners();
    let owned = |reg: &super::Registry| reg.windows.values().filter(|w| w.owner == caller.tgid).count();
    if owned(&REGISTRY.lock()) >= limits::MAX_WINDOWS_PER_PROCESS {
        return Err(EAGAIN);
    }
    let ring = Ring::new().ok_or(ENOMEM)?;
    let waits = Arc::try_new(Waits {
        acquire: WaitQueueHead::new(),
        events: Arc::try_new(WaitQueueHead::new()).map_err(|_| ENOMEM)?,
    })
    .map_err(|_| ENOMEM)?;
    let count = a.buffers as usize;
    let keys = uids(count);
    let mut buffers: Vec<Buffer> = Vec::new();
    buffers.try_reserve_exact(count).map_err(|_| ENOMEM)?;
    for &uid in keys.iter() {
        match new_buffer(uid, a.width, a.height) {
            Ok(b) => buffers.push(b),
            Err(e) => {
                unmap_all(&buffers, None);
                return Err(e);
            }
        }
    }
    let tree_len = count as u64 * tree_kib as u64 * 1024;
    let tree = match room_for(tree_len).and_then(|()| Mem::new(tree_len)) {
        Ok(t) => t,
        Err(e) => {
            unmap_all(&buffers, None);
            return Err(e);
        }
    };
    let tree_addr = match mem::map(&tree, true) {
        Ok(addr) => addr,
        Err(e) => {
            unmap_all(&buffers, None);
            return Err(e);
        }
    };
    // The commit point.
    let mut reg = REGISTRY.lock();
    if owned(&reg) >= limits::MAX_WINDOWS_PER_PROCESS {
        drop(reg);
        unmap_all(&buffers, Some((&tree, tree_addr)));
        return Err(EAGAIN);
    }
    let id = reg.next_window;
    reg.next_window += 1;
    a.id = id;
    a.stride = a.width * 4;
    a.buffer = [0; 3];
    for (i, b) in buffers.iter().enumerate() {
        a.buffer[i] = b.addr;
    }
    a.tree = tree_addr;
    a.tree_kib = tree_kib;
    reg.windows.insert(
        id,
        Window {
            id,
            owner: caller.tgid,
            registered: false,
            title: [0; limits::MAX_TITLE],
            title_len: 0,
            width: a.width,
            height: a.height,
            buffers,
            tree,
            tree_addr,
            tree_kib,
            front: None,
            commit_id: 0,
            front_commit: 0,
            pixel_gen: 0,
            log: Default::default(),
            log_floor: 0,
            x: 0,
            y: 0,
            z: 0,
            geometry: 0,
            meta: Meta::ZERO,
            ring,
            waits,
            holder: None,
            present: WindowPresent::new(),
        },
    );
    Ok(0)
}

/// Undo a `CREATE` whose output could not be copied out.
pub fn uncreate(caller: &Caller, id: u64) {
    let _ = destroy(caller, id);
}

/// `DESTROY`: remove the window and unmap its buffers from the owner.
pub fn destroy(caller: &Caller, id: u64) -> Result<u64, i32> {
    let w = {
        let mut reg = REGISTRY.lock();
        reg.window(id, caller, who::owner)?;
        let w = reg.windows.remove(&id).ok_or(ENOENT)?;
        reg.present.uncover(w.present.shown);
        w
    };
    unmap_all(&w.buffers, Some((&w.tree, w.tree_addr)));
    w.waits.acquire.wake_up();
    w.waits.events.wake_up();
    Ok(0)
}

/// Copy a title in and check it: at most 64 bytes of UTF-8.
fn title_in(t: &Title) -> Result<([u8; limits::MAX_TITLE], usize), i32> {
    if t.reserved != 0 || t.len as usize > limits::MAX_TITLE {
        return Err(EINVAL);
    }
    let mut title = [0u8; limits::MAX_TITLE];
    let len = t.len as usize;
    if len > 0 {
        crate::syscall::userptr::read_user_bytes(title.as_mut_ptr(), t.ptr, len).map_err(|_| EFAULT)?;
    }
    core::str::from_utf8(&title[..len]).map_err(|_| EINVAL)?;
    Ok((title, len))
}

/// `REGISTER` (`register` true) and `SET_TITLE`.
pub fn set_title(caller: &Caller, t: &Title, register: bool) -> Result<u64, i32> {
    let (title, len) = title_in(t)?;
    let mut reg = REGISTRY.lock();
    let w = reg.window(t.id, caller, who::owner)?;
    if register {
        w.registered = true;
    }
    w.title = title;
    w.title_len = len;
    Ok(0)
}

fn info(w: &Window, backend: u32) -> WinInfo {
    use breengel_abi::{geometry_flags as g, win_flags as f};
    let mut i = WinInfo::ZERO;
    i.id = w.id;
    i.owner = w.owner as u32;
    i.flags = (if w.registered { f::REGISTERED } else { 0 })
        | (if w.geometry & g::VISIBLE != 0 { f::VISIBLE } else { 0 })
        | (if w.geometry & g::MINIMIZED != 0 { f::MINIMIZED } else { 0 })
        | (if w.geometry & g::FOCUSED != 0 { f::FOCUSED } else { 0 });
    i.width = w.width;
    i.height = w.height;
    i.x = w.x;
    i.y = w.y;
    i.z = w.z;
    i.title_len = w.title_len as u32;
    i.title = w.title;
    i.commit_id = w.commit_id;
    i.pixel_gen = w.pixel_gen;
    i.backend = backend;
    i
}

/// `LIST`: every registered window, up to `capacity` copied out; returns how many there are.
pub fn list(_caller: &Caller, l: &List) -> Result<u64, i32> {
    if l.reserved != 0 {
        return Err(EINVAL);
    }
    sweep_dead_owners();
    let backend = super::present::backend_code();
    let entries: Vec<WinInfo> = {
        let reg = REGISTRY.lock();
        let mut v = Vec::new();
        v.try_reserve_exact(reg.windows.len()).map_err(|_| ENOMEM)?;
        v.extend(reg.windows.values().filter(|w| w.registered).map(|w| info(w, backend)));
        v
    };
    let n = entries.len().min(l.capacity as usize);
    let bytes = n * core::mem::size_of::<WinInfo>();
    mem::probe_writable(l.ptr, bytes as u64)?;
    // SAFETY: WinInfo is plain data without padding.
    let raw = unsafe { core::slice::from_raw_parts(entries.as_ptr() as *const u8, bytes) };
    mem::copy_out(l.ptr, raw)?;
    Ok(entries.len() as u64)
}

/// Pick a free buffer to hand out: one already holding the newest front first, else the
/// newest base.
fn pick_free(w: &Window) -> Option<usize> {
    w.buffers
        .iter()
        .enumerate()
        .filter(|(_, b)| b.state == BufState::Free && !b.acquired && !b.committing)
        .max_by_key(|(_, b)| (b.base == w.front_commit && w.front.is_some(), b.base))
        .map(|(i, _)| i)
}

/// The damage of every pixel commit after `base` up to the front, or `Full` when `base` is none
/// or any of those commits has left the log.
fn since(w: &Window, base: u64) -> Damage {
    if base == 0 || base < w.log_floor {
        return Damage::Full;
    }
    let mut d = Damage::none();
    for (id, damage) in w.log.iter() {
        if *id > base && *id <= w.front_commit {
            d.add(damage);
        }
    }
    d
}

/// A pause inside the replay copy, for `acquire-stall`: until `release-stall` fires or 1 s.
static STALL: WaitQueueHead = WaitQueueHead::new();

fn stall() {
    use breengel_abi::tc_point::RELEASE_STALL;
    let end = now_ns() + 1_000_000_000;
    while now_ns() < end {
        if crate::test_control::take(RELEASE_STALL, 0).is_some() {
            return;
        }
        let d = IpcDeadline::Monotonic(now_ns() + 2_000_000);
        if let WaitEnd::Interrupted(_) = wait_on(&STALL, &d, || true) {
            return;
        }
    }
}

/// Copy `rects` of a `stride`-byte-per-row image from `src` to `dst`, pausing half way when
/// `acquire-stall` is armed for `window`.
fn replay(window: u64, src: &Mem, dst: &Mem, stride: u32, rects: &[Rect]) {
    use breengel_abi::tc_point::ACQUIRE_STALL;
    let rows: u64 = rects.iter().map(|r| r.h.max(0) as u64).sum();
    let pause_at = crate::test_control::take(ACQUIRE_STALL, window).map(|_| rows / 2);
    let mut done = 0u64;
    for r in rects {
        for y in r.y..r.y + r.h {
            if pause_at == Some(done) {
                stall();
            }
            let offset = y as u64 * stride as u64 + r.x as u64 * 4;
            dst.copy_from(src, offset, r.w as usize * 4);
            done += 1;
        }
    }
    if pause_at.is_some_and(|at| at >= done) {
        stall();
    }
}

/// `ACQUIRE`: hand out a free buffer, replayed in the kernel to the newest front.
pub fn acquire(caller: &Caller, a: &mut Acquire) -> Result<u64, i32> {
    if a.reserved != 0 {
        return Err(EINVAL);
    }
    let until = deadline(a.timeout_ns);
    // Take a buffer, compute its replay set and pin the front it copies from.
    let (index, uid, dst, addr, base, set, source, stride, width, height) = loop {
        let waits = {
            let mut reg = REGISTRY.lock();
            let w = reg.window(a.id, caller, who::owner)?;
            if let Some(i) = pick_free(w) {
                let (stride, width, height, front_commit) = (w.stride(), w.width, w.height, w.front_commit);
                let set = match w.front {
                    Some(_) if w.buffers[i].base != front_commit => since(w, w.buffers[i].base),
                    _ => Damage::none(),
                };
                let source = match (w.front, set.is_empty()) {
                    (Some(f), false) => {
                        let fb = &mut w.buffers[f];
                        fb.pins += 1;
                        Some(Pin::new(w.id, fb.uid, fb.mem.clone()))
                    }
                    _ => None,
                };
                let b = &mut w.buffers[i];
                b.acquired = true;
                let base = if source.is_some() || w.front.is_some() { front_commit } else { b.base };
                break (i, b.uid, b.mem.clone(), b.addr, base, set, source, stride, width, height);
            }
            w.waits.clone()
        };
        let id = a.id;
        wait(&waits.acquire, &until, || {
            REGISTRY
                .lock()
                .windows
                .get(&id)
                .is_some_and(|w| pick_free(w).is_none())
        })?;
    };
    let rects = set.rects(width, height);
    if let Some(src) = source.as_ref() {
        replay(a.id, src.mem(), &dst, stride, &rects);
    }
    drop(source);
    // Publish the base, unless the window or the buffer went away during the copy.
    let alive = {
        let mut reg = REGISTRY.lock();
        match reg.windows.get_mut(&a.id).and_then(|w| w.buffer_mut(uid)) {
            Some(b) if b.acquired => {
                b.base = base;
                true
            }
            _ => false,
        }
    };
    if !alive {
        return Err(ENOENT);
    }
    if let Err(e) = mem::protect(&dst, addr, true) {
        unacquire(a.id, uid);
        return Err(e);
    }
    a.buffer = index as u32;
    a.replay_full = matches!(set, Damage::Full) as u32;
    a.base = base;
    a.replay_count = rects.len().min(limits::MAX_DAMAGE) as u32;
    a.replay = [Rect::default(); 16];
    for (i, r) in rects.iter().take(limits::MAX_DAMAGE).enumerate() {
        a.replay[i] = *r;
    }
    Ok(0)
}

/// Give back buffer `index`, which `ACQUIRE` took, when its result could not be delivered.
pub fn unacquire_index(window: u64, index: u32) {
    let uid = {
        let reg = REGISTRY.lock();
        reg.windows
            .get(&window)
            .and_then(|w| w.buffers.get(index as usize))
            .map(|b| b.uid)
    };
    if let Some(uid) = uid {
        unacquire(window, uid);
    }
}

/// Give back a buffer `ACQUIRE` took, when its result could not be delivered.
pub fn unacquire(window: u64, uid: u64) {
    let waits = {
        let mut reg = REGISTRY.lock();
        let Some(w) = reg.windows.get_mut(&window) else {
            return;
        };
        if let Some(b) = w.buffer_mut(uid) {
            b.acquired = false;
        }
        w.waits.clone()
    };
    waits.acquire.wake_up();
}

fn rect_in(r: &Rect, width: u32, height: u32) -> bool {
    r.w >= 0
        && r.h >= 0
        && r.x >= 0
        && r.y >= 0
        && r.x as i64 + r.w as i64 <= width as i64
        && r.y as i64 + r.h as i64 <= height as i64
}

/// `COMMIT`: publish an acquired buffer as the new front.
pub fn commit(caller: &Caller, c: &mut Commit, meta: Meta) -> Result<u64, i32> {
    if c.reserved != 0 || c.damage_count as usize > limits::MAX_DAMAGE {
        return Err(EINVAL);
    }
    let given: Vec<Rect> = c.damage[..c.damage_count as usize].to_vec();
    // Check, and claim the buffer for this commit.
    let (uid, buf_mem, addr) = {
        let mut reg = REGISTRY.lock();
        let w = reg.window(c.id, caller, who::owner)?;
        let (width, height) = (w.width, w.height);
        if c.buffer as usize >= w.buffers.len() || !given.iter().all(|r| rect_in(r, width, height)) {
            return Err(EINVAL);
        }
        let b = &mut w.buffers[c.buffer as usize];
        if !b.acquired || b.committing {
            return Err(EBUSY);
        }
        b.committing = true;
        (b.uid, b.mem.clone(), b.addr)
    };
    // The buffer is read-only to its owner before it becomes the front.
    if let Err(e) = mem::protect(&buf_mem, addr, false) {
        let mut reg = REGISTRY.lock();
        if let Some(b) = reg.windows.get_mut(&c.id).and_then(|w| w.buffer_mut(uid)) {
            b.committing = false;
        }
        return Err(e);
    }
    let waits = {
        let mut reg = REGISTRY.lock();
        let w = reg.windows.get_mut(&c.id).ok_or(ENOENT)?;
        let Some(index) = w.buffers.iter().position(|b| b.uid == uid) else {
            return Err(ENOENT);
        };
        let base = w.buffers[index].base;
        let mut damage = Damage::Rects(given);
        if w.front.is_none() {
            damage = Damage::Full;
        } else if base != w.front_commit {
            let missed = since(w, base);
            damage.add(&missed);
        }
        let damage = match damage {
            Damage::Rects(r) if r.len() > limits::MAX_DAMAGE => Damage::Rects(alloc::vec![super::bounds(&r)]),
            d => d,
        };
        w.commit_id += 1;
        let id = w.commit_id;
        if !damage.is_empty() {
            w.pixel_gen += 1;
        }
        let mut released = false;
        if let Some(f) = w.front.filter(|&f| f != index) {
            let old = &mut w.buffers[f];
            old.state = if old.pins > 0 { BufState::Retired } else { BufState::Free };
            released = old.state == BufState::Free;
        }
        let b = &mut w.buffers[index];
        b.state = BufState::Front;
        b.acquired = false;
        b.committing = false;
        b.base = id;
        w.front = Some(index);
        w.front_commit = id;
        if w.log.len() == limits::DAMAGE_LOG {
            if let Some((evicted, _)) = w.log.pop_front() {
                w.log_floor = evicted;
            }
        }
        let rects = damage.rects(w.width, w.height).len() as u64;
        w.present.pending.add(&damage);
        w.log.push_back((id, damage));
        w.meta = meta;
        c.commit_id = id;
        c.pixel_gen = w.pixel_gen;
        crate::tracing::providers::window::count_damage_rects(rects);
        released.then(|| w.waits.clone())
    };
    if let Some(waits) = waits {
        waits.acquire.wake_up();
    }
    Ok(0)
}

/// `COMMIT_NO_CHANGE`: new metadata against the current front's pixels.
pub fn no_change(caller: &Caller, n: &mut NoChange, meta: Meta) -> Result<u64, i32> {
    if n.reserved != 0 {
        return Err(EINVAL);
    }
    let mut reg = REGISTRY.lock();
    let w = reg.window(n.id, caller, who::owner)?;
    w.commit_id += 1;
    w.meta = meta;
    n.commit_id = w.commit_id;
    Ok(0)
}

/// Copy `height` rows of `width` BGRX pixels from `src` (`stride` bytes per row) to the user
/// at `dst`, with every X byte 0xFF.
fn pixels_out(src: Option<&Mem>, stride: u32, width: u32, height: u32, dst: u64) -> Result<(), i32> {
    let row_len = width as usize * 4;
    let mut row = Vec::new();
    row.try_reserve_exact(row_len).map_err(|_| ENOMEM)?;
    row.resize(row_len, 0);
    for y in 0..height as u64 {
        match src {
            Some(m) => m.read(y * stride as u64, &mut row),
            None => row.fill(0),
        }
        for px in row.chunks_exact_mut(4) {
            px[3] = 0xFF;
        }
        mem::copy_out(dst + y * row_len as u64, &row)?;
    }
    Ok(())
}

/// Copy `len` bytes at `offset` of `src` to the user at `dst`.
fn bytes_out(src: &Mem, offset: u64, len: u64, dst: u64) -> Result<(), i32> {
    let mut chunk = [0u8; 512];
    let mut done = 0u64;
    while done < len {
        let n = (len - done).min(chunk.len() as u64) as usize;
        src.read(offset + done, &mut chunk[..n]);
        mem::copy_out(dst + done, &chunk[..n])?;
        done += n as u64;
    }
    Ok(())
}

/// `CAPTURE`: a whole committed generation, and that same commit's tree page on request.
pub fn capture(caller: &Caller, c: &mut Capture) -> Result<u64, i32> {
    let (pin, tree, tree_offset, tree_len, width, height, stride, commit_id) = {
        let mut reg = REGISTRY.lock();
        let w = reg.window(c.id, caller, who::owner_compositor_or_root)?;
        let tree_len = w.tree_kib as u64 * 1024;
        if c.dst_len < buffer_bytes(w.width, w.height) || (c.tree_dst != 0 && c.tree_len < tree_len) {
            return Err(EINVAL);
        }
        let (pin, tree_offset, commit_id) = match w.front {
            Some(f) => {
                let b = &mut w.buffers[f];
                b.pins += 1;
                (Some(Pin::new(w.id, b.uid, b.mem.clone())), f as u64 * tree_len, w.front_commit)
            }
            None => (None, 0, 0),
        };
        (pin, w.tree.clone(), tree_offset, tree_len, w.width, w.height, w.stride(), commit_id)
    };
    let need = buffer_bytes(width, height);
    mem::probe_writable(c.dst, need)?;
    if c.tree_dst != 0 {
        mem::probe_writable(c.tree_dst, tree_len)?;
    }
    pixels_out(pin.as_ref().map(|p| p.mem()), stride, width, height, c.dst)?;
    if c.tree_dst != 0 {
        bytes_out(&tree, tree_offset, tree_len, c.tree_dst)?;
    }
    drop(pin);
    c.commit_id = commit_id;
    c.width = width;
    c.height = height;
    Ok(0)
}

/// `RESIZE`: new buffers of the new size, each holding the old front in the intersection.
/// A refusal tells the window `ResizeFailed` with the old size; success, `Resized` with the
/// new one, once the buffers have it.
pub fn resize(caller: &Caller, r: &mut Resize) -> Result<u64, i32> {
    let (old_w, old_h, count, front) = {
        let mut reg = REGISTRY.lock();
        let w = reg.window(r.id, caller, who::owner)?;
        let front = w.front.map(|f| w.buffers[f].mem.clone());
        (w.width, w.height, w.buffers.len(), front)
    };
    let refuse = |e: i32| {
        tell(r.id, |ev| {
            ev.kind = event_kind::RESIZE_FAILED;
            ev.width = old_w;
            ev.height = old_h;
        });
        Err(e)
    };
    if r.reserved != 0 || !valid_size(r.width, r.height) {
        return refuse(EINVAL);
    }
    let keys = uids(count);
    let mut buffers: Vec<Buffer> = Vec::new();
    if buffers.try_reserve_exact(count).is_err() {
        return refuse(ENOMEM);
    }
    for &uid in keys.iter() {
        match new_buffer(uid, r.width, r.height) {
            Ok(b) => buffers.push(b),
            Err(e) => {
                unmap_all(&buffers, None);
                return refuse(e);
            }
        }
    }
    if let Some(old) = front.as_ref() {
        let cols = old_w.min(r.width) as usize * 4;
        for y in 0..old_h.min(r.height) as u64 {
            let mut row = [0u8; 512];
            let mut done = 0usize;
            while done < cols {
                let n = (cols - done).min(row.len());
                old.read(y * old_w as u64 * 4 + done as u64, &mut row[..n]);
                for b in buffers.iter() {
                    b.mem.write(y * r.width as u64 * 4 + done as u64, &row[..n]);
                }
                done += n;
            }
        }
    }
    let (old, waits) = {
        let mut reg = REGISTRY.lock();
        let Some(w) = reg.windows.get_mut(&r.id) else {
            drop(reg);
            unmap_all(&buffers, None);
            return Err(ENOENT);
        };
        let had_front = w.front.is_some();
        for b in buffers.iter_mut() {
            b.base = if had_front { w.front_commit } else { 0 };
        }
        if had_front {
            buffers[0].state = BufState::Front;
            w.pixel_gen += 1;
        }
        r.stride = r.width * 4;
        r.buffer = [0; 3];
        for (i, b) in buffers.iter().enumerate() {
            r.buffer[i] = b.addr;
        }
        let old = core::mem::replace(&mut w.buffers, buffers);
        w.front = had_front.then_some(0);
        w.width = r.width;
        w.height = r.height;
        w.log.clear();
        w.log_floor = w.front_commit;
        w.present.pending = Damage::Full;
        let shown = w.present.shown;
        let mut ev = kernel_event(event_kind::RESIZED);
        ev.width = r.width;
        ev.height = r.height;
        w.ring.push(ev);
        let waits = w.waits.clone();
        reg.present.uncover(shown);
        (old, waits)
    };
    unmap_all(&old, None);
    waits.acquire.wake_up();
    waits.events.wake_up();
    Ok(0)
}

/// Queue a kernel-made event, filled by `fill`, on window `id`.
fn tell(id: u64, fill: impl FnOnce(&mut WindowEvent)) {
    let waits = {
        let mut reg = REGISTRY.lock();
        let Some(w) = reg.windows.get_mut(&id) else {
            return;
        };
        let mut ev = kernel_event(0);
        fill(&mut ev);
        w.ring.push(ev);
        w.waits.clone()
    };
    waits.events.wake_up();
}

/// `READ_EVENTS`: the owner takes up to `capacity` events, waiting for one when asked.
pub fn read_events(caller: &Caller, r: &mut ReadEvents) -> Result<u64, i32> {
    if r.flags & !READ_EVENTS_BLOCK != 0 {
        return Err(EINVAL);
    }
    let size = core::mem::size_of::<WindowEvent>();
    let capacity = (r.capacity as usize).min(limits::EVENT_RING + 1);
    mem::probe_writable(r.ptr, (r.capacity as u64).saturating_mul(size as u64))?;
    let until = deadline(r.timeout_ns);
    loop {
        let mut out = Vec::new();
        out.try_reserve_exact(capacity).map_err(|_| ENOMEM)?;
        let (lost, waits) = {
            let mut reg = REGISTRY.lock();
            let w = reg.window(r.id, caller, who::owner)?;
            (w.ring.take(&mut out, capacity), w.waits.clone())
        };
        if !out.is_empty() || capacity == 0 {
            // SAFETY: WindowEvent is plain data without padding.
            let raw = unsafe { core::slice::from_raw_parts(out.as_ptr() as *const u8, out.len() * size) };
            if let Err(e) = mem::copy_out(r.ptr, raw) {
                let mut reg = REGISTRY.lock();
                if let Some(w) = reg.windows.get_mut(&r.id) {
                    w.ring.untake(out, lost);
                }
                return Err(e);
            }
            r.lost = lost;
            return Ok(out.len() as u64);
        }
        if r.flags & READ_EVENTS_BLOCK == 0 {
            r.lost = 0;
            return Ok(0);
        }
        let id = r.id;
        wait(&waits.events, &until, || {
            REGISTRY
                .lock()
                .windows
                .get(&id)
                .is_some_and(|w| !w.ring.has_event())
        })?;
    }
}

/// `EVENT_FD` (the owner): a new descriptor that polls readable while the window has an event.
pub fn event_fd(caller: &Caller, id: u64) -> Result<u64, i32> {
    use crate::ipc::fd::{FdKind, FileDescriptor};
    let queue = {
        let mut reg = REGISTRY.lock();
        reg.window(id, caller, who::owner)?.waits.events.clone()
    };
    let file: Arc<dyn crate::ipc::object::IpcFile> =
        Arc::try_new(super::events::EventFile::new(id, queue)).map_err(|_| ENOMEM)?;
    let entry = FileDescriptor::try_opened(FdKind::Ipc(file), 0).map_err(|_| ENOMEM)?;
    crate::process::with_process_manager(|manager| {
        let (_, process) = manager.find_process_by_thread_mut(caller.tid).ok_or(breengel_abi::errno::ESRCH)?;
        process.fd_table.alloc_with_entry(entry).map_err(|_| EMFILE)
    })
    .ok_or(breengel_abi::errno::ESRCH)?
    .map(|fd| fd as u64)
}

/// `WRITE_EVENTS` (compositor only): all of the events, or none when one is malformed.
pub fn write_events(caller: &Caller, wr: &WriteEvents) -> Result<u64, i32> {
    if wr.reserved != 0 {
        return Err(EINVAL);
    }
    REGISTRY.lock().window(wr.id, caller, who::compositor)?;
    let count = wr.count as usize;
    if count > 4 * limits::EVENT_RING {
        return Err(EINVAL);
    }
    let size = core::mem::size_of::<WindowEvent>();
    let raw = mem::copy_in(wr.ptr, count * size)?;
    let mut events = Vec::new();
    events.try_reserve_exact(count).map_err(|_| ENOMEM)?;
    for chunk in raw.chunks_exact(size) {
        // SAFETY: the chunk is exactly one WindowEvent of plain data.
        let e: WindowEvent = unsafe { core::ptr::read_unaligned(chunk.as_ptr() as *const WindowEvent) };
        if !writable(&e) {
            return Err(EINVAL);
        }
        events.push(e);
    }
    let (queued, waits) = {
        let mut reg = REGISTRY.lock();
        let w = reg.window(wr.id, caller, who::compositor)?;
        let queued = events.into_iter().filter(|e| w.ring.push(*e)).count();
        (queued, w.waits.clone())
    };
    waits.events.wake_up();
    Ok(queued as u64)
}

/// `SET_GEOMETRY` (compositor only).
pub fn set_geometry(caller: &Caller, g: &Geometry) -> Result<u64, i32> {
    if g.flags & !0b111 != 0 {
        return Err(EINVAL);
    }
    let mut reg = REGISTRY.lock();
    let w = reg.window(g.id, caller, who::compositor)?;
    w.x = g.x;
    w.y = g.y;
    w.z = g.z;
    w.geometry = g.flags;
    Ok(0)
}

/// `MAP_COMMITTED` (owner or compositor): every buffer mapped read-only into the caller.
pub fn map_committed(caller: &Caller, m: &mut MapBuffers) -> Result<u64, i32> {
    if m.reserved != 0 {
        return Err(EINVAL);
    }
    let (mems, stride) = {
        let mut reg = REGISTRY.lock();
        let w = reg.window(m.id, caller, who::owner_or_compositor)?;
        let mems: Vec<Arc<Mem>> = w.buffers.iter().map(|b| b.mem.clone()).collect();
        (mems, w.stride())
    };
    let mut mapped: Vec<(Arc<Mem>, u64)> = Vec::new();
    for mem_i in mems.into_iter() {
        match mem::map(&mem_i, false) {
            Ok(addr) => mapped.push((mem_i, addr)),
            Err(e) => {
                for (mm, addr) in mapped.iter() {
                    mem::unmap(mm, *addr);
                }
                return Err(e);
            }
        }
    }
    m.buffer = [0; 3];
    for (i, (_, addr)) in mapped.iter().enumerate() {
        m.buffer[i] = *addr;
    }
    m.stride = stride;
    Ok(0)
}

/// `READ_META` (owner, compositor or root): the newest commit's id and the metadata it
/// published.
pub fn read_meta(caller: &Caller, id: u64) -> Result<(u64, Meta), i32> {
    let mut reg = REGISTRY.lock();
    let w = reg.window(id, caller, who::owner_compositor_or_root)?;
    Ok((w.commit_id, w.meta))
}

/// `MAP_TREE` (owner, compositor or root): the window's tree pages, read-only.
pub fn map_tree(caller: &Caller, m: &mut MapTree) -> Result<u64, i32> {
    let tree = {
        let mut reg = REGISTRY.lock();
        reg.window(m.id, caller, who::owner_compositor_or_root)?.tree.clone()
    };
    m.addr = mem::map(&tree, false)?;
    m.len = tree.len();
    Ok(0)
}

/// `CLAIM_COMPOSITOR` (root, with no live claim): a new epoch, told to every window.
pub fn claim(caller: &Caller, c: &mut Claim) -> Result<u64, i32> {
    if !caller.is_root() {
        return Err(EPERM);
    }
    if c.flags != 0 || c.reserved != 0 {
        return Err(EINVAL);
    }
    sweep_dead_owners();
    let woken = {
        let mut reg = REGISTRY.lock();
        if reg.claim.is_some() {
            return Err(EBUSY);
        }
        reg.epoch += 1;
        let epoch = reg.epoch;
        reg.claim = Some(Held {
            holder: caller.tgid,
            epoch,
        });
        c.epoch = epoch;
        let mut woken = Vec::new();
        let _ = woken.try_reserve_exact(reg.windows.len());
        for w in reg.windows.values_mut() {
            let mut ev = kernel_event(event_kind::COMPOSITOR_CHANGED);
            ev.epoch = epoch;
            w.ring.push(ev);
            woken.push(w.waits.clone());
        }
        woken
    };
    for waits in woken {
        waits.events.wake_up();
    }
    Ok(0)
}

/// `RELEASE_COMPOSITOR` (the compositor).
pub fn release(caller: &Caller) -> Result<u64, i32> {
    let mut reg = REGISTRY.lock();
    if !reg.is_compositor(caller) {
        return Err(EPERM);
    }
    reg.claim = None;
    Ok(0)
}

/// `HOLD_EVIDENCE` (root, neither owner nor compositor): become the window's evidence holder.
pub fn hold_evidence(caller: &Caller, id: u64) -> Result<u64, i32> {
    let mut reg = REGISTRY.lock();
    let w = reg.window(id, caller, |owner, compositor, c| c.is_root() && !owner && !compositor)?;
    match w.holder {
        Some(h) if h != caller.tgid => Err(EBUSY),
        _ => {
            w.holder = Some(caller.tgid);
            Ok(0)
        }
    }
}

/// `READ_REMNANT` (the holder): the remnant's pixels and tree, as `CAPTURE` gives them.
pub fn read_remnant(caller: &Caller, c: &mut Capture) -> Result<u64, i32> {
    if !caller.is_root() {
        return Err(EPERM);
    }
    let (pixels, tree, tree_offset, tree_len, width, height, commit_id) = {
        let reg = REGISTRY.lock();
        let r = reg.remnants.get(&c.id).filter(|r| r.holder == caller.tgid).ok_or(ENOENT)?;
        (r.pixels.clone(), r.tree.clone(), r.tree_offset, r.tree_len, r.width, r.height, r.commit_id)
    };
    if c.dst_len < buffer_bytes(width, height) || (c.tree_dst != 0 && c.tree_len < tree_len) {
        return Err(EINVAL);
    }
    mem::probe_writable(c.dst, buffer_bytes(width, height))?;
    if c.tree_dst != 0 {
        mem::probe_writable(c.tree_dst, tree_len)?;
    }
    pixels_out(pixels.as_deref(), width * 4, width, height, c.dst)?;
    if c.tree_dst != 0 {
        bytes_out(&tree, tree_offset, tree_len, c.tree_dst)?;
    }
    c.commit_id = commit_id;
    c.width = width;
    c.height = height;
    Ok(0)
}

/// `RELEASE_REMNANT` (the holder).
pub fn release_remnant(caller: &Caller, id: u64) -> Result<u64, i32> {
    if !caller.is_root() {
        return Err(EPERM);
    }
    let removed = {
        let mut reg = REGISTRY.lock();
        match reg.remnants.get(&id) {
            Some(r) if r.holder == caller.tgid => reg.remnants.remove(&id),
            _ => return Err(ENOENT),
        }
    };
    drop(removed);
    Ok(0)
}

/// The thread group `tgid` has exited (`at_exit`) or exec'd: its windows go, kept as remnants
/// for their evidence holders on exit; its claim ends; on exit, the remnants it held and its
/// evidence registrations go too.
pub fn group_gone(tgid: u64, at_exit: bool) {
    let mut gone: Vec<Window> = Vec::new();
    let mut freed: Vec<Remnant> = Vec::new();
    {
        let mut reg = REGISTRY.lock();
        let ids: Vec<u64> = reg.windows.values().filter(|w| w.owner == tgid).map(|w| w.id).collect();
        for id in ids {
            if let Some(w) = reg.windows.remove(&id) {
                reg.present.uncover(w.present.shown);
                if at_exit {
                    if let Some(holder) = w.holder.filter(|&h| h != tgid) {
                        let kept = reg.remnants.values().filter(|r| r.holder == holder).count();
                        if kept < limits::MAX_REMNANTS {
                            let pixels = w.front.map(|f| w.buffers[f].mem.clone());
                            let tree_len = w.tree_kib as u64 * 1024;
                            let tree_offset = w.front.map_or(0, |f| f as u64 * tree_len);
                            let charge = pixels.as_ref().map_or(0, |m| m.len()) + w.tree.len();
                            let remnant = Remnant {
                                holder,
                                width: w.width,
                                height: w.height,
                                commit_id: w.front_commit,
                                pixels,
                                tree: w.tree.clone(),
                                tree_offset,
                                tree_len,
                                charge,
                            };
                            reg.remnants.insert(id, remnant);
                        }
                    }
                }
                gone.push(w);
            }
        }
        if reg.claim.is_some_and(|c| c.holder == tgid) {
            reg.claim = None;
        }
        if at_exit {
            let held: Vec<u64> = reg.remnants.iter().filter(|(_, r)| r.holder == tgid).map(|(id, _)| *id).collect();
            for id in held {
                if let Some(r) = reg.remnants.remove(&id) {
                    freed.push(r);
                }
            }
            for w in reg.windows.values_mut() {
                if w.holder == Some(tgid) {
                    w.holder = None;
                }
            }
        }
    }
    for w in gone.iter() {
        w.waits.acquire.wake_up();
        w.waits.events.wake_up();
    }
    drop(gone);
    drop(freed);
}

/// Destroy the windows of owners with no live thread left (a backstop for the exit hooks).
pub fn sweep_dead_owners() {
    let owners: Vec<u64> = {
        let reg = REGISTRY.lock();
        let mut v: Vec<u64> = reg.windows.values().map(|w| w.owner).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    if owners.is_empty() {
        return;
    }
    let dead: Vec<u64> = crate::process::with_process_manager(|m| {
        owners
            .iter()
            .copied()
            .filter(|&tgid| {
                !m.all_processes().iter().any(|p| {
                    p.thread_group_id.unwrap_or(p.id.as_u64()) == tgid && !p.is_terminated()
                })
            })
            .collect()
    })
    .unwrap_or_default();
    for tgid in dead {
        group_gone(tgid, true);
    }
}
