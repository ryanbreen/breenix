# CE transfer costs (24.7-ce-transfers)

This unit follows `d363d0c8` and the batched-draws report on
`metal-gt710/m7-8-7`. It changes transfer preparation and packet generation;
hardware performance and the full milestone 8.7 gate remain for acceptance.

## Check first

Commit `b52cf04a` adds `tests/gt710_ce_transfers_test.py`. It executes the
production upload loops with counted page/uaccess services, and the production
readback band geometry with the CE encoder. Its 16 MiB matching-format upload
permits at most four API byte visits per byte; the baseline made five
(83,886,080 visits for 16,777,216 bytes). This metric counts API span visits
and converted-channel input bytes, not physical memory-bus traffic or time.
The full-image observation requires two data launches, one per poison; the
baseline needed four. Clipped format conversion already passed. The check
also requires byte-mode bulk packets, exact byte widths and the original
three ordered epoch/sequence releases, plus the phase names in the recorder,
ABI and scorer.

The first invocation exposed two fixture compile errors (a trait import and
an unused namespace); correcting them exposed the traffic and launch failures.
Later fixture extraction changes include the new cost guards and cache
publication without changing any assertion or budget. The existing cost check
keeps its original slot assertions and checks appended slots separately.

## Change

Upload copies user rows directly into charged staging pages. Matching BGRA,
RGBA and R8 formats then copy between page intersections, without stack
round-trips or per-pixel channel conversion. Unlike formats retain the channel
conversion. The complete source still stages before resource backing changes,
so a user fault cannot publish a partial upload. Quotas, charges and device
holds keep their existing lifetimes.

Native observations use bands of at most 16 MiB, so both the 2048x2048 upload
image and the 2560x1600 full-mode rectangle need one transfer per poison.
The other backends keep their 8 MiB bands. The existing backing receives both
poisons in order; staging retains the first completed image. The second
observation invalidates and compares pages directly for spans larger than
64 bytes; tiny edges retain the scratch path. RGBA canonicalization swaps
channels in staging pages directly. Record-v2 field production and publication
rules stay intact.

Bulk, tightly packed full tiled rows use byte dimensions with REMAP disabled.
The fast path requires at least 1 MiB, four-byte texels and width at least 256;
small images, padded rows and subrectangles retain their pixel-mode
packets. Engine ordering, PTE-selected L2 behaviour, stale-fence controls and
the three completion words are retained. A confirmed backing mapping is reused only
for the same retained allocation, avoiding a page-address vector on each pass.
A failed or uncertain mapping cannot take that path.

## Cost fields

The heap diagnostic grows from 56 to 72 words. Existing offsets are preserved;
the scorer requires the exact expanded field set. Each new field has cumulative
`_n` and `_ns` values:

| Prefix | Measured interval |
|---|---|
| `user_copy` | Upload staging allocation, pin/charge and recoverable user copy |
| `read_prepare` | Readback validation, output probes and staging allocation |
| `upload_convert` | Staging-to-backing copy/conversion and cache publication |
| `poison` | Each poison fill and cache publication |
| `transfer_map` | Native transfer preparation, mapping lookup/publication and holds |
| `ce` | Transfer send through its awaited result, including queue time |
| `compare` | First snapshot, second comparison and RGBA canonicalization |
| `copy_out` | Recoverable user pixel and record publication |

Existing `read`, `upload`, `submit` and `wait` totals remain the enclosing
syscall wall times, including validation, waits and retirement outside the
subphases. Subphases are diagnostic intervals: overlapped bands and nested
operation totals must not be summed as disjoint times. Userspace allocation
and scheduling outside syscalls remain visible in the suite's timed VALUE;
this unit does not alter that interval or infer a hardware speedup from host
operation counts.

## Sources

The file-local MIT grants were read before use:

- [NVIDIA open-gpu-doc cla0b5.h](https://raw.githubusercontent.com/NVIDIA/open-gpu-doc/9fdf5c4062007929d9f4e6cbad9c9771fe61b880/classes/dma-copy/cla0b5.h),
  Copyright (c) 2019 NVIDIA CORPORATION: CE layouts, widths, REMAP and launch fields.
- [Nouveau nouveau_boa0b5.c](https://raw.githubusercontent.com/torvalds/linux/v6.8/drivers/gpu/drm/nouveau/nouveau_boa0b5.c),
  Linux v6.8 `e8f897f4afef0031fe618a8e94127a0934896aba`, Copyright 2007
  Dave Airlied: byte-count, non-pipelined/flushed multiline copy with REMAP off.

The original full grants remain in `copy.rs`; `THIRD_PARTY_NOTICES` maps the
sources to that consumer. CPU staging and band policy are Breenix code.
Shader and firmware assets are unchanged under G5.

## Validation

The final transfer checks passed 3/3 Python checks (three executed Rust cases
plus the field check). Direct comparison and cached-mapping checks each passed
one executed Rust case. They cover a 16 MiB compare without scratch copies,
unaligned page crossings, poison-valued pixels, exact differing-byte counts,
backing identity and failed-publication retention.

The unchanged legacy copy checks passed 3/3, tiled/linear transfer checks 4/4,
observation checks 5/5, staging/quota checks 5/5 and resource checks 15/15.
The resource check initially rejected byte-mode span-image packets; restricting
the fast path to bulk images repaired the implementation. The checks were not
changed. The legacy copy source's complete API was re-exported in a temporary
standalone harness so unused public methods did not become dead-code errors;
all assertions ran unchanged. The cost recorder check and the three heap-boundary
checks passed, including exact ABI/scorer names and digest-bound transport.
Host Rust checks compiled with `-Dwarnings` under `taskpolicy -b`.

The x86 production command `cargo build --release --bin qemu-uefi` completed
with exit 0 in 1m31s and no warnings. The ARM64 production command
`cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
completed with exit 0 in 1m46s. Cargo emitted its future-incompatibility
warning for the pinned toolchain's `core` crate; no changed-code warning was
emitted. `cargo report future-incompatibilities --id 1` locates it in the
NEON `target_feature` definitions under `core_arch`, referencing Rust issue
#134375, outside this diff.
Both builds use `CARGO_BUILD_JOBS=4` under `taskpolicy -b`.

Claim checks recorded for this unit:

```
claim-lint: scripts/claim-lint.py --files tests/gt710_ce_transfers_test.py tests/gt710_cost_test.py -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-ce-transfers/check-message -> exit 0
claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files tests/gt710_ce_compare_test.py tests/gt710_ce_transfers_test.py tests/gt710_cost_test.py -> exit 0
claim-lint: scripts/claim-lint.py --files docs/gui/gt710-ce-transfers.md -> exit 1 (an unquantified phrase; made the completion count explicit)
claim-lint: scripts/claim-lint.py --files docs/gui/gt710-ce-transfers.md -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-ce-transfers/change-message -> exit 0
claim-lint: scripts/claim-lint.py -> exit 0
```

The linter skips extensionless files, including the scorer and notices; their
changed fields and source mapping were checked directly. No boot, persistent
session, PR or merge is part of this unit. The full 170-case native gate,
performance bounds, console deadline and GPU-suite timing remain acceptance
requirements.
