# Coverage blend costs (24.7-coverage-blend)

This unit follows the CE transfer change `223326bb` on
`metal-gt710/m7-8-7`. It implements the performance work in milestone 8's
PR 8.7 without a boot. The unchanged shared GPU suite remains the pixel
and timing oracle; the native 170-case gate is still acceptance work.

## Check first

Commit `3bf08258` adds the stream-cost check. Its first run compiled and
failed: the full-screen coverage stream allocated 3,443,484 bytes against a
65,536-byte budget, and its snapshot used tiled launch `0x206` instead of
pitch byte launch `0x386`. The phase fields were absent too. The check keeps
the exact full-screen destination copy, one draw, and three ordered
engine releases. It compares compacted/relocated payload and packets with
the existing encoder, rather than introducing another pixel oracle.

The final run allocates 5,640 bytes for that translation, including the
chunk vectors and packing. This host allocator metric counts requested
allocation/reallocation bytes, not CPU nanoseconds or GPU throughput.
The added parity case covers the 25 probe variants, empty fenced work,
solid batches and corruption/suppression controls. The probe fixture first
used an invalid divisor for routine 11; setting the documented valid
operand fixed the fixture without changing its comparisons.

## Change

Runtime translation starts with the mutable inputs. It no longer builds
and grows the shader/VAB prefix that `compact()` immediately discarded.
The retained channel catalogue still supplies code, and its VAB supplies
scratch. Bootstrap/admission/parking retain the complete-image encoder.
Channel publication still installs those bindings before a draw; retirement,
parking waits, references and process charges retain their existing rules.
The shader bytes and the shared integer formula are unchanged under G5.

A bulk whole-image CE region copy with equal width, height, texel size,
pitch, block height and byte extent now copies the complete padded byte
string with pitch source and destination and REMAP disabled. The alias
check precedes this choice. Partial, differing-layout, small and overlapping
copies retain their tiled/staged path. For 2560x1600 BGRA8 the snapshot
copies 17,039,360 bytes, including tiled padding, on the GPU; it is not
replaced with CPU pixels. Epoch and sequence releases are unchanged.
The initial implementation accidentally selected REMAP in the pitch helper;
the unchanged launch assertion caught it and the implementation was fixed.

The A0B5 fields follow NVIDIA open-gpu-doc `classes/dma-copy/cla0b5.h` at
`9fdf5c4062007929d9f4e6cbad9c9771fe61b880`, MIT, copyright 2019 NVIDIA
CORPORATION. Its file-local grant was read before use. The complete notice
remains in the implementation and packet check, with their consumer mapping
in `THIRD_PARTY_NOTICES`. No new external implementation source is imported.

## Cost observations and transport

The ABI grows from 72 to 80 words, preserving the earlier offsets. These
appended pairs appear in the case scorer's existing delta table:

| Pair prefix | Interval |
|---|---|
| `scratch_prepare` | CPU snapshot reservation, allocation and VM mapping |
| `scratch_copy` | Successful encoded CE copy publication to observed completion |
| `draw_execute` | Successful encoded draw publication to observed engine completion |
| `native_submit` | Adapter send CPU path, including packet preparation and publication |

The two execution intervals include queueing and reaper scheduling; they
are observed completion latency, not device timestamp queries. A failed or
reset request does not record successful execution. Enclosing syscall
`submit`/`wait` and translation/frame-preparation intervals remain available.
The copy slot includes other encoded region copies; coverage-throughput
has just its destination snapshot. These nested/overlapping intervals must
not be summed as disjoint time.

The expanded named JSON exceeded the existing 256 KiB host check. The
producer now sends the 48 phase words as `costs_v1` in fixed ABI order,
keeping the first 32 inventory words named. The scorer checks the exact
array length and unsigned integer domain, rejects mixed array/named cost
representations, and expands to the original names for analysis. The
serial anchor and final upload digest still cover the original bytes.
No size bound or existing heap check was loosened. A round-trip check runs
the production producer and mutates length, type, overflow and digest.
Its first run caught the missing normalization in the scorer's return path;
that implementation was repaired.

## Expected GPU time and the clock record

The earlier unit's 8.6 measurements were 227 ms for coverage, with 66 ms
in submit and 99 ms in waits. Those totals do not distinguish the snapshot
from the draw. The retained 8.6 GPU transcript contains no `GT710_CLOCK_V1`:
the clock-record unit was added afterwards and has not booted. Therefore
there is no measured GPC/memory frequency for this unit to substitute into
a board-time prediction. No frequency or hardware speedup is claimed here.

A useful conditional model for acceptance is, in milliseconds:

```
P = 4,096,000 pixels
B = effective device bandwidth in decimal GB/s
f = GPC clock in kHz from GT710_CLOCK_V1
R = effective scalar shader instructions issued per GPC cycle
Tcopy = 34.07872 / B
Tdraw ~= max(36.864 / B, 91 * P / (f * R))
Tgpu ~= Tcopy + Tdraw
```

The traffic terms count snapshot read+write and one R8 mask read, one BGRA
scratch read and one BGRA output write per pixel. The unchanged committed
coverage disassembly contains 91 non-scheduling instruction words and two
texel fetches; the issue model treats them uniformly, so it is only an
estimate. Cache misses, instruction-specific throughput, occupancy, launch,
parking and observation overhead can increase elapsed time. Memory PLL
frequency alone is not effective bandwidth and must not be relabelled as B.

For illustration, B=8 and R=128 give about 11.45 ms at f=405000 and
15.05 ms at f=270000; these are assumed scenarios, not board clock readings.
At f=270000 and R=64 the model is about 25.83 ms, already beyond 16.7 ms.
The traffic alone requires about 4.25 GB/s to fit 16.7 ms. The real gate
also includes CPU submit, waits and corner observation; the new records
must decide whether the remaining cost is mapping, CE or shader execution.
The 100000 kHz host-reference field is not the GPC clock and is excluded from
this calculation. Acceptance must print the actual startup/final clock
records with the case costs and retain the unchanged 16,700 us bound.

## Validation

The final new check passes four Python checks, including three executed
Rust stream cases and an executed runtime recorder case. Existing checks
pass: draw encoder 11, blend encoder 4, sampled encoder 6, coverage spans 9,
probe encoder 3, solid batching 2, tiled transfers 4, adapter 28, legacy
pitch copy 3 and shapes 6 (76 Rust cases). The original cost recorder check,
three heap-boundary checks, two catalogue-asset checks, two firmware checks,
five alpha-oracle/scorer checks and two performance-scorer checks pass.
The earlier heap-size failure was fixed by transport compaction, and its
original check passes unchanged. Host Rust checks compile with `-Dwarnings`
under `taskpolicy -b` and `CARGO_BUILD_JOBS=4`.

The x86 production release build (`cargo build --release --bin qemu-uefi`)
passed in 1m27s without warnings. The ARM64 production kernel release build
passed in 1m57s. Cargo reported the pinned toolchain's existing `core`
future-compatibility warning: NEON `target_feature` declarations under
`core_arch`, Rust issue #134375, outside this change. The dependency
`libbreenix-libc` release build passed without warnings. The scoped
`suite-gpu` build initially stopped on a noncanonical Rust-fork path, then
compiled but could not link because this checkout lacked `libc.a`; its
retry with the canonical fork path and built dependency passed in 9.98s
without warnings. The builds use `CARGO_BUILD_JOBS=4` under `taskpolicy -b`. No boot, persistent session, deployment, PR or merge
is part of this unit. Shared-suite pixels, performance bounds, the console
330 s handoff deadline and the GPU suite's own elapsed-time report remain
the acceptance contract.

Claim checks so far:

```
claim-lint: scripts/claim-lint.py --files tests/gt710_coverage_cost_test.py tests/gt710_coverage_cost_test.rs -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-coverage-blend/check-message -> exit 0
claim-lint: scripts/claim-lint.py -> exit 1 (unquantified comment; reworded)
claim-lint: scripts/claim-lint.py --files docs/gui/gt710-coverage-blend.md -> exit 1 (two phrases; reworded)
claim-lint: scripts/claim-lint.py --files docs/gui/gt710-coverage-blend.md -> exit 0
claim-lint: scripts/claim-lint.py -> exit 0
```

Final claim checks:

```
claim-lint: scripts/claim-lint.py --files docs/gui/gt710-coverage-blend.md -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-coverage-blend/change-message -> exit 0
claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files docs/gui/gt710-coverage-blend.md -> exit 0
```

The linter skips extensionless files, including the scorer and notices; the
compact transport and source mapping were reviewed directly and exercised
by the round-trip check where applicable.
