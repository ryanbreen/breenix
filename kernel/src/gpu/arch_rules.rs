//! Pure admission of translated device pages; OS mappings stay in `arch`.
use super::dma_policy;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    Arm64,
    X86_64,
}

/// Admit one device-contiguous span from page addresses in allocation order.
/// ARM callers supply IPAs, including the boot contract's relocation. Native
/// x86 callers supply physical frames and the observed DMA-remapping status.
pub fn device_span(
    arch: Architecture,
    pages: &[u64],
    offset: usize,
    len: usize,
    statuses: &[u32],
) -> Result<u64, &'static str> {
    if offset >= 4096 || len == 0 {
        return Err("invalid DMA span");
    }
    let bytes = offset.checked_add(len).ok_or("DMA span overflow")?;
    if pages.len() != bytes.div_ceil(4096) {
        return Err("DMA span backing length mismatch");
    }
    let first = pages[0];
    for (i, &page) in pages.iter().enumerate() {
        if page & 4095 != 0
            || first.checked_add((i as u64).checked_mul(4096).ok_or("DMA span overflow")?)
                != Some(page)
        {
            return Err("DMA span is not page-aligned and contiguous");
        }
        if arch == Architecture::X86_64 {
            dma_policy::identity_page(page, statuses)?;
        }
    }
    first.checked_add(bytes as u64).ok_or("DMA span overflow")?;
    first.checked_add(offset as u64).ok_or("DMA span overflow")
}
