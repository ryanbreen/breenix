# A.4 unit 12: arena call timing

This unit implements coordinator ARM ledger line 400's instrumentation for
#2915. It performs no boot and makes no acceptance or performance-fix claim.
The starting runtime is fd950046; the check-only commit is e002ae50.

`libs/libuitest/src/arena.rs` emits one record when `next_checkpoint` returns,
after the replaced arena's file and mappings have been dropped:

```text
VALUE arena-call-ns ok open=ELAPSED/CPU set_len=ELAPSED/CPU ...
```

The status is `ok` or `error`. Values are integer nanoseconds. The table
has 14 fixed entries, in this order: open, set_len, header_write,
mmap_screen, mmap_frame, mmap_tree, rename, close, munmap_header_screen,
munmap_header_frame, munmap_header_tree, munmap_payload_screen,
munmap_payload_frame and munmap_payload_tree. A call that did not happen
is `-`; an unavailable CPU clock is `ELAPSED/unsupported`.
The maximum formatted line is 809 bytes (both numbers at u64::MAX), under
the 1024-byte bound. The device write is immediate, through the existing
`/dev/screen-evidence` route, before the caller continues. An interrupted
checkpoint has no completed record. The ordinary initial create/open and
payload access paths do not activate call timing or emit these records.

The elapsed clock is `std::time::Instant`. CPU time uses the existing
CLOCK_THREAD_CPUTIME_ID (3) interface: the C wrapper at
`libs/libbreenix-libc/src/lib.rs:1589` forwards the query; ARM dispatch at
`kernel/src/arch_impl/aarch64/syscall_entry.rs:1089` delegates to the shared
handler. `kernel/src/syscall/time.rs:49` dispatches clock 3 to
`kernel/src/syscall/clocks.rs:10`, which reads the calling thread's user
and system nanoseconds. The kernel reports microsecond counter resolution
in `kernel/src/syscall/clocks.rs:35`. No kernel clock was added.
Host checks use Darwin's clock identifier 16 on that platform.

The file format, mapping flags/ranges, checkpoint count, rename and
create_new semantics stay unchanged. The replacement still closes the
old file before unmapping its three headers and populated payloads,
in the original field order. Mapping errors capture errno before clock
queries. No deadline, generation predicate or comparison changes.

## Validation

The new host check compiles the production arena with deterministic
operation clocks and real temporary files/mappings. On fd950046 it compiled
with `-Dwarnings` and failed because the record count on checkpoint
return was 0 instead of 1. After the instrumentation it passed, checking
separate elapsed/CPU intervals for the 14 calls, immediate complete output,
absent payloads, unsupported CPU clocks, preserved drop order and the
existing create_new refusal. No wall-clock threshold is used.

`tests/uitest_arena_mapping_test.py` passed. The five existing arena-only
cases in `tests/uitest_arena.rs` passed against the changed production file,
including immutable older readers and partial-publication refusal, without
changing their assertions. These were compiled in a temporary fixture that
excluded the unrelated artifact module and its artifact-only case.

The full `tests/uitest_arena_test.py` wrapper fails compilation with six
missing-dependency errors in `libs/libuitest/src/artifacts.rs` (breenix_suite,
libimg and libimg_encode). A temporary fd950046 export reproduces the same
six errors; the wrapper, fixture and artifact writer match origin/main
5ee353ad. #2918 tracks the dependency setup repair. Its artifact case is
not claimed as passing. The first temporary export omitted the SHA-256
module; adding that module corrected only the temporary export.

Scoped ARM64 and x86 `suite-ui-testing` release builds exited 0, without
warnings or errors, using jobs 4 and background policy. The Rust library
pin verified as 666252fd61839b9e9b1bc9e777a0c84c5cf3dc6d. Logs, commands,
lint exit statuses and push identity are recorded in the effort ledger.
No tests or builds are repeated after committing or pushing the same code.

#2915 remains open for the later coordinator-directed acceptance unit and
its per-call attribution. This unit does not identify the costly syscall.
