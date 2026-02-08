//! PTY pair (master/slave) implementation

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use crate::tty::termios::Termios;
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

    /// Check if there is data available for the master to read
    pub fn has_master_data(&self) -> bool {
        !self.slave_to_master.lock().is_empty()
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
    /// Echo output is discarded - in a real PTY, the terminal emulator (master)
    /// is responsible for displaying what it writes. The echo callback is still
    /// called for line discipline state tracking, but output isn't sent to the
    /// slave_to_master buffer to avoid polluting slave->master data flow.
    pub fn master_write(&self, data: &[u8]) -> Result<usize, i32> {
        let mut ldisc = self.ldisc.lock();
        let _termios = self.termios.lock();

        let mut written = 0;
        for &byte in data {
            // Process through line discipline - echo callback is a no-op
            // because the master (terminal emulator) handles its own display
            let _signal = ldisc.input_char(byte, &mut |_echo_byte| {
                // Discard echo - master handles its own display
            });
            written += 1;
        }

        drop(_termios);
        drop(ldisc);

        if written > 0 {
            // Wake threads blocked on slave_read
            self.wake_slave_waiters();
        }

        Ok(written)
    }

    /// Read data from master (slave's output)
    pub fn master_read(&self, buf: &mut [u8]) -> Result<usize, i32> {
        let mut buffer = self.slave_to_master.lock();
        let n = buffer.read(buf);
        if n == 0 {
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
