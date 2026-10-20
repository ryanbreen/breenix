//! `/bin/sync`: flush the mounted filesystems and their devices (POSIX `sync`). The persistent install's session
//! ends with a power-off, so its writes are flushed before it (docs/metal/README.md, "Persistent sessions").

fn main() {
    // SAFETY: sync takes no arguments and only flushes filesystem state.
    unsafe { libbreenix::syscall::raw::syscall0(libbreenix::syscall::nr::SYNC) };
}
