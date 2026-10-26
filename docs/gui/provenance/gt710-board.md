# GK208B board inventory sources

E19 milestone 4 PR 4.1 uses the following MIT hardware layouts from nouveau
at Linux v6.8 commit `e8f897f4afef0031fe618a8e94127a0934896aba`.
The file-local grants were read before adaptation. Linux OS integration is not
included. The Rust files retain the source copyright and license headers.

| Source under `drivers/gpu/drm/nouveau/nvkm/` | SHA-256 | Consumer |
|---|---|---|
| `subdev/bios/bit.c` | `955da1bbd3288f978830d2d713406cbfe2929627a36769354fac8515495a9af8` | `kernel/src/gpu/gk208/board.rs` |
| `subdev/bios/dcb.c` | `328e5f6d7220e4ca45f1ceb2b3d1443d5e41a513673c5411ae199b7939586a0e` | `kernel/src/gpu/gk208/board.rs` |
| `subdev/bios/i2c.c` | `4d51892b5c771f01770ce092071f463422dff53e047e648ee7cca49d828e46aa` | `kernel/src/gpu/gk208/board.rs` |
| `subdev/bios/conn.c` | `8b658d114d554ac9623b5ef59a2d8ff22d87905b7c364f711a6d387634bf625d` | `kernel/src/gpu/gk208/board.rs` |
| `subdev/bios/pll.c` | `31b051120e80bcb546c5b1ab43abf10a05c9c5e029cf316223b828237ad12da4` | `kernel/src/gpu/gk208/board.rs` |
| `subdev/bios/shadowramin.c` | `5993608771cf9fd77db71b2983b00b5ec1e69ef6e534005bd6b2cfea47d31ffc` | `kernel/src/gpu/gk208/mod.rs` |
| `engine/disp/gf119.c` | `b00c2a325e0783dd7948694fda03520f46ed9eb656c16b04c6f4c47d32271522` | `kernel/src/gpu/gk208/mod.rs` |
| `subdev/i2c/busgf119.c` | `24cdf28deab20486c48b4702c473375b6d91e4e6a0286475e5f9f48ca88a17f7` | `kernel/src/gpu/gk208/mod.rs` |
| `subdev/i2c/padgf119.c` | `494eeea1b2b4418d2fee0e1e300739eacd1954d9d7c8459328fc370c9bf247fd` | `kernel/src/gpu/gk208/mod.rs` |
| `subdev/i2c/padg94.c` | `47680e51b2bd1f34473a898eed4f862ef930afafa7433cae214fa5645da9cdd3` | `kernel/src/gpu/gk208/mod.rs` |
| `subdev/devinit/gf100.c` | `260e57fd22b56e7f765b26ef014900e8e2e43e0fa9aac750734ffa7e9b0872be` | `kernel/src/gpu/gk208/mod.rs` |

The parser decodes only supported BIT/DCB/I2C/connector/PLL layouts. It does
not execute VBIOS scripts. The BIT C v2 payload supported here is exactly
the four-byte PLL-table pointer; larger payload layouts are rejected.

DDC transaction sequencing is written from NXP UM10204 (I2C) and VESA E-DDC.
EDID decoding is written from VESA E-EDID Release A Revision 2, Tables 3.19
and 3.22. Standard timing 81 00 maps to VESA DMT 1.0 Revision 13 p.46:
1280x800, 83.500 MHz, negative H-sync, positive V-sync (flags 28).

The source selection follows operator ruling G1 for GPU hardware enablement.
The remainder of the repository keeps its existing clean-room policy.
