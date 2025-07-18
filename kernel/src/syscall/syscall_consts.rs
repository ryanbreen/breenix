/// Syscall number constants shared between kernel and userspace
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_GET_TIME: u64 = 4;
pub const SYS_YIELD: u64 = 5;
pub const SYS_GETPID: u64 = 6;
pub const SYS_FORK: u64 = 7;
pub const SYS_EXEC: u64 = 8;
pub const SYS_EXIT: u64 = 9;
pub const SYS_WAIT: u64 = 10;

// Test-only syscalls (only available with testing feature)
#[cfg(feature = "testing")]
pub const SYS_SHARE_TEST_PAGE: u64 = 400;
#[cfg(feature = "testing")]
pub const SYS_GET_SHARED_TEST_PAGE: u64 = 401;