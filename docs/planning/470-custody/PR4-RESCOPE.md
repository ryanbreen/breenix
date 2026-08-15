> Derived 2026-08-15 against main f611edd4; design doc recovered from transcripts.

# PR-4 re-scope on today's main (f611edd4)

**Decision artifact. No code changed. All line references are `main @ f611edd4`.**

## Bottom line

PR-4 is **still real and still worth its own PR**, but it has shrunk to one
coherent change: *make the x86 superseded-exec root retire through custody the
way the x86 **exit** root already does*. The Q4 receipt shift absorbed the
*plumbing* (who holds the old roots, when they are consumed, under what proof)
but **not the terminal action** — the 174-line descriptor walk is still the
thing that actually frees, on every x86 exec, at four call sites.

Two of the five originally-deferred items have **changed shape enough that they
should not ride in PR-4**: the x86 `UnpublishedPageTable`/failed-exec path is a
different semantic delta with a different oracle, and the `AlreadyTerminated`
leak turns out to be **arch-neutral, not x86-exec-specific**, and it rests on a
rationale that is now demonstrably obsolete.

**Recommended split**

| PR | Content | Rough size |
|----|---------|-----------|
| **PR-4** (keep) | items 1+2+3: delete the walk, route x86 old roots through `release_mapped_leaves` + `retire_bounded`, delete `RetiredByExecWalk` + `PT_EXEC_WALK_LEASES_UNRETURNED`, re-sync 4 ratchet blocks, add an **exec cohort** oracle | ~450 changed / net **−150** |
| **PR-4b** (split out) | item 4: x86 `UnpublishedPageTable` for exec + fix the early `page_table.take()` in x86 `exec_process` + x86 F4 failed-exec oracle | ~180 |
| **new issue** (split out) | item 5: `AlreadyTerminated` abandons recoverable table custody on **both** arches | ~30 prod + ~80 oracle |

---

## Per-item disposition

### Item 1 — "delete the x86 `cleanup_for_exec` descriptor walk" → **STILL LIVE, unchanged, this is PR-4's core**

The walk moved (it was ~1827–2000 in the PR-3-era spec) but is otherwise
untouched: `kernel/src/memory/process_memory.rs:2021-2194`, **174 lines**.

It is the *only* remaining production consumer of the pre-custody model. What it
does today:

- `process_memory.rs:2027` — stamps `Disposition::RetiredByExecWalk`
- `process_memory.rs:2028-2031` — counts every table lease it is about to strand
- `process_memory.rs:2049-2167` — walks L4 slots `0..256`, `frame_decref` +
  `deallocate_frame` on every `USER_ACCESSIBLE` leaf
- `process_memory.rs:2170-2185` — raw `deallocate_frame` on L1/L2/L3/L4 frames
- `process_memory.rs:2188-2193` — a `log::info!` per exec

The aarch64 sibling that PR-4 makes it match is five lines
(`process_memory.rs:2199-2203`):

```rust
pub(crate) fn cleanup_for_exec(&mut self, pid: u64, budget: &mut u32) -> RetireProgress {
    self.release_mapped_leaves();
    self.retire_bounded(pid, budget)
}
```

**Why the substitution is much less risky than when the design was written.**
Both primitives are already arch-neutral and already run on x86 in production
for the *live* root at exit — `kernel/src/task/process_task.rs:255-256`:

```rust
page_table.release_mapped_leaves();
let progress = page_table.retire_bounded(self.pid, &mut budget);
```

PR-3 shipped that with "beast custody balance = 0". Leaf custody is recorded
arch-neutrally in `map_page` (`process_memory.rs:1399-1418`, `1475-1480`), and
fork populates the child through `map_page` (`kernel/src/process/fork.rs:206`,
`:236`), so fork'd x86 children carry `LeafRecord`s too. In other words the
substitute path is already proven to release a full, fork-derived x86 address
space; PR-4 points it at a second class of root.

**The honest semantic delta (this is what C21 wants evidenced), in both directions:**

- *Custody frees what the walk leaks*: every table lease the walk strands.
  `PT_EXEC_WALK_LEASES_UNRETURNED` (`process_memory.rs:2029`) is the live
  production count of exactly this. Also: frames allocated into a partially
  built hierarchy that the walk cannot reach.
- *The walk frees what custody refuses*: any mapped leaf with no `LeafRecord`
  (custody raises `LEAF_CUSTODY_REFUSED`, `process_memory.rs:1572`) and any
  table frame not issued by `TableRecorder`. Post-PR-3 both should be
  structurally zero on x86 — `ProcessPageTable::new()` explicitly refuses to
  inherit any `USER_ACCESSIBLE` lower-half L4 entry
  (`process_memory.rs:592-601`) and allocates its own PML4[0] PDPT, claiming it
  as owned (`:611-630`); `TableRecorder` is the only allocator handed to the
  mapper (`:1449`). **This is measurable before writing a line of PR-4** — see
  the evidence plan.

**Correction to the PR-3-era framing:** the walk is *not* a ledger-corrupting
double-free today. `deallocate_frame` (`kernel/src/memory/frame_allocator.rs:1140`)
goes through `current_lease_for_frame` → `return_lease`, so it does perform the
`ST_ALLOCATED`→`ST_FREE` transition. But `current_lease_for_frame`
(`frame_allocator.rs:1114-1126`) **reads the current generation out of the
ledger** — i.e. it forges a lease. That is precisely the fail-closed
double-release/stale guard that PR-1a exists to provide, defeated on this one
path. The correct claim for the PR body is "the exec walk is generation-blind
and bypasses the custody refusal discipline", not "the exec walk double-frees".

### Item 2 — "route x86 `pending_old_page_tables` through `retire_bounded`" → **HALF ABSORBED by the Q4 receipt shift; the other half is PR-4**

Absorbed (do not re-do):

- `process_task.rs:481-489` — `defer_process_resources` now takes
  `pending_old_page_tables` into the `PendingProcessReclaim` receipt alongside
  the live root ("Carry superseded exec roots into the same proof-gated receipt
  as the current root", `:473-474`).
- `process_task.rs:244-247` — the exit-time drain is now **budgeted and
  proof-gated**, inside `reclaim_bounded`, and correctly refuses to touch the
  live root until the old ones are done (`:248-250`).

Still live (this is PR-4):

- `process_task.rs:364-385` `drain_old_page_tables_counted` — after all that
  proof plumbing, line **382** is still `old_page_table.cleanup_for_exec();`,
  i.e. the walk. It also charges one budget unit per *whole address space*
  (`:381`), so a large exec'd image is an unbounded unit of work inside a
  "bounded" drain — the aarch64 loop (`:234-243`) charges per frame. PR-4 fixes
  the budget granularity for free by adopting the aarch64 shape.
- `kernel/src/process/process.rs:587-592` — x86 `drain_old_page_tables` is
  still the **unbounded, non-proof-gated** eager walk, kept alive by four
  exec-entry callers: `manager.rs:2451`, `:2794` (x86 `exec_process` /
  `exec_process_with_argv`) and `:3077`, `:3377` (the aarch64 equivalents,
  which already go through custody via `process.rs:594-614`). All four run
  **under the PM lock** (they are `ProcessManager` methods) — the same #527
  class PR-3 flagged. PR-4 makes the x86 pair structurally identical to the
  aarch64 pair, and the two `#[cfg]` bodies in `process.rs` collapse into one.

### Item 3 — "remove `Disposition::RetiredByExecWalk` + `PT_EXEC_WALK_LEASES_UNRETURNED`" → **STILL LIVE, falls out of item 1, but drags the ratchets**

Deletion sites are small and enumerable:

- `process_memory.rs:214-215` (enum arm), `:1963-1964` (the `retire_bounded`
  short-circuit that makes the walk and custody mutually exclusive),
  `:2218-2219` (the `Drop` arm), `:2027` (the stamp)
- `kernel/src/tracing/providers/teardown.rs:474`, `:584` (counter + registry)
- `process_memory.rs:2229-2244` — `disposition_gate_counters()` shrinks
  `[u64; 10]` → `[u64; 9]`; index `5` is consumed positionally at `:2417-2419`
  and `:2437-2442`, so every index above it shifts.

**The real cost is the structural ratchet in `tests/teardown_structure.rs`**,
which currently hard-encodes the two-body split as an invariant. Four blocks
need a deliberate, negative-control-verified re-sync:

- `:1933` `PROCESS_MEMORY_FRAME_RETURNS` — `"…#[cfg(target_arch=x86_64)] fn cleanup_for_exec", 7`
  (seven `deallocate_frame` sites) → the entry disappears entirely
- `:1971` `PROCESS_PAGE_TABLE_RETIRE_SITES` — the aarch64-only
  `cleanup_for_exec` anchor becomes arch-neutral
- `:3018-3034` — the body validator that asserts `cleanup_bodies.len() == 2`
  with **exactly one** "legacy" body containing `RetiredByExecWalk` +
  `PT_EXEC_WALK_LEASES_UNRETURNED` and **exactly one** "custody" body
- `:3160-3175` — the same `cleanup_bodies.len() == 2` assertion in the negative
  control

Post-PR-4 the honest replacement is: *one* `cleanup_for_exec` body, custody-shaped,
with a negative control that FAILs if a raw `deallocate_frame` or a
`RetiredByExecWalk`-equivalent is reintroduced anywhere in `process_memory.rs`
outside `release_leaf_record`/`retire_bounded`. That is a *stronger* ratchet than
today's, and it is the argument for doing this now rather than carrying the
two-body special case forever.

### Item 4 — "x86 `UnpublishedPageTable` / failed-exec release path" → **STILL LIVE, but a DIFFERENT delta. Split into PR-4b.**

`UnpublishedPageTable` is `#[cfg(target_arch = "aarch64")]` at every one of its
six impl blocks (`process_memory.rs:230, 236, 254, 263, 270`), used only at
`manager.rs:3087` and `:3389`. x86 exec builds a raw `Box`:
`manager.rs:2483` and `:2811`.

Consequence on x86 today: any `?` between construction and publish — e.g.
`crate::elf::load_elf_into_page_table(...)?` at `manager.rs:2525`, or
`.ok_or("Failed to allocate frame for exec stack")?` at `:2557` — drops the new
`Box<ProcessPageTable>` `Undecided`. `Drop` (`process_memory.rs:2209-2211`)
only counts `PT_ROOT_DROPPED_UNDECIDED`; it cannot free. **Every failed x86 exec
leaks the entire half-built address space** (root + tables + every leaf the ELF
loader mapped).

There is a **second, x86-`exec_process`-only defect** stacked on this:
`manager.rs:2466` takes the old page table *before* the fallible ELF load and
stack mapping. `exec_process_with_argv` deliberately does not — see the comment
at `manager.rs:2900-2916` ("All fallible operations have succeeded — now it's
safe to take the old page table") and the standing warning that early-taking
"caused a use-after-free on exec failure". PR-1b's non-freeing `Drop` downgraded
that from UAF to *leak + `process.page_table == None` on a live process*, but the
asymmetry is still there and should be closed with the same change.

**Why split**: this is failed-exec/never-published release, not
superseded-exec release. Different trigger, different proof obligation (no
hardware-liveness proof needed at all — the table was never in CR3, which is
exactly the rationale in `process_memory.rs:227-229`), and a different oracle.
The oracle template already exists — the aarch64 F4 block at
`process_memory.rs:2375-2424` — but porting it needs an x86 corrupt-ELF fixture:
`corrupt_executable_fixture()` is `#[cfg(target_arch = "aarch64")]`
(`process_memory.rs:2249-2250`) and builds an `EM_AARCH64` header.

Adjacent, do **not** fold in: `create_process` (`manager.rs:147`, `:389`,
`:613`) and `fork_process` (`:2217`) also build raw `Box`es on **both** arches.
That is a genuinely wider hardening item → **#556**.

### Item 5 — "fix `release_mapped_leaves` under `AlreadyTerminated` abandon paths" → **STILL LIVE, but it is ARCH-NEUTRAL and its rationale is obsolete. File separately.**

Two production sites, neither `#[cfg]`-gated:

- `kernel/src/process/manager.rs:1127-1135` (`exit_process_locked`)
- `kernel/src/task/process_task.rs:651-660` (`handle_thread_exit`)

Both do the same three things when `already_terminated`:

```rust
if let Some(page_table) = process.page_table.take() {
    page_table.abandon(AbandonReason::AlreadyTerminated);
}
drop(process.stack.take());
process.pending_old_page_tables.clear();
```

The stated rationale is identical at both: *"Preserve the single-CoW-decref
invariant: external `terminate()` already walked these mappings, so raw-drop
them without another reclaim/decref path."*

**That rationale no longer holds, in two independent ways:**

1. `release_mapped_leaves` is already exactly-once **by construction** — it
   early-returns on `self.leaves.released` (`process_memory.rs:1559-1561`) and
   sets the flag at `:1577`. A second call after `terminate()` →
   `cleanup_cow_frames()` (`process.rs:398`, `:576-580`) is a no-op. The
   double-decref it is protecting against is structurally impossible.
2. `retire_bounded` returns **table** leases only, never leaves. `terminate()`
   never returns table leases. So routing the abandoned root through
   `retire_bounded` recovers root + intermediate tables with **zero** decref
   exposure — the two paths do not overlap at all.
3. `pending_old_page_tables.clear()` is not covered by the rationale in any
   case: `terminate()` only walks `self.page_table`. Those superseded exec roots
   are dropped `Undecided` and leak **entirely** — leaves *and* tables.

The fix shape is small: replace `abandon` + `clear()` with
`defer_process_resources(process)` and return `Some(receipt)` — the proof
pipeline already exists on this exact code path in the sibling `else` branch
(`manager.rs:1140-1142`, `process_task.rs:671-675`), and `exit_process_locked`
already returns `Option<RetirementReceipt>`.

**Why not in PR-4**: it fires on aarch64 too, it changes the abandon *safety
story* (which two reviewers signed off on in PR-3), and it needs its own
double-terminate oracle. Burying it in an "x86 exec walk" PR is exactly the kind
of scope-blend that cost the campaign rounds before.

**Cheap pre-measurement, already shipped**: `PT_ROOT_DROPPED_UNDECIDED` is
printed on every boot by `emit_root_custody_summary()`
(`teardown.rs:628-637`, the `[PT_ROOT_CUSTODY:…:undecided=N:…]` line). A nonzero
`undecided` on an x86 gate run is a live, quantified leak from item 4 and/or
item 5 combined.

---

## Remaining genuine PR-4 scope

1. Collapse `cleanup_for_exec` to one arch-neutral custody body; delete
   `process_memory.rs:2021-2194`.
2. Collapse `process.rs:587-614` to one bounded `drain_old_page_tables` +
   `drain_old_page_tables_bounded`.
3. Rewrite `process_task.rs:364-385` to the per-frame budget loop
   (`process_task.rs:234-243` is the template); keep the
   `record_masked_frames_walked` producer at `:374-376` — the leaf-timing oracle
   depends on it.
4. Delete `Disposition::RetiredByExecWalk` (4 sites) and
   `PT_EXEC_WALK_LEASES_UNRETURNED` (4 sites); shrink and re-index
   `disposition_gate_counters()`.
5. Re-sync the four `tests/teardown_structure.rs` blocks, replacing the
   two-body special case with a stronger single-body + raw-free negative control.
6. New **exec cohort** oracle (below).

Explicitly **out**: items 4 and 5 above; `create_process`/`fork` unpublished
wrapping (#556); the under-PM-lock exec drain (#527) — PR-4 makes x86 match
aarch64, it does not fix the lock discipline for either.

## Oracle / evidence plan

The retire-cohort oracle already in `teardown.rs` (per-PID exactness at
`:1544-1600`, global deltas at `:1574-1596`, refusal-counter equality at
`:1596-1607`) is the right template — but it is a **fork/exit** cohort and never
execs. PR-4's oracle is the exec variant:

- **Anti-vacuity first.** Before the change, capture on an x86 gate run:
  `PT_EXEC_WALK_LEASES_UNRETURNED` (must be **> 0**, else the walk isn't
  actually stranding anything and the PR's premise is wrong) and
  `LEAF_CUSTODY_REFUSED` / `LEAF_DECREF_UNREGISTERED` (must be **0**, else
  custody is *not* a superset and the PR would regress into a leak).
- **Exec cohort test**: N children, each exec'ing once or twice, then exiting.
  Assert per PID: `table_frames_returned == table_frames_recorded + roots`,
  `roots_retired == 1 per superseded root + 1 final`, `LEAF_MAPPINGS_RELEASED ==
  LEAF_MAPPINGS_RECORDED`, `table_frames_lost == 0`, and the full refusal-counter
  array unchanged. Global: `PT_ROOT_DROPPED_UNDECIDED` delta `== 0`,
  `PT_ROOT_DROPPED_MID_RETIRE` delta `== 0`, `PT_RETIRE_BUDGET_REQUEUED` behaves.
- **Designated mutations** (each must turn the oracle red): drop
  `release_mapped_leaves` from the new body; drop `retire_bounded`; return
  `Complete` unconditionally; strip the `Vacant`→`Owned` root-slot claim at
  `process_memory.rs:1456-1458`; strip the `USER_ACCESSIBLE` skip at
  `process_memory.rs:592-601`.
- **Ratchet negative controls**: reintroducing a raw `deallocate_frame` in
  `process_memory.rs` outside `release_leaf_record`/`retire_bounded` must FAIL
  `teardown_structure`; reintroducing a `RetiredByExecWalk`-shaped disposition
  must FAIL.
- **Free-frame accounting**: exec-heavy loop, `memory_stats().allocated_frames −
  free_list_len_for_gate()` returns to baseline — the same measurement PR-1c and
  PR-2 used.
- **Platform**: x86 full gate on beast (this is an x86-only production change);
  aarch64 clean run to prove the shared-body refactor didn't disturb the arm
  path; Parallels 3× per campaign standard. Note per MEMORY: grep
  `serial_user.log` for `FAILED` yourself — though PR #565 landed the exit-tally
  gate honesty fix, so the x86 "PASS" is now meaningfully truthful.

## Size call

- Core PR-4: **~450 lines touched, net ≈ −150** (−174 walk, −~30 dead
  disposition/counter, +~10 shared body, +~40 ratchet, +~150 oracle). Files:
  `process_memory.rs`, `process.rs`, `process_task.rs`, `teardown.rs`,
  `tests/teardown_structure.rs`. Concentrated, one concept, easy to review.
- PR-4b (item 4): ~180.
- Item 5 issue: ~30 production + ~80 oracle.

## Verdict on "own PR vs fold into #556"

**Keep PR-4 as its own PR.** It (a) is the last production path in the kernel
that frees page-table memory outside custody, (b) deletes 174 lines rather than
adding a guard, (c) removes a two-body special case that the structural ratchet
is currently forced to encode, and (d) carries a behaviour change on the exec
hot path that deserves its own bisect point and its own red/green evidence. It
is *smaller* than PR-3 by a wide margin and no longer needs to invent
machinery — it consumes machinery PR-1/2/3 already proved on x86.

**Do not** fold item 4 or item 5 into it, and **do** send the
`create_process`/`fork` unpublished-wrapping generalisation to #556.

## Open questions

1. **I could not read DESIGN-470-v2 §7 / C21 — the file is gone**
   (`/private/tmp/.../scratchpad/470design/` no longer exists; it is not in the
   repo or in git history). Everything above is derived from the code on main
   plus the PR-3-era item list in the brief. If C21 imposes an acceptance bar I
   have not reproduced — specifically if it requires a *demonstrated over-free*
   (a red proof that the walk frees a frame it does not own) rather than the
   leases-stranded/refusals-zero equality argument I propose — the evidence plan
   needs a designated fixture I have not scoped. Worth recovering the doc before
   the PR-4 brief is written.
2. **Is `PT_EXEC_WALK_LEASES_UNRETURNED` actually nonzero in production?** If it
   is zero on a real x86 gate run, PR-4 is a pure-cleanup PR with no defect
   behind it, and the framing (and possibly the priority relative to #545
   follow-ups) changes. One boot of the existing x86 gate answers this; I did not
   run it (read-only slot, and a gate run is a dispatched-agent job).
3. **Item 5's `already_terminated` branch — how often does it fire with a
   non-empty `pending_old_page_tables`?** The `[PT_ROOT_CUSTODY:…]` line gives
   the aggregate `undecided` count but does not separate item-4 (failed exec)
   from item-5 (terminated-path old roots). If the operator wants item 5
   prioritised, one throwaway counter split settles it.
