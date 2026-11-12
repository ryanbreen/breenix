# A.3 timed measurement at 530d986f

A.3 remains unproven. Coordinator ledger line 216 authorized this measurement;
its transport validity conditions pass. The selected verdict is **79/81 FAIL**.
No runtime repair, proof boot, R240 boot, board update or merge follows this unit.
The remaining mechanism is not established well enough to write a forcing check.

## Boot and validation

- Head: `530d986ff4a6774731d9b8bbf032ad4e563279ff`, clean, with current
  `origin/main` (`ac3cdf6c`) already an ancestor.
- Vigil: `20261111T221646Z-qemu-92267`, standalone window-manager, ARM64 HVF,
  headless SPICE GL through `scripts/boot-interactive.sh --mode suite --suite
  window-manager --gpu-gl --no-display`. No debugger.
- The normal userspace, disk and kernel build path passed before QEMU started.
  No compiler warning/error; the existing pinned-core future notice is G35(2).
  The FP/SIMD guard reports 0 forbidden and 32 allowlisted instructions.
- The immediate pre-exec gate checked the clean head, disk-resident suite equality,
  observation labels, trace options and load. Suite SHA-256:
  `74bb338c4763979acb29e898357ad28ccdab12c6e32c612b9eddf17209926b04`;
  kernel: `29c77dffc3133b82aa349a8adb874df680d845bf1f60112517eaab4c8ce79175`.
- No lane builds or heavy work ran while the guest ran. The 2-second host-load
  sampler is linked as `host-load.jsonl` in the Vigil record.

The raw serial SHA-256 is
`4b60fe7143b90ab947344049e7513272f71a918a8fde2d9a8830e9aa5e81ce36`.
The timestamped QEMU trace SHA-256 is
`d93f0d7f46991920039c6273ca7ff71f4c5c2a392d891832c6c71ebf515d97ec`.
The host serial-clock sidecar SHA-256 is
`0d8b4675555d26737cb8543a0f456dc4453c9537fab2c8564f6d2e486950f483`;
the load sidecar is
`48d12c909fbb3a394d3cab2bca7e60d603c6363e3c427fd1969e152b694a697f`.
Analysis intermediates and the pre-exec gate are retained locally in
`/tmp/a3-measurement-530d986f`.

Before causal analysis, the required timestamp-prefix counts were checked:

| QEMU family | Timestamped / total |
| --- | ---: |
| Read handler | 75250/75250 |
| Read/write completion | 88450/88450 |
| Request completion | 88709/88709 |
| MMIO IRQ | 177879/177879 |

Serial contains retained `VALUE wm-case-budget` rows, including both killed cases.
The existing `suite-verdict.py` API was invoked once with the categories selected
by `docs/gui/gt710-step-21-7.json`, without modifying scorer or selection. It
refuses the two recovery failures, 79/81. DONE reports passed=79 failed=2;
MEMORY reclaimed=PASS retains 10 pages against 48; standalone READY follows.
The launcher's renderer check accepts ANGLE/Metal with 998 records. The launcher
exits 1. This is a measurement, not acceptance.

Host load at the first post-gate sample was 5.51/6.17/6.97; the final sample was
10.24/8.99/7.96. One-minute load ranged 4.65–11.41. Approximate case ranges,
using guest/host clock correlation and the 2-second samples, were 10.87–11.12
for stack-restored, 10.96–11.41 for supervisor-restarts, and 10.04–10.43 for
animation latency. These correlations do not prove host starvation.

## Where the disk time goes

The retained replacement windows contain these measured read intervals. Worker
PIDs differ from their bwm leader because the clone records use child identities.
Each worker's 199 reads include configuration and metadata, not just font data.

| Window / leader / worker tid | Reads | Guest submit to used completion, ms | Host handler to request complete, ms | Gate wait, ms | Used completion to waiter resume, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| 61 / 725 / 945 | 199 | 1566.813 | 48.878 | 0.005378 | 6.688 |
| 63 / 747 / 971 | 199 | 1993.691 | 249.595 | 0.008002 | 4.294 |
| 65 / 772 / 1001 | 199 | 1879.608 | 79.604 | 0.006742 | 168.165 |

Correlation matches sector and length in request order, then request pointer to
host completion. The resulting read matches are strictly ordered: 228, 226 and
226 including leader reads. For each window, intersecting
`host_complete - guest_used <= clock_offset <= host_handle - guest_submit`
gives offset widths 8.458, 13.625 and 12.666 microseconds respectively. Each
window has its own offset; a single fixed offset across the whole boot is not
assumed. Each of the 199 worker requests has exactly one device-0 IRQ-entry row
between its submit and used-completion stamps.

For window 65, the bounded totals are 373.265–375.785 ms before the host handler,
79.604 ms in host handler-to-complete, 810.486–813.007 ms from host completion
to guest IRQ entry, and 613.733 ms inside the guest IRQ handler before the used
completion stamp. The 168.165 ms after used completion is a separate interval.
This does not support blaming disk media or the request gate for the whole delay.

Two specific requests localize the ambiguity:

| Request | Sector / bytes | Guest submit-to-used, ms | Host handler-to-complete, ms | Guest IRQ entry | Entry-to-used, ms |
| --- | --- | ---: | ---: | ---: | ---: |
| 65613 | 624524 / 1024 | 325.752416 | 0.038 | 620394148375 ns, CPU 0 | 0.091791 |
| 65620 | 460854 / 4096 | 310.346583 | 0.035 | 620731726291 ns, CPU 0 | 310.241042 |

For 65620, QEMU completes at host 1794436029.006939 and raises IRQ at
1794436029.006945. A different device's queue notification follows at
1794436029.006969; the next recorded MMIO IRQ-clear event is at
1794436029.317201, a 310.232 ms gap. Guest used completion is at
621041967333 ns. The host trace does not name that other device's type or record
its command boundaries. Treating it as graphics work is a lead, not attribution.

`kernel/src/drivers/virtio/block_mmio.rs::handle_interrupt` stamps entry before
reading interrupt status at offset 0x60, writing acknowledgment at 0x64, executing
`dsb sy`, and reading the used ring. The state lookup and physical offset access
are initialized memory reads; this handler has no blocking lock or polling loop.
The 310.241 ms is between those stamps, not waiter scheduling after completion.
The capture does not say which access/barrier was delayed or whether the host
stopped running the vCPU there. The global GPU mask maximum is 265215 us, with
an older raw-counter start of 1621735175 at timer_interrupt.rs:474; it does not
identify either request's interval. No debugger was attached.

## Recovery budgets and margins

At the first watchdog snapshot, the child budgets show:

| Case | Elapsed from logic begin, ms | Completed diagnostics, ms | Open diagnostic so far, ms | Retained completed waits, ms |
| --- | ---: | ---: | ---: | ---: |
| stack-restored | 9997.499 | 1826.029 | 0.695 | 6741.483 |
| supervisor-restarts | 9994.152 | 1637.027 | 0.096 | 7829.671 |

The 2/2 killed-child budgets retain `logic_end_ns=0`, `report_begin_ns=0` and
`report_end_ns=0`, with phase diagnostic-begin. They fail
the 10000 ms outer watchdog. Retained wait totals are not a complete partition:
nested arm/end commands overwrite the budget's current wait slot in
`kernel/src/window/stall_trace.rs::command`; named rows retain individual waits,
but the scalar does not cover every outer wait. Diagnostics and waits are parts
of logic elapsed, not additions to it.

`stop_child` dumps retained rows before sending SIGKILL. Consequently, the later
stack diagnostic total of 13592.440 ms includes execution while the parent dumps
after the deadline. It must not be attributed to the original 10-second budget.
The first snapshot above preserves the deadline observation.

Stack restoration's final client observation begins at 623815762958 ns and ends
at 623825323166 ns, passed=false, only 9.560 ms later. The source's
`until_from` also stops when `case_ms_left() < 200`; it has almost exhausted its
outer budget. This is not an independently measured two-second client timeout.
The replacement's startup phase export is cut off during diagnostics, so no
complete first-client timeline is claimed for this case.

Supervisor replacement PID 802 reaches first-composite at 2170.195 ms after
compositor-exec: **170.195 ms beyond 2000 ms**, with no positive headroom.
Font-config to font-file-ready takes 1923.132 ms. The client observation itself
lasts 2007.970 ms. Its accounting window and disk rows are absent from the export;
`Accounting::text` discovers windows from start rows in the bounded event ring,
so an evicted start cannot be reconstructed from its disk slots. The observed
ring overwrite count prevents claiming complete accounting for this replacement.

Restart-adopts reaps in 16.977 ms against 500 ms, a 483.023 ms margin (96.6%).
Its one fast observation does not resolve the older intermittent pre-exit delay.
The all-minimized case passes in 5669 ms; that is not a click-delivery margin.

## Animation and stop

Animation latency passes in 15575 ms against its 20000 ms outer deadline:
4425 ms headroom, **22.1%**, below line 167's 25% margin rule. The retained logic
interval is 15575.292 ms, including 1326.305 ms of completed diagnostics and
10553.332 ms of retained completed waits. Report serialization is 0.014459 ms.
The measured internal latency median/max are 14436/28070 us against 33000/100000 us.

The earlier 20002 ms failure at `20261111T213711Z-qemu-82836` still lacks
retained budget commands and timed host traces. Its split serial fragments do
not supply an authoritative budget. This pass does not establish why that run
missed its outer deadline; the mechanism remains unresolved.

Coordinator lines 208 and 216 require stopping when causality is not established.
No hypothesis model is presented as a forcing check, and no runtime repair is
made. The next question for one separately authorized instrumented measurement
is whether long host MMIO/graphics service encloses CPU 0's pauses, or whether
the host is not running the vCPU. Bracket status/ACK/barrier stages with numeric
stamps, correlate host device/graphics command entry and return with main/vCPU
running/waiting state, and retain accounting window boundaries independently
of event-ring churn. A distinguishing prediction belongs in the ledger before
any such boot. The existing deadlines, cases and selection remain unchanged.

This report is the unit's only repository change. The source tree's normal ARM
build/staging already passed before the measurement; documentation does not
justify another build or boot. PR #2874 remains draft and issue #2872 open.
