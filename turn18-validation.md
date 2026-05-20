# Turn 18 validation

Status: profile-only turn, no source changes intended.

Commands and checks:

- Confirmed directive state was `WAITING_CODEX` and Turn 18 requested a
  Linux-profile-only pass.
- Identified local Linux source tree:
  `/Users/wrb/fun/code/backups/transcode/home/wrb/code/linux`.
- `git -C /Users/wrb/fun/code/backups/transcode/home/wrb/code/linux describe --tags --dirty --always`
  returned `v5.9-rc8-224-g6f2f486d57c4-dirty`.
- Linux `Makefile` reports `VERSION = 5`, `PATCHLEVEL = 9`, `SUBLEVEL = 0`,
  `EXTRAVERSION = -rc8`.
- `ssh -o BatchMode=yes -o ConnectTimeout=5 linux-probe true` failed because
  the remote host key changed and strict checking rejected the connection.
  Therefore live bpftrace validation was skipped, per directive fallback.
- Breenix source reads were read-only. No kernel file was edited.

Kernel diff sanity check:

- `turn18-artifacts/kernel-diff-stat.txt` was generated with
  `git diff --stat kernel/`.
- Expected result: empty output, meaning no changes under `kernel/`.

Artifacts produced:

- `linux-profile-virtio-net-completion.md`
- `turn18-artifacts/breenix-net-polling-surface.txt`
- `turn18-artifacts/breenix-vs-linux-net-gaps.md`
- `turn18-validation.md`

Recommendation:

- Plan B: convert P6 + P7 + P8 together, because Linux's virtio-net completion
  model couples RX/TX completion through NAPI and Breenix's current polling
  surface shares one `process_rx()` drain primitive across NetRx, init polling,
  on-demand ARP, and x86 hlt-loop workarounds.
