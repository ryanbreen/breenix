# #789 — wedged-boot specimens, disassembly and the mutation probe

Evidence for `../../789-RCA-2026-09-04.md`, produced on this Mac against
`fix/562-761-aarch64-testing-profile` @ `2b3fb187`, booting the production
profile (no `--features`) on the gate's own QEMU line plus `-gdb tcp::<port>`.

## Specimens

Three boots that stopped producing output, interrupted under GDB and read out.
In 3 of 3 the `SCHEDULER` spin-lock byte is 1, 4 of 4 vCPUs are spinning on it,
and the CPU whose interrupted context holds it was interrupted inside
`kernel::task::scheduler::is_current_idle_thread`, called (x30) from
`kernel::task::completion::Completion::wait_timeout_inner + 624`.
claim-lint:ok: 3 of 3 specimens, `specimen-A/gdb-capture.txt`,
`specimen-B/gdb-stack-symbols.txt`, `specimen-C/gdb-stack-symbols.txt`.

| dir | kernel | last guest line | interrupted PC on the holding CPU |
|---|---|---|---|
| `specimen-A/` | DWARF build (same source, different layout) | `[spawn] path='/bin/tty_oracle'` @ 6689 ms | `is_current_idle_thread + 340` |
| `specimen-B/` | the gate binary | `[syscall] exit(0) pid=16 name=xhci_counters` | `is_current_idle_thread + 72` |
| `specimen-C/` | the gate binary | `[spawn] path='/bin/futex_handoff_oracle'` | `is_current_idle_thread + 248` |

* `serial.txt` — the guest serial for that boot. Specimen A's carries a
  `trace_dump_counters()` dump appended from GDB after the wedge.
* `gdb-capture.txt` (A) — four backtraces, lock byte, interrupted contexts,
  per-CPU scheduler state, ready queues, the 12-row thread table, and the
  `IDLE_SLEEP_REFUSED` / `IDLE_IDENTITY_UNREADABLE` / `PINNED_HOME_CPU_UNAVAILABLE`
  counters.
* `specimen-B/gdb-stack-symbols.txt` and `specimen-C/gdb-stack-symbols.txt` —
  per-CPU `pc`/`sp`/`cpsr` plus each stack word that resolves into kernel text,
  which is how the exception frame is located without DWARF.
* `gdb-stack-raw.txt` (B) — the raw `x/48gx $sp` dumps behind that.
* `qemu-cpu-sample.txt` (C) — `ps -o pid,%cpu,etime,comm` on the wedged QEMU:
  392.6%.
* `run-log.txt` — the runner's own verdict line for that boot.

## `is_current_idle_thread-gate.disas`

`disassemble 0xffff0000404125b0,+424` against the gate kernel. It shows the
`ldaxrb`/`stxrb` acquire at `+12`/`+20`, the `clrex`/`ret` not-acquired exit at
`+56`/`+60`, four `stlrb wzr, [x8]` releases at `+200`/`+316`/`+348`/`+372`, and
no `daifset` anywhere: the whole body between acquire and release runs with the
global scheduler lock held and interrupts enabled.

## `probe/`

Ten boots of the unmodified kernel against ten of a kernel with one scratch line
added — 3/10 versus 10/10. See `probe/README.md`.

## `tools/`

* `run_until_wedge.sh` — boot until one wedges, then leave that QEMU alive with
  its gdbstub for inspection.
* `run_n.sh` — boot N times and score each, sampling the QEMU process's `%cpu`.
* `full.gdb` — the whole-state capture (needs a DWARF build).
* `capB3.gdb` — the stack-word symbolizer (minsyms only; works on the gate
  binary).
