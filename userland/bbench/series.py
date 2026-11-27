"""Build one schema-v1 bbench series from saved receipts."""

import json
from pathlib import Path
from statistics import median
from tempfile import NamedTemporaryFile

from tables import DEFERRED_DISK, ROWS, SECOND, values


def series_rows(groups, system, target):
    seed_failure = next((sample["error"] for sample in groups.get("noop-seed", [])
                         if sample["error"]), None)
    for row in ROWS:
        samples = groups.get(row.case, [])
        times = values(samples, row.fields, row.scale)
        reason = None
        if times:
            state = "measured"
        elif samples:
            state = "failed"
            reason = ", ".join(dict.fromkeys(sample["error"] for sample in samples))
        elif row.case == "cargo-noop" and seed_failure:
            state = "failed"
            reason = f"Cargo no-op setup: {seed_failure}"
        else:
            state = "deferred"
            reason = ("#3226" if system == "breenix" and target == "x86-metal"
                      and row.case in DEFERRED_DISK else "No timed receipts")
        yield {
            "id": row.id, "title": row.title, "area": row.id.split(".", 1)[0],
            "unit": "s" if row.scale == SECOND else "ms", "better": "lower",
            "state": state, "median": median(times) if times else None,
            "min": min(times) if times else None, "max": max(times) if times else None,
            "completed": len(times), "attempts": len(samples), "cache": row.cache,
            "reason": reason,
        }


def write_series(directory, series_id, at, metadata, groups):
    series = {
        "version": 1, "id": series_id, "at": at, **metadata,
        "rows": list(series_rows(groups, metadata["system"], metadata["target"])),
    }
    encoded = json.dumps(series, indent=2, allow_nan=False) + "\n"
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f"{series_id}.series.json"
    with NamedTemporaryFile(mode="w", encoding="utf-8", dir=directory, delete=False) as file:
        pending = Path(file.name)
        try:
            file.write(encoded)
            file.close()
            pending.replace(path)
        finally:
            pending.unlink(missing_ok=True)
    return path
