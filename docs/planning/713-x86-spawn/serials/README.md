# #713 x86 SPAWN — evidence serials

Preserved by the #713 fix-round-2 (N8), copied from the prove pass's
evidence at beast's `breenix-x86` Incus VM (`/root/p713-prove/`) and the
local Mac scratchpad (leg4/leg5), per this project's standing habit of
keeping gate serials with the campaign. `*.log` files were renamed `.txt`
(this repo's `.gitignore` has a blanket `*.log` rule); boot-firmware/disk
blobs (`.img`, `OVMF_*.fd`) were not copied — they are standard QEMU boot
inputs, not evidence.

- `leg1/` — 12 sequential `docker/qemu/run-x86-prod-profile-boot-test.sh`
  runs on beast, each `boot-N.txt` (the gate's own driver output) plus a
  `boot-N-serials/` directory (`serial_kernel.txt`, `serial_user.txt`,
  `qemu.txt`). All 12 PASS. `leg1-driver.txt` is the outer driver log for
  the whole battery; `run-leg1.sh` is the script that ran it.
- `leg2-mutation1.txt` — anti-vacuity: Tier-1 dispatch line reverted to
  `ENOSYS`. Reddened as required.
- `leg2-mutation2.txt` / `leg2-revert-confirm.txt` — anti-vacuity: the C2
  hard-error publish-failure arm forced live (`set_main_thread` skipped).
  Reddened via a distinct `ENOMEM` errno, then reverted and reconfirmed
  green.
- `leg3-boot-tests.txt` / `leg3-full-gate.txt` — the full
  `testing,external_test_bins` boot-tests battery (15/15) and the
  KVM-accelerated merge-guard gate (1/1), both on beast.
- `leg4/` — aarch64 prod-profile boot + 50-boot service-sequence gate
  (local Mac), confirming the shared-file edits (`manager.rs`/`handlers.rs`)
  did not disturb aarch64.
- `leg5/` — host structural test-suite logs (local Mac), one file per
  suite; `teardown_structure.txt`/`-retry.txt` and
  `context_restore_structure.txt`/`-retry.txt` are the pre-fix-round-2 runs
  that surfaced #727 (now closed — see `tests/teardown_structure.rs`'s
  current census, which is green at fix-round-2's head).

See `docs/planning/713-x86-spawn/` (this directory's parent) for narrative
docs, and the #713 PR/issue thread on GitHub for the review, prove, and
fix-round writeups this evidence backs.
