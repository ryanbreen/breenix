# bwm retained-allocation audit (E21 21.6 u4)

The unchanged `liveness/memory-bounded` case runs eight animating clients for
60 warmup epochs, samples bwm's VmRSS, runs through epoch 660, then requires
at most 64 KiB growth (`userspace/programs/src/wm_suite/cases.rs:1289`).
This audit concerns storage reachable across bwm loop iterations. Temporary
allocations and GPU resources are distinguished below; host allocation results
are not a measurement of guest VmRSS.

## Allocation inventory

References beginning with `bwm` are relative to `userspace/programs/src/`.
N is the maximum simultaneously discovered window count, K is the number of
unreleased device/key tuples, P is pending launches, and C is connected
clipboard peers. Vec capacity follows its high-water demand, rather than its
current length. These are workload-relative bounds, not an assertion that an
unlimited number of clients consumes constant memory.

| Retained allocation and source | Growth trigger | Bound and retirement |
|---|---|---|
| Window metadata, `bwm.rs:160`, `bwm/desk.rs:284` | One fixed-size `Window` per discovered client | O(N) capacity; no title allocation (inline 32 bytes); discovery removes departed windows at `desk.rs:204`. No append for an ordinary client commit. |
| Stack order, creation order, chromeless ids and minimized ids, `bwm/stack.rs:4` | Window creation/minimization | Four O(N) vectors; duplicate membership is refused, removal retires ids at `stack.rs:87`. Raises remove and insert at unchanged length. |
| Working frame layers, `bwm.rs:217`, `bwm/chrome.rs:445` | Visible client count | O(N) `Layer` capacity. Previously collected a replacement each presentation; now clears and extends the working vector. Eight layers occupy 640 capacity bytes on the tested host. |
| Saved validation layers, `bwm/chrome.rs:8`, `bwm/chrome/validation.rs:19`, `bwm.rs:653` | Successful presentation | One O(N) scene; a presentation updates that single scene. `PresentedScene::clone_from` reuses its layer vector; the working scene is returned to the loop for reuse. Invalidation drops the saved scene at `validation.rs:49`; a later reacquisition can allocate it again. |
| Validation first-frame requests, `bwm/chrome/validation.rs:35` | Newly discovered or resized client | At most one fixed-size request per unchecked live client, O(N) capacity. Retained against live unchecked windows at `bwm.rs:618`; success or exhausted retry removes it. Attempt flags reset on presentation without allocating. |
| Sampled-front metadata, `bwm/compositor.rs:117`, `bwm/compositor.rs:305` | Successful presentation | One snapshot's O(N) metadata. Previously collected a replacement each presentation; now clear/extend reuses capacity. Eight entries occupy 256 capacity bytes on the tested host. Startup clears it; reopen replaces the compositor. |
| Consumed hotkey keys, `bwm.rs:167`, `bwm.rs:325`, `bwm.rs:344` | A consumed press from a distinct device/key tuple | One BTreeSet entry per outstanding tuple, O(K), removed by the matching release. After ready input batches, registration pruning at `bwm.rs:577` also drops retired device ids even if reader pressure lost a release. The input core synthesizes held-key releases on removal (`kernel/src/input/stream.rs:170`). The animation workload creates no such entries. |
| Repeat keys, `bwm/timing.rs:10` | Non-consumed key press | One map row per held device/key tuple, O(K). Release, focus change or SYN_DROPPED clears relevant state (`timing.rs:15`, `timing.rs:31`). Due-repeat vectors are temporary. |
| Last clicks, `bwm/timing.rs:49` | Distinct device/button press | Previously one map entry per historical device/button. Now a reused vector with at most three button slots per registered pointer. `timing.rs:63` and `bwm.rs:576` retire removed device ids after ready input batches, using the shared capability query (`bwm/input.rs:228`). Capacity follows the high-water simultaneous pointer count, with retired slots reused. Source timestamps from different devices are not compared for retirement. No epoch term for a fixed input workload. |
| Active click counts, `bwm/timing.rs:49`, `bwm/timing.rs:62` | Button press | One map row per unreleased device/button, at most three per held pointer; release removes it at `timing.rs:67`. |
| Pointer device state, `bwm/pointer_route.rs:70`, `bwm/pointer_route.rs:169` | First pointer event or held button | One map row per device with held buttons/grab; idle rows are removed at `pointer_route.rs:226`, cancellation removes its device at `pointer_route.rs:141`. Hover and captured `Content` are inline copies, not allocations. |
| Absolute-axis capability cache, `bwm/input.rs:206`, `bwm/input.rs:246` | Cache miss by device id | At most 64 cache entries, each at most 512 `Capability` values. A full cache is cleared before insertion (`input.rs:258`); removed device ids therefore cannot accumulate indefinitely. Re-querying after eviction is intentional bounded-cache work, not epoch growth. |
| Input retry queues, `bwm/input_backlog.rs:26`, `bwm/input_backlog.rs:71` | Delivery backpressure | At most 1,024 events per registered target (`input_backlog.rs:4`, `input_backlog.rs:78`), plus one overflow count and one close request per target. Successful queues disappear on flush; failed-window tombstones last only to registry removal (`input_backlog.rs:48`). O(1024 N) pending-event storage, independent of epoch count. |
| Discarded input identities, `bwm/input_backlog.rs:29` | Pressure, terminal send failure, registry removal | Drained by `input::losses` via `take_discarded`, before/after flush and on retain (`input.rs:83`, `input.rs:97`, `input.rs:124`). Production enqueue immediately flushes (`input.rs:142`); this vector does not grow across completed loop iterations. |
| Pending child launches and copied executable paths, `bwm/children.rs:4`, `bwm/children.rs:14` | Hotkey spawn | One entry/path per launch awaiting a composed first frame. Acknowledgment, exit or five seconds of UI time retires it (`children.rs:24`, `children.rs:34`, `children.rs:43`). O(P) plus path bytes; paths come from the fixed hotkey configuration. No spawn occurs per animation epoch. |
| Close escalation, `bwm/close.rs:23`, `bwm/close.rs:40` | First close of a window | One `Pending` per live window, O(N) capacity. Repeated requests mutate the same row; the loop retains only matching live id/owner pairs (`bwm.rs:283`). |
| Launch error window, `bwm/launch.rs:4`, `bwm/launch.rs:14` | Failed or timed-out launch | One optional BreenGL window at 640 x 140; another error reuses it. Close drops it (`launch.rs:35`). GPU resources belong to that one window. |
| Clipboard connections, `bwm/clipboard_socket.rs:14`, `bwm/clipboard_socket.rs:62` | Accepted connection | C <= 16, at most two per owning process (`clipboard_server.rs:6`, `clipboard_server.rs:12`). Completion, hangup/error or five seconds without progress drops a connection. |
| Clipboard request payload, `bwm/clipboard.rs:13`, `bwm/clipboard.rs:78` | Incremental SET bytes | At most 16 MiB per connection (header refused above that at `clipboard.rs:63`). Vec growth is per received byte up to that cap, not per epoch. Completion moves the payload into the store; connection teardown drops unfinished input. |
| Clipboard reply bytes, `bwm/clipboard_server.rs:26`, `bwm/clipboard.rs:104` | Completed GET/SET/error | At most 16 MiB + 16 bytes per connection; held only until reply completion or connection teardown. The GET payload clone used to encode it is temporary. |
| Clipboard store, `bwm/clipboard.rs:116`, `bwm/clipboard.rs:133` | Successful SET | Exactly one payload, at most 16 MiB; replacement drops the previous payload. Survives the setter's exit by design. This request-owned data is already bounded; it is not a presentation buffer. |
| Poll descriptor vector, `bwm.rs:221`, `bwm.rs:251` | Clipboard connections | Two fixed descriptors plus at most one listener and 16 clients: <= 19 initialized entries. Truncate/append reuses capacity; geometric capacity can exceed 19. |
| Hotkey bindings and Exec paths, `bwm/route.rs:54`, `bwm/route.rs:99` | Startup config load only | Parsed from at most 4,096 bytes, or five built-in defaults. The event loop reads those startup bindings. Action clones used in one iteration are temporary. |
| Font file, `bwm.rs:135`, `libs/libfont/src/lib.rs:78` | Startup font parse | One owned copy of the selected font; no append during animation. The startup read buffer and FontConfig paths are dropped. |
| Font entries and atlas-page metadata, `bwm.rs:138`, `libs/libgfx/src/ttf_font.rs:438` | First use of a new chrome glyph | Cache configured for 256 entries. LRU eviction frees empty atlas pages (`ttf_font.rs:591`, `ttf_font.rs:628`); page row classes are bounded by the selected font and fixed chrome size. Vec capacities retain their high-water sizes. Ordinary client animation does not redraw titles or grow the glyph alphabet. |
| Surface clip stacks, `libs/libgfx/src/surface.rs:172`, called from `bwm/chrome.rs:398` | Partial chrome redraw | bwm pushes one clip and pops it before returning; each persistent surface retains capacity for that depth. Surface batch vectors exist only during `batch`, not across loop iterations. |
| Shared BGL context command and clip buffers, `libs/bgl/src/lib.rs:9`, `libs/bgl/src/lib.rs:153`, `libs/libgfx/src/device.rs:42` | Desktop/font drawing | One process context, three fixed declared table slots; command capacity follows the largest drawing submission and is cleared on submit. Clip capacity follows drawing depth, not epoch count. |

`bwm/clock.rs:4` holds only scalar time/ack fields. `bwm/gesture.rs:5`,
`:18` and `:58` hold only ids, points and at most two resize tuples. The input
source context, pointer hover, clock text, remembered defaults and poll wait
wrapper are inline state; they allocate no retained heap storage.

The compositor also owns a context, three GPU targets and two GPU-backed
Surfaces (`bwm/compositor.rs:103`, `:140`, `:174`). Counts stay fixed for a
mode; recovery drops the old compositor. Pixel storage is in device resources,
not a new CPU image per epoch. The font scratch slot contains two GPU scratch
target slots and one reusable geometry buffer (`libs/libgfx/src/ttf_font.rs:77`,
`:351`), with allocation driven by glyph geometry rather than animation epochs.

## Temporary allocation inventory

These buffers are released within the same loop iteration. They can influence
allocator residency, so a host retained-capacity check cannot replace VmRSS
acceptance, but they are not accumulating scene history:

- LIST scratch and discovery ids (`bwm/session.rs:45`, `bwm/desk.rs:190`),
  stack reconciliation/cycling, pointer content/notice arrays, hotkey action
  clones, normalized input batches/payloads/positions and due repeats
  (`bwm.rs:286`, `:305`, `:322`, `:404`, `:715`, `:742`,
  `bwm/input.rs:226`, `:267`, `bwm/pointer_route.rs:247`, `bwm/stack.rs:66`).
- The frame damage vector has at most three rectangles (`bwm.rs:626`). Snapshot
  window/damage arrays, clipped damage (at most 16 emitted rectangles, or one
  bounding rectangle), GPU stream words and pointer-run arrays are temporary
  (`bwm/compositor.rs:258`, `:259`, `:291`, `:337`, `:383`, `:829`).
- Visibility subtraction/output, first-frame eligibility ids, captures,
  readback bands and records (`bwm/compositor.rs:399`, `:458`, `:494`, `:496`,
  `:636`; `bwm/chrome/validation.rs:61`) belong to one deferred check or
  eligibility calculation. At most eight captures exist in this case's
  eight-window scene. Each readback band is width x 32 x 4 bytes.
- Error strings, evidence record formatting, launch acknowledgment lists,
  expired-launch errors, clipboard authorization LIST scratch and GET encoding
  intermediates are request/check-local (`bwm/chrome.rs:26`,
  `bwm/launch.rs:106`, `bwm/children.rs:24`, `bwm/clipboard_socket.rs:43`,
  `bwm/clipboard.rs:147`). No vector of those results survives an iteration.

## Deterministic evidence and implementation choice

Check-first commit: `fad73c34`. The compositor, validation and timing source
files at that commit have identical git blobs to `5424b3c5`, so the red run
executes that baseline's production allocation code. No boot was used.

`python3 tests/bwm_memory_bound_test.py` compiles path-included production
modules with a host GPU boundary that returns eight successful snapshots. A
non-allocating observer records allocation size and birth epoch, drops freed
records, and sums the capacity of surviving allocations born after epoch 60
at each of the 600 measured boundaries. The 3/3 loops run through epoch 660 before
the assertion. Temporary snapshot/command allocations are excluded because
they are already freed at the boundary. A pointer address reused by the host
allocator is still recorded with its new birth epoch.

The baseline failures were 153,600 summed capacity-bytes for sampled fronts,
76,800 for replacement validation layers, and 113,277,648 for click history
under successive short-lived pointer devices. These sums are neither peak
live bytes nor RSS. The validation test uses eight plain geometry rows to
exercise the generic production queue; the actual `PresentedScene` uses
`Vec<Layer>::clone_from` through its explicit Clone implementation.

The 3/3 checks pass after the repair, reporting zero newly allocated
retained capacity after warmup. The existing four validation and three timing
checks also pass, including the inclusive double-click boundary. Two additional cross-device timing checks, committed first as `15aaf878`
and `3625aacb`, fail on the initial expiry change (`af6deef2`): either an
earlier or a later second-device timestamp can erase the first device's
pending double-click. Input timestamp validation is per device
(`kernel/src/input/stream.rs:761`). The final repair therefore retires history
by device registration, not by cross-device timestamp comparisons. The 5/5
timing checks and the affected 660-epoch click-capacity check pass; unchanged
compositor/queue capacity results stand. The final click-capacity loop retires
its short-lived devices after each batch through the production retention
method. The original red run used the check-first `fad73c34` workload against
the baseline, before that retention method existed.
The original
memory case, shared cases, cumulative selection and threshold are unchanged.

The implementation uses Vec capacity reuse rather than an arbitrary window
ceiling. The working scene and saved validation scene remain distinct so a
failed composition cannot corrupt the last successfully presented geometry.
Click matching keeps the existing inclusive 400 ms policy. History retirement
uses the shared device-registration query and reuses storage. The query runs
after ready input batches, keeps state on transient errors, and treats only
ENODEV as retirement. The same query retires stale consumed-key device ids.
It adds no idle-loop queries or driver-specific interface. This is a first-principles userspace change; no
external driver source, firmware, architecture branch or backend-specific
interface is involved.

The 2/2 scoped release builds of `userspace/programs --bin bwm` passed for x86_64 and
aarch64 with CARGO_BUILD_JOBS=4 under `taskpolicy -b`, with zero warnings.
No kernel source changed. Hardware acceptance remains in the acceptance unit;
ARM still lacks runtime validation of restart adoption, supervision and the
600-epoch VmRSS bound. No VM or hardware boot, PR or merge was performed here.
