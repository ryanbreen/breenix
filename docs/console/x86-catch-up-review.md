# x86 catch-up review evidence, 2026-10-18

PR #1919 review M1 requested an x86 suite battery for the release build-override
optimization. The optimization is retained. Run
`20261018T174845Z-x86_64-gate-e62c` booted the production kernel at
`50cc365a50ce6bde9233c2c488b832b636255d7d` once and ran the following sequence.
The gate exited 0 after scoring the manifests and completed remote cleanup.
The kernel build reported 0 warnings. No quiet-host wait was requested.

| Suite | Passed | Failed | Skipped |
| --- | ---: | ---: | ---: |
| smoke | 5 | 0 | 0 |
| files-io | 219 | 0 | 0 |
| directories | 117 | 0 | 0 |
| processes | 154 | 0 | 0 |
| signals | 125 | 0 | 0 |
| time | 92 | 0 | 0 |
| memory | 120 | 0 | 0 |
| threads | 143 | 0 | 0 |
| ipc | 140 | 0 | 0 |
| terminals | 151 | 0 | 0 |
| networking | 194 | 0 | 0 |
| libc | 258 | 0 | 0 |
| rust-std | 174 | 0 | 0 |
| utilities | 194 | 0 | 0 |
| sdk | 108 | 0 | 0 |
| Total | 2194 | 0 | 0 |

The x86 console suite's existing 86/86 result at `855e4f90` is reused:
`fa83f59b` changes documentation and `50cc365a` changes host boot admission,
with no change to kernel runtime code. This report does not claim a new metal
measurement. The metal target remains a separate catch-up lane.

GDB named breakpoints resolved for `kernel::kernel_main` (ELF address `0x1edaa0`)
and `kernel::syscall::time::sys_clock_gettime` (`0x2813a0`).
`kernel::task::workqueue::worker_thread_fn` did not resolve in this production
ELF; optimized builds do not promise a breakpoint location for that name.
These are ELF addresses, before the runtime PIE relocation. The checked ELF
was byte-identical to `kernel-x86_64` extracted from the UEFI image selected
by the built launcher; its SHA-256 is
`30661cd28639968ed3862fb265f5536f1d36a007d67378794733da4b8e9afe16`.

Ordinary host boot admission now records load without starting the CPU-gap
load probe (`50cc365a`). Two focused host checks passed: start/verdict record
publication with a probe that raises if called, and explicit quiet-probe
classification. One earlier x86 preparation was cancelled before boot upon
discovering the automatic probe; it ran no suite. No suite was rerun after
pushing, and the documentation in this commit needs no kernel retest.
