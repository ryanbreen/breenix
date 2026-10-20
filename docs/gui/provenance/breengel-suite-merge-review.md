# PR #1625 final merge review

The integration merges are `c97dfaf1` (main at `161fb6a6`) and `a7766f3d`
(main at `718994f6`). Main supplies the GPU core, console, drivers, display
handover, graphics syscalls 410–413, version 1 window cleanup and bwm input
fixes. The branch retains its window protocol and pure libraries, adapting
`libfont::glyph_segments` and libgfx's `draw_char_at` to main's callers.
Syscall 414 is published through ARM64 `dispatch_numbered` beside 415/416.
The merge takes main's Tier 1/Tier 2 files. The later integration repairs are
`fce7e5df` (main's x86 gate script), `bf6a5e1f` (pending mmap placements) and
`7bf8b1a1` (ARM64 user-pointer copies).

## Findings

- Major, `kernel/src/syscall/window.rs`: repaired in `c980ef12`. Op 14
  (`MAP_COMMITTED`) returns ENOSYS after the existing argument-size check.
  The CPU mapping implementation and its unused authority helper are removed.
  The stub names `gpu-display`; it cannot call the deleted mapper.
- Minor, `kernel/src/syscall/mod.rs` / PR description: the PR body is rewritten
  to describe the kernel and graphics-library changes and label the old
  measurements as historical. The old claim of unchanged main code is withdrawn.

## Evidence and limits

The affected code is ARM64-only. One release build completed successfully:

```sh
export CARGO_BUILD_JOBS=4
taskpolicy -b cargo build --release --target aarch64-breenix-kernel.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

Cargo reported `Finished release profile [optimized]` in 47.47 seconds.
The sole warning was the pinned toolchain's core-library future-incompatibility
notice covered by #945. The build emitted no warning attributed to this change.
Source review confirms the ENOSYS arm and deletion of the mapper. `git diff
--check` passed. No boot or guest suite ran in this round; the build and source
review do not establish final-head GPU suite acceptance. This round changes no
Tier 1/Tier 2 file and adds no workflow or test expectation change.

Claim-lint invocations (external draft names abbreviated):

- `scripts/claim-lint.py --base HEAD`: exit 0 before the code commit.
- `scripts/claim-lint.py --commit-msg <code-message>`: exit 0.
- `scripts/claim-lint.py --files <PR-body>`: exit 1 on three old absolute
  claims; exit 0 after replacing that stale text.
- `scripts/claim-lint.py --base HEAD`: exit 0 for this record.
- `scripts/claim-lint.py --commit-msg <record-message>`: exit 0.

## Historical baselines and lane ownership

`be15c333` measured the original 380 cases: ARM64 QEMU 63 passed / 317 failed,
x86-64 62 passed / 318 failed, with 0 skipped on both. The later frozen
`breengel-cpu-393` tag points to `0585917d`; `docs/gui/gpu.md` section 2.7 records
393/393 on ARM64 QEMU, Parallels and VMware, with evidence at `4e855bb6`.
These CPU-era measurements do not validate the GPU merge or this retirement fix.

The lane assignments remain those of `docs/gui/gpu.md` section 2.7:
`gpu-window` moves window buffers, replay, capture and remnants to GPU resources;
`gpu-display` registers the GPU present backend and implements composition,
readback, cursor, display handover and stage records, with contract and manifest
updates together. The existing backend-dependent paths still answer EIO or
ENOSYS; the retired MAP_COMMITTED arm specifically answers ENOSYS.
`boot_ribbon::draw` remains without a caller. `gpu-raster`, `gpu-text` and
`gpu-apps` own their respective migrations. Acceptance follows ARM64 first,
then target catch-up. Those planned migrations are not closed by this review.
