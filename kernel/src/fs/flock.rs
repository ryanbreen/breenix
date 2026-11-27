//! LSB Core 5.0, Generic Part, chapter 14: flock.

use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::flock_table::{Attempt, LockTable, Mode};
use crate::ipc::fd::{FdKind, FileDescriptor, OpenFileDescription};
use crate::syscall::{errno, SyscallResult};
use crate::task::thread::ThreadState;
use crate::task::waitqueue::{schedule_current_wait, PrepareOutcome, WaitQueueHead};

const LOCK_SH: i32 = 1;
const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;
const LOCK_UN: i32 = 8;

#[derive(PartialEq, Eq)]
enum FileKey {
    Inode { mount: usize, inode: u64, incarnation: u64 },
    Pipe(u64),
    Device(u64),
    DevfsDirectory,
    DevptsDirectory,
    ProcfsDirectory(String),
}

impl FileKey {
    fn inode(node: &super::vfs::VNode) -> Self {
        Self::Inode { mount: node.mount, inode: node.ino, incarnation: node.incarnation }
    }

    fn of(entry: &FileDescriptor) -> Result<Option<Self>, i32> {
        if let Some(node) = &entry.namespace_node {
            return Ok(Some(Self::inode(node)));
        }
        Ok(Some(match &entry.kind {
            FdKind::RegularFile(file) => Self::inode(&file.open.node),
            FdKind::Directory(dir) => Self::inode(&dir.lock().open.node),
            FdKind::PipeRead(pipe) | FdKind::PipeWrite(pipe) => Self::Pipe(pipe.lock().inode()),
            FdKind::FifoRead(_, pipe, _) | FdKind::FifoWrite(_, pipe, _)
            | FdKind::FifoReadWrite(_, pipe, _) => Self::Pipe(pipe.lock().inode()),
            FdKind::Device(device) => Self::Device(device.inode()),
            FdKind::DevfsDirectory { .. } => Self::DevfsDirectory,
            FdKind::DevptsDirectory { .. } => Self::DevptsDirectory,
            FdKind::ProcfsDirectory { path, .. } => {
                let mut key = String::new();
                key.try_reserve_exact(path.len()).map_err(|_| errno::ENOLCK)?;
                key.push_str(path);
                Self::ProcfsDirectory(key)
            }
            // Objects made for one description (such as sockets or a
            // procfs read snapshot) have no competing path-based opens.
            _ => return Ok(None),
        }))
    }
}

// Backends can return different VNodes for separate opens of the same inode.
// Intern the wait queue and lock table by file identity, only for flock users.
static FILES: Mutex<Vec<(FileKey, Weak<FileLocks>)>> = Mutex::new(Vec::new());
static DEFERRED: Mutex<Option<Arc<FileLocks>>> = Mutex::new(None);

pub(crate) struct FileLocks {
    table: Mutex<LockTable<OpenFileDescription>>,
    waiters: WaitQueueHead,
    registered: bool,
    deferred_next: Mutex<Option<Arc<FileLocks>>>,
    deferred_queued: AtomicBool,
}

impl FileLocks {
    fn new(registered: bool) -> Result<Arc<Self>, i32> {
        Arc::try_new(Self {
            table: Mutex::new(LockTable::new()),
            waiters: WaitQueueHead::new(),
            registered,
            deferred_next: Mutex::new(None),
            deferred_queued: AtomicBool::new(false),
        })
        .map_err(|_| errno::ENOLCK)
    }

    fn for_key(key: Option<FileKey>) -> Result<Arc<Self>, i32> {
        let Some(key) = key else { return Self::new(false) };
        crate::arch_without_interrupts(|| {
            let mut files = FILES.lock();
            if let Some(file) = files.iter().find_map(|(id, file)| {
                if *id == key { file.upgrade() } else { None }
            }) {
                return Ok(file);
            }
            files.try_reserve(1).map_err(|_| errno::ENOLCK)?;
            let file = Self::new(true)?;
            files.push((key, Arc::downgrade(&file)));
            Ok(file)
        })
    }

    fn with_table<R>(&self, f: impl FnOnce(&mut LockTable<OpenFileDescription>) -> R) -> R {
        crate::arch_without_interrupts(|| f(&mut self.table.lock()))
    }

    fn lock(
        &self,
        owner: &Weak<OpenFileDescription>,
        mode: Mode,
        nonblocking: bool,
    ) -> Result<(), i32> {
        loop {
            // Pin the owner only while changing the table. A sleeping caller
            // must not keep a description open after the last descriptor closes.
            let description = owner.upgrade().ok_or(errno::EBADF)?;
            let attempt = self.with_table(|table| table.try_lock(owner, mode, nonblocking));
            drop(description);
            match attempt? {
                Attempt::Acquired => return Ok(()),
                Attempt::Released => self.waiters.wake_up(),
                Attempt::Blocked => {
                    // Release changes the table before waking. Recheck it
                    // under the queue lock so a release cannot miss this wait.
                    let outcome = self.waiters.prepare_to_wait_checked(
                        ThreadState::BlockedOnIO,
                        None,
                        || self.with_table(|table| table.should_wait(owner, mode)),
                    );
                    match outcome {
                        PrepareOutcome::Mismatch => continue,
                        PrepareOutcome::PublishFailed => return Err(errno::EINTR),
                        PrepareOutcome::OutOfMemory => return Err(errno::ENOLCK),
                        PrepareOutcome::Queued => {}
                    }
                    let mut signal = crate::syscall::check_signals_for_wait();
                    if signal.is_none() {
                        schedule_current_wait();
                        signal = crate::syscall::check_signals_for_wait();
                    }
                    self.waiters.finish_wait();
                    if let Some(error) = signal {
                        return Err(error);
                    }
                }
            }
        }
    }

    fn unlock(&self, owner: *const OpenFileDescription) {
        if self.with_table(|table| table.unlock(owner)) {
            self.waiters.wake_up();
        }
    }

    pub(crate) fn close(self: &Arc<Self>, owner: *const OpenFileDescription) {
        self.with_table(|table| table.unlock(owner));
        if Arc::strong_count(self) == 1 {
            return;
        }
        // Also wake a waiter whose description closed while it was waiting.
        // Last close can run during teardown with PM or the scheduler held.
        // Defer the wake itself in that case, like the socket and UFS drains.
        if crate::process::process_manager_held_on_current_cpu()
            || crate::tracing::providers::teardown::scheduler_scope_active()
            || crate::per_cpu::in_interrupt()
        {
            crate::arch_without_interrupts(|| {
                let mut head = DEFERRED.lock();
                if !self.deferred_queued.swap(true, Ordering::AcqRel) {
                    *self.deferred_next.lock() = head.take();
                    *head = Some(self.clone());
                }
            });
        } else {
            self.waiters.wake_up();
        }
    }
}

impl Drop for FileLocks {
    fn drop(&mut self) {
        if self.registered {
            crate::arch_without_interrupts(|| {
                let mut files = FILES.lock();
                // Another opener may have interned a replacement for the key
                // while this entry's last strong reference was dropping.
                files.retain(|(_, file)| !core::ptr::eq(file.as_ptr(), self));
                if files.is_empty() {
                    *files = Vec::new();
                }
            });
        }
    }
}

/// Run wakes from close paths that cannot take scheduler locks.
pub(crate) fn wake_deferred() {
    let mut next = crate::arch_without_interrupts(|| DEFERRED.lock().take());
    while let Some(file) = next {
        next = crate::arch_without_interrupts(|| file.deferred_next.lock().take());
        file.deferred_queued.store(false, Ordering::Release);
        file.waiters.wake_up();
    }
}

pub(crate) fn sys_flock(fd: i32, operation: i32) -> SyscallResult {
    match flock(fd, operation) {
        Ok(()) => SyscallResult::Ok(0),
        Err(error) => SyscallResult::Err(error as u64),
    }
}

fn flock(fd: i32, operation: i32) -> Result<(), i32> {
    let mode = match operation & !LOCK_NB {
        LOCK_SH => Some(Mode::Shared),
        LOCK_EX => Some(Mode::Exclusive),
        LOCK_UN => None,
        _ => return Err(errno::EINVAL),
    };
    let tid = crate::task::scheduler::current_thread_id().ok_or(errno::EBADF)?;
    let (description, key) = {
        let manager = crate::process::manager();
        let (_, process) = manager.as_ref()
            .and_then(|manager| manager.find_process_by_thread(tid))
            .ok_or(errno::EBADF)?;
        let entry = process.fd_table.get(fd).ok_or(errno::EBADF)?;
        let description = entry.open_description();
        let key = if mode.is_some() && description.flock.get().is_none() {
            FileKey::of(entry)?
        } else {
            None
        };
        (description, key)
    };
    let Some(mode) = mode else {
        if let Some(file) = description.flock.get() {
            file.unlock(Arc::as_ptr(&description));
        }
        return Ok(());
    };
    let file = description.flock.try_call_once(|| FileLocks::for_key(key))?.clone();
    let owner = Arc::downgrade(&description);
    drop(description);
    file.lock(&owner, mode, operation & LOCK_NB != 0)
}
