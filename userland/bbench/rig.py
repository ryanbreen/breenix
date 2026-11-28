#!/usr/bin/env python3
"""The ARM64 QEMU performance rig (DEV-31): Breenix and stock Linux in one VM shape.

Both guests boot in the shape below on this Mac, at the same time, and bbench's
samples alternate between them (Linux, Breenix, Linux, ...) case by case, so host
drift lands on both. Each run writes one schema-v1 series per system.

  rig.py fetch-linux                  download and verify the pinned image, make the raw disk
  rig.py provision-linux              first boot: account, compiler, rustup toolchain
  rig.py build-breenix                build the Breenix inputs at HEAD and stamp their commit
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

from series import series_id_problems, write_series
from tables import collect_receipts
import vigil_rules

ROOT = Path(__file__).resolve().parents[2]
BENCH = Path(__file__).resolve().parent
CACHE = Path.home() / "Library/Caches/breenix-bench"
LINUX_CACHE = CACHE / "linux-arm64"
SERIES_DIR = Path.home() / "Library/Caches/breenix-devbox/bench-series"
PIN = json.loads(BENCH.joinpath("linux-arm64.json").read_text())
SYSTEMS = ("linux", "breenix")
RUNS = 5
MIB = 1024 * 1024
GIB = 1024 * MIB

# The repository's standard ARM64 Breenix QEMU shape (run.sh and the production-
# profile boot test): virt with GICv3, scripts/qemu-arm64.py's accelerator (HVF
# with -cpu host on Apple Silicon by default; the series record what it resolved,
# launch_profile), four CPUs, raw virtio-blk-device disks at
# QEMU's default cache mode (writeback), virtio-net-device on user networking,
# virtio-rng and the virtio input and GPU devices the kernel enumerates.
CPUS = 4
# Memory: 512 MiB is the default boot, but the rig runs the compile rows, and the
# suites behind them declare more (docs/suites/*.json minimumGuestMemory): rustc and
# cargo 1024 MiB, and the toolchain effort's self-host acceptance 2048 MiB. Both
# guests get the largest, so neither page cache is starved below the standard.
TOOLCHAIN_SUITES = ("rustc", "cargo", "self-host")


def standard_memory_mib(suites=TOOLCHAIN_SUITES):
    return max(json.loads((ROOT / f"docs/suites/{suite}.json").read_text())
               ["minimumGuestMemory"]["mib"] for suite in suites)


MEMORY = f"{standard_memory_mib()}M"
# Linux's root holds the distribution, gcc and the toolchain (2.8 GB on a 4 GiB disk),
# and the sequential row writes 1 GiB, so both guests get 8 GiB.
DISK_BYTES = 8192 * MIB
# The disk rows run on the filesystem each system targets, as its root: BreenFS (the
# R240's / and /home, and the program's target) and Linux's default ext4. A root it is,
# because BreenFS does not take Breenix's boot-time /home slot (fs/breenfs/boot.rs) and
# explicit mounts beside an ext2 root would leave that root read-only (fs/vfs/boot.rs).
DISK_CASES = ("sequential", "small-files", "walk", "fsync")
ROOT_FS = {"breenix": "breenfs", "linux": "ext4"}
ROOT_CACHE = {
    "breenix": f"BreenFS root on the {DISK_BYTES // GIB} GiB raw virtio-blk disk (writeback)",
    "linux": f"ext4 root on the {DISK_BYTES // GIB} GiB raw virtio-blk disk (writeback)",
}
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
# The standard image, copied and given the hosted toolchain by build-breenix.
STANDARD_IMAGE = ROOT / "target/ext2-aarch64.img"
BREENIX_IMAGE = CACHE / "breenix-arm64/root.raw"
# The hosted aarch64 toolchain, staged by scripts/rust_host_stage.py --suite rustc,cargo
# (the rustc and cargo suite boots' staging): the fork's rustc and driver, its sysroot
# and target specs, K7 Cargo, and bld with libc.a and linker.ld.
BREENIX_TOOLCHAIN = "/usr/libexec/breenix/rustc"
BREENIX_TARGET = f"{BREENIX_TOOLCHAIN}/targets/aarch64-breenix.json"
FORK = json.loads((ROOT / "toolchain/rust-host/k5/aarch64.json").read_text())["fork_commit"]
FORK_AHEAD = 16  # commits of FORK past FORK_BASE (see FORK_BASE)
BREENIX_RELEASE = ROOT / "userspace/programs/target/aarch64-breenix/release"
BREENIX_O3 = ROOT / "userspace/programs/target/bench-o3/aarch64-breenix/release"
BREENIX_KEY = CACHE / "breenix-arm64/test-ssh/id_ed25519"
# bbench with libbreenix (the crypto) at opt-level z, for the -oz crypto rows.
BREENIX_OZ = ROOT / "userspace/programs/target/aarch64-breenix/bench-oz"
BREENIX_ARTIFACTS = (BREENIX_KERNEL, BREENIX_IMAGE, BREENIX_RELEASE / "bbench",
                     BREENIX_RELEASE / "bbench-exit", BREENIX_OZ / "bbench",
                     BREENIX_O3 / "bbench")
# build-breenix records the commit and each artifact's SHA-256; run refuses anything else.
BUILD_STAMP = CACHE / "breenix-arm64/build.json"
# The rustc,cargo suite boot's builds (scripts/boot-interactive.sh) before the image.
BREENIX_BUILD = (
    ["userspace/programs/build.sh", "--arch", "aarch64"],
    ["userspace/programs/build.sh", "--arch", "aarch64", "--bbench"],
    ["userspace/programs/build.sh", "--arch", "aarch64", "--rust-link"],
    ["userspace/programs/build.sh", "--arch", "aarch64", "--hosted-bin", "suite-rustc",
     "--features", "rustc"],
    ["userspace/programs/build.sh", "--arch", "aarch64", "--cargo-suite"],
    ["userspace/programs/build.sh", "--arch", "aarch64", "--rustc"],
    ["userspace/programs/build.sh", "--arch", "aarch64", "--kd-loader"],
    ["scripts/create_ext2_disk.sh", "--arch", "aarch64"],
    ["cargo", "build", "--release", "--target", "aarch64-breenix-kernel.json",
     "-Z", "build-std=core,alloc", "-Z", "build-std-features=compiler-builtins-mem",
     "-p", "kernel", "--bin", "kernel-aarch64"],
)
LINUX_KEY = CACHE / "rig-ssh/id_ed25519"

COMPILE = ("rustc-version", "hello-object", "cargo-noop", "yes-dev-box", "yes-release")
SELFHOST = ("bobj", "bld", "bmake", "bcc")
# Riskiest last: a guest that falls over in the disk rows has run everything else.
CASES = (*COMPILE, "fork-exec", "getpid", "malloc-1t", "malloc-4t", "anonymous", "file-map",
         "copy-bytes", "aes-gcm-oz", "aes-gcm-o3", "hmac-sha256-oz", "hmac-sha256-o3", "network",
         "tcp-send", "tcp-recv", "fsync", "small-files", "walk", "sequential")
SKIP = {"breenix": set(), "linux": set()}
DEFERRED = {
    "breenix": {**{case: "The QEMU rig does not build the toolchain workspace" for case in SELFHOST},
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


def probe_binary(case):
    """The bbench build a case runs: the crypto rows name their opt-level."""
    for level in ("oz", "o3"):
        if case.endswith(f"-{level}"):
            return f"bbench-{level}"
    return "bbench"


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


def launch_profile(argv):
    """The accelerator and CPU scripts/qemu-arm64.py resolves for a guest's `argv` in
    this environment, as the launcher names them ('hvf-host', 'tcg-max')."""
    result = subprocess.run([argv[0], argv[1], "--print-profile", *argv[2:]],
                            capture_output=True, text=True)
    if result.returncode != 0 or not result.stdout.strip():
        raise RuntimeError(f"the QEMU launcher resolved no profile: {result.stderr.strip()}")
    return result.stdout.strip()


def profile_text(profile):
    """'hvf-host' -> 'HVF -cpu host'."""
    accelerator, _, cpu = profile.partition("-")
    return f"{accelerator.upper()} -cpu {cpu}"


def vm_shape(profile):
    """The full VM shape, for the provenance sidecar."""
    return (f"QEMU {qemu_version()} virt,gic-version=3, {profile_text(profile)}, {CPUS} vCPUs, "
            f"{MEMORY[:-1]} MiB, one {DISK_BYTES // MIB} MiB raw virtio-blk-device root disk at "
            f"cache=writeback, QEMU's default ({ROOT_CACHE['breenix']}; {ROOT_CACHE['linux']}), "
            f"virtio-net-device on user networking; host {host_description()}")


def hardware(profile):
    """The series' one-line hardware field (Vigil allows 200 characters)."""
    return (f"QEMU {qemu_version()} virt GICv3, {profile_text(profile)}, {CPUS} vCPUs, "
            f"{MEMORY[:-1]} MiB, "
            f"{DISK_BYTES // GIB} GiB raw virtio-blk (writeback), virtio-net user; "
            f"{host_description()}")


def root_problem(system, mounts):
    """Why the guest's root (from /proc/mounts) is not the filesystem the disk rows name,
    read-write; None when it is. The last mount of / is the one in effect."""
    lines = [line.split() for line in mounts.splitlines() if line.split()[1:2] == ["/"]]
    if not lines:
        return f"{system}: no root mount in /proc/mounts"
    fields = lines[-1]
    kind, options = (fields[2:4] + ["", ""])[:2]
    if kind != ROOT_FS[system] or "rw" not in options.split(","):
        return (f"{system}: the root is {' '.join(fields)}, not {ROOT_FS[system]} read-write")
    return None


def kernel_release(uname):
    """'Linux 6.8.0-147-generic #147-Ubuntu SMP ...' -> 'Linux 6.8.0-147-generic'."""
    words = uname.split()
    return " ".join(words[:2]) if words else None


def tcp_summary(system, before, after):
    """Retransmissions during the run, in a few words: the series note's share of the
    counters (the sidecar keeps them all)."""
    keys = ("RetransSegs", "TCPTimeouts") if system == "linux" else ("retransmit_timeouts",)
    if not isinstance(before, dict) or not isinstance(after, dict):
        return "TCP counters unread"
    parts = []
    for key in keys:
        if isinstance(before.get(key), int) and isinstance(after.get(key), int):
            parts.append(f"{key} +{after[key] - before[key]}")
    return "TCP " + ", ".join(parts) if parts else "TCP counters unread"


class Guest:
    """One VM in the rig's shape, reached over SSH on a forwarded loopback port."""

    def __init__(self, system, run_dir, disk, headed, extra=(), kernel=BREENIX_KERNEL):
        self.system = system
        self.run_dir = run_dir
        self.disk = disk
        self.headed = headed
        self.extra = list(extra)
        self.kernel = kernel
        self.port = None
        self.process = None
        self.boots = 0
        # The launch profile of each boot, as the launcher resolved it (launch_profile).
        self.profiles = []
        self.key = LINUX_KEY if system == "linux" else BREENIX_KEY
        self.user = "bench" if system == "linux" else "sshtest"
        self.home = LINUX_HOME if system == "linux" else "/etc/breenix/test-ssh/home"
        self.known_hosts = run_dir / f"known_hosts-{system}"

    def argv(self):
        boot = (["-bios", EDK2] if self.system == "linux" else ["-kernel", self.kernel])
        disks = ["-device", "virtio-blk-device,drive=disk0",
                 "-drive", f"if=none,id=disk0,format=raw,cache=writeback,file={self.disk}"]
        return [
            sys.executable, ROOT / "scripts/qemu-arm64.py", "qemu-system-aarch64", *SHAPE_ARGS,
            *boot, "-display", "cocoa" if self.headed else "none",
            *disks,
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
        # The launcher's choice in this same environment, just before it makes it.
        self.profiles.append(launch_profile(argv))
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
    def __init__(self, args, commit):
        self.args = args
        # The commit the stamped Breenix inputs were built from; Linux builds it too.
        self.commit = commit
        self.run_dir = CACHE / "runs" / args.run
        self.run_dir.mkdir(parents=True, exist_ok=False)
        self.out = {system: self.run_dir / system for system in SYSTEMS}
        for path in self.out.values():
            path.mkdir()
        self.endpoint = TcpEndpoint()
        threading.Thread(target=self.endpoint.serve_forever, daemon=True).start()
        self.guests = {}
        self.notes = {system: [] for system in SYSTEMS}
        # Each guest's root mount line, as the guest reports it, by boot number: only
        # boots whose setup passed, which are the only boots that take samples.
        self.roots = {system: {} for system in SYSTEMS}
        # The verified build record and the run's clones of its artifacts (boot()).
        self.build = None
        self.inputs = {}
        self.started = datetime.now(timezone.utc).replace(microsecond=0)

    def bin(self, system, name):
        return f"{self.guests[system].home}/bbench/bin/{name}"

    def dirs(self, system):
        home = self.guests[system].home
        return f"{home}/bbench/data/{self.args.run}", f"{home}/bbench/reports"

    def boot(self):
        commit = self.commit
        disk = self.run_dir / "breenix-disk.raw"
        # The build this run verified, cloned into the run before anything boots.
        self.build, self.inputs = snapshot_inputs(BUILD_STAMP, commit, BREENIX_ARTIFACTS,
                                                  self.run_dir / "inputs")
        run_command(["cp", "-c", self.inputs[BREENIX_IMAGE], disk])
        self.guests["breenix"] = Guest("breenix", self.run_dir, disk, self.args.headed,
                                        kernel=self.inputs[BREENIX_KERNEL])
        linux_disk = self.run_dir / "linux-disk.raw"
        run_command(["cp", "-c", LINUX_CACHE / f"provisioned-{PIN['sha256'][:12]}-{DISK_BYTES // MIB}.raw",
                     linux_disk])
        self.guests["linux"] = Guest("linux", self.run_dir, linux_disk, self.args.headed)
        for system in SYSTEMS:
            self.guests[system].start()
            self.setup(system)
        profiles = {system: self.guests[system].profiles[-1] for system in SYSTEMS}
        if len(set(profiles.values())) != 1:
            raise RuntimeError(f"the guests launched with different profiles: {profiles}")
        return commit

    def check_root(self, system):
        """Refuse the run unless the guest's root is the filesystem the disk rows name."""
        mounts = self.guests[system].ssh(["/bin/cat", "/proc/mounts"]).stdout.decode()
        problem = root_problem(system, mounts)
        if problem:
            raise RuntimeError(problem)
        return next(line for line in reversed(mounts.splitlines()) if line.split()[1:2] == ["/"])

    def setup(self, system):
        """Make a freshly booted guest ready for samples. Its root line is recorded, under
        its boot number, only once every step has passed."""
        guest = self.guests[system]
        data, reports = self.dirs(system)
        home = guest.home
        root = self.check_root(system)
        guest.ssh(["/bin/mkdir", "-p", f"{home}/bbench/bin", data, reports, f"{home}/bbench/net"])
        if system == "breenix":
            for artifact, name in ((BREENIX_RELEASE / "bbench", "bbench"),
                                   (BREENIX_RELEASE / "bbench-exit", "bbench-exit"),
                                   (BREENIX_OZ / "bbench", "bbench-oz"),
                                   (BREENIX_O3 / "bbench", "bbench-o3")):
                # The run's verified clone, on the first boot and each restart.
                source = self.inputs[artifact]
                result = guest.scp(source, guest.remote(f"{home}/bbench/bin/{name}"))
                if result.returncode:
                    raise RuntimeError(f"breenix: upload of {name} failed: {result.stderr!r}")
                if sha256(source) != guest.ssh(["/bin/sha256sum", f"{home}/bbench/bin/{name}"]
                                               ).stdout.decode().split()[0]:
                    raise RuntimeError(f"breenix: {name} arrived damaged")
            guest.ssh(["/bin/chmod", "755", *(f"{home}/bbench/bin/{name}"
                                              for name in ("bbench", "bbench-exit", "bbench-oz",
                                                           "bbench-o3"))])
            # The compile rows build userland's yes from the same source as Linux.
            self.sync_source(guest)
        else:
            self.sync_source(guest)
            cargo = (f"cd {home}/src/userland && PATH={LINUX_TOOLCHAIN}:/usr/bin:/bin "
                     f"{LINUX_TOOLCHAIN}/cargo build -q --offline --frozen -p bbench")
            guest.ssh(f"{cargo} --profile release --target-dir {home}/bbench/target", timeout=1800)
            guest.ssh(f"{cargo} --profile bench-oz --target-dir {home}/bbench/target", timeout=1800)
            guest.ssh(f"{cargo} --profile bench-o3 --target-dir {home}/bbench/target", timeout=1800)
            guest.ssh(f"cp {home}/bbench/target/release/bbench {home}/bbench/target/release/bbench-exit "
                      f"{home}/bbench/bin/ && cp {home}/bbench/target/bench-oz/bbench "
                      f"{home}/bbench/bin/bbench-oz && cp {home}/bbench/target/bench-o3/bbench "
                      f"{home}/bbench/bin/bbench-o3")
        fixture = f"{home}/bbench/fixture.bin"
        made = self.bbench(system, f"{self.args.run}-fixture-boot{guest.boots}",
                           ["fixture", fixture, FIXTURE_BYTES])
        if made["error"]:
            raise RuntimeError(f"{system}: fixture: {made['error']}")
        self.roots[system][guest.boots] = root

    def sync_source(self, guest):
        manifest = tomllib.loads((ROOT / "libs/libbreenix/Cargo.toml").read_text())
        targets = [ROOT / "libs/libbreenix" / entry["path"]
                   for kind in ("test", "bin", "example", "bench") for entry in manifest.get(kind, [])
                   if "path" in entry]
        paths = ["userland", "libs/libbreenix", "libs/input-abi", "libs/breengel-abi",
                 "libs/gpu-abi", "toolchain/rust-host/k5/corpus",
                 *sorted({str(path.resolve().relative_to(ROOT)) for path in targets})]
        archive = self.run_dir / "source.tar"
        if not archive.exists():
            run_command(["git", "-C", ROOT, "archive", "--format=tar", "-o", archive, self.commit,
                         *paths])
        result = guest.scp(archive, guest.remote(f"{guest.home}/source.tar"))
        if result.returncode:
            raise RuntimeError(f"{guest.system}: source upload failed: {result.stderr!r}")
        guest.ssh(f"rm -rf {guest.home}/src && mkdir -p {guest.home}/src && "
                  f"tar -xf {guest.home}/source.tar -C {guest.home}/src && "
                  f"rm {guest.home}/source.tar", timeout=900)

    def compile_env(self, system, case):
        """The environment a compile row runs in. Linux: the pinned toolchain on PATH.
        Breenix: the staged hosted toolchain, as the cargo suite's invocation sets it
        (userspace/programs/src/suite_cargo.rs)."""
        if system == "linux":
            # Cargo finds rustc on PATH; name the toolchain's own binaries, not rustup's proxies.
            return ["/usr/bin/env", f"PATH={LINUX_TOOLCHAIN}:/usr/bin:/bin",
                    f"CARGO_BUILD_JOBS={CPUS}"]
        if case not in COMPILE:
            return []
        fixture = BREENIX_TOOLCHAIN
        flags = "\x1f".join([
            f"--sysroot={fixture}/sysroot", f"-Clinker={fixture}/bin/bld",
            f"-Clink-arg=-T{fixture}/lib/linker.ld", f"-Lnative={fixture}/lib",
            "-Clink-arg=--allow-multiple-definition", "-Cdefault-linker-libraries=no", "-Dwarnings"])
        return ["/bin/env", f"RUST_TARGET_PATH={fixture}/targets", f"RUSTC={fixture}/bin/rustc",
                f"CARGO_HOME={self.guests[system].home}/bbench/cargo-home", "CARGO_TERM_COLOR=never",
                f"CARGO_BUILD_JOBS={CPUS}", f"CARGO_ENCODED_RUSTFLAGS={flags}"]

    def bbench(self, system, sample, words, binary="bbench", timeout=900, case=None):
        guest = self.guests[system]
        data, reports = self.dirs(system)
        argv = [*self.compile_env(system, case),
                self.bin(system, binary), data, reports, sample, *words]
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

        # The same compiles on both systems, each with its own toolchain and native target:
        # Linux's pinned nightly, and Breenix's staged hosted build of the fork.
        if system == "breenix":
            rustc, cargo = f"{BREENIX_TOOLCHAIN}/bin/rustc", f"{BREENIX_TOOLCHAIN}/bin/cargo"
            target = ["--target", BREENIX_TARGET]
            sysroot = ["--sysroot", f"{BREENIX_TOOLCHAIN}/sysroot"]
        else:
            rustc, cargo = f"{LINUX_TOOLCHAIN}/rustc", f"{LINUX_TOOLCHAIN}/cargo"
            target = sysroot = []

        def build(profile, output):
            return ["command", f"{home}/src/userland", cargo, "build",
                    "--offline", "--frozen", "--profile", profile, "-p", "yes",
                    "--target-dir", output, *target]

        return {
            "rustc-version": lambda: ["command", data, rustc, "-vV"],
            "hello-object": lambda: ["command", f"{home}/src/toolchain/rust-host/k5/corpus",
                                     rustc, "--edition=2021", *sysroot, "-Dwarnings",
                                     "--emit=obj", *target, "--crate-type=lib", "hello.rs", "-o",
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
        warmup = None
        if case == "cargo-noop":
            seed = f"{self.args.run}-noop-seed"
            if number == 1:
                self.save(system, seed, self.bbench(system, seed,
                                                    self.workload(system, case, seed), case=case))
            warmup = self.warmup_failure(system, seed)
        if warmup:
            # Without a built target the next build does the real work, so it is not timed.
            receipt = failure_receipt(name, [case], warmup)
        else:
            receipt = self.bbench(system, name, self.workload(system, case, name),
                                  binary=probe_binary(case), case=case)
        self.save(system, name, receipt)
        if receipt["error"]:
            print(f"{system} {name} failed: {receipt['error']}", flush=True)
            # A guest that stopped, or no longer answers SSH, is booted again.
            if not guest.alive() or guest.ssh(["/bin/true"], timeout=30,
                                              check=False).returncode != 0:
                self.restart(system)

    def warmup_failure(self, system, seed):
        """Why the Cargo no-op warm-up gave no built target, from its saved receipt, or
        None when it succeeded."""
        path = self.out[system] / f"{seed}.json"
        if not path.exists():
            return "Cargo no-op warm-up failed: it did not run"
        error = json.loads(path.read_text())["error"]
        return f"Cargo no-op warm-up failed: {error}" if error else None

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
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            # A guest that is up but not set up must not take samples: stop it, so
            # sample() records the attempt as lost.
            print(f"{system}: restart failed: {error}", flush=True)
            self.notes[system].append(f"boot {guest.boots} failed setup and was stopped")
            guest.stop()

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
        identity = guest.ssh([f"{BREENIX_TOOLCHAIN}/bin/rustc", "-vV"], check=False)
        return {"rustc": identity.stdout.decode(errors="replace").splitlines()}

    @staticmethod
    def breenix_toolchain(facts):
        """The Breenix series' toolchain field: the hosted fork the compile rows ran."""
        release = next(iter(facts.get("rustc") or []), "") or "rustc"
        return (f"{release} hosted on Breenix: the Breenix Rust fork {FORK[:9]}, {FORK_AHEAD} "
                f"commits ahead of {FORK_BASE['channel']} ({FORK_BASE['commit'][:9]})")

    def write(self, system, commit, before, after, facts):
        """Write the system's series inside Vigil's limits, with the full provenance in
        its sidecar (series.write_series)."""
        groups = collect_receipts(self.out[system], [self.args.run], "scp-upload")
        short = commit[:8]
        if system == "linux":
            kernel = kernel_release(facts.get("kernel") or "") or "unknown"
            toolchain = facts.get("toolchain") or f"rustc {FORK_BASE['channel']}"
            note = (f"Stock {PIN['distribution']} {PIN['release']}, kernel unmodified; bbench "
                    f"built in the guest from {short} with {FORK_BASE['channel']} "
                    f"({FORK_BASE['commit'][:9]}), the Breenix fork's upstream base.")
        else:
            kernel = short
            toolchain = self.breenix_toolchain(facts)
            note = (f"Production kernel and system image at {short} on a BreenFS root; bbench "
                    f"run as sshtest. Compile rows: the staged hosted fork {FORK[:9]} ({FORK_AHEAD} commits "
                    f"ahead of Linux's {FORK_BASE['channel']}).")
        note += f" Samples alternate Linux then Breenix. {tcp_summary(system, before, after)}."
        if self.notes[system]:
            note += f" {len(self.notes[system])} VM restart(s)."
        # Each boot's launch profile. boot() refuses guests launched with different ones,
        # and a restart inherits the environment of the run's first boot.
        profiles = self.guests[system].profiles
        provenance = {
            "system": system, "commit": commit, "vmShape": vm_shape(profiles[0]),
            "launchProfiles": profiles,
            "samples": "Linux then Breenix per sample of each case; both VMs stay up and one "
                       "runs a sample at a time",
            "fixture": f"{FIXTURE_BYTES // MIB} MiB file written in the guest for the "
                       "driver-map rows",
            "tcpCounters": {"atBoot": before, "afterNetworkRows": after},
            "notes": self.notes[system], "facts": facts,
            "rootFilesystem": {"filesystem": ROOT_CACHE[system],
                               "mounts": getattr(self, "roots", {}).get(system, {})},
        }
        if system == "linux":
            provenance["image"] = PIN
            provenance["toolchain"] = FORK_BASE
        else:
            provenance["toolchain"] = {"fork": FORK, "commitsAhead": FORK_AHEAD, "base": FORK_BASE,
                                       "staged": BREENIX_TOOLCHAIN,
                                       # The record snapshot_inputs verified, not the
                                       # shared stamp as it reads now.
                                       "stamp": self.build}
        metadata = {
            "system": system, "target": "arm64-qemu", "hardware": hardware(profiles[0]),
            "kernelCommit": kernel, "userlandCommit": short, "toolchain": toolchain,
            "session": self.args.run, "pr": self.args.pr, "note": note,
        }
        path = write_series(SERIES_DIR, f"{self.args.run}-{system}",
                            self.started.isoformat().replace("+00:00", "Z"), metadata, groups,
                            deferred=DEFERRED[system], provenance=provenance,
                            cache={case: ROOT_CACHE[system] for case in DISK_CASES})
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


def write_build_stamp(stamp, commit, artifacts):
    stamp.parent.mkdir(parents=True, exist_ok=True)
    record = {"commit": commit, "artifacts": {str(path): sha256(path) for path in artifacts}}
    stamp.write_text(json.dumps(record, indent=2) + "\n")


def read_build_stamp(stamp, head):
    """The build record, read once. Refuses inputs build-breenix did not build at `head`."""
    if not stamp.exists():
        raise ValueError(f"no build stamp at {stamp}: build the Breenix inputs with "
                         "scripts/bench-qemu-arm64.sh build-breenix")
    record = json.loads(stamp.read_text())
    if record["commit"] != head:
        raise ValueError(f"the Breenix inputs were built at {record['commit']}, not HEAD {head}: "
                         "run scripts/bench-qemu-arm64.sh build-breenix")
    return record


def check_artifacts(record, artifacts, copies=None):
    """Refuse any artifact (or its copy in `copies`) whose SHA-256 is not the record's."""
    copies = copies or {}
    for path in artifacts:
        recorded = record["artifacts"].get(str(path))
        if recorded is None:
            raise ValueError(f"the build stamp does not cover {path}: run build-breenix")
        actual = Path(copies.get(path, path))
        if not actual.exists() or sha256(actual) != recorded:
            raise ValueError(f"{path} changed after build-breenix built it at {record['commit']}")


def verify_build_stamp(stamp, head, artifacts):
    """The commit the Breenix inputs were built from. Refuses inputs that build-breenix
    did not build at `head`, and any artifact that changed after that build."""
    check_artifacts(read_build_stamp(stamp, head), artifacts)
    return head


def snapshot_inputs(stamp, head, artifacts, into):
    """Clone each stamped artifact into the run (`into`) and verify the clones against
    the stamp, read once. Every boot of the run, restarts included, and its sidecar use
    these, so a build-breenix elsewhere that replaces the shared stamp or artifacts
    during the run changes nothing in it. Returns the record and each artifact's clone."""
    record = read_build_stamp(stamp, head)
    into.mkdir(parents=True)
    clones = {}
    for index, path in enumerate(artifacts):
        if not Path(path).exists():
            raise ValueError(f"missing Breenix input {path}: run build-breenix")
        clones[path] = into / f"{index}-{Path(path).name}"
        # A copy-on-write clone on APFS: the 8 GiB image costs no space or time.
        run_command(["cp", "-c", path, clones[path]])
    check_artifacts(record, artifacts, clones)
    return record, clones


def build_breenix():
    """Build the Breenix inputs (RIG.md) from a clean tree and stamp their commit."""
    if git("status", "--porcelain", "--untracked-files=no"):
        raise ValueError("commit the tree first: the build stamp records its commit")
    commit = git("rev-parse", "HEAD")
    env = toolchain_build_env()
    for words in BREENIX_BUILD:
        run_command(words, cwd=ROOT, env=env)
    # As the suite boot does after --kd-loader: the pinned libc beside the loader.
    kd = Path(env.get("BREENIX_KD_BUILD_DIR", ROOT / "userspace/programs/target/kd/aarch64"))
    shutil.copyfile(env["BREENIX_KD_LIBC_INPUT"], kd / "libc.so")
    stage_image(env)
    if git("rev-parse", "HEAD") != commit or git("status", "--porcelain", "--untracked-files=no"):
        raise ValueError(f"the tree moved from {commit} during the build; nothing was stamped")
    write_build_stamp(BUILD_STAMP, commit, BREENIX_ARTIFACTS)
    print(f"Breenix inputs built and stamped at {commit} ({BUILD_STAMP})")


def toolchain_build_env():
    """The environment of the rustc and cargo suite boots' builds and staging. The hosted
    toolchain's inputs are the E25 build caches, named the way those boots name them."""
    env = {key: value for key, value in os.environ.items() if key != "CARGO_TARGET_DIR"}
    missing = [name for name in ("BREENIX_RUSTC_PORT_BUILDS", "BREENIX_K7_STAGE") if not env.get(name)]
    if missing:
        raise ValueError(f"set {' and '.join(missing)} to the ARM K5 cache and the K7 stage "
                         "(userland/bbench/RIG.md, Inputs)")
    cache = Path(env["BREENIX_RUSTC_PORT_BUILDS"])
    env.setdefault("BREENIX_K5_SYSROOT", str(cache / "sysroot"))
    env.setdefault("BREENIX_KD_LIBC_INPUT",
                   env.get("BREENIX_HOSTED_LIBC") or str(cache / "shared-libc/libc.so"))
    env["BREENIX_TEST_SSH_KEY_DIR"] = str(BREENIX_KEY.parent)
    return env


def stage_image(env):
    """Copy the standard image, stage the hosted toolchain into it with the suite boots'
    staging (scripts/rust_host_stage.py --suite rustc,cargo), then convert it to a BreenFS
    root of the rig's disk size with their converter (scripts/create-breenfs-root.sh,
    as scripts/lib/root-fs.sh does for --root-fs breenfs)."""
    BREENIX_IMAGE.parent.mkdir(parents=True, exist_ok=True)
    staged = BREENIX_IMAGE.with_name("root-ext2.raw")
    partial = BREENIX_IMAGE.with_name(BREENIX_IMAGE.name + ".partial")
    for path in (staged, partial):
        path.unlink(missing_ok=True)
    shutil.copyfile(STANDARD_IMAGE, staged)
    run_command([sys.executable, ROOT / "scripts/rust_host_stage.py", "--arch", "aarch64",
                 "--image", staged, "--suite", "rustc,cargo"], cwd=ROOT, env=env)
    if staged.stat().st_size > DISK_BYTES:
        raise ValueError(f"the staged image is {staged.stat().st_size} bytes, more than the "
                         f"rig's {DISK_BYTES}-byte disk")
    # The converter sizes the BreenFS image to its input.
    with open(staged, "r+b") as disk:
        disk.truncate(DISK_BYTES)
    run_command(["bash", ROOT / "scripts/create-breenfs-root.sh", staged, partial], cwd=ROOT,
                env=env)
    staged.unlink()
    partial.rename(BREENIX_IMAGE)


def check_breenix_inputs():
    """The commit the Breenix inputs were built from, once they are present and fit."""
    missing = [path for path in (*BREENIX_ARTIFACTS, BREENIX_KEY) if not path.exists()]
    if missing:
        raise ValueError("missing Breenix inputs (run scripts/bench-qemu-arm64.sh "
                         "build-breenix): " + ", ".join(map(str, missing)))
    if BREENIX_IMAGE.stat().st_size != DISK_BYTES:
        raise ValueError(f"{BREENIX_IMAGE} must be {DISK_BYTES // MIB} MiB, the rig's disk size "
                         "(scripts/bench-qemu-arm64.sh build-breenix)")
    return verify_build_stamp(BUILD_STAMP, git("rev-parse", "HEAD"), BREENIX_ARTIFACTS)


def run_id_problems(run):
    """Why the run's series could not be written, decided before anything boots: each
    system's series ID and sidecar reference, and the run ID as the session field."""
    found = [problem for system in SYSTEMS for problem in series_id_problems(f"{run}-{system}")]
    session = vigil_rules.text_problem(run, vigil_rules.LIMITS["session"])
    if session:
        found.append(f"the session field {session}")
    return found


def cli(argv):
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("fetch-linux")
    commands.add_parser("provision-linux")
    commands.add_parser("build-breenix", help="build the Breenix inputs at HEAD and stamp them")
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
    if args.command == "build-breenix":
        build_breenix()
        return 0
    if args.command == "ratios":
        print(ratio_table(args.linux, args.breenix))
        return 0
    if args.command == "provision-linux":
        provision_linux()
        return 0
    if not args.run.replace("-", "").isalnum():
        parser.error("--run must contain only letters, digits or hyphens")
    problems = run_id_problems(args.run)
    if problems:
        parser.error(f"--run {args.run!r} cannot be published: {'; '.join(problems)}")
    if git("status", "--porcelain", "--untracked-files=no"):
        parser.error("commit the tree first: the series record its commit")
    commit = check_breenix_inputs()
    provision_linux()
    rig = Rig(args, commit)
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
