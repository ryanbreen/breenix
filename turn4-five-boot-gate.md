# Turn 4/5 Five-Boot Stress Gate

## Turn 4 Result

Turn 4 was blocked before the five-boot gate could run because the sibling AHCI Ralph was actively using the same Parallels `breenix-*` VM namespace. The reusable harness was committed in `37f2938d`.

## Turn 5 Retry

Status: INCONCLUSIVE.

All five coordinated Parallels boots ran for at least 220 seconds of active rendering, and none reproduced the original fatal path:

- `stuck_tid=13`: 0/5 boots
- softlock / CPU0 regression / FAR=0xccd / panic: 0/5 boots
- final FPS: min 130, max 216, mean 191.8
- completions: min 121166, max 188765, mean 146003.0
- `rescue_tid=13`: max 5, under the <= 10 gate

The strict gate still failed because every boot showed at least one `gpu_pci_lock=busy` window longer than 5 seconds in freeze-watch sampling. Completions and FPS continued to advance during those windows, so this is not evidence of the old stuck render thread wedge, but it violates the Turn 5 criterion as written.

## A. Coordination Log

The harness checked AHCI Ralph state and live Parallels VMs before each boot. It opened the gate only when AHCI was `AWAITING_REVIEW`, `STOP`, or missing, and no `breenix-*` VM was present.

| Boot | First Check | Gate Opened | Notes |
| --- | --- | --- | --- |
| 1 | 06:35:56 AHCI=`AWAITING_REVIEW`, no VMs | immediate | Started clean. |
| 2 | 06:41:04 AHCI=`WAITING_CODEX`, no VMs | 07:04:40 AHCI=`AWAITING_REVIEW`, no VMs | Waited while AHCI ran. |
| 3 | 07:10:41 AHCI=`WAITING_CODEX`, `breenix-1779188680` visible | 07:32:01 AHCI=`AWAITING_REVIEW`, no VMs | Waited through AHCI VMs and one stale self VM. |
| 4 | 07:38:06 AHCI=`STOP`, no VMs | immediate | AHCI complete. |
| 5 | 07:45:43 AHCI=`STOP`, `breenix-1779190688` visible | 07:52:00 AHCI=`STOP`, no VMs | Waited until the recorded boot-4 VM was cleaned up. |

No global `breenix-*` cleanup was used. Manual cleanup, when needed, was limited to the harness-recorded VM name for this Ralph.

## B. Harness Patch

Harness changes committed in `8531bdba`:

```diff
- delete_breenix_vms
+ wait_for_coordination_gate "$boot_num"
+ echo "$VM_NAME" >"$boot_dir/vm-name.txt"
+ prlctl stop "$VM_NAME" --kill
+ prlctl delete "$VM_NAME"
```

Other guardrails:

- Added AHCI state coordination with a 30 minute max wait.
- Logged each coordination sample to `boot-N/coordination.log`.
- Required no visible `breenix-*` VMs before launching each boot, stricter than the requested running/stopping-only check.
- Added `timeout -k 5` around Parallels and GDB helper calls.
- Narrowed future poll detection to GPU/virtio/fence polling patterns so XHCI HID startup polling is not counted as a GPU polling regression.

## C. Per-Boot Table

| Boot | Status | Reason | Max uptime ms | Last completion ms | FPS | Completes | rescue_tid=13 | Max busy ms | Raw `_poll` hits |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | fail | gpu_pci_lock_busy_gt_5s | 222571 | 220573 | 130 | 121166 | 5 | 15007 | 1 |
| 2 | fail | gpu_pci_lock_busy_gt_5s | 221409 | 220432 | 215 | 142271 | 5 | 10004 | 1 |
| 3 | fail | gpu_pci_lock_busy_gt_5s | 250434 | 250430 | 186 | 140612 | 1 | 15007 | 0 |
| 4 | fail | gpu_pci_lock_busy_gt_5s | 315454 | 315454 | 212 | 188765 | 0 | 25014 | 0 |
| 5 | fail | gpu_pci_lock_busy_gt_5s | 221423 | 220436 | 216 | 137201 | 5 | 10005 | 1 |

The raw `_poll` hits in boots 1, 2, and 5 are XHCI HID polling startup messages:

```text
[xhci] start_hid_polling: ...
[boot] USB HID input active via XHCI (polled from timer)
```

No virtio-gpu polling regression was found in the serial logs.

## D. Aggregate Metrics

```text
boot-1: fail + gpu_pci_lock_busy_gt_5s
boot-2: fail + gpu_pci_lock_busy_gt_5s
boot-3: fail + gpu_pci_lock_busy_gt_5s
boot-4: fail + gpu_pci_lock_busy_gt_5s
boot-5: fail + gpu_pci_lock_busy_gt_5s
overall: fail
fps_at_end: min=130, max=216, mean=191.8
completes_at_end: min=121166, max=188765, mean=146003.0
rescue_tid13_events: min=0, max=5, mean=3.2
```

## E. Turn 6 Scope

The next turn should decide whether the `gpu_pci_lock=busy` criterion is intentionally stricter than forward progress. If it is, diagnose and shorten the BWM/virtio-gpu lock hold windows without adding polling fallback or hot-path logging. If the criterion is meant to detect wedges only, refine the gate to distinguish a busy lock with advancing completions from a persistent GPU stall.
