# E24 24.15s: failure-only prelatch observations

Coordinator ledger line 520(s) requests observations for the unresolved libc and
UI findings in [the attribution report](e24-prelatch-attribution.md). This unit
adds diagnostics; it does not attribute those failures or repair runtime behavior.

<!-- claim-lint:ok: libs/libbreenix/src/suite_control.rs::Scope::failure_records and suite_observations.rs::records gate rows on failed; tests/prelatch_observations_test.py exercises both outcomes. -->
`CASE_OBSERVATION <suite> <case> op=<name> started=<n> completed=<n>
total_ns=<n> max_ns=<n> first_ns=<n> last_start_ns=<n> last_end_ns=<n>`
is emitted by the suite runner after a failed case. Success emits none of these
records. They go through the runner's serial emitter, without a panel, artifact
or network upload. Existing UI_CONTROL and UI_STARTUP records keep their behavior.

<!-- claim-lint:ok: libs/libbreenix/src/suite_observations.rs OPS/FIELDS fix the storage; Span and stamp use atomic cells and records formats at the failure boundary. -->
The existing anonymous shared diagnostics page holds a fixed catalog of 21
operation slots (seven atomic words each), alongside its existing counters.
There is no growing event list or additional file. Repeated operations aggregate
counts, total duration and maximum duration, with first/last timestamps.
`completed` means the operation returned, including an error return; it is not a
success verdict. A killed or blocked operation can leave started above completed.
A zero timestamp means the diagnostic clock was unavailable. The records do not
change any case result, limit, watchdog or deadline.

## libc readdir-many

<!-- claim-lint:ok: test_c_guest_marks_real_readdir_phases_without_changing_result runs the production C case; userspace/libc-suite/harness.c writes result_fd and restores errno, and userspace/programs/src/suite_libc.rs forwards the stamps. -->
The C case stamps setup done, opendir reached (immediately before the call),
enumeration start and enumeration end. The helper writes four bounded `OBS`
lines to its private result pipe, preserving errno. Serial publication belongs to the runner's failed-case
path. The case-side reader copies complete stamps into the shared page while
the helper runs; repeated reads of a fragmented line do not duplicate a stamp.
The result reader consumes those transport lines before the unchanged PASS/FAIL
result. The outer runner retains the stamps when the case or helper is killed.
An unreached phase has no row. An entirely stuck case-side pipe reader cannot
forward a later helper stamp; this transport limitation remains visible as a
missing row, rather than an invented completion.

## UI operations

Configure spans cover parent preparation, fixtures, trace creation, verification
and evidence arena configuration, including setup that precedes startup.trace.
Window waits retain the whole wait and separate registry-call and composite-file
read spans, the first matching registry observation and the first accepted
composite's presentation timestamp. The existing WaitWindow predicate still
judges the same registry, composite and absolute deadline; the stamp describes the first accepted observation, rather than each later wait.

Teardown spans cover output drain, descendants, device release, display handback,
configuration-tree deletion, evidence remnants and collector reply writing.
Configuration-tree deletion is measured on both explicit and collector teardown.
These fixed slots are shared with the runner before either child forks, so they
do not depend on a surviving configuration directory, collection reply or case.

## Host validation and limits

Check commit `00f458c0` precedes the implementation. Its C probe returned PASS
without any phase stamps; its Rust probe failed because the observation API was
absent. After implementation the two checks passed in 0.415 seconds. They compile
the production readdir case/harness and shared observation code, test successful
suppression, fragmented phase transport, bounded aggregation and an unfinished
span surviving SIGKILL. After adding the POSIX feature declaration required by
Breenix's C headers, only the C probe was repeated: PASS in 0.220 seconds.
The startup-retention checks passed 2/2 in 0.644 seconds; their explicit-teardown
fixture now supplies the current init_desktop argument and production span type,
with its assertions unchanged.

Builds under taskpolicy with four Cargo jobs:

| Build | Result | Time |
|---|---|---|
| x86-64 kernel, qemu-uefi release | exit 0, no warnings | 2.03 s |
| ARM64 kernel release | exit 0, no new warnings | 0.31 s |
| ARM64 suite-libc and suite-ui-testing release | exit 0, no warnings | 29.09 s |
| x86-64 suite-libc and suite-ui-testing release | exit 0, no warnings | 26.32 s |
| Both C source objects on both architectures; ARM64 helper link | exit 0, no warnings | 2.915 s |

ARM64's kernel emits the previously accepted pinned-core future-compatibility
notice. One userspace command stopped before compilation because Cargo required
a canonical fork path; the corrected command built the same two targets.

No boot is run in this unit. ARM still lacks repaired-head breenfs-root aggregate
acceptance after q/r/s; #3030's prelatch failures and #3065's original capture
mechanism remain open. The R240 acceptance/AHCI #3061 observation and
Parallels/VMware catch-up also remain pending. These diagnostics supply evidence
for the next authorized acceptance; the host checks do not establish a guest
performance or filesystem acceptance result. No PR or merge is performed here.
