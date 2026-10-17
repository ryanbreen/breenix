//! What the firmware's fixed ACPI tables say about the platform, read once at
//! boot before the TSC is calibrated: the root table's census, and from the
//! FADT the power-management timer, the reset register and whether MSI may be
//! used. Each firmware takes this path, QEMU's included.
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::acpi::{self, GenericAddress};

/// The PM timer's system I/O port, or 0 when the FADT names no timer in I/O
/// space.
static PM_TIMER_PORT: AtomicU32 = AtomicU32::new(0);
static PM_TIMER_32BIT: AtomicBool = AtomicBool::new(false);
/// Cleared when the FADT's IA-PC boot flags say MSI is not supported.
static MSI_ALLOWED: AtomicBool = AtomicBool::new(true);

/// A fixed-capacity line: a census longer than this is cut at a signature
/// boundary and says so.
struct Line {
    bytes: [u8; 240],
    len: usize,
}

impl Line {
    const fn new() -> Self {
        Self { bytes: [0; 240], len: 0 }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("[acpi] tables: <unprintable>")
    }
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > self.bytes.len() {
            return Err(core::fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// A Generic Address as `io:0x...` or `mem:0x...`.
struct Gas(GenericAddress);

impl core::fmt::Display for Gas {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0.space {
            0 => write!(f, "mem:{:#x}", self.0.address),
            1 => write!(f, "io:{:#x}", self.0.address),
            space => write!(f, "space{}:{:#x}", space, self.0.address),
        }
    }
}

/// Print the table census and the FADT line, and record what later code reads.
pub fn init(rsdp: Option<u64>, physical_memory_offset: u64) {
    let mut line = Line::new();
    let _ = line.write_str("[acpi] tables:");
    let mut cut = false;
    let visited = acpi::visit_table_signatures(rsdp, physical_memory_offset, |signature| {
        if cut {
            return;
        }
        let printable = signature.iter().all(|byte| byte.is_ascii_graphic());
        let text = if printable {
            core::str::from_utf8(&signature).unwrap_or("????")
        } else {
            "????"
        };
        let before = line.len;
        if write!(line, " {}", text).is_err() {
            line.len = before;
            cut = true;
        }
    });
    match visited {
        Some(_) if cut => log::info!("{} (and more)", line.as_str()),
        Some(_) => log::info!("{}", line.as_str()),
        None => log::info!("[acpi] tables: none (no usable RSDP or root table)"),
    }

    let Some(fadt) = acpi::read_fadt(rsdp, physical_memory_offset) else {
        log::info!("[acpi] fadt absent or invalid");
        return;
    };
    if fadt.msi_not_supported() {
        MSI_ALLOWED.store(false, Ordering::Release);
    }
    let timer = fadt.pm_timer.filter(|_| !fadt.hardware_reduced());
    if let Some(port) = timer.filter(|gas| gas.space == 1 && gas.address <= 0xffff) {
        PM_TIMER_PORT.store(port.address as u32, Ordering::Release);
        PM_TIMER_32BIT.store(fadt.pm_timer_32bit, Ordering::Release);
    }
    let mut pm_timer = Line::new();
    let _ = match timer {
        Some(gas) => write!(pm_timer, "{} bits={}", Gas(gas), if fadt.pm_timer_32bit { 32 } else { 24 }),
        None => pm_timer.write_str("none"),
    };
    let mut reset = Line::new();
    let _ = match fadt.reset {
        Some((register, value)) => write!(reset, "{} value={:#x}", Gas(register), value),
        None => reset.write_str("none"),
    };
    log::info!(
        "[acpi] fadt rev={} sci={} pm_timer={} reset={} boot_arch={:#06x} msi={}",
        fadt.revision,
        fadt.sci_interrupt,
        pm_timer.as_str(),
        reset.as_str(),
        fadt.boot_arch,
        if fadt.msi_not_supported() { "not-supported" } else { "allowed" }
    );
}

/// The PM timer's I/O port and whether it counts 32 bits, when the FADT names
/// one in system I/O space.
pub fn pm_timer() -> Option<(u16, bool)> {
    let port = PM_TIMER_PORT.load(Ordering::Acquire);
    (port != 0).then(|| (port as u16, PM_TIMER_32BIT.load(Ordering::Acquire)))
}

/// Whether the FADT permits MSI (an absent FADT does not forbid it).
pub fn msi_allowed() -> bool {
    MSI_ALLOWED.load(Ordering::Acquire)
}
