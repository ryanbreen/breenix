# A.3 landing review disposition

PR #2874 remains open after this attempt. No new boot was run. The latest
coordinator ruling read was ledger line 329; the UART blocker is an agent
report, not a coordinator hold or a new ruling.

## Pacing

`Pacer` inferred about one second from idle clock composites on displays
reporting period_ns=0. The first-input host check failed at 995.5 ms. The fix caps only
an inferred period at one 60 Hz frame; hardware-reported periods and the
fast-interval median are unchanged. 14/14 pacing checks and both scoped
bwm release builds passed without warnings. Runtime acceptance remains due.

## Scanout consumers

The completion-driven retry moves from libuitest into libbreenix so the
remaining successful-read consumers can use it. Existing deadline-aware
capture and BreenGL callers retain their deadlines. Other successful-read
callers use the device's ordinary two-second bound. Raw syscall contract
and fault checks keep their direct calls. Startup validates a retained
plain console or composed screen, propagating observation failure rather
than replacing a failed previous-context capture with the value 0.

The extended production-body check first failed because a direct scanout
consumer returned EAGAIN. It then passed busy completion, completion before
poll registration, timeout, fatal error and the existing BreenGL ratio.
The completion descriptor check also passed. ARM64 and x86 release builds of suite-window-manager, suite-ui-testing,
suite-console, suite-breengel and kiosk_test passed without warnings. Runtime acceptance remains pending.

## UART contract blocker

The new `ordered_output_recovers_from_a_cpu_that_never_returns` test fails
on the current implementation after 1000 scripted acquisition steps:
`acquire_ordered` retries forever without STALL_NS takeover or a ticket.
A proposed repair uses `acquire`'s FIFO ticket and bounded recovery. It
passes 9/9 UART-owner checks, including that reproduction.

That repair fails the unchanged `serial_normal_order_test.py` assertion.
Its fake counter forces an owner beyond the takeover budget, then requires
the wire to equal exactly the complete request followed by the exit line,
with no abandoned prefix or replay. Bounded takeover permits a waiting CPU
to write while the first CPU remains stopped; already-transmitted bytes
cannot be retracted. An indefinitely paused owner and a halted owner cannot be distinguished by this protocol.

The check was neither edited nor skipped. The proposed implementation was
withdrawn; the reproduction remains a check-first commit and fails until
the contract is resolved. The coordinator must decide the conflicting
expectation before a repair and acceptance can proceed.

## Host fork and Minors

Rebuttal of private-source visibility: coordinator ledger line 322(1)
explicitly requires the private pinned fork, and the existing fork report
records identical b4/fork/b5 source trees and authenticated rebuild inputs.
This does not close the separate correctness review of GL ownership,
fence ordering, reset and unrealize lifetime. That review remains open;
the earlier guest run cannot substitute for that review.

Remaining Minors are collected in #2904. Load-timeout continuation implements
coordinator line 182(2), while the core notice comes from the pinned
compiler. Neither gate policy was changed here. The stale memory-ownership
export description was removed as a trivial documentation correction.

## Existing proofs

The accepted runtime head was e4f26b20:

- ARM window-manager: 20261112T125550Z-qemu-30266, selected 81/81,
  memory 10/48, READY and accepted ANGLE/Metal.
- Native window-manager: 20261112T130009Z-metal, selected 81/81,
  memory 12/48 and READY.
- Native ui-testing: 20261112T131920Z-metal, 108/108,
  memory 11/48 and READY.

These precede this attempt's runtime fixes. No acceptance, merge, issue
closure or board completion is inferred for the new head.
