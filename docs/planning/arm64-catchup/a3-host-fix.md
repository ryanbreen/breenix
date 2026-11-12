# A.3 host renderer isolation — a3-u25-fix

This unit implements the host-side direction selected in
[a3-fix-design.md](a3-fix-design.md), under ARM coordinator ledger line 248
and line 243's normal-design restriction. Issue #2890 tracks this host work;
#2872 still tracks window-manager acceptance. Breenix runtime code, suite
checks and deadlines are unchanged. No Breenix boot belongs to this unit.

## Renderer ownership

`scripts/build-qemu-gl.sh` applies `renderer-worker.patch` after renderer
attestation, then `angle-blob-cache.patch`, and builds bundle pin
`10.0.12-utm-4d73aaf0-b3`. The new patch hashes travel in the signed bundle's
PINS file. The underlying QEMU, virglrenderer and ANGLE source pins are unchanged.

In the patched QEMU, `hw/display/virtio-gpu-gl.c` creates a single renderer
worker for the EGL path. Its `virtio_gpu_renderer_call` sends a call record
through `breenix-render-worker.h`. The worker executes native VirGL calls
without acquiring BQL; `renderer_run_call` asserts that its thread does not
hold BQL. `breenix-virgl-rpc.h` marshals 30 API entrypoints, including context
creation/destruction, resource operations, transfers, submits, polling and
cleanup. Read-only capability discovery before worker initialization remains
synchronous. The CGL path retains its existing context implementation.

Device state stays on QEMU's main loop under BQL. A coroutine serializes
control, cursor, flush, poll, unmap-resume and asynchronous fence callbacks;
its event queue coalesces repeated notifications from a source. During a
renderer call that coroutine yields. A completion bottom half retires the
call and resumes the coroutine on the main loop. The copied SUBMIT_3D buffer,
IOV arguments, command and device remain live across that suspension. Existing
command handlers consume the returned result and produce their existing
responses. Asynchronous fence retirement joins the same device event queue,
so a callback cannot retire a command before the active handler puts it on
the fence queue.

The main loop creates a private EGL sharing context from the display's
context group. Renderer contexts are created and made current on the worker;
display contexts and surfaces remain on their original threads. Publishing a
scanout or GL update explicitly flushes the producer context before invoking
display callbacks. Native texture cleanup copies its handle into an owned
worker record, retaining it until the FIFO release runs. The macOS worker
surrounds jobs with an Objective-C autorelease pool for ANGLE/Metal objects.
Renderer attestation also runs on the worker, against the executing context.

Reset first closes event admission and drains the active coroutine. A reset
from a vCPU is marshalled to the main loop while that vCPU waits without BQL;
its resources and descriptors are not destroyed while the renderer is using
them. Unrealize quiesces similarly, removes scanouts, stops renderer fence
producers while their callback bottom halves still exist, cleans up the
renderer, destroys the sharing context, and joins the worker. Worker arguments
are retired on the main loop before their storage is freed.

This is host device execution isolation, independent of guest case timing.
It adds no guest delay, yield, command splitting, test mode, startup deferral,
or deadline change. Display-backend rendering remains on its original main-loop
path; the execution isolation covers VirGL.

## Persistent program cache

`ui/egl-helpers.c` installs EGL_ANDROID_blob_cache callbacks after EGL
initialization and before renderer contexts. `breenix-blob-cache.h` obtains
the macOS user cache directory with `confstr`, creates a private namespace
from bundle pin and EGL identity, and uses ANGLE's opaque key. ANGLE's key
also incorporates renderer/program identity, as the design report documents.

Records contain the key, a SHA-256 of the value and the opaque value. Reads
refuse invalid keys, short records, oversized records and digest mismatches.
A missing, corrupt or unwritable cache becomes ordinary compilation. The
size-query and too-small-buffer cases return the required size without copying.
Writes use a private temporary file and atomic rename. A process mutex and
file lock serialize callbacks and processes. Entry values are limited to
16 MiB and the record budget to 64 MiB, evicting older entries before a write;
stale temporary files are removed while holding the lock. No cache blobs or
site configuration are committed.

## Forcing check and validation scope

Check-first commit `80333570` adds `tests/a3_qemu_renderer_test.py`. Against
the pinned old source it compiles the production SUBMIT_3D handler with an
unfinished renderer operation and fails with:

```text
FAIL: unfinished SUBMIT_3D renderer holds BQL; unrelated MMIO blocked
```

The fixed harness additionally compiles the production RPC wrapper, worker
and QEMU call adapter. A condition-variable handshake holds the renderer
unfinished while another host thread probes the modeled BQL for unrelated
MMIO. Reset work is queued before renderer release; the check verifies FIFO
execution, completion order and drain before freeing worker storage. The
harness was strengthened after the first commit to exercise those production
adapters and the in-flight queue. Its progress and ordering expectations were
not relaxed. The final scoped run passes 1/1 with `cc -Wall -Wextra -Werror`.

This check uses a modeled BQL and the adapter's non-coroutine wait path. It
is not an executing VirtQueue, an HVF measurement, or a window-manager verdict.
The coroutine and device lifetime integration also rely on the ownership
review above and the QEMU build; live suite acceptance is still outstanding.

`tests/a3_qemu_blob_cache_test.py` compiles the actual cache header into a
host-only shim and uses the design unit's production shader catalogue through
ANGLE/Metal. It checks record size probes, insufficient-buffer behavior,
round-trip contents and corruption refusal, then populates the cache and
uses it from a fresh process. The shim observes callback traffic; it does not
replace the production disk parser/storage implementation. The harness's
`--host-cache-library` option selects this callback provider for the experiment.

The cache check passes 1/1: the populate process makes 16 writes (236319
value bytes), and the fresh persistent process makes 32 value reads and no
writes. Its first-context link total is 13.207 ms, versus 750.946 ms with the
cache disabled; compile still takes 348.897 ms in that persistent context.
Those host observations do not establish a suite deadline margin. The
[host result record](serials/a3-u25-host.json) retains the 96 successful
program rows, library identities, bundle PINS and patched-source hashes.

The complete final recipe builds, signs and installs b3 in 604 seconds;
its binary SHA-256 is
`d8237d0bcaeaffcbc43367def35094864bb02b906293dbbffc0f1d4f0a7e18cb`.
The installed build's 11 patched source files match a clean reconstruction
from the pinned archive and recipe patches. The modified QEMU objects compile
without warnings. The host dependency build still emits warnings in unchanged
SPICE/UTM sources and from Homebrew deployment-target mismatches. The earlier
compile errors in the new integration were corrected before this final recipe
run. ARM64 release passes with only the known pinned-core future-compatibility
advisory; x86 release passes without warnings. No guest source changed between
those builds and the final host fix.

Reproduce the scoped checks after building the recipe:

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b python3 tests/a3_qemu_renderer_test.py
CARGO_BUILD_JOBS=4 taskpolicy -b python3 tests/a3_qemu_blob_cache_test.py
```

The ARM ledger records claim-lint, commit and push results. No R240 boot,
Mac deployment, PR creation or merge is performed here. The acceptance unit
still owes its single final-head ARM window-manager run and the coordinator's
unchanged 25% deadline headroom check.
