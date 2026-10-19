# GPU landing validation, PR #1957

The prior validation source cut is `53f900d9`, containing main `f79e590c` and
gpu/integration `cac29713`. The review follow-up merges main `3c67ba8e` without
conflicts, then corrects comments and validation descriptions in `e85bf333`.
The PR remains draft: its strict failure is not established as independent of
the GPU changes. The main-only comparison below leaves this Major open.

## Review dispositions

1. **ARM64 scope (#1704).** The link base changes from `0x40080000` to
   `0x40200000`, the frame pool comes from DTB/UEFI conventional memory in
   `platform_config`, and the native loader supplies HardwareConfig v5. This
   affects QEMU, Parallels and VMware. QEMU's successful GL boot has 350 MiB in
   three frame ranges. Native testing boots reach their report with 2845 MiB in
   two ranges on Parallels and 1790 MiB in ten ranges on VMware. Parallels loads
   at `0x40200000`; VMware relocates that base to `0x80200000`.
2. **Shared x86 code.** 2/2 production and `boot_tests` kernel builds pass with zero
   warnings on the `0c55e0eb` source tree. This covers output accounting, log
   notices, procfs and contiguous frame allocation. The removed 16-attempt cap
   stays removed: 16 isolated short runs can precede a usable contiguous run.
   The frontier advances across a finite memory map, failed CAS operations
   observe competing progress, short claims are returned, and a ledger refusal
   fails closed. This PR adds the 16-short-run regression; its recorded run passes.
   No x86 QEMU was run (S23).
3. **Host coverage.** [The inventory](landing-1957-host.json) contains all 80
   requested targets: 70 structure files, six `gpu_*` targets,
   `frame_allocator_contiguous`, `log_notice`, `suite_manifests` and
   `rust_vendor_test`. Of these, 77 targets pass and three have inherited
   failures reproduced on main `b055f525`: #1901 (12 passed / 1 failed), #1950
   (4 passed / 2 failed), and #1775 (14 passed / 1 failed). The changed teardown
   structure target passes 108/108 after the overlapping main merges, including
   the IRQ-safe census and its planted mutations. GPU host cases pass 19/19,
   contiguous allocation 3/3, log notices 5/5 and Python vendor tests 3/3.
   Boot-contract library tests additionally pass 15/15. Initial dependency-free
   harness attempts for two dependency-bearing targets executed no tests;
   their reported runs used the declared dependencies.
4. **Suite expectation disclosures.** The PR body lists each changed case and
   its ruling: `submit/context-create` and `performance/first-draw-latency`
   (28, 33), `isolation/test-hooks-absent` (34), `readback/owner-only` (37),
   and `isolation/client-cannot-scanout` (38). No executable case expectation
   was changed by these landing repairs.
5. **Console destructor.** `bc260be2` keeps the merge-introduced
   `IrqSafeMutex`, which was in neither original parent. A displaced Console
   is moved out with `mem::replace`; the guard restores IRQ state before
   `drop(displaced)` invokes device/context cleanup. The IRQ-masked budget
   remains 500 us. No Tier-1 or Tier-2 file was edited by the landing repairs.

## Heap repair (#1914)

Native ARM64 uses a 128 MiB heap at `0x50200000..0x58200000`, followed by the
32 MiB stack pool at `0x58200000..0x5a200000`. This adds 64 MiB of headroom for
the legacy startup reservation described in #1914. Before allocator use,
`09a7d9a1` requires the final firmware conventional-memory map to cover the
combined heap/stack storage; firmware-owned storage is refused. Both native
testing boots pass that guard and complete their userspace reports.

QEMU's new GPU core uses frame backing and keeps its 64 MiB heap. A uniform
128 MiB heap first reduced its frame pool to 286 MiB and caused two real ENOMEM
case failures (168 passed / 2 failed, Vigil `20261018T194236Z-qemu`). The repair
`83a06386` returns QEMU's unused heap tail to frames. The resulting 350 MiB frame
pool passes the GL suite 170/170 without skips. The x86 heap remains 64 MiB.

## Boot evidence

[The boot inventory](landing-1957-boots.json) retains source cuts, Vigil IDs,
counts and raw serial SHA-256 digests. Native testing used the same built ARM64
userspace root image on main and the landing. These are actual testing kernels,
with 75/75 programs loaded and `USERSPACE TEST REPORT DONE` on all four boots.

| Platform | Main `4d4b7afd` | Landing `0c55e0eb` | Unchanged scorer |
| --- | --- | --- | --- |
| Parallels | `20261018T210620Z-parallels` | `20261018T210904Z-parallels` | Both exit 1: missing read-only fixture (#1982) |
| VMware | `20261018T210745Z-vmware` | `20261018T210952Z-vmware` | Both exit 1: missing read-only fixture (#1982) |

Each pair has the same 20-entry normalized nonzero child-exit set; accounting
is conserved. The raw started/exited counts differ with the workload's fork
iterations and are retained in the inventory. The no-SKIP rule is unchanged.
These are comparisons showing the landing is no worse; they are not green
native testing gates. The two baseline records defaulted to `display=false`
in recorder metadata even though `run.sh` used its Parallels window and VMware
GUI paths. Their historical metadata was preserved. Landing native records
have `display=true` registered before launch.

| GPU platform | Source cut | Passed / failed / skipped | Vigil |
| --- | --- | --- | --- |
| ARM64 QEMU GL | `09a7d9a1` | 170 / 0 / 0 | `20261018T202525Z-qemu` |
| Parallels | `0c55e0eb` | 0 / 170 / 0 | `20261018T211231Z-parallels` |
| VMware | `0c55e0eb` | 0 / 170 / 0 | `20261018T211252Z-vmware` |

QEMU uses four CPUs, 512 MiB, HVF and a visible GL window, with accepted Metal
attestation. The native suites complete, with 170/170 cases on each reporting no GPU-core
device admitted to the profile (`CAPS status 1`); #1984 tracks native catch-up.
#1956's Parallels scanout window shape remains a named catch-up issue under
ruling 53, outside this landing's required work.

Strict at `0c55e0eb` completes 121/121 kernel tests and emits `BOOT_TESTS:PASS`,
but the unchanged gate exits 1 with `migration_refused=1`; the other 5/5
pinned-placement fields are zero. Vigil: `20261018T211351Z-qemu-strict`.
The counter producer and affinity code match main. #1873 and the landing
ledger retain the blocker. Earlier actual strict failures at `65c7cc37` and
`09a7d9a1` are retained as `20261018T193929Z-qemu-strict` and
`20261018T205657Z-qemu-strict`. A failed display launcher produced no guest
serial and was separately filed as `20261018T192837Z-qemu-strict-launch-failure`.

## Build and run policy

3/3 ARM64 production, `boot_tests` and `testing` builds at `0c55e0eb` pass with only
the exempt pinned-core notice #945. The unchanged HardwareConfig v5 UEFI
loader build passes 1/1 with zero warnings. Production's SIMD guard reports zero
unexpected FP/SIMD memory instructions (32 documented allowlist entries).
Builds use four jobs and Darwin background policy. Boots use normal priority,
visible windows, one mac-boot slot, and no quiet-host wait or artificial load.
All actual boot attempts are filed in Vigil; only this session's VMs/processes
were stopped.

The last main merge, `53f900d9`, incorporates `f79e590c`'s x86 DHCP polling fix
without conflicts. Those files are outside the GPU diff; ARM64 retains its spin
hint. Unaffected runs were not repeated after that merge. The prior merge's
changed ARM64 boot-test process lifetime and receipt oracle were built and
received the strict run above. The production receipt API's new return value
is ignored by its existing consumers; the GPU behavior and suite were not
changed by that main merge.

The review scan against main `f79e590c` reports 149 claim-lint findings in
122 files. These prose findings arrive on main with this PR; they are not
inherited from main, and the branch is not reported clean. The landing repair
files, repair/merge messages and PR body passed their scoped claim-lint checks.


## Review follow-up: main strict comparison

The isolated main `f79e590c` strict baseline passes **1/1**, exit 0, with
`TESTS_COMPLETE:121/121`, `BOOT_TESTS:PASS` and 0 in all six
`PINNED_HOME_CPU_UNAVAILABLE` fields. The unchanged strict scorer ends the
boot on `scored_pass` at guest uptime 14649 ms. Vigil:
`20261018T213326Z-1957-main-strict`; serial SHA-256:
`33513edb234cc4330db6a5866f001ffe1a97fc04c80ecd4e935ecf20b3e88d48`.

This used TCG cortex-a72, four CPUs and 512 MiB, matching the retained branch
strict profile, with a visible Cocoa window, normal boot priority and the
mac-boot slot. The baseline uses the landing's built root image and simple-exit
ELF; the dependency lock differs only by the GPU ABI crate absent from main.
The initial fresh-checkout build stopped on that missing generated ELF; after
supplying it, the build passes with only the exempt pinned-core notice #945.
No guest launched during the failed build attempt. Vigil registration was
corrected to mode `tests` and display metadata was set before the VM launched.

The retained branch `0c55e0eb` strict run exits 1 with `migration_refused=1`.
The main baseline therefore does not substantiate the earlier attribution to
#1873. Matching scheduler producer/affinity code does not exclude the branch's
IRQ-mask or serial timing changes. One main pass does not establish a
deterministic regression either; the refusal's producing path remains to be
identified. **The Major is unresolved and the landing remains blocked.** No
scheduler repair, counter change or threshold change is justified by this
comparison alone. The landing ledger records the blocker.

The follow-up changes comments and evidence only. The clean main merge adds
x86-only clone reclamation already landed in #1979; it changes no ARM64 source.
Previously recorded executable suites were not rerun. Comment corrections pass
scoped claim-lint and diff whitespace checks. The broader prose, heap admission,
per-CPU storage admission, x86 output accounting, GPU exit cost, procfs task
lookup and frozen-copy enforcement Minors are tracked in #1987.
