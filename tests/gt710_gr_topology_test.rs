/*
 * Copyright 2011 Red Hat Inc.
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
// Hand calculations from Linux v6.8 e8f897f4afef0031fe618a8e94127a0934896aba,
// nvkm/engine/gr/gf100.c:gf100_gr_oneinit (lines 2009-2023) and
// gk208.c:gk208_gr (.ppc_nr = 1), both MIT; checked vendored headers.
// GPC_UNIT(g, o) = 0x500000 + g * 0x8000 + o.
#[path = "../kernel/src/gpu/gk208/gr/topology.rs"]
mod topology;
use topology::decode;

#[test]
fn full_part_ignores_unrelated_high_count_bits_and_retains_raw_pairs() {
    // low five bits of 0x00010001 = 1 GPC; 2 TPCs; popcount(3) = 2.
    let raw = [[0x409604, 0x00010001], [0x502608, 2], [0x500c30, 3]];
    let t = decode(&raw).unwrap();
    assert_eq!(t.gpc_nr, 1);
    assert_eq!(&t.tpc_nr[..1], &[2]);
    assert_eq!(&t.tpc_mask[..1], &[3]);
    assert!(t.tpc_nr[1..].iter().all(|v| *v == 0));
    assert!(t.tpc_mask[1..].iter().all(|v| *v == 0));
    assert_eq!(t.raw, raw);
}

#[test]
fn fused_off_tpc_preserves_the_surviving_bit_position() {
    // TPC bit 1 survives: count 1, mask 2; deriving mask 1 loses its position.
    let raw = [[0x500c30, 2], [0x409604, 1], [0x502608, 1]];
    let t = decode(&raw).unwrap();
    assert_eq!(t.gpc_nr, 1);
    assert_eq!(&t.tpc_nr[..1], &[1]);
    assert_eq!(&t.tpc_mask[..1], &[2]);
    assert_eq!(t.raw, raw);
}

#[test]
fn per_gpc_counts_follow_the_register_stride() {
    let raw = [[0x409604, 2], [0x502608, 2], [0x500c30, 3],
               [0x50a608, 1], [0x508c30, 1]];
    let t = decode(&raw).unwrap();
    assert_eq!(t.gpc_nr, 2);
    assert_eq!(&t.tpc_nr[..2], &[2, 1]);
    assert_eq!(&t.tpc_mask[..2], &[3, 1]);
}

#[test]
fn rejects_out_of_range_counts_and_inconsistent_or_incomplete_snapshots() {
    for raw in [
        vec![[0x409604, 0]],
        vec![[0x409604, 1], [0x502608, 9], [0x500c30, 0x1ff]],
        vec![[0x409604, 1], [0x502608, 0], [0x500c30, 0]],
        vec![[0x409604, 1], [0x502608, 2], [0x500c30, 1]],
        vec![[0x409604, 1], [0x502608, 1], [0x500c30, 0x100]],
        vec![[0x409604, 1], [0x502608, 1]],
        vec![[0x409604, 1], [0x409604, 1], [0x502608, 1], [0x500c30, 1]],
        vec![[0x409604, 2], [0x502608, 1], [0x500c30, 1]],
        vec![[0x409604, 1], [0x502608, 1], [0x500c30, 1], [0x1234, 0]],
    ] {
        assert!(decode(&raw).is_err(), "accepted {raw:x?}");
    }
}

// gpc.fuc:149-156 masks GPCCS_UNITS to five bits. The ZCULL count shares
// this word (rnndb ctxctl.xml:317-320), and is not another active TPC.
#[test]
fn tpc_count_excludes_zcull_units_and_preserves_the_raw_word() {
    let raw = [[0x409604, 0x00010001], [0x502608, 0x00010002], [0x500c30, 3]];
    let t = decode(&raw).unwrap();
    assert_eq!(t.tpc_nr[0], 2);
    assert_eq!(t.tpc_mask[0], 3);
    assert_eq!(t.raw, raw);
    for units in [0x10000, 0x10009] {
        assert!(decode(&[[0x409604, 1], [0x502608, units], [0x500c30, 3]]).is_err());
    }
}
