/*
 * Copyright 2017 Red Hat Inc.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */
//! GK104 40-bit DMA and GF100 coherent HOST PTE fields (checked MIT sources).
//! Platform structures: Intel VT-d Architecture Specification, DMAR and GSTS.
use alloc::vec::Vec;
pub const LIMIT: u64 = 1 << 40;
pub fn identity_page(address: u64, statuses: &[u32]) -> Result<u64, &'static str> {
    if statuses.iter().any(|s| s & (1 << 31) != 0) {
        return Err("active DMA translation needs a platform mapping");
    }
    if address == 0 || address & 4095 != 0 || address.checked_add(4096).is_none_or(|e| e > LIMIT) {
        return Err("DMA page exceeds the verified 40-bit aperture");
    }
    Ok(address)
}
pub fn coherent_pte(address: u64) -> Result<u64, &'static str> {
    Ok((identity_page(address, &[])? >> 8) | 1 | (1 << 32) | (2 << 33))
}
/// Parse only a checksum-valid, bounded DMAR and retain DRHD bases read-only.
pub fn dmar_units(bytes: &[u8]) -> Result<Vec<u64>, &'static str> {
    if bytes.len() < 48
        || bytes.len() > 65536
        || &bytes[..4] != b"DMAR"
        || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize != bytes.len()
        || bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0
    {
        return Err("invalid DMAR table");
    }
    let mut units = Vec::new();
    let mut at = 48;
    while at < bytes.len() {
        let header = bytes.get(at..at + 4).ok_or("truncated DMAR structure")?;
        let kind = u16::from_le_bytes(header[..2].try_into().unwrap());
        let len = u16::from_le_bytes(header[2..].try_into().unwrap()) as usize;
        if len < 4 || at + len > bytes.len() {
            return Err("invalid DMAR structure length");
        }
        if kind == 0 {
            if len < 16 || units.len() == 64 {
                return Err("invalid DRHD");
            }
            let base = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
            if base == 0
                || base & 4095 != 0
                || base.checked_add(4096).is_none_or(|e| e > 1 << 52)
                || units.contains(&base)
            {
                return Err("invalid DRHD register base");
            }
            units.push(base);
        }
        at += len;
    }
    if units.is_empty() {
        return Err("DMAR has no remapping unit");
    }
    Ok(units)
}
