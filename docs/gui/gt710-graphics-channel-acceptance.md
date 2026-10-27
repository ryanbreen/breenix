# M7.2 graphics-channel acceptance handoff

No R240 boot was run for u6. The acceptance unit must collect the actual
record and bulk artifact and run `scripts/metal/gt710-suite`; this document
is an expectation, not hardware evidence. The console handoff-to-done limit
is 330 seconds. The added negative-control observation windows total 20 ms;
stream-consumption and successful releases have separate 2-second bounds.
The completed boot time remains unmeasured for this tree.

The channel is A06F channel 0 on the GR engine/runlist discovered from the
64 TOP words. A197 binds on subchannel 0. Its system-memory release target
is GPU VA 0x02000000; adjacent words hold the epoch halves. There are four
submissions: withheld D; clean poison 85; valid old-epoch release; clean
poison 170. Both clean submissions use the same per-boot nonce. A missing
engine receipt stops further submissions before re-poisoning the target.

The record carries the following schema-1 fields. Runtime placeholders
below must be replaced with device/allocator observations; they are not
literal strings emitted by the kernel. Firmware and init-list hashes,
method words, poison/control results, pending markers and fault addresses
are literal expectations. Context byte counts, topology-dependent storage,
VAs, epoch, nonce and times must come from the acceptance capture.
Fault values are captured before preemption/reset. Teardown records the
empty runlist count and unbound channel; allocator counters are read after
reset, VM detach and backing reclamation.

```json
{
  "schema": 1,
  "bdf": "<live PCI BDF>",
  "chipset": 262,
  "firmware": {
    "fecs.code.bin": {
      "sha256": "b3d192b4423fe1e150bd66b819374724ddb2ba9c40f92e4c83aba03139b85d3f",
      "readback_sha256": "b3d192b4423fe1e150bd66b819374724ddb2ba9c40f92e4c83aba03139b85d3f"
    },
    "fecs.data.bin": {
      "sha256": "3d4dc9a6c009e844ce7d538496cb267ad78984f0256ccfc29c73412a446806a6",
      "readback_sha256": "3d4dc9a6c009e844ce7d538496cb267ad78984f0256ccfc29c73412a446806a6"
    },
    "gpccs.code.bin": {
      "sha256": "ccc302340f23f6624bd278038b696b27eb20cc5a256547ea9a244aff6e3164b6",
      "readback_sha256": "ccc302340f23f6624bd278038b696b27eb20cc5a256547ea9a244aff6e3164b6"
    },
    "gpccs.data.bin": {
      "sha256": "67859e7b5e6e4002910d5ae94d2668828c867a7d0ef1787624dd97cc8839b405",
      "readback_sha256": "67859e7b5e6e4002910d5ae94d2668828c867a7d0ef1787624dd97cc8839b405"
    }
  },
  "status": [
    [
      4233216,
      "<live FECS word, bit 31 set>"
    ],
    [
      5253120,
      "<live GPCCS word, bit 31 set>"
    ]
  ],
  "elapsed_us": "<startup measured us, 0 < value <= 2000000>",
  "bound_us": 2000000,
  "fecs_size": "<FECS scratch-1 byte count>",
  "queries": [
    {
      "method": 1,
      "poison": 357913941,
      "reply": 2505397589,
      "size": "<fecs_size>",
      "elapsed_us": "<measured us>",
      "bound_us": 2000000
    },
    {
      "method": 2,
      "poison": 357913941,
      "reply": 2505397589,
      "size": "<fecs_size>",
      "elapsed_us": "<measured us>",
      "bound_us": 2000000
    },
    {
      "method": 1,
      "poison": 715827882,
      "reply": 2863311530,
      "size": "<fecs_size>",
      "elapsed_us": "<measured us>",
      "bound_us": 2000000
    },
    {
      "method": 2,
      "poison": 715827882,
      "reply": 2863311530,
      "size": "<fecs_size>",
      "elapsed_us": "<measured us>",
      "bound_us": 2000000
    }
  ],
  "withheld_start": {
    "started": false,
    "ready": false,
    "poison": 1431655765,
    "reply": 1431655765,
    "accepted": false
  },
  "stale_reply": {
    "method": 1,
    "poison": 0,
    "planted": 2147483648,
    "reply": 2147483648,
    "accepted": false
  },
  "topology_raw": [
    [
      4232708,
      "<live word>"
    ],
    [
      5252616,
      "<live word>"
    ],
    [
      5246000,
      "<live word>"
    ]
  ],
  "topology": {
    "gpc_nr": 1,
    "tpc_nr": [
      "<live count>"
    ],
    "tpc_mask": [
      "<live mask>"
    ]
  },
  "golden_bytes": "<FECS scratch-1 byte count>",
  "saved_end": "<computed from raw save_layout>",
  "save_layout": {
    "gpc": "<live word>",
    "strands": "<live word>",
    "strand_base": "<live word>",
    "strand_words": "<live word>",
    "units": [
      {"controller": 4231168, "mmio_base": "<MMCTX_SAVE_SWBASE>",
       "mmio_words": "<MMCTX_LOAD_COUNT>",
       "strands": [{"base": "<indexed STRAND_SAVE_SWBASE>", "words": "<indexed STRAND_WORDS>"}]},
      {"controller": 5251072, "mmio_base": "<MMCTX_SAVE_SWBASE>",
       "mmio_words": "<MMCTX_LOAD_COUNT>",
       "strands": [{"base": "<indexed STRAND_SAVE_SWBASE>", "words": "<indexed STRAND_WORDS>"}]}
    ]
  },
  "runnable_header": {
    "count": "<patch pair count>",
    "va": "<MMIO list VA>",
    "words": [
      "<patch pair count>",
      "<MMIO list VA >> 8>"
    ],
    "sha256": "<actual CPU-filled header digest>"
  },
  "image_bulk_bytes": "<actual GR image-row bytes>",
  "bulk_artifact_estimate": "<actual preceding bulk bytes plus GR image rows, <= 2097152>",
  "saves": [
    {
      "poison": 85,
      "va": "<golden-85 VA>",
      "sha256": "<same SHA-256 of written bytes in offset order>",
      "image_sha256": "<actual image-85 SHA-256>"
    },
    {
      "poison": 170,
      "va": "<golden-170 VA>",
      "sha256": "<same SHA-256 of written bytes in offset order>",
      "image_sha256": "<actual image-170 SHA-256>"
    }
  ],
  "init_lists": [
    {
      "name": "mmio",
      "count": 115,
      "sha256": "7e944a30c8d9f35bfefef91d0a73e4c1135510ad2615265af141cf75ced4e79e"
    },
    {
      "name": "icmd",
      "count": 245,
      "sha256": "e062254f9033263c696cb5f66ff2cd150f0ce771022fc6021cb696ec72195ff2"
    },
    {
      "name": "mthd",
      "count": 311,
      "sha256": "4004603ade727d25413eae88f9f796b1e0e3780a21f467095e54008b050a2041"
    },
    {
      "name": "hub",
      "count": 95,
      "sha256": "436d91aea0155fb03f1a402dff9d34c7849d9076f6e27e1663364e145976b5b5"
    },
    {
      "name": "gpc_0",
      "count": 28,
      "sha256": "cb9bd415e525aff81bb331a6dc07a302226baa5265baa9709123be009c847739"
    },
    {
      "name": "gpc_1",
      "count": 18,
      "sha256": "26ff9fdc13e750bbf4a6e773333d38af7ddb50ca75f0bc4c53c6e507a2764ee7"
    },
    {
      "name": "tpc",
      "count": 44,
      "sha256": "d9f5c076e64f442d3edfca20b8e11ca27b1dafad8b64dd477f8eed933963eb90"
    },
    {
      "name": "ppc",
      "count": 14,
      "sha256": "47d9ecd1b081049e83745541d580984c5a00b6290ef71ee5ed2a3a2c6ff35968"
    },
    {
      "name": "zcull",
      "count": 24,
      "sha256": "6627431341d439972b5f1ec0442db9fc79ff21ba084cc79061288c03f9b1681e"
    }
  ],
  "global_buffers": [
    {
      "name": "bundle",
      "va": "<mapped bundle VA>",
      "bytes": 12288,
      "align": 256
    },
    {
      "name": "pagepool",
      "va": "<mapped pagepool VA>",
      "bytes": 32768,
      "align": 256
    },
    {
      "name": "attrib",
      "va": "<mapped attrib VA>",
      "bytes": "<topology-dependent byte count>",
      "align": 4096
    },
    {
      "name": "mmio",
      "va": "<mapped mmio VA>",
      "bytes": 4096,
      "align": 256
    }
  ],
  "channel": {
    "id": 0,
    "epoch": "<current boot epoch>",
    "instance": "<owned instance physical address>",
    "class_id": 41367,
    "top": "<64 live TOP words>",
    "engine": "<TOP GR engine>",
    "runlist": "<TOP GR runlist>",
    "bind": "<0x80000000 | instance >> 12>",
    "state": "<live state: bit 0 set, runlist in bits 19:16>",
    "current_context": "<0x80000000 | instance >> 12>"
  },
  "releases": [
    {
      "class_id": 41367,
      "method": 6924,
      "word": 268496912,
      "target": 33554432,
      "poison": 85,
      "nonce": "<same per-boot nonce>",
      "observed": "<same per-boot nonce>",
      "epoch": "<current boot epoch>",
      "elapsed_us": "<measured us, 0 < value <= 2000000>",
      "bound_us": 2000000
    },
    {
      "class_id": 41367,
      "method": 6924,
      "word": 268496912,
      "target": 33554432,
      "poison": 170,
      "nonce": "<same per-boot nonce>",
      "observed": "<same per-boot nonce>",
      "epoch": "<current boot epoch>",
      "elapsed_us": "<measured us, 0 < value <= 2000000>",
      "bound_us": 2000000
    }
  ],
  "withheld_release": {
    "poison": 85,
    "observed": 85,
    "accepted": false
  },
  "stale_release": {
    "epoch": "<current boot epoch minus one>",
    "nonce": "<same per-boot nonce>",
    "observed": "<same per-boot nonce>",
    "accepted": false
  },
  "faults": [
    [
      4194560,
      0
    ],
    [
      4194568,
      0
    ],
    [
      9628,
      0
    ]
  ],
  "teardown": {
    "runlist_count": 0,
    "binding": 0
  },
  "baseline": {
    "reserved": "<same measured reserved count>",
    "payload": "<same measured payload count>",
    "guards": "<same measured guards count>",
    "charged": "<same measured charged count>",
    "display": "<same measured display count>",
    "engine": "<same measured engine count>",
    "probe": "<same measured probe count>",
    "free": "<same measured free count>",
    "allocations": "<same measured allocations count>"
  },
  "after": {
    "reserved": "<same measured reserved count>",
    "payload": "<same measured payload count>",
    "guards": "<same measured guards count>",
    "charged": "<same measured charged count>",
    "display": "<same measured display count>",
    "engine": "<same measured engine count>",
    "probe": "<same measured probe count>",
    "free": "<same measured free count>",
    "allocations": "<same measured allocations count>"
  },
  "firmware_writes": 0,
  "draw": "pending-7.4",
  "shaders": "pending-7.3",
  "context_switch": "pending-7.5"
}
```

<!-- claim-lint:ok: G7 requirements, exercised by tests/gt710_graphics_report_test.py and tests/gt710_grctx_test.rs. -->
Bulk context images use `GT710_GR_IMAGE` rows in the network artifact.
Serial carries only `GT710_GR_IMAGE_BULK <bytes> <sha256>` for those rows,
plus the enclosing artifact digest. The single `GT710_GR` JSON line carries
metadata and digests, not image bytes. The fixture/formatter check exercises
the complete record through the U1 comparator. G7 derives written-byte coverage independently from the two
units' firmware-computed MMIO and indexed strand registers captured after
method 2, citing MIT `hub.fuc:134-213`, `gpc.fuc:188-242` and
`com.fuc:314-333`. Written bytes must match; outside that set the original
poison must remain. Section allocation boundaries must account for the
FECS size. Runnable copies zero unwritten spans before their separate
CPU-filled header check. `GT710_GR_LAYOUT` also retains the map if context
validation refuses the probe. `GT710_GR_IRQ` records PMC, PGRAPH and FECS
status/enables and PCI INTx/MSI routing before and after the probe.

Open hardware observations for acceptance: GR runlist/PBDMA discovery,
context restore from the copied image and patched MMIO list, A197 releases
into coherent system memory, FECS current-context encoding, raw fault words,
preemption/drain/reset and allocator baseline, artifact capacity and console
time. U4/U5 startup and golden-context assumptions also await that boot.
Drawing, shaders and context switching remain pending 7.4, 7.3 and 7.5.
