/*
 * rnndb copyright 2006
 * Artur Huillet
 * Ben Skeggs
 * B. R.
 * Carlos Martin
 * Christoph Bumiller
 * Dawid Gajownik
 * Dmitry Baryshkov
 * Dmitry Eremin-Solenikov
 * EdB
 * Erik Waling
 * Francisco Jerez
 * Ilia Mirkin
 * jb17bsome
 * Jeremy Kolb
 * Laurent Carlier
 * Luca Barbieri
 * Maarten Maathuis
 * Marcelina Kościelnicka
 * Mark Carey
 * Matthieu Castet
 * nvidiaman
 * Patrice Mandin
 * Pekka Paalanen
 * Peter Popov
 * Richard Hughes
 * Rudi Cilibrasi
 * Serge Martin
 * Simon Raffeiner
 * Stephane Loeuillet
 * Stephane Marchesin
 * sturmflut
 * Sylvain Munaut
 * Victor Stinner
 * Wladmir van der Laan
 * Younes Manton
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 * 
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial
 * portions of the Software.
 * 
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE COPYRIGHT OWNER(S) AND/OR ITS SUPPLIERS BE
 * LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
 * WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */
//! Read-only pre-reset snapshots. Register fields come from pinned MIT rnndb
//! gf100_pgraph/{pgraph,gpc,tpc}.xml and fifo/gf100_pfifo.xml.
use alloc::{vec::Vec, vec};

pub fn capture(raw: &[[u32; 2]], pbdma: u32, mut read: impl FnMut(u32) -> u32)
    -> Result<Vec<(u32, u32)>, &'static str> {
    let topology = super::topology::decode(raw)?;
    if pbdma >= 32 { return Err("stall PBDMA index out of range"); }
    let mut addresses = vec![0x400100,0x400108,0x400110,0x400118,
        0x400380,0x400384,0x400388,0x400390,0x400604,0x400608,0x40060c,
        0x400700,0x400704,0x400708,0x40070c];
    // Live PBDMA state, unlike PGRAPH.TRAPPED_ADDR which is a trap record.
    let dma = 0x40000 + pbdma * 0x2000;
    for offset in [0x14,0x18,0x20,0x28,0x38,0x3c,0x40,0x44,0x5c,0x68,0x6c,0x70,0x84,0x88] {
        addresses.push(dma + offset);
    }
    for gpc in 0..topology.gpc_nr {
        let base = 0x500000 + u32::from(gpc) * 0x8000;
        for tpc in 0..8 {
            if topology.tpc_mask[usize::from(gpc)] & (1 << tpc) == 0 { continue; }
            let unit = base + 0x4000 + tpc * 0x800;
            // POLY, TEX, L1/LSU, MASTER and SM status; both 32-bit halves
            // of active, paused and trapped warp masks. No debug selector writes.
            for offset in [0x60,0x238,0x23c,0x2dc,0x410,0x500,
                0x614,0x618,0x624,0x628,0x634,0x638,0x644,0x648,0x64c,0x650,0x728] {
                addresses.push(unit + offset);
            }
        }
    }
    Ok(addresses.into_iter().map(|address| (address,read(address))).collect())
}
