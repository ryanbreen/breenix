//! PTY pair (master/slave) implementation

use crate::task::waitqueue::WaitQueueHead;
use crate::tty::ctty::TermCtl;
use crate::tty::termios::{Termios, NOFLSH};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Global counter: total bytes written through PTY slave_write (diagnostic)
pub static PTY_SLAVE_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
use crate::syscall::errno::{EAGAIN, EIO};
use crate::tty::ioctl::Winsize;
use crate::tty::ctty::TtyRef;
use crate::tty::line_discipline::LineDiscipline;
use alloc::sync::Arc;

/// Ring buffer size for PTY data transfer
pub(crate) const PTY_BUFFER_SIZE: usize = 4096;

/// A slave's permission bits after allocation and grantpt: owner read and
/// write, group write (POSIX grantpt).
const SLAVE_MODE: u32 = 0o620;

/// Simple ring buffer for PTY I/O
pub struct PtyBuffer {
    data: [u8; PTY_BUFFER_SIZE],
    read_pos: usize,
    write_pos: usize,
}

impl PtyBuffer {
    pub const fn new() -> Self {
        Self {
            data: [0; PTY_BUFFER_SIZE],
            read_pos: 0,
            write_pos: 0,
        }
    }

    pub fn write(&mut self, buf: &[u8]) -> usize {
        let mut written = 0;
        for &byte in buf {
            let next_write = (self.write_pos + 1) % PTY_BUFFER_SIZE;
            if next_write == self.read_pos {
                break; // Buffer full
            }
            self.data[self.write_pos] = byte;
            self.write_pos = next_write;
            written += 1;
        }
        written
    }

    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let mut read = 0;
        for byte in buf.iter_mut() {
            if self.read_pos == self.write_pos {
                break; // Buffer empty
            }
            *byte = self.data[self.read_pos];
            self.read_pos = (self.read_pos + 1) % PTY_BUFFER_SIZE;
            read += 1;
        }
        read
    }

    pub fn is_empty(&self) -> bool {
        self.read_pos == self.write_pos
    }

    /// Put `bytes`, just read, back at the front, as many as there is room
    pub fn clear(&mut self) {
        self.read_pos = self.write_pos;
    }

    /// Discard everything unread except the last `n` bytes written.
    pub fn keep_last(&mut self, n: usize) {
        let n = n.min(self.available());
        self.read_pos = (self.write_pos + PTY_BUFFER_SIZE - n) % PTY_BUFFER_SIZE;
    }

    /// Copy before advancing the ring so EFAULT leaves all bytes available.
    pub fn read_user(&mut self, ptr: u64, scratch: &mut [u8]) -> Result<usize, i32> {
        let n = scratch.len().min(self.available());
        for (i, dst) in scratch[..n].iter_mut().enumerate() {
            *dst = self.data[(self.read_pos + i) % PTY_BUFFER_SIZE];
        }
        if n > 0 {
            crate::syscall::userptr::write_user_bytes(ptr, scratch.as_ptr(), n).map_err(|e| e as i32)?;
            self.read_pos = (self.read_pos + n) % PTY_BUFFER_SIZE;
        }
        Ok(n)
    }

    /// Bytes that can still be written before the ring is full.
    pub fn space(&self) -> usize {
        PTY_BUFFER_SIZE - 1 - self.available()
    }

    pub fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            PTY_BUFFER_SIZE - self.read_pos + self.write_pos
        }
    }
}

/// A PTY pair (master + slave)
pub struct PtyPair {
    /// PTY number (0, 1, 2, ...)
    pub pty_num: u32,

    /// Buffer for data from slave to master (slave writes, master reads)
    pub slave_to_master: spin::Mutex<PtyBuffer>,

    /// Line discipline for the slave side. Its termios is the pair's only copy.
    pub ldisc: spin::Mutex<LineDiscipline>,

    /// Window size (rows, columns, pixels)
    pub winsize: spin::Mutex<Winsize>,

    /// Locked state (true until unlockpt() called)
    pub locked: AtomicBool,

    /// Master side reference count
    pub master_refcount: AtomicU32,

    /// Slave side reference count (tracks open slave FDs)
    pub slave_refcount: AtomicU32,

    /// Whether a slave FD was ever opened (for POLLHUP semantics)
    /// POLLHUP should only be reported when the slave was connected and then
    /// disconnected — not when it was never connected.
    pub slave_was_opened: AtomicBool,

    /// Set once the last master reference is gone.
    master_gone: AtomicBool,

    /// The last master close's hangup has not finished; the pair keeps its
    /// number until it has.
    pub(super) hangup_pending: AtomicBool,

    /// Slave output suspended by TCXONC(TCOOFF).
    output_stopped: AtomicBool,
    input_flow_stopped: AtomicBool,
    pending_echo: spin::Mutex<alloc::boxed::Box<PtyBuffer>>,

    /// Master output (input to the slave) suspended by TCXONC(TCOOFF) on
    /// the master.
    master_output_stopped: AtomicBool,

    /// The slave device's owner, group and permission bits: set from the
    /// opener of the master, changed by grantpt, checked by a slave open.
    slave_uid: AtomicU32,
    slave_gid: AtomicU32,
    slave_mode: AtomicU32,

    /// The terminal's session and foreground process group.
    pub ctl: spin::Mutex<TermCtl>,

    /// Threads blocked reading the master (waiting for slave output)
    pub master_readers: WaitQueueHead,

    /// Threads blocked reading the slave (waiting for master input)
    pub slave_readers: WaitQueueHead,

    /// Threads blocked writing the slave (waiting for space in slave_to_master)
    pub slave_writers: WaitQueueHead,

    /// Threads blocked writing the master (waiting for room in the slave's
    /// input queue, `line_discipline::MAX_INPUT`)
    pub master_writers: WaitQueueHead,
}

impl PtyPair {
    /// A new pair whose slave belongs to `uid` and `gid`, mode 0620 (owner
    /// read and write, group write, as grantpt leaves it). Fails with ENOMEM
    /// instead of panicking when the line discipline's queues cannot be
    /// allocated.
    pub fn try_new(pty_num: u32, uid: u32, gid: u32) -> Result<Self, i32> {
        Ok(Self {
            pty_num,
            slave_to_master: spin::Mutex::new(PtyBuffer::new()),
            ldisc: spin::Mutex::new(LineDiscipline::try_new()?),
            winsize: spin::Mutex::new(Winsize::default_pty()),
            locked: AtomicBool::new(true), // Locked until unlockpt()
            master_refcount: AtomicU32::new(1),
            slave_refcount: AtomicU32::new(0), // No slave FDs open initially
            slave_was_opened: AtomicBool::new(false),
            master_gone: AtomicBool::new(false),
            hangup_pending: AtomicBool::new(false),
            output_stopped: AtomicBool::new(false),
            input_flow_stopped: AtomicBool::new(false),
            pending_echo: spin::Mutex::new(alloc::boxed::Box::try_new(PtyBuffer::new())
                .map_err(|_| crate::syscall::errno::ENOMEM)?),
            master_output_stopped: AtomicBool::new(false),
            slave_uid: AtomicU32::new(uid),
            slave_gid: AtomicU32::new(gid),
            slave_mode: AtomicU32::new(SLAVE_MODE),
            ctl: spin::Mutex::new(TermCtl::new()),
            master_readers: WaitQueueHead::new(),
            slave_readers: WaitQueueHead::new(),
            slave_writers: WaitQueueHead::new(),
            master_writers: WaitQueueHead::new(),
        })
    }

    /// The slave device's (uid, gid, mode), mode without the file type.
    pub fn slave_owner(&self) -> (u32, u32, u32) {
        (
            self.slave_uid.load(Ordering::SeqCst),
            self.slave_gid.load(Ordering::SeqCst),
            self.slave_mode.load(Ordering::SeqCst),
        )
    }

    /// grantpt: the slave belongs to `uid`, with mode 0620.
    pub fn grant(&self, uid: u32) {
        self.slave_uid.store(uid, Ordering::SeqCst);
        self.slave_mode.store(SLAVE_MODE, Ordering::SeqCst);
    }

    /// The pair's termios (the line discipline's copy).
    pub fn termios_snapshot(&self) -> Termios {
        *self.ldisc.lock().termios()
    }

    /// Increment slave reference count (called when a slave FD is created)
    pub fn slave_open(&self) {
        self.slave_was_opened.store(true, Ordering::SeqCst);
        self.slave_refcount.fetch_add(1, Ordering::SeqCst);
    }

    /// Take a slave reference for an open that has not installed its
    /// descriptor yet. The slave counts as opened (for the master's EIO and
    /// POLLHUP) only once `mark_slave_opened` follows, so an open that fails
    /// and gives the reference back leaves the pair as it found it.
    pub(super) fn take_slave_ref(&self) {
        self.slave_refcount.fetch_add(1, Ordering::SeqCst);
    }

    /// The open that took a slave reference installed its descriptor.
    pub fn mark_slave_opened(&self) {
        self.slave_was_opened.store(true, Ordering::SeqCst);
    }

    /// Decrement slave reference count. Returns true if this was the last slave.
    /// When the last slave closes, wakes master readers so they can detect
    /// hangup. The wake is deferred: close paths can hold the process manager.
    pub fn slave_close(&self) -> bool {
        let old = self.slave_refcount.fetch_sub(1, Ordering::SeqCst);
        if old == 1 {
            self.master_readers.wake_up_deferred();
            self.master_writers.wake_up_deferred();
            true
        } else {
            false
        }
    }

    /// Record that the last master reference is gone. The PTY table lock is
    /// held, so `open_slave` cannot see the master count at zero without it.
    pub(super) fn mark_master_closed(&self) {
        self.master_gone.store(true, Ordering::SeqCst);
    }

    /// Wake the slave side after the master closed. Deferred wakes, for the
    /// same reason as `slave_close`.
    pub(super) fn wake_slave_side(&self) {
        self.slave_readers.wake_up_deferred();
        self.slave_writers.wake_up_deferred();
    }

    /// True once every master reference has been closed.
    pub fn master_closed(&self) -> bool {
        self.master_gone.load(Ordering::SeqCst)
    }

    /// Check if any slave FDs are still open
    pub fn has_slave_open(&self) -> bool {
        self.slave_refcount.load(Ordering::SeqCst) > 0
    }

    /// Check if the slave has hung up (was opened and then all slave FDs closed).
    /// Returns false if the slave was never opened — this is the "not yet connected"
    /// state, not a hangup.
    pub fn has_slave_hung_up(&self) -> bool {
        self.slave_was_opened.load(Ordering::SeqCst) && !self.has_slave_open()
    }

    /// Suspend or resume slave output (TCXONC). Resuming wakes blocked writers.
    pub fn set_output_stopped(&self, stopped: bool) {
        self.output_stopped.store(stopped, Ordering::SeqCst);
        if !stopped {
            self.drain_echo();
            self.slave_writers.wake_up();
        }
    }

    /// Discard the slave output the master has not read (TCOFLUSH), and
    /// wake writers waiting for room.
    pub fn flush_output(&self) {
        let mut echo = self.pending_echo.lock();
        let mut out = self.slave_to_master.lock();
        echo.clear();
        out.clear();
        drop(out);
        drop(echo);
        self.slave_writers.wake_up();
    }

    /// True while slave output is suspended.
    pub fn output_stopped(&self) -> bool {
        self.output_stopped.load(Ordering::SeqCst)
            || self.input_flow_stopped.load(Ordering::SeqCst)
    }

    /// Publish an input/settings flow change under ldisc, then wake outside it.
    pub fn update_input_flow(&self) {
        let ld = self.ldisc.lock();
        let stopped = ld.flow_stopped();
        self.input_flow_stopped.store(stopped, Ordering::SeqCst);
        drop(ld);
        if !stopped { self.drain_echo(); self.slave_writers.wake_up(); }
    }

    /// The fixed echo FIFO retains output while stopped and drains as the
    /// master frees room. No input lock is acquired below the output lock.
    pub fn drain_echo(&self) {
        if self.output_stopped() { return; }
        let mut echo = self.pending_echo.lock();
        let mut out = self.slave_to_master.lock();
        let mut byte = [0];
        while out.space() > 0 && echo.read(&mut byte) != 0 { out.write(&byte); }
        drop(out);
        drop(echo);
        self.master_readers.wake_up();
    }

    fn append_echo(echo: &mut PtyBuffer, c: u8, onlcr: bool) -> usize {
        let crlf = [b'\r', c];
        let unit = if onlcr && c == b'\n' { &crlf[..] } else { &crlf[1..] };
        // Like Linux's bounded echo FIFO, retain the most recent output on
        // overflow; input must not wait for its own echo to be read.
        let mut discard = [0];
        while echo.space() < unit.len() { echo.read(&mut discard); }
        echo.write(unit)
    }

    /// Suspend or resume master output, the slave's input (TCXONC on the
    /// master). Resuming wakes blocked master writers.
    pub fn set_master_output_stopped(&self, stopped: bool) {
        self.master_output_stopped.store(stopped, Ordering::SeqCst);
        if !stopped {
            self.master_writers.wake_up();
        }
    }

    /// True while master output is suspended.
    pub fn master_output_stopped(&self) -> bool {
        self.master_output_stopped.load(Ordering::SeqCst)
    }

    /// TCIOFF/TCION on the slave: send the STOP or START character `c` to
    /// the master, ahead of stopped output, as Linux sends it out of band.
    /// False when the master's buffer is full.
    pub fn send_flow_char_to_master(&self, c: u8) -> bool {
        let sent = self.slave_to_master.lock().write(core::slice::from_ref(&c)) == 1;
        if sent {
            self.master_readers.wake_up();
        }
        sent
    }

    /// Feed `data` from the master to the slave's line discipline. Stops
    /// when the input queue has no room (`LineDiscipline::input_room`), or
    /// just after a signal character, so that each signal character's signal
    /// is sent before the input after it is processed. Returns how many bytes
    /// were consumed; the caller waits on `master_writers` for room.
    ///
    /// Echo goes straight into the output ring while the line discipline is
    /// held (ldisc, then slave_to_master, is the lock order), so it keeps its
    /// place among other writers' echo. A signal character without NOFLSH
    /// discards the unread output first (POSIX 11.1.9), under the same
    /// locks, keeping only that character's own echo, as Linux echoes it
    /// after the flush. Signals and wakes follow after both locks are
    /// released.
    pub fn master_write(self: &Arc<Self>, data: &[u8]) -> usize {
        self.feed_slave(data, false)
    }

    /// TCIOFF/TCION on the master: send the STOP or START character `c` to
    /// the slave's input, even while master output is stopped. False when
    /// the slave's input queue has no room.
    pub fn send_flow_char_to_slave(self: &Arc<Self>, c: u8) -> bool {
        self.feed_slave(core::slice::from_ref(&c), true) == 1
    }

    /// `master_write`, ignoring a TCOOFF on the master when `ignore_stop`.
    fn feed_slave(self: &Arc<Self>, data: &[u8], ignore_stop: bool) -> usize {
        let mut signal_to_deliver = None;
        let mut count = 0;
        let mut flushed = false;
        let stopped;
        {
            let mut ld = self.ldisc.lock();
            let mut echo = self.pending_echo.lock();
            let onlcr = ld.termios().is_opost() && ld.termios().is_onlcr();
            for &byte in data {
                if !ld.can_receive(byte) || (!ignore_stop && self.master_output_stopped()) { break; }
                let mut echo_len = 0;
                let signal = ld.input_char(byte, &mut |c| {
                    echo_len += Self::append_echo(&mut echo, c, onlcr);
                });
                count += 1;
                if let Some(signal) = signal {
                    if ld.termios().c_lflag & NOFLSH == 0 {
                        echo.keep_last(echo_len);
                        self.slave_to_master.lock().clear();
                        flushed = true;
                    }
                    signal_to_deliver = Some(signal);
                    break;
                }
            }
            stopped = ld.flow_stopped();
            self.input_flow_stopped.store(stopped, Ordering::SeqCst);
        }
        self.drain_echo();
        if flushed || !stopped { self.slave_writers.wake_up(); }
        if let Some(sig) = signal_to_deliver {
            crate::tty::jobctl::signal_foreground(&TtyRef::Pty(self.clone()), sig);
        }
        if count > 0 { self.slave_readers.wake_up(); }
        count
    }

    /// Read data from master (slave's output)
    ///
    /// Returns Ok(n) if data was read, Err(EAGAIN) if no data available but
    /// slave is still connected (or never connected), or Err(EIO) once the
    /// slave was connected and then all slave FDs were closed (Linux).
    pub fn master_read(&self, buf: &mut [u8]) -> Result<usize, i32> {
        let n = self.slave_to_master.lock().read(buf);
        if n == 0 {
            if self.has_slave_hung_up() {
                return Err(EIO);
            }
            return Err(EAGAIN);
        }
        self.drain_echo();
        self.slave_writers.wake_up();
        Ok(n)
    }

    /// Copy slave output into `ring` (the slave_to_master buffer) until it is
    /// full, translating \n to \r\n when `onlcr` (OPOST|ONLCR) is set.
    /// Returns how many bytes of `data` were consumed.
    pub fn put_output(ring: &mut PtyBuffer, data: &[u8], onlcr: bool) -> usize {
        let mut written = 0;
        for &byte in data {
            let out: &[u8] = if onlcr && byte == b'\n' { b"\r\n" } else { core::slice::from_ref(&byte) };
            if ring.space() < out.len() {
                break;
            }
            ring.write(out);
            written += 1;
        }
        if written > 0 {
            PTY_SLAVE_BYTES_WRITTEN.fetch_add(written as u64, Ordering::Relaxed);
        }
        written
    }

    /// Check if PTY is unlocked (slave can be opened)
    pub fn is_unlocked(&self) -> bool {
        !self.locked.load(Ordering::SeqCst)
    }

    /// Unlock the PTY (called by unlockpt syscall)
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::SeqCst);
    }

    /// Get the slave device path
    pub fn slave_path(&self) -> Result<alloc::string::String, i32> {
        use core::fmt::Write;
        let mut path = alloc::string::String::new();
        path.try_reserve_exact(20).map_err(|_| crate::syscall::errno::ENOMEM)?;
        write!(&mut path, "/dev/pts/{}", self.pty_num).map_err(|_| crate::syscall::errno::ENOMEM)?;
        Ok(path)
    }
}
