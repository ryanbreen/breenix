//! PTY pair (master/slave) implementation

use crate::task::waitqueue::WaitQueueHead;
use crate::tty::ctty::TermCtl;
use crate::tty::termios::Termios;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Global counter: total bytes written through PTY slave_write (diagnostic)
pub static PTY_SLAVE_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
use crate::syscall::errno::EAGAIN;
use crate::tty::ioctl::Winsize;
use crate::tty::ctty::TtyRef;
use crate::tty::line_discipline::LineDiscipline;
use alloc::sync::Arc;

/// Ring buffer size for PTY data transfer
const PTY_BUFFER_SIZE: usize = 4096;

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

    /// Slave output suspended by TCXONC(TCOOFF).
    output_stopped: AtomicBool,

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
    pub fn new(pty_num: u32) -> Self {
        Self {
            pty_num,
            slave_to_master: spin::Mutex::new(PtyBuffer::new()),
            ldisc: spin::Mutex::new(LineDiscipline::new()),
            winsize: spin::Mutex::new(Winsize::default_pty()),
            locked: AtomicBool::new(true), // Locked until unlockpt()
            master_refcount: AtomicU32::new(1),
            slave_refcount: AtomicU32::new(0), // No slave FDs open initially
            slave_was_opened: AtomicBool::new(false),
            master_gone: AtomicBool::new(false),
            output_stopped: AtomicBool::new(false),
            ctl: spin::Mutex::new(TermCtl::new()),
            master_readers: WaitQueueHead::new(),
            slave_readers: WaitQueueHead::new(),
            slave_writers: WaitQueueHead::new(),
            master_writers: WaitQueueHead::new(),
        }
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

    /// Decrement slave reference count. Returns true if this was the last slave.
    /// When the last slave closes, wakes master readers so they can detect
    /// hangup. The wake is deferred: close paths can hold the process manager.
    pub fn slave_close(&self) -> bool {
        let old = self.slave_refcount.fetch_sub(1, Ordering::SeqCst);
        if old == 1 {
            self.master_readers.wake_up_deferred();
            true
        } else {
            false
        }
    }

    /// Record that the last master reference is gone and wake the slave side.
    /// Deferred wakes, for the same reason as `slave_close`.
    pub(super) fn mark_master_closed(&self) {
        self.master_gone.store(true, Ordering::SeqCst);
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
            self.slave_writers.wake_up();
        }
    }

    /// True while slave output is suspended.
    pub fn output_stopped(&self) -> bool {
        self.output_stopped.load(Ordering::SeqCst)
    }

    /// Write data to master (goes to slave's input via line discipline)
    ///
    /// Echo output from the line discipline (e.g., ^C when ISIG+ECHO are set)
    /// is written to the slave_to_master buffer so the terminal emulator can
    /// display it. Echo bytes are collected during line discipline processing
    /// and flushed after releasing the ldisc lock to avoid nested locking.
    ///
    /// If the line discipline generates a signal (e.g., SIGINT from Ctrl+C),
    /// it is delivered to the foreground process group.
    pub fn master_write(self: &Arc<Self>, data: &[u8]) -> Result<usize, i32> {
        let mut signal_to_deliver = None;
        let mut echo_buf = [0u8; 256];
        let mut echo_len = 0usize;
        let written;

        {
            let mut ldisc = self.ldisc.lock();

            let mut count = 0;
            for &byte in data {
                // Process through line discipline, collecting echo output
                let signal = ldisc.input_char(byte, &mut |echo_byte| {
                    if echo_len < echo_buf.len() {
                        echo_buf[echo_len] = echo_byte;
                        echo_len += 1;
                    }
                });
                if signal.is_some() {
                    signal_to_deliver = signal;
                }
                count += 1;
            }
            written = count;
            // ldisc lock dropped here
        }

        // Write echo bytes to slave_to_master so the terminal emulator can
        // display them (e.g., ^C echo). Done after releasing ldisc lock.
        if echo_len > 0 {
            let mut buffer = self.slave_to_master.lock();
            buffer.write(&echo_buf[..echo_len]);
        }

        // Deliver signal to foreground process group if one was generated
        // (must be done after releasing the ldisc lock to avoid deadlock)
        if let Some(sig) = signal_to_deliver {
            crate::tty::jobctl::signal_foreground(&TtyRef::Pty(self.clone()), sig);
        }

        if written > 0 {
            // Wake threads blocked on slave_read
            self.slave_readers.wake_up();
        }

        // Also wake master readers if echo was produced (they need to see echo output)
        if echo_len > 0 {
            self.master_readers.wake_up();
        }

        Ok(written)
    }

    /// Read data from master (slave's output)
    ///
    /// Returns Ok(n) if data was read, Err(EAGAIN) if no data available but
    /// slave is still connected (or never connected), or Ok(0) (EOF) if slave
    /// was connected and then all slave FDs were closed (hangup).
    pub fn master_read(&self, buf: &mut [u8]) -> Result<usize, i32> {
        let n = self.slave_to_master.lock().read(buf);
        if n == 0 {
            // No data — check if slave hung up (was opened then closed)
            if self.has_slave_hung_up() {
                return Ok(0); // EOF — slave hung up
            }
            return Err(EAGAIN);
        }
        self.slave_writers.wake_up();
        Ok(n)
    }

    /// Write data from slave (goes to master's read buffer)
    ///
    /// Performs output processing (OPOST) if enabled in termios:
    /// - ONLCR: translates \n to \r\n for proper terminal display
    pub fn slave_write(&self, data: &[u8]) -> Result<usize, i32> {
        let termios = self.termios_snapshot();
        let do_onlcr = termios.is_opost() && termios.is_onlcr();

        let mut buffer = self.slave_to_master.lock();
        let n = if do_onlcr {
            // Output processing: translate \n → \r\n
            let mut written = 0;
            for &byte in data {
                if byte == b'\n' {
                    if buffer.write(&[b'\r', b'\n']) < 2 {
                        break;
                    }
                } else {
                    if buffer.write(&[byte]) < 1 {
                        break;
                    }
                }
                written += 1;
            }
            written
        } else {
            buffer.write(data)
        };
        drop(buffer);
        if n == 0 {
            return Err(EAGAIN);
        }
        PTY_SLAVE_BYTES_WRITTEN.fetch_add(n as u64, Ordering::Relaxed);
        // Wake threads blocked on master_read
        self.master_readers.wake_up();
        Ok(n)
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
    pub fn slave_path(&self) -> alloc::string::String {
        alloc::format!("/dev/pts/{}", self.pty_num)
    }
}
