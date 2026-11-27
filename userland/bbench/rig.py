#!/usr/bin/env python3
"""The ARM64 QEMU performance rig (DEV-31): Breenix and stock Linux in one VM shape.

Both guests boot in the shape below on this Mac, at the same time, and bbench's
samples alternate between them (Linux, Breenix, Linux, ...) case by case, so host
drift lands on both. Each run writes one schema-v1 series per system.

  rig.py fetch-linux                  download and verify the pinned image, make the raw disk
  rig.py provision-linux              first boot: account, compiler, rustup toolchain
  rig.py run --run ID [--count N] [--cases a,b] [--headed]

Run it through scripts/bench-qemu-arm64.sh, which holds the host's QEMU slot.
Linux is a black box here: the rig boots the distribution's own kernel unmodified,
runs bbench built from this checkout and reads its timings and counters.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import signal
import socket
import socketserver
import subprocess
import sys
import threading
import time
import tomllib
import urllib.request

from series import write_series
from tables import collect_receipts

ROOT = Path(__file__).resolve().parents[2]
BENCH = Path(__file__).resolve().parent
CACHE = Path.home() / "Library/Caches/breenix-bench"
LINUX_CACHE = CACHE / "linux-arm64"
SERIES_DIR = Path.home() / "Library/Caches/breenix-devbox/bench-series"
PIN = json.loads(BENCH.joinpath("linux-arm64.json").read_text())
SYSTEMS = ("linux", "breenix")
RUNS = 5
MIB = 1024 * 1024

# The repository's standard ARM64 Breenix QEMU shape (run.sh and the production-
# profile boot test): virt with GICv3, scripts/qemu-arm64.py's accelerator (HVF
# with -cpu host on Apple Silicon), four CPUs, 512 MiB, one raw virtio-blk-device
# at QEMU's default cache mode (writeback), virtio-net-device on user networking,
# virtio-rng and the virtio input and GPU devices the kernel enumerates.
CPUS = 4
MEMORY = "512M"
# Linux's root holds the distribution, gcc and the toolchain (2.8 GB on a 4 GiB disk),
# and the sequential row writes 1 GiB, so both guests get 8 GiB.
DISK_BYTES = 8192 * MIB
SHAPE_ARGS = [
    "-M", "virt,gic-version=3", "-cpu", "max", "-smp", str(CPUS), "-m", MEMORY,
    "-device", "virtio-gpu-device", "-device", "virtio-keyboard-device",
    "-device", "virtio-tablet-device", "-device", "virtio-rng-device",
]
EDK2 = Path("/opt/homebrew/share/qemu/edk2-aarch64-code.fd")
# The upstream nightly the Breenix Rust fork (rust-fork.toml, a763a7dd7) is 16 commits
# ahead of and 0 behind (GitHub compare). It also builds Breenix's userspace.
FORK_BASE = {"channel": "nightly-2025-06-24",
             "commit": "706f244db581212cabf2e619e0113d70999b2bbe"}
LINUX_HOME = "/home/bench"
LINUX_TOOLCHAIN = (f"{LINUX_HOME}/.rustup/toolchains/{FORK_BASE['channel']}"
                   "-aarch64-unknown-linux-gnu/bin")

BREENIX_KERNEL = ROOT / "target/aarch64-breenix-kernel/release/kernel-aarch64"
BREENIX_IMAGE = ROOT / "target/ext2-aarch64.img"
BREENIX_RELEASE = ROOT / "userspace/programs/target/aarch64-breenix/release"
BREENIX_O3 = ROOT / "userspace/programs/target/bench-o3/aarch64-breenix/release"
BREENIX_KEY = CACHE / "breenix-arm64/test-ssh/id_ed25519"
LINUX_KEY = CACHE / "rig-ssh/id_ed25519"

COMPILE = ("rustc-version", "hello-object", "cargo-noop", "yes-dev-box", "yes-release")
SELFHOST = ("bobj", "bld", "bmake", "bcc")
# Riskiest last: a guest that falls over in the disk rows has run everything else.
CASES = (*COMPILE, "fork-exec", "getpid", "malloc-1t", "malloc-4t", "anonymous", "file-map",
         "copy-bytes", "aes-gcm-oz", "aes-gcm-o3", "hmac-sha256-oz", "hmac-sha256-o3", "network",
         "tcp-send", "tcp-recv", "fsync", "small-files", "walk", "sequential")
NO_BREENIX_RUSTC = ("No aarch64 Breenix-hosted rustc or Cargo in the standard image; "
                    "the hosted ARM toolchain is staged only by the rustc and cargo suite boots")
SKIP = {"breenix": set(COMPILE), "linux": set()}
DEFERRED = {
    "breenix": {**{case: NO_BREENIX_RUSTC for case in COMPILE},
                **{case: "The QEMU rig does not build the toolchain workspace" for case in SELFHOST},
                "bsync-upload": "bsync is measured on the R240 only"},
    "linux": {**{case: "The QEMU rig does not build the toolchain workspace" for case in SELFHOST},
              "bsync-upload": "bsync is measured on the R240 only"},
}
NETWORK = ("network", "tcp-send", "tcp-recv")
# The rows the composite network case writes, one receipt each per sample.
NETWORK_PARTS = ("scp-upload", "download", "ssh")
TCP_BYTES = 32_000_000
CRYPTO_BYTES = 8 * MIB
SCP_BYTES = 8_000_000
FIXTURE_BYTES = 64 * MIB


def schedule(cases, count, systems, skip):
    """Samples in run order: per case, sample n on each system in turn (A/B/A/B)."""
    order = []
    for case in cases:
        runs_on = [system for system in systems if case not in skip.get(system, ())]
        samples = 1 if case == "bcc" else count
        order += [(case, number, system) for number in range(1, samples + 1) for system in runs_on]
    return order


def sha256(path):
    with open(path, "rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def download(url, destination):
    request = urllib.request.Request(url, headers={"User-Agent": "breenix-bench-rig"})
    with urllib.request.urlopen(request, timeout=60) as response, open(destination, "xb") as out:
        shutil.copyfileobj(response, out, 1 << 20)


def fetch_image(pin, cache, fetch=download):
    """Return the pinned image in `cache`, downloading it if needed. A download is
    kept only once its SHA-256 matches the pin; a cached copy is checked again."""
    cache.mkdir(parents=True, exist_ok=True)
    final = cache / pin["file"]
    if final.exists():
        actual = sha256(final)
        if actual != pin["sha256"]:
            raise ValueError(f"cached {final} has SHA-256 {actual}, pinned {pin['sha256']}; "
                             "move it aside and fetch again")
        return final
    partial = final.with_name(final.name + ".partial")
    partial.unlink(missing_ok=True)
    try:
        fetch(pin["url"], partial)
        actual = sha256(partial)
        if actual != pin["sha256"]:
            raise ValueError(f"{pin['url']} has SHA-256 {actual}, pinned {pin['sha256']}")
        partial.rename(final)
    finally:
        partial.unlink(missing_ok=True)
    return final


def run_command(words, **kwargs):
    return subprocess.run([str(word) for word in words], check=True, **kwargs)


def git(*args):
    return subprocess.run(["git", "-C", ROOT, *args], check=True, capture_output=True,
                          text=True).stdout.strip()


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def qemu_version():
    text = subprocess.run(["qemu-system-aarch64", "--version"], capture_output=True, text=True)
    return text.stdout.splitlines()[0].removeprefix("QEMU emulator version ").strip()


def host_description():
    model = subprocess.run(["sysctl", "-n", "hw.model"], capture_output=True, text=True).stdout
    chip = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True,
                          text=True).stdout
    return f"{chip.strip()} ({model.strip()}), macOS {platform.mac_ver()[0]}"


def hardware():
    return (f"QEMU {qemu_version()} virt,gic-version=3, HVF -cpu host, {CPUS} vCPUs, "
            f"{MEMORY[:-1]} MiB, one {DISK_BYTES // MIB} MiB raw virtio-blk-device at QEMU's "
            f"default cache (writeback), virtio-net-device on user networking; host "
            f"{host_description()}")


class Guest:
    """One VM in the rig's shape, reached over SSH on a forwarded loopback port."""

    def __init__(self, system, run_dir, disk, headed, extra=()):
        self.system = system
        self.run_dir = run_dir
        self.disk = disk
        self.headed = headed
        self.extra = list(extra)
        self.port = None
        self.process = None
        self.boots = 0
        self.key = LINUX_KEY if system == "linux" else BREENIX_KEY
        self.user = "bench" if system == "linux" else "sshtest"
        self.home = LINUX_HOME if system == "linux" else "/etc/breenix/test-ssh/home"
        self.known_hosts = run_dir / f"known_hosts-{system}"

    def argv(self):
        boot = (["-bios", EDK2] if self.system == "linux" else ["-kernel", BREENIX_KERNEL])
        return [
            sys.executable, ROOT / "scripts/qemu-arm64.py", "qemu-system-aarch64", *SHAPE_ARGS,
            *boot, "-display", "cocoa" if self.headed else "none",
            "-device", "virtio-blk-device,drive=disk0",
            "-drive", f"if=none,id=disk0,format=raw,file={self.disk}",
            *self.extra,
            "-device", "virtio-net-device,netdev=net0",
            "-netdev", f"user,id=net0,hostfwd=tcp:127.0.0.1:{self.port}-:22",
            "-serial", f"file:{self.run_dir / f'serial-{self.system}-{self.boots}.txt'}",
            "-no-reboot",
        ]

    def start(self, wait=240):
        self.boots += 1
        self.port = free_port()
        log = (self.run_dir / f"qemu-{self.system}-{self.boots}.log").open("wb")
        argv = [str(word) for word in self.argv()]
        (self.run_dir / f"qemu-{self.system}-{self.boots}.argv").write_text(shlex.join(argv) + "\n")
        self.process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        (self.run_dir / f"qemu-{self.system}-{self.boots}.pid").write_text(f"{self.process.pid}\n")
        print(f"{self.system}: QEMU pid {self.process.pid}, SSH port {self.port}", flush=True)
        deadline = time.monotonic() + wait
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"{self.system} QEMU exited with {self.process.returncode}")
            if self.ssh(["/bin/true"], timeout=15, check=False).returncode == 0:
                return
            time.sleep(2)
        raise RuntimeError(f"{self.system} did not answer SSH within {wait} s")

    def alive(self):
        return self.process is not None and self.process.poll() is None

    def stop(self):
        if not self.alive():
            return
        if self.system == "linux":
            self.ssh(["sudo", "/sbin/poweroff"], timeout=30, check=False)
            try:
                self.process.wait(60)
                return
            except subprocess.TimeoutExpired:
                pass
        # Only the QEMU this rig started, by the pid it recorded.
        self.process.send_signal(signal.SIGTERM)
        try:
            self.process.wait(30)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()

    def ssh_options(self):
        return ["-i", self.key, "-o", "IdentitiesOnly=yes", "-o", "BatchMode=yes",
                "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=15",
                "-o", "ServerAliveCountMax=8", "-o", "StrictHostKeyChecking=accept-new",
                "-o", f"UserKnownHostsFile={self.known_hosts}", "-o", "LogLevel=ERROR"]

    def ssh_argv(self, words):
        command = words if isinstance(words, str) else shlex.join(map(str, words))
        return ["/usr/bin/ssh", *self.ssh_options(), "-p", str(self.port),
                f"{self.user}@127.0.0.1", command]

    def ssh(self, words, timeout=600, check=True, **kwargs):
        argv = [str(word) for word in self.ssh_argv(words)]
        try:
            result = subprocess.run(argv, capture_output=True, timeout=timeout, **kwargs)
        except subprocess.TimeoutExpired as error:
            if check:
                raise
            return subprocess.CompletedProcess(argv, 124, error.stdout or b"", error.stderr or b"")
        if check and result.returncode != 0:
            raise RuntimeError(f"{self.system}: {argv[-1]} exited {result.returncode}: "
                               f"{result.stderr.decode(errors='replace').strip()}")
        return result

    def scp(self, source, destination, timeout=600):
        return subprocess.run(["/usr/bin/scp", *map(str, self.ssh_options()), "-P",
                               str(self.port), str(source), str(destination)],
                              capture_output=True, timeout=timeout)

    def remote(self, path):
        return f"{self.user}@127.0.0.1:{path}"


# ---------------------------------------------------------------- Linux image


def fetch_linux():
    image = fetch_image(PIN, LINUX_CACHE)
    base = LINUX_CACHE / f"base-{PIN['sha256'][:12]}-{DISK_BYTES // MIB}.raw"
    if not base.exists():
        partial = base.with_name(base.name + ".partial")
        partial.unlink(missing_ok=True)
        run_command(["qemu-img", "convert", "-f", PIN["format"], "-O", "raw", image, partial])
        run_command(["qemu-img", "resize", "-f", "raw", partial, str(DISK_BYTES)])
        partial.rename(base)
    print(f"Linux image {image} verified ({PIN['sha256']}); raw disk {base}")
    return base


def user_data(public_key):
    return "\n".join([
        "#cloud-config",
        "users:",
        "  - name: bench",
        "    shell: /bin/bash",
        "    sudo: ALL=(ALL) NOPASSWD:ALL",
        f"    ssh_authorized_keys: [{json.dumps(public_key.strip())}]",
        "ssh_pwauth: false",
        "",
    ])


def provision_linux():
    base = fetch_linux()
    provisioned = LINUX_CACHE / f"provisioned-{PIN['sha256'][:12]}-{DISK_BYTES // MIB}.raw"
    record = provisioned.with_suffix(".json")
    if provisioned.exists() and record.exists():
        print(f"Linux already provisioned: {provisioned}")
        return provisioned
    if not LINUX_KEY.exists():
        LINUX_KEY.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        run_command(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "breenix-bench-rig",
                     "-f", LINUX_KEY])
    work = LINUX_CACHE / "provision"
    shutil.rmtree(work, ignore_errors=True)
    seed = work / "seed"
    seed.mkdir(parents=True)
    (seed / "meta-data").write_text("instance-id: breenix-bench-rig\nlocal-hostname: bench\n")
    (seed / "user-data").write_text(user_data(LINUX_KEY.with_suffix(".pub").read_text()))
    iso = work / "seed.iso"
    run_command(["hdiutil", "makehybrid", "-quiet", "-iso", "-joliet", "-iso-volume-name",
                 "CIDATA", "-joliet-volume-name", "cidata", "-o", iso, seed])
    disk = provisioned.with_name(provisioned.name + ".partial")
    disk.unlink(missing_ok=True)
    run_command(["cp", "-c", base, disk])
    guest = Guest("linux", work, disk, headed=False, extra=[
        "-device", "virtio-blk-device,drive=seed",
        "-drive", f"if=none,id=seed,format=raw,readonly=on,file={iso}"])
    try:
        guest.start(wait=600)
        steps = [
            "cloud-init status --wait >/dev/null",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get -q update",
            "sudo DEBIAN_FRONTEND=noninteractive apt-get -qy install --no-install-recommends "
            "gcc libc6-dev",
            # Quiet the host during measurements: no package downloads or upgrades.
            "sudo systemctl disable --now apt-daily.timer apt-daily-upgrade.timer "
            "unattended-upgrades.service",
            "sudo touch /etc/cloud/cloud-init.disabled",
            "curl -sSfLo /tmp/rustup-init "
            "https://static.rust-lang.org/rustup/dist/aarch64-unknown-linux-gnu/rustup-init",
            "curl -sSfLo /tmp/rustup-init.sha256 "
            "https://static.rust-lang.org/rustup/dist/aarch64-unknown-linux-gnu/rustup-init.sha256",
            "cd /tmp && sed 's| .*|  rustup-init|' rustup-init.sha256 | sha256sum -c -",
            f"chmod +x /tmp/rustup-init && /tmp/rustup-init -y -q --profile minimal "
            f"--default-toolchain {FORK_BASE['channel']}",
        ]
        for step in steps:
            print(f"linux: {step}", flush=True)
            guest.ssh(step, timeout=1800)
        identity = guest.ssh(f"{LINUX_TOOLCHAIN}/rustc -vV").stdout.decode()
        if f"commit-hash: {FORK_BASE['commit']}" not in identity:
            raise RuntimeError(f"guest rustc is not {FORK_BASE['commit']}:\n{identity}")
        facts = {
            "image": PIN, "rustc": identity.splitlines(),
            "cargo": guest.ssh(f"{LINUX_TOOLCHAIN}/cargo -V").stdout.decode().strip(),
            "kernel": guest.ssh("uname -srvm").stdout.decode().strip(),
            "gcc": guest.ssh("gcc --version").stdout.decode().splitlines()[0],
            "provisioned": datetime.now(timezone.utc).isoformat(),
        }
    finally:
        guest.stop()
    disk.rename(provisioned)
    record.write_text(json.dumps(facts, indent=2) + "\n")
    print(f"Linux provisioned: {provisioned}")
    return provisioned


# ---------------------------------------------------------------- the run


class TcpEndpoint(socketserver.ThreadingTCPServer):
    """Host end of bbench's raw TCP workload (attr_work::tcp)."""
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self):
        super().__init__(("127.0.0.1", 0), TcpHandler)


class TcpHandler(socketserver.BaseRequestHandler):
    def handle(self):
        request = self.read(9)
        if request is None:
            return
        count = int.from_bytes(request[1:], "big")
        if request[:1] == b"S":
            received = 0
            while received < count:
                data = self.request.recv(min(1 << 20, count - received))
                if not data:
                    break
                received += len(data)
            self.request.sendall(received.to_bytes(8, "big"))
        elif request[:1] == b"R":
            chunk = bytes((i * 31 % 251) for i in range(1 << 16))
            remaining = count
            while remaining:
                part = chunk[:min(len(chunk), remaining)]
                self.request.sendall(part)
                remaining -= len(part)
            self.request.shutdown(socket.SHUT_WR)
            while self.request.recv(1 << 16):
                pass

    def read(self, size):
        data = b""
        while len(data) < size:
            part = self.request.recv(size - len(data))
            if not part:
                return None
            data += part
        return data


def failure_receipt(sample, words, error):
    return {"sample": sample, "argv": list(map(str, words)), "wall_ns": 0, "metrics": {},
            "error": error, "before": {}, "after": {}}


class Rig:
    def __init__(self, args):
        self.args = args
        self.run_dir = CACHE / "runs" / args.run
        self.run_dir.mkdir(parents=True, exist_ok=False)
        self.out = {system: self.run_dir / system for system in SYSTEMS}
        for path in self.out.values():
            path.mkdir()
        self.endpoint = TcpEndpoint()
        threading.Thread(target=self.endpoint.serve_forever, daemon=True).start()
        self.guests = {}
        self.notes = {system: [] for system in SYSTEMS}
        self.started = datetime.now(timezone.utc).replace(microsecond=0)

    def bin(self, system, name):
        return f"{self.guests[system].home}/bbench/bin/{name}"

    def dirs(self, system):
        home = self.guests[system].home
        return f"{home}/bbench/data/{self.args.run}", f"{home}/bbench/reports"

    def boot(self):
        commit = git("rev-parse", "HEAD")
        disk = self.run_dir / "breenix-disk.raw"
        run_command(["cp", "-c", BREENIX_IMAGE, disk])
        self.guests["breenix"] = Guest("breenix", self.run_dir, disk, self.args.headed)
        linux_disk = self.run_dir / "linux-disk.raw"
        run_command(["cp", "-c", LINUX_CACHE / f"provisioned-{PIN['sha256'][:12]}-{DISK_BYTES // MIB}.raw",
                     linux_disk])
        self.guests["linux"] = Guest("linux", self.run_dir, linux_disk, self.args.headed)
        for system in SYSTEMS:
            self.guests[system].start()
            self.setup(system)
        return commit

    def setup(self, system):
        guest = self.guests[system]
        data, reports = self.dirs(system)
        home = guest.home
        guest.ssh(["/bin/mkdir", "-p", f"{home}/bbench/bin", data, reports, f"{home}/bbench/net"])
        if system == "breenix":
            for source, name in ((BREENIX_RELEASE / "bbench", "bbench"),
                                 (BREENIX_RELEASE / "bbench-exit", "bbench-exit"),
                                 (BREENIX_O3 / "bbench", "bbench-o3")):
                result = guest.scp(source, guest.remote(f"{home}/bbench/bin/{name}"))
                if result.returncode:
                    raise RuntimeError(f"breenix: upload of {name} failed: {result.stderr!r}")
                if sha256(source) != guest.ssh(["/bin/sha256sum", f"{home}/bbench/bin/{name}"]
                                               ).stdout.decode().split()[0]:
                    raise RuntimeError(f"breenix: {name} arrived damaged")
            guest.ssh(["/bin/chmod", "755", *(f"{home}/bbench/bin/{name}"
                                              for name in ("bbench", "bbench-exit", "bbench-o3"))])
        else:
            self.sync_source(guest)
            cargo = (f"cd {home}/src/userland && PATH={LINUX_TOOLCHAIN}:/usr/bin:/bin "
                     f"{LINUX_TOOLCHAIN}/cargo build -q --offline --frozen -p bbench")
            guest.ssh(f"{cargo} --profile release --target-dir {home}/bbench/target", timeout=1800)
            guest.ssh(f"{cargo} --profile bench-o3 --target-dir {home}/bbench/target", timeout=1800)
            guest.ssh(f"cp {home}/bbench/target/release/bbench {home}/bbench/target/release/bbench-exit "
                      f"{home}/bbench/bin/ && cp {home}/bbench/target/bench-o3/bbench "
                      f"{home}/bbench/bin/bbench-o3")
        fixture = f"{home}/bbench/fixture.bin"
        made = self.bbench(system, f"{self.args.run}-fixture-boot{guest.boots}",
                           ["fixture", fixture, FIXTURE_BYTES])
        if made["error"]:
            raise RuntimeError(f"{system}: fixture: {made['error']}")

    def sync_source(self, guest):
        manifest = tomllib.loads((ROOT / "libs/libbreenix/Cargo.toml").read_text())
        targets = [ROOT / "libs/libbreenix" / entry["path"]
                   for kind in ("test", "bin", "example", "bench") for entry in manifest.get(kind, [])
                   if "path" in entry]
        paths = ["userland", "libs/libbreenix", "libs/input-abi", "libs/breengel-abi",
                 "libs/gpu-abi", "toolchain/rust-host/k5/corpus",
                 *sorted({str(path.resolve().relative_to(ROOT)) for path in targets})]
        archive = subprocess.run(["git", "-C", ROOT, "archive", "--format=tar", "HEAD", *paths],
                                 check=True, capture_output=True).stdout
        guest.ssh(f"rm -rf {guest.home}/src && mkdir -p {guest.home}/src && "
                  f"tar -xf - -C {guest.home}/src", input=archive)

    def bbench(self, system, sample, words, binary="bbench", timeout=900):
        guest = self.guests[system]
        data, reports = self.dirs(system)
        argv = [self.bin(system, binary), data, reports, sample, *words]
        if system == "linux":
            # Cargo finds rustc on PATH; name the toolchain's own binaries, not rustup's proxies.
            argv = ["/usr/bin/env", f"PATH={LINUX_TOOLCHAIN}:/usr/bin:/bin", *argv]
        result = guest.ssh(argv, timeout=timeout, check=False)
        payload = result.stdout
        if result.returncode == 255 and not payload and guest.alive():
            payload = guest.ssh(["/bin/cat", f"{reports}/{sample}.json"], check=False).stdout
        try:
            value = json.loads(payload)
        except ValueError:
            reason = ("timed out" if result.returncode == 124 else
                      f"no receipt (ssh exit {result.returncode}: "
                      f"{result.stderr.decode(errors='replace').strip()[:300]})")
            if not guest.alive():
                reason = f"{system} VM stopped during the sample"
            return failure_receipt(sample, words, reason)
        if value.get("sample") != sample:
            return failure_receipt(sample, words, "receipt names another sample")
        return value

    def workload(self, system, case, sample):
        guest = self.guests[system]
        home = guest.home
        data, _ = self.dirs(system)
        # The host's loopback, as user networking presents it to the guest.
        tcp = f"10.0.2.2:{self.endpoint.server_address[1]}"

        def build(profile, target):
            return ["command", f"{home}/src/userland", f"{LINUX_TOOLCHAIN}/cargo", "build",
                    "--offline", "--frozen", "--profile", profile, "-p", "yes",
                    "--target-dir", target]

        return {
            "rustc-version": lambda: ["command", data, f"{LINUX_TOOLCHAIN}/rustc", "-vV"],
            "hello-object": lambda: ["command", f"{home}/src/toolchain/rust-host/k5/corpus",
                                     f"{LINUX_TOOLCHAIN}/rustc", "--edition=2021", "-Dwarnings",
                                     "--emit=obj", "--crate-type=lib", "hello.rs", "-o",
                                     f"{data}/{sample}.o"],
            "cargo-noop": lambda: build("dev-box", f"{data}/noop-artifacts"),
            "yes-dev-box": lambda: build("dev-box", f"{data}/{sample}-artifacts"),
            "yes-release": lambda: build("release", f"{data}/{sample}-artifacts"),
            "fork-exec": lambda: ["fork-exec", self.bin(system, "bbench-exit"), 100],
            "getpid": lambda: ["getpid", 1_000_000],
            "malloc-1t": lambda: ["malloc", 1, 1_000_000],
            "malloc-4t": lambda: ["malloc", 4, 1_000_000],
            "anonymous": lambda: ["anonymous"],
            "file-map": lambda: ["file-map", f"{home}/bbench/fixture.bin"],
            "copy-bytes": lambda: ["copy-bytes"],
            "aes-gcm-oz": lambda: ["aes-gcm", CRYPTO_BYTES],
            "aes-gcm-o3": lambda: ["aes-gcm", CRYPTO_BYTES],
            "hmac-sha256-oz": lambda: ["hmac-sha256", CRYPTO_BYTES],
            "hmac-sha256-o3": lambda: ["hmac-sha256", CRYPTO_BYTES],
            "tcp-send": lambda: ["tcp", "send", tcp, TCP_BYTES],
            "tcp-recv": lambda: ["tcp", "recv", tcp, TCP_BYTES],
            "sequential": lambda: ["sequential", 1024 * MIB],
            "small-files": lambda: ["small-files", 10_000],
            "walk": lambda: ["walk", 10_000],
            "fsync": lambda: ["fsync", 32],
        }[case]()

    def sample(self, case, number, system):
        guest = self.guests[system]
        name = f"{self.args.run}-{case}-{number}"
        print(f"{system} {name}", flush=True)
        if not guest.alive():
            self.restart(system)
        if not guest.alive():
            # The network case is three rows; each one records the lost attempt.
            for part in (NETWORK_PARTS if case == "network" else (case,)):
                sample = f"{self.args.run}-{part}-{number}"
                self.save(system, sample, failure_receipt(sample, [part],
                                                          f"{system} VM could not be restarted"))
            return
        if case == "network":
            try:
                self.network(system, number)
            except (RuntimeError, subprocess.TimeoutExpired, IndexError) as error:
                for part in NETWORK_PARTS:
                    sample = f"{self.args.run}-{part}-{number}"
                    if not (self.out[system] / f"{sample}.json").exists():
                        self.save(system, sample, failure_receipt(sample, [part], str(error)))
                if not guest.alive() or guest.ssh(["/bin/true"], timeout=30,
                                                  check=False).returncode != 0:
                    self.restart(system)
            return
        if case == "cargo-noop" and number == 1:
            seed = f"{self.args.run}-noop-seed"
            self.save(system, seed, self.bbench(system, seed, self.workload(system, case, seed)))
        binary = "bbench-o3" if case.endswith("-o3") else "bbench"
        receipt = self.bbench(system, name, self.workload(system, case, name), binary=binary)
        self.save(system, name, receipt)
        if receipt["error"]:
            print(f"{system} {name} failed: {receipt['error']}", flush=True)
            # A guest that stopped, or no longer answers SSH, is booted again.
            if not guest.alive() or guest.ssh(["/bin/true"], timeout=30,
                                              check=False).returncode != 0:
                self.restart(system)

    def restart(self, system):
        guest = self.guests[system]
        if guest.boots > 2:
            guest.stop()
            return
        tail = self.serial_tail(system)
        self.notes[system].append(f"VM restarted after boot {guest.boots}")
        print(f"{system}: VM down; serial tail:\n{tail}", flush=True)
        guest.stop()
        try:
            guest.start()
            self.setup(system)
        except RuntimeError as error:
            print(f"{system}: restart failed: {error}", flush=True)

    def serial_tail(self, system):
        path = self.run_dir / f"serial-{system}-{self.guests[system].boots}.txt"
        return path.read_bytes()[-2000:].decode(errors="replace") if path.exists() else ""

    def save(self, system, sample, receipt):
        path = self.out[system] / f"{sample}.json"
        path.write_text(json.dumps(receipt) + "\n")

    def network(self, system, number):
        guest = self.guests[system]
        payload = self.run_dir / "network-payload"
        if not payload.exists():
            payload.write_bytes(os.urandom(SCP_BYTES))
        digest = sha256(payload)
        remote = f"{guest.home}/bbench/net/{number}"
        guest.ssh(["/bin/mkdir", "-p", remote])
        returned = self.run_dir / f"{system}-download-{number}"
        commands = {
            "scp-upload": lambda: guest.scp(payload, guest.remote(f"{remote}/payload")),
            "download": lambda: guest.scp(guest.remote(f"{remote}/payload"), returned),
            "ssh": lambda: subprocess.run([str(w) for w in guest.ssh_argv("true")],
                                          capture_output=True, timeout=120),
        }
        for case, action in commands.items():
            sample = f"{self.args.run}-{case}-{number}"
            start = time.monotonic_ns()
            try:
                result = action()
                error = f"exit status: {result.returncode}" if result.returncode else None
            except subprocess.TimeoutExpired:
                error = "timed out"
            elapsed = time.monotonic_ns() - start
            metrics = {}
            if case != "ssh" and not error:
                if case == "download":
                    actual = sha256(returned)
                    returned.unlink()
                else:
                    actual = guest.ssh(["/bin/sha256sum", f"{remote}/payload"]
                                       ).stdout.decode().split()[0]
                error = None if actual == digest else "payload SHA-256 mismatch"
                metrics = {"bytes": SCP_BYTES, "sha256": actual}
            self.save(system, sample, {"sample": sample, "argv": [case], "wall_ns": elapsed,
                                       "metrics": metrics, "error": error, "before": {},
                                       "after": {}})
        guest.ssh(["/bin/rm", "-rf", remote], check=False)

    def tcp_counters(self, system):
        guest = self.guests[system]
        if not guest.alive():
            return None
        paths = (["/proc/net/snmp", "/proc/net/netstat"] if system == "linux"
                 else ["/proc/net/tcp_stats"])
        text = guest.ssh(["/bin/cat", *paths], check=False)
        if text.returncode:
            return {"error": text.stderr.decode(errors="replace").strip()}
        return parse_tcp_counters(system, text.stdout.decode())

    def run(self):
        commit = self.boot()
        before = {system: self.tcp_counters(system) for system in SYSTEMS}
        cases = self.args.cases.split(",") if self.args.cases else list(CASES)
        unknown = set(cases) - set(CASES)
        if unknown:
            raise ValueError(f"unknown cases: {', '.join(sorted(unknown))}")
        order = schedule(cases, self.args.count, SYSTEMS, SKIP)
        # Read the retransmission counters right after the last network sample, before
        # a later row can restart a guest and reset its kernel's counters.
        network = [index for index, (case, _, _) in enumerate(order) if case in NETWORK]
        last_network = network[-1] if network else len(order) - 1
        after = None
        for index, (case, number, system) in enumerate(order):
            self.sample(case, number, system)
            if index == last_network:
                after = {system: self.tcp_counters(system) for system in SYSTEMS}
        after = after or dict.fromkeys(SYSTEMS)
        facts = {system: self.facts(system) for system in SYSTEMS}
        for guest in self.guests.values():
            guest.stop()
        return [self.write(system, commit, before[system], after[system], facts[system])
                for system in SYSTEMS]

    def facts(self, system):
        guest = self.guests[system]
        if not guest.alive():
            return {}
        if system == "linux":
            kernel = guest.ssh("uname -srv", check=False).stdout.decode().strip()
            toolchain = guest.ssh(f"{LINUX_TOOLCHAIN}/rustc -V", check=False).stdout.decode().strip()
            return {"kernel": kernel, "toolchain": toolchain}
        return {}

    def write(self, system, commit, before, after, facts):
        groups = collect_receipts(self.out[system], [self.args.run], "scp-upload")
        short = commit[:8]
        if system == "linux":
            kernel = facts.get("kernel") or "unknown"
            toolchain = facts.get("toolchain") or f"rustc {FORK_BASE['channel']}"
            note = (f"Stock {PIN['distribution']} ({PIN['url']}, SHA-256 {PIN['sha256']}), "
                    f"kernel unmodified; bbench built in the guest from {short} with "
                    f"{FORK_BASE['channel']} ({FORK_BASE['commit'][:9]}), the upstream base of "
                    "the Breenix Rust fork (a763a7dd7, 16 commits ahead). "
                    "apt timers and cloud-init are disabled after provisioning.")
        else:
            kernel = short
            toolchain = (f"rustc {FORK_BASE['channel']} ({FORK_BASE['commit'][:9]}) with the "
                         "Breenix std fork via build-std")
            note = (f"Production kernel and system image at {short}; bbench uploaded over SSH "
                    "and run as sshtest on the root ext2 disk.")
        note += (f" Samples alternate Linux then Breenix per case; both VMs stay up and "
                 f"one runs a sample at a time. Fixture for the driver-map rows: a "
                 f"{FIXTURE_BYTES // MIB} MiB "
                 f"file written in the guest. TCP retransmission counters at boot / after the "
                 f"network rows: "
                 f"{json.dumps(before)} / {json.dumps(after)}.")
        if self.notes[system]:
            note += " " + "; ".join(self.notes[system]) + "."
        metadata = {
            "system": system, "target": "arm64-qemu", "hardware": hardware(),
            "kernelCommit": kernel, "userlandCommit": short, "toolchain": toolchain,
            "session": self.args.run, "pr": self.args.pr, "note": note,
        }
        path = write_series(SERIES_DIR, f"{self.args.run}-{system}",
                            self.started.isoformat().replace("+00:00", "Z"), metadata, groups,
                            deferred=DEFERRED[system])
        print(f"Series: {path}", flush=True)
        return path


def parse_tcp_counters(system, text):
    """Retransmission counters: Linux's Tcp RetransSegs and TcpExt TCPTimeouts, and
    Breenix's /proc/net/tcp_stats lines."""
    if system == "breenix":
        return {key.strip(): int(value) for key, _, value in
                (line.partition(":") for line in text.splitlines()) if value.strip().isdigit()}
    tables = {}
    lines = text.splitlines()
    for header, values in zip(lines, lines[1:]):
        name, _, fields = header.partition(":")
        vname, _, numbers = values.partition(":")
        if name == vname and not fields.split()[0].lstrip("-").isdigit():
            tables[name] = dict(zip(fields.split(), numbers.split()))
    return {"RetransSegs": int(tables.get("Tcp", {}).get("RetransSegs", -1)),
            "TCPTimeouts": int(tables.get("TcpExt", {}).get("TCPTimeouts", -1))}


def ratios(linux, breenix):
    """Per-row Breenix/Linux ratio, chosen so that above 1 means Breenix is slower
    (DEV-28(1)). None unless both systems measured the row."""
    reference = {row["id"]: row for row in linux["rows"]}
    for row in breenix["rows"]:
        other = reference.get(row["id"])
        ratio = None
        if other and row["median"] and other["median"]:
            ratio = (row["median"] / other["median"] if row["better"] == "lower"
                     else other["median"] / row["median"])
        yield {"id": row["id"], "title": row["title"], "unit": row["unit"],
               "breenix": row, "linux": other, "ratio": ratio}


def ratio_table(linux_path, breenix_path):
    linux, breenix = (json.loads(Path(path).read_text()) for path in (linux_path, breenix_path))

    def cell(row):
        if row is None or row["median"] is None:
            return f"{row['state']}: {row['reason']}" if row else "—"
        return (f"{row['median']:.4g} [{row['min']:.4g}, {row['max']:.4g}] "
                f"{row['completed']}/{row['attempts']}")

    lines = ["| Row | Unit | Linux | Breenix | Breenix/Linux |", "| --- | --- | --- | --- | ---: |"]
    for entry in ratios(linux, breenix):
        ratio = f"{entry['ratio']:.2f}" if entry["ratio"] is not None else "—"
        lines.append(f"| {entry['title']} (`{entry['id']}`) | {entry['unit']} | "
                     f"{cell(entry['linux'])} | {cell(entry['breenix'])} | {ratio} |")
    return "\n".join(lines)


def check_breenix_inputs():
    missing = [path for path in (BREENIX_KERNEL, BREENIX_IMAGE, BREENIX_RELEASE / "bbench",
                                 BREENIX_RELEASE / "bbench-exit", BREENIX_O3 / "bbench",
                                 BREENIX_KEY) if not path.exists()]
    if missing:
        raise ValueError("missing Breenix inputs (see scripts/bench-qemu-arm64.sh): "
                         + ", ".join(map(str, missing)))
    if BREENIX_IMAGE.stat().st_size != DISK_BYTES:
        raise ValueError(f"{BREENIX_IMAGE} must be {DISK_BYTES // MIB} MiB, the rig's disk size "
                         f"(scripts/create_ext2_disk.sh --arch aarch64 --size {DISK_BYTES // MIB})")


def cli(argv):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("fetch-linux")
    commands.add_parser("provision-linux")
    table = commands.add_parser("ratios", help="Markdown table of a run's two series")
    table.add_argument("linux", type=Path)
    table.add_argument("breenix", type=Path)
    run = commands.add_parser("run")
    run.add_argument("--run", required=True, help="run ID (letters, digits, hyphens)")
    run.add_argument("--count", type=int, default=RUNS, choices=range(1, RUNS + 1))
    run.add_argument("--cases", help="comma-separated subset of the rig's cases")
    run.add_argument("--headed", action="store_true", help="show each VM in a window")
    run.add_argument("--pr", type=int)
    args = parser.parse_args(argv)
    if args.command == "fetch-linux":
        fetch_linux()
        return 0
    if args.command == "ratios":
        print(ratio_table(args.linux, args.breenix))
        return 0
    if args.command == "provision-linux":
        provision_linux()
        return 0
    if not args.run.replace("-", "").isalnum():
        parser.error("--run must contain only letters, digits or hyphens")
    if git("status", "--porcelain", "--untracked-files=no"):
        parser.error("commit the tree first: the series record its commit")
    check_breenix_inputs()
    provision_linux()
    rig = Rig(args)
    try:
        rig.run()
    finally:
        for guest in rig.guests.values():
            guest.stop()
        rig.endpoint.shutdown()
    return 0


if __name__ == "__main__":
    # A SIGTERM from the launcher unwinds through cli's cleanup, which stops the VMs.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    raise SystemExit(cli(sys.argv[1:]))
