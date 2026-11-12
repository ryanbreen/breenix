# A.3 launch identity repair

This unit implements coordinator ledger line 351 items 1-3 at baseline
`fb1293277c94d4c62c1b72c3fc474780d2a1a5ec`. It runs no guest boot.
Acceptance and PR handling remain with the acceptance unit.

## What offset 64 means

`libs/gpu-abi/src/lib.rs:759` defines offset 64 as `record::CONTEXT`;
offset 176 is `COMPOSITE_SERIAL` (line 777). These are different namespaces.

- **GK208, Plain console and Composed bwm:**
  `kernel/src/gpu/mod.rs:6203-6212` obtains native identities through
  `gk208::transfer_identities(id)`. The re-export is in
  `kernel/src/gpu/gk208/mod.rs:121`; its implementation,
  `kernel/src/gpu/gk208/ce/readback.rs:558-565`, returns the resource registry's
  shared `r.channel()` as the context. It does not return the presenting
  process's handle. `kernel/src/gpu/mod.rs:6270-6275` passes this identity to
  `observation::write_backend_fields`, which writes offset 64 unconditionally
  (`kernel/src/gpu/observation.rs:114-126`), including for Plain observations.
  `kernel/src/gpu/gk208/resource.rs:441-452` changes that channel identity only
  when the native channel incarnation changes; line 445 retains it for the
  same incarnation. Thus console and bwm observations can both carry 1.
  The public-context translation at `resource.rs:661-665` also returns this
  shared channel, rather than a unique compositor identity.

<!-- claim-lint:ok: kernel/src/gpu/mod.rs initializes the record at line 6190 and conditionally writes its writer context at lines 6261-6266. -->
- **Virtio/VirGL, Plain console and Composed bwm:**
  `kernel/src/gpu/mod.rs:6208-6210` instead uses the completed writer's
  `LastWrite.device_ctx` (or 0 with no writer). `dev::context_id` returns the
  core context ID for VirGL (`kernel/src/gpu/dev.rs:77-82`). Console writes
  record it at `kernel/src/gpu/console.rs:430-453`; composed target writes
  record it at `kernel/src/gpu/window_targets.rs:352-356`. Readback writes it
  when a writer exists (`kernel/src/gpu/mod.rs:6261-6266`); otherwise the
  initially zero record leaves offset 64 at 0 (line 6190). Core context IDs
  are allocated from `next_ctx` initialized to 1 (`mod.rs:830`), incremented
  at `mod.rs:3486-3487`. The console and a newly created bwm context therefore
  differ in this path. These are device IDs, not the public handles looked up
  by `ctx_of` (`mod.rs:1152-1154`).

The coordinator's equal-context hypothesis is confirmed for GK208. The cited
`mod.rs:6808` assignment is in `op_counters`, not the readback serializer; it
does not establish the meaning of offset 64.

## Identity and reporting

<!-- claim-lint:ok: userspace/programs/src/wm_suite/desktop.rs lines 533 and 581-585 retain baseline validation and require serial advancement. -->
The baseline read remains `Screen::read_kind(None)?`, including its observation
validation and error propagation. The acknowledgment still requires a validated
Composed observation (flags 3 via `Screen::read`, `desktop.rs:349-350` and
`libs/libbreenix/src/window.rs:94-118`) and the expected desktop pixel. It now
requires its composite serial to exceed the baseline serial.

<!-- claim-lint:ok: kernel/src/gpu/mod.rs lines 6190 and 6236-6244 leave composite serial 0 without composition; tests/fixtures/a3_launch_identity.rs checks stale serial rejection. -->
The kernel reserves that serial from the registry's presentation sequence
(`kernel/src/window/scene.rs:405-417`), stores it with the target's next
generation (`kernel/src/gpu/compose.rs:383-387`), and serializes it only with
matching composition provenance (`kernel/src/gpu/mod.rs:6141`, `6236-6244`).
Plain observations carry no composite serial and leave it zero. A repeated or
older composite therefore cannot acknowledge launch even if its context differs.

Failure text now leads with the completed context, composite serial, flags and
pixel before client provenance and the last readback error. Launch sends that
diagnostic before the startup trace so the trace cannot exhaust the budget
first. The existing 160-character chunk and 1024-byte per-case diagnostic cap
remain; the other 1024 bytes remain reserved for elapsed reports.

## Local validation

<!-- claim-lint:ok: tests/a3_launch_identity_test.py executes 2 of 2 production-body Rust checks; /tmp/a3-u40-red.log and /tmp/a3-u40-green.log record their results. -->
- Check-first commit: `7ca3c97f`. `python3 tests/a3_launch_identity_test.py`
  ran two production-body Rust checks on unchanged `fb129327`; both failed:
  equal-context acknowledgment refused, and completed observation truncated.
  After the repair both pass. The identity check also rejects stale/zero
  serials, Plain observations, wrong pixels, and failed/invalid baselines.
  The reporting check retains the completed fields in the first bounded line
  with a long prior error, and caps repeated writes at 1024 bytes.
- `python3 -m unittest discover -s tests -p wm_elapsed_test.py`: exit 0.
- `python3 -m unittest discover -s tests -p wm_pipeline_evidence_test.py`: exit 0.
- `bash scripts/rust-fork.sh verify`: exit 0, pin `666252fd`.

<!-- claim-lint:ok: 2 of 2 scoped builds exit 0; /tmp/a3-u40-aarch64-release.log and /tmp/a3-u40-x86_64-release.log contain 0 warning and 0 error lines. -->
- Scoped `suite-window-manager` release builds for `aarch64-breenix.json` and
  `x86_64-breenix.json` (`framebuffer-v1` on x86): both exit 0, zero warnings
  and errors. Both used the verified forked std, the production linker flags,
  `CARGO_BUILD_JOBS=4` and `taskpolicy -b`. No kernel code changed.
- Build logs: `/tmp/a3-u40-aarch64-release.log` and
  `/tmp/a3-u40-x86_64-release.log`; red and green check logs:
  `/tmp/a3-u40-red.log` and `/tmp/a3-u40-green.log`.
- `git diff --check`: exit 0.
- claim-lint: `python3 scripts/claim-lint.py` -> exit 0 before the check commit;
  exit 1 on the initial notes (five evidence-annotation findings) and on the
  first annotation placement (four findings); exit 0 after placing the
  resolving annotations before their paragraphs.
- claim-lint: `python3 scripts/claim-lint.py --files docs/planning/arm64-catchup/a3-u40-launch-identity.md` -> exit 1 twice as above, then exit 0 with the annotations above.
- claim-lint: `python3 scripts/claim-lint.py --commit-msg /tmp/a3-u40-check-msg.txt` -> exit 0.
- claim-lint: `python3 scripts/claim-lint.py --commit-msg /tmp/a3-u40-fix-msg.txt` -> exit 0.
