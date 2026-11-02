#!/usr/bin/env python3
"""Run the installed GL QEMU with its existing queue/fence trace events.

Select with BREENIX_QEMU_GL_BINARY; boot-interactive owns the Mac slot, window,
Vigil registration and suite verdict. This wrapper changes none of those.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
EVENTS = (
    "virtio_pci_notify_write", "virtio_queue_notify", "virtqueue_pop",
    "virtqueue_fill", "virtqueue_flush", "virtio_gpu_cmd_ctx_submit",
    "virtio_gpu_fence_ctrl", "virtio_gpu_fence_resp",
    "virtio_gpu_cmd_suspended", "virtio_gpu_inc_inflight_fences",
    "virtio_gpu_dec_inflight_fences",
    "virtio_set_status", "virtio_gpu_cmd_ctx_create", "virtio_gpu_cmd_ctx_destroy",
    "virtio_notify", "virtio_pci_notify",
)


def main():
    binary = os.environ.get("BREENIX_QEMU_TRACE_BINARY") or subprocess.check_output(
        [str(ROOT / "scripts/build-qemu-gl.sh"), "--print-path"], text=True).strip()
    arguments = sys.argv[1:]
    if "-kernel" not in arguments:
        os.execv(binary, [binary, *arguments])
    renderer = os.environ.get("BREENIX_HOST_RENDERER_JSON")
    if not renderer:
        sys.exit("queue trace needs boot-interactive --gpu-gl and its per-run renderer path")
    directory = Path(renderer).parent
    available = subprocess.run([binary, "-trace", "help"], capture_output=True, text=True, check=True)
    names = set((available.stdout + available.stderr).splitlines())
    missing = set(EVENTS) - names
    if missing:
        sys.exit("installed QEMU lacks required trace events: " + ", ".join(sorted(missing)))
    events = directory / "qemu-gpu-events"
    events.write_text("\n".join(EVENTS) + "\n")
    (directory / "qemu-gpu-trace.json").write_text(json.dumps({
        "binary": binary,
        "version": subprocess.check_output([binary, "--version"], text=True),
        "events": EVENTS,
        "log": "qemu-gpu-trace.log",
        "guestFeature": "gpu_queue_trace",
        "hostPatchesAdded": False,
    }, indent=2) + "\n")
    os.execv(binary, [binary, "-msg", "timestamp=on", "-D", str(directory / "qemu-gpu-trace.log"),
                         "-trace", f"events={events}", *arguments])


if __name__ == "__main__":
    main()
