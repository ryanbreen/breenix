//! WasmKernel -- orchestrates the Breenix POSIX kernel subsystems in WASM.
//!
//! Owns the RamFs, FdTable, and provides syscall-like methods that route
//! to breenix-core's real implementations.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use breenix_core::fs::devfs::{device_read, device_write, DeviceType};
use breenix_core::fs::ramfs::RamFs;
use breenix_core::fs::vfs::{FileType, OpenFlags, VfsError};
use breenix_core::ipc::fd::{DirectoryFile, FdKind, FdTable, RegularFile};
use breenix_core::ipc::pipe;
use spin::Mutex;

/// The WASM kernel: a single-process POSIX environment running in the browser.
///
/// All filesystem, fd-table, and environment state lives here.
/// JavaScript drives it by feeding keyboard input and pulling output.
pub struct WasmKernel {
    /// The root filesystem (in-memory)
    pub fs: RamFs,
    /// Per-process file descriptor table
    pub fd_table: FdTable,
    /// Current working directory
    pub cwd: String,
    /// Environment variables
    pub env: BTreeMap<String, String>,
    /// Output buffer (captures stdout/stderr writes)
    output_buf: Vec<u8>,
    /// Process ID (simulated)
    pub pid: u64,
    /// Exit code of last command
    pub last_exit_code: i32,
}

impl WasmKernel {
    /// Create a new kernel with a populated filesystem and default environment.
    pub fn new() -> Self {
        let mut kernel = Self {
            fs: RamFs::new(),
            fd_table: FdTable::new(),
            cwd: String::from("/"),
            env: BTreeMap::new(),
            output_buf: Vec::new(),
            pid: 1,
            last_exit_code: 0,
        };
        kernel.populate_fs();
        kernel.populate_env();
        kernel
    }

    /// Populate the initial filesystem with standard directories and files.
    fn populate_fs(&mut self) {
        // Standard directories
        let _ = self.fs.mkdir_p("/bin", 0o755);
        let _ = self.fs.mkdir_p("/dev", 0o755);
        let _ = self.fs.mkdir_p("/etc", 0o755);
        let _ = self.fs.mkdir_p("/home/user", 0o755);
        let _ = self.fs.mkdir_p("/proc", 0o555);
        let _ = self.fs.mkdir_p("/tmp", 0o1777);
        let _ = self.fs.mkdir_p("/var/log", 0o755);
        let _ = self.fs.mkdir_p("/usr/bin", 0o755);
        let _ = self.fs.mkdir_p("/usr/lib", 0o755);

        // Device nodes in /dev
        let _ = self.fs.mknod("/dev/null", FileType::CharDevice, 1, 3);
        let _ = self.fs.mknod("/dev/zero", FileType::CharDevice, 1, 5);
        let _ = self
            .fs
            .mknod("/dev/console", FileType::CharDevice, 5, 1);
        let _ = self.fs.mknod("/dev/tty", FileType::CharDevice, 5, 0);

        // /etc files
        let _ = self.fs.create_file("/etc/hostname", 0o644);
        let _ = self.fs.write_file("/etc/hostname", b"breenix\n");

        let _ = self.fs.create_file("/etc/passwd", 0o644);
        let _ = self.fs.write_file(
            "/etc/passwd",
            b"root:x:0:0:root:/root:/bin/sh\nuser:x:1000:1000:user:/home/user:/bin/sh\n",
        );

        let _ = self.fs.create_file("/etc/os-release", 0o644);
        let _ = self.fs.write_file(
            "/etc/os-release",
            b"NAME=\"Breenix\"\nVERSION=\"0.1.0\"\nID=breenix\nPRETTY_NAME=\"Breenix OS 0.1.0\"\n",
        );

        // Simulated /proc files
        let _ = self.fs.create_file("/proc/version", 0o444);
        let _ = self.fs.write_file(
            "/proc/version",
            b"Breenix version 0.1.0 (wasm32) #1 SMP PREEMPT\n",
        );

        let _ = self.fs.create_file("/proc/uptime", 0o444);
        let _ = self.fs.write_file("/proc/uptime", b"0.00 0.00\n");

        let _ = self.fs.create_file("/proc/meminfo", 0o444);
        let _ = self.fs.write_file(
            "/proc/meminfo",
            b"MemTotal:       262144 kB\nMemFree:        131072 kB\nMemAvailable:   196608 kB\n",
        );
    }

    /// Set up default environment variables.
    fn populate_env(&mut self) {
        self.env
            .insert(String::from("HOME"), String::from("/home/user"));
        self.env
            .insert(String::from("USER"), String::from("user"));
        self.env
            .insert(String::from("SHELL"), String::from("/bin/sh"));
        self.env
            .insert(String::from("PATH"), String::from("/usr/bin:/bin"));
        self.env.insert(String::from("PWD"), String::from("/"));
        self.env
            .insert(String::from("TERM"), String::from("xterm-256color"));
        self.env
            .insert(String::from("HOSTNAME"), String::from("breenix"));
    }

    // ================================================================
    // Syscall-like methods
    // ================================================================

    /// Write bytes to a file descriptor.
    ///
    /// For stdout/stderr (fds 1 and 2), output is captured in an internal
    /// buffer that JavaScript can drain via `take_output()`.
    ///
    /// Returns the number of bytes written, or a negative errno on error.
    pub fn sys_write(&mut self, fd: i32, buf: &[u8]) -> Result<usize, i32> {
        let fd_entry = self.fd_table.get(fd).ok_or(9)?; // EBADF

        // We need to clone the kind to release the borrow on fd_table/self
        let kind = fd_entry.kind.clone();

        match &kind {
            FdKind::StdIo(1) | FdKind::StdIo(2) => {
                // stdout/stderr: capture in output buffer
                self.output_buf.extend_from_slice(buf);
                Ok(buf.len())
            }
            FdKind::StdIo(0) => Err(9), // EBADF: can't write to stdin
            FdKind::StdIo(_) => Err(9),
            FdKind::PipeWrite(pipe_buf) => pipe_buf.lock().write(buf),
            FdKind::Device(device_type) => {
                let result = device_write(*device_type, buf);
                // Console/Tty output also goes to the output buffer
                if matches!(device_type, DeviceType::Console | DeviceType::Tty) {
                    self.output_buf.extend_from_slice(buf);
                }
                result
            }
            FdKind::RegularFile(file) => {
                let (ino, pos) = {
                    let f = file.lock();
                    (f.inode_num, f.position as usize)
                };
                // Read current content, expand/overwrite at position
                let mut data = self.fs.read_file_by_inode(ino).unwrap_or_default();
                if pos >= data.len() {
                    data.resize(pos, 0);
                    data.extend_from_slice(buf);
                } else {
                    let end = pos + buf.len();
                    if end > data.len() {
                        data.resize(end, 0);
                    }
                    data[pos..pos + buf.len()].copy_from_slice(buf);
                }
                let _ = self.fs.write_file_by_inode(ino, &data);
                file.lock().position += buf.len() as u64;
                Ok(buf.len())
            }
            _ => Err(9), // EBADF for unsupported fd types
        }
    }

    /// Read bytes from a file descriptor.
    ///
    /// Returns the number of bytes read, 0 for EOF, or a negative errno.
    pub fn sys_read(&mut self, fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
        let fd_entry = self.fd_table.get(fd).ok_or(9)?;
        let kind = fd_entry.kind.clone();

        match &kind {
            FdKind::StdIo(0) => {
                // stdin reads are handled externally by JavaScript
                Err(11) // EAGAIN
            }
            FdKind::StdIo(_) => Err(9),
            FdKind::PipeRead(pipe_buf) => pipe_buf.lock().read(buf),
            FdKind::Device(device_type) => device_read(*device_type, buf),
            FdKind::RegularFile(file) => {
                let (ino, pos) = {
                    let f = file.lock();
                    (f.inode_num, f.position as usize)
                };
                let data = self.fs.read_file_by_inode(ino).unwrap_or_default();
                if pos >= data.len() {
                    return Ok(0); // EOF
                }
                let available = &data[pos..];
                let to_read = buf.len().min(available.len());
                buf[..to_read].copy_from_slice(&available[..to_read]);
                file.lock().position += to_read as u64;
                Ok(to_read)
            }
            _ => Err(9),
        }
    }

    /// Open a file, returning a file descriptor number.
    ///
    /// Supports `O_CREAT` (create if missing) and `O_TRUNC` (truncate on open).
    pub fn sys_open(&mut self, path: &str, flags: u32, mode: u16) -> Result<i32, i32> {
        let resolved = self.resolve_path(path);
        let open_flags = OpenFlags::from_flags(flags);

        match self.fs.resolve_path(&resolved) {
            Ok(ino) => {
                // Path exists
                let inode = self.fs.inode(&resolved).map_err(|_| 2)?; // ENOENT
                if inode.is_dir() {
                    let dir_file = DirectoryFile {
                        inode_num: ino,
                        mount_id: 0,
                        position: 0,
                    };
                    self.fd_table
                        .alloc(FdKind::Directory(Arc::new(Mutex::new(dir_file))))
                } else {
                    if open_flags.truncate {
                        let _ = self.fs.truncate(&resolved);
                    }
                    let reg_file = RegularFile {
                        inode_num: ino,
                        mount_id: 0,
                        position: 0,
                        flags,
                    };
                    self.fd_table
                        .alloc(FdKind::RegularFile(Arc::new(Mutex::new(reg_file))))
                }
            }
            Err(VfsError::NotFound) if open_flags.create => {
                // Create new file
                let ino = self.fs.create_file(&resolved, mode).map_err(|_| 2)?;
                let reg_file = RegularFile {
                    inode_num: ino,
                    mount_id: 0,
                    position: 0,
                    flags,
                };
                self.fd_table
                    .alloc(FdKind::RegularFile(Arc::new(Mutex::new(reg_file))))
            }
            Err(_) => Err(2), // ENOENT
        }
    }

    /// Close a file descriptor.
    pub fn sys_close(&mut self, fd: i32) -> Result<(), i32> {
        let fd_entry = self.fd_table.close(fd)?;
        match fd_entry.kind {
            FdKind::PipeRead(buf) => buf.lock().close_read(),
            FdKind::PipeWrite(buf) => buf.lock().close_write(),
            _ => {}
        }
        Ok(())
    }

    /// Create a pipe, returning `(read_fd, write_fd)`.
    pub fn sys_pipe(&mut self) -> Result<(i32, i32), i32> {
        let (read_buf, write_buf) = pipe::create_pipe();
        let read_fd = self.fd_table.alloc(FdKind::PipeRead(read_buf))?;
        let write_fd = self.fd_table.alloc(FdKind::PipeWrite(write_buf))?;
        Ok((read_fd, write_fd))
    }

    /// Duplicate a file descriptor (lowest available slot).
    pub fn sys_dup(&mut self, old_fd: i32) -> Result<i32, i32> {
        self.fd_table.dup(old_fd)
    }

    /// Duplicate a file descriptor to a specific slot.
    pub fn sys_dup2(&mut self, old_fd: i32, new_fd: i32) -> Result<i32, i32> {
        self.fd_table.dup2(old_fd, new_fd)
    }

    /// Get the current working directory.
    pub fn sys_getcwd(&self) -> String {
        self.cwd.clone()
    }

    /// Change the current working directory.
    pub fn sys_chdir(&mut self, path: &str) -> Result<(), i32> {
        let resolved = self.resolve_path(path);
        if self.fs.is_dir(&resolved) {
            self.cwd = resolved.clone();
            self.env.insert(String::from("PWD"), resolved);
            Ok(())
        } else {
            Err(2) // ENOENT or ENOTDIR
        }
    }

    /// Get file status information (stat).
    pub fn sys_stat(&self, path: &str) -> Result<breenix_core::fs::vfs::FileStat, i32> {
        let resolved = self.resolve_path(path);
        self.fs.stat(&resolved).map_err(|_| 2) // ENOENT
    }

    /// List directory entries.
    pub fn sys_readdir(
        &self,
        path: &str,
    ) -> Result<Vec<breenix_core::fs::vfs::DirEntry>, i32> {
        let resolved = self.resolve_path(path);
        self.fs.readdir(&resolved).map_err(|e| match e {
            VfsError::NotFound => 2,      // ENOENT
            VfsError::NotDirectory => 20,  // ENOTDIR
            _ => 5,                        // EIO
        })
    }

    /// Remove a file (unlink).
    pub fn sys_unlink(&mut self, path: &str) -> Result<(), i32> {
        let resolved = self.resolve_path(path);
        self.fs.unlink(&resolved).map_err(|e| match e {
            VfsError::NotFound => 2,
            VfsError::IsDirectory => 21, // EISDIR
            _ => 5,
        })
    }

    /// Create a directory.
    pub fn sys_mkdir(&mut self, path: &str, mode: u16) -> Result<(), i32> {
        let resolved = self.resolve_path(path);
        self.fs.mkdir(&resolved, mode).map(|_| ()).map_err(|e| match e {
            VfsError::AlreadyExists => 17, // EEXIST
            VfsError::NotFound => 2,
            _ => 5,
        })
    }

    /// Remove an empty directory.
    pub fn sys_rmdir(&mut self, path: &str) -> Result<(), i32> {
        let resolved = self.resolve_path(path);
        self.fs.rmdir(&resolved).map_err(|e| match e {
            VfsError::NotFound => 2,
            VfsError::NotDirectory => 20,
            VfsError::IoError => 39, // ENOTEMPTY
            _ => 5,
        })
    }

    // ================================================================
    // Output management
    // ================================================================

    /// Drain the accumulated output buffer.
    ///
    /// This is the primary way JavaScript retrieves terminal output produced
    /// by syscall writes to stdout/stderr and console devices.
    pub fn take_output(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.output_buf)
    }

    /// Check if there is pending output to display.
    pub fn has_output(&self) -> bool {
        !self.output_buf.is_empty()
    }

    // ================================================================
    // Path resolution
    // ================================================================

    /// Resolve a path relative to the current working directory.
    ///
    /// Absolute paths are normalized in place; relative paths are joined
    /// with the cwd first and then normalized.
    pub fn resolve_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            normalize_path(path)
        } else {
            let mut full = self.cwd.clone();
            if !full.ends_with('/') {
                full.push('/');
            }
            full.push_str(path);
            normalize_path(&full)
        }
    }
}

impl Default for WasmKernel {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalize a path: resolve `.` and `..`, collapse consecutive slashes.
fn normalize_path(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            name => {
                components.push(name);
            }
        }
    }
    if components.is_empty() {
        String::from("/")
    } else {
        let mut result = String::new();
        for c in &components {
            result.push('/');
            result.push_str(c);
        }
        result
    }
}
