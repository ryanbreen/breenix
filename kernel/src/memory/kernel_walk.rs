//! The physical address behind a kernel virtual address, for handing to a
//! device. A missing translation is an error: the caller must not program a
//! device with an address nobody mapped.
//!
//! Pure computation over the page-table entries or the translation result, so
//! the host tests compile this file as is.

/// Why an address has no physical translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalkError {
    /// x86-64: the entry at `level` (4 = PML4, 3 = PDPT, 2 = PD, 1 = PT) is not present.
    NotPresent { level: u8 },
    /// ARM64: the MMU's address translation reported a fault (PAR_EL1.F).
    Fault,
}

const PRESENT: u64 = 1;
const PAGE_SIZE: u64 = 1 << 7;
/// Bits 51:12 of an entry: the next table's or the page's physical address.
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

/// The x86-64 four-level walk (Intel SDM Vol. 3A section 4.5) from the PML4
/// at `root_phys`. `read(table_phys, index)` returns entry `index` of the
/// table at `table_phys`. A PDPT or PD entry with PS set maps a 1 GiB or 2 MiB
/// page; its address field is the page's base (bit 12 of such an entry is PAT,
/// not an address bit).
pub fn walk_x86_64(root_phys: u64, virt: u64, mut read: impl FnMut(u64, usize) -> u64) -> Result<u64, WalkError> {
    let mut table = root_phys & ADDR_MASK;
    for level in (1..=4u8).rev() {
        let shift = 12 + 9 * u32::from(level - 1);
        let entry = read(table, ((virt >> shift) & 0x1ff) as usize);
        if entry & PRESENT == 0 {
            return Err(WalkError::NotPresent { level });
        }
        if level == 1 || ((level == 2 || level == 3) && entry & PAGE_SIZE != 0) {
            let offset_mask = (1u64 << shift) - 1;
            return Ok((entry & ADDR_MASK & !offset_mask) | (virt & offset_mask));
        }
        table = entry & ADDR_MASK;
    }
    unreachable!("level 1 always returns")
}

/// The result of an ARM64 `AT S1E1R` for `virt`, read from PAR_EL1 (Arm ARM,
/// PAR_EL1): bit 0 (F) set means the translation faulted; otherwise bits 51:12
/// hold the output address of the page, to which the page offset is added.
pub fn decode_par_el1(par: u64, virt: u64) -> Result<u64, WalkError> {
    if par & 1 != 0 {
        return Err(WalkError::Fault);
    }
    Ok((par & ADDR_MASK) | (virt & 0xfff))
}
