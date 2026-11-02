# Early GR control-stream refusal (#2669)

Investigation: `rel-u1-gr-refusal-2669`, 2026-11-02. Base: main `fb85908d`.
Raw captures, register values and the line-by-line comparison remain in private
boot evidence. This document distinguishes a host-reproduced idle-wait defect from
an explanation of the intermittent hardware refusal, which is not yet proven.

## Capture comparison

| Production console boot | Head | GR outcome | Before-to-after capture interval |
|---|---|---|---|
| `20261102T082927Z-metal` | `24ddde6a` | Complete | 11.876 s |
| `20261102T093446Z-metal` | `992e1d65` | Complete | 12.131 s |
| `20261102T102648Z-metal` | `d8e9b26f` | Control stream not consumed | 7.567 s |
| `20261102T110818Z-metal` | `4bbaf847` | Complete | 11.873 s |

These intervals use `serial.ts` capture arrival times, not GPU execution timers.
The GR source is unchanged between `992e1d65` and `d8e9b26f`.

Line-by-line through early startup:

1. The 4/4 captures have the same `before` phase at serial line 640, the
   topology at 641 and the first two save-layout records at 642–643. Those
   two layout lines are byte-identical across 4/4 boots. Initial PBDMA
   pointer/reference words vary on both successful and refusing boots; their
   variation alone does not distinguish the failure.
2. The three clean captures proceed to shader evidence at 644–645, presentation,
   drawing and context-switch producers, then `channel-drained` at 651.
   The refusing capture instead restores IRQ state at 644 and prints
   `GT710_GR_REFUSAL GR control stream not consumed; backing retained` at 645.
   Its graphics, gr-contexts, draw and shaders producer records are absent.
3. In the actual producer, this refusal has one source: `Session::issue_words`
   waits up to two seconds for the first withheld-release stream or the later
   stale-release stream to be consumed. Consumption requires the expected
   current PBDMA channel, matching GP_GET and REFERENCE, and the exact channel
   instance binding. The old record does not identify which stream timed out.
   It is not a timeout in firmware upload or a mailbox wait.
4. Reaching both save-layout records establishes that firmware upload/readback,
   FECS/GPCCS readiness, the withheld-start/stale-mailbox controls and both
   context bind/save request pairs have already completed. The three clean
   `GT710_GR` records additionally retain identical verified firmware hashes,
   a 21 us ready handshake, 4–21 us bind/save replies and 12–14 us positive
   channel releases, inside the existing two-second bounds. The refusing
   boot has no terminal `GT710_GR` record, so its exact successful handshake
   times and hashes were not retained; they must not be reconstructed from a
   nearby boot.
5. The `after` snapshot is too late: teardown has unbound channels and gated
   PGRAPH. Its PRI refusal values cannot describe the stalled live engine.
   Cleared MMU/FIFO interrupt words after teardown do not rule out a pre-reset
   context-switch problem.
6. The 4/4 later clock-startup records report successful target clocks, the
   same initial temperature and a ready PMU. These records follow threading
   and GR teardown. No capture contains a simultaneous GR-refusal clock,
   thermal or PMU observation; subsequent health does not prove earlier health.

## Reference sequence and producer repair

Compared the complete relevant MIT nouveau v6.8 routines, without importing or
translating their implementation:

- [`gr/gf100.c`](https://github.com/torvalds/linux/blob/v6.8/drivers/gpu/drm/nouveau/nvkm/engine/gr/gf100.c):
  `gf100_gr_init`, `gf100_gr_init_ctxctl_int`, `gf100_gr_init_csdata`,
  `gf100_gr_wait_idle` and the ctxctl debug routines.
- [`fifo/gk104.c`](https://github.com/torvalds/linux/blob/v6.8/drivers/gpu/drm/nouveau/nvkm/engine/fifo/gk104.c):
  GK104 engine state and context-switch indication. GF100's older FIFO state
  layout is different and cannot be used for GK208.
- [`gf100_pfifo.xml`](https://github.com/envytools/envytools/blob/f102b823/rnndb/fifo/gf100_pfifo.xml):
  the GK104+ engine-state register stride; the existing pinned Falcon/ctxctl
  register definitions and source-built firmware protocol remain unchanged.

Breenix already uploads/verifies both Falcon images, extends the register lists,
starts FECS (which starts GPCCS), waits for both ready indications and validates
image size and fresh poisoned method replies. Its two-second startup bound
matches nouveau's internal firmware path. Starting both Falcons independently
as in the external NVIDIA firmware path would be the wrong protocol here.

A concrete deviation is in idle admission. Refreshing `0x400700` and finding
`0x40060c.BUSY` clear does not establish that the FIFO has finished switching
contexts. GK104's GR engine state at `0x2640` can still have BUSY and CHSW set.
Both Breenix's startup and golden-context idle waits omitted that condition;
the golden-context path also refreshed the status only once, before its loop.

The repair shares a bounded `ctxctl::wait_idle` between those two producers.
The polling loop refreshes engine state, requires GR execution idle and no active
FIFO context switch, and rejects PRI refusal words. The two-second bound stays
unchanged. Host register models reproduce the premature admission in both old
paths and refuse it after the repair; non-switching states still admit startup.

One additional reference difference is the MC `unk260` wrapper around firmware
upload. Its hardware semantics are not specified by the pinned PMC register
reference. This investigation does not introduce unexplained global PMC writes:
verified uploads and two completed saves place the observed failure later.
Its relevance remains unestablished, rather than a claimed equivalent sequence.

## Most likely cause and retained diagnostic

The most likely failure region is first runnable-channel scheduling/context
restore, after successful firmware communication and golden saves, rather than
firmware loading, thermal shutdown or the console scorer. Incomplete idle
admission is a host-reproduced sequencing defect and a plausible contributor, but
the historical capture cannot prove it caused this particular refusal. A clean
confirmation boot cannot by itself prove an intermittent defect eliminated.

At the unchanged control-stream deadline, the new diagnostic reads the listed state
before emitting serial output or returning to cleanup:

- Channel identity, epoch, sequence, nonce, elapsed/bound, USERD GET/PUT and the
  three coherent receipt words distinguish withheld from stale control and
  pending from completed command publication.
- FIFO engine/runlist/binding, current PBDMA channel, GP_GET/REFERENCE, interrupt
  and live method state distinguish scheduling, binding and command progress.
- GR busy/trap words, FECS/GPCCS CPU controls and mailbox/current-context words
  distinguish a stalled restore from a live engine that did not consume work.
- PMU CPU/queue controls, PGOB command ports, sensor controls and the read-only
  clock sampler preserve power, clock and temperature evidence at refusal.

The records are `GT710_GR_CONTROL_TIMEOUT`, `_FIFO`, `_FALCON`, `_POWER` and
`_CLOCK`. There is no reset, retry, register-selector write or receipt fallback
in this diagnostic. The original refusal and retention policy remain intact;
no suite, runner, scorer, expected value or acceptance bound changes.

## Checks and hardware scope

Check-first commit `a491b7dd` contains three failing checks: ctxctl startup
admitting an active context switch, golden generation doing the same, and a
control timeout returning without a retained capture. After the repair, the
ctxctl host binary passes 26 tests, the context host binary 14 and the timeout
check one. Host tests establish sequencing and capture invocation, not GPU
execution. The release build and final production-console measurement are
recorded in the unit ledger. The changed driver is x86-only; ARM code and ARM
boot results are unaffected by this diff.
