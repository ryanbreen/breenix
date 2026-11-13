# A.5 u6b: keep desktop clients out of machine boot checks

Issue #2925 is an init startup ownership defect. ARM's machine-check tail
launched an ambient Bounce before the GUI suite. Move that launch to normal
init startup and PID-1 desktop startup, preserving the desktop demo while
leaving service-only startup without a GUI client. This is ordinary startup
ownership: no test-mode branch, delay, yield or graphics workaround is added.

## Code and retained evidence

Read the full catch-up plan, U1/U2/U16/U19/U28/U29/U31, the debugging protocol,
ARM ledger lines 243, 257, 458 and coordinator ruling 459, and the u5 report.
Continued `arm-catchup/a-5` from `80103c11`, after u6a. A fetch found main still
at `d3041e9e`, already an ancestor; no merge or rebase was needed.

The retained u5 capture is Vigil `20261113T040422Z-qemu-87249`, serial SHA-256
`796aabcf1d56ed6bc75c14a07d9c8cea636ae9e077bcc807d381ae72af437625`.
The private capture is `/tmp/a5-u5-baseline-be737c42`. Its serial shows:

- Lines 528–547: init launches the original Bounce, PID 27, window 1.
- Line 3960, during demos/close-each: `exit(0) pid=27 name=bounce`, then init
  reaps PID 27. The close reached a working Bounce exit path.
- Line 4000: the session's newly launched Bounce, PID 805, is killed by cleanup.
- Lines 4017/4027/4029: AppExit started=1/completed=0, InputDelivery=24/24,
  and the unchanged case times out at 10000 ms.

The stopped image's `/var/log/kernel.log` also records PID 805's SIGKILL;
`/tmp/uitest` is empty. Reading these artifacts required no VM execution.

At the pre-fix head, `init.rs:257–264` calls `start_bounce()` from
`finish_boot_checks()`, and `run_desktop_suite` calls that machine-check tail.
The launch is ARM-only. `guiapps_suite/demos.rs:14–17,36` launches its own
Bounce, then closes it through the shared App API. `libuitest/src/app.rs:88–93`
retains the launched PID but resolves the window by title.
`window_wait.rs:24–26` can select the already registered ambient window before
the new child registers its own. `app.rs:179–186` consequently sends the close
to the ambient window and waits for the other PID. The two capture PIDs establish
this mismatch; completed input delivery alone did not establish correct targeting.

Bwm's path is physical Alt+Q through `bwm/route.rs::key_pressed`,
`bwm/launch.rs::execute_hotkey_action`, `bwm/input.rs::send_id`, and
`bwm/session.rs::event` (CLOSE_REQUESTED). Breengel decodes that event in
`event.rs:147`. Bounce consumes it at `bounce.rs:252` and returns at `:271`;
Particles, Confetti, Demo, Rectangles and Biconkit also return on CloseRequested.
The retained PID-27 exit supports that delivery/consumption path. No demo exit
handler, window teardown, ARM syscall or GPU driver needed a repair.

## Check first, then repair

First commit **2afdfff7** extends `tests/init_desktop_clock_test.py` to execute
the production `finish_boot_checks` body, with its ARM calls enabled on the
host, inside the existing production desktop-suite startup harness. At suite
spawn it requires zero ambient apps. Its original clock-readiness/refusal
assertions remain. On `80103c11` the new assertion fails with actual **1**,
expected **0**. Evidence: `/tmp/a5-u6b-red.log`.

The initial harness compile exposed a missing `activate_desktop_shares` stub;
a refusing stub was added before recording that behavioral red result. The
stub panics if this standalone scenario reaches sequence-only work.

The repair moves the ARM launch to the two normal startup callers. The suite
still runs the same machine checks, shares startup and clock barrier, without
launching Bounce. No suite case, manifest, runner, scorer or threshold changes.

The regression check then passed. The focused child-failure and suite-supervisor
checks passed in that same invocation. The desktop-order harness initially
failed compilation because its fake process module lacked `exit`; after adding
a refusing stub, only that check was rerun and passed. Its expectation now also
requires ARM's desktop Bounce launch before the post-startup machine checks. The
nested desktop continues to omit machine checks and the demo launch.
Evidence: `/tmp/a5-u6b-green.log` and `/tmp/a5-u6b-desktop-green.log`.

## Changed files and reach

| File | Change | x86 suite reach |
|---|---|---|
| `userspace/programs/src/init.rs` | Relocate ARM-only Bounce startup out of machine checks | ARM-only runtime change: the removed and inserted calls are cfg(aarch64); x86 execution is unchanged |
| `tests/init_desktop_clock_test.py` | Production startup regression, committed first | Host check only |
| `tests/init_desktop_boot_test.py` | Preserve desktop startup order, including ARM Bounce; complete the fake process boundary | Host check only |
| `tests/init_desktop_child_failure_test.py` | Supply the moved launch's fake boundary | Host check only |
| `tests/init_desktop_suite_test.py` | Refuse ambient launch in the suite startup fixture | Host check only |
| This report | Evidence and handoff | Documentation only |

ARM runtime reach is init's GUI suite startup and its ordinary startup callers.
Normal desktop and default-path Bounce startup remain; the GUI suite loses only
the unrelated ambient app. Kernel, console, GPU, BreenGL and shared UI production
code are unchanged. No R240 suite is reached by the runtime diff.

## Validation and handoff

The focused host checks passed: startup isolation/clock readiness, PID-1 versus
nested desktop ordering, desktop failure handling, and suite supervision.
Their Rust harness compilations use `-Dwarnings`. After the initial relocation,
the normal callers were adjusted to preserve the original Bounce-before-checks
ordering. Only the affected desktop-order check and ARM init build were repeated;
the service-only check's executed production body did not change.
Final order-check evidence: `/tmp/a5-u6b-desktop-order-green.log`.

Scoped release builds used `CARGO_BUILD_JOBS=4`, `taskpolicy -b`, the verified
Rust fork `666252fd61839b9e9b1bc9e777a0c84c5cf3dc6d`, and `--bin init` for
`aarch64-breenix.json` and `x86_64-breenix.json`. The missing local x86 libc
archive was built and cleaned through the production builder's archive step;
no libc source changed. x86 build evidence: `/tmp/a5-u6b-x86-build.log` and
`/tmp/a5-u6b-x86-libc-build.log`. Both exited 0 with zero warning/error records.
The final order adjustment moves only cfg(aarch64) calls, so x86's compiled
code is unchanged and its build was not repeated.

No boots, debugger attachments, PR creation or merge in this unit. Guest
acceptance remains for the acceptance unit's one final-head ARM gui-apps boot;
this report does not claim a new guest verdict. #2925 stays open pending that
acceptance. #2926 and #2927 remain with their subsequent units under ruling 459.

Final ARM init build exited 0 with zero warning/error records:
`/tmp/a5-u6b-arm-final-build.log`. The earlier ARM build also exited 0, before
the ordering refinement (`/tmp/a5-u6b-arm-build.log`).

Claim lint first found two unquantified table entries in this report; they were
rewritten to state the actual reach. Unit-diff claim lint and whitespace checks
then exited 0. The first check remains committed separately from the repair.
