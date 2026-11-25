//! Interpreter-to-libc TLS template ABI (ELF gABI §7.7, AMD64 psABI §4.4.1).

/// The initial load set has at most this many TLS-bearing objects.
pub const MAX_TLS_MODULES: usize = 32;

/// One template in the initial load set. Its one-based table index is the
/// module ID. The interpreter supplies a process-lifetime, relocated tdata image.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TlsModule {
    pub image: *const u8,
    pub file_size: usize,
    pub mem_size: usize,
    pub align: usize,
}
