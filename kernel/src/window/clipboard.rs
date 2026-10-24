//! W22 per-euid clipboard storage within a claim, retained after its setter exits.
//! User copies and process accounting run outside the registry lock. SET copies its
//! result before publishing, then rechecks the claim and serial under the registry.

use super::{events::kernel_event, mem, Caller, Registry, REGISTRY};
use alloc::{sync::Arc, vec::Vec};
use breengel_abi::errno::{E2BIG, EAGAIN, EFAULT, EINVAL, ENOENT, ENOMEM};
use breengel_abi::{
    event_kind, ClipboardGet, ClipboardSet, ClipboardSubscribe, CLIPBOARD_MAX_BYTES,
    CLIPBOARD_TEXT_UTF8,
};
use core::sync::atomic::{AtomicU64, Ordering};

// Kernel notices occupy a delivery namespace separate from compositor input.
static NEXT_NOTICE: AtomicU64 = AtomicU64::new(1 << 63);

struct Charge {
    limits: Arc<crate::process::limits::Limits>,
    bytes: u64,
}

impl Charge {
    fn new(caller: &Caller, bytes: u64) -> Result<Self, i32> {
        crate::process::with_process_manager(|m| {
            let (_, p) = m.find_process_by_thread(caller.tid).ok_or(ENOENT)?;
            if !p.reserve_clipboard_bytes(bytes) {
                return Err(ENOMEM);
            }
            Ok(Self {
                limits: p.limits.clone(),
                bytes,
            })
        })
        .unwrap_or(Err(ENOENT))
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.limits.release_clipboard_bytes(self.bytes);
    }
}

pub(super) struct Text {
    pages: Arc<mem::Mem>,
    len: usize,
    _charge: Charge,
}

pub(super) struct User {
    euid: u32,
    text: Option<Arc<Text>>,
    owner: Option<u64>,
    serial: u64,
}

pub struct State {
    users: Vec<User>,
    generation: u64,
}

impl State {
    pub const fn new() -> Self {
        Self {
            users: Vec::new(),
            generation: 0,
        }
    }

    fn user(&self, euid: u32) -> Option<&User> {
        self.users.iter().find(|u| u.euid == euid)
    }

    fn user_mut(&mut self, euid: u32) -> Result<&mut User, i32> {
        let index = match self.users.iter().position(|u| u.euid == euid) {
            Some(index) => index,
            None => {
                self.users.try_reserve(1).map_err(|_| ENOMEM)?;
                let index = self.users.len();
                self.users.push(User { euid, text: None, owner: None, serial: 1 });
                index
            }
        };
        Ok(&mut self.users[index])
    }
}

/// End the claim under its registry transaction; free storage after releasing the lock.
pub(super) fn clear(reg: &mut Registry) -> Option<Vec<User>> {
    reg.clipboard.generation += 1;
    for w in reg.windows.values_mut() {
        w.clipboard_subscribed = None;
    }
    Some(core::mem::take(&mut reg.clipboard.users))
}

pub(super) fn window_gone(reg: &mut Registry, id: u64) {
    for user in &mut reg.clipboard.users {
        if user.owner == Some(id) {
            user.owner = None;
        }
    }
}

fn authorized(reg: &Registry, caller: &Caller, id: u64) -> Result<(u64, u64), i32> {
    let w = reg.windows.get(&id).ok_or(ENOENT)?;
    if !w.registered || w.owner != caller.tgid {
        return Err(ENOENT);
    }
    reg.claim.map(|c| (c.epoch, reg.clipboard.generation)).ok_or(ENOENT)
}

fn notice(w: &mut super::Window, kind: u16) {
    let mut event = kernel_event(kind);
    event.serial = NEXT_NOTICE.fetch_add(1, Ordering::Relaxed);
    w.ring.push(event);
}

pub fn set(caller: &Caller, a: &mut ClipboardSet, out: u64) -> Result<u64, i32> {
    if a.reserved != 0 || a.mime != CLIPBOARD_TEXT_UTF8 {
        return Err(EINVAL);
    }
    let epoch = authorized(&REGISTRY.lock(), caller, a.id)?;
    if a.len > CLIPBOARD_MAX_BYTES {
        return Err(E2BIG);
    }
    let charge = Charge::new(caller, a.len)?;
    let pages = mem::Mem::new(a.len)?;
    let len = a.len as usize;
    let mut copied = Ok(());
    pages.chunks(0, len, |ptr, n, at| {
        if copied.is_ok() {
            copied = crate::syscall::userptr::read_user_bytes(ptr, a.ptr + at as u64, n)
                .map_err(|_| EFAULT);
        }
    });
    copied?;
    // Validate a bounded scratch buffer, preserving an incomplete UTF-8 suffix
    // between pages. Payload bytes stay in page frames.
    let mut scratch = [0u8; 4099];
    let mut tail = 0;
    let mut at = 0;
    while at < len {
        let n = (len - at).min(4096);
        pages.read(at as u64, &mut scratch[tail..tail + n]);
        let end = tail + n;
        tail = match core::str::from_utf8(&scratch[..end]) {
            Ok(_) => 0,
            Err(e) if e.error_len().is_none() => {
                let start = e.valid_up_to();
                scratch.copy_within(start..end, 0);
                end - start
            }
            Err(_) => return Err(EINVAL),
        };
        at += n;
    }
    if tail != 0 {
        return Err(EINVAL);
    }
    let text = Arc::try_new(Text {
        pages,
        len,
        _charge: charge,
    })
    .map_err(|_| ENOMEM)?;
    loop {
        let (serial, windows) = {
            let reg = REGISTRY.lock();
            if authorized(&reg, caller, a.id)? != epoch {
                return Err(ENOENT);
            }
            (reg.clipboard.user(caller.euid).map_or(1, |u| u.serial) + 1, reg.windows.len())
        };
        let mut wakes = Vec::new();
        wakes.try_reserve_exact(windows).map_err(|_| ENOMEM)?;
        a.serial = serial;
        crate::syscall::window::copy_out(out, a)?;
        // As for LIST (B46), consult current credentials outside the registry.
        // A process that changed euid cannot keep receiving its former user's notices.
        let same_uid: Vec<u64> = crate::process::with_process_manager(|m| {
            m.all_processes().iter()
                .filter(|p| !p.is_terminated() && p.cred.euid == caller.euid)
                .map(|p| p.thread_group_id.unwrap_or(p.id.as_u64()))
                .collect()
        }).ok_or(ENOENT)?;
        let (old, wakes) = {
            let mut reg = REGISTRY.lock();
            if authorized(&reg, caller, a.id)? != epoch {
                return Err(ENOENT);
            }
            // Another setter won while the user copy ran: assign a later serial.
            if reg.clipboard.user(caller.euid).map_or(1, |u| u.serial) + 1 != serial
                || reg.windows.len() > wakes.capacity() {
                continue;
            }
            let user = reg.clipboard.user_mut(caller.euid)?;
            let previous = user.owner;
            let old = user.text.replace(text.clone());
            user.owner = Some(a.id);
            user.serial = serial;
            for w in reg.windows.values_mut() {
                if !same_uid.contains(&w.owner) {
                    continue;
                }
                let lost = previous == Some(w.id) && w.id != a.id;
                let subscribed = w.clipboard_subscribed == Some(caller.euid);
                if lost {
                    notice(w, event_kind::CLIPBOARD_LOST);
                }
                if subscribed {
                    notice(w, event_kind::CLIPBOARD_CHANGED);
                }
                if lost || subscribed {
                    wakes.push(w.waits.clone());
                }
            }
            (old, wakes)
        };
        drop(old);
        for waits in wakes {
            waits.events.wake_up();
        }
        return Ok(0);
    }
}

pub fn get(caller: &Caller, a: &mut ClipboardGet, out: u64) -> Result<u64, i32> {
    if a.reserved != 0 || a.mime != CLIPBOARD_TEXT_UTF8 {
        return Err(EINVAL);
    }
    let (text, serial) = {
        let reg = REGISTRY.lock();
        authorized(&reg, caller, a.id)?;
        let user = reg.clipboard.user(caller.euid);
        let serial = user.map_or(1, |u| u.serial);
        if a.serial != 0 && a.serial != serial {
            return Err(EAGAIN);
        }
        (user.and_then(|u| u.text.clone()), serial)
    };
    let len = text.as_ref().map_or(0, |t| t.len);
    let query = a.dst == 0 && a.capacity == 0;
    if !query {
        if a.capacity < len as u64 {
            return Err(EINVAL);
        }
        mem::probe_writable(a.dst, len as u64)?;
    }
    a.len = len as u64;
    a.serial = serial;
    crate::syscall::window::copy_out(out, a)?;
    if !query {
        if let Some(text) = text {
            let mut copied = Ok(());
            text.pages.chunks(0, len, |ptr, n, at| {
                if copied.is_ok() {
                    copied = crate::syscall::userptr::write_user_bytes(a.dst + at as u64, ptr, n)
                        .map_err(|_| EFAULT);
                }
            });
            copied?;
        }
    }
    Ok(0)
}

pub fn subscribe(caller: &Caller, a: &ClipboardSubscribe) -> Result<u64, i32> {
    if a.reserved != 0 || a.flags > 1 {
        return Err(EINVAL);
    }
    let mut reg = REGISTRY.lock();
    authorized(&reg, caller, a.id)?;
    reg.windows
        .get_mut(&a.id)
        .ok_or(ENOENT)?
        .clipboard_subscribed = if a.flags == 1 { Some(caller.euid) } else { None };
    Ok(0)
}
