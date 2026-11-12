# A.3 EGL blob-cache dispatch repair — a3-u26

This unit addresses ARM coordinator ledger line 254(1)–(2), following the
pre-guest abort in `20261112T025722Z-qemu-45699`. Issue #2890 tracks the host
graphics changes; #2872 retains window-manager acceptance. This unit changes
the optional host program-cache installation and its check, with no Breenix
runtime change or boot.

## Cause

The pinned UTM libepoxy fork is `bf98587477fe68d07b93319ece7b40a7d0e2eabe`.
Its generated `build-breenix/src/egl_generated_dispatch.c:4829–4831` resolves
`eglSetBlobCacheFuncsANDROID` through `PROVIDER_EGL_ANDROID_blob_cache`.
That provider exists: lines 3691–3693 consult
`epoxy_conservative_has_egl_extension(provider_name)` before calling
`eglGetProcAddress`. This is a display-scope mismatch, not an omitted provider
entry or an EGL-version requirement.

In `src/dispatch_egl.c:77–80`, the conservative helper calls
`epoxy_has_egl_extension(eglGetCurrentDisplay(), ext)`. The latter checks the
supplied display's extension string and the client extension string
(`dispatch_egl.c:94–97`). Before a context is made current,
`eglGetCurrentDisplay()` returns `EGL_NO_DISPLAY`. ANGLE advertises
`EGL_ANDROID_blob_cache` on its initialized display, but does not advertise it
in the client extension string queried with `EGL_NO_DISPLAY`.

QEMU's `ui/spice-display.c:1767` calls
`qemu_egl_init_dpy_cocoa(DISPLAY_GL_MODE_ES)` for headless SPICE GL. That
initializer calls `qemu_egl_init_dpy`, which initializes EGL, binds the API,
chooses a config and installs the cache before SPICE creates its context.
The cache's explicit-display extension check therefore succeeds, while the
subsequent libepoxy dispatch checks `EGL_NO_DISPLAY` and aborts. Generated
dispatch prints the missing-provider message and calls `abort()` at
`egl_generated_dispatch.c:3903–3911`.

## Repair

`scripts/qemu-gl/angle-blob-cache.patch` resolves
`eglSetBlobCacheFuncsANDROID` with `eglGetProcAddress` after checking the
initialized display's extension support. It calls that typed function pointer
instead of the current-display-dependent extension dispatch. If the address is
absent, it logs `eglSetBlobCacheFuncsANDROID unavailable; program cache disabled`
and returns before allocating the cache directory or opening its lock.
The normal EGL initialization and context creation can continue.

No context is created just to satisfy the loader, no extension string is
fabricated, and no resolver failure handler is overridden. The repair belongs
to QEMU's host patch under ledger line 243's normal-design restriction.
`scripts/build-qemu-gl.sh` advances the bundle pin from b3 to b4 so that the
launcher does not reuse b3.

## Check and scope

Check-first commit `39dea5b8` adds `tests/a3_qemu_egl_init_test.py`. The host
program links the recipe-built `libcommon.a.p/ui_egl-helpers.c.o` and bundled
libepoxy/GLib, with the bundle's framework search path. It executes the same
SPICE display initializer and next context-creation step without a guest,
hypervisor, window or debugger. It does not copy or model the initializer.
Unused object functions are removed by the host linker's dead stripping.
The harness supplies only the QEMU error logger used by the reachable path.

Against b3, the available-entrypoint mode failed with SIGABRT (subprocess
return code -6), with the same `No provider of eglSetBlobCacheFuncsANDROID
found` / `EGL_ANDROID_blob_cache` diagnostic as the retained boot. The red log
is `/tmp/a3-u26-egl-red.log`.

The check requires initialization with no current display, the display-only
blob-cache extension, and an ANGLE/Metal context. A second fresh host process
intercepts only the blob-cache procedure lookup to return NULL; it requires
the cache-disabled log and successful display/context initialization. These
expectations were committed before the repair.

The first b4 check passed the real-entrypoint mode, then its missing-entrypoint
interceptor recursed through a saved unresolved libepoxy core thunk. The host
crash record identified `missing_blob` and
`epoxy_eglGetProcAddress_global_rewrite_ptr`; `src/dispatch_common.h:99–106`
shows that thunk forwards through the replaceable dispatch pointer. The harness
now resolves that core thunk before wrapping it. The missing-entrypoint
condition and assertions are unchanged. The corrected check passes 1/1, with
both host processes exiting 0 and the absent-function log present.

The earlier shader harness called ANGLE directly and installed callbacks after
making its root context current. It could not catch this initialization-order
defect. `tests/a3_qemu_blob_cache_test.py` still exercises the production disk
callbacks and real shader catalogue. Its callback-traffic observation now
wraps the resolved procedure instead of the old direct-dispatch call. Its
populate, fresh-process hit, no-fresh-write, record integrity and successful
link assertions are unchanged.

This host-only diff does not reach a guest kernel or userspace binary, so its
build scope is the GL QEMU recipe. It changes no renderer worker code and does
not repeat that unit's BQL test. The acceptance unit retains the boot and
review obligations in coordinator line 254(3)–(4), including diagnostic cleanup;
this unit makes no window-manager deadline or suite acceptance claim.

## Build and results

The complete recipe builds and signs `10.0.12-utm-4d73aaf0-b4` in 650 seconds.
Its QEMU executable SHA-256 is
`e52ce6461a0bad90d7105991d695fd9288485256a568ee0e15ca9f43f3d0c09b`.
The [host result record](serials/a3-u26-host.json) includes the signed bundle's
PINS, library and compiled-helper identities, dispatch-source hashes, check
log hashes and shader results. The built cache header equals the recipe's
patch payload. QEMU's changed EGL helper compiles without warnings; warning
messages from unchanged dependencies match the retained b3 build log. There
are no compiler errors in the b4 log. No warning suppression is added.

The persistent-cache check passes 1/1, with 96 successful program rows across
the three processes and two contexts per process. Population writes 16 blobs
(236314 value bytes); the fresh persistent process makes 32 successful value
reads and no writes. These are cache-correctness observations, not a suite
latency claim. Both checks run under `taskpolicy -b` with
`CARGO_BUILD_JOBS=4`; their C shims compile with `-Wall -Wextra -Werror`.

```sh
CARGO_BUILD_JOBS=4 taskpolicy -b python3 tests/a3_qemu_egl_init_test.py
CARGO_BUILD_JOBS=4 taskpolicy -b python3 tests/a3_qemu_blob_cache_test.py
```

The ARM ledger records claim-lint invocations, commits and push status. The
freshness check requires merging newer origin/main before pushing; that merge
does not change this host recipe or its shader inputs.
