# #721 x86 exec — beast liveness-window timing evidence

`beast-liveness-probe-serial_{kernel,user}.txt` are the raw serial captures
from a manual, per-second-sampled boot of this branch's zero-feature x86_64
production kernel on beast (Incus container `breenix-x86`, `pc,accel=tcg`,
`-smp 1`), run outside the gate script to measure exactly how long each
userspace milestone takes to appear after QEMU launch.

Sampled markers and their first-observed second (t=0 is QEMU launch):

| t (s) | marker |
|------:|--------|
| 11 | `Serial command task started` (STEADY_STATE_LITERAL) |
| 14 | `[init] spawn smoke: exited (code 0)` |
| 18 | `[init] tty_oracle exited pid=...` |
| 21 | `[EXEC_SMOKE:LAUNCH]` |
| 34 | `[EXEC_SMOKE:TARGET_OK]` |
| 39 | `[init] bsshd started (PID ...)` |

`docker/qemu/run-x86-prod-profile-boot-test.sh`'s `LIVENESS_WINDOW_SECONDS`
sleeps AFTER steady state is reached and then kills QEMU and reads final
marker counts -- so the budget it needs is steady-state-to-bsshd, i.e. the
39-11=28s span above, not the raw 39s from launch. The pre-#721 value (15s)
was sized for spawn-smoke + the 13-arm tty-oracle alone; #721 inserted
exec_smoke's own work (a full exec -- new page table, ELF load, frame
allocation, argv stack setup -- immediately followed by a target that sleeps
100ms and yields 8 times) between `run_tty_oracle()` and `start_bsshd()`,
pushing that span past 15s on beast's slower TCG (it did NOT reproduce on a
faster host: the same kernel, same gate script, passed clean on the
implementer's own Mac in the same session). `run-x86-prod-profile-boot-test.sh`
was updated to `LIVENESS_WINDOW_SECONDS=60`, ~2x margin over the measured 28s.

Not a functional defect: the full exec (LAUNCH -> TARGET_ENTER argc=2 ->
TARGET_OK -> LAUNCHER_EXIT code=0) and the subsequent bsshd start all complete
correctly, just outside the previous window.
