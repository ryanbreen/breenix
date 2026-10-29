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
