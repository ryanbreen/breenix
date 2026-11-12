# A.3 host shader-stall fix design — a3-u24

Choose host-side renderer execution outside QEMU's global lock as the complete
mechanism repair, with a persistent ANGLE program cache as a useful accompanying
optimization. Choose no Breenix runtime change. The measured program cache removes
most repeated link work, but leaves shader translation and cold-cache work under
that lock. It cannot by itself establish the required deadline headroom.

This is the design unit in coordinator ARM ledger line 248(1), governed by
line 243's normal-design restriction and the earlier measurement rulings. It
implements neither the cache in QEMU nor the renderer execution change. Issue
#2890 tracks the host work; #2872 retains recovery acceptance. The historical
mechanism report's stop conclusions predate line 248's accepted mechanism.
This unit does not reinterpret the retained observations as per-instant mutex
ownership or convert a shader microbenchmark into suite acceptance.

## Exact sources and lock path

The sources inspected are the revisions installed by `scripts/build-qemu-gl.sh`:

| Prefix used below | Source identity |
| --- | --- |
| Q | `qemu-10.0.12-utm.tar.xz`, SHA-256 `7c9605290b34152debb842e965a55d2e4fbba4793e536ab677b4027e5dc0ba1a`, with UTM's `qemu-10.0.12-utm.patch` at UTM `4d73aaf0d284f1c22e3e1c1072e729988e134268` (patch SHA-256 `84eee5ca4a512571478029c75bf31f89a3768f476b0f79457e2445c002ed4c13`), then our attestation patch |
| V | [UTM virglrenderer](https://github.com/utmapp/virglrenderer/tree/451505ad0aa449290f2c0672da6877d57ee770e5), commit `451505ad0aa449290f2c0672da6877d57ee770e5` |
| A | [WebKit ANGLE](https://github.com/utmapp/WebKit/tree/ed78ab6e1a37f4f11583a0bd038f22ec91f3ff10/Source/ThirdParty/ANGLE), WebKit commit `ed78ab6e1a37f4f11583a0bd038f22ec91f3ff10`; paths below are relative to `Source/ThirdParty/ANGLE` |

Q line numbers refer to the **patched build tree**, not upstream QEMU main.
Both downloaded Q hashes were checked. The V and A checkouts' HEADs matched
the pins above. The installed bundle is `10.0.12-utm-4d73aaf0-b2`; its PINS,
measured-library hashes and cited-source hashes are retained in
[the measurement record](serials/a3-u24-angle-cache.json). Reconstruct Q by
unpacking that archive and applying those two patches in build-script order.
No external implementation source is copied into this change.

| Where | What happens |
| --- | --- |
| Q `hw/display/virtio-gpu.c:1033–1036,1149–1154,1585` | Queue notification schedules the control bottom half; that bottom half calls the GL device's control handler. |
| Q `hw/virtio/virtio.c:4407–4415`; `util/main-loop.c:175–180,626–635` | The guarded bottom half uses the I/O-handler AioContext, whose GSource is attached to the main-loop GLib context. This is not a renderer IOThread. |
| Q `util/main-loop.c:293–314` | On the macOS/POSIX path, the loop releases BQL around poll, reacquires it at line 308, then dispatches GLib sources at line 310. The control bottom half executes after reacquisition. |
| Q `hw/display/virtio-gpu-gl.c:63–99,198–199`; `hw/display/virtio-gpu.c:1045–1063` | The handler drains the queue and calls `process_cmdq`, which calls the device's `virtio_gpu_virgl_process_cmd` synchronously. |
| Q `hw/display/virtio-gpu-virgl.c:1348–1371,804–848` | SUBMIT_3D selects `virgl_cmd_submit_3d`. It validates/copies the guest stream, then calls `virgl_renderer_submit_cmd` at line 837 without releasing BQL. Our attestation follows the call. |
| Q `target/arm/hvf/hvf.c:2575–2600,2691–2696` | HVF releases BQL to run the vCPU, reacquires it on the ordinary exit path, then services MMIO through address-space reads/writes. A vCPU cannot service that exit while GL processing holds BQL. |
| V `src/virglrenderer.c:306–322`; `src/vrend/vrend_decode.c:2049–2108` | Submit finds the context and directly invokes its decoder; the decoder switches GL context and dispatches stream commands in order. There is no asynchronous submit worker here. |
| V `src/vrend/vrend_renderer.c:5780–5848,5892,6089` | The first draw selects shader variants and the linked program; missing variants compile, and a missing program is added/linked. A new guest context repeats this work. |
| V `src/vrend/vrend_renderer.c:1493–1503,1981–1986,2194` | Compile calls `glShaderSource`, `glCompileShader`, then `glGetShaderiv(COMPILE_STATUS)`; link calls `glLinkProgram`, then `glGetProgramiv(LINK_STATUS)`. The status queries resolve completion before the submit returns. |
| A `src/libANGLE/Shader.cpp:622–720,727–747`; `src/libANGLE/Program.cpp:974–1050,1210–1214` | ANGLE can post compiler/link tasks, but shader resolution and `resolveLinkImpl` wait for them. Internal ANGLE parallelism therefore does not release QEMU's BQL. |
| A `src/libANGLE/renderer/metal/ShaderMtl.mm:51–110`; `ProgramMtl.mm:225–291`; `mtl_utils.mm:870–931` | Shader compilation sets up Metal translation. Program link resolves resources, obtains translated sources, generates MSL, and compiles Metal shader libraries; the Metal helper calls `newLibraryWithSource`. |

Thus the source chain matches the accepted sampled path: a draw within a
SUBMIT_3D can wait for shader translation/link while executing the main-loop
callback with BQL held. This explains cross-device MMIO interference without
assuming that the GPU hardware itself was busy for the whole interval.

## Host experiment

Run once with a new, empty output directory:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b python3 scripts/measure-angle-cache.py --output /tmp/angle-design-result
```

The harness loads the **installed** bundle's EGL, GLESv2, libepoxy and
virglrenderer libraries. It creates a surfaceless EGL Metal display and verifies
ANGLE/Metal from GL_RENDERER. It opens no window, VM or debugger. The renderer's
host-model text is omitted from the committed measurement record.

The shader inputs are the production pass-through vertex program and the 16
fragment programs in `kernel/src/gpu/virgl/encode.rs:205–246` and
`kernel/src/gpu/virgl/pipelines/mod.rs:3–18`. Their byte hashes are in the record.
These are the catalogue that `catalogue_mask` selects and `warm_next` warms
(`kernel/src/gpu/mod.rs:415–416,2991–3050`), including the compositor's solid,
copy and font paths. The experiment includes the catalogue's expensive programs,
not only the first frame's cheapest pair. Profile-only probes are excluded,
matching `encode.rs:249–258`.

The harness encodes CREATE_SHADER with production TGSI text/token capacities
and legal text continuations, then uses VirGL's LINK_SHADER command to force
that pair through the real translator/compiler/linker. Its path is
V `src/vrend/vrend_decode.c:1246–1261` →
`src/vrend/vrend_renderer.c:5961–6004` → `vrend_select_program`, also used by draw.
Each process creates and destroys two fresh guest-style VirGL contexts, retaining
the shared EGL display/root context. Each pipeline must observe two shader
compile-status queries and one successful program link-status query. This
measures a repeated new-context link, not reuse of one already-linked GL object.

Three fresh processes run in fixed order: no application cache, empty persistent
cache populated by linking, and reuse of that cache from a new process. Persistent
callbacks implement the EGL_ANDROID_blob_cache size-query/copy protocol and
store opaque blobs by ANGLE's supplied key. Feature overrides are cleared for
the experiment. The installed ANGLE default program-cache capacity is 0 bytes
(A `src/libANGLE/Constants.h:20`), so the disabled mode also has no ANGLE memory
program cache; Metal's own library/driver caches remain in their normal state.
<!-- claim-lint:ok: docs/planning/arm64-catchup/serials/a3-u24-angle-cache.json retains the installed library identities; A src/libANGLE/Constants.h:20 sets the default capacity. -->
“First” below means a fresh process/context and application-cache miss, **not**
a flushed system compiler cache or rebooted host.

Timing hooks replace libepoxy's exported dispatch pointers **inside the harness
process only**, forwarding to the installed ANGLE functions. Compile time spans
`glCompileShader` through its status query; link time spans `glLinkProgram`
through its status query. The submit column additionally includes TGSI parsing,
Python/FFI overhead and captured-GLSL file writes. It is not a QEMU CPU-time
measurement. Generated GLSL is retained locally by hash; the JSON records the
hashes by pipeline. No QEMU or bundle file is modified.

### Measured milliseconds

The scoped check's single successful run produced these numbers, retained at
`serials/a3-u24-angle-cache.json` (sums of its 16 pipeline rows per context):

| Process / context | Compile | Link | Submit total | Largest single pair compile + link |
| --- | ---: | ---: | ---: | ---: |
| Cache disabled / first | 406.523 | 713.890 | 1177.025 | 335.181 |
| Cache disabled / repeated | 400.722 | 674.545 | 1126.546 | 336.163 |
| Empty cache / first | 387.710 | 829.771 | 1264.342 | 326.509 |
| Populated cache / repeated | 356.090 | 8.553 | 410.055 | 161.075 |
| Persistent cache, fresh process / first | 405.160 | 16.889 | 482.299 | 158.583 |
| Persistent cache / repeated | 371.661 | 8.558 | 422.850 | 140.907 |

The populated process wrote 16 blobs totalling 236327 bytes; its second context
made 16 successful value reads. The fresh persistent process made 32 successful
value reads and 0 writes. The callback's `gets` includes size probes (48 in
the populate process, 64 in the persistent process); `hits` counts successful
value copies, not probes. The disabled process invoked no cache callbacks.
The record retains the 96 successful program rows, library identities and shader
hashes; these counts do not represent 96 independent performance trials.
<!-- claim-lint:ok: docs/planning/arm64-catchup/serials/a3-u24-angle-cache.json records 96 program rows and per-process gets, hits, puts and bytes_written. -->

Fresh-process total link time falls from 713.890 to 16.889 ms, about 42 times
cheaper in this experiment. Total submit time falls from 1177.025 to 482.299 ms.
But compile totals stay about 400 ms. In the persistent process's first context,
pipeline 8 alone still takes 155.848 ms to compile. Cold population still
costs 1264.342 ms of submit time. These are ordered observations, not statistical
bounds or proof of a 25% suite margin. One earlier development execution showed
the same residual compile behavior; it is not counted as another acceptance run.
<!-- claim-lint:ok: docs/planning/arm64-catchup/serials/a3-u24-angle-cache.json records the measured compile_ms, link_ms and submit_ms; the paragraph explicitly limits statistical and acceptance claims. -->

The experiment forces compile/link with a default-state LINK_SHADER rather than
replaying the compositor's resource transfers, exact draw-state variants or
Metal render-pipeline creation. It therefore measures this real shader set's
compile/link cache behavior, not total startup, presentation correctness or the
complete 19–65 KB guest command. Cold driver compilation, additional variants,
resource transfer and first-draw pipeline creation remain acceptance concerns.

## Candidate assessment and choice

### (a) Persistent program/shader cache — host-side optimization, selected

Wire EGL_ANDROID_blob_cache into QEMU's common EGL-display initialization
(Q `ui/egl-helpers.c:475–525`, after successful EGL initialization and before
renderer contexts), through an additional patch in `scripts/build-qemu-gl.sh`.
That location serves headless SPICE GL as well as desktop EGL. Neither Breenix
nor virgl's guest protocol needs a new API.

ANGLE already hashes shader keys, ANGLE version, context version, renderer and
pre-link bindings (A `src/libANGLE/MemoryProgramCache.cpp:65–108`). Its program
link checks the cache before ordinary linking (`Program.cpp:974–1013`), stores
a successful program (`Program.cpp:2344–2361`), and delegates external persistence
to blob callbacks (`BlobCache.cpp:105–117,155–194,259–270`). Metal can load the
saved executable and compile/load its MSL libraries (`ProgramMtl.mm:177–202`).
The measured hits demonstrate that the built stack supports this path.

For implementation: use a private per-user cache directory from the host's
cache-directory API, namespaced by the QEMU bundle pin and ANGLE/renderer
identity; use ANGLE's opaque keys, atomic replacement and bounded reads. Bound
aggregate storage (proposed 64 MiB), evict older entries, reject oversized or
invalid records, and let an absent/corrupt cache become an ordinary compile miss.
Do not embed site paths, hardware identities or cache blobs in the repository.
Callbacks can run on compiler threads: protect storage bookkeeping, keep read
size/probe semantics correct and avoid unsynchronized global mutable state.
A cache write failure must not invalidate an otherwise valid program.

This is normal host graphics design: reuse an identical validated program across
contexts and processes rather than paying translation/link cost repeatedly.
It improves ordinary desktop/client startup, independent of test mode. However,
cache misses and the measured residual shader compile still need the lock fix.

Do **not** blindly enable `cacheCompiledShader` as a Metal shortcut. That frontend
feature controls `MemoryShaderCache` at A `Display.cpp:1620–1625`. The generic
shader deserializer explicitly supports backends without extra link-time state
(`Shader.cpp:841–855`), while `ShaderMtl` maintains a separate compiled Metal state
(`renderer/metal/ShaderMtl.h:25–43`, initialized by `ShaderMtl.mm:59–60`). Program
cache support is established here; persistent standalone Metal shader support
is not. Extending that serialization would be a separate host ANGLE change with
correctness work, not a safe environment toggle.

### (b) VirGL outside BQL — host-side mechanism repair, selected direction

There is no clean launcher switch in this pinned stack. Q's GL device properties
are stats/venus/neptune (`hw/display/virtio-gpu-gl.c:154–164`). THREAD_SYNC moves
fence waiting, not stream execution. V `src/virglrenderer.h:111–115,146–150`
explicitly says RENDER_SERVER is ignored by the VirGL renderer. Enabling Venus,
Neptune or another renderer would change the graphics protocol/backend, not
move this VirGL command stream to a worker.

Carry a proper single-owner renderer execution path in the QEMU host build,
keeping its public guest protocol and rendering unchanged. The design boundary:

1. Under BQL, validate and copy command payloads and retain the resources and
   device lifetime needed by that work. Enqueue renderer jobs in guest order.
2. One renderer thread owns the EGL/VirGL contexts and renderer calls, including
   context/resource operations, submits and transfers. It executes without BQL.
   Merely moving SUBMIT_3D while leaving resource destruction on the main loop
   would race the same renderer state.
3. Return bounded completion records to a main-loop bottom half. Under BQL,
   complete virtqueues, register/retire protocol fences and publish scanout work.
   Marshal callbacks that currently reach QEMU display/virtqueue state; fence
   ordering and resource lifetime must remain correct across that handoff.
4. Reset/unrealize quiesce the queue and account for in-flight jobs before
   destroying renderer objects or contexts. Display consumers retain completed
   shared textures until their own completion boundary. Preserve failure replies
   and handle worker/context loss without dereferencing torn-down guest state.

This is normal emulator/device design: a slow host compiler must not serialize
unrelated virtual devices. It covers cold shaders and future new programs as
well as cache hits; it does not rely on finding a favorable startup interleaving.
It is a larger lifecycle change, requiring implementation validation, not a
claim that an arbitrary `bql_unlock()` wrapper is safe. In particular, the
current reset path waits for its main-loop bottom half and then frees queued
commands (Q `hw/display/virtio-gpu.c:1604–1668`); callbacks and display state also
assume their current thread. A copied stream alone leaves those lifetime obligations unresolved.
<!-- claim-lint:ok: #2890 tracks this proposed host lifecycle repair; Q hw/display/virtio-gpu.c:1604-1668 is the current reset path, not completed worker validation. -->

The cache numbers justify including the optimization, but reject cache-only as
a complete fix: the remaining pair still blocks for 158.583 ms in the measured
persistent first context, and an empty cache reintroduces the whole cost.
The selected mechanism repair is execution isolation; no worker speedup or
finished BQL-safety validation is claimed by this design unit.

### (c) Breenix alternatives considered

A context can create programs lazily: `bind_programs` already creates just the
requested pair (`kernel/src/gpu/virgl/encode.rs:454–481`), while background
`warm_next` walks the remaining catalogue. Making eager preparation explicit
per application, or warming a declared likely-use set, could reduce startup
CPU/memory demand on its own merits. It must preserve first-use compilation
and a predictable interaction path across backends. Simply removing eager
warm-up would move a shader cost into a later click, repeating the earlier
regression identified in ledger line 243; it would not solve BQL serialization.

Compositor-scoped program reuse is also normal, but the guest already keeps
program handles within each GPU context (`encode.rs:439–481`); those objects
cannot simply survive destruction of that context. A cross-process compiled
program service would introduce new lifetime/API work while still missing the
host lock defect. Do not introduce one for this issue. No sleep, yield, command
padding/splitting, test-sensitive path or deadline change is selected.

## Check, validation and next unit

Check-first commit `1d97b9a8` adds `tests/a3_angle_cache_test.py`. Its initial
run failed because the new measurement harness did not exist. That is red-first
for the host tool, **not** a forcing test of a runtime fix. The implementation
adds the harness and retains the check's results via optional `A3_ANGLE_OUTPUT`;
its assertions and expected cache behavior are unchanged.

The scoped check passed 1/1 in 5.806 s under `taskpolicy -b`, with four build
jobs exported and no compiler step or warnings. It verifies the 16-program
catalogue in two fresh contexts per process, successful compile/link status,
ANGLE/Metal, and real persisted value reads across processes. No timing
threshold was invented for this design experiment. No guest source changed,
so neither x86 nor ARM kernel/userspace builds or suite boots are reached.

The implementation unit must add a cause-specific deterministic host check:
force a renderer job to remain unfinished while unrelated MMIO work completes,
and verify ordered response/fence handling and reset with in-flight work.
For cache integration, demonstrate a miss/populate/restart-hit through the
patched host stack using this shader input. These are required future checks,
not completed evidence. Final-head acceptance remains the single authorized
headless ARM window-manager acceptance run with unchanged deadlines and the coordinator's
25% headroom requirement. The host-only design reaches no R240 suite. This unit
runs no Breenix boot, deployment, PR creation or merge.

Claim-lint invocations and their observed exit statuses, plus final commit/push
identity, are recorded in the ARM ledger. The report, harness, check and
measurement record are this unit's deliverables; window-manager acceptance
remains open.
