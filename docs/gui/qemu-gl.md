# The GL QEMU for ARM64 boots on a Mac

ARM64 Breenix boots on a Mac reach a host GPU through `virtio-gpu-gl-pci`, which Homebrew's QEMU
does not have: it has no VirGL, and its Cocoa display has no GL scanout. `scripts/build-qemu-gl.sh`
builds a second QEMU for this and installs it beside Homebrew's, which it leaves alone.

## The stack

The script builds UTM's matched host stack, pinned at UTM's dependency manifest (UTM commit
`4d73aaf0`): QEMU 10.0.12-utm with UTM's patch set, WebKit's ANGLE built for its Metal and OpenGL
backends, UTM's libepoxy and virglrenderer forks, and the SPICE server. UTM builds QEMU as a
library for its app; the script builds the same patched source as a command-line
`qemu-system-aarch64` with HVF, Cocoa, OpenGL, VirGL and SPICE. Every source is pinned by commit
or SHA-256 in the script, and the installed bundle's `PINS` file lists them.

- **Install:** `scripts/build-qemu-gl.sh` (about 9 minutes from clean at background priority with
  4 jobs; ANGLE is about half of it). It installs `QEMU-GL.app` under
  `~/Library/Caches/breenix/qemu-gl/<pin>/`, ad-hoc signed with the hypervisor entitlement, with
  App Nap off. `--print-path` prints the executable.
- **Backend:** ANGLE runs on Metal only with `ANGLE_DEFAULT_PLATFORM=metal`, as UTM sets it;
  without it ANGLE picks its OpenGL backend. The launcher sets it.
- **Not built:** Venus and Neptune (Vulkan and Direct3D) in virglrenderer, which VirGL does not
  use. Libraries with no graphics code (glib, pixman, libslirp, OpenSSL, libjpeg, Opus, libpng)
  come from Homebrew.

## Booting with it

`scripts/boot-interactive.sh --gpu-gl` boots on the GL QEMU and adds `-device virtio-gpu-gl-pci`
beside the usual `virtio-gpu-device`. One binary serves both frontends:

- the window: `-display cocoa,gl=es`, UTM's QEMU drawing GL scanouts through ANGLE on Metal;
- no window (`--no-display`): `-display none` with SPICE GL on a local socket, no client attached.

## Host renderer attestation

Nothing in the device protocol lets the guest tell a GPU from a software renderer, so the run
carries host evidence. `scripts/qemu-gl/host-renderer-attestation.patch`, applied on top of UTM's
patches, makes QEMU's VirGL glue write `host-renderer.json` beside the serial log: the GL vendor,
renderer and version of the context that executes the guest's commands, read in the QEMU process
after each renderer context is created and after its first `SUBMIT_3D`.

After the VM stops, `scripts/check-host-renderer.py` judges the file, and a refused run fails the
boot. It refuses a missing or empty file, a software renderer (llvmpipe, softpipe, lavapipe,
SwiftShader, anything "Software"), any renderer other than the Apple GPU through ANGLE's Metal
backend, and a guest capset renderer string (printed as `VIRGL_CAPSET_RENDERER=`) that the host's
does not contain. A boot whose guest never creates a VirGL context has no attestation and is
refused.

`scripts/qemu-gl/plant-software-renderer.sh DIR` builds the negative control: a copy of the bundle
with the same QEMU executable whose ANGLE renders on Apple's software OpenGL renderer. Boot it with
`BREENIX_QEMU_GL_BINARY=<printed path> ANGLE_DEFAULT_PLATFORM=gl` and `--gpu-gl`; the check
refuses it, naming the software renderer. (WebKit's ANGLE has no SwiftShader backend, and Mesa's
llvmpipe cannot stand in: UTM's SPICE and Cocoa displays need ANGLE's IOSurface support.)
