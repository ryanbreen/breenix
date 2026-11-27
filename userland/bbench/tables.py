"""Summarize bbench JSON receipts as a Markdown table."""

import argparse
from collections import defaultdict
import json
from pathlib import Path
from statistics import median
from typing import NamedTuple

SECOND = 10**9
MILLISECOND = 10**6
# A throughput row: the receipt's bytes metric over its summed time fields, in MB/s.
THROUGHPUT = 0
COLD_TARGET = "Cold target, natural file cache"
COLD_BUILD = "Cold target, uncontrolled file cache / cold target, warm toolchain"
DEFERRED = "Deferred to #3226"
DEFERRED_DISK = ("sequential", "small-files", "walk", "fsync")
# Cases whose work changed: their rows keep the historical receipts, and a new
# series records them as deferred with the row that replaced them.
RETIRED = {"copy": "Retired: it copied 4 GiB of i32 values; the 1 GiB byte copy is "
                   "memory.copy-bytes"}


class Row(NamedTuple):
    id: str
    case: str
    title: str
    fields: tuple[str, ...]
    scale: int
    cache: str
    report_cache: str


# Fields within a row are summed per receipt before computing the median.
ROWS = (
    Row("compile.rustc-vv", "rustc-version", "rustc -vV", ("wall_ns",), SECOND,
        "Natural compiler and driver cache", "Mixed driver residency / warm compiler"),
    Row("compile.hello-object", "hello-object", "K5 hello object", ("wall_ns",), SECOND,
        "Fresh object, natural compiler cache",
        "Fresh object, mixed driver residency / fresh object, first and warm compiler runs"),
    Row("compile.cargo-noop", "cargo-noop", "Cargo no-op, userland yes", ("wall_ns",), SECOND,
        "Warm target, natural file cache",
        "Warm target, device reads during check / warm target and toolchain"),
    Row("compile.yes-dev-box", "yes-dev-box", "Cold yes, dev-box",
        ("wall_ns",), SECOND, COLD_TARGET, COLD_BUILD),
    Row("compile.yes-release", "yes-release", "Cold yes, release",
        ("wall_ns",), SECOND, COLD_TARGET, COLD_BUILD),
    Row("compile.bobj", "bobj", "Cold bobj, selfhost", ("wall_ns",), SECOND, COLD_TARGET, COLD_BUILD),
    Row("compile.bld", "bld", "Cold bld, selfhost", ("wall_ns",), SECOND, COLD_TARGET, COLD_BUILD),
    Row("compile.bmake", "bmake", "Cold bmake, selfhost", ("wall_ns",), SECOND, COLD_TARGET, COLD_BUILD),
    Row("compile.bcc", "bcc", "Cold bcc, selfhost", ("wall_ns",), SECOND, COLD_TARGET, COLD_BUILD),
    Row("process.fork-exec", "fork-exec", "Fork/exec/wait, per child (ms)", ("wall_ns",), MILLISECOND * 100,
        "Repeated child, natural executable cache",
        "Repeated child, uncontrolled executable cache / repeated child, warm executable"),
    Row("process.rustc-first-output", "rustc-version", "rustc exec to first output", ("first_output_ns",), SECOND,
        "Natural compiler and driver cache", "Mixed driver residency / warm compiler"),
    Row("memory.anonymous-touch", "anonymous", "Touch 256 MiB anonymous", ("wall_ns",), SECOND,
        "Fresh zero pages", "Fresh zero pages on both"),
    Row("memory.driver-mmap", "file-map", "Driver mmap", ("map_ns",), SECOND,
        "No eviction before map, touch follows map", "Device reads on eager map / lazy map, no eviction"),
    Row("memory.driver-touch", "file-map", "Driver first touch", ("touch_ns",), SECOND,
        "No eviction before map, touch follows map", "Resident after map / first and warm touches"),
    Row("memory.driver-map-touch", "file-map", "Driver map + touch", ("map_ns", "touch_ns"), SECOND,
        "No eviction before map, touch follows map", "Device reads on map / first and warm touches"),
    # Retired: the probe's buffers held i32 values, so this row copied 4 GiB (RETIRED).
    Row("memory.copy", "copy", "Copy 4 GiB total (i32 buffers)", ("copy_ns",), SECOND,
        "Resident buffers", "Resident buffers on both"),
    Row("memory.copy-bytes", "copy-bytes", "Copy 1 GiB of bytes", ("copy_ns",), SECOND,
        "Resident buffers", "Resident buffers on both"),
    Row("disk.sequential-write", "sequential", "Write 1 GiB and fsync", ("write_ns",), SECOND,
        "New file, buffered write followed by fsync",
        "Deferred / new file, buffered write followed by fsync"),
    Row("disk.sequential-read", "sequential", "Read 1 GiB immediately after fsync", ("read_ns",), SECOND,
        "Warm from preceding write", "Deferred / warm from preceding write"),
    Row("disk.create", "small-files", "Create 10,000 files", ("create_ns",), SECOND,
        "Fresh files in newly made directories", "Fresh files in newly made directories on both"),
    Row("disk.stat", "small-files", "Stat 10,000 files", ("stat_ns",), SECOND,
        "Warm from create", "Warm from create on both"),
    Row("disk.unlink", "small-files", "Unlink 10,000 files", ("unlink_ns",), SECOND,
        "Warm from stat", "Warm from stat on both"),
    Row("disk.walk", "walk", "Walk target tree", ("walk_ns",), SECOND,
        "Warm from fixture creation", "Deferred / warm from fixture creation"),
    Row("disk.fsync", "fsync", "Fsync 4 KiB append, per call (ms)", ("fsync_ns",), MILLISECOND,
        "Recently dirtied file", "Deferred / recently dirtied file"),
    Row("network.bsync-upload", "bsync-upload", "8 MB upload, bsync", ("wall_ns",), SECOND,
        "Warm source, empty destination", "Warm source, empty destination / —"),
    Row("network.scp-upload", "scp-upload", "SCP upload (Mac 100 MB, QEMU 8 MB)", ("wall_ns",), SECOND,
        "Warm source, empty destination", "— / warm source, empty destination"),
    Row("network.scp-download", "download", "SCP download (R240 and QEMU 8 MB, Mac 100 MB)", ("wall_ns",), SECOND,
        "After upload and verification",
        "Recently uploaded and checked, file cache uncontrolled / warm loopback cache"),
    Row("ssh.round-trip", "ssh", "SSH round trip", ("wall_ns",), SECOND,
        "Fresh SSH connection", "Fresh SSH connection on both"),
    # Attribution rows (audit step 0.5): each isolates one mechanism behind the rows above.
    Row("process.getpid", "getpid", "getpid ×1M (ms)", ("getpid_ns",), MILLISECOND,
        "One process, 1,000,000 calls", "One process, 1,000,000 calls"),
    Row("memory.malloc-1t", "malloc-1t", "malloc/free 64 B ×1M, 1 thread (ms)", ("malloc_ns",),
        MILLISECOND, "One allocate-free pair at a time", "One allocate-free pair at a time"),
    Row("memory.malloc-4t", "malloc-4t", "malloc/free 64 B ×1M per thread, 4 threads (ms)",
        ("malloc_ns",), MILLISECOND, "Four threads started together",
        "Four threads started together"),
    Row("network.tcp-guest-to-host", "tcp-send", "Raw TCP 32 MB, guest to host (MB/s)",
        ("tcp_ns",), THROUGHPUT, "Fresh connection, delivery acknowledged by the host",
        "Fresh connection, delivery acknowledged by the host"),
    Row("network.tcp-host-to-guest", "tcp-recv", "Raw TCP 32 MB, host to guest (MB/s)",
        ("tcp_ns",), THROUGHPUT, "Fresh connection, until the last byte is read",
        "Fresh connection, until the last byte is read"),
    *(Row(f"ssh.{name}-{level}", f"{case}-{level}", f"{title}, opt-level {level[1]} (MB/s)",
          ("crypto_ns",), THROUGHPUT, "8 MiB in 32 KiB SSH packets", "8 MiB in 32 KiB SSH packets")
      for name, case, title in (("aes256-gcm", "aes-gcm", "AES-256-GCM seal"),
                                ("hmac-sha256", "hmac-sha256", "HMAC-SHA256"))
      for level in ("oz", "o3")),
)


def collect_receipts(directory, prefixes, upload_case):
    groups = defaultdict(list)
    directory = Path(directory)
    if not directory.is_dir():
        raise ValueError(f"receipt directory does not exist: {directory}")
    cases = {row.case for row in ROWS}
    for prefix in dict.fromkeys(prefixes):
        for path in sorted(directory.glob(f"{prefix}-*.json")):
            label = path.stem.removeprefix(prefix + "-")
            if label == "noop-seed":
                case = label
            else:
                case, _, index = label.rpartition("-")
                if case == "upload":  # Older receipts used this name for both upload methods.
                    case = upload_case
                if not index.isdecimal() or case not in cases:
                    continue
            sample = json.loads(path.read_bytes())
            if sample["sample"] != path.stem:
                raise ValueError(f"sample ID does not match {path}")
            groups[case].append(sample)
    return groups


def collect(prefix, directory, upload_case):
    groups = collect_receipts(directory, [prefix], upload_case)
    groups.pop("noop-seed", None)
    if not groups:
        raise ValueError(f"no timed receipts for {prefix} in {directory}")
    return groups


def values(samples, fields, scale):
    def elapsed(sample):
        return sum(sample["wall_ns"] if field == "wall_ns" else sample["metrics"][field]
                   for field in fields)

    if scale == THROUGHPUT:
        # Bytes per microsecond is MB/s (10^6 bytes per second).
        return [sample["metrics"]["bytes"] * 1000 / elapsed(sample)
                for sample in samples if sample["error"] is None]
    return [elapsed(sample) / scale for sample in samples if sample["error"] is None]


def unit(row):
    return {THROUGHPUT: "MB/s", SECOND: "s"}.get(row.scale, "ms")


def better(row):
    return "higher" if row.scale == THROUGHPUT else "lower"


def summary(samples):
    if not samples:
        return "—"
    middle = median(samples)
    precision = 4 if middle < .01 else 3
    text = f"{middle:.{precision}f}"
    if len(samples) > 1:
        text += f" [{min(samples):.{precision}f}, {max(samples):.{precision}f}]"
    return text


def render(box, mac):
    lines = [
        "| Workload | R240 | Mac | R240 completed / receipts | Cache state or preparation |",
        "| --- | ---: | ---: | ---: | --- |",
    ]
    for row in ROWS:
        box_receipts = box.get(row.case, [])
        left = values(box_receipts, row.fields, row.scale)
        right = values(mac.get(row.case, []), row.fields, row.scale)
        deferred = row.case in ("sequential", "walk", "fsync")
        box_result = DEFERRED if deferred else summary(left)
        count = f"{len(left)}/{len(box_receipts)}" if box_receipts else "—"
        lines.append(
            f"| {row.title} | {box_result} | {summary(right)} | {count} | {row.report_cache} |"
        )
    return "\n".join(lines)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--box", nargs=2, required=True, metavar=("RUN", "DIR"))
    parser.add_argument("--mac", nargs=2, required=True, metavar=("RUN", "DIR"))
    return parser


def table(args):
    return render(collect(*args.box, "bsync-upload"), collect(*args.mac, "scp-upload"))


if __name__ == "__main__":
    print(table(arguments().parse_args()))
