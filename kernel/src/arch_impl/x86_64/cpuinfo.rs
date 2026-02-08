//! x86_64 CPU identification via CPUID instruction.
//!
//! Reads real hardware information from the CPU using the CPUID instruction
//! and presents it in a format suitable for /proc/cpuinfo.

use alloc::string::String;
use alloc::vec::Vec;
use core::arch::x86_64::__cpuid;
use spin::Once;

/// Cached CPU information, populated once at boot.
pub struct CpuInfo {
    /// Vendor ID string (e.g., "GenuineIntel", "AuthenticAMD")
    pub vendor_id: [u8; 12],
    /// Brand string from extended CPUID leaves (e.g., "Intel(R) Core(TM)...")
    pub brand_string: Option<[u8; 48]>,
    /// CPU family (adjusted)
    pub family: u32,
    /// CPU model (adjusted)
    pub model: u32,
    /// CPU stepping
    pub stepping: u32,
    /// Max standard CPUID leaf
    pub max_leaf: u32,
    /// Feature flags from leaf 1 ECX
    pub features_ecx: u32,
    /// Feature flags from leaf 1 EDX
    pub features_edx: u32,
    /// Extended feature flags from leaf 0x80000001 ECX
    pub ext_features_ecx: u32,
    /// Extended feature flags from leaf 0x80000001 EDX
    pub ext_features_edx: u32,
    /// Number of logical processors (from leaf 1 EBX[23:16])
    pub logical_processors: u32,
    /// CLFLUSH line size (from leaf 1 EBX[15:8]) * 8
    pub clflush_size: u32,
}

static CPU_INFO: Once<CpuInfo> = Once::new();

/// Initialize CPU detection. Must be called once during boot.
pub fn init() {
    CPU_INFO.call_once(detect_cpu);
}

/// Get a reference to the cached CPU info.
pub fn get() -> Option<&'static CpuInfo> {
    CPU_INFO.get()
}

fn detect_cpu() -> CpuInfo {
    // Leaf 0: vendor ID and max standard leaf
    let leaf0 = unsafe { __cpuid(0) };
    let max_leaf = leaf0.eax;

    let mut vendor_id = [0u8; 12];
    vendor_id[0..4].copy_from_slice(&leaf0.ebx.to_le_bytes());
    vendor_id[4..8].copy_from_slice(&leaf0.edx.to_le_bytes());
    vendor_id[8..12].copy_from_slice(&leaf0.ecx.to_le_bytes());

    // Leaf 1: family/model/stepping and feature flags
    let (family, model, stepping, features_ecx, features_edx, logical_processors, clflush_size) =
        if max_leaf >= 1 {
            let leaf1 = unsafe { __cpuid(1) };
            let raw_stepping = leaf1.eax & 0xF;
            let raw_model = (leaf1.eax >> 4) & 0xF;
            let raw_family = (leaf1.eax >> 8) & 0xF;
            let ext_model = (leaf1.eax >> 16) & 0xF;
            let ext_family = (leaf1.eax >> 20) & 0xFF;

            // Adjusted family/model per Intel CPUID spec
            let adj_family = if raw_family == 0xF {
                raw_family + ext_family
            } else {
                raw_family
            };
            let adj_model = if raw_family == 0x6 || raw_family == 0xF {
                (ext_model << 4) | raw_model
            } else {
                raw_model
            };

            let logical = (leaf1.ebx >> 16) & 0xFF;
            let clflush = ((leaf1.ebx >> 8) & 0xFF) * 8;

            (
                adj_family,
                adj_model,
                raw_stepping,
                leaf1.ecx,
                leaf1.edx,
                logical,
                clflush,
            )
        } else {
            (0, 0, 0, 0, 0, 1, 0)
        };

    // Extended leaves
    let leaf_ext0 = unsafe { __cpuid(0x80000000) };
    let max_ext_leaf = leaf_ext0.eax;

    let (ext_features_ecx, ext_features_edx) = if max_ext_leaf >= 0x80000001 {
        let leaf_ext1 = unsafe { __cpuid(0x80000001) };
        (leaf_ext1.ecx, leaf_ext1.edx)
    } else {
        (0, 0)
    };

    // Brand string (leaves 0x80000002-0x80000004)
    let brand_string = if max_ext_leaf >= 0x80000004 {
        let mut brand = [0u8; 48];
        for i in 0..3u32 {
            let leaf = unsafe { __cpuid(0x80000002 + i) };
            let offset = (i as usize) * 16;
            brand[offset..offset + 4].copy_from_slice(&leaf.eax.to_le_bytes());
            brand[offset + 4..offset + 8].copy_from_slice(&leaf.ebx.to_le_bytes());
            brand[offset + 8..offset + 12].copy_from_slice(&leaf.ecx.to_le_bytes());
            brand[offset + 12..offset + 16].copy_from_slice(&leaf.edx.to_le_bytes());
        }
        Some(brand)
    } else {
        None
    };

    CpuInfo {
        vendor_id,
        max_leaf,
        brand_string,
        family,
        model,
        stepping,
        features_ecx,
        features_edx,
        ext_features_ecx,
        ext_features_edx,
        logical_processors,
        clflush_size,
    }
}

impl CpuInfo {
    /// Get vendor ID as a string slice.
    pub fn vendor_str(&self) -> &str {
        core::str::from_utf8(&self.vendor_id).unwrap_or("Unknown")
    }

    /// Get brand/model name string.
    pub fn brand_str(&self) -> &str {
        if let Some(ref brand) = self.brand_string {
            // Find the null terminator or end of array
            let len = brand.iter().position(|&b| b == 0).unwrap_or(48);
            core::str::from_utf8(&brand[..len])
                .unwrap_or("Unknown")
                .trim()
        } else {
            "Unknown"
        }
    }

    /// Query the last-level cache size in KB using CPUID leaf 0x04.
    ///
    /// Iterates deterministic cache parameters (leaf 4, sub-leaves 0,1,2...)
    /// and returns the size of the highest-level cache found.
    pub fn cache_size_kb(&self) -> u32 {
        if self.max_leaf < 4 {
            return 0;
        }

        let mut last_level_size_kb = 0u32;

        for sub_leaf in 0..16u32 {
            let result = unsafe { core::arch::x86_64::__cpuid_count(4, sub_leaf) };
            let cache_type = result.eax & 0x1F;
            if cache_type == 0 {
                break; // No more caches
            }

            let ways = ((result.ebx >> 22) & 0x3FF) + 1;
            let partitions = ((result.ebx >> 12) & 0x3FF) + 1;
            let line_size = (result.ebx & 0xFFF) + 1;
            let sets = result.ecx + 1;

            let size_bytes = ways * partitions * line_size * sets;
            let size_kb = size_bytes / 1024;

            if size_kb > last_level_size_kb {
                last_level_size_kb = size_kb;
            }
        }

        last_level_size_kb
    }

    /// Generate the flags string from CPUID feature bits.
    pub fn flags_string(&self) -> String {
        let mut flags: Vec<&str> = Vec::new();

        // EDX feature flags (leaf 1)
        let edx = self.features_edx;
        if edx & (1 << 0) != 0 { flags.push("fpu"); }
        if edx & (1 << 1) != 0 { flags.push("vme"); }
        if edx & (1 << 2) != 0 { flags.push("de"); }
        if edx & (1 << 3) != 0 { flags.push("pse"); }
        if edx & (1 << 4) != 0 { flags.push("tsc"); }
        if edx & (1 << 5) != 0 { flags.push("msr"); }
        if edx & (1 << 6) != 0 { flags.push("pae"); }
        if edx & (1 << 7) != 0 { flags.push("mce"); }
        if edx & (1 << 8) != 0 { flags.push("cx8"); }
        if edx & (1 << 9) != 0 { flags.push("apic"); }
        if edx & (1 << 11) != 0 { flags.push("sep"); }
        if edx & (1 << 12) != 0 { flags.push("mtrr"); }
        if edx & (1 << 13) != 0 { flags.push("pge"); }
        if edx & (1 << 14) != 0 { flags.push("mca"); }
        if edx & (1 << 15) != 0 { flags.push("cmov"); }
        if edx & (1 << 16) != 0 { flags.push("pat"); }
        if edx & (1 << 17) != 0 { flags.push("pse36"); }
        if edx & (1 << 19) != 0 { flags.push("clflush"); }
        if edx & (1 << 23) != 0 { flags.push("mmx"); }
        if edx & (1 << 24) != 0 { flags.push("fxsr"); }
        if edx & (1 << 25) != 0 { flags.push("sse"); }
        if edx & (1 << 26) != 0 { flags.push("sse2"); }
        if edx & (1 << 28) != 0 { flags.push("ht"); }

        // ECX feature flags (leaf 1)
        let ecx = self.features_ecx;
        if ecx & (1 << 0) != 0 { flags.push("sse3"); }
        if ecx & (1 << 1) != 0 { flags.push("pclmulqdq"); }
        if ecx & (1 << 3) != 0 { flags.push("monitor"); }
        if ecx & (1 << 9) != 0 { flags.push("ssse3"); }
        if ecx & (1 << 12) != 0 { flags.push("fma"); }
        if ecx & (1 << 13) != 0 { flags.push("cx16"); }
        if ecx & (1 << 19) != 0 { flags.push("sse4_1"); }
        if ecx & (1 << 20) != 0 { flags.push("sse4_2"); }
        if ecx & (1 << 21) != 0 { flags.push("x2apic"); }
        if ecx & (1 << 22) != 0 { flags.push("movbe"); }
        if ecx & (1 << 23) != 0 { flags.push("popcnt"); }
        if ecx & (1 << 25) != 0 { flags.push("aes"); }
        if ecx & (1 << 26) != 0 { flags.push("xsave"); }
        if ecx & (1 << 28) != 0 { flags.push("avx"); }
        if ecx & (1 << 29) != 0 { flags.push("f16c"); }
        if ecx & (1 << 30) != 0 { flags.push("rdrand"); }
        if ecx & (1u32 << 31) != 0 { flags.push("hypervisor"); }

        // Extended EDX features (leaf 0x80000001)
        let ext_edx = self.ext_features_edx;
        if ext_edx & (1 << 11) != 0 { flags.push("syscall"); }
        if ext_edx & (1 << 20) != 0 { flags.push("nx"); }
        if ext_edx & (1 << 26) != 0 { flags.push("pdpe1gb"); }
        if ext_edx & (1 << 27) != 0 { flags.push("rdtscp"); }
        if ext_edx & (1 << 29) != 0 { flags.push("lm"); }

        // Extended ECX features (leaf 0x80000001)
        let ext_ecx = self.ext_features_ecx;
        if ext_ecx & (1 << 0) != 0 { flags.push("lahf_lm"); }
        if ext_ecx & (1 << 5) != 0 { flags.push("abm"); }
        if ext_ecx & (1 << 6) != 0 { flags.push("sse4a"); }

        let mut result = String::new();
        for (i, flag) in flags.iter().enumerate() {
            if i > 0 {
                result.push(' ');
            }
            result.push_str(flag);
        }
        result
    }
}
