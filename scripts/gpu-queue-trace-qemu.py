#!/usr/bin/env python3
"""Enable existing QEMU queue/fence trace events through its QMP socket.

Start this observer before boot-interactive --qmp SOCKET. The real installed
QEMU runs through its normal QoS launcher, Mac slot, window and Vigil recorder.
"""
import argparse
import json
from pathlib import Path
import socket
import sys
import time

EVENTS = (
    "virtio_pci_notify_write", "virtio_queue_notify", "virtqueue_pop",
    "virtqueue_fill", "virtqueue_flush", "virtio_gpu_cmd_ctx_submit",
    "virtio_gpu_fence_ctrl", "virtio_gpu_fence_resp",
    "virtio_gpu_cmd_suspended", "virtio_gpu_inc_inflight_fences",
    "virtio_gpu_dec_inflight_fences",
    "virtio_set_status", "virtio_gpu_cmd_ctx_create", "virtio_gpu_cmd_ctx_destroy",
    "virtio_notify", "virtio_pci_notify",
)


def enable(stream, log):
    if "QMP" not in json.loads(stream.readline()):
        raise RuntimeError("missing QMP greeting")

    def command(name, arguments=None):
        request = {"execute": name}
        if arguments is not None:
            request["arguments"] = arguments
        stream.write(json.dumps(request) + "\n")
        stream.flush()
        while True:
            line = stream.readline()
            if not line:
                raise RuntimeError("QMP closed before trace activation")
            reply = json.loads(line)
            if "error" in reply:
                raise RuntimeError(str(reply["error"]))
            if "return" in reply:
                return reply["return"]

    command("qmp_capabilities")
    result = command("human-monitor-command", {"command-line": "logfile " + json.dumps(str(log))})
    if result.strip():
        raise RuntimeError("QEMU refused trace logfile: " + result)
    version = command("query-version")
    for event in EVENTS:
        command("trace-event-set-state", {"name": event, "enable": True})
    return version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("socket", type=Path)
    parser.add_argument("log", type=Path)
    parser.add_argument("--wait", type=float, default=1800)
    args = parser.parse_args()
    deadline = time.monotonic() + args.wait
    while True:
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        connection.settimeout(10)
        try:
            connection.connect(str(args.socket))
            break
        except (FileNotFoundError, ConnectionRefusedError):
            connection.close()
            if time.monotonic() >= deadline:
                raise RuntimeError("QMP socket did not become ready")
            time.sleep(0.02)
    with connection, connection.makefile("rw") as stream:
        version = enable(stream, args.log.resolve())
    args.log.with_suffix(".json").write_text(json.dumps({
        "version": version,
        "events": EVENTS,
        "log": str(args.log),
        "enabledAt": time.time(),
        "guestFeature": "gpu_queue_trace",
        "hostPatchesAdded": False,
    }, indent=2) + "\n")
    print("GPU host tracing enabled: " + str(args.log), flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        sys.exit(str(error))
