# Rectangle command batching (24.7-batched-draws)

This unit follows the clock-record unit at `3b80aecd` on
`metal-gt710/m7-8-7`. It optimizes the native encoder for the shared
`performance/rect-throughput` stream; it does not certify a hardware time.
The full 8.7 acceptance gate and its existing pixels and bounds still apply.

The check-first commit `e4b18a5a` adds
`tests/gt710_batched_draws_test.rs`. The timed stream has 100 solid commands,
each containing 100 disjoint 16x16 rectangles at 2560x1600, using the shared
case's colour sequence. The old encoder fails its draw-count assertion:
10000 actual, 100 required. The check also requires at most 101 idle waits,
one constant block and one 600-vertex buffer per command, including original
coordinates, colours and inclusive vertex limits. It exercises compaction,
bounded request packing and relocation. Additional checks cover buffered
rectangles, clipping, graphics-to-copy ordering and retained-state parking.

The encoder now allocates one constant block and a contiguous 6xN vertex
buffer for each legacy solid DRAW or DRAW_BUFFER command. CLEAR remains a
single rectangle. Per-instance sampled and shape constants keep their
existing layout. The draw sequence uses one array count covering the buffer.
Memory evidence derives the vertex region from its actual start and inclusive
limit rather than describing only the first 256 bytes.

Ordinary draws omit the trailing WAIT_FOR_IDLE. A graphics chunk immediately
before a CE copy retains the wait, including destination snapshots used by
blended reads. The retained parking stream starts with a wait before changing
input bindings and ends with a wait before unbind/release. Thus bounded native
requests still drain their inputs before retirement, including requests split
by the existing 1400-word packing limit.

Source methods follow Mesa 24.3.4 at
`769e51468b49b2a42f0a0eaf71cf9eed5ff4e5de`:
`nvc0_vbo.c:358-376,592-597` for fetch, inclusive limit and array draws;
`nvc0_screen.c:920-939` for conditional serialization and constant binding.
Both file-local MIT grants were read before adaptation. The existing Christoph
Bumiller copyright and full grant remain in `encode.rs`, with the consumer
mapping added to `THIRD_PARTY_NOTICES`. Shader assets are unchanged (G5).

## Checks

The new batching checks passed 2/2. The unchanged draw encoder checks passed
11/11, blend encoder 4/4, sampled encoder 6/6, shapes 6/6, coverage spans 9/9
and probes 3/3: 41 Rust cases compiled with `-Dwarnings` and passed. The nine
allocation-failure host checks passed in the auxiliary Python run. The draw
and blend test files have no diff.

Four auxiliary fixture errors reproduce with the pre-change encoder at
`3b80aecd` and are tracked separately in
[#2603](https://github.com/ryanbreen/breenix/issues/2603): a missing extraction
marker in the request-packing fixture, missing `bind()` in constant retirement,
an obsolete full-catalogue size assertion in frame retirement, and missing
`present.rs` in blend scratch. Their checks were not changed in this unit.

The standalone sampled check needs `CARGO_MANIFEST_DIR` set to `libs/gpu-abi`
for its asset path; its first compile without that Cargo-provided environment
failed, then the correctly configured compile and runtime check passed.

Claim checks recorded for this unit:

```
claim-lint: scripts/claim-lint.py --files tests/gt710_batched_draws_test.rs -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-batched-draws/check-message -> exit 0
claim-lint: scripts/claim-lint.py -> exit 1 (one source comment rephrased)
claim-lint: scripts/claim-lint.py -> exit 0
claim-lint: scripts/claim-lint.py --files /tmp/gt710-batched-draws/fixture-issue.md -> exit 0
```

The linter skips extensionless files, including `THIRD_PARTY_NOTICES`; its new
source mapping was checked directly.

The x86 production `cargo build --release --bin qemu-uefi` completed with exit
0 and no warnings in 1m30s, with `CARGO_BUILD_JOBS=4` under `taskpolicy -b`.
Only native GK208 encoder code changed; no shared-core or ARM code changed,
so no ARM build was run. No boot, persistent session, PR or merge was run.

Final prose checks:

```
claim-lint: scripts/claim-lint.py --files docs/gui/gt710-batched-draws.md -> exit 0
claim-lint: scripts/claim-lint.py --commit-msg /tmp/gt710-batched-draws/change-message -> exit 0
claim-lint: scripts/claim-lint.py -> exit 0
```
