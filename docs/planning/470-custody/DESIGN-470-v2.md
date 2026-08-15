# DESIGN-470-v2 — allocation-derived custody with allocator-generation authority

**Issue:** #470 (F3: root/PML4 + intermediate-table leak on every process exit; F4: aarch64 exec-path leak)
**Baseline:** `main` @ `363eb912` (post-#528, post-#531/#532). Every file:line below was re-verified against this commit.
**Supersedes:** design-A-custody.md, design-B-oracle.md, design-C-minimal.md, and branch `fix/470-process-root-reclaim`.

---

## 0. Provenance: how the judges were resolved

The two judges split on the nominal winner and converged on the artifact.

| | Judge 1 | Judge 2 |
|---|---|---|
| Ranking | **A** > C > B | **B** > A > C |
| Reason for #1 | A is the only design that frees at exactly one proof-discharged site *and* fits the size law | B is the only design covering all 23 properties (generation authority, page-keyed leaves, bounded release) |
| Reason B/C rejected | B's PR-1 is the 3rd PR and closes no part of #470; est. not credible | C frees roots at 3 sites with no liveness proof — leak→UAF regression |
| Graft recommendation | Graft **B's ledger-init discipline, generation upgrade, counter-with-live-arm rule, budget/refusal separation** into A; graft **C's exec-disposition fix** | Graft **A's `retire`/`abandon`/non-freeing `Drop`, A's narrow table-only recorder as the first slice**; graft **C's committed-effect accounting and honest-residual ledger** |

Both graft lists describe the same document. This design is therefore **Design A's structure — one custody record per address space, one freeing function, one proof-discharged call site, PR-1 split at a behaviour-preserving seam — with Design B's allocator authority model (per-frame generation + state, allocation-time duplicate detection, explicitly budgeted release, virtual-page-keyed leaf specification) grafted at the allocator choke point, and Design C's exec disposition fix, committed-effect accounting, and residual-disclosure discipline grafted throughout.**

Every constraint that either judge scored **VIOLATED**, **PARTIAL**, or **UNADDRESSED** against the base design is resolved in §10; nothing is carried forward unresolved.

### 0.1 Three factual corrections to the input designs (verified on `363eb912`)

1. **Design C's x86 allocation inventory is wrong, and Judge 1's verification table repeated the error.** `process_memory.rs:801,:820,:839,:886,:905,:924` are inside a block comment opened at `:767` and closed at `:949` — dead text, not conditional kernel-stack/IST PDPT allocations. The only live direct x86 table allocation in `new()` is the fresh PML4[0] PDPT at `:601-602`. Judge 2 is correct. PR-3's x86 inventory in §8 is corrected accordingly.
2. **The `already_terminated` drop in `manager.rs` is at `:1150`, not `:1151`** (Design A cites `:1151`).
3. **Three dead direct table allocators exist and are unratcheted by all three designs:** `deep_copy_pml4_entry`/`deep_copy_l3_entry`/`deep_copy_l2_entry` allocate intermediate tables at `process_memory.rs:105`, `:170`, `:251` under `#[allow(dead_code)]` with no callers. They bypass any recorder. PR-1b **deletes** them (subtractive) and ratchet R2 keeps them gone.

---

## 1. The custody model

### 1.1 Frame classes on current main

| Class | What / where | Owner | Freed today | Freed by this design |
|---|---|---|---|---|
| **Root** | `ProcessPageTable::level_4_frame` (`process_memory.rs:78-85`), allocated `:303` (aarch64 L0) / `:380` (x86 PML4) | the `ProcessPageTable` value, exclusively | exec walk only (`:1845` x86, `:1986` aarch64); **leaked on every ordinary exit** | `retire_bounded()`, exclusively (aarch64, PR-1c) |
| **Intermediate table** | aarch64 L1/L2/L3 via `arch_stub.rs:1055-1092` `get_or_create_table_inner`; x86 PDPT/PD/PT via the `x86_64` crate mapper; plus x86's PML4[0] PDPT at `:601-602` | same value, exclusively | exec walk only (`:1832-1840`, `:1973-1981`); **leaked on every ordinary exit** | `retire_bounded()`, from the custody record |
| **Leaf (user data)** | 4 KiB/2 MiB frames at the bottom | the refcount registry (`frame_metadata.rs`); an address space holds a *reference* | `cleanup_cow_frames` (`process/process.rs:567` x86, `:606` aarch64 → `cleanup_cow_page_table:686`) on exit; the exec walk | **unchanged in PR-1**; PR-2 converts (§1.6) |
| **Kernel-shared table** | x86 master PML4[256..512] copied by `set_addr`; aarch64 TTBR1 side | the kernel | never (correct) | never — structurally: never allocated by us, never recorded |

**The load-bearing structural fact for PR-1:** on the exit path, leaves are already released before the root is dropped. `release_process_resources` (`process_task.rs:285`) runs `cleanup_cow_frames()` at `:289`, `drain_old_page_tables()` at `:290`, then `drop(process.page_table.take())` at `:291`. The deferred path does the same: `PendingProcessReclaim::reclaim` (`process_task.rs:179`) → `cleanup_cow_page_table` at `:181` → `cleanup_for_exec` for old roots at `:184` → `drop(self.page_table.take())` at `:186`. **#470-F3 therefore leaks only root + intermediate tables — the two classes with exclusive custody.** PR-1 frees zero leaves and inherits zero leaf-custody obligations.

### 1.2 Layer 1 — the frame ledger (allocator authority)

*(Design B's model, sized and initialized the way Design A's cost discipline requires. This is the layer that answers C15/C22 and Judge 2's stale-authority objection to a one-bit design.)*

The allocator already has an index space: `BootInfoFrameAllocator::get_usable_frame(n)` (`frame_allocator.rs:98`) maps a **usable-frame ordinal** to a `PhysFrame`, and `NEXT_FREE_FRAME: AtomicUsize` (`:42`) is the sequential frontier in that ordinal space. The ledger reuses it — no new address map, no sparse-region hazard.

```rust
// kernel/src/memory/frame_allocator.rs
/// One u32 per usable frame: state in bits 0..2, generation in bits 2..32.
/// Indexed by the allocator's own usable-frame ordinal.
static FRAME_LEDGER: spin::Once<&'static [AtomicU32]> = spin::Once::new();

const ST_NEVER: u32 = 0;      // above the frontier, never handed out
const ST_ALLOCATED: u32 = 1;  // outstanding
const ST_FREE: u32 = 2;       // returned, reusable

/// Unforgeable return authority. No public constructor; produced only by
/// `allocate_frame_leased()`; `Copy` so a lease can be stored in a record.
#[derive(Clone, Copy)]
pub struct FrameLease { frame: PhysFrame, index: u32, generation: u32 }

pub enum ReturnOutcome { Returned, LostContended, RefusedDoubleRelease, RefusedStale, RefusedUntracked }
```

**Operations, all O(1) except the ordinal lookup, which is O(regions ≤ 128) and typically 1–4 iterations — the same cost class the sequential allocator already pays:**

- `allocate_frame_leased() -> Option<FrameLease>`: calls the existing `allocate_frame()` (`:298`), computes the ordinal, then CAS `ST_FREE|ST_NEVER → ST_ALLOCATED` with `generation += 1`. **If the observed state is already `ST_ALLOCATED`, the allocator has handed out a live frame twice**: count `FRAME_DUPLICATE_ALLOC_REFUSED`, do **not** hand the frame to the caller (it is dropped from the free pool, not leaked into a second owner), retry once; a second failure returns `None` with a sticky `AllocatorCorrupt` reason that `map_page` surfaces as `MapError::AllocatorCorrupt`, never as OOM. **This is C22, caught at allocation, before the mapper can zero a frame that is a live table elsewhere** — the defect Judge 2 found in Design A's record-and-proceed answer.
- `return_lease(lease) -> ReturnOutcome`: one indexed load; requires `ST_ALLOCATED` **and** `generation == lease.generation`; transitions to `ST_FREE`, then pushes onto `FREE_FRAMES` under the existing `try_lock` (`:338`). Refusals: `ST_FREE` → `RefusedDoubleRelease`; generation mismatch → `RefusedStale` (stale authority after reuse — what a one-bit design cannot catch); no ordinal → `RefusedUntracked`. **A refusal leaks one frame; it never corrupts.** `return_lease` contains no log/format/heap (ratchet R7).
- `deallocate_frame(frame)` (existing signature, `:328`, all other callers) keeps its behaviour and its pre-existing logging, but routes its push through the same ledger transition using the ledger's own generation. It therefore gains the double-release guard for every caller on both arches, without gaining staleness detection (it has no lease). **One choke point, two wrappers** — ratchet R1 proves that every `FREE_FRAMES … push` in the tree lies inside `return_lease`'s span.

**Initialization (this is where Design A was not implementable).** aarch64 calls `frame_allocator::init_aarch64` at `main_aarch64.rs:491` **before** `init_aarch64_heap()` at `:492`, so a heap-backed table cannot be sized in `init`. The ledger is built by an explicit `init_frame_ledger()` called **immediately after heap init**, at a quiescent single-threaded boot point:

- aarch64: `main_aarch64.rs:492`, on the line after `init_aarch64_heap()`.
- x86: `kernel/src/memory/mod.rs`, after `heap::init` (`:130` / `:135`) and before slab init (`:138`).

Seeding is exact, not heuristic, because the allocator's whole state is readable at that instant: ordinals `< NEXT_FREE_FRAME` → `ST_ALLOCATED(gen 1)`; every frame currently in `FREE_FRAMES` → `ST_FREE(gen 1)`; ordinals `≥ NEXT_FREE_FRAME` → `ST_NEVER`. Pre-ledger bootstrap frames are therefore correctly `ST_ALLOCATED` and a double release of one is caught. Ratchet R9 pins the two init call sites and proves every `ProcessPageTable` constructor is downstream of them.

**Cost:** 4 bytes per usable frame = **512 KiB for 512 MiB of RAM (0.1%)**, one heap allocation at boot. (Design B's ≥24 B/frame variant — Judge 1's B-2 objection — is not adopted: class/refcount/owner stay out of the ledger; refcounts remain in `frame_metadata.rs`.)

### 1.3 Layer 2 — the address-space custody record

*(Design A's core, unchanged in shape; the record now holds leases instead of raw frames.)*

```rust
// kernel/src/memory/process_memory.rs
pub struct ProcessPageTable {
    level_4_frame: PhysFrame,          // unchanged; still the hardware-facing field
    mapper: OffsetPageTable<'static>,
    root_lease: FrameLease,            // NEW — return authority for level_4_frame
    tables: OwnedTableFrames,          // NEW
}

struct OwnedTableFrames {
    leases: Vec<FrameLease>,
    disposition: Disposition,
}

/// The ONLY `FrameAllocator` a process mapper is ever given.
struct TableRecorder<'a>(&'a mut OwnedTableFrames);

unsafe impl FrameAllocator<Size4KiB> for TableRecorder<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let lease = crate::memory::frame_allocator::allocate_frame_leased()?;
        self.0.leases.push(lease);          // record, no dedup, no policy, no failure mode
        Some(lease.frame)
    }
}
```

**Why the record is complete, on both arches, by construction:**

- aarch64: `get_or_create_table_inner` (`arch_stub.rs:1055-1092`) is the only creator of an aarch64 intermediate table and takes its frame from the caller-supplied allocator at `:1068`; it is reached only from `map_to`/`map_to_with_table_flags`.
- x86: the `x86_64` crate's `map_to_with_table_flags` uses the supplied allocator identically.
- Exactly **two** live sites hand an allocator to a process mapper: `map_page` (`process_memory.rs:1251-1257`, the `&mut GlobalFrameAllocator` at `:1256`) and `update_page_flags` (`:1360-1367`, at `:1366`). Both become `&mut TableRecorder(tables)` via `let Self { mapper, tables, .. } = self;`.
- The two escape hatches — `ProcessPageTable::mapper()` (`:1565`) and `::allocate_stack()` (`:1571`) — are `#[allow(dead_code)]` with zero callers (`grep -rn "\.mapper()" kernel/src` finds only `paging::get_mapper()` on the kernel table). **PR-1b deletes both.** The three dead `deep_copy_*` helpers (`:105`, `:170`, `:251`) are deleted with them (§0.1).
- Roots allocated outside the mapper are leased at their own sites: `:303` (aarch64 L0), `:380` (x86 PML4) → `root_lease`; x86's conditional PML4[0] PDPT (`:601-602`) → `tables.leases.push(...)` on the following line. **This is C2 fixed at the source: a conditional allocation produces a conditional record automatically, because the record is written where the allocation happens.** No second place has to know a count.

**Why the record never goes stale:** no `unmap` path frees an intermediate table — aarch64 `arch_stub.rs:1210-1261` only clears the L3 entry and returns the leaf; `unmap_user_pages` (`:1644`) and `clear_user_entries` (`:1614`) unlink without freeing. A recorded lease stays a live table of this hierarchy from record to retire. **No purge path is needed, which is why C16 is structurally vacuous here: there is no key.** The record is a field, moved with the value into `PendingProcessReclaim.page_table` (`process_task.rs:71`) and `pending_old_page_tables` (`process/process.rs:275`).

### 1.4 Layer 3 — dispositions: `retire_bounded` / `abandon` / non-freeing `Drop`

```rust
enum Disposition {
    Undecided,
    Retiring,                       // budget exhausted mid-retirement; requeued
    Retired,                        // all leases returned or refused; root returned
    RetiredByExecWalk,              // consumed by the pre-existing cleanup_for_exec walk
    Abandoned(AbandonReason),
}

enum AbandonReason {
    NoProofPipeline,     // aarch64 non-deferred exit — PR-2 target
    NoArchPipeline,      // x86 — no liveness proof exists yet — PR-3 target
    AlreadyTerminated,   // terminate() already walked this hierarchy
}

pub(crate) enum RetireProgress { Complete, Budgeted }

const RETIRE_FRAME_BUDGET: u32 = 64;

impl ProcessPageTable {
    /// The ONLY function that returns a process table frame.
    /// Resumable: pops leases until the budget is spent; the root goes LAST.
    pub(crate) fn retire_bounded(&mut self, pid: u64, budget: &mut u32) -> RetireProgress {
        self.tables.disposition = Disposition::Retiring;
        while *budget > 0 {
            *budget -= 1;
            match self.tables.leases.pop() {
                Some(lease) => self.account(pid, return_lease(lease)),
                None => {
                    self.account(pid, return_lease(self.root_lease));
                    self.tables.disposition = Disposition::Retired;
                    return RetireProgress::Complete;
                }
            }
        }
        RetireProgress::Budgeted
    }

    /// Explicit, counted leak. Byte-for-byte main's runtime behaviour plus one counter.
    pub(crate) fn abandon(mut self, reason: AbandonReason) { /* set disposition + trace_count! */ }
}

impl Drop for ProcessPageTable {
    fn drop(&mut self) {
        match self.tables.disposition {
            Disposition::Undecided => trace_count!(PT_ROOT_DROPPED_UNDECIDED),   // CI-asserted 0
            Disposition::Retiring  => trace_count!(PT_ROOT_DROPPED_MID_RETIRE),  // CI-asserted 0
            _ => {}
        }
        // NEVER frees. A drop that reaches here leaks exactly as main does today.
    }
}
```

Four properties, each load-bearing:

1. **One freeing function, resumable, root last.** `pop()` (not `drain`) makes partial progress durable with no iterator state to persist. The root is returned only when the table list is empty, so a requeued receipt still owns a root frame that is still `ST_ALLOCATED` — the liveness question its re-proof asks is unchanged.
2. **`Drop` refuses to free.** The failure mode of a forgotten disposition is the leak main already has, never a free. Because it cannot free, `Drop` needs no liveness proof, and "the backstop cannot free" is a checkable span assertion (R4).
3. **`abandon` makes the residual visible in production.** Each reason has its own unconditional counter (§4). The counters are the encoded to-do list for PR-2/PR-3.
4. **`RetiredByExecWalk` closes the exec false-positive.** Both `cleanup_for_exec` bodies (`process_memory.rs:1689` x86, `:1861` aarch64) consume `self` and free the root by walking; PR-1b sets `self.tables.disposition = Disposition::RetiredByExecWalk` in each body before the implicit drop. **Without this, `PT_ROOT_DROPPED_UNDECIDED == 0` false-fires on every exec-superseded root on any real boot** (`bsh` execs constantly) — Judge 1's A-1 and Judge 2's finding 5, grafted from Design C. Their `owned_tables` leases are dropped unreturned, which is the pre-existing exec leak, now *measured* by `PT_EXEC_WALK_LEASES_UNRETURNED` and closed by PR-2.

### 1.5 Custody transfer, end to end

**Creation** → `new()` leases the root (`:303`/`:380`) and, on x86, the PML4[0] PDPT (`:601-602`); `disposition = Undecided`.
**Growth** → every `map_page` (`:1189`) / `update_page_flags` (`:1324`) that needs a table node takes it from `TableRecorder`.
**Handoff** → `Process.page_table: Option<Box<ProcessPageTable>>` (`process/process.rs:224`); the record rides inside the `Box`.
**Exec supersede** → `manager.rs:2489`, `:2939`, `:3210`, `:3380` `take()` the outgoing root into `pending_old_page_tables` (`process/process.rs:275`); the new root starts an empty record. PR-1 changes nothing here except the disposition set inside `cleanup_for_exec`.
**Fork** → the child's `ProcessPageTable::new()` is built outside PM (`syscall_entry.rs:936`) and `setup_cow_pages_with_vmas` maps parent frames via `child_page_table.map_page`, increfing each leaf (`fork.rs:213`, `:244`, `:254`). **Table custody is never inherited or shared** — which is why the branch's +68 lines in `fork.rs` are dropped entirely.
**Exit** — three dispositions on main, and what each becomes:

| Site (verified) | main today | this design | Liveness evidence |
|---|---|---|---|
| `process_task.rs:186` — `PendingProcessReclaim::reclaim`, downstream of the full `RootProof` | `drop(self.page_table.take())` → leak | **`retire_bounded()`** | epoch fence + local TTBR0 + every captured CPU's saved/next shadow (`process_task.rs:118-177`) + scheduler cached roots (`:911`) + live/creating rows (`:917`), all discharged at `:905-935` with **no lock held** |
| `process_task.rs:291` — `release_process_resources` | `drop(process.page_table.take())` → leak | `abandon(NoProofPipeline)` aarch64 / `abandon(NoArchPipeline)` x86 | only the online-shadow mask was consulted (`:297-318`) — **not** a proof. `manager.rs:1152-1156` states the repo's own reason: fault exits always defer because grace covers hardware TTBR0 lag |
| `process_task.rs:471`, `manager.rs:1150` — `already_terminated` | `drop(process.page_table.take())` → leak | `abandon(AlreadyTerminated)` | **none of any kind** |

**Only the first site frees.** Design C's decision to retire at all four is the leak→use-after-free regression both judges flagged (Judge 1 C-1, Judge 2 §"L2/L3/L4 are memory-unsafe"); it is rejected here, and ratchet R3 pins the single `retire_bounded` call site so it cannot creep.

### 1.6 Leaf custody — specified now, implemented in PR-2

C14 is the finding that forced a design change, and C17 requires the key to distinguish *mappings*, not frames. The specification (Design B's model, motivated by Design A's live-defect finding):

**The defect that proves the model is needed.** `frame_decref`'s untracked arm (`frame_metadata.rs:~100-115`) ends `true` — "not tracked ⇒ private ⇒ safe to free". Two live paths hand it kernel-owned frames: `map_user_stack_to_process` (`process_memory.rs:2152`) maps frames translated out of the kernel table, and `map_user_stack_to_process_with_phys` (`:2318`, called from `manager.rs:516` and `:693`) maps caller-supplied kernel-allocated stack physical addresses `USER_ACCESSIBLE`. `cleanup_cow_frames` walks user pages, finds them untracked, gets `true`, and frees a kernel stack frame. **This is a live over-free on main, not a hypothetical.**

**The model (PR-2):**
- Ownership is derived from **allocator state**, never from a caller-supplied enum: after `map_to` succeeds, the page-table code classifies the frame from the ledger — `ST_ALLOCATED` + unassigned → this mapping takes the first reference; `ST_ALLOCATED` + already assigned → validated shared/alias reference (fork, CoW, two VAs); registered external span → `External`, never decref'd; free/stale/out-of-region → mapping fails with a precise refusal counter. A caller cannot construct the authority type.
- Custody is keyed by **virtual page**, in a sorted `Vec<LeafRecord { page: VirtPage, mapping: LeafMapping }>` local to the `ProcessPageTable`. **One frame mapped at two VAs yields two records and two balanced references** — C17, which a frame-refcount-only model cannot express (Judge 2's VIOLATED on Design A). `unmap_page(page)` consumes the exact record; the physical frame returns only at refcount zero.
- **Ordering is reserve → publish descriptor → commit** (Judge 2's B-1): the record is reserved before the descriptor becomes visible and rolled back on map failure, so preemption can never expose a mapping with no custody.
- `frame_decref`'s untracked arm flips to **fail-closed** (refuse, count `LEAF_DECREF_UNREGISTERED`, leak) only *after* the introduction sites are converted: `interrupts.rs:801`, `:915`, `:1037`, `exception.rs:2150` (`frame_register`), `fork.rs:213`/`:244`/`:254` (`frame_incref`), `process_memory.rs:2271`, and the two borrowed-stack sites `:2152`/`:2318`.

**PR-1 implements none of this and frees zero leaves**, so no leaf-ownership decision exists in PR-1 to get wrong — a claim made checkable by R1, not asserted.

### 1.7 Invariants

| # | Invariant | Why it holds |
|---|---|---|
| I1 | A lease in `tables.leases` was issued by the allocator to *this* value's mapper | `push` occurs only inside `TableRecorder::allocate_frame`; R2 proves `TableRecorder` is the only allocator a process mapper receives |
| I2 | A frame is leased to at most one owner at a time | ledger CAS `Free|Never → Allocated`; a violation is the duplicate case, refused at allocation (§1.2) |
| I3 | A recorded table stays a live table of this hierarchy until retirement | no unmap path frees a table (`arch_stub.rs:1210-1261`); nothing else writes `tables` |
| I4 | Each lease is returned at most once, and the root exactly once and last | `pop()` removes on use; the root is returned only on the empty-list branch; `Disposition::Retired` is terminal |
| I5 | A stale or duplicate return is refused, not executed | ledger generation + state check in `return_lease` (both arches, all callers) |
| I6 | A duplicate allocator return never reaches a mapper | detected at allocation; the frame is withheld and `MapError::AllocatorCorrupt` is returned (never OOM) |
| I7 | A root that is not provably dead is never freed | `retire_bounded` is called from exactly one site downstream of the merged `RootProof`; R3 freezes it |
| I8 | A root that is never retired leaks and is counted | `Drop` counts `Undecided`/`Retiring`; `abandon` counts by reason; the exec walk counts unreturned leases |
| I9 | No accounting result can suppress a correct free | refusals are per-lease; there is no gate, no balance precondition, and the root is attempted regardless of a refused table (C2) |
| I10 | Partial retirement is resumable and re-proved | budget exhaustion requeues the receipt intact; the next pass re-runs the full proof before continuing |

---

## 2. Drain-path cost budget

Drain entry points: `context_switch.rs:4454` (top of `schedule_from_kernel`, before `disable_interrupts()`) and `syscall_entry.rs:932`. Both are normal context, interrupts enabled, no PM/SCHEDULER held.

| Resource | On this path **today** | **Added** by PR-1c | Note |
|---|---|---|---|
| Logging / formatting | `log::trace!` (`frame_allocator.rs:339`) and `log::warn!` (`:331`, `:348`) per leaf free, via `cleanup_cow_page_table` (`process/process.rs:698`) and `cleanup_for_exec` (`process_task.rs:184`); `log::info!` in the aarch64 exec walk | **0** | table returns go through `return_lease`, a **log-free** primitive; R7 asserts no `log::`/`format!` in its span or in `retire_bounded`. This is the honest version of Design A's "zero added logging" (Judge 1's C3 PARTIAL) — made literally true rather than argued |
| Heap allocation | 3 `Vec::new()` per old root in `cleanup_for_exec` (`:1872-1874`) | **0** | `tables.leases` grows at `map_page` time |
| Heap deallocation | `Box<ProcessPageTable>` + those `Vec`s | **+1** (the leases buffer, freed in the same drop sequence as the `Box` that already frees there) | disclosed, not hidden |
| Stack | `cleanup_for_exec` locals | **+0 bytes** | no fixed buffer anywhere (C12) |
| Descriptor reads | up to 512·(1+512+512²) per old root in the exec walk | **0** | there is no walk |
| Frame returns | leaves only | **≤ 64 per drain selection** (`RETIRE_FRAME_BUDGET`), T+1 total across passes | explicit budget (C11) |
| Per-frame validation | none | **O(1)** indexed load + compare; ordinal lookup O(regions ≤ 128), typically 1–4 | replaces the branch's O(T²·512) |
| Locks | `FREE_FRAMES.try_lock` per free | +≤64 `try_lock`s per selection, same primitive | no new lock, no blocking acquire |

**Budget arithmetic.** A typical `bsh` address space allocates 3–9 tables; the budget never engages. A process mapping 1 GiB of 4 KiB pages allocates ~514 tables — that is the case the budget exists for: ≤64 returns per selection at roughly one indexed atomic + one `try_lock` + one push each, then requeue. Requeue reuses the existing refusal arm verbatim (`PENDING_PROCESS_RECLAIMS.lock().push(reclaim)` at `process_task.rs:929-931`) and the existing `last_pass` bounded-pass rule prevents re-selection within the same pass, so no livelock shape is added.

**One accounting correction (Design C's graft, Judge 2's critique of Design A):** `record_table_frames_reclaimed(count + 1)` counted *attempts*. Here `PT_TABLE_FRAMES_RETURNED` increments only on `ReturnOutcome::Returned`; `PT_RETIRE_FRAMES_LOST` increments on `LostContended`; refusals go to their own ledger counters. The per-PID equality (§3.2) is therefore an equality over **committed effects**, and it cannot pass while frames were silently not returned.

**Pre-existing residual, named:** the leaf walk and the exec walk already on this path remain unbounded in PR-1. PR-1 does not worsen them; PR-2 deletes the aarch64 exec walk (removing 3 `Vec`s and a `log::info!` from the drain) and PR-3/PR-4 the x86 one.

### 2.1 #527 non-widening proof

#527 is the latent PM→SCHEDULER inversion (`manager.rs:3281`, `:3573` — `with_thread_mut` under PM; hierarchy at `scheduler.rs:13-14`), latent only because the reverse edge is `try_lock`-only. This design: touches neither exec entry point (`syscall_entry.rs:1179-1197`, `handlers.rs:2377-2385`) nor `manager.rs:3281`/`:3573`; adds no blocking acquire; the only new lock op anywhere is `FREE_FRAMES.try_lock` inside `return_lease`. The two `abandon` calls under PM (`process_task.rs:471`, `manager.rs:1150`) perform an atomic counter increment and nothing else — no allocation, no second lock. Ratchet R8 pins the `with_thread_mut(` site set; the existing `validate_blocking_primitives`/`RAW_SCHEDULER_LOCK_SITES` ratchets (`tests/teardown_structure.rs:308`, `:319`, `:376`) re-check it.

---

## 3. The oracle suite

Design rule (Design B's, adopted verbatim): **a counter lands only in the PR that gives it a live production arm, and every refusal counter has a same-PR injection or a real blocker exercise.** This structurally prevents C18's dead-counter finding.

### 3.1 Counters — unconditional, both architectures

All declared with `counter!` (`teardown.rs:336+`) → `define_trace_counter!`, added to `COUNTERS` (`:449`) with `COUNTER_COUNT` (`:444`) updated in the same commit, exposed via `snapshot()` (`:510`) and `/proc/trace/counters`. **None carries `cfg(target_arch)` or `cfg(feature)`** — C13 and C19 answered structurally.

| PR | Counter | Fires when | Healthy | Live arm in the same PR |
|---|---|---|---|---|
| 1a | `FRAME_RETURN_REFUSED_DOUBLE` | return of a frame already `ST_FREE` | 0 | O2/A injection |
| 1a | `FRAME_RETURN_REFUSED_STALE` | lease generation ≠ ledger generation | 0 | O2/B injection |
| 1a | `FRAME_RETURN_REFUSED_UNTRACKED` | frame outside every usable region | 0 | O2/C injection |
| 1a | `FRAME_DUPLICATE_ALLOC_REFUSED` | allocation popped an `ST_ALLOCATED` frame | 0 | O2/D injection |
| 1a | `FRAME_LOST_CONTENDED` | `FREE_FRAMES.try_lock` failed (`:345-352`) | small | O2/E holds the real lock |
| 1b | `PT_TABLE_FRAMES_RECORDED` | each `TableRecorder` push | grows | cohort O1 |
| 1b | `PT_ROOT_ABANDONED_NO_PROOF` | `abandon(NoProofPipeline)` | **>0 until PR-2** | every aarch64 non-deferred exit |
| 1b | `PT_ROOT_ABANDONED_NO_ARCH` | `abandon(NoArchPipeline)` | **>0 until PR-3** | every x86 exit — C21's detection surface |
| 1b | `PT_ROOT_ABANDONED_TERMINATED` | `abandon(AlreadyTerminated)` | 0 today (see note) | O2/G direct producer |
| 1b | `PT_ROOT_DROPPED_UNDECIDED` | `Drop` with `Undecided` | **0**, CI-asserted | O2/H drops an undisposed table |
| 1b | `PT_EXEC_WALK_LEASES_UNRETURNED` | leases dropped by `cleanup_for_exec` | >0 until PR-2 | every exec |
| 1c | `PT_ROOTS_RETIRED` | each completed `retire_bounded` | grows | cohort O1 |
| 1c | `PT_TABLE_FRAMES_RETURNED` | `ReturnOutcome::Returned` inside retirement | grows | cohort O1 |
| 1c | `PT_RETIRE_FRAMES_LOST` | `LostContended` inside retirement | 0 | O2/E |
| 1c | `PT_ROOT_DROPPED_MID_RETIRE` | `Drop` with `Retiring` | **0**, CI-asserted | O2/I |
| 1c | `PT_RETIRE_BUDGET_REQUEUED` | budget exhausted, receipt requeued | 0 typical | O2/F builds >64 tables |

`COUNTER_COUNT`: 47 → 52 (1a) → 58 (1b) → 63 (1c). The identity `PT_ROOTS_RETIRED + Σ abandons + PT_ROOT_DROPPED_UNDECIDED + PT_ROOT_DROPPED_MID_RETIRE == roots destroyed` is production-checkable and is why `abandon` takes a reason.

*Note on `PT_ROOT_ABANDONED_TERMINATED`:* Judge 2 established that in the current cohort a repeat `handle_thread_exit` finds `page_table == None` (`manager.rs:1142-1165`), so the arm may legitimately never fire in production. It is therefore **not presented as proof of anything**: the site is pinned structurally by R5, its healthy value is 0 with the reason stated, and its live producer is a direct call in O2/G. That is the honest reading of C18 — no arm claims evidence it does not have.

### 3.2 O1 — the standing cohort (extends `fork_exit_defer_reclaim_pairing_test`)

Main's harness is already right: `teardown.rs:918`, 64 forked children across 9 adapted-site classes, per-PID slots (`BootTestPidCountSlot`, `:518`), strict per-PID scoring at `:1143-1157`, Acquire fence before scoring. **C4 is already closed on main** — the branch's `BootTestRootCounts` is redundant. Four additions:

1. **Anti-vacuity, computed not hard-coded.** Cohort roots are built by `ProcessPageTable::new()` at `:946`, `:1006`, `:1067` with no mappings, so table counts would be trivially satisfiable. Before each child exits, map three pages in three *distinct* L1 subtrees (`0x0000_0000_0040_0000`, `0x0000_0080_0040_0000`, `0x0000_0100_0040_0000`), forcing 3×(L1+L2+L3)=9 table frames. The expected value is derived by the test from the VAs it chose: `expected_tables = distinct_l1_subtrees * 3`. **No IRQ mask** — #528 is fixed, and masking the corruption class under test is exactly what C10 forbids.
2. **Four new per-PID fields** on the existing slot (`:518-530`): `table_frames_recorded`, `table_frames_returned`, `table_frames_lost`, `roots_retired`, written through the existing `record_boot_test_pid_count` (`:578`) / read through `boot_test_pid_counts` (`:660`, widened past its current `(defer, reclaim)` tuple).
3. **Per-PID equalities, never sums** (C4; the fix for Design C's `>= 4` floor, which Judge 2 scored VIOLATED):
   ```
   defer_count            == 1                      (existing)
   reclaim_count          == 1                      (existing)
   roots_retired          == 1
   table_frames_recorded  == expected_tables        (anti-vacuity floor, test-derived)
   table_frames_returned  == table_frames_recorded + 1   (+1 = root; committed effects only)
   table_frames_lost      == 0
   ```
4. **Global floors both directions over the cohort window:** `PT_ROOT_DROPPED_UNDECIDED` delta == 0, `PT_ROOT_DROPPED_MID_RETIRE` delta == 0, `PT_ROOTS_RETIRED` delta == number of deferred children.

Judge 2's finding 6 (the class-1 immediate-release fixture sits outside the tracked PID array, and no tracked root witnesses `abandon`) is resolved two ways: the immediate-release root's PID is registered in the tracked array so `abandon(NoProofPipeline)` has a per-PID witness, and every remaining disposition gets a direct producer in O2 rather than a contorted cohort fixture.

### 3.3 O2 — the standing injection gate (`Arch::Any`, both architectures, nothing masked)

New `frame_custody_refusal_gate_test`, registered beside `deferred_fault_ring_overflow_injection` (`test_framework/registry.rs:5357-5363`), `TestStage::EarlyBoot`. Every sub-case asserts the counter that must fire **and** the counters that must not, then restores a clean state and asserts a healthy operation still succeeds (so no counter latches and no refusal is sticky).

| # | Injection | Must fire | Must NOT fire |
|---|---|---|---|
| A | lease a frame, `return_lease` twice | `FRAME_RETURN_REFUSED_DOUBLE` += 1 | first return `Returned`; frame appears exactly once in `FREE_FRAMES` |
| B | return a lease whose generation was bumped by a re-allocation of the same frame | `FRAME_RETURN_REFUSED_STALE` += 1 | the current owner's frame stays `ST_ALLOCATED` |
| C | `return_lease` on a synthesized out-of-region frame | `FRAME_RETURN_REFUSED_UNTRACKED` += 1 | nothing enters `FREE_FRAMES` |
| D | push a still-`ST_ALLOCATED` frame onto `FREE_FRAMES` (test-only hook), then allocate | `FRAME_DUPLICATE_ALLOC_REFUSED` += 1; `map_page` returns `MapError::AllocatorCorrupt` | **no OOM is reported**; the live owner's ledger entry is unchanged (C22 in both directions) |
| E | hold `FREE_FRAMES` on this CPU across a retirement (real `try_lock` failure, real production path) | `FRAME_LOST_CONTENDED`, `PT_RETIRE_FRAMES_LOST` == tables+1 | `PT_TABLE_FRAMES_RETURNED` += 0 — and this is reported as *loss*, never as corruption (Design B's budget/contention-vs-refusal separation, r2-F13) |
| F | build a table with >64 recorded leases, retire | `PT_RETIRE_BUDGET_REQUEUED` ≥ 1; completion after N passes with `returned == recorded + 1` | no refusal counter — **an oversized address space is never labelled corrupt** |
| G | `abandon(AlreadyTerminated)` on a constructed table | `PT_ROOT_ABANDONED_TERMINATED` += 1 | no frame returned |
| H | drop a constructed table with `Undecided` | `PT_ROOT_DROPPED_UNDECIDED` += 1 | `PT_TABLE_FRAMES_RETURNED` += 0 — the backstop counts, never frees |
| I | interrupt a retirement at the budget and drop the object | `PT_ROOT_DROPPED_MID_RETIRE` += 1 | no double return of an already-popped lease |
| J | call `retire_bounded` again after `Complete` | all deltas 0 | idempotence |

Sub-cases A–E drive the **real production primitives** with real state, not a simulated flag; E in particular is the most authentic refusal in any of the three designs (Judge 1's praise for Design C's O2/B) and is preserved here.

### 3.4 O3 — cross-architecture non-vacuity (C19)

The same test, both arches, inverted expectations: after a real process exit, x86 asserts `PT_TABLE_FRAMES_RETURNED` delta == 0 **and** `PT_ROOT_ABANDONED_NO_ARCH` delta == 1 (the residual leak is *measured*); aarch64 asserts `PT_TABLE_FRAMES_RETURNED` delta ≥ 4 **and** `PT_ROOT_ABANDONED_NO_ARCH` == 0. Neither direction is compile-time vacuous; no `cfg(not(aarch64))` assertion exists anywhere in the suite.

### 3.5 Mutation matrix

C7 demanded four designated mutations, two of which the branch never recorded. Six of them become **standing negative ratchets** inside the existing `deliberately_broken_variants_fail_the_ratchet` test (`tests/teardown_structure.rs:1285`) via `with_replaced_source` (`:91`), so they run on every `cargo test --test teardown_structure` and cannot go stale; two require a boot and are recorded as serial excerpts in both directions.

| ID | Mutation | Must fail |
|---|---|---|
| M1 (structural) | add a `return_lease`/`deallocate_frame` call outside `retire_bounded` in `process_memory.rs` | R1 (span membership — the count-equality evasion of C6 does not work) |
| M2 (structural) | restore a bare `drop(process.page_table.take())` at any adapted site, including `drop({ … })` and `core::mem::drop(…)` | R5 (site set + literal reason) |
| M3 (structural) | pass `&mut GlobalFrameAllocator` to the mapper again | R2 |
| M4 (structural) | make `Drop` free instead of count | R4 |
| M5 (structural) | put a `log::info!` in `retire_bounded` or `return_lease` | R7 |
| M6 (structural) | move the ledger init call after the first process constructor | R9 |
| M7 (**runtime**) | stub the root return out of `retire_bounded` | O1 per-PID `returned == recorded + 1` fails **for the first affected PID**, not a cohort sum |
| M8 (**runtime**) | remove the cohort's three sentinel mappings | O1 anti-vacuity floor `recorded == expected_tables` collapses to 0 — **the mutation C7 says proves the floor is not satisfiable by a hollow hierarchy** |

**On the retired mutation.** C7 designated "delete the L1 loop from the aarch64 walk" as the headline anti-vacuity evidence. That mutation has no subject here: the walk is gone. M8 replaces it and is strictly stronger — the floor is computed by the test from the VAs it chose, so a hollow hierarchy cannot satisfy it. This substitution is stated in the PR body rather than silently made (Judge 1 scored Design A's version PARTIAL for exactly the omission of that statement).

---

## 4. Structural ratchets (`tests/teardown_structure.rs`)

Main's harness already provides what C5/C6 asked for and the branch failed to use: `sites_matching` → `BTreeSet<(path,line)>` (`:48`), `assert_exact` (`:70`), `validate_exact` (`:74`), `with_synthetic_source` (`:80`), `with_replaced_source` (`:91`), `function_body` — a lexically scoped byte span with comment/string awareness (`:116`, self-tested at `:237`).

| ID | Property (invariant) | Expression |
|---|---|---|
| **R1** | One physical-return choke point (I4, C6) | every `FREE_FRAMES` … `push(` occurrence in `kernel/src/memory/frame_allocator.rs` ⊆ `function_body(src,"return_lease")`; and `sites_matching(deallocate_frame(\|return_lease()` in `process_memory.rs` ⊆ `function_body(src,"retire_bounded")` ∪ the frozen pre-existing exec-walk set `{1744,1783,1820,1832,1836,1840,1845,1906,1934,1961,1973,1977,1981,1986}` via `assert_exact`. **Span membership, never counts.** |
| **R2** | The recorder cannot be bypassed (I1) | `GlobalFrameAllocator` occurrences in `process_memory.rs` == ∅; `assert_exact` on the two `TableRecorder` construction sites; source contains neither `pub fn mapper(` nor `pub fn allocate_stack(` nor `fn deep_copy_pml4_entry` / `deep_copy_l3_entry` / `deep_copy_l2_entry` |
| **R3** | Freeing happens only downstream of the merged proof (I7, C23) | `assert_exact(sites_matching(".retire_bounded("), &[("kernel/src/task/process_task.rs", <line in reclaim_bounded>)])` |
| **R4** | The backstop cannot free (I8) | `function_body` of `impl Drop for ProcessPageTable` contains `trace_count!` and contains none of `deallocate_frame`, `return_lease`, `retire_bounded` |
| **R5** | Every non-freeing disposition is explicit (C5) | `assert_exact` on the `abandon(` sites `{process_task.rs:291, :471, manager.rs:1150}` each with its literal reason; and each `cleanup_for_exec` body (`process_memory.rs:1689`, `:1861`) contains exactly one `Disposition::RetiredByExecWalk`; and the structural set of `drop(...)` expressions whose argument subtree contains `.page_table.take()` is **empty** outside that pinned set — normalized over blocks, parens and `core::mem::drop` |
| **R6** | Counter inventory stays complete and readable (C13) | `declarations.len()` equality (`:655`, 47 → 63) and the existing readers==declarations equality (`:651`) |
| **R7** | The drain stays minimal (C3) | `function_body` of `retire_bounded`, `return_lease`, `reclaim_bounded` contains none of `log::`, `serial_println!`, `format!`, `vec!`, `Vec::new`, `Vec::with_capacity`, `alloc::` |
| **R8** | #527 not widened | pinned exact `with_thread_mut(` site set, unchanged |
| **R9** | Ledger exists before the first process root (B's discipline; fixes A's init-order defect) | `assert_exact` on `init_frame_ledger()` call sites `{memory/mod.rs:<post-heap>, main_aarch64.rs:492}`; every `ProcessPageTable::new` body's first `allocate_frame_leased()` is preceded by `ensure_frame_ledger()` in the same span |

Each ratchet ships with a negative variant built by `with_replaced_source` asserting `validate_exact(...).is_err()`, using a *syntactically different* forbidden form (block-wrapped drop, qualified `core::mem::drop`, an outside free swapped for an inside free, a constructor moved to another impl) so the ratchet is proven to recognize spans and semantics rather than today's spelling.

---

## 5. x86 parity — verified, not assumed

**What changes on x86 in PR-1:** the ledger and its guard (shared file, both arches); `ProcessPageTable` gains `root_lease`/`tables`; `new()` leases the PML4 (`:380`) and the PML4[0] PDPT (`:601-602`); `map_page`/`update_page_flags` pass `TableRecorder`; `release_process_resources` calls `abandon(NoArchPipeline)`; both `cleanup_for_exec` bodies set `RetiredByExecWalk`; all counters and O2/O3 compile and run.

**What does not change on x86: not one frame is freed differently.** `cleanup_for_exec`'s x86 walk (`:1689-1855`) is untouched; `cleanup_cow_frames` (`process/process.rs:567`) is untouched; there is no `retire_bounded` call in x86-reachable code (R3 pins the single site inside `#[cfg(target_arch = "aarch64")]` code). x86 keeps leaking roots exactly as main does — now counted by `PT_ROOT_ABANDONED_NO_ARCH`, which is the detection surface C21 demanded.

**Why the kernel-shared upper half is safe under custody:** x86 `new()` installs master PML4[256..512] by `set_addr` without allocating; those frames are never leased, so `retire_bounded` cannot reach them when PR-3 enables it. Custody makes structural what main achieves with a `USER_ACCESSIBLE` filter (`:1718`, `:1755`) — the heuristic C2 showed can desync. If a user mapping ever lands under a kernel-shared L3, the recorder simply does not record that L3 (we did not allocate it) while recording the PD/PT beneath it (we did) — exactly right, and the case that broke the branch's counting model.

**Compile-time consequence to check first (Design C's catch):** adding `Drop` to `ProcessPageTable` forbids partial moves of the value. Both `cleanup_for_exec` bodies take `self` and read the `Copy` field `level_4_frame` (`:1845`, `:1986`); that stays legal, but the x86 build is the proof, and it is the first command in the acceptance record.

**Verification record required in every PR body** (all x86 work runs on beast per CLAUDE.md): `cargo build --release --features testing,external_test_bins --bin qemu-uefi` grepped for `^(warning|error)` → empty; `./docker/qemu/run-boot-parallel.sh 3` green; kthread 3/3 green; O2 and O3's x86 assertions executed in the x86 test build. Same test, same file, both arches — the cheapest possible proof that the one shared mechanism behaves identically.

**PR-3's x86 liveness proof (scope stated now, so parity is not assumed):** x86 is a one-CPU scheduler configuration on main (`scheduler.rs:606-610`), so there is no remote-hardware-root claim; on exit, custody is queued rather than released while CR3 may still name the dying root; a fixed-budget drain runs at the next `check_need_resched_and_switch` entry before any lock, refusing if hardware CR3 has not yet moved; the proof reads actual CR3, both per-CPU shadows (`arch_impl/x86_64/percpu.rs:176-225`), all live process rows, and the selected next root. **`kernel/src/interrupts/context_switch.rs` is a Tier-2 high-scrutiny file: PR-3 requires explicit user approval before that edit** (Design B's gate, adopted — Design A's PR-3 omitted it). PR-3 also records the corrected x86 direct-allocation inventory: the fresh PML4[0] PDPT at `:601-602` only (**not** the commented-out `:801-:924`).

---

## 6. Post-#528 defense decision table

#528 is fixed (general-regs-only kernel via `aarch64-breenix-kernel.json`, unmasked 0/60), so construction-time descriptor corruption is no longer a live producer. Independently, **this design reads no descriptor on any free path**, which retires a second and larger class of defenses. Each prior mechanism is judged on both grounds; "load-bearing" means removing it makes a custody or free proof false.

| # | Prior defense (branch location) | Verdict | One-line justification |
|---|---|---|---|
| 1 | `page_table_with_sentinel()` IRQ mask (`teardown.rs:1125`, `:1157`, `:1261`) | **DROP the mask, KEEP the sentinel mapping** | The mask existed only for #528, and C10 forbids masking the corruption class the refusal is scored on; the *mapping* stays as the anti-vacuity witness. |
| 2 | `structure_balance_proved` / `reclamation_preconditions_proved` gate | **DROP** | It gated freeing on a derived count; there is no derived count, and a gate whose failure mode is "quietly stop fixing the bug" is worse than none (C2). |
| 3 | `frame_has_allocator_provenance` ("ever allocated, below frontier") | **DROP; replaced, load-bearing** | C9: it proves nothing about *this* address space; the ledger generation + the local record prove exactly that, in O(1). |
| 4 | `free_list.contains()` membership check | **KEEP the property, REPLACE the implementation; load-bearing** | O(free-list) on the drain is r3-F4's exact objection; the O(1) ledger state+generation catches strictly more (stale authority after reuse). |
| 5 | `deallocate_frame_proven_owned` two-tier split | **DROP** | The split is what let a caller bypass the guard (C15); one choke point, two thin wrappers, one guard. |
| 6 | `duplicate_table_frame` re-walk per frame (O(T²·512)) | **DROP** | C11; uniqueness now comes from single-recording plus the ledger, at zero drain cost. |
| 7 | `RetireTableFrames` 1024-entry / 8 KiB stack array | **DROP** | C12; there is no walk result to buffer and the record already lives in the existing `Box`. |
| 8 | `LeafPolicy::{DecrefAndFree, StructureOnly}` | **DROP** | C14: caller-declared ownership with no backstop; PR-1 frees no leaf and PR-2 derives ownership from allocator state. |
| 9 | `LeafCustody { Owned, AlreadyReleased }` threading | **DROP** | Same root cause; the exit-path leaf release is untouched, so there is nothing to parameterise. |
| 10 | `leaf_mappings: BTreeMap<(root_addr, frame_addr), …>` | **DROP** | C16 (recyclable key, never purged) and C17 (frame-keyed) are properties of *having a side registry*; custody is a field of the owning value. |
| 11 | `RootRetireProof` token + `superseded_by_exec()` | **DROP the token, KEEP the obligation; load-bearing** | C23: a token minted with no check presents as discharged; the obligation becomes a one-call-site positional invariant frozen by R3. |
| 12 | `UNPROVED_ROOT_DROP_REFUSED` + non-freeing `Drop` | **KEEP, reshaped; both arches, production** | The one prior defense worth its lines: it is defense-in-depth for correctness but **load-bearing for the oracle**, and it converts the x86 residual into a measured number (C19/C21). |
| 13 | `ProvenanceRefused` arm + its four handling sites | **DROP as built; the concept survives as the four ledger refusals** | C18: the branch's arm was unreachable; each replacement has a reachable trigger and a standing injection (O2/A–D). |
| 14 | `ProcessTableFrames::allocate_frame` dedup-with-OOM | **DROP the dedup, KEEP the shim; replaced** | C22: duplicate detection moves *upstream* to allocation, where it can withhold the frame; the recorder has no policy and therefore no failure mode. |
| 15 | Descriptor provenance/range preflight before descending | **DROP for PR-1; optional audit-only later** | It cannot add to or veto an allocation-derived free set; keeping it as production code (Design B) buys a walk whose only consumer is a test. |
| 16 | `PT_NONUSER_LEAF_SEEN` as a release input | **DROP as authority; audit only** | User flags express access, not ownership; x86 intentionally shares non-user kernel hierarchy. |
| 17 | Per-PID boot-test scoring | **KEEP as a second-line oracle** | C13's objection was that it was the *only* loud signal; behind unconditional production counters it is in its correct role. Main's version is already sound — extend, don't rewrite. |
| 18 | String-blacklist / count-equality ratchets | **DROP** | C5/C6; replaced with exact site sets plus `function_body` span membership, which main's harness already supports. |
| 19 | `mutation-evidence.txt` artifact | **DROP** | C7: incomplete and unenforced; six mutations become standing negative ratchets, two remain runtime evidence. |
| 20 | Try-lock-refusal leak on `FREE_FRAMES` contention | **KEEP the behaviour, ADD the meter** | Converting it to a retry would put a spin on the drain (C3/C11); `FRAME_LOST_CONTENDED` + `PT_RETIRE_FRAMES_LOST` make the loss countable and reported as loss, never as corruption. |
| 21 | Receipt custody, bounded pass, park/unpark | **KEEP unchanged (main's, not the branch's); load-bearing** | They solve custody escape and livelock independently of #528, and the budget requeue reuses them verbatim. |
| 22 | Fail-loud philosophy generally | **KEEP** | Independent of #528: every refusal here leaks rather than frees, and every leak is counted. |

**Net: 5 of 22 survive** (4, 12, 17, 20, 21) — three of them main's own machinery. Seventeen existed to make walk-derived freeing safe, or to paper over an authority model that did not exist.

---

## 7. PR plan against the size law

Law (`PLAN.md:208-210`): **≤ ~230 changed non-generated lines across ≤ 5 production files**; measure with `git diff --numstat` *before* opening; over the ceiling ⇒ split at the named seam, never waive.

The branch failed at 1718 insertions / 10 files. The honest consequence of adding a real authority layer is that "PR-1" is **three** merges, each independently green, each independently revertable, each closing something. This is not Design B's shape (two foundations before anything ships): **PR-1a ships a live double-free guard for every caller on both arches on day one, and PR-1b ships production visibility of the leak it will close.**

### PR-1a — "frame ledger: O(1) generation-checked frame returns"

| File | Change | Est. |
|---|---|---|
| `kernel/src/memory/frame_allocator.rs` | ledger statics, ordinal map, `init_frame_ledger` seeding, `FrameLease`, `allocate_frame_leased`, `return_lease`, duplicate-at-allocation detection, `deallocate_frame` routed through the transition | +85 / −6 |
| `kernel/src/memory/mod.rs` | one post-heap `init_frame_ledger()` call | +2 |
| `kernel/src/main_aarch64.rs` | same, at `:492` | +2 |
| `kernel/src/tracing/providers/teardown.rs` | 5 counters + inventory + `COUNTER_COUNT` 47→52 | +22 |
| **Total** | **4 production files** | **~117** |

Tests: O2/A–E + R1/R9 + negatives (~90 lines, outside the production count).
**Live effect:** every allocator caller on both architectures gains a double-release guard and a duplicate-allocation refusal, with five injection-exercised counters. **Revert:** total; restores main's allocator exactly.

### PR-1b — "process page-table custody record (instrumentation only, frees nothing)"

| File | Change | Est. |
|---|---|---|
| `kernel/src/memory/process_memory.rs` | `OwnedTableFrames`, `TableRecorder`, `Disposition`/`AbandonReason`, `abandon`, non-freeing `Drop`; lease the root at `:303`/`:380` and the x86 PDPT at `:601-602`; swap the allocator at `:1256`/`:1366`; set `RetiredByExecWalk` in both `cleanup_for_exec` bodies (`:1689`, `:1861`); **delete** `mapper()` (`:1565`), `allocate_stack()` (`:1571`) and the three dead `deep_copy_*` helpers (`:105`, `:170`, `:251`) | +105 / −64 |
| `kernel/src/task/process_task.rs` | `:291` → `abandon(NoProofPipeline\|NoArchPipeline)`; `:471` → `abandon(AlreadyTerminated)` | +7 / −2 |
| `kernel/src/process/manager.rs` | `:1150` → `abandon(AlreadyTerminated)` | +3 / −1 |
| `kernel/src/tracing/providers/teardown.rs` | 6 counters + inventory + `COUNTER_COUNT` 52→58 | +26 |
| **Total** | **4 production files** | **~208 changed (−67 of it deletions)** |

Tests: O2/G–H, R2/R4/R5/R6/R7 + negatives (~110 lines).
**Live effect:** every root drop on both arches is now classified and counted — the leak becomes a production number (`PT_ROOT_ABANDONED_*`, `PT_EXEC_WALK_LEASES_UNRETURNED`) before a single line of freeing lands. **Behaviour-preserving by construction:** no `deallocate_frame`/`return_lease` call is added anywhere, which R1 proves. **Revert:** total.

### PR-1c — "aarch64: retire process roots and tables from recorded custody"

| File | Change | Est. |
|---|---|---|
| `kernel/src/memory/process_memory.rs` | `retire_bounded` + `RetireProgress` + committed-effect accounting | +40 |
| `kernel/src/task/process_task.rs` | `reclaim()` → `reclaim_bounded()`; `:186` `drop` → `retire_bounded`; budget/requeue arm at `:925-935`; move `record_reclaim` to the `Complete` branch; `leaves_released` flag on `PendingProcessReclaim` | +35 / −6 |
| `kernel/src/tracing/providers/teardown.rs` | 5 counters + inventory (58→63) + 4 per-PID slot fields + cohort sentinel mappings + strict per-PID scoring | +105 |
| **Total** | **3 production files** | **~186** |

Tests: O1 extension, O2/E–F/I–J, O3, R3 + negatives, M7/M8 runtime evidence (~120 lines).
**Live effect:** #470-F3's aarch64 half is closed. **Revert:** restores the leak, leaves 1a/1b's safe observation layer intact.

**Combining seam:** if 1a and 1b together measure ≤230 changed lines across ≤5 files at `git diff --numstat`, they may merge as one PR. Do not combine 1c with anything — it is the only PR that changes what gets freed, and it must be revertable alone.

**Acceptance record, every PR** (SPEC-470 §12, adapted): both aarch64 builds warning-free and error-free; `cargo test --test teardown_structure` green including all negatives and the updated `declarations.len()`; one `docker/qemu/run-aarch64-full-test.sh --boot-tests-only --rebuild` reaching `[BOOT_TESTS:PASS]`; `clean-gate.sh` 0/100 and `starved-gate.sh` 0/100 under 14 host hogs; **beast x86 build + boot 3/3 + kthread 3/3**; **Parallels 3× consecutive long-window green**, each from a force-stopped VM with a truncated serial log, reaching `bsshd: listening` with no `DATA_ABORT`, soft lockup, panic marker, or nonzero `PT_ROOT_DROPPED_*` (mandatory — #525 established that Parallels gates kernel-path merges and QEMU alone missed a deterministic boot fault); `git diff --numstat` inside the ceiling with the count quoted in the body; revert story verified by a `git revert` dry run; a plain statement of what #470 still owes. All QEMU processes killed and `pgrep` empty before reporting.

### The rest of the campaign

| PR | Scope | Why not earlier |
|---|---|---|
| **PR-2** | #470-F4 + leaf custody: virtual-page-keyed leaf records classified from allocator state (§1.6), fork/CoW/external conversion, `frame_decref` untracked arm flipped fail-closed, aarch64 `cleanup_for_exec` (`:1861`) collapsed into `retire_bounded` + a `walk_mapped_pages` decref (deleting 3 `Vec`s and a `log::info!` from the drain), failed-exec release for never-published tables. Named internal seam: (i) leaf ledger + classifier, (ii) fork/external + exec conversion. | Converting the leaf-introduction sites is a ≥5-file change on its own; flipping `frame_decref` before they are converted turns today's over-free into a mass leak. |
| **PR-3** | x86 liveness proof + `abandon(NoArchPipeline)` → `retire_bounded` (§5). **Requires explicit user approval for the Tier-2 `interrupts/context_switch.rs` edit.** Success is trivially observable: `PT_ROOT_ABANDONED_NO_ARCH` → 0 while `PT_ROOTS_RETIRED` picks up the traffic. | No x86 root-liveness proof exists; freeing an x86 PML4 that CR3 may still name is the inversion the campaign exists to prevent. |
| **PR-4** | Delete the x86 `cleanup_for_exec` walk (`:1689-1855`) in favour of custody + decref. | It changes x86 exec-path leaf semantics (the walk decrefs non-`USER_ACCESSIBLE` leaves that `cleanup_cow_page_table` skips); a semantic delta needs its own PR and its own evidence — exactly the mistake C21 flagged. |

---

## 8. Salvage from `fix/470-process-root-reclaim`

The branch is based on **pre-#528 main** (merge-base `e73b2205`), before the general-regs-only kernel target. **Nothing cherry-picks cleanly; branch fresh from `363eb912`.**

**Salvage (ideas, re-implemented small):**

| From | What | How it lands here |
|---|---|---|
| `8e899680` | `ProcessTableFrames<'a>` — a mapper-local recording allocator. **The one genuinely right idea in 1718 insertions.** | `TableRecorder` (§1.3), stripped of the dedup/OOM logic C22 condemned, recording generation-bearing leases |
| `af0b2d74` | `retire(self, …)` consuming shape | `retire_bounded(&mut self, pid, budget)` — the same custody signature, made resumable for C11 |
| `b7bfcdee` | `UNPROVED_ROOT_DROP_REFUSED` + a `Drop` that counts and cannot free | `PT_ROOT_DROPPED_UNDECIDED`/`_MID_RETIRE`, promoted from aarch64+boot-tests-only to production, both arches (C19/C21) |
| `4f8057ca`, `e15b60a5` | strict per-PID scoring | main has independently landed the equivalent (`teardown.rs:518-530`, `:1143-1157`) — **salvage the idea of adding table fields to the existing slot; do not port `BootTestRootCounts`** |
| `0a1228ce`, `e8cb2c53` | the R-series ratchet ambition and mutation organisation | re-expressed with main's `sites_matching`/`function_body` span helpers so C5/C6's "counts and string blacklists" objection cannot recur |
| branch `teardown.rs:426-517` | the `PT_*` counter naming/description convention | kept |

**Discard outright** (and the reason): `8e899680`+`e15b60a5`'s entire `frame_metadata.rs` `FrameRegistry`/`leaf_mappings` rewrite (C15/C16/C17 all attach to that side registry; restores `frame_metadata.rs` to its untouched main state); all of `fork.rs` (+68 — table custody is never inherited); all of `syscall/graphics.rs` (+4 — collateral from the two-tier free split); `RootRetireProof`/`superseded_by_exec` (C23); `LeafPolicy` and `LeafCustody` (C14); `frame_has_allocator_provenance` (C9); `deallocate_frame_proven_owned` (C15); `FrameDeallocationOutcome::ProvenanceRefused` and its four arms (C18); `begin_walk_cross_check`/`record_walk_frame` (C11/C12); `RetireTableFrames` (C12); `structure_balance_proved` (C2); `page_table_with_sentinel`'s IRQ masking (C1/C10 — keep the mapping, drop the mask); the string-blacklist ratchets (C5/C6); `scratchpad/470/mutation-evidence.txt` and the `mutation-r1-*.log` artifacts (C7 — superseded by standing negative ratchets that cannot go stale).

---

## 9. Honest residuals

1. **x86 roots and tables still leak after PR-1** — by design (§5), now *measured* (`PT_ROOT_ABANDONED_NO_ARCH`, 1 per exit). Closed by PR-3.
2. **aarch64 non-deferred and already-terminated exits still leak** — `abandon(NoProofPipeline)` / `abandon(AlreadyTerminated)`, counted, closed by PR-2. Freeing at those sites without a proof is the regression this design refuses to ship.
3. **The exec-supersede walks keep their pre-existing defects** — descriptor-derived frees, subtrees unlinked by `clear_user_entries` (`:1614`) invisible to the walk, `USER_ACCESSIBLE` filtering. PR-1 neither fixes nor worsens them; their dropped leases are counted by `PT_EXEC_WALK_LEASES_UNRETURNED`. Closed by PR-2/PR-4.
4. **`FRAME_LOST_CONTENDED` frames are genuinely lost** — `deallocate_frame`'s contention path (`:345-352`) drops the frame; pre-existing, unchanged, now countable. A retry would put a spin on the drain (C3/C11).
5. **The pre-existing leaf and exec walks on the drain path remain unbounded.** Only the work PR-1 adds is budgeted. PR-2 removes the aarch64 walk entirely.
6. **PR-1a may go red on a pre-existing double release.** Main has never had this guard; a nonzero `FRAME_RETURN_REFUSED_DOUBLE` on the first boot is a real defect found by this work (the kernel-stack over-free in §1.6 is the prime candidate) and gets fixed in this work — not disclosed and deferred. Refusal semantics mean the discovery is a counter, not a crash.
7. **Leaf pages under `clear_user_entries`-unlinked subtrees still leak** — the decref walk cannot reach them; pre-existing, not this leak class.

---

## 10. Ratification crosswalk — every constraint to the element that satisfies it

Judge scores are shown as **J1/J2** against the base design (A) so that every downgrade is visibly resolved. "→" marks what this document changed to close it.

| # | Property demanded | J1 / J2 on base | Satisfied by |
|---|---|---|---|
| **C1** | No test-only IRQ mask justifying descriptor-derived frees | SAT / SAT | No descriptor is read on any free path (§1.3); the sentinel *mask* is dropped and the *mapping* kept as the anti-vacuity witness (§6 row 1) |
| **C2** | A miscount must never silently suppress freeing | SAT / SAT | Nothing is counted-then-gated: `retire_bounded` attempts every recorded lease independently and the root regardless (I9); the record is written at each allocation site, so a conditional allocation yields a conditional record (§1.3) |
| **C3** | Zero added log/format/heap on context-switch- or syscall-reachable paths | **PARTIAL** / SAT | → `return_lease` is a **log-free** primitive, so the "+1 `log::trace!` per new free" gap J1 found is closed structurally, not argued; R7 asserts it; §2 prices the one added heap *de*allocation honestly |
| **C4** | Per-object equalities, never cohort sums | SAT / SAT | Per-PID equality `returned == recorded + 1` with a test-derived floor (§3.2); Design C's `>= 4` floor (J2: VIOLATED) is rejected |
| **C5** | Structural containment ratchets, not string blacklists | SAT / SAT | R1/R4/R5 are byte-span membership via `function_body`; R5 normalizes block-wrapped and qualified `drop` (§4) |
| **C6** | "One freeing function" by span, not counts | SAT / SAT | R1 is `⊆ span(...)` plus a frozen exact list; no count equality exists anywhere in the suite |
| **C7** | Every designated mutation recorded, especially anti-vacuity | **PARTIAL / PARTIAL** | → six mutations are standing negative ratchets; M8 (sentinel removal) collapses a **test-derived** floor; the retired "delete the L1 loop" mutation is *stated* as retired-with-its-subject and replaced, rather than silently substituted (§3.5) |
| **C8** | One preflight covering the whole walk; leaves gated like tables | SAT / SAT | PR-1 has zero walks and frees zero leaves, so no second ungated walk exists (§1.1); PR-2 gives leaves the same discharged-proof + valid-lease condition (§1.6) |
| **C9** | Provenance tied to the *specific* retiring address space | SAT / SAT | The record is a field of that address space and membership is creatable only by its own mapper's allocator (I1); the ledger adds generation so identity survives frame reuse |
| **C10** | Refusal exercised by injection, both directions, unmasked | SAT / SAT | O2/A–F: four real refusals, real lock contention, real budget exhaustion, nothing masked, each with a must-not-fire direction and a restore-to-healthy step (§3.3) |
| **C11** | No unbounded or superlinear hot-path work | SAT / **VIOLATED** | → explicit `RETIRE_FRAME_BUDGET = 64` with requeue through main's existing bounded-pass/park machinery; per-frame validation is O(1) (§2, §1.4). J2's blocking objection is closed by a budget, not by an argument that T is small |
| **C12** | No large fixed stack buffers on the drain | SAT / SAT | None; the record lives in the already-existing `Box` |
| **C13** | Loud failure in production, real PIDs, both arches | SAT / SAT | 16 unconditional counters, no `boot_tests`/arch cfg on any; per-PID boot slots demoted to a second-line oracle (§3.1) |
| **C14** | Leaf ownership allocation-derived, not caller-declared | SAT / **PARTIAL** | → PR-1 frees no leaf, *and* the leaf model is fully specified: allocator-state classifier, private lease type, no caller-selectable policy (§1.6), landing in PR-2 where the frees are |
| **C15** | Do not remove the double-free guard; the replacement must catch the same scenarios | SAT / **PARTIAL** | → per-frame **generation + state**, not one bit: catches double release *and* stale authority after reuse (J2's exact objection), O(1), single choke point, both arches, injection-tested (§1.2, O2/A–B) |
| **C16** | Custody key purged on retirement; never a reassignable address | SAT / SAT | No key exists: custody is a field created, moved and destroyed with the owner; ledger indices are ordinals paired with a generation, never an ownership key |
| **C17** | Custody must distinguish mappings by virtual page, not frame | SAT (vacuous) / **VIOLATED** | → the PR-2 leaf model keys records by **virtual page** in a page-table-local sorted vector, so one frame mapped at two VAs yields two records and two balanced references (§1.6). J2's blocking objection is closed by specification, not by calling it page-independent |
| **C18** | No unreachable refusal arm presented as live proof | SAT / SAT | Every counter lands with a live arm in the same PR (§3.1); the one arm that may never fire in production (`ALREADY_TERMINATED`) is explicitly not presented as evidence and is pinned structurally instead |
| **C19** | No compile-time-vacuous assertion on the target arch | SAT / SAT | O3 runs on both arches with inverted live expectations; no counter or ratchet carries an arch cfg (§3.4) |
| **C20** | The standing cohort exercises every path the design adds | **PARTIAL / PARTIAL** | → the immediate-release fixture's PID is registered in the tracked array so `abandon(NoProofPipeline)` has a per-PID witness, and every other disposition gets a direct producer in O2/G–J (§3.2, §3.3) — J2's finding 6 closed with fixtures, not with claims |
| **C21** | No x86 semantic change without backstop, detection and verification | SAT / **PARTIAL** | → x86 frees nothing differently (R1/R3-pinned) and gains the ledger guard, six counters, O2+O3, and a named beast record; the exec-`Drop` false alarm J2 found is closed by `RetiredByExecWalk` (§1.4, §5); the corrected x86 allocation inventory is `:601-602` only (§0.1) |
| **C22** | Duplicate allocator return handled loudly and correctly | SAT / **VIOLATED** | → detected at **allocation**: the frame is withheld, `FRAME_DUPLICATE_ALLOC_REFUSED` fires, `map_page` returns `MapError::AllocatorCorrupt` (never OOM), and the live owner is untouched — so the mapper can no longer zero a frame that is a live table elsewhere (§1.2, O2/D) |
| **C23** | No unconditionally minted proof token | SAT / SAT | Token deleted; the obligation is a single-call-site positional invariant downstream of the merged five-leg `RootProof`, frozen by R3 — and, unlike Design C, the three unproved sites `abandon` rather than free (§1.5) |

**Also carried, from the judges' non-checklist findings:** A-2 (aarch64 ledger init ordering) → §1.2 post-heap `init_frame_ledger` + exact seeding + R9. A's linear `record` scan → removed entirely (no dedup in the recorder; uniqueness comes from the ledger). A's `record_table_frames_reclaimed(count + 1)` attempt-counting → committed-effect accounting (§2). B-1 (map-then-classify ordering) → reserve → publish → commit in §1.6. B-2 (unpriced side table) → 4 B/frame, priced in §1.2. B-3 (partially retired address space) → re-proof on every requeued pass, no proofed-typestate shortcut (I10). Judge 2's `deep_copy_*` gap → deleted in PR-1b and ratcheted by R2. Judge 2's `BuildingPageTable` `Drop`-partial-move concern → avoided: no typestate in PR-1; the `Drop`-forbids-partial-move consequence is checked by the x86 build as the first acceptance command (§5).

**Nothing in C1–C23 is left unresolved, and no item requires operator input.** The one approval gate in the campaign is procedural and belongs to PR-3: editing the Tier-2 file `kernel/src/interrupts/context_switch.rs` (§5).
