//! PTY pair (master/slave) implementation

// Allow unused - these are public API fields/methods for Phase 2+ PTY syscalls:
// - PtyPair fields are used for PTY lifecycle management (unlockpt, ptsname)
// - PtyBuffer::new() is used by PtyPair constructor
// - PtyPair methods will be called from PTY syscalls
#![allow(dead_code)]

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

    /// Slave side reference count
    pub slave_refcount: AtomicU32,

    /// Session ID (if any)
    pub session: spin::Mutex<Option<u32>>,

    /// Foreground process group ID
    pub foreground_pgid: spin::Mutex<Option<u32>>,

    /// Controlling process ID (for TIOCSCTTY/TIOCNOTTY)
    pub controlling_pid: spin::Mutex<Option<u32>>,
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
            slave_refcount: AtomicU32::new(0),
            session: spin::Mutex::new(None),
            foreground_pgid: spin::Mutex::new(None),
            controlling_pid: spin::Mutex::new(None),
        }
    }

    /// Write data to master (goes to slave's input via line discipline)
    pub fn master_write(&self, data: &[u8]) -> Result<usize, i32> {
        let mut ldisc = self.ldisc.lock();
        let termios = self.termios.lock();

        // Collect echo output to send back to master
        let mut echo_output: Vec<u8> = Vec::new();

        let mut written = 0;
        for &byte in data {
            // Process through line discipline with echo callback
            let _signal = ldisc.input_char(byte, &mut |echo_byte| {
                echo_output.push(echo_byte);
            });

            // If echo produced output, send it to slave_to_master (so master can read it)
            if !echo_output.is_empty() {
                let mut s2m_buffer = self.slave_to_master.lock();
                for &echo_byte in &echo_output {
                    s2m_buffer.write(&[echo_byte]);
                }
                echo_output.clear();
            }

            written += 1;
        }

        // Drop termios lock (was used for potential future CR/NL mapping)
        drop(termios);

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
        if n == 0 {
            return Err(EAGAIN);
        }
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
