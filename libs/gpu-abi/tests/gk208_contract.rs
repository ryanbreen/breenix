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
//! Native row of docs/gui/gpu.md section 1.8; hardware source symbols are
//! listed there. These checks pin the ABI, not hardware execution.
use core::mem::{offset_of, size_of};
use gpu_abi::{backend, capability, capability_origin as origin, feature, gk208, record, Caps, Plain};

#[test]
fn native_backend_constants_are_class_qualified_and_disjoint() {
    assert_eq!(backend::GK208, 6);
    assert_eq!(feature::NATIVE_CLASSES, 1 << 9);
    assert_eq!(feature::NATIVE_CLASSES & (feature::VIRGL | feature::VERSION_1 | feature::DX_CONTEXT), 0);
    assert_eq!(record::BOUNDARY_NATIVE, 5);
    assert_ne!(record::BOUNDARY_NATIVE, record::BOUNDARY_HOST);
    assert_eq!(gk208::CAPSET_IDS, [0x927d, 0xa06f, 0xa0b5, 0xa197]);
    assert!(gk208::CAPSET_IDS.windows(2).all(|ids| ids[0] < ids[1]));
    assert_eq!(gk208::CAPSET_ID, 0xa197);
    assert_eq!(gk208::CAPSET_VERSION, 0);
    assert_eq!(gk208::SHADING_LEVEL, 0);
    assert_eq!(gk208::MAX_2D_SIZE, 16_384);
    assert_eq!(gk208::DRAW_RELEASE, 0xa197_1b0c);
    assert_eq!(gk208::TRANSFER_RELEASE, 0xa0b5_0300);
    assert_eq!(gk208::EVO_UPDATE_LATCH, 0x927d_0080);
    assert_eq!(gk208::SUCCESS, 0x6000_0000);
    assert_eq!(gk208::MMU_FAULT_FIRST, 0x6001_0000);
    assert_eq!(gk208::MMU_FAULT_LAST, 0x6001_000f);
    assert_eq!(gk208::MMU_FAULT_PDE, 0x6001_0000);
    assert_eq!(gk208::MMU_FAULT_PTE, 0x6001_0002);
    let kinds = [gk208::DRAW_RELEASE, gk208::TRANSFER_RELEASE, gk208::EVO_UPDATE_LATCH];
    let virtual_kinds = [0, 0x205, 0x206, 0x207, 1042, 1102, 1104, 1152];
    for (i, kind) in kinds.iter().enumerate() {
        assert!(!virtual_kinds.contains(kind));
        assert!(!kinds[i + 1..].contains(kind));
    }
    for answer in [gk208::SUCCESS, gk208::MMU_FAULT_FIRST, gk208::MMU_FAULT_LAST] {
        assert!(![0, 1, 3, 0x1100].contains(&answer));
        assert!(!(0x1200..=0x12ff).contains(&answer));
    }
    assert!(gk208::SUCCESS < gk208::MMU_FAULT_FIRST);
}

#[test]
fn native_origins_fit_caps_without_moving_the_wire_layouts() {
    assert_eq!([origin::UNPROBED, origin::CLASS_TABLE, origin::CHIP_LIMIT,
        origin::HARDWARE_READ, origin::EXECUTABLE_PROBE], [0, 1, 2, 3, 4]);
    assert_eq!(size_of::<Caps>(), 512);
    assert_eq!(offset_of!(Caps, backend), 16);
    assert_eq!(offset_of!(Caps, capset_ids), 36);
    assert_eq!(offset_of!(Caps, capability_origins), 120);
    assert_eq!(offset_of!(Caps, renderer), 128);
    assert_eq!(offset_of!(Caps, scanouts), 256);
    let mut caps = Caps::ZERO;
    caps.backend = backend::GK208;
    for id in 1..=capability::LAST {
        assert_eq!(caps.capability_origin(id), origin::UNPROBED);
        for value in 1..=4 {
            // Independent wire packing: 3 bits per id, id 1 first; id 11
            // straddles the two u32s. Do not use an encoder under test.
            let packed = u64::from(value) << ((id - 1) * 3);
            caps.capability_origins = [packed as u32, (packed >> 32) as u32];
            for other in 1..=capability::LAST {
                assert_eq!(caps.capability_origin(other), if other == id {value} else {0});
            }
        }
        caps.capability_origins = [0, 0];
    }
    assert_eq!(caps.version, 0); // no constructor silently claims admission
    assert_eq!(caps.capability_origin(0), origin::UNPROBED);
    assert_eq!(caps.capability_origin(capability::LAST + 1), origin::UNPROBED);
    assert_eq!(record::VERSION, 2);
    assert_eq!(record::SIZE, 1024);
    assert_eq!([record::BACKEND, record::BOUNDARY, record::CONTEXT, record::RESOURCE,
        record::RENDER_KIND, record::READBACK_KIND, record::COPY_KIND,
        record::PRESENT_STATUS, record::RENDERER, record::RESERVED],
        [16, 20, 64, 80, 116, 132, 152, 192, 712, 776]);
    assert_eq!(size_of::<[u8; record::SIZE]>(), 1024);
}
