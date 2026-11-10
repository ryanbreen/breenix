//! User filesystem mounts and the record-oriented daemon channel.
//!
//! The mount owns request state independently of the endpoint. Descriptor close
//! publishes death even when an in-flight read retains the endpoint. The close
//! hook links an owned wake record; reclaim delivers it outside PROCESS_MANAGER.

mod args;
pub mod file;
mod io_split;
mod node_gates;
pub mod cover;
pub mod path;
pub mod table;

use crate::ipc::object::IpcFile;
use crate::ipc::wait::{wait_on, IpcDeadline, WaitEnd};
use crate::syscall::{errno::*, userptr, SyscallResult};
use crate::task::waitqueue::WaitQueueHead;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
use table::{Error, RequestTable};

type Answer = Result<Vec<u8>, u64>;
struct Pending {
    answer: Mutex<Option<Answer>>,
}

struct State {
    requests: RequestTable<Vec<u8>, Arc<Pending>, Answer>,
    dead: bool,
    detached: bool,
    open_files: usize,
    releases: Option<alloc::boxed::Box<Release>>,
}

struct Release {
    bytes: Vec<u8>,
    pending: Arc<Pending>,
    next: Option<alloc::boxed::Box<Release>>,
}

/// The id is stable for the mount's lifetime, including daemon death.
pub struct Mount {
    pub id: u64,
    cover: cover::Cover<(crate::fs::namei::Mount, u32)>,
    point: crate::fs::namei::Resolved<Vec<u8>>,
    state: Mutex<State>,
    readers: Arc<WaitQueueHead>,
    callers: WaitQueueHead,
    deferred_next: Mutex<Option<Arc<Mount>>>,
    deferred_queued: AtomicBool,
    mutations: Mutex<node_gates::NodeGates<file::Gate>>,
}

static MOUNTS: Mutex<Vec<Arc<Mount>>> = Mutex::new(Vec::new());
static NEXT_MOUNT: AtomicU64 = AtomicU64::new(1);
// Enqueueing a wake on this owned list needs no allocation or scheduler access.
static DEFERRED: Mutex<Option<Arc<Mount>>> = Mutex::new(None);

fn now_ns() -> u64 {
    let (sec, ns) = crate::time::get_monotonic_time_ns();
    sec.saturating_mul(1_000_000_000).saturating_add(ns)
}

fn table_errno(error: Error) -> u64 {
    match error {
        Error::Full => EAGAIN as u64,
        Error::Eio => EIO as u64,
        Error::Enotconn => ENOTCONN as u64,
    }
}

fn finish((pending, answer): (Arc<Pending>, Result<Answer, Error>)) {
    *pending.answer.lock() = Some(answer.unwrap_or_else(|e| Err(table_errno(e))));
}

impl State {
    fn expire(&mut self) {
        for completion in self.requests.expire(now_ns()) {
            finish(completion);
        }
    }

    fn disconnect(&mut self) {
        self.dead = true;
        for completion in self.requests.dead() {
            finish(completion);
        }
    }
}

impl Mount {
    fn mutation_gate(&self, node: u64) -> Result<Arc<file::Gate>, u64> {
        self.mutations.lock().get_or_insert(node,
            || Arc::try_new(file::Gate::new()).map_err(|_| ENOMEM as u64), ENOMEM as u64)
    }
    fn defer(self: &Arc<Self>) {
        let mut head = DEFERRED.lock();
        if !self.deferred_queued.swap(true, Ordering::AcqRel) {
            *self.deferred_next.lock() = head.take();
            *head = Some(self.clone());
        }
    }
    pub fn path(&self) -> &[u8] {
        &self.point.path
    }

    fn wake(&self) {
        self.callers.wake_up();
        self.readers.wake_up();
    }
}

/// Enter a mount by the covered inode, even after an ancestor is renamed.
pub fn covering(mount: crate::fs::namei::Mount, ino: u32) -> Option<u64> {
    cover::enter(MOUNTS.lock().iter().map(|m| &m.cover), (mount, ino))
}

pub fn covered_directory(id: u64) -> Result<(crate::fs::namei::Mount, u32), u64> {
    Ok(get(id)?.cover.covered)
}

pub fn covered_parent(id: u64) -> Option<(crate::fs::namei::Mount, u32)> {
    cover::leave(MOUNTS.lock().iter().map(|m| &m.cover), id)
}

pub fn get(id: u64) -> Result<Arc<Mount>, u64> {
    MOUNTS
        .lock()
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .ok_or(ENOTCONN as u64)
}

/// One open user file description, shared by dup/fork with its IpcFile.
pub struct FileLease(Arc<Mount>);

pub fn open_file(id: u64) -> Result<FileLease, u64> {
    let mount = get(id)?;
    {
        let mut state = mount.state.lock();
        if state.dead || state.detached {
            return Err(ENOTCONN as u64);
        }
        state.open_files = state.open_files.checked_add(1).ok_or(EMFILE as u64)?;
    }
    Ok(FileLease(mount))
}

impl Drop for FileLease {
    fn drop(&mut self) {
        self.0.state.lock().open_files -= 1;
    }
}

/// Submit a framed operation; the table assigns its id, ignoring the input id.
/// No process, namespace or filesystem guard may be held across this call.
pub fn call(id: u64, request: ufs_abi::Request<'_>) -> Answer {
    let mount = get(id)?;
    if request.payload.len() > ufs_abi::MAX_DATA {
        return Err(EINVAL as u64);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(ufs_abi::REQUEST_HEADER + request.payload.len())
        .map_err(|_| ENOMEM as u64)?;
    bytes.resize(ufs_abi::REQUEST_HEADER + request.payload.len(), 0);
    request.encode(&mut bytes).map_err(|_| EINVAL as u64)?;
    let pending = Arc::try_new(Pending {
        answer: Mutex::new(None),
    })
    .map_err(|_| ENOMEM as u64)?;
    let deadline = now_ns().saturating_add(ufs_abi::payload::REQUEST_TIMEOUT_MS * 1_000_000);
    let submitted = {
        let mut state = mount.state.lock();
        state.expire();
        if state.detached {
            Err(ENOTCONN as u64)
        } else {
            state
                .requests
                .submit(bytes, pending.clone(), deadline)
                .map_err(table_errno)
        }
    };
    mount.wake();
    let request_id = submitted?;
    loop {
        if let Some(answer) = pending.answer.lock().take() {
            return answer;
        }
        match wait_on(&mount.callers, &IpcDeadline::Monotonic(deadline), || {
            pending.answer.lock().is_none()
        }) {
            WaitEnd::Woken => {}
            WaitEnd::TimedOut => {
                mount.state.lock().expire();
                mount.wake();
            }
            WaitEnd::Interrupted(errno) => {
                mount.state.lock().requests.cancel(request_id);
                mount.wake();
                return pending.answer.lock().take().unwrap_or(Err(errno));
            }
        }
    }
}

/// An unbound open description, bound at most once by mount(2).
pub struct Endpoint {
    binding: Mutex<Option<Arc<Mount>>>,
    readers: Arc<WaitQueueHead>,
    read_serial: file::Gate,
}

impl core::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("UfsEndpoint")
    }
}

impl Endpoint {
    pub fn open() -> Result<Arc<Self>, u64> {
        let readers = Arc::try_new(WaitQueueHead::new()).map_err(|_| ENOMEM as u64)?;
        Arc::try_new(Self {
            binding: Mutex::new(None),
            readers,
            read_serial: file::Gate::new(),
        })
        .map_err(|_| ENOMEM as u64)
    }

    fn disconnect(&self) {
        let Some(mount) = self.binding.lock().clone() else {
            return;
        };
        {
            let mut state = mount.state.lock();
            if state.dead {
                return;
            }
            state.disconnect();
        }
        mount.defer();
    }

    fn read_record(&self, buf: u64, count: usize, nonblocking: bool) -> Result<u64, u64> {
        if count == 0 {
            return Ok(0);
        }
        loop {
            let mount = self.binding.lock().clone();
            if let Some(mount) = mount {
                // Serialize readers while allowing timeout, close and replies to
                // proceed during a potentially faulting userspace copy.
                let serial = self.read_serial.enter()?;
                let record = (|| {
                    let mut state = mount.state.lock();
                    state.expire();
                    // Daemon reads move preallocated close records into the
                    // request table; the close hook only links these records.
                    while !state.dead && !state.detached && state.requests.has_capacity() {
                        let Some(mut release) = state.releases.take() else { break; };
                        state.releases = release.next.take();
                        let deadline = now_ns().saturating_add(ufs_abi::payload::REQUEST_TIMEOUT_MS * 1_000_000);
                        let _ = state.requests.submit(release.bytes, release.pending, deadline);
                    }
                    if state.detached {
                        return Ok(Some((0, Vec::new())));
                    }
                    if state.dead {
                        return Err(ENOTCONN as u64);
                    }
                    let Some((id, bytes)) = state.requests.peek_next() else {
                        return Ok(None);
                    };
                    if count < bytes.len() {
                        return Err(EMSGSIZE as u64);
                    }
                    let mut snapshot = Vec::new();
                    snapshot
                        .try_reserve_exact(bytes.len())
                        .map_err(|_| ENOMEM as u64)?;
                    snapshot.extend_from_slice(bytes);
                    snapshot[..8].copy_from_slice(&id.to_le_bytes());
                    Ok(Some((id, snapshot)))
                })();
                mount.callers.wake_up();
                if let Some((id, bytes)) = record? {
                    userptr::write_user_bytes(buf, bytes.as_ptr(), bytes.len())?;
                    mount.state.lock().requests.mark_sent(id);
                    return Ok(bytes.len() as u64);
                }
                drop(serial);
            }
            if nonblocking {
                return Err(EAGAIN as u64);
            }
            match wait_on(&self.readers, &IpcDeadline::Never, || {
                self.poll_events() & 1 == 0
            }) {
                WaitEnd::Interrupted(errno) => return Err(errno),
                WaitEnd::Woken | WaitEnd::TimedOut => {}
            }
        }
    }
}

impl IpcFile for Endpoint {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn on_close(&self, _pid: crate::process::ProcessId) {
        self.disconnect();
    }
    fn fill_stat(&self, st: &mut crate::syscall::fs::Stat) {
        st.st_dev = 0xffff0002;
        st.st_ino = crate::fs::devfs::DeviceType::Ufs.inode();
        st.st_mode = 0o020600;
        st.st_nlink = 1;
        st.st_blksize = ufs_abi::MAX_MESSAGE as _;
    }
    fn statfs_magic(&self) -> u64 {
        ufs_abi::payload::MAGIC as u64
    }
    fn poll_queue(&self) -> Option<Arc<WaitQueueHead>> {
        Some(self.readers.clone())
    }
    fn poll_events(&self) -> i16 {
        let binding = self.binding.lock();
        let Some(mount) = binding.as_ref() else {
            return 0;
        };
        let state = mount.state.lock();
        if state.detached || state.dead {
            return 1 | 0x10;
        } // POLLIN | POLLHUP
        4 | if state.requests.peek_next().is_some() || (state.releases.is_some() && state.requests.has_capacity()) {
            1
        } else {
            0
        } // POLLOUT
    }
    fn read(&self, buf: u64, count: usize) -> Result<u64, u64> {
        self.read_record(buf, count, false)
    }
    fn read_with_status(&self, buf: u64, count: usize, nonblocking: bool) -> Result<u64, u64> {
        self.read_record(buf, count, nonblocking)
    }
    fn write_buffer_size(&self, requested: usize) -> usize {
        requested.min(ufs_abi::MAX_MESSAGE + 1)
    }
    fn write(&self, bytes: &[u8], _append: bool) -> Result<u64, u64> {
        let reply = ufs_abi::Reply::decode(bytes).map_err(|_| EINVAL as u64)?;
        if !(-4095..=0).contains(&reply.errno) || (reply.errno != 0 && !reply.payload.is_empty()) {
            return Err(EINVAL as u64);
        }
        let mount = self.binding.lock().clone().ok_or(ENOTCONN as u64)?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(reply.payload.len())
            .map_err(|_| ENOMEM as u64)?;
        payload.extend_from_slice(reply.payload);
        let answer = if reply.errno == 0 {
            Ok(payload)
        } else {
            Err((-reply.errno) as u64)
        };
        let result = {
            let mut state = mount.state.lock();
            state.expire();
            if state.dead || state.detached {
                Err(ENOTCONN as u64)
            } else {
                if let Some(completion) = state.requests.complete(reply.id, answer) {
                    finish(completion);
                }
                Ok(bytes.len() as u64)
            }
        };
        mount.wake();
        result
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// Called by the reclaim path after refusing PM/scheduler contexts. The list
/// retains even a detached mount until its final close notifications run.
pub fn wake_deferred() {
    let mut next = DEFERRED.lock().take();
    while let Some(mount) = next {
        next = mount.deferred_next.lock().take();
        mount.deferred_queued.store(false, Ordering::Release);
        mount.wake();
    }
}

fn mount(target: u64, fstype: u64, flags: u64, data: u64) -> Result<(), u64> {
    let cred = crate::fs::permissions::Credentials::current(false)?;
    args::require_root(cred.euid)?;
    let fstype = userptr::copy_path_bytes_from_user(fstype)?;
    // Validate type/flags before touching the filesystem-specific pointer.
    if fstype != b"ufs" || flags != 0 {
        return Err(EINVAL as u64);
    }
    let fd = userptr::copy_from_user::<u32>(data as *const u32)?;
    let fd = args::parse(cred.euid, &fstype, flags, &fd.to_ne_bytes())?;
    let entry = crate::syscall::metadata::descriptor_entry(fd)?;
    if !entry.readable() || !entry.writable() {
        return Err(EBADF as u64);
    }
    let crate::ipc::FdKind::Ipc(object) = entry.kind else {
        return Err(EINVAL as u64);
    };
    let endpoint = object
        .as_any()
        .downcast_ref::<Endpoint>()
        .ok_or(EINVAL as u64)?;
    let raw = userptr::copy_path_bytes_from_user(target)?;
    let point =
        crate::syscall::metadata::resolve_at(crate::syscall::at_path::AT_FDCWD, &raw, true, &cred)
            .map_err(|e| {
                if e == ENOENT as u64 || e == ENOTDIR as u64 {
                    EBUSY as u64
                } else {
                    e
                }
            })?;
    let crate::fs::namei::Target::Inode { mount: covered_mount, ino: covered_ino, .. } = point.target else {
        return Err(EBUSY as u64);
    };
    if point.is_mount_point() {
        return Err(EBUSY as u64);
    }
    crate::fs::namei::working_dir(&point).map_err(|e| {
        if e == ENOENT as u64 || e == ENOTDIR as u64 {
            EBUSY as u64
        } else {
            e
        }
    })?;
    if crate::fs::vfs::mount::list_mounts()
        .iter()
        .any(|(_, path, _, _)| path.as_bytes() == point.path)
    {
        return Err(EBUSY as u64);
    }
    let mut parent_path = raw;
    parent_path.try_reserve(3).map_err(|_| ENOMEM as u64)?;
    parent_path.extend_from_slice(b"/..");
    let parent = crate::syscall::metadata::resolve_at(crate::syscall::at_path::AT_FDCWD, &parent_path, true, &cred)?;
    let crate::fs::namei::Target::Inode { mount: parent_mount, ino: parent_ino, .. } = parent.target else {
        return Err(EBUSY as u64);
    };
    let mut binding = endpoint.binding.lock();
    if binding.is_some() {
        return Err(EBUSY as u64);
    }
    let mut mounts = MOUNTS.lock();
    if mounts.iter().any(|m| m.cover.covered == (covered_mount, covered_ino)) {
        return Err(EBUSY as u64);
    }
    mounts.try_reserve(1).map_err(|_| ENOMEM as u64)?;
    let id = NEXT_MOUNT
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| ENOSPC as u64)?;
    let mount = Arc::try_new(Mount {
        id,
        cover: cover::Cover { mount: id, covered: (covered_mount, covered_ino), parent: (parent_mount, parent_ino) },
        point,
        state: Mutex::new(State {
            requests: RequestTable::new(),
            dead: false,
            detached: false,
            open_files: 0,
            releases: None,
        }),
        readers: endpoint.readers.clone(),
        callers: WaitQueueHead::new(),
        deferred_next: Mutex::new(None),
        deferred_queued: AtomicBool::new(false),
        mutations: Mutex::new(node_gates::NodeGates::new()),
    })
    .map_err(|_| ENOMEM as u64)?;
    mounts.push(mount.clone());
    *binding = Some(mount);
    drop(mounts);
    drop(binding);
    endpoint.readers.wake_up();
    Ok(())
}

fn umount(target: u64, flags: u64) -> Result<(), u64> {
    let cred = crate::fs::permissions::Credentials::current(false)?;
    args::require_root(cred.euid)?;
    if flags != 0 {
        return Err(EINVAL as u64);
    }
    let raw = userptr::copy_path_bytes_from_user(target)?;
    let point =
        crate::syscall::metadata::resolve_at(crate::syscall::at_path::AT_FDCWD, &raw, true, &cred)?;
    let mount = {
        let mut mounts = MOUNTS.lock();
        let index = mounts
            .iter()
            .position(|m| matches!(point.target, crate::fs::namei::Target::User { mount, node: ufs_abi::payload::ROOT, .. } if mount == m.id))
            .ok_or(EINVAL as u64)?;
        let mut state = mounts[index].state.lock();
        // Detach invalidates existing handles as well as pathname access.
        // FileLease keeps their mount storage alive until the final close.
        state.detached = true;
        state.disconnect();
        drop(state);
        mounts.remove(index)
    };
    mount.wake();
    Ok(())
}

pub fn sys_mount(_source: u64, target: u64, fstype: u64, flags: u64, data: u64) -> SyscallResult {
    match mount(target, fstype, flags, data) {
        Ok(()) => SyscallResult::Ok(0),
        Err(e) => SyscallResult::Err(e),
    }
}

pub fn sys_umount2(target: u64, flags: u64) -> SyscallResult {
    match umount(target, flags) {
        Ok(()) => SyscallResult::Ok(0),
        Err(e) => SyscallResult::Err(e),
    }
}
