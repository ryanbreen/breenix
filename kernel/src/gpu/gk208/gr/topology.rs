/*
 * Copyright 2012 Red Hat Inc.
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
 *
 * Authors: Ben Skeggs
 */
/*
 * Copyright 2013 Red Hat Inc.
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
 *
 * Authors: Ben Skeggs <bskeggs@redhat.com>
 */
//! Pure decoder of the GF100 topology reads selected by GK208's one-PPC setup.
//! Source: Linux v6.8 e8f897f4afef0031fe618a8e94127a0934896aba,
//! nvkm/engine/gr/gf100.c:gf100_gr_oneinit and gk208.c:gk208_gr (MIT).

// The count occupies five bits; each PPC mask uses eight TPC bits.
pub const MAX_GPCS: usize = 31;
pub const MAX_TPCS: u32 = 8;

#[derive(Debug, PartialEq, Eq)]
pub struct Topology<'a> {
    pub gpc_nr: u8,
    pub tpc_nr: [u8; MAX_GPCS],
    pub tpc_mask: [u8; MAX_GPCS],
    /// Original address/value pairs, including the per-GPC PPC masks.
    pub raw: &'a [[u32; 2]],
}

fn read(raw: &[[u32; 2]], address: u32) -> Result<u32, &'static str> {
    let mut matches = raw.iter().filter(|pair| pair[0] == address);
    let value = matches.next().ok_or("missing GR topology register")?[1];
    if matches.next().is_some() {
        return Err("duplicate GR topology register");
    }
    Ok(value)
}

pub fn decode(raw: &[[u32; 2]]) -> Result<Topology<'_>, &'static str> {
    let gpc_nr = (read(raw, 0x409604)? & 0x1f) as u8;
    if gpc_nr == 0 || usize::from(gpc_nr) > MAX_GPCS {
        return Err("GR GPC count out of range");
    }
    if raw.len() != 1 + usize::from(gpc_nr) * 2 {
        return Err("unexpected GR topology register count");
    }
    let mut result = Topology {
        gpc_nr,
        tpc_nr: [0; MAX_GPCS],
        tpc_mask: [0; MAX_GPCS],
        raw,
    };
    for gpc in 0..gpc_nr {
        let base = 0x500000 + u32::from(gpc) * 0x8000;
        let count = read(raw, base + 0x2608)?;
        let mask = read(raw, base + 0x0c30)?;
        if count == 0 || count > MAX_TPCS || mask > 0xff || mask.count_ones() != count {
            return Err("GR TPC count or PPC mask out of range or inconsistent");
        }
        result.tpc_nr[usize::from(gpc)] = count as u8;
        result.tpc_mask[usize::from(gpc)] = mask as u8;
    }
    Ok(result)
}
