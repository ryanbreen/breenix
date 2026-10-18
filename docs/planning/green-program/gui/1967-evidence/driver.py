#!/usr/bin/env python3
"""Scratch test equipment (not in the repo): move the tablet pointer across the
guest screen through QMP while the boot tests run, and take screendumps.

    driver.py QMP_SOCKET SERIAL_GLOB OUT_DIR
"""
import glob
import json
import math
import os
import socket
import sys
import time

sock_path, serial_glob, out_dir = sys.argv[1:4]
W, H = 1280, 800
ABS = 32768


def log(msg):
    print(time.strftime("%H:%M:%S"), msg, flush=True)


class Qmp:
    def __init__(self, path):
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.connect(path)
        self.f = self.s.makefile("rwb", buffering=0)
        self.f.readline()
        self.cmd("qmp_capabilities")

    def cmd(self, name, **args):
        msg = {"execute": name}
        if args:
            msg["arguments"] = args
        self.f.write((json.dumps(msg) + "\n").encode())
        while True:
            line = self.f.readline()
            if not line:
                raise EOFError("QMP closed")
            reply = json.loads(line)
            if "return" in reply or "error" in reply:
                return reply

    def move(self, px, py):
        x = min(ABS - 1, max(0, int(px * ABS / W)))
        y = min(ABS - 1, max(0, int(py * ABS / H)))
        return self.cmd("input-send-event", events=[
            {"type": "abs", "data": {"axis": "x", "value": x}},
            {"type": "abs", "data": {"axis": "y", "value": y}},
        ])


def serial_text():
    paths = glob.glob(serial_glob)
    if not paths:
        return ""
    with open(max(paths, key=os.path.getmtime), "rb") as f:
        return f.read().decode("utf-8", "replace")


log(f"waiting for {sock_path}")
deadline = time.time() + 5 * 3600
while not os.path.exists(sock_path):
    if time.time() > deadline:
        sys.exit("socket never appeared")
    time.sleep(1)
for _ in range(50):
    try:
        q = Qmp(sock_path)
        break
    except OSError:
        time.sleep(0.2)
else:
    sys.exit("could not connect")
log("connected")

while "[BOOT_TESTS:START]" not in serial_text():
    time.sleep(0.2)
log("boot tests started")

shot = 0
moves = 0
events = []
t0 = time.time()


def sweep(seconds, t_offset):
    global moves
    end = time.time() + seconds
    k = 0
    while time.time() < end:
        t = t_offset + k * 0.035
        px = W / 2 + (W / 2 - 20) * math.sin(1.3 * t)
        py = H / 2 + (H / 2 - 30) * math.sin(2.1 * t + 0.4)
        q.move(px, py)
        moves += 1
        k += 1
        time.sleep(0.025)
    return t_offset + k * 0.035


def settle_and_dump(px, py, tag):
    global shot
    q.move(px, py)
    time.sleep(0.6)
    name = f"{shot:03d}-{tag}"
    path = os.path.join(out_dir, name + ".ppm")
    reply = q.cmd("screendump", filename=path, format="ppm")
    events.append({"shot": name, "t": round(time.time() - t0, 2), "pointer": [px, py],
                   "reply": "error" if "error" in reply else "ok",
                   "serial_tail": serial_text().count("\n")})
    log(f"dump {name} pointer=({px},{py}) {reply}")
    shot += 1


try:
    t = 0.0
    parks = [(300, 600), (900, 620), (500, 300), (1100, 200), (200, 700), (700, 560)]
    i = 0
    while True:
        done = "[TESTS_COMPLETE" in serial_text()
        t = sweep(1.5, t)
        px, py = parks[i % len(parks)]
        settle_and_dump(px, py, "after-tests" if done else "during")
        i += 1
        if done:
            break
        if time.time() - t0 > 600:
            log("tests did not complete within 600 s of starting")
            break

    # One more sweep over everything after the run, then a final settled frame.
    t = sweep(4.0, t)
    settle_and_dump(640, 650, "final")
except (EOFError, OSError) as e:
    log(f"QMP ended: {e!r}")
with open(os.path.join(out_dir, "driver.json"), "w") as f:
    json.dump({"moves": moves, "events": events}, f, indent=1)
log(f"done: {moves} pointer moves, {shot} screendumps")
