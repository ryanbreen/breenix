# F22 Phase 1 - bwm Lifecycle Trace

## Command

```sh
> /tmp/breenix-parallels-serial.log
./run.sh --parallels --test 90
cp /tmp/breenix-parallels-serial.log logs/f22-phase1/serial.log
```

The harness exited non-zero because the screenshot helper did not find the Parallels
window, but the VM booted and produced a complete serial trace. The leftover VM was
stopped with `prlctl stop breenix-1776456116 --kill`.

## Lifecycle Checkpoints

- Kernel VirGL proof draw completed:
  - `Step 6: VirGL CLEAR (cornflower blue)`
  - `Step 10: SET_SCANOUT + RESOURCE_FLUSH`
  - `VirGL 3D pipeline initialized successfully`
- Init entered userspace:
  - `[init] Breenix init starting (PID 1)`
  - `Welcome to Breenix OS`
- Expected init-script completion messages were not present in this trace:
  - no `[init] Boot script completed`
  - no `[init] bsshd started`
- Background services did start:
  - `TELNETD_STARTING`
  - `TELNETD_LISTENING`
  - `[blogd] Breenix log daemon starting`
- bwm was spawned and reached its own startup code:
  - `[bwm] Breenix Window Manager starting... (v2-chromeless-skip)`
  - `[bwm] GPU compositing mode (VirGL), display: 1280x960`
  - `[bwm] Direct compositor mapping: 1280x960 at 0x7ffffdb4e000`
  - `[bwm] display font loaded: /usr/share/fonts/DejaVuSans.ttf`
- Userspace compositor VirGL submissions occurred after kernel Step 9/10:
  - frame #0 texture update: `[virgl] SUBMIT_3D OK: id=3 ...`
  - window-composite frame 0: `[virgl] SUBMIT_3D OK: id=4 ...`
  - window-composite frame 1: `[virgl] SUBMIT_3D OK: id=5 ...`
- The soft-lockup dump confirms bwm exists:
  - `PID 5 [ready] /bin/bwm`

## Last emitted checkpoint

Last emitted bwm/compositor checkpoint before lockup:

```text
[composite-win] frame=2 bg_dirty=true windows=0 bg=1280x960
[composite-submit] frame=2 windows=0 dwords=342 vb_offset=256 draw_idx=8
[composite-win] frame=2 complete
```

There is no matching `[virgl] SUBMIT_3D OK` for frame 2. Immediately after that,
the system prints:

```text
!!! SOFT LOCKUP DETECTED !!!
No context switch for ~1 seconds (1000 ticks)
```

## Next Likely Failed Step

bwm is not missing from init and is not silent. The first missing checkpoint is the
third userspace compositor submission acknowledgement. The likely failure is in the
userspace VirGL/compositor submit or flush path after bwm has already taken over the
compositor buffer and submitted initial frames.

The process/thread dump points at a scheduler stall while the process set contains
`/bin/bwm`; this needs code inspection around `userspace/programs/src/bwm.rs` and the
kernel/userspace compositor submit path, not init.js spawn debugging.
