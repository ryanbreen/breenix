/*
Native interface constants derived from NVIDIA open-gpu-doc at
9fdf5c4062007929d9f4e6cbad9c9771fe61b880:
classes/{host/cla06f.h,3d/cla197.h}:
Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.
classes/dma-copy/cla0b5.h:
Copyright (c) 2019, NVIDIA CORPORATION. All rights reserved.
classes/display/cl927d.h:
Copyright (c) 1993-2022 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Fault reasons from nouveau nvkm/engine/fifo/gk104.c (Linux v6.8):
Copyright 2012 Red Hat Inc.
Class selection from nouveau nvkm/engine/fifo/gk208.c (Linux v6.8):
Copyright 2013 Red Hat Inc.
2D limit from Mesa 24.3.4 src/gallium/drivers/nouveau/nvc0/nvc0_screen.c:
Copyright 2010 Christoph Bumiller

MIT License
Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
*/
//! Fixture checks for native capability evidence and the public observation row.
#[path = "../kernel/src/gpu/gk208/caps.rs"] pub mod caps;
#[path = "../kernel/src/gpu/completion.rs"] pub mod gpu_completion;
#[path = "../kernel/src/gpu/observation.rs"] pub mod observation;
use gpu_abi::{capability as c, capability_origin as o, record as r};
fn board() -> caps::BoardState {
    let mut b = caps::BoardState::empty();
    b.chipset = 0x106;
    b.channel = (0x123000, 17);
    b.graphics_ready = true;
    // Creation order is deliberately not numeric.
    b.created_classes = [0xa197, 0x927d, 0xa0b5, 0xa06f];
    b.scanout_count = 1;
    b.scanouts[0] = gpu_abi::ScanoutInfo { enabled: 1, width: 1280, height: 1024, reserved: 99 };
    b
}
#[test] fn caps_preserve_evidence_and_refuse_unprobed_pixels() {
    let b = board();
    let p = caps::Profile::from_board(&b, b.channel);
    let a = p.record(None);
    assert_eq!((a.version, a.size, a.backend), (1, 512, 6));
    assert_eq!((a.status, a.missing), (gpu_abi::status::MISSING_CAPABILITY, c::BGRA_RENDER));
    assert!(a.reason.starts_with(b"missing b8g8r8a8-render\0"));
    assert!(a.reason[23..].iter().all(|v| *v == 0));
    assert_eq!((a.features, a.capset_id, a.capset_version, a.shading_level), (1 << 9, 0xa197, 0, 0));
    assert_eq!(a.capset_count, 4);
    assert_eq!(a.capset_ids, [0x927d, 0xa06f, 0xa0b5, 0xa197, 0, 0, 0, 0]);
    assert_eq!((a.max_width, a.max_height), (16384, 16384));
    assert_eq!((a.profile, a.pipelines), (0, 0));
    assert_eq!(a.formats.map(|f| (f.format, f.flags)), [(0,0), (1,0), (2,0)]);
    assert_eq!(a.capability_origin(c::FEATURE_3D), o::HARDWARE_READ);
    assert_eq!(a.capability_origin(c::CAPABILITY_SET), o::CLASS_TABLE);
    assert_eq!(a.capability_origin(c::TARGET_SIZE), o::CHIP_LIMIT);
    for id in c::BGRA_RENDER..=c::REGION_COPY { assert_eq!(a.capability_origin(id), o::UNPROBED); }
    assert_eq!(a.capability_origins[1] & !1, 0);
    assert_eq!((a.scanout_count, a.scanouts[0].enabled, a.scanouts[0].width, a.scanouts[0].height), (1,1,1280,1024));
    assert!(a.scanouts.iter().all(|s| s.reserved == 0));
    assert!(a.scanouts[1..].iter().all(|s| s.enabled == 0 && s.width == 0 && s.height == 0));
    assert!(a.renderer.starts_with(b"GK208B\0"));
    for id in 1..=c::LAST {
        let expected = id.min(c::BGRA_RENDER);
        assert_eq!(p.admission(Some(id)), Err((gpu_abi::status::MISSING_CAPABILITY, expected)));
        let masked = p.record(Some(id));
        assert_eq!((masked.status, masked.missing), (gpu_abi::status::MISSING_CAPABILITY, expected));
        assert_eq!(p.record(None).missing, c::BGRA_RENDER);
    }
}
#[test] fn absent_objects_unknown_chip_and_stale_channel_never_gain_capabilities() {
    for id in gpu_abi::gk208::CAPSET_IDS {
        let mut b = board();
        b.created_classes.iter_mut().filter(|v| **v == id).for_each(|v| *v = 0);
        let a = caps::Profile::from_board(&b, b.channel).record(None);
        assert_eq!(a.capset_count, 3);
        assert!(!a.capset_ids.contains(&id));
        assert!(a.capset_ids[..3].windows(2).all(|v| v[0] < v[1]));
        assert_eq!(a.capset_id, if id == 0xa197 { 0 } else { 0xa197 });
        assert_eq!(a.status, gpu_abi::status::MISSING_CAPABILITY);
        assert_eq!(a.missing, if id == 0xa197 { c::FEATURE_3D } else { c::CAPABILITY_SET });
    }
    let b = board();
    for identity in [(0x124000,17), (0x123000,18), (0,0)] {
        let a = caps::Profile::from_board(&b, identity).record(None);
        assert_eq!((a.capset_count, a.features, a.missing), (0,0,c::FEATURE_3D));
        assert!(a.capability_origins.iter().all(|v| *v == 0));
    }
    let mut b = board(); b.chipset = 0x108;
    let a = caps::Profile::from_board(&b, b.channel).record(None);
    assert_eq!((a.max_width, a.max_height, a.capset_count, a.missing), (0,0,0,c::FEATURE_3D));
}
fn complete(sequence:u32, caller:u64) -> gpu_completion::CeCompletion {
    let q = gpu_completion::Request {context:0x123000,epoch:17,caller,sequence,resource:0x456000,generation:9};
    q.complete(gpu_completion::CeSample {context:q.context,epoch:q.epoch,sequence,consumed:sequence,healthy:true}).unwrap()
}
#[test] fn native_observation_maps_receipts_without_virtual_answers() {
    let version = observation::ResourceVersion {context:0x123000,epoch:17,resource:0x456000,generation:9};
    let n = observation::native_record(version,complete(1,9001),[complete(2,23),complete(3,42)], &[1,2,3,4], &[1,2,3,4]).unwrap();
    for upload in [false,true] {
        let mut out = [0u8; r::SIZE];
        n.write_backend_fields(&mut out, upload);
        let word = |off| u32::from_le_bytes(out[off..off+4].try_into().unwrap());
        let wide = |off| u64::from_le_bytes(out[off..off+8].try_into().unwrap());
        assert_eq!((word(r::BACKEND),word(r::BOUNDARY)),(6,r::BOUNDARY_NATIVE));
        assert_eq!((wide(r::CONTEXT),wide(r::RESOURCE),wide(r::GENERATION)),(version.context,version.resource,9));
        assert_eq!((wide(r::RENDER_FENCE),wide(r::READBACK_FENCE)),(9001,42));
        for off in [r::RENDER_STATUS,r::READBACK_STATUS,r::COPY_STATUS] {assert_eq!(word(off),gpu_abi::gk208::SUCCESS);}
        assert_eq!(word(r::RENDER_KIND),if upload {gpu_abi::gk208::TRANSFER_RELEASE} else {gpu_abi::gk208::DRAW_RELEASE});
        for off in [r::READBACK_KIND,r::COPY_KIND] {assert_eq!(word(off),gpu_abi::gk208::TRANSFER_RELEASE);}
        assert_eq!((wide(r::READBACK_SURFACE),wide(r::READBACK_MOB)),(0,0));
        assert_eq!(word(r::UNWRITTEN),0);
        assert!(out[r::RESERVED..].iter().all(|v| *v == 0));
    }
    assert!(observation::native_record(version,complete(1,9001),[complete(2,23),complete(3,42)], &[1,2,3,4], &[1,2,0,4]).is_err());
}

fn probed_board() -> caps::BoardState {
    let mut b = board();
    for id in c::BGRA_RENDER..=c::REGION_COPY {
        let expected = caps::probe_expected(id).unwrap();
        assert!(expected.len() > 512);
        b.probes.record(id, true, expected, expected);
    }
    b
}
#[test]
fn exact_executed_probes_admit_and_publish_only_measured_caps() {
    let b = probed_board();
    let p = caps::Profile::from_board(&b, b.channel);
    let a = p.record(None);
    assert_eq!((a.status, a.missing), (gpu_abi::status::ADMITTED, 0));
    for id in c::BGRA_RENDER..=c::REGION_COPY {
        assert_eq!(a.capability_origin(id), o::EXECUTABLE_PROBE);
        assert_eq!(p.admission(Some(id)), Err((gpu_abi::status::MISSING_CAPABILITY, id)));
    }
    assert_eq!(a.formats.map(|f| f.flags), [3, 3, 1]);
    assert_eq!(a.profile, 7);
    // Admission of arithmetic/format primitives does not claim the later catalogue.
    assert_eq!(a.pipelines, 0);
    for identity in [(0x124000,17), (0x123000,18)] {
        let stale = caps::Profile::from_board(&b, identity).record(None);
        assert_eq!(stale.capability_origin(c::REGION_COPY), o::UNPROBED);
    }
}
#[test]
fn bad_swizzle_block_height_byte_or_missing_execution_refuses_its_capability() {
    for id in c::BGRA_RENDER..=c::REGION_COPY {
        let expected = caps::probe_expected(id).unwrap();
        for fault in 0..6 {
            let mut b = probed_board();
            let mut bad = expected.to_vec();
            match fault {
                0 => for pixel in bad.chunks_exact_mut(4) { pixel.swap(0, 2); },
                // Wrong block height exchanges GOB 1 in column 0 with GOB 0
                // in column 1: rows 8..15 at x=0 versus rows 0..7 at x=16.
                1 => for y in 0..8 { for x in 0..16 { for k in 0..4 {
                    bad.swap(((y+8)*80+x)*4+k, (y*80+x+16)*4+k);
                } } },
                2 => bad[513] ^= 1,
                3 => (), // Exact bytes alone do not prove this boot ran the probe.
                4 => { bad.pop(); },
                5 => bad.fill(0xaa),
                _ => unreachable!(),
            }
            assert!(fault == 3 || bad != expected, "ineffective fault {fault}, capability {id}");
            // Exercise both destinations independently, after an earlier pass:
            // failed remeasurement must revoke any prior executable evidence.
            for second in [false, true] {
                let (a, z) = if second { (expected, bad.as_slice()) } else { (bad.as_slice(), expected) };
                b.probes.record(id, fault != 3, a, z);
                let a = caps::Profile::from_board(&b, b.channel).record(None);
                assert_eq!((a.status, a.missing), (gpu_abi::status::MISSING_CAPABILITY, id));
                assert_eq!(a.capability_origin(id), o::UNPROBED);
                let reason = format!("missing {}\0", c::name(id));
                assert!(a.reason.starts_with(reason.as_bytes()));
                for other in c::BGRA_RENDER..=c::REGION_COPY {
                    if other != id { assert_eq!(a.capability_origin(other), o::EXECUTABLE_PROBE); }
                }
            }
        }
    }
}
#[test]
fn probe_literals_cover_unique_addresses_and_the_integer_program_formula() {
    let colour = caps::probe_expected(c::BGRA_RENDER).unwrap();
    let rgba = caps::probe_expected(c::RGBA_RENDER).unwrap();
    let integers = caps::probe_expected(c::INTEGER_OPS).unwrap();
    let r8 = caps::probe_expected(c::R8_SAMPLE).unwrap();
    let inputs = include_bytes!("../kernel/src/gpu/gk208/caps/integer.inputs");
    let mut comparisons = [0; 2];
    let mut unique = std::collections::BTreeSet::new();
    for n in 0..1280usize {
        let p = [n as u8, (n>>8) as u8, (73*n+19) as u8, 128+(n%128) as u8];
        assert_eq!(&colour[4*n..4*n+4], &p);
        assert_eq!(&rgba[4*n..4*n+4], &[p[2],p[1],p[0],p[3]]);
        assert_eq!(&r8[4*n..4*n+4], &[0,0,n as u8,255]);
        let v = (37*(n>>8)+11) as u8;
        assert_eq!(&r8[5120+4*n..5120+4*n+4], &[0,0,v,255]);
        let [a,b,l,r]: [u32;4] = core::array::from_fn(|k|
            u32::from_le_bytes(inputs[16*n+4*k..16*n+4*k+4].try_into().unwrap()));
        comparisons[usize::from(a<b)] += 1;
        let q = a.wrapping_add(b) ^ a.wrapping_mul(b)
            ^ (a.wrapping_shl(l&31) | b.wrapping_shr(r&31)) ^ if a<b {u32::MAX} else {0};
        assert_eq!(&integers[4*n..4*n+4], &q.to_le_bytes());
        assert!(unique.insert(q), "integer pattern repeats a pixel");
    }
    assert!(comparisons.iter().all(|n| *n>0));
}
