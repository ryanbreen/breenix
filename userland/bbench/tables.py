"""Summarize bbench JSON receipts as a Markdown table."""

import argparse
from collections import defaultdict
import json
from pathlib import Path
from statistics import median

SECOND = 10**9
MILLISECOND = 10**6
COLD_BUILD = "Cold target, uncontrolled file cache / cold target, warm toolchain"
DEFERRED = "Deferred to #3226"

# Fields within a row are summed per receipt before computing the median.
ROWS = (
    ("rustc-version", "rustc -vV", ("wall_ns",), SECOND,
     "Mixed driver residency / warm compiler"),
    ("hello-object", "K5 hello object", ("wall_ns",), SECOND,
     "Fresh object, mixed driver residency / fresh object, first and warm compiler runs"),
    ("cargo-noop", "Cargo no-op, userland yes", ("wall_ns",), SECOND,
     "Warm target, device reads during check / warm target and toolchain"),
    ("yes-dev-box", "Cold yes, dev-box", ("wall_ns",), SECOND, COLD_BUILD),
    ("yes-release", "Cold yes, release", ("wall_ns",), SECOND, COLD_BUILD),
    ("bobj", "Cold bobj, selfhost", ("wall_ns",), SECOND, COLD_BUILD),
    ("bld", "Cold bld, selfhost", ("wall_ns",), SECOND, COLD_BUILD),
    ("bmake", "Cold bmake, selfhost", ("wall_ns",), SECOND, COLD_BUILD),
    ("bcc", "Cold bcc, selfhost", ("wall_ns",), SECOND, COLD_BUILD),
    ("fork-exec", "Fork/exec/wait, per child (ms)", ("wall_ns",), MILLISECOND * 100,
     "Repeated child, uncontrolled executable cache / repeated child, warm executable"),
    ("rustc-version", "rustc exec to first output", ("first_output_ns",), SECOND,
     "Mixed driver residency / warm compiler"),
    ("anonymous", "Touch 256 MiB anonymous", ("wall_ns",), SECOND,
     "Fresh zero pages on both"),
    ("file-map", "Driver mmap", ("map_ns",), SECOND,
     "Device reads on eager map / lazy map, no eviction"),
    ("file-map", "Driver first touch", ("touch_ns",), SECOND,
     "Resident after map / first and warm touches"),
    ("file-map", "Driver map + touch", ("map_ns", "touch_ns"), SECOND,
     "Device reads on map / first and warm touches"),
    ("copy", "Copy 1 GiB total", ("copy_ns",), SECOND,
     "Resident buffers on both"),
    ("malloc-1t", "malloc/free ×1M, 1 thread", ("malloc_free_ns",), SECOND,
     "Warm, 64-byte blocks"),
    ("malloc-4t", "malloc/free ×1M, 4 threads", ("malloc_free_ns",), SECOND,
     "Warm, 64-byte blocks, 250,000 pairs per thread"),
    ("sequential", "Write 1 GiB and fsync", ("write_ns",), SECOND,
     "Deferred / new file, buffered write followed by fsync"),
    ("sequential", "Read 1 GiB immediately after fsync", ("read_ns",), SECOND,
     "Deferred / warm from preceding write"),
    ("small-files", "Create 10,000 files", ("create_ns",), SECOND,
     "Fresh files in newly made directories on both"),
    ("small-files", "Stat 10,000 files", ("stat_ns",), SECOND,
     "Warm from create on both"),
    ("small-files", "Unlink 10,000 files", ("unlink_ns",), SECOND,
     "Warm from stat on both"),
    ("walk", "Walk target tree", ("walk_ns",), SECOND,
     "Deferred / warm from fixture creation"),
    ("fsync", "Fsync 4 KiB append, per call (ms)", ("fsync_ns",), MILLISECOND,
     "Deferred / recently dirtied file"),
    ("bsync-upload", "8 MB upload, bsync", ("wall_ns",), SECOND,
     "Warm source, empty destination / —"),
    ("scp-upload", "100 MB upload, loopback SCP", ("wall_ns",), SECOND,
     "— / warm source, empty destination"),
    ("download", "SCP download (R240 8 MB, Mac 100 MB)", ("wall_ns",), SECOND,
     "Recently uploaded and checked, file cache uncontrolled / warm loopback cache"),
    ("ssh", "SSH round trip", ("wall_ns",), SECOND,
     "Fresh SSH connection on both"),
)


def collect(prefix, directory, upload_case):
    groups = defaultdict(list)
    directory = Path(directory)
    if not directory.is_dir():
        raise ValueError(f"receipt directory does not exist: {directory}")
    for path in sorted(directory.glob(f"{prefix}-*.json")):
        label = path.stem.removeprefix(prefix + "-")
        case, _, index = label.rpartition("-")
        if not index.isdecimal():
            continue  # Cargo no-op seed is setup, not a timed no-op sample.
        sample = json.loads(path.read_bytes())
        if sample["sample"] != path.stem:
            raise ValueError(f"sample ID does not match {path}")
        if case == "upload":  # Older receipts predate the bsync/SCP distinction.
            case = upload_case
        groups[case].append(sample)
    if not groups:
        raise ValueError(f"no timed receipts for {prefix} in {directory}")
    return groups


def values(samples, fields, scale):
    return [
        sum(sample["wall_ns"] if field == "wall_ns" else sample["metrics"][field]
            for field in fields) / scale
        for sample in samples if sample["error"] is None
    ]


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
    for case, label, fields, scale, cache in ROWS:
        if case in ("malloc-1t", "malloc-4t") and case not in box and case not in mac:
            continue  # Keep receipts from before these rows reproducible.
        box_receipts = box.get(case, [])
        left = values(box_receipts, fields, scale)
        right = values(mac.get(case, []), fields, scale)
        box_result = DEFERRED if case in ("sequential", "walk", "fsync") else summary(left)
        count = f"{len(left)}/{len(box_receipts)}" if box_receipts else "—"
        lines.append(f"| {label} | {box_result} | {summary(right)} | {count} | {cache} |")
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
