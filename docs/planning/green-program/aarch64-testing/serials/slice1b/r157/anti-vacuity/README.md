# R157 anti-vacuity — the two checks this round added

Each run is `cargo test --test ttbr0_shadow_reconciliation_structure --
--nocapture`, from the repository root, at the R157 branch tree. Every file in
this directory is the run's own stdout+stderr with an `EXIT=` line appended, so
the exit status is in the artifact rather than restated about it.
claim-lint:ok: 6 of 6 files here carry an `EXIT=` line as their last line.

The two checks under test:

* `the_discipline_publishes_the_dispatch_asid` — the dispatch path's ASID tag
  (read out of `set_next_ttbr0_for_thread`'s own `tagged_ttbr0` binding) and the
  install discipline's `USER_ASID_TTBR0` must denote the same ASID; the
  normaliser must REPLACE the field rather than or into it; and the value the
  register takes must be the same normalised binding the corridor word is
  handed.
* `every_blocking_resume_restore_uses_the_guarded_helper` — the sites that
  resolve the current thread's own process root and re-install it must use
  `ttbr0::restore_process_ttbr0`, not the unconditional adopt path.

| file | mutation | exit | what it shows |
|---|---|---|---|
| `00-branch-green.txt` | unmutated control | 0 | 23 passed; the census printout names the 5 members of the blocking-resume family |
| `01-mutation-asid-normalisation-deleted.txt` | the normalising rebinding removed from `adopt_process_ttbr0` | 101 | the ASID check fails: the installed value is no longer a normalised binding |
| `02-mutation-dispatch-asid-disagrees.txt` | `context_switch.rs` tags `2u64 << 48` | 101 | the ASID check fails with `left: (2, 48)` / `right: (1, 48)` |
| `03-mutation-resume-site-unguarded.txt` | `wait.rs` calls `adopt_process_ttbr0` again | 101 | the resume census names `kernel/src/syscall/wait.rs::ensure_current_address_space` |
| `04-mutation-normaliser-or-only.txt` | `process_root_ttbr0` ors instead of masking | 101 | the ASID check fails: "must mask the ASID field before setting it" |
| `05-mutation-inline-tag-bypasses-normaliser.txt` | the discipline tags inline | 101 | the ASID check fails, printing the bypassing expression |

Run 02 is the one that carries the class claim. It is the only mutation that
perturbs a file outside the discipline module, and it is what makes "the two
return paths agree on the ASID" a checked property of the kernel rather than a
sentence in a document.

## What these runs do NOT show

* **They are structural.** Each is a source-shape check; 0 of the 6 runs boot a
  kernel, and 0 of the 6 observe TTBR0's ASID field at a return to EL0.
* **Run 03 perturbs one member of the family.** The other 4 members are held by
  the same assertion, but this directory exercises the assertion against 1 of
  them. The coverage floor inside the check (at least 5 sites, in at least 5
  distinct files) is what keeps the census from going quiet if the walk stops
  reaching them, and that floor is asserted, not sampled here.
* **No mutation targets the guard's runtime behaviour.** The skip arm of
  `restore_process_ttbr0` is argued safe in section 4b of
  `docs/planning/green-program/aarch64-testing/TTBR0-SLICE1B-2026-09-04.md` from
  the reclamation interlock; 0 runs here force a root-reuse race to test that
  argument.
