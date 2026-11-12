# A.3 renderer preparation change

Coordinator ARM ledger line 273(2), subject to line 243, governs this unit.
The preceding [startup analysis](a3-startup.md) recommends preparing the
renderer operations bwm uses rather than two general-purpose catalogues.
This is renderer specialization on both architectures, independent of the
host renderer or suite. The unit runs no boot and does not advance A.3 acceptance.

## Work removed

Bwm declares two preparation sets in
`userspace/programs/src/bwm/render_requirements.rs`: SOLID/COPY for scene
composition, and SOLID/COPY/NEAREST_SCALED/COVERAGE/COVERAGE_SPANS/BITMAP_TEXT
for chrome. The latter includes bitmap fallback as well as TrueType LCD
coverage, later window titles, controls and clock changes. This is eight
context/pipeline pairs instead of the previous 32 worker warm-up steps.
The extra bitmap pair relative to the analysis's seven TrueType pairs
keeps the ordinary fallback renderer prepared too.

`CONTEXT_CREATE` and `CLAIM_COMPOSITOR` accept an explicit nonempty
preparation set (bit 31 plus pipeline bits). Flags 0 retain full-catalogue
preparation. Bare low flags, including the existing GPU case's flags=1,
remain invalid. The shared ABI documents describe the extension; record
sizes, reserved fields and guest cases are unchanged.

The GPU worker selects only required bits and records readiness after those
bits complete. Native scratch is released after that set completes; the
native base SOLID completion still counts. A reserved context transfers only
to a matching preparation request. Libgfx's first-use selection refuses a
conflicting later selection rather than discarding live surfaces. Bwm keeps
its existing font coverage, target initialization, validation, ordering and
first-frame dependencies. There are no added delays or suite-specific paths.

## Deterministic result

Check-first commit `8aa43a3d` adds `tests/a3_startup_pipelines_test.py`. Its original
invocation on parent 621c8a60 compiles the production worker's eligibility,
selection and completion accounting under `taskpolicy -b`, `-Dwarnings`.
Forced device answers reproduce **32 warm-up steps**, and the assertion
requiring the renderer's eight used pairs fails. This is a deterministic
workload reproduction of the catalogue work identified in the retained
capture, not a measurement of guest milliseconds or shader execution.

After the implementation, that check passes with two scene and six chrome
steps. It also covers general-purpose preparation, invalid mask rejection,
completion-only accounting and the already-completed native SOLID step.
Production call-site checks connect the declarations to bwm's libgfx context,
claim reservation and scene context. The shared reservation check exercises
both the existing full-catalogue wait and explicit SOLID/COPY readiness.

The five scoped host checks pass: the new cold-work check,
`gpu_qemu_prepare_test`, the completion-accounting method of
`gt710_catalogue_warmup_test`, `bwm_startup_target_overlap_test` and
`bwm_startup_font_overlap_test`. Fixture changes adapt mock signatures and
fields to the explicit API without removing their assertions. The initial
font-overlap invocation found a stale fixture reference to `stage::context`;
updating that reference to `context_for` made its rerun pass. The other four
checks were not rerun after that fixture-only correction.

## Release builds

The ARM64 and x86 kernel release builds pass (2/2). Commands are the production
ARM64 target build (`-p kernel --bin kernel-aarch64`, `-Z build-std=core,alloc`
and `compiler-builtins-mem`) and the scoped x86 kernel target build
(`-p kernel --bin kernel --target x86_64-unknown-none`). Both use
`CARGO_BUILD_JOBS=4` under `taskpolicy -b`. The x86 build emits no warnings;
the ARM build emits Cargo's toolchain `core` future-incompatibility summary
for NEON ABI attributes (Cargo report id 1), with no changed-code warnings.
The report points into the installed Rust toolchain, outside this diff.

Scoped `bwm` release builds also pass on ARM64 and x86 with no warnings.
They use `userspace/programs/build.sh`'s verified Rust fork and linker flags,
with `cargo build --release --target ../../<arch>-breenix.json --bin bwm`,
`CARGO_BUILD_JOBS=4` and `taskpolicy -b`. No disk image is staged by these
builds. Logs are retained under `/tmp/a3-u29-` with suffixes
`arm-kernel-build.log`, `x86-kernel-build.log`, `aarch64-bwm-build.log` and
`x86_64-bwm-build.log`.

`python3 scripts/claim-lint.py --base 621c8a60` exits 0 for this unit's
changed hunks. `git diff --cached --check` exits 0. No boot was run.

## Handoff

Runtime acceptance remains tracked by #2872. The retained 1766 ms replacement time and the 1500 ms requirement remain the
analysis's measurements and acceptance boundary. Removing unused preparation
work does not establish a particular speedup. The acceptance unit must measure
the final head under the committed gate and coordinator line 273(3), including
the required headroom. No deadline, guest case, acceptance threshold, board
cell or PR state changes in this unit.
