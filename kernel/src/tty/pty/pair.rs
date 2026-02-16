//! PTY pair (master/slave) implementation

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use crate::tty::termios::Termios;

/// Global counter: total bytes written through PTY slave_write (diagnostic)
pub static PTY_SLAVE_BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);
use crate::tty::ioctl::Winsize;
use crate::tty::line_discipline::LineDiscipline;
use crate::syscall::errno::EAGAIN;

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

    #[allow(dead_code)]
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

    /// Buffer for data from master to slave (master writes, slave reads)
    pub master_to_slave: spin::Mutex<PtyBuffer>,

    /// Buffer for data from slave to master (slave writes, master reads)
    pub slave_to_master: spin::Mutex<PtyBuffer>,

    /// Line discipline for the slave side
    pub ldisc: spin::Mutex<LineDiscipline>,

    /// Terminal attributes
    pub termios: spin::Mutex<Termios>,

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

    /// Foreground process group ID
    pub foreground_pgid: spin::Mutex<Option<u32>>,

    /// Controlling process ID (for TIOCSCTTY/TIOCNOTTY)
    pub controlling_pid: spin::Mutex<Option<u32>>,

    /// Threads blocked reading from master (waiting for slave to write)
    pub master_waiters: spin::Mutex<Vec<u64>>,

    /// Threads blocked reading from slave (waiting for master to write)
    pub slave_waiters: spin::Mutex<Vec<u64>>,
}

impl PtyPair {
    pub fn new(pty_num: u32) -> Self {
        Self {
            pty_num,
            master_to_slave: spin::Mutex::new(PtyBuffer::new()),
            slave_to_master: spin::Mutex::new(PtyBuffer::new()),
            ldisc: spin::Mutex::new(LineDiscipline::new()),
            termios: spin::Mutex::new(Termios::default()),
            winsize: spin::Mutex::new(Winsize::default_pty()),
            locked: AtomicBool::new(true), // Locked until unlockpt()
            master_refcount: AtomicU32::new(1),
            slave_refcount: AtomicU32::new(0), // No slave FDs open initially
            slave_was_opened: AtomicBool::new(false),
            foreground_pgid: spin::Mutex::new(None),
            controlling_pid: spin::Mutex::new(None),
            master_waiters: spin::Mutex::new(Vec::new()),
            slave_waiters: spin::Mutex::new(Vec::new()),
        }
    }

    /// Register a thread as waiting for data on the master side
    pub fn register_master_waiter(&self, thread_id: u64) {
        let mut waiters = self.master_waiters.lock();
        if !waiters.contains(&thread_id) {
            waiters.push(thread_id);
        }
    }

    /// Unregister a thread from the master wait queue
    pub fn unregister_master_waiter(&self, thread_id: u64) {
        let mut waiters = self.master_waiters.lock();
        waiters.retain(|&id| id != thread_id);
    }

    /// Wake all threads waiting for data on the master side
    pub fn wake_master_waiters(&self) {
        let readers: Vec<u64> = {
            let mut waiters = self.master_waiters.lock();
            waiters.drain(..).collect()
        };

        if !readers.is_empty() {
            crate::task::scheduler::with_scheduler(|sched| {
                for thread_id in &readers {
                    sched.unblock(*thread_id);
                }
            });
            crate::task::scheduler::set_need_resched();
        }
    }

    /// Register a thread as waiting for data on the slave side
    pub fn register_slave_waiter(&self, thread_id: u64) {
        let mut waiters = self.slave_waiters.lock();
        if !waiters.contains(&thread_id) {
            waiters.push(thread_id);
        }
    }

    /// Unregister a thread from the slave wait queue
    pub fn unregister_slave_waiter(&self, thread_id: u64) {
        let mut waiters = self.slave_waiters.lock();
        waiters.retain(|&id| id != thread_id);
    }

    /// Wake all threads waiting for data on the slave side
    pub fn wake_slave_waiters(&self) {
        let readers: Vec<u64> = {
            let mut waiters = self.slave_waiters.lock();
            waiters.drain(..).collect()
        };

        if !readers.is_empty() {
            crate::task::scheduler::with_scheduler(|sched| {
                for thread_id in &readers {
                    sched.unblock(*thread_id);
                }
            });
            crate::task::scheduler::set_need_resched();
        }
    }

    /// Increment slave reference count (called when a slave FD is created)
    pub fn slave_open(&self) {
        self.slave_was_opened.store(true, Ordering::SeqCst);
        self.slave_refcount.fetch_add(1, Ordering::SeqCst);
    }

    /// Decrement slave reference count. Returns true if this was the last slave.
    /// When the last slave closes, wakes master waiters so they can detect hangup.
    pub fn slave_close(&self) -> bool {
        let old = self.slave_refcount.fetch_sub(1, Ordering::SeqCst);
        if old == 1 {
            // Last slave closed — wake master readers so they see EOF/POLLHUP
            self.wake_master_waiters();
            true
        } else {
            false
        }
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

    /// Check if there is data available for the master to read
    pub fn has_master_data(&self) -> bool {
        !self.slave_to_master.lock().is_empty()
    }

    /// Check if master should be woken (data available OR slave hung up)
    pub fn should_wake_master(&self) -> bool {
        self.has_master_data() || self.has_slave_hung_up()
    }

    /// Check if there is data available for the slave to read
    pub fn has_slave_data(&self) -> bool {
        let ldisc = self.ldisc.lock();
        if ldisc.has_data() {
            return true;
        }
        drop(ldisc);
        !self.master_to_slave.lock().is_empty()
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
    pub fn master_write(&self, data: &[u8]) -> Result<usize, i32> {
        let mut signal_to_deliver = None;
        let mut echo_buf = [0u8; 256];
        let mut echo_len = 0usize;
        let written;

        {
            let mut ldisc = self.ldisc.lock();
            let _termios = self.termios.lock();

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
            // ldisc and termios locks dropped here
        }

        // Write echo bytes to slave_to_master so the terminal emulator can
        // display them (e.g., ^C echo). Done after releasing ldisc lock.
        if echo_len > 0 {
            let mut buffer = self.slave_to_master.lock();
            buffer.write(&echo_buf[..echo_len]);
        }

        // Deliver signal to foreground process group if one was generated
        // (must be done after releasing ldisc/termios locks to avoid deadlock)
        if let Some(sig) = signal_to_deliver {
            self.send_signal_to_foreground(sig);
        }

        if written > 0 {
            // Wake threads blocked on slave_read
            self.wake_slave_waiters();
        }

        // Also wake master readers if echo was produced (they need to see echo output)
        if echo_len > 0 {
            self.wake_master_waiters();
        }

        Ok(written)
    }

    /// Send a signal to the foreground process group
    ///
    /// Called when the line discipline generates a signal character
    /// (e.g., Ctrl+C -> SIGINT, Ctrl+\ -> SIGQUIT, Ctrl+Z -> SIGTSTP).
    fn send_signal_to_foreground(&self, sig: u32) {
        let pgid = match *self.foreground_pgid.lock() {
            Some(pgid) => pgid,
            None => {
                log::debug!("PTY{}: Signal {} but no foreground pgid", self.pty_num, sig);
                return;
            }
        };

        let pgid_as_pid = crate::process::ProcessId::new(pgid as u64);

        // Collect target PIDs and thread IDs for all non-terminated processes
        // in the foreground group (collect first, then deliver, to avoid
        // holding the manager lock while waking threads)
        let targets: Vec<(crate::process::ProcessId, Option<u64>)> = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                manager
                    .all_processes()
                    .iter()
                    .filter(|p| p.pgid == pgid_as_pid && !p.is_terminated())
                    .map(|p| (p.id, p.main_thread.as_ref().map(|t| t.id)))
                    .collect()
            } else {
                return;
            }
        };

        if targets.is_empty() {
            log::debug!(
                "PTY{}: No processes in foreground group {} for signal {}",
                self.pty_num,
                pgid,
                sig
            );
            return;
        }

        // Set signal pending on each process
        {
            let mut manager_guard = crate::process::manager();
            if let Some(ref mut pm) = *manager_guard {
                for &(pid, _) in &targets {
                    if let Some(proc) = pm.get_process_mut(pid) {
                        proc.signals.set_pending(sig);
                        if matches!(proc.state, crate::process::ProcessState::Blocked) {
                            proc.set_ready();
                        }
                        log::info!(
                            "PTY{}: Sent signal {} to process {} (PID {})",
                            self.pty_num,
                            sig,
                            proc.name,
                            pid.as_u64()
                        );
                    }
                }
            }
        }

        // Wake threads that may be blocked on signals or waitpid
        for &(_, thread_id) in &targets {
            if let Some(tid) = thread_id {
                crate::task::scheduler::with_scheduler(|sched| {
                    sched.unblock_for_signal(tid);
                    sched.unblock_for_child_exit(tid);
                });
            }
        }
        crate::task::scheduler::set_need_resched();
    }

    /// Read data from master (slave's output)
    ///
    /// Returns Ok(n) if data was read, Err(EAGAIN) if no data available but
    /// slave is still connected (or never connected), or Ok(0) (EOF) if slave
    /// was connected and then all slave FDs were closed (hangup).
    pub fn master_read(&self, buf: &mut [u8]) -> Result<usize, i32> {
        let mut buffer = self.slave_to_master.lock();
        let n = buffer.read(buf);
        if n == 0 {
            // No data — check if slave hung up (was opened then closed)
            if self.has_slave_hung_up() {
                return Ok(0); // EOF — slave hung up
            }
            return Err(EAGAIN);
        }
        Ok(n)
    }

    /// Write data from slave (goes to master's read buffer)
    pub fn slave_write(&self, data: &[u8]) -> Result<usize, i32> {
        let mut buffer = self.slave_to_master.lock();
        let n = buffer.write(data);
        drop(buffer);
        if n == 0 {
            return Err(EAGAIN);
        }
        PTY_SLAVE_BYTES_WRITTEN.fetch_add(n as u64, Ordering::Relaxed);
        // Wake threads blocked on master_read
        self.wake_master_waiters();
        Ok(n)
    }

    /// Read data for slave (from line discipline output)
    pub fn slave_read(&self, buf: &mut [u8]) -> Result<usize, i32> {
        let mut ldisc = self.ldisc.lock();
        let read_result = ldisc.read(buf);
        match read_result {
            Ok(n) if n > 0 => return Ok(n),
            _ => {}
        }

        // Also try the direct buffer if line discipline has no data
        let mut buffer = self.master_to_slave.lock();
        let n = buffer.read(buf);
        if n == 0 {
            return Err(EAGAIN);
        }
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
