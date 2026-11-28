//! Breenix's capability words and signal-stack sizing auxiliary entries.
pub const AT_HWCAP: u64 = 16;
pub const AT_HWCAP2: u64 = 26;
/// The tag number and kernel-frame semantics follow the published x86 ELF
/// auxiliary-vector interface, used by Breenix on both architectures.
/// https://docs.kernel.org/next/x86/elf_auxvec.html
pub const AT_MINSIGSTKSZ: u64 = 51;

/// Breenix assignments. These words have their own encoding.
pub mod x86 {
    pub const HWCAP_SSE2: u64 = 1 << 0;
    pub const HWCAP_AESNI: u64 = 1 << 1;
    pub const HWCAP_PCLMULQDQ: u64 = 1 << 2;
    pub const HWCAP_AVX: u64 = 1 << 3;
    pub const HWCAP_AVX2: u64 = 1 << 4;
    pub const HWCAP_AVX512F: u64 = 1 << 5;
    pub const HWCAP2_ERMS: u64 = 1 << 0;
    pub const HWCAP2_FSRM: u64 = 1 << 1;
}

/// The assigned bits follow the published Linux ARM64 AT_HWCAP ABI.
/// Breenix reserves HWCAP2 and clears unassigned bits.
/// https://docs.kernel.org/arch/arm64/elf_hwcaps.html
pub mod arm {
    pub const HWCAP_FP: u64 = 1 << 0;
    pub const HWCAP_ASIMD: u64 = 1 << 1;
    pub const HWCAP_AES: u64 = 1 << 3;
    pub const HWCAP_PMULL: u64 = 1 << 4;
    pub const HWCAP_SHA1: u64 = 1 << 5;
    pub const HWCAP_SHA2: u64 = 1 << 6;
    pub const HWCAP_CRC32: u64 = 1 << 7;
}
