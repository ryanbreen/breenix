A rescue pass can pop a per-CPU worker from its own stalled home queue and
then count a migration refusal/hold for work that was already placed.
This change restores that existing membership before the migration guard
at the rescue census sites. Offline homes and stack-slot conflicts decline
to the existing counted guard dispositions; the guard's three aarch64
placement/hold arms get targeted reschedule kicks. Retention itself does not
kick. An SGI requests rescheduling and does not establish prompt dispatch.

The census-shaped ratchet records baseline RED, ten destructive mutation
failures (including the three kicks separately), three permitted shape
changes, and two separate forced-state host probes. These host probes do not
measure hardware dispatch or add a fourth live boot-oracle leg.

Validation is incomplete. Strict stopped before booting at 63/64 structure
suites: an additional early impl block confused the existing coreproof
scanner. The helper is now in the existing impl, its 4-test suite passes,
and the mutation matrix has been rerun. The corrected boot_tests build is
clean using the documented lane-local Rust source repair for the upstream
soft-float/NEON warning. Strict was not retried under PROOF ONCE/R52; x86
boot-tests and prod were not run. Follow-up issue 941 tracks the remaining
validation. This draft is not presented as gate-accepted.

The round document has revisions, source citations, build logs, mutation
transcripts, claim-lint records, the chips table and the not-claimed list:
`docs/planning/green-program/aarch64-testing/3F-PR2-2026-09-07.md`.
No production pin, PR 2b scan bound, daemon conversion, or issue 562 repair
is claimed here.
