# A.3 retained-capture audit and coordinator stop

This audit started from branch `arm-catchup/a-3` at `d86fd891` and fetched
`origin/main` at `ac3cdf6c`, already an ancestor of this branch. The full
catch-up plan and the coordinator ledger were read before investigating.
Coordinator line 198 remains the newest coordinator ruling in that ledger.
Its paragraph (4) requires a stop when the measurement does not establish the
mechanism. The request to continue does not lift that stop line.

The input is Vigil `20261111T202836Z-qemu-17664`, runtime `472b7fc1`.
Its raw serial SHA-256 is
`e06c55190e3f58e3c0b98c0c098d6333e3da84d2c4dac612329efdb8d0f5115a`.
`run.json` identifies a standalone ARM64 QEMU window-manager suite with
`display=false`. The cached summary is incomplete; the raw serial contains
81 CASE records and DONE `passed=78 failed=3 skipped=0 total=81`.
MEMORY reclaimed passes at 10 pages against 48, and READY follows DONE and
MEMORY. Those records do not turn the three failed cases into acceptance.

| Case | Retained measurement and limit |
| --- | --- |
| stacking/all-minimized | Positive click acknowledgment missing at 1000 ms; total case 4745 ms. Wait begins 180077523958 ns, deadline 181077523958 ns, snapshot 180578289541 ns. There is no successful acknowledgment from which to claim a passing margin. |
| recovery/stack-restored | Outer case watchdog at 10000 ms. The client-81 observer reports false over 476229745791–476236708833 ns, only 6.963042 ms. This is not a measured 2000 ms wait: `until_from` can return when the case has under 200 ms left. The capture does not attribute the preceding case budget to a causal defect. |
| recovery/supervisor-restarts | Client-83 commit-4 observer spans 488636877166–490645970791 ns: 2009.093625 ms against 2000 ms, margin -9.093625 ms. The first readback row for claimant 762 has first=serial=4620 and busy=0, matching the claim-baseline refusal in `kernel/src/syscall/window.rs`. This locates refusal, not the cause of the startup delay. |

The request arithmetic was independently recomputed from the raw capture,
rather than rerunning the scorer or the guest. Font worker 763/997 read
757076 bytes over 488646199083–490694642125 ns: 2048.443042 ms. The 189 request
rows in that interval account for 332.112746 ms enqueue-to-gate, 0.009043 ms
gate-to-submit, 1710.236915 ms submit-to-IRQ observation, 3.724877 ms
IRQ-to-resume, 0.232628 ms resume-to-release, and 2.126833 ms outside the
request lifetimes. Their byte count is 761856 including metadata.
Request 68472 alone spans 301.702916 ms from submit to IRQ observation.

The producing code leaves the decisive interval unobserved.
`kernel/src/drivers/virtio/block_mmio.rs::submit_read` stamps stage 2 before
the avail index is published and before `notify_queue`.
`handle_interrupt` reads the used ring and calls `BlockMmioCompletion::complete`,
which stamps stage 3. Thus delayed device/backend completion and delayed guest
IRQ consumption can produce the same retained timestamps. The capture directory
has no independent backend-completion trace. A forcing test of either proposed
cause would select an unmeasured hypothesis.

The raw window-67 thread rows sum to 12546.724262 ms labeled on-CPU, exceeding
four-vCPU capacity of 9182.441668 ms. `accounting_hooks.rs::thread_state` records
scheduler state changes, with a generic fallback for unnamed blocking; it does
not provide independent physical dispatch boundaries. These rows cannot decide
CPU starvation or name the leader's 1946.537790 ms kind-8/object-0 wait.
Restart-adopts' 17.561875 ms reap has 482.438125 ms margin against 500 ms in
this capture; it does not explain the older 893 ms pre-exit delay.

**Blocker:** during a long request such as 68472, did backend completion occur
promptly while guest IRQ consumption was delayed, or did backend service itself
consume the interval? One specifically instrumented measurement would need
correlated host completion/IRQ publication and guest handling timestamps, plus
physical dispatch ownership and named join/GPU wait objects. The positive-click
wait also needs its blocked compositor wait's wake condition identified; the
stack-restored case needs its consumed budget attributed before a repair.
Coordinator authorization for that observation unit is outstanding. No new
measurement or acceptance boot is authorized by this audit.

This unit changes documentation only. It adds no behavior repair or hypothesis
check, runs no build or guest boot, and does not update a board cell or merge the
PR. The existing measurement remains evidence for `472b7fc1`. A.3 remains
unproven and PR #2874 remains draft. Further work is tracked by #2872.

Validation for this audit:

- Raw-capture arithmetic and SHA-256 recomputation: completed as described above.
- `python3 scripts/claim-lint.py --files docs/planning/arm64-catchup/a3-stop-audit.md` -> exit 0.
- `python3 scripts/claim-lint.py --commit-msg /tmp/a3-stop-audit-commit.txt` -> exit 0.
- `git diff --check` -> exit 0.
