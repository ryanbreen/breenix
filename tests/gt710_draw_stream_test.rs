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

/*******************************************************************************
    Copyright (c) 2020, NVIDIA CORPORATION. All rights reserved.

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
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
    THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
    FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    DEALINGS IN THE SOFTWARE.

*******************************************************************************/

/*
 * Copyright 2010 Christoph Bumiller
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
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */

/*
 * Copyright 2008 Ben Skeggs
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
 * THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 */

// Pinned references: NVIDIA open-gpu-doc 9fdf5c4062007929d9f4e6cbad9c9771fe61b880;
// Mesa 24.3.4 769e51468b49b2a42f0a0eaf71cf9eed5ff4e5de, nvc0/ paths.
// Run: rustc --edition=2021 --test tests/gt710_draw_stream_test.rs -o /tmp/gt710-draw-test
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/gr/channel.rs"]
pub mod channel;
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
pub mod shaders;
#[path = "../kernel/src/gpu/gk208/gr/draw.rs"]
pub mod draw;

fn request() -> draw::Draw {
    draw::Draw {
        target_va: 0x1234560000, pitch: 256, width: 64, height: 64,
        vertex_va: 0x2345670000, quarantine_va: 0x4567880000, vertex_program: 0x30, fragment_program: 0x1b0,
        report_va: 0x3456780000, epoch: 0x1122334455667788, nonce: 0xaabbccdd,
    }
}

#[test]
fn literal_a197_draw_stream() {
    let expected = [
        // Mesa 24.3.4 nvc0_screen.c:1274-1283; MIT rnndb gf100_3d.xml:990-998.
        // 1 MiB backing, 128 KiB alignment, high/low VA and size selector 3.
        0x200305ef, 0x00000045, 0x67880000, 0x00000003,
        // cla197.h:697-812; nvc0_surface.c:590-599 (linear WIDTH is pitch bytes; A8R8G8B8 is little-endian BGRA)
        0x20090200, 0x00000012, 0x34560000, 0x00000100, 0x00000040, 0x000000cf, 0x00001000, 0x00000001, 0x00000000, 0x00000000,
        // cla197.h:1685-1694; nvc0_surface.c:588
        0x20010487, 0x00000001,
        // cla197.h:2386-2387; nvc0_surface.c:601
        0x2001054e, 0x00000000,
        // cla197.h:1777-1780; nvc0_state.c:370
        0x200104b3, 0x00000000,
        // cla197.h:1818-1821; nvc0_state.c:372
        0x200104ba, 0x00000000,
        // cla197.h:2124-2127; nvc0_state.c:395
        0x200104e0, 0x00000000,
        // cla197.h:2440-2443; nvc0_state.c:411
        0x20010565, 0x00000000,
        // cla197.h:3176-3179; nvc0_state.c:377
        0x2001066f, 0x00000000,
        // cla197.h:819-835; JSON viewport scale/translate, z unchanged
        0x20060280, 0x42000000, 0x42000000, 0x3f800000, 0x42000000, 0x42000000, 0x00000000,
        // cla197.h:837-849; nvc0_screen.c:1340-1343; JSON 64x64 viewport
        0x20040300, 0x00400000, 0x00400000, 0x00000000, 0x3f800000,
        // cla197.h:1076-1090; nvc0_screen.c:1356-1359; JSON target extent
        0x20030380, 0x00000001, 0x00400000, 0x00400000,
        // cla197.h:1195-1201; nvc0_surface.c:584-586
        0x200203fd, 0x00400000, 0x00400000,
        // cla197.h:851-857; nvc0_screen.c:1323-1332; JSON window clip
        0x20020340, 0x00400000, 0x00400000,
        // cla197.h:3103-3106; enable window rectangle 0
        0x20010653, 0x00000001,
        // cla197.h:3108-3112; inside-any window clipping
        0x20010654, 0x00000000,
        // cla197.h:1070-1074; nvc0_screen.c:1316-1318
        0x2002037e, 0x00000000, 0x00000000,
        // cla197.h:2243-2249; nvc0_screen.c:1314-1315; top-left, no flip
        0x200104eb, 0x00000000,
        // cla197.h:3023-3026; nvc0_state.c:327; half-integer centres
        0x20010649, 0x00000000,
        // cla197.h:3028-3031; nvc0_screen.c:1338-1339
        0x2001064b, 0x00000001,
        // cla197.h:3038-3064; nvc0_screen.c:1345-1346; Mesa view-volume clip baseline
        0x2001064f, 0x00000002,
        // cla197.h:2311-2335; no user clip planes
        0x20010544, 0x00000000,
        // cla197.h:3007-3010; nvc0_state.c:279-280; cull disabled
        0x20010646, 0x00000000,
        // cla197.h:985-995; nvc0_state.c:273-276; fill both faces without MME macros
        0x2002036b, 0x00001b02, 0x00001b02,
        // cla197.h:2119-2122; nvc0_surface.c:1050
        0x200104d8, 0x00000000,
        // cla197.h:2114-2117; nvc0_screen.c:1193-1194
        0x200104d7, 0x00000000,
        // cla197.h:3186-3189; nvc0_state.c:135-143; logic op disabled
        0x20010671, 0x00000000,
        // cla197.h:1112-1115; nvc0_state.c:171; common colour mask
        0x200103e4, 0x00000001,
        // cla197.h:3245-3257; nvc0_state.c:177-178; four RGBA components writable
        0x20010680, 0x00001111,
        // cla197.h:2381-2384; nvc0_screen.c:1183-1184
        0x2001054d, 0x00000000,
        // cla197.h:2551-2563; nvc0_screen.c:1185-1186; one sample
        0x20010574, 0x00000000,
        // cla197.h:2389-2395; nvc0_screen.c:1187-1188
        0x2001054f, 0x00000000,
        // MIT envytools rnndb graph/gf100_3d.xml:217-222; one pass, sample shading disabled
        0x200101d5, 0x00000001,
        // cla197.h:1137-1147; nvc0_surface.c:1056-1060
        0x200403ef, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff,
        // cla197.h:2521-2524; linear colour writes
        0x2001056e, 0x00000000,
        // cla197.h:3240-3243; nvc0_screen.c:1174-1176; uncompressed target
        0x20010678, 0x00000000,
        // cla197.h:1788-1793; nvc0_screen.c:1195-1196; smooth shading
        0x200104b5, 0x00001d01,
        // cla197.h:1823-1826; no alpha test
        0x200104bb, 0x00000000,
        // cla197.h:559-562; nvc0_screen.c:1395-1396; raster enabled
        0x200100df, 0x00000001,
        // cla197.h:1127-1130; nvc0_screen.c:1397-1398; separate fragment outputs
        0x200103eb, 0x00000001,
        // cla197.h:2403-2409; nvc0_screen.c:1162-1163; unconditional render mode
        0x20010556, 0x00000001,
        // cla197.h:2725-2728; nvc0_surface.c:1401; primitive restart disabled
        0x20010591, 0x00000000,
        // cla197.h:2565-2568; nvc0_screen.c:1418; edge flag
        0x20010579, 0x00000001,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x20010433, 0x000000ff,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x20020438, 0x000000ff, 0x000000ff,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x2002043b, 0x000000ff, 0x000000ff,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200101d3, 0x0000003f,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200105aa, 0x00030003,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200105e5, 0x00020002,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200104ab, 0x00000000,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x20010086, 0x00000010,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x2001043f, 0x00000010,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200104a4, 0x00000010,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200204b6, 0x00000010, 0x00000010,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x20010450, 0x00000010,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x20010584, 0x0000000e,
        // cla197.h:2693-2697; vertex ID copy disabled
        0x20010587, 0x00000000,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200100c3, 0x00000000,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200100c0, 0x00000003,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200100b4, 0x003fffff,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200103f7, 0x00000001,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x20010670, 0x00000001,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200101d7, 0x00000003,
        // nvc0_screen.c:688-761; Kepler branch of nvc0_magic_3d_init
        0x200101ff, 0x00000001,
        // cla197.h:3600-3609; nvc0_screen.c:1399-1412; unused stages off
        0x20010800, 0x00000000,
        // cla197.h:3600-3609; nvc0_screen.c:1399-1412; unused stages off
        0x20010820, 0x00000020,
        // cla197.h:3600-3609; nvc0_screen.c:1399-1412; unused stages off
        0x20010830, 0x00000030,
        // cla197.h:3600-3609; nvc0_screen.c:1399-1412; unused stages off
        0x20010840, 0x00000040,
        // cla197.h:3600-3613; vertex SPH from 7.1/7.3
        0x20020810, 0x00000011, 0x00000030,
        // cla197.h:3618-3619; committed vertex metadata
        0x20010813, 0x00000008,
        // cla197.h:3600-3613; fragment SPH from 7.1/7.3
        0x20020850, 0x00000051, 0x000001b0,
        // cla197.h:3618-3619; committed fragment metadata
        0x20010853, 0x00000004,
        // cla197.h:2991-2994; nvc0_vbo.c:481-483; per-vertex stream
        0x20010620, 0x00000000,
        // cla197.h:3367-3380; nvc0_vbo.c:358-368; 32-byte stride
        0x20040700, 0x00001020, 0x00000023, 0x45670000, 0x00000000,
        // cla197.h:3588-3592; nvc0_vbo.c:371-376; inclusive end of three vertices
        0x200207c0, 0x00000023, 0x4567005f,
        // cla197.h:1569-1605; nvc0_vbo.c:155-160; float4 at offsets 0/16 to a[0x80]/a[0x90]
        0x20020458, 0x38200000, 0x38200800,
        // cla197.h:2277-2278; nvc0_vbo.c:1020; invalidate vertex fetch cache
        0x2001050b, 0x00000000,
        // cla197.h:273-288; invalidate instructions after upload
        0x20010087, 0x00000001,
        // cla197.h:2663-2691; nvc0_vbo.c:592-593; TRIANGLES
        0x20010586, 0x00000004,
        // cla197.h:941-945; nvc0_vbo.c:594-596; first 0, count 3
        0x2002035d, 0x00000000, 0x00000003,
        // cla197.h:2660-2661; nvc0_vbo.c:597; END
        0x20010585, 0x00000000,
        // cla197.h:52-53; WAIT_FOR_IDLE before report
        0x20010044, 0x00000000,
        // cla197.h:3274-3366; low epoch, flushed all-pipeline one-word release
        0x200406c0, 0x00000034, 0x56780004, 0x55667788, 0x1000f010,
        // cla197.h:3274-3366; high epoch
        0x200406c0, 0x00000034, 0x56780008, 0x11223344, 0x1000f010,
        // cla197.h:3274-3366; nonce last
        0x200406c0, 0x00000034, 0x56780000, 0xaabbccdd, 0x1000f010,
    ];
    assert_eq!(draw::commands(request()).unwrap(), expected);
}

#[test]
fn vertex_words_match_json_bits() {
    // The source data is GPU vertex input.
    let json = include_str!("../docs/gui/provenance/gt710-triangle.json");
    let mut expected = Vec::new();
    for vertex in json.split("\"clip\":").skip(1) {
        for key in [None, Some("\"rgba\":")] {
            let tail = key.map_or(vertex, |key| vertex.split_once(key).unwrap().1);
            let list = tail.split_once('[').unwrap().1.split_once(']').unwrap().0;
            expected.extend(list.split(',').map(|v| v.trim().parse::<f32>().unwrap().to_bits()));
        }
    }
    assert_eq!(expected.len(), 24);
    assert_eq!(draw::VERTEX_WORDS.as_slice(), expected);
    assert_eq!(draw::VERTEX_WORDS[0], 0xbf7e0000);
    assert_eq!(draw::VERTEX_WORDS[8], 0x3c000000);
}

#[test]
fn addresses_offsets_and_bounds_are_encoded_without_truncation() {
    let mut r = request();
    r.target_va += 0x100000000;
    r.vertex_va += 0x200000000;
    r.report_va += 0x300000000;
    r.quarantine_va += 0x400000000;
    r.pitch = 512;
    r.vertex_program = 0x3b0;
    r.fragment_program = 0x2b0;
    r.epoch = 0xfedcba9876543210;
    r.nonce = 0x12345678;
    let stream = draw::commands(r).unwrap();
    // Decode DMA_INCR only; no extra bind/draw/host release may sneak in.
    let mut methods = std::collections::BTreeMap::new();
    let mut reports = Vec::new();
    let mut i = 0;
    while i < stream.len() {
        let h = stream[i];
        assert_eq!(h & 0xe000e000, 0x20000000);
        assert_eq!((h >> 13) & 7, channel::SUBCHANNEL);
        let n = ((h >> 16) & 0x1fff) as usize;
        assert!(n > 0);
        let m = (h & 0x1fff) * 4;
        if m == 0x1b00 { reports.push(stream[i+1..i+1+n].to_vec()); }
        for j in 0..n { methods.insert(m + 4*j as u32, stream[i+1+j]); }
        i += n+1;
    }
    // envytools rnndb graph/gf100_3d.xml:217-222: bits 3:0 must be 1, 2, 4 or 8.
    assert!(matches!(methods[&0x0754] & 0xf, 1 | 2 | 4 | 8));
    for (m,v) in [(0x17bc,0x49),(0x17c0,0x67880000),(0x17c4,3),(0x800,0x13),(0x808,512),(0x1c04,0x25),(0x1f00,0x25),
                  (0x2044,0x3b0),(0x2144,0x2b0)] { assert_eq!(methods[&m],v); }
    assert_eq!(reports, [[0x37,0x56780004,0x76543210,0x1000f010],
                         [0x37,0x56780008,0xfedcba98,0x1000f010],
                         [0x37,0x56780000,0x12345678,0x1000f010]]);
    for bad in [draw::Draw { quarantine_va: 0, ..r },
                draw::Draw { quarantine_va: r.quarantine_va+4096, ..r },
                draw::Draw { quarantine_va: (1<<40)-(1<<17), ..r },
                draw::Draw { target_va: 0, ..r }, draw::Draw { target_va: 1<<40, ..r },
                draw::Draw { target_va: r.target_va+4, ..r }, draw::Draw { pitch: 255, ..r },
                draw::Draw { width: 0, ..r }, draw::Draw { height: 65, ..r },
                draw::Draw { vertex_va: (1<<40)-4, ..r }, draw::Draw { vertex_va: 0, ..r },
                draw::Draw { report_va: (1<<40)-4, ..r }, draw::Draw { report_va: r.report_va+1, ..r },
                draw::Draw { vertex_program: 0, ..r }, draw::Draw { fragment_program: 0x1b4, ..r },
                draw::Draw { epoch: 0, ..r }, draw::Draw { nonce: 0, ..r }] {
        assert!(draw::commands(bad).is_err());
    }
}
