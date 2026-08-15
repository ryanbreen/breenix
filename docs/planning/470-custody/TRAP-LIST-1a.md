# TRAP-LIST-1a — the implementer's contract for #470 PR-1a

**Status:** binding. This document plus `470design/DESIGN-470-v2.md` (§1.2, §3.1 PR-1a rows,
§3.3 O2/A–E, §4 R1/R7/R9, §7 PR-1a) is the complete brief. Land PR-1a in one pass by satisfying
both.

**Baseline.** All "current main" anchors are `main` = `363eb912` (`Merge pull request #532`).
`kernel/src/memory/frame_allocator.rs` is 405 lines; `tests/teardown_structure.rs` is 1480 lines;
`kernel/src/test_framework/executor.rs` is 447 lines. Verify with
`git show main:<path> | grep -n …` before anchoring an edit — do not trust a line number that
does not match the symbol named beside it here.

**Reference-only branch.** `fix/470-pr1a-frame-ledger` (tip `7d9a798a`, 9 commits off `363eb912`)
is the five-round artifact these traps were extracted from. **Do not cherry-pick it** — §5 below
says exactly which ideas to re-derive and which commits embody traps. Every "wrong shape" quoted
below is real code from that branch, cited as `branch@<sha>:<file>:<line>`.

**Provenance of every item.** `r1`–`r5` = `470pr1a/review-1a-r{1..5}.md`; `b1`–`b5` =
`blocking-1a-r{1..5}.md`; `fk5` = `fix-notes-1a-r5-kernel.md`; `oe` = `oracle-evidence-1a.md`;
`dev` = `deviation-record-1a.md`.

**Operator rulings that bind this work.**
- 2026-08-11: diff size, line counts and file counts are **not** review criteria and produce no
  findings. Do not weaken coverage or a root-cause fix to hit a line budget; do not cite the
  withdrawn size law as a justification for anything (r4 f7).
- Standing: harness determinism is fixed **at root** — never with sleeps, retries, floors, or
  relaxed equalities (`MEMORY.md`, the #513/#518/#521/#524 flake-purge campaign).
- Standing (#525): Parallels gates kernel-path merges. QEMU-only missed a deterministic boot
  fault once; PR-1a touches the boot path on both arches.

---

# 1. MECHANISM TRAPS

Each trap: **what it is** → **the exact wrong shape** (real code from the branch) → **the
required right shape**, anchored on current main.

---

## MT-1 — The x86 host-RAM boot panic (the headline defect; blocking in r3, r4 and only closed in r5)

**Trap.** `init_frame_ledger` sizes its metadata from the *advertised* usable-frame count. On x86
that count is the bootloader's entire usable map, while the kernel heap is a **fixed 64 MiB**
(`main:kernel/src/memory/heap.rs:27`). Eager whole-map metadata therefore makes *installed RAM
itself* a boot-panic trigger: allocation failure inside `memory::init`, into Rust's alloc error
handler, **before any serial test marker is emitted** — the kernel dies on a machine where `main`
boots fine.

**The exact wrong shape** (`branch@bd8b13e7`, reviewed at r3 f6 / r4 f5 / b4 §5):

```rust
// kernel/src/memory/frame_allocator.rs, inside init_frame_ledger
let ledger: Vec<AtomicU32> = (0..total_frames).map(|index| { … }).collect();  // 4 B/frame, infallible
FREE_FRAMES.lock().reserve_exact(total_frames);                                // 8 B/frame, infallible
assert!(u32::try_from(total_frames).is_ok());                                  // boot panic on huge maps
```

Both live simultaneously, both infallible (no `try_reserve`), no cap, no fallback = **12 B per
advertised frame**. r4 f5 computed the failure point at ~21 GiB of usable RAM; an 8 GiB machine
permanently spends ~24 MiB (37% of the whole kernel heap) on a free list whose steady-state length
is a handful of entries.

**The required right shape** (`branch@9287c09d` is correct in substance here — re-derive it):

1. **Chunked, demand-backed ledger.** A fallibly-allocated *directory* of
   `spin::Once<&'static [AtomicU32]>`, one entry per 65 536-ordinal chunk (256 MiB of RAM), each
   realised slot still the ratified 4-byte generation/state `AtomicU32`:
   `chunks.try_reserve_exact(total_frames.div_ceil(65_536))`. Cost: **24 B per 256 MiB of
   advertised RAM** — ~2 KB at 21 GiB, ~98 KB at 1 TiB (r5 blocker-5 disposition).
2. **Boot realises only chunks below the frontier.** `frontier = NEXT_FREE_FRAME.load(Acquire)`,
   then `ensure_chunk` for each chunk start `< frontier`. On x86 the frontier at ledger-init time
   is page tables + the 64 MiB heap mapping ≈ 16.4 k frames → exactly **one** 256 KB chunk,
   independent of installed RAM.
3. **No whole-RAM free-list reservation.** Reserve `frontier` entries at boot (~130 KB); grow
   capacity afterwards with **`try_reserve`** on the *allocation* side only (see MT-3 for the
   correct failure semantics), always **before** the `NEXT_FREE_FRAME` CAS publishes a new frame.
4. **Cap, don't assert.** `total_frames = advertised.min(u32::MAX as usize)`, and `ensure_chunk`
   returns `Err` for `index >= total_frames` so an oversized firmware map stops exposing frames
   instead of aborting boot. `u32::MAX` stays reserved as the untracked sentinel and is therefore
   never a real ordinal (r4 Part 3, r5 "also checked").
5. Failure of the *small* directory/frontier allocation stays a **loud explicit panic**, not the
   allocator error handler.

**Anchors on main.** `frame_allocator.rs:42` (`NEXT_FREE_FRAME`), `:47` (`FREE_FRAMES`), `:98`
(`get_usable_frame`, the ordinal space to reuse), `:136-160` (the sequential-alloc CAS loop that
must gain the pre-publication `prepare` call), `:177` / `:256` (`init` / `init_aarch64`, source of
`MEMORY_INFO` regions). New `init_frame_ledger` goes in this file.

**Do not** "fix" MT-1 by capping RAM, by `cfg`-disabling the guard on x86, or by catching the
allocation failure and continuing without a ledger. The guard must be live on both arches on day
one (§7 PR-1a "Live effect").

---

## MT-2 — The init-order constraint behind MT-1 (and behind R9)

**Trap.** The ledger is heap-backed, so it cannot be built in `frame_allocator::init*`. The two
arches order heap init differently, and getting this wrong either (a) reintroduces MT-1 by sizing
metadata before the frontier is meaningful, or (b) puts the ledger *after* the first
`ProcessPageTable` root, which is exactly what R9 exists to forbid.

**The facts on current main:**

| Arch | Frame allocator init | Heap init | Where `init_frame_ledger()` must go |
|---|---|---|---|
| aarch64 | `main_aarch64.rs:491` `frame_allocator::init_aarch64(fa_start, fa_end)` | `main_aarch64.rs:492` `memory::init_aarch64_heap()` (which itself does `heap::init` at `memory/mod.rs:52` **and `slab::init()` at `:53`**) | `main_aarch64.rs:493`, i.e. the line immediately after `init_aarch64_heap()` and **before** `kernel_stack::init()` (currently `:493`) |
| x86_64 | `memory/mod.rs:74` `frame_allocator::init(memory_regions)` | `memory/mod.rs:130` / `:135` (two cfg arms) | `memory/mod.rs:~137`, after **both** heap arms and **before** `slab::init()` at `:139` |

**Consequences you must encode, not assume:**

- The aarch64 ordering clause is *not* "ledger before slab" — on ARM `slab::init()` runs inside
  `init_aarch64_heap()` and therefore precedes the ledger. Write the per-arch clauses separately
  (ARM: after `init_aarch64_heap`, before `kernel_stack::init` and `process::init`
  (`main_aarch64.rs:799`); x86: after `heap::init`, before `slab::init`). r4 blocker 9 was raised
  because the ARM branch of the validator silently had *no* "no constructor before the ledger"
  clause at all.
- Because the ledger is heap-backed it allocates **no frames** and cannot alias its own entries
  (r1 "Verified correct").
- `memory::init` is x86-only and is called once, from `main.rs:180`; ARM never calls it. There is
  no double-init today, but `init_frame_ledger` must be idempotent by an early
  `if FRAME_LEDGER.get().is_some() { return; }` — **not** by building the whole structure and
  throwing it away inside `call_once` (r1 f12: that shape rebuilds the entire allocation on every
  redundant call).
- **Pre-ledger returns are legal and must stay legal.** Before publication, `return_lease` still
  pushes to the legacy free list; seeding then fixes up state (MT-5a). This is `dev` deviation 10
  and is required by the x86 ordering.

---

## MT-3 — The spurious-OOM defect (r5 f5, b5 §5): lock contention manufactured out-of-memory

**Trap.** The r5 fix for MT-1 moved free-list capacity growth onto the allocation path. Written
naively, a **transient `try_lock` failure** becomes `allocate_frame() → None`, which every caller
in the tree reads as out-of-memory. Before r5 the sequential allocation path never touched the
free-list lock at all, so contention *could not* fail an allocation. A CoW fault then becomes a
SIGSEGV and a mapping fails, on a kernel with plenty of free memory.

**The exact wrong shape** (`branch@9287c09d:kernel/src/memory/frame_allocator.rs:265-302`):

```rust
fn ensure_free_frame_capacity(required: usize) -> bool {
    if FREE_FRAME_CAPACITY.load(Ordering::Acquire) >= required { return true; }
    let Some(mut free_list) = FREE_FRAMES.try_lock() else { return false; };   // ← transient loss
    …
}
fn prepare_frame_for_allocation(index: usize) -> bool {
    …  ledger.ensure_chunk(index).is_ok() && ensure_free_frame_capacity(index + 1)
}
// BootInfoFrameAllocator::allocate_frame:
if !prepare_frame_for_allocation(current) { return None; }                     // ← reported as OOM
```

Contending parties are all live: `return_lease`'s push, `allocate_candidate`'s pop,
`memory_stats()` (`main:frame_allocator.rs:385`), and O2/E, which **deliberately holds
`FREE_FRAMES.lock()` across a return**.

**The required right shape.** Distinguish "cannot get capacity now" from "no memory":

- On `try_lock` failure, **do not fail the allocation**. Either (a) retry the whole
  `NEXT_FREE_FRAME` CAS loop iteration (it is already a retry loop —
  `main:frame_allocator.rs:136-160`), bounded, or (b) publish the frame and accept that a later
  `return_lease` of it may report `LostContended` (a counted, honest loss) rather than converting a
  transient lock hold into OOM.
- Reserve `Err`/`None` from the prepare path for the **genuine** conditions only: `try_reserve`
  allocation failure, and `index >= total_frames`.
- Keep the invariant that matters: capacity and chunk are prepared **before** the CAS publishes the
  frame, so `return_lease` never needs to grow anything (MT-4).

**Verification that this is fixed:** a mutation that makes `FREE_FRAMES.try_lock()` fail
unconditionally on the prepare path must **not** turn a healthy boot into an allocation failure.

---

## MT-4 — `return_lease` must never allocate, and the ratchet that "proves" it is text-only (r5 f8, b5 §6)

**Trap.** "`return_lease` is allocation-free" is the property that makes the drain-path cost claim
(§2) *literally* true rather than argued, and it is what R7 is named for. Two ways it is false while
every ratchet stays green:

1. **The pre-ledger branch pushes into a `Vec` whose capacity was never reserved**
   (`branch@7d9a798a:frame_allocator.rs:546-548`) — boot-only and single-threaded, but the record
   stated the property unconditionally.
2. **Post-ledger the property rests on `len < capacity`, which is exact-boundary.** Capacity is
   grown to the frontier high-water mark and `len` is bounded by the same frontier, so
   `len == capacity` is arithmetically reachable (every exposed frame simultaneously free). The
   gate's own `inject_duplicate_candidates` pushes 3 duplicates, transiently exceeding the
   distinct-frame bound (`branch:frame_allocator_tests.rs:4-9`).

**The exact wrong ratchet shape** (`branch@7d9a798a:tests/teardown_structure.rs:731-747`): forbid
the substrings `log::` / `format!` / `vec!` / `Vec::` / `alloc::` inside `function_body("return_lease")`.
That cannot see an *implicit* `Vec::push` reallocation — the only way the invariant actually
breaks. The `debug_assert!(free_list.len() < free_list.capacity())` that documents it is compiled
out of the release boot-test build.

**The required right shape:**

- Reserve capacity for the **pre-ledger** window too (or state the exception precisely, in the
  deviation record *and* in a comment on that branch — an unconditional claim about a conditional
  property is the disclosure defect, not the code).
- Make the boundary unreachable by construction: reserve `frontier + K` where `K` covers the
  boot-test injection headroom, or have the injector reserve its own headroom before pushing.
- Keep the substring ban (it is cheap and catches the tempting `log::warn!`), **and** add the
  structural clause it is missing: `function_body("return_lease")` must contain no
  `reserve`/`try_reserve`/`resize`/`with_capacity` call, and the *capacity-preparation* call must
  appear in `prepare_frame_for_allocation` **before** the `NEXT_FREE_FRAME.compare_exchange`
  (span-ordered, not merely present).
- R7's span must cover the transitively-called helpers, not just `return_lease`'s own body:
  `frame_ordinal`, `claim_frame`, `counted` all run on every return/allocation (r1 f5, closed in
  r2 — do not regress it). `deallocate_frame` keeps its pre-existing logging and stays outside the
  span (§1.2 says so explicitly).

---

## MT-5 — Seeding and CAS semantic slips

These are the small, individually-plausible edits that each broke a §1.2 guarantee. All were caught
at least once across the five rounds.

### MT-5a — Seeding order and the pre-ledger window

**Right shape (verified correct in r1, r2 and r3 — do not "improve" it):** at a quiescent
single-threaded boot point, snapshot `frontier = NEXT_FREE_FRAME`, then

1. ordinals `< frontier` → `ST_ALLOCATED(gen 1)`;
2. ordinals `≥ frontier` → `ST_NEVER`;
3. **then** every frame currently in `FREE_FRAMES` → `ST_FREE(gen 1)` — the free-list pass must
   come **after** the frontier pass, or bootstrap frames that are already free get mislabelled.

Pre-ledger bootstrap frames therefore land `ST_ALLOCATED` and their double release **is** caught,
which is the whole point.

**Trap (r2 F12 / r3 f10, never closed).** Quiescence is *assumed*, never checked. Any frame handed
out between the frontier snapshot and publication is seeded `ST_NEVER` while live — and
`claim_frame` CAS-claims `ST_NEVER`, so that frame is **re-allocatable while live**: exactly the
double-hand-out C22 exists to prevent. Safe today only because `heap::init` pre-maps the whole
64 MiB and nothing else allocates frames there — an invariant of a *different* subsystem, held
implicitly.
**Right shape:** one line — `assert_eq!(NEXT_FREE_FRAME.load(Acquire), frontier)` immediately
before publication.

### MT-5b — The low-memory-floor guard must stay unconditional (r1 f1, b1 §1)

**Wrong shape** (`branch@13d3fa29:frame_allocator.rs:502-508`):

```rust
if frame.start_address().as_u64() < LOW_MEMORY_FLOOR {
    log::warn!("Refusing to deallocate frame …");
    if FRAME_LEDGER.get().is_none() { return; }   // ← was an unconditional `return;`
}
```

`init_aarch64` (`main:frame_allocator.rs:256`) applies **no** floor clamp (x86's `init` does, at
`main:frame_allocator.rs:202-208`), so on any aarch64 platform whose `frame_alloc_start()` is below
1 MiB a below-floor frame acquires a valid ordinal and is pushed straight back into the allocatable
pool. The `log::warn!("Refusing …")` also becomes a lie.
**Right shape:** keep `main:frame_allocator.rs:330`'s unconditional `return;`.

### MT-5c — `ST_NEVER` is a distinct refusal class; do not fold it into `RefusedUntracked` (r1 f9)

**Wrong shape:** a `_ =>` arm routing an in-region, never-handed-out frame into `RefusedUntracked`,
so "this address is not memory we manage" and "this frame was never allocated" share one counter —
and it is the counter whose healthy value O2 asserts exactly.
**Right shape:** a sixth outcome `RefusedNeverAllocated` + `FRAME_RETURN_REFUSED_NEVER_ALLOCATED`.
This is a **ratified-design departure** (§3.1 says 5 counters, `COUNTER_COUNT` 47→52; you will ship
6 and 53, and downstream 1b/1c targets shift 58/63 → 59/64). It is sound on the merits — the
`ST_NEVER` case has a real production producer through `deallocate_frame` — but it **must** appear
in the deviation record as a departure, not as a restatement of §3.1 (r3 f8, r4 blocker 8, `dev` 5).

### MT-5d — Do not delete `FRAME_RETURN_REFUSED_STALE` (r2 F1, b2 §1)

**Wrong shape** (`branch@bd8b13e7`): the counter was removed on the premise "`deallocate_frame`
loads the slot's generation immediately before calling `return_lease`, so a mismatch is
impossible." **That premise is false under concurrency** and describes exactly C15's headline
scenario: CPU-A reads generation `G`; before A's CAS, CPU-B double-releases the same frame and a
third party re-allocates it, bumping to `G+1`; A's return takes the stale arm — and with the
counter gone, "a frame was double-released and had already been re-issued to a new owner" leaves
**zero** evidence in a production kernel.
**Right shape:** the counter stays, with the honest disclosure that in a non-`boot_tests` PR-1a
kernel this arm is reachable only through that narrow race (r4 blocker 7, `dev` 6). Do **not**
present the boot fixture as evidence of a common production stale-lease path.

### MT-5e — `frame_ordinal` and `get_usable_frame` must be provably inverse (r1 f11, r2 F11, r3 f17)

**Trap.** `get_usable_frame` (`main:frame_allocator.rs:98`) sizes each region as `(end-start)/4096`
(flooring); a `frame_ordinal` that accepts any address in `start..end` maps the trailing partial
frame to `ordinal_base + region_frames`, which **aliases the next region's ordinal 0**. Two
physical frames then share one ledger slot → a spurious `FRAME_DUPLICATE_ALLOC_REFUSED` (a healthy
frame withheld) or a false `RefusedDoubleRelease` (a frame leaked).
**Right shape:** `debug_assert!(region.start % 4096 == 0 && region.end % 4096 == 0)` in both
`init` and `init_aarch64`, or a round-trip check in `init_frame_ledger`. Both functions must walk
`info.regions[..region_count]` identically — verified sound on the branch (r4 Part 3), keep it that
way.

### MT-5f — `frame_ordinal`'s region-bounds guard is load-bearing (r4 f4, b4 §4)

Dropping `(region.start..region.end).contains(&address)` and returning the running ordinal makes
`deallocate_frame` of an out-of-region frame resolve to a *real* slot, read that slot's current
generation (so the generation check trivially matches), CAS a **foreign** frame's slot
`ST_ALLOCATED → ST_FREE`, and push the foreign frame onto `FREE_FRAMES`. See §2 O2/C for the oracle
that must catch this.

### MT-5g — The bootstrap seeding `expect` is a new boot-panic path (r5 f7)

**Wrong shape** (`branch@7d9a798a:frame_allocator.rs:254-261`):
`ledger.get(index).expect("bootstrap free frame missing ledger chunk")` — `get` returns `None` for
any in-range ordinal whose chunk has not been realised, and boot realises only chunks below the
frontier. Any frame sitting in the free list at ledger-init time with ordinal `≥ frontier` panics
the kernel during `memory::init`. Unreachable today (no `deallocate_frame` call site runs before
`init_frame_ledger` on either arch — verified in r5), but the pre-ledger branch of `return_lease`
pushes **unconditionally with no ordinal check**, so a future pre-ledger free of a reclaimed
loader/firmware page arms it.
**Right shape:** call `ensure_chunk(index)` in the seeding loop and mark; on `Err`, skip the frame
(and count it) rather than panicking.

### MT-5h — Lazy chunk realisation must not spin inside `spin::Once` on the allocation path (r5 f6, PLAUSIBLE)

**Wrong shape:** performing a heap allocation and a 256 KB `resize_with` **inside**
`spin::Once::try_call_once` (`branch@7d9a798a:frame_allocator.rs:65-89`). `try_call_once` spins
while another party runs the initialiser; if that party is preempted or faults and a nested
allocation lands in the **same** chunk (very likely — both are at the frontier), the nested
allocation spins on a `Once` whose initialiser cannot resume. In thread context with interrupts
enabled this resolves; from an interrupt handler with interrupts disabled it is a hard hang.
**Right shape:** build the chunk **outside** the `Once` (allocate + initialise into a local, then
publish with a single `call_once`/CAS, freeing the loser's allocation), so no allocation happens
under the initialiser lock. No current ratchet can see this hazard — it is your job to not
introduce it.

### MT-5i — Every discarded allocation candidate must be countable (r3 f3, r4 f8)

**Trap.** `claim_frame` returns `Err(())` from three places: the counted duplicate arm, and the
**uncounted** ordinal/bounds failure and impossible-state arms. `allocate_claimed` loops on every
`Err(())`, so an untracked candidate is dropped with **no counter and no log** — steady, invisible
frame loss in the one function whose §1.2/residual-4 contract is that every loss is countable.
**Right shape:** either a distinct `Err` variant carrying the reason, or reuse
`FRAME_RETURN_REFUSED_UNTRACKED`/`_NEVER_ALLOCATED` on the allocation side. Do not leave a silent
drain.

### MT-5j — Semantics that are correct on the branch: keep them

Verified sound across r1–r5; re-derive without "improving":

- **`allocate_frame()` itself routes through `claim_frame`**, not only `allocate_frame_leased`
  (`dev` 7). §1.2 only routes the leased API; applying the guard to *every* consumer is stricter
  and is the live safety effect PR-1a ships. It is a **stated** deviation.
- **Duplicate-at-allocation withholds the frame**: `claim_frame` returns `Err` *before* the frame
  escapes, `allocate_claimed` retries with another candidate, the true owner's slot is untouched
  (I6/C22's safety half, live on both arches day one).
- **CAS ordering**: state is committed to `ST_FREE` **before** the free-list push, so a frame is
  `ST_FREE` whenever it is reachable from the list, never the reverse; a lost `try_lock` leaks
  rather than double-publishing.
- **30-bit generation** (`+1` at claim, `& 0x3fff_ffff`), symmetric with the `observed >> 2`
  comparison; `ST_NEVER` (state 0, gen 0) stays distinguishable from a wrapped gen-0 allocation
  because the state bits differ.
- **`return_lease` cross-checks `frame_ordinal(frame) == lease.index`** — stronger than §1.2 asked
  for; keep it.
- **`u32::MAX` untracked sentinel is unaliasable** because `total_frames` is capped at `u32::MAX`,
  so the maximum valid ordinal is `u32::MAX - 1`.
- **`deallocate_frame` is intentionally fail-closed** (`dev` 9): refused returns leak rather than
  entering the reuse pool. This is a **semantic delta on all 32 existing callers on both arches**
  and must be named as such — §1.2's "keeps its behaviour" is accurate only for frames the ledger
  issued (r2 F15, b2 §11).

---

# 2. ORACLE CONTRACT

For every oracle in PR-1a scope: the property it guards, the **mutation command that must make it
FAIL**, and the **MUST-NOT shapes** — evasions discovered across the five rounds. An oracle that
does not fail on its mutation is not an oracle; shipping one is the "non-mutation-sensitive oracle
surviving" blocking category.

**Global rule for every mutation below:** apply it to the **real tree**, compile, run the command,
record the verbatim failure excerpt, then `git checkout --` and re-run to record the green. Line-
count-preserving mutations only, so no line-anchored inventory drifts and gives you an incidental
red (r5 f1 caught its own first attempt failing that way).

---

## R1 — one physical-return choke point (§4/R1, I4, C6)

**Property.** *Every* insertion of a frame into the reuse pool in the tree lies inside
`function_body("return_lease")` — plus, in `process_memory.rs`, the frozen pre-existing exec-walk
`deallocate_frame(` site set pinned by `assert_exact` (the 14 lines named in §4/R1). **Span
membership, never counts.**

**Mutations that must FAIL (`cargo test --test teardown_structure`):**

| # | Mutation, applied inside `allocate_candidate` (which already holds a `free_list` binding) | Source |
|---|---|---|
| M-R1.1 | `free_list.push(frame);` | design M1 / r4 blocker 5 |
| M-R1.2 | `free_list.insert(0, frame);` | r4 f2 → recorded FAIL in `oe` §R1 |
| M-R1.3 | `free_list /* trivia */ .insert(…);` | r5 blocker-2 disposition |
| M-R1.4 | `let list = &mut *free_list; list.insert(0, frame);` | `oe` "reborrowed_free_list" |
| M-R1.5 | **`if let Some(mut aliased) = FREE_FRAMES.try_lock() { aliased.insert(0, PhysFrame::containing_address(PhysAddr::new(0x100000))); }`** | **r5 f1, b5 §1 — CONFIRMED still green on the branch** |
| M-R1.6 | in `frame_allocator_tests.rs`, outside every allowed span: `FREE_FRAMES.lock().append(&mut alloc::vec![frame]); FREE_FRAMES.lock().extend_from_slice(&[frame]);` | r2 F4 — CONFIRMED green at r2 |
| M-R1.7 | add a `return_lease(`/`deallocate_frame(` call in `process_memory.rs` outside the pinned span | design M1 / r1 f7 |

**MUST-NOT shapes (each one shipped at least once and was evaded):**

- **MUST NOT** enumerate method names. `[".push(", ".append(", ".extend(", ".extend_from_slice("]`
  is one method away from evasion (`insert`, `resize`, `push_within_capacity`, `splice`,
  `swap_remove`+re-add) — r4 f2, b4 §2.
- **MUST NOT** root the receiver-chain walk at a fixed identifier set (`FREE_FRAMES`,
  `free_list`). Binding the guard to *any other name* defeats it: the chain from `FREE_FRAMES`
  terminates at the allowed `.try_lock(`, and the insertion is spelled on an untracked name —
  r5 f1, **the single confirmed still-open evasion at branch tip**.
- **MUST NOT** scope the scan to `frame_allocator.rs` only. `frame_allocator_tests.rs` is
  `#[path]`-included as `mod boot_tests` with `use super::*` and has full access to the private
  `FREE_FRAMES` (r1 f6, b1 §4).
- **MUST NOT** use count equality ("there are still N pushes") in place of span membership — the
  C6 evasion the design names explicitly.
- **MUST NOT** run `match_indices` over raw file text. Use the comment/string-aware mask, or a
  `.push(` in a comment or string false-fires (r2 F4 tail).

**Required right shape (spelling-independent, alias-closed):**

1. Compute the code mask (comment/string-aware, byte-level) for each scanned file — re-derive
   `code_mask` + `code_offsets` + `code_sites` from `branch@4d759a62`, and keep its self-test
   (`code_mask_reports_only_real_code_occurrences`).
2. Build the alias set by **transitive closure**, not a fixed identifier list: seed
   `{FREE_FRAMES}`; for every `let`/`if let`/`let … else`/`match` binding whose initializer span
   contains any member of the set (including through `.lock()`, `.try_lock()`, `&mut *x`,
   `Some(mut x)` patterns), add the bound name.
3. Require every **mutating** use of any alias — i.e. every method call whose name is **not** in a
   short read-only allow-list (`len`, `capacity`, `iter`, `is_empty`, `pop`, `lock`, `try_lock`,
   plus the separately-allowlisted boot-test helpers) — and every bare occurrence of the
   `FREE_FRAMES` identifier, to lie inside an allowlisted function span. Allowlist by **span**, not
   by method: `init_frame_ledger`, `allocate_candidate` (pop only), `return_lease`, `memory_stats`,
   and the explicitly capability-allowlisted boot-only fixture helpers (`dev` 15).
4. Ship a standing negative (`with_replaced_source` inside
   `deliberately_broken_variants_fail_the_ratchet`, `main:tests/teardown_structure.rs:1285`) for
   **each** of M-R1.1 … M-R1.7, each using a *syntactically different* forbidden form (§4's own
   requirement; r1 f7 caught a negative that was byte-identical to the production spelling).
5. If the closure is still not total, **say so precisely** in the deviation record — the r5 residual
   note disclosed helper-function and `DerefMut` paths but **not** that renaming the binding
   suffices, which is the disclosure defect (r5 f1 final paragraph).

---

## R7 — the drain stays minimal (§4/R7, C3)

**Property.** `function_body` of `return_lease` — **and of every helper it transitively calls on
every return**: `frame_ordinal`, `claim_frame`, `counted` — contains none of `log::`,
`serial_println!`, `format!`, `vec!`, `Vec::new`, `Vec::with_capacity`, `alloc::`; and no capacity
growth (MT-4).

**Mutations that must FAIL:**
- M-R7.1: `log::warn!("refused")` inside `counted`'s refusal arm (this is the *single most tempting*
  place, and the r1 f5 shape that R7 originally missed).
- M-R7.2: `log::info!` inside `return_lease`.
- M-R7.3: a `reserve`/`try_reserve` call inside `return_lease`.

**MUST-NOT:** check only `return_lease`'s own body (r1 f5); treat the `debug_assert!` as the proof
(compiled out of release — r5 f8). `deallocate_frame` keeps its pre-existing logging and stays
**outside** the span, deliberately (§1.2).

---

## R9 — the ledger exists before the first process root (§4/R9)

**Property (three clauses, all required):**
1. `init_frame_ledger()` call sites are **exactly** `{memory/mod.rs:<post-heap, pre-slab>,
   main_aarch64.rs:<after init_aarch64_heap>}` via `assert_exact`.
2. **Per-arch ordering**: on x86, the ledger call precedes `slab::init()`; on aarch64 it follows
   `init_aarch64_heap()` and precedes `kernel_stack::init()` and `process::init()`; and on **both**
   arches no `ProcessPageTable::new(` occurrence precedes the ledger call in the boot function.
3. A **whole-tree inventory** of `ProcessPageTable::new(` construction sites, pinned by
   `assert_exact`, so a new constructor anywhere is a red.

**Mutations that must FAIL:**
- M-R9.1: move `init_frame_ledger()` below `slab::init()` (x86) / below `kernel_stack::init()` (ARM).
- M-R9.2: insert `let _p = ProcessPageTable::new();` **before** the ledger call in
  `main_aarch64.rs`. This must fail via the **ordering** validator, called **directly** by the
  negative.
- M-R9.3 (recorded FAIL in `oe` §R9): inside `manager.rs::fork_process_with_context`, before the
  existing child page-table constructor, add
  `let _shadow_page_table = crate::memory::process_memory::ProcessPageTable::new().expect("root alloc");`
  → must produce `R9 frame-ledger initialization moved`.

**MUST-NOT shapes:**
- **MUST NOT** use the whole-line predicate
  `line.contains("ProcessPageTable::new(") && !line.contains('"')`. It exists to skip five log
  strings and silently drops any **real** call site whose line carries an inline `.expect("…")` or
  `map_err(|e| …"…")` — r4 f3, b4 §3, demonstrated in `oe`. Use the code mask instead, so a
  constructor on a quoted line is counted and the log strings are excluded lexically.
- **MUST NOT** let the negative pass through a *different* validator. r3 f9 / r4 blocker 9: the ARM
  negative "passed" only because `PROCESS_PAGE_TABLE_CONSTRUCTORS`'s `assert_exact` caught the new
  call site, while the ordering clause it was named for did not exist. **Every negative must invoke
  the validator it is named for, directly.**
- **MUST NOT** inflate the pinned constructor constant to absorb string text — the inventory must
  stay at its true count (13 constructors / 2 ledger-init calls on the branch) when the mask is
  introduced. A constant that moves when you change the *lexer* means the lexer is wrong.
- **Known residual to disclose, not hide:** the ordering clause is a textual scan of the boot
  function's own body, so a constructor reached **indirectly** (from something
  `init_aarch64_heap()` calls) is invisible to it (r2 F9).

---

## O2 — the standing injection gate (§3.3 A–E)

**Global properties.** Every sub-case asserts the counter that must fire **and** the counters that
must not, then **restores clean state and asserts a healthy operation still succeeds** (so no
counter latches and no refusal is sticky). A and C originally omitted the restore (r1 f16, b1 §10)
— `healthy_round_trip()` after every arm. Sub-cases drive the **real production primitives**; a
simulated flag is not an oracle.

The end-of-gate aggregate delta must be exact for the five refusal counters and a **floor** for
contention: `double +1, stale +1, never +1, untracked +1, duplicate +3, contended ≥ +1`. Including
`FRAME_LOST_CONTENDED` in a per-operation equality is a flake and was one (r3 f12, b3 §12) —
`healthy_round_trip` compares `counters()[..5]` only.

### O2/A — double release

**Property:** first `return_lease` → `Returned`; second → `RefusedDoubleRelease`; the frame appears
**exactly once** in `FREE_FRAMES`; a healthy round trip still succeeds.
**Mutation that must FAIL:** make the `ST_FREE` arm of `return_lease` push anyway (drop the
`RefusedDoubleRelease` return).
**MUST-NOT:** assert only `FREE_FRAMES.lock().len() == free_before + 1`. §3.3-A's stated property is
*appears exactly once*; use `free_frame_count(frame) == 1` (r3 f18). Keep **both** — length catches
a second insertion of a different frame.

### O2/B — stale authority

**Property:** a lease copied before the frame was returned **and re-claimed** is refused
`RefusedStale`, and the current owner's slot is untouched (state `ST_ALLOCATED`, generation ==
current).
**Mutation that must FAIL:** `if observed >> 2 != lease.generation && false {` in `return_lease`
(the r5 f9 comparison confirmed to turn the suite red — this is the *correct* behaviour for a
generation pin, and it is the sibling that proves the serial-join pin is broken).
**MUST-NOT (the r4 f9 evasion):** pin the fixture with substring presence
(`stale.contains("return_lease(stale)")`, `stale.contains("take_free_frame(stale.frame)")`).
Editing `take_free_frame` to return the frame *without* re-claiming it leaves the substrings intact
and B degenerates into testing nothing.
**Required right shape (the strongest fix of round 5 — re-derive it):** the fixture builds stale
authority through **real transitions** (`allocate_frame_leased` → `return_lease` → `take_free_frame`
→ `claim_frame`) **and proves it at runtime**:

```rust
if current.index != stale.index || current.generation == stale.generation { return None; }
```

with a `None` fixture failing the gate. Then a structural pin on top:
`function_body("take_free_frame")` must contain `claim_frame(candidate)` and must **not** contain a
`FrameLease {` literal. Four standing negatives (synthesised lease, removed self-check,
`if false` generation test, mis-mapped counter).
**Known wart to fix, not copy:** the fixture's `LostContended` recovery branch calls
`claim_frame(stale.frame)` on a still-`ST_ALLOCATED` frame, which takes the duplicate branch,
pollutes `FRAME_DUPLICATE_ALLOC_REFUSED`, and produces a misleading `B: exact frame reuse failed`
plus a broken `duplicate +3` aggregate (r5 f12). Handle contention by retrying the fixture, not by
a fallback that corrupts counters.

### O2/C — untracked vs never-allocated

**Property:** an out-of-region frame and an in-region-never-handed-out frame are **distinct**
refusals (`RefusedUntracked` vs `RefusedNeverAllocated`); nothing enters `FREE_FRAMES` in either
case; and the untracked arm exercises **`frame_ordinal`'s region-bounds logic** in the failing
direction.
**Mutation that must FAIL (recorded in `oe`, ARM boot):** replace the trailing `None` in
`frame_ordinal` with `Some(0)` → the boot suite must go red with
`C: untracked frame return was not isolated` (and the late healthy-counter guard red too).
**MUST-NOT (r4 f4, b4 §4):** hand-forge the lease —
`FrameLease { frame: …PhysAddr::new(0), index: u32::MAX, generation: 0 }` + `return_lease(untracked)`
proves only that `ledger.get(out_of_range)` returns `None`, leaving the production producer
(`deallocate_frame`'s `frame_ordinal(frame)` lookup) completely unoracled.
**Required right shape:** derive the fixture frame from the **live memory map** —
`MEMORY_INFO.get()` → max `region.end` → round up to a 4 KiB boundary — and drive **production
`deallocate_frame(untracked)`**, then assert `untracked +1` and an unchanged free-list length after
unconditionally scrubbing the frame. Structural pins: the gate must contain
`above_top_of_ram_frame()` and `deallocate_frame(untracked)`, must contain **no `FrameLease {`
literal**, and the helper must contain `MEMORY_INFO.get()` / `region.end` / `.max()`; plus a
`hand_forged_untracked` standing negative that restores the pre-r5 shape and requires `Err`.

### O2/D — duplicate at allocation

**Property:** three injected duplicates of a **live** frame are each withheld and counted
(`FRAME_DUPLICATE_ALLOC_REFUSED += 3`); the live owner's ledger entry is unchanged **read
directly**, not inferred; no OOM is reported to the caller.
**Mutation that must FAIL:** make `claim_frame`'s `ST_ALLOCATED` arm return `Ok(None)` instead of
`Err(())` — the duplicate escapes to a second owner.
**MUST-NOT:**
- **MUST NOT** infer "the live owner's slot survived" from a later successful `return_lease(live)`;
  read the slot the way B does (r2 F18, r1 f16).
- **MUST NOT** leave injected corruption behind on any exit path. `remove_duplicate_candidates(live.frame)`
  must run **unconditionally, immediately after the single allocation attempt and before any failure
  is evaluated**; owner restoration must cover all result shapes; `restore_lease` must republish an
  already-committed `ST_FREE` frame if the `try_lock` races (r3 f11, b3 §11). A failing boot test
  that leaves real duplicates in the production free list produces a later memory-corruption crash
  that masks the original failure.
- **MUST NOT** keep the dead defensive arm
  `Some(replacement) if replacement.frame == live.frame => restore_lease(replacement)`; it is
  unreachable, and if it *ever* fired it would return only `replacement` and leak `live` — the frame
  it was written to protect (r4 f10). Delete it or restore both.
- C22's "never as OOM" half is **not** delivered in PR-1a (no `MapError::AllocatorCorrupt`); that is
  a stated deviation (`dev` 4), not something to paper over. Do **not** reintroduce a dormant
  `ALLOCATOR_CORRUPT` static with no production reader — that is PLAN.md law 2 and it was correctly
  removed (r2 F6, b2 §6).

### O2/E — contention is loss, never corruption

**Property:** holding the **real** `FREE_FRAMES` lock on this CPU across a **real** `return_lease`
produces `FRAME_LOST_CONTENDED` (floor, `≥ +1`), the ledger is left `ST_FREE` with the frame off the
list, and the test repairs it rather than leaving the pool corrupted.
**MUST-NOT:** assert an equality on the contention counter anywhere (r3 f12); leave the frame
unrepaired. Note the interaction with MT-3: E deliberately holds the lock, so a capacity-growth path
that fails on `try_lock` will manufacture OOM **inside your own gate**.

### O2 registration — one execution path per architecture

**Property:** the gate function has **exactly one** dispatch path per arch: the aarch64 `TestDef`
(`arch: Arch::Aarch64`, `stage: TestStage::SerialBoot`) and the x86 direct hook from `memory::init`.
They are architecture-**exclusive**, so fixing x86 staged dispatch (#533) cannot double-run it.
**Why it matters:** a second execution sees `start[..5] = [1,1,1,1,3]` and fails
`A-E: unexpected refusal preceded injection gate`, and the late healthy-counters check sees
`[2,2,2,2,6]` — a deterministic self-inflicted failure armed for whoever lands #533 (r3 f1, b3 §1).
**Mutations that must FAIL:**
- M-REG.1: rewrite the `TestDef`'s `arch: Arch::Aarch64` to `Arch::Any`.
- M-REG.2 (**r5 f2, b5 §2 — CONFIRMED still green at branch tip**): add
  `use crate::memory::frame_allocator::frame_custody_refusal_gate_test as aliased_frame_custody_gate;`
  before `PROCESS_TESTS`, plus a second `TestDef { name: "frame_custody_refusal_gate_x86",
  func: aliased_frame_custody_gate, arch: Arch::Any, stage: TestStage::EarlyBoot }`.
- M-REG.3: a second `TestDef` with a **different name** but the same fully-qualified `func:` path
  (r4 f11, b4 §8).

**MUST-NOT shapes:**
- **MUST NOT** key the pin on the display **name** (`registry.split("name: \"frame_custody_refusal_gate\"").nth(1)`),
  which matches only the *first* `TestDef` with that literal name — r4 f11.
- **MUST NOT** key the pin on one literal spelling of the `func:` path. Counting
  `code_offsets(registry, mask, "func: crate::memory::frame_allocator::frame_custody_refusal_gate_test") == 1`
  counts a spelling, not a resolved function — r5 f2.
**Required right shape:** count code occurrences of the **last path segment identifier**
(`frame_custody_refusal_gate_test`) across the registry; require exactly one enclosing `TestDef`
(brace-matched) and check its `name`, `arch` and `stage`; **and** forbid any
`use …frame_custody_refusal_gate_test as` alias in the file. Separately pin exactly one call site in
`memory/mod.rs` under `cfg(target_arch = "x86_64")`.

---

## The boot-allocation-shape pin (guards MT-1)

**Property:** `init_frame_ledger` contains **no** `(0..total_frames)` loop and **no**
`reserve_exact(total_frames)`; it **does** contain `try_reserve_exact(chunk_count)`; and chunk +
capacity preparation appears **before** the `NEXT_FREE_FRAME.compare_exchange` in the sequential
allocation path (span-ordered).
**Mutations that must FAIL:** restore `FREE_FRAMES.lock().reserve_exact(total_frames);`; restore the
flat `(0..total_frames).map(…).collect()`; move `prepare_frame_for_allocation(current)` to *after*
the CAS.
**MUST-NOT:** rely on a boot run as the proof. The x86 harness runs QEMU at `-m 512`
(`branch:docker/qemu/run-x86-boot-tests.sh:38`), which advertises ~131 k frames — **a size that
would not have panicked before the fix either**. Nothing in the branch tests the large-RAM regime;
the disposition rests on the code analysis, and the record must say so (r5 f10).

---

## The x86 harness pin

**Property:** the x86 script asserts **exactly one** `[TEST:process:frame_custody_refusal_gate:PASS]`
marker **and** the exact counter vector
`\[FRAME_CUSTODY_COUNTERS:x86:double=1:stale=1:never=1:untracked=1:duplicate=3:contended=[1-9][0-9]*\]`,
and aborts on `[BOOT_TESTS:FAIL|KERNEL PANIC|panic!`.
**Mutation that must FAIL:** rewrite the marker count test `-eq 1` to `-ge 0`.
**MUST-NOT:** gate on `[BOOT_TESTS:PASS]`. The x86 build emits it unconditionally from
`advance_stage_marker_only` alongside `[TESTS_COMPLETE:0/0]` — a vacuous gate (r3 f14, b3 §14). The
script header must state why it does not use that marker.

---

## Counter inventory (R6-lite, §3.1)

**Property:** six counters declared with `counter!`, all present in `COUNTERS`
(`main:kernel/src/tracing/providers/teardown.rs:449`), `COUNTER_COUNT` 47 → **53**
(`main:…:444`), exposed by `snapshot()` (`:510`), **none** carrying `cfg(target_arch)` or
`cfg(feature)` (C13/C19), each with a real production producer and a same-PR injection arm.
**Mutations that must FAIL:** delete one `COUNTERS` entry (declarations/readers equality); add a
counter with no producer (the `all_phase_zero_counters_have_registered_readers_and_honest_runtime_gates`
test).
**Disclosure obligation:** 6/53 is a departure from the ratified 5/52 and shifts 1b/1c to 59/64 —
deviation record, not silence (MT-5c).

---

# 3. HARNESS TRAPS

## HT-1 — The racy-O2 gate class, and the only acceptable fix shape

**The trap (r4 f1, b4 §1 — the most serious finding of the campaign).**
`run_staged_tests` spawns **one kthread per subsystem** for the same stage and joins them all
afterwards (`main:kernel/src/test_framework/executor.rs:190`, header comment at `:1`). O2 was a
PROCESS-subsystem `EarlyBoot` test; the MEMORY subsystem has an `Arch::Any` `EarlyBoot` test
literally named `frame_allocator` (`main:registry.rs:4808`) which allocates three frames and then
`deallocate_frame`s them, plus `heap_large_alloc`/`heap_many_small` — **same stage, another kthread,
same global `FREE_FRAMES`.**

Two concrete failures on a completely healthy kernel:
- **A:** MEMORY's `deallocate_frame` lands between the gate's `let free_before = …len()` and its
  `len() != free_before + 1` → `A: double return or recovery was not exact`.
- **B (worse — it defeats the injection):** MEMORY pushes its three freed frames between
  `inject_duplicate_candidates(live.frame, 3)` and the allocation. `FREE_FRAMES` is a **LIFO
  `Vec`**, so the first pop returns a *clean* frame, `claim_frame` succeeds immediately,
  `duplicate` advances by 0, `remove_duplicate_candidates` silently discards the injection, and the
  gate fails `D: duplicate live frame escaped allocation` — two red tests from one benign
  interleaving.

**The required fix shape (deterministic, at the scheduling boundary — `branch@9287c09d`, re-derive):**

1. Add `TestStage::SerialBoot = 0` **ahead of** `EarlyBoot` (`main:registry.rs:72-91`), documented
   as "must run alone before the parallel cohort; reserved for tests that deliberately mutate shared
   global state and restore it exactly."
2. Register the aarch64 gate at `Arch::Aarch64` / `SerialBoot`.
3. `run_all_tests` runs **and joins** `run_staged_tests(SerialBoot)` **before** printing
   `[STAGE:early:ADVANCE]` and storing `CURRENT_STAGE = EarlyBoot`
   (`main:executor.rs:122-190` is the function to edit).
4. Inside `run_staged_tests`' spawn loop, **join each SerialBoot subsystem immediately** rather than
   pushing its handle, so the stage stays serial if a second exclusive test is ever added.
5. Renumber coherently and completely: `TestStage::from_u8` (`main:registry.rs:104`),
   `TestStage::COUNT` 4 → 5 (`:91`), **both** `progress.rs` arrays
   (`main:progress.rs:27,:29,:82,:145-148`), and `display.rs`'s `STAGE_COLORS`
   (`main:display.rs:267`). Then grep `docker/`, `scripts/`, `xtask/` to confirm **no consumer parses
   stage integers or the `EARLY_BOOT` count** (r5 verified this — repeat it, don't assume).
6. Keep **every** exact assertion. No sleep, no retry-until-green, no counter floor replacing an
   equality, no relaxed free-list equality, no IRQ masking.
7. Standing negatives in **both** directions: gate moved back to `EarlyBoot`; serial-join condition
   flipped.

**The serial-join pin must not be evadable (r5 f9, b5 §7 — CONFIRMED open).**
Pinning `run_staged.contains("if target_stage == TestStage::SerialBoot")` — **no trailing brace** —
is defeated by mutating the source to `if target_stage == TestStage::SerialBoot && false {`, which
compiles and leaves the suite green. Pin the condition **through its opening brace**
(`"if target_stage == TestStage::SerialBoot {"`, as its sibling generation pin does), and
additionally require that the true-arm block contains `join_test_thread(` and the else-arm contains
`handles.push(`.

**Residual you must disclose (r5 f13):** serialisation removes the *identified* concurrent producer;
the assertions are still exact equalities over a global LIFO and remain valid only while nothing else
in the kernel allocates or frees during the gate. No such producer exists at that point in boot
today (display init runs before the stage, `render_progress` after). Say that; do not claim the
dependency is gone.

## HT-2 — Vacuous gates

Every one of these shipped and was caught:

| Pattern | Where it appeared | Rule |
|---|---|---|
| Gating CI on a marker the build emits unconditionally (`[BOOT_TESTS:PASS]` with `[TESTS_COMPLETE:0/0]`) | r3 f14 | Gate on the specific evidence, and state in the script header what is *not* being claimed |
| A structural pin that is a **prefix** of a mutable expression (`&& false` evasion) | r5 f9 | Pin through the closing token; add a sibling positive/negative pair |
| A standing negative that goes red via a **different** validator | r3 f9, r4 blocker 9 | Every negative calls its named validator directly |
| A pinned **count** where the property is **membership** | design C6, r4 f2 | Span membership, always |
| A pin keyed on a **display name** or one **literal path spelling** | r4 f11, r5 f2 | Key on the resolved item; forbid aliasing |
| Byte-identical build logs offered as two-architecture evidence | r4 f14 | Log the command line, target and feature set with each artifact |
| A placeholder string where a measurement belongs (`BEAST_COUNTERS_PLACEHOLDER`) | r5 f10 | Transmit the real vector or say it was not obtained |

## HT-3 — Unfailable tests (CLAUDE.md testing-integrity rule)

- `if ticks >= 0 { Pass }` on an **unsigned** value — vacuous (r3 f15).
- Replacing it with an unconditional `TestResult::Pass` — worse (r3 f15, b3 §15). The correct fix is
  to delegate to the real `test_timer_ticks()` (`main:registry.rs:1198`), which fails on
  `timestamp did not advance on x86_64`, and pin that with a standing negative that replaces it with
  `TestResult::Pass`. Note the forward cost: when #533 lands, `test_timer_ticks` is registered in its
  own right and the 5 M-iteration spin runs twice per boot (r4 f12) — name it, don't discover it.
- A fixture that can silently degenerate. The runtime self-check in O2/B is the model: a fixture that
  cannot construct genuine authority must return `None` and **fail** the gate, not pass vacuously.

## HT-4 — Test code with production side effects

- A gate that reads `NEXT_FREE_FRAME` and then frees the frame at that index races a concurrent
  allocation and **actually frees a live frame** — memory corruption, not a red test (r2 F17). Derive
  such fixtures from the memory map (`above_top_of_ram_frame`) or re-check the frontier with a CAS.
- Injected duplicates left in the production free list on any failure path corrupt the rest of the
  boot (r3 f11). Clean up unconditionally, before evaluating any failure.
- The x86 gate ends in `assert!(result.is_pass(), …)` executed from `memory::init`, so a pre-existing
  production refusal panics the kernel during memory initialisation rather than being counted — a
  defensible fail-loud choice that is **harsher than design residual 6 describes** ("the discovery is
  a counter, not a crash"). State it (r4 f15).

---

# 4. HONESTY CONTRACT

## 4.1 What the deviation record must contain

One durable artifact, and it must be **copied into the PR body** (a PR must exist — `gh pr list`
returned `[]` through r4 and r5, which is itself a blocking finding: b4 §6, b5 §4).

Required properties:

1. **Live deviations only.** Every entry describes a difference that exists in the tree at the head
   you are shipping. No historical narrative, no "we originally measured".
2. **Every entry cites its design section**, states the implementation difference, and gives a
   justification that stands **on its own merits**. Format (from `dev`, which is the right shape):
   *Design section* / *Difference* / *Justification*.
3. **No dead premises.** The 2026-08-11 ruling withdrew the size law, so any justification of the
   form "…and would exceed the 230-line seam" is void and must be restated without it (r4 f7).
4. **Stamped at the final SHA**, with the diff inventory regenerated at that SHA.
5. **Explicitly retires the claims it supersedes**, correctly (see 4.2).
6. **Names, at minimum, these deviations** (all established across r1–r5; `dev` 1–17 is the working
   set): chunked demand-backed ledger; free-list capacity following the high-water mark; the `u32`
   cap as a ceiling rather than an assertion; C22's missing `MapError::AllocatorCorrupt` channel
   ("this is not exact C22 conformance"); 6 counters / 53 total vs the ratified 5 / 52 with the
   59/64 downstream shift; the stale outcome's fixture-plus-narrow-race status; `allocate_frame()`
   also routing through the claim guard; the leased API staying private and `boot_tests`-only;
   `deallocate_frame` being intentionally fail-closed (**not** "behaviour unchanged"); pre-ledger
   bootstrap retaining legacy return behaviour; the ≤128-region ordinal lookup cost replacing §1.2's
   "1–4 iterations"; O2 serialized on aarch64 / directly hooked on x86 with #533 named; the x86
   counter line being an **injection checkpoint**, not a post-cohort read; O2/E asserting allocator
   contention rather than PR-1c retirement accounting; boot-only fixture authority allowlisted
   separately from production return authority; and the two x86-support drive-bys
   (`test_timer_init` → `test_timer_ticks`, `display.rs` cfg-gating) named as
   "warning fixes newly exposed by the x86 `boot_tests` build" (r4 f13).
7. **A plain statement of what #470 still owes** (§7's acceptance record requires it).

## 4.2 Stale-claim failures to avoid — each one was a blocking finding

| Failure | Round | Rule |
|---|---|---|
| The record lives only in a scratchpad; no PR carries it | r4 f6, r5 f4 | Open the PR; paste the record into the body |
| Record stamped at a **superseded SHA**, with line counts that no longer match the branch | r5 f4 | Re-stamp and regenerate at the head you ship |
| A notes file stale at an older head, asserting a test that does not exist and contradicting its sibling notes | r2 F3 | One record. Mark superseded documents as superseded, explicitly |
| "No dormant allocator-corruption latch" presented as a **delivered feature** while C22's error channel is missing | r4 f6 | State the unmet clause, not the adjacent achievement |
| Residual-6 stated as a cross-arch result when it is aarch64-only | r1 f15, r2 F2, r3 f13 | "No refusal fired during the aarch64 boot-test window"; the x86 line is an injection checkpoint taken at the end of `memory::init`, before any process has existed, so it measures nothing about post-cohort leakage |
| "The ~21 GiB ceiling is gone" when it **moved** from boot panic to runtime allocation failure | r5 f3, b5 §3 | Runtime cost is still ~12 B of heap per **exposed** frame (4 B ledger slot, realised per chunk, plus 8 B of never-released `FREE_FRAMES` capacity per published frame) ≈ the same ~21 GiB of *allocatable* RAM against a 64 MiB heap — a genuinely better failure mode (`allocate_frame() → None`), which is what to say |
| Mutation evidence attributed to a baseline that **predates** the code it exercises | r5 f11 | Record the SHA the run actually used, and whether the tree had uncommitted changes |
| Two byte-identical `Finished release profile in 0.06s` logs offered as x86 + ARM build evidence | r4 f14 | Capture the command line, target and features; per `MEMORY.md`, all x86 builds run on beast |
| A placeholder where a counter vector belongs | r5 f10 | Transmit the measurement or state that it was not obtained |
| Claiming the harness exercised a regime it cannot reach (`-m 512` vs the large-RAM path) | r5 f10 | Say the disposition rests on code analysis, and that the boot green is corroboration |

## 4.3 The acceptance record (§7, mandatory per PR)

Both aarch64 builds warning-free and error-free; `cargo test --test teardown_structure` green
**including every standing negative**; one `docker/qemu/run-aarch64-full-test.sh --boot-tests-only
--rebuild` reaching `[BOOT_TESTS:PASS]` at the **final** head; `clean-gate.sh` 0/100 and
`starved-gate.sh` 0/100 under 14 host hogs (both scripts are already in this scratchpad directory);
**beast x86 build + boot 3/3 + kthread 3/3**; **Parallels 3× consecutive long-window green**, each
from a force-stopped VM with a truncated serial log (#525: Parallels gates kernel-path merges, and
PR-1a is a kernel-path change on both arches); `git diff --numstat` quoted in the body (as a fact,
not as a gate); revert dry run; all QEMU processes killed and `pgrep` empty before reporting.

---

# 5. SALVAGE MAP — `fix/470-pr1a-frame-ledger` (reference only; nothing cherry-picks)

Merge base `363eb912`. Read a hunk, understand the property, **re-derive it**; do not `git
cherry-pick`, because every commit after the first carries repair scar tissue for the commit before
it.

| Commit | Contents | Verdict |
|---|---|---|
| `13d3fa29` *memory: add generation-checked frame ledger* (4 files, +193/−17) | ledger statics, ordinal map, seeding, `FrameLease`/`ReturnOutcome`, `return_lease`, duplicate-at-allocation, `deallocate_frame` routing, 5 counters | **MIXED.** The **CAS/generation state machine, seeding order and duplicate-withhold are the sound core** — re-derive. **Embodies MT-1** (flat whole-RAM ledger), **MT-5b** (ledger-conditional low-memory floor), **MT-5c** (`ST_NEVER` folded into `RefusedUntracked`), and ships `ALLOCATOR_CORRUPT` as dormant code |
| `b0c689e4` *test: ratchet frame ledger authority* (+150 test lines, +135 ratchet) | first O2 gate, first R1/R7/R9 | **MOSTLY TRAP.** R1 is a 3-spelling blacklist (r2 F4, evasion demonstrated); R7's span excludes the helpers (r1 f5); R1 does not scan `frame_allocator_tests.rs` (r1 f6); the one negative is byte-identical to the production spelling (r1 f7); O2/A and /C never restore to healthy (r1 f16). Salvage only the **structure** (a gate driving real primitives + standing negatives inside `deliberately_broken_variants_fail_the_ratchet`) |
| `bd8b13e7` *address frame ledger review findings* | r1 fixes | **MIXED, contains the two worst regressions.** Sound: restored the unconditional floor `return`; extended R7 to the helpers; `healthy_round_trip` for A/C. **Traps: deleted `FRAME_RETURN_REFUSED_STALE` on a false single-threaded premise (MT-5d), and added `FREE_FRAMES.reserve_exact(total_frames)` — the 8 B/frame half of MT-1** |
| `bc8686ee` *close frame ledger review r2 findings* | r2 fixes | **MOSTLY SOUND — the best commit to learn from.** Restored the stale counter; added `RefusedNeverAllocated` with a real production producer (MT-5c); replaced R1's blacklist with span containment; removed `ALLOCATOR_CORRUPT` entirely (PLAN.md law 2); added the x86 harness script; `display.rs` cfg-gating. Residual traps: R1 still function-granular (r3 f5); R9's ARM branch still missing its ordering clause (r3 f9) |
| `e8829c20` *close r3 blockers* | r3 fixes | **MOSTLY SOUND.** Arch-exclusive gate registration; R1's push-containment clause; the **stale fixture built through real transitions** (re-derive); `test_timer_ticks` replacing the vacuous timer test; the non-vacuous x86 script; `healthy_round_trip` scoped to `[..5]`; unconditional duplicate cleanup before failure evaluation. Residual traps: **the racy O2 gate is still there** (HT-1), push-containment is a 4-name list (r4 f2), R9's inventory drops quoted lines (r4 f3), O2/C hand-forges the lease (r4 f4), the registration pin is name-keyed (r4 f11) |
| `9287c09d` *make frame ledger boot and gate deterministic* (7 files) | the r5 kernel fix | **SOUND IN CONCEPT, THREE NEW TRAPS.** Re-derive: the **chunked demand-backed ledger** (MT-1), the **`SerialBoot` stage + immediate join** (HT-1), the `u32` cap-not-assert change, the boot-allocation-shape ratchet. Do **not** copy: `ensure_free_frame_capacity`'s `try_lock`-failure → OOM (**MT-3**), `ensure_chunk`'s allocation inside `spin::Once::try_call_once` (**MT-5h**), the bootstrap `.expect("bootstrap free frame missing ledger chunk")` (**MT-5g**) |
| `4d759a62` *harden oracles against lexical evasions* (+360 ratchet) | the r5 oracle fix | **SOUND — re-derive most of it.** The byte-level `code_mask` lexer + `code_offsets`/`code_sites` + its self-test; `braced_block`/`enclosing_test_def`; the `above_top_of_ram_frame` O2/C fixture driving real `deallocate_frame`; the **runtime identity+generation self-check** in `stale_lease_fixture`; the production-unreachability disclosure moved into a pinned production comment |
| `dddf98fd` *close lexical trivia evasion in free-list ratchet* (+22/−11) | receiver-chain lexer | **PARTIAL TRAP.** Closes `free_list /* trivia */ .insert(…)` and `&mut *free_list` re-borrows, but the walk is rooted at the fixed identifiers `{FREE_FRAMES, free_list}` and the containment loop knows four method names — **the confirmed-open R1 alias evasion (r5 f1)**. Replace with the transitive alias closure in §2/R1 |
| `7d9a798a` *close round-5 oracle ratchet residuals* (+59 ratchet) | registration + serial-join pins | **PARTIAL TRAP.** The `TestDef` brace-matching + arch/stage check is sound; the **occurrence count keyed on one literal `func:` spelling is evaded by `use … as`** (r5 f2), and the **serial-join pin lacks its trailing brace and is evaded by `&& false`** (r5 f9). Re-derive per §2 "O2 registration" and §3/HT-1 |

**Also on the branch, worth taking:** `docker/qemu/run-x86-boot-tests.sh` (79 lines, no equivalent
on main — `git ls-tree main docker/qemu/` has no x86 boot-test script) in its `e8829c20` form: exact
counter-vector regex, exactly-one PASS marker, abort on `FAIL|KERNEL PANIC|panic!`, and a header
stating that its `[BOOT_TESTS:PASS]` is marker-only and is deliberately not treated as evidence.

**Do not take:** `impl-1a-notes.md` in any form (superseded and materially false — r2 F3, r4 f6);
any evidence artifact stamped at a SHA other than the one you ship.

---

# 6. One-pass landing checklist

1. Branch fresh from `main` (`363eb912`+). Never push to main; feature branch + PR.
2. Kernel: chunked ledger (MT-1) with the MT-3 / MT-5g / MT-5h corrections; seeding order + the
   quiescence assert (MT-5a); unconditional floor (MT-5b); six-outcome taxonomy (MT-5c); stale
   counter retained (MT-5d); alignment asserts (MT-5e); countable candidate drops (MT-5i); the
   MT-5j properties preserved verbatim.
3. Init wiring at the two anchors in MT-2, with the per-arch ordering clauses written separately.
4. Counters: 6 declarations, `COUNTERS`, `COUNTER_COUNT` 47→53, `snapshot()`, no `cfg` on any.
5. Harness: `SerialBoot` + immediate join + complete renumbering (HT-1), arch-exclusive gate
   registration, the x86 direct hook, the x86 script.
6. O2/A–E per §2, every arm restoring to healthy, cleanup unconditional, fixtures self-checking.
7. Ratchets R1/R7/R9 + the four pins per §2, each with its standing negative(s) calling its own
   validator directly.
8. Run **every** mutation in §2 on the real tree; record verbatim FAIL and the restored PASS; leave
   `git status --porcelain` empty.
9. Acceptance record per §4.3, at the final SHA.
10. Deviation record per §4.1, stamped at that SHA, pasted into the PR body.
