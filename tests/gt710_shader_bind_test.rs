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

// Literal oracle: MIT cla197.h:2634-2638 and 3600-3619, cla06f.h DMA_INCR.
// Mesa 24.3.4 nvc0_screen.c:895-898 writes upper then lower code address;
// nvc0_program.c:775-805 and 817-856 place the SPH at the program offset.
extern crate alloc;
#[path = "../kernel/src/gpu/gk208/gr/channel.rs"]
pub mod channel;
#[path = "../kernel/src/gpu/gk208/gr/shaders.rs"]
pub mod shaders;
#[path = "../kernel/src/gpu/gk208/gr/binding.rs"]
pub mod binding;

#[test]
fn a197_shader_binding_and_release_are_literal_words() {
    assert_eq!(shaders::VERTEX_GPRS, 8);
    assert_eq!(shaders::FRAGMENT_GPRS, 4);
    assert_eq!(channel::SUBCHANNEL, 0);
    let words = binding::commands(
        0x12_34567000, 0x600000, 0x1122334455667788, 0x89abcdef, 5,
    ).unwrap();
    assert_eq!(words, [
        0x20010000, 0x0000a197,
        0x20020582, 0x00000012, 0x34567000,
        0x20020810, 0x00000011, 0x00000030,
        0x20010813, 0x00000008,
        0x20020850, 0x00000051, 0x000001b0,
        0x20010853, 0x00000004,
        0x200406c0, 0x00000000, 0x00600004, 0x55667788, 0x1000f010,
        0x200406c0, 0x00000000, 0x00600008, 0x11223344, 0x1000f010,
        0x200406c0, 0x00000000, 0x00600000, 0x89abcdef, 0x1000f010,
        0x20010014, 0x00000005,
    ]);
    for code in [0, 1 << 40, 0x10000001] {
        assert!(binding::commands(code, 0x600000, 1, 1, 5).is_err());
    }
}
