//! AArch64 CPU identification via system registers.
//!
//! Reads real hardware information from ARM64 system registers
//! and presents it in a format suitable for /proc/cpuinfo.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Once;

/// Cached CPU information, populated once at boot.
pub struct CpuInfo {
    /// MIDR_EL1: Main ID Register
    pub midr: u64,
    /// MPIDR_EL1: Multiprocessor Affinity Register
    pub mpidr: u64,
    /// ID_AA64ISAR0_EL1: Instruction Set Attribute Register 0
    pub isar0: u64,
    /// ID_AA64ISAR1_EL1: Instruction Set Attribute Register 1
    pub isar1: u64,
    /// ID_AA64PFR0_EL1: Processor Feature Register 0
    pub pfr0: u64,
    /// ID_AA64MMFR0_EL1: Memory Model Feature Register 0
    pub mmfr0: u64,
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
    let midr: u64;
    let mpidr: u64;
    let isar0: u64;
    let isar1: u64;
    let pfr0: u64;
    let mmfr0: u64;

    unsafe {
        core::arch::asm!("mrs {}, midr_el1", out(reg) midr, options(nomem, nostack));
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
        core::arch::asm!("mrs {}, id_aa64isar0_el1", out(reg) isar0, options(nomem, nostack));
        core::arch::asm!("mrs {}, id_aa64isar1_el1", out(reg) isar1, options(nomem, nostack));
        core::arch::asm!("mrs {}, id_aa64pfr0_el1", out(reg) pfr0, options(nomem, nostack));
        core::arch::asm!("mrs {}, id_aa64mmfr0_el1", out(reg) mmfr0, options(nomem, nostack));
    }

    CpuInfo {
        midr,
        mpidr,
        isar0,
        isar1,
        pfr0,
        mmfr0,
    }
}

impl CpuInfo {
    /// CPU implementer code from MIDR_EL1[31:24]
    pub fn implementer(&self) -> u8 {
        ((self.midr >> 24) & 0xFF) as u8
    }

    /// CPU variant from MIDR_EL1[23:20]
    pub fn variant(&self) -> u8 {
        ((self.midr >> 20) & 0xF) as u8
    }

    /// CPU architecture from MIDR_EL1[19:16]
    pub fn architecture(&self) -> u8 {
        ((self.midr >> 16) & 0xF) as u8
    }

    /// CPU part number from MIDR_EL1[15:4]
    pub fn part_number(&self) -> u16 {
        ((self.midr >> 4) & 0xFFF) as u16
    }

    /// CPU revision from MIDR_EL1[3:0]
    pub fn revision(&self) -> u8 {
        (self.midr & 0xF) as u8
    }

    /// Human-readable implementer name.
    pub fn implementer_name(&self) -> &'static str {
        match self.implementer() {
            0x41 => "ARM",
            0x42 => "Broadcom",
            0x43 => "Cavium",
            0x44 => "DEC",
            0x46 => "Fujitsu",
            0x48 => "HiSilicon",
            0x49 => "Infineon",
            0x4D => "Motorola/Freescale",
            0x4E => "NVIDIA",
            0x50 => "Applied Micro",
            0x51 => "Qualcomm",
            0x53 => "Samsung",
            0x56 => "Marvell",
            0x61 => "Apple",
            0x66 => "Faraday",
            0x69 => "Intel",
            0xC0 => "Ampere",
            _ => "Unknown",
        }
    }

    /// Human-readable part name for known cores.
    pub fn part_name(&self) -> &'static str {
        let imp = self.implementer();
        let part = self.part_number();
        match (imp, part) {
            // ARM cores
            (0x41, 0xD03) => "Cortex-A53",
            (0x41, 0xD04) => "Cortex-A35",
            (0x41, 0xD05) => "Cortex-A55",
            (0x41, 0xD07) => "Cortex-A57",
            (0x41, 0xD08) => "Cortex-A72",
            (0x41, 0xD09) => "Cortex-A73",
            (0x41, 0xD0A) => "Cortex-A75",
            (0x41, 0xD0B) => "Cortex-A76",
            (0x41, 0xD0C) => "Neoverse-N1",
            (0x41, 0xD0D) => "Cortex-A77",
            (0x41, 0xD40) => "Neoverse-V1",
            (0x41, 0xD41) => "Cortex-A78",
            (0x41, 0xD44) => "Cortex-X1",
            (0x41, 0xD46) => "Cortex-A510",
            (0x41, 0xD47) => "Cortex-A710",
            (0x41, 0xD48) => "Cortex-X2",
            (0x41, 0xD49) => "Neoverse-N2",
            (0x41, 0xD4A) => "Neoverse-E1",
            // Apple cores (QEMU may report as these)
            (0x61, 0x022) => "Icestorm (M1 efficiency)",
            (0x61, 0x023) => "Firestorm (M1 performance)",
            (0x61, 0x024) => "Icestorm (M1 Pro efficiency)",
            (0x61, 0x025) => "Firestorm (M1 Pro performance)",
            (0x61, 0x028) => "Blizzard (M2 efficiency)",
            (0x61, 0x029) => "Avalanche (M2 performance)",
            (0x61, 0x032) => "Sawtooth (M3 efficiency)",
            (0x61, 0x033) => "Everest (M3 performance)",
            // Qualcomm
            (0x51, 0x800) => "Kryo 260 (A73)",
            (0x51, 0x801) => "Kryo 260 (A53)",
            (0x51, 0xC00) => "Falkor",
            _ => "Unknown",
        }
    }

    /// Generate the features string from ID register fields.
    pub fn features_string(&self) -> String {
        let mut features: Vec<&str> = Vec::new();

        // Check PFR0 for basic feature presence
        let fp = (self.pfr0 >> 16) & 0xF;
        let advsimd = (self.pfr0 >> 20) & 0xF;

        if fp < 0xF {
            features.push("fp");
        }
        if advsimd < 0xF {
            features.push("asimd");
        }

        // ID_AA64ISAR0_EL1 fields
        let aes = (self.isar0 >> 4) & 0xF;
        if aes >= 1 { features.push("aes"); }
        if aes >= 2 { features.push("pmull"); }

        let sha1 = (self.isar0 >> 8) & 0xF;
        if sha1 >= 1 { features.push("sha1"); }

        let sha2 = (self.isar0 >> 12) & 0xF;
        if sha2 >= 1 { features.push("sha2"); }
        if sha2 >= 2 { features.push("sha512"); }

        let crc32 = (self.isar0 >> 16) & 0xF;
        if crc32 >= 1 { features.push("crc32"); }

        let atomic = (self.isar0 >> 20) & 0xF;
        if atomic >= 2 { features.push("atomics"); }

        let rdm = (self.isar0 >> 28) & 0xF;
        if rdm >= 1 { features.push("asimdrdm"); }

        let sha3 = (self.isar0 >> 32) & 0xF;
        if sha3 >= 1 { features.push("sha3"); }

        let sm3 = (self.isar0 >> 36) & 0xF;
        if sm3 >= 1 { features.push("sm3"); }

        let sm4 = (self.isar0 >> 40) & 0xF;
        if sm4 >= 1 { features.push("sm4"); }

        let dp = (self.isar0 >> 44) & 0xF;
        if dp >= 1 { features.push("asimddp"); }

        let fhm = (self.isar0 >> 48) & 0xF;
        if fhm >= 1 { features.push("asimdfhm"); }

        let ts = (self.isar0 >> 52) & 0xF;
        if ts >= 1 { features.push("flagm"); }
        if ts >= 2 { features.push("flagm2"); }

        let rndr = (self.isar0 >> 60) & 0xF;
        if rndr >= 1 { features.push("rng"); }

        // ID_AA64ISAR1_EL1 fields
        let dpb = self.isar1 & 0xF;
        if dpb >= 1 { features.push("dcpop"); }
        if dpb >= 2 { features.push("dcpodp"); }

        let jscvt = (self.isar1 >> 12) & 0xF;
        if jscvt >= 1 { features.push("jscvt"); }

        let fcma = (self.isar1 >> 16) & 0xF;
        if fcma >= 1 { features.push("fcma"); }

        let lrcpc = (self.isar1 >> 20) & 0xF;
        if lrcpc >= 1 { features.push("lrcpc"); }
        if lrcpc >= 2 { features.push("ilrcpc"); }

        let frintts = (self.isar1 >> 32) & 0xF;
        if frintts >= 1 { features.push("frint"); }

        let sb = (self.isar1 >> 36) & 0xF;
        if sb >= 1 { features.push("sb"); }

        let specres = (self.isar1 >> 40) & 0xF;
        if specres >= 1 { features.push("specres"); }

        let bf16 = (self.isar1 >> 44) & 0xF;
        if bf16 >= 1 { features.push("bf16"); }

        let i8mm = (self.isar1 >> 52) & 0xF;
        if i8mm >= 1 { features.push("i8mm"); }

        // PFR0: SVE, EL levels, etc.
        let sve = (self.pfr0 >> 32) & 0xF;
        if sve >= 1 { features.push("sve"); }

        let dit = (self.pfr0 >> 48) & 0xF;
        if dit >= 1 { features.push("dit"); }

        // MMFR0: physical address size
        let parange = self.mmfr0 & 0xF;
        let pa_bits = match parange {
            0 => 32,
            1 => 36,
            2 => 40,
            3 => 42,
            4 => 44,
            5 => 48,
            6 => 52,
            _ => 0,
        };
        if pa_bits > 0 {
            // Not reported as a feature flag, but useful info
        }

        let mut result = String::new();
        for (i, feat) in features.iter().enumerate() {
            if i > 0 {
                result.push(' ');
            }
            result.push_str(feat);
        }
        result
    }

    /// Physical address bits supported.
    pub fn pa_bits(&self) -> u8 {
        let parange = self.mmfr0 & 0xF;
        match parange {
            0 => 32,
            1 => 36,
            2 => 40,
            3 => 42,
            4 => 44,
            5 => 48,
            6 => 52,
            _ => 0,
        }
    }
}
