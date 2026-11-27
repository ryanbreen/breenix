"""Build one schema-v1 bbench series from saved receipts.

Vigil records a series only when every text field fits its rules (vigil_rules.py),
so the writer keeps each field inside them. Anything that does not fit, and the
full provenance the caller gives, goes to a sidecar beside the series,
`<id>.provenance.json`, which the series note names.
"""

import json
from pathlib import Path
from statistics import median
from tempfile import NamedTemporaryFile
import unicodedata

from tables import DEFERRED_DISK, RETIRED, ROWS, better, unit, values
import vigil_rules

ELLIPSIS = "…"


def series_rows(groups, system, target, deferred=None, cache=None):
    """Yield the rows. `deferred` maps a case a system cannot run to the reason;
    `cache` maps a case to text appended to its rows' cache description."""
    deferred = deferred or {}
    cache = cache or {}
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
            if row.case in deferred:
                reason = deferred[row.case]
            elif system == "breenix" and target == "x86-metal" and row.case in DEFERRED_DISK:
                reason = "#3226"
            elif row.case in RETIRED:
                reason = RETIRED[row.case]
            else:
                reason = "No timed receipts"
        yield {
            "id": row.id, "title": row.title, "area": row.id.split(".", 1)[0],
            "unit": unit(row), "better": better(row),
            "state": state, "median": median(times) if times else None,
            "min": min(times) if times else None, "max": max(times) if times else None,
            "completed": len(times), "attempts": len(samples),
            "cache": f"{row.cache}; {cache[row.case]}" if row.case in cache else row.cache,
            "reason": reason,
        }


def fit(value, limit):
    """`value` as Vigil accepts it: one line, at most `limit` characters, with IPv4
    literals, host names and credential-like text replaced by a placeholder."""
    if value is None:
        return None
    text = "".join(" " if unicodedata.category(ch) == "Cc" else ch for ch in value)
    text = " ".join(text.split())
    text = vigil_rules.IPV4.sub(
        lambda match: match.group(0) if vigil_rules.VERSION_PREFIX.search(
            text[:match.start()]) else "an address", text)
    text = vigil_rules.CREDENTIALS.sub("[redacted]", text)
    text = vigil_rules.HOST.sub("a host", text)
    text = vigil_rules.APP_HOST.sub("an app", text)
    if len(text) > limit:
        text = text[:limit - 1].rstrip() + ELLIPSIS
    if not text.strip() or vigil_rules.text_problem(text, limit):
        return "See the provenance sidecar"[:limit]
    return text


def fit_series(series, sidecar_name, has_provenance):
    """Fit every text field in place. Returns the original of each changed field."""
    shortened = {}
    for row in series["rows"]:
        for field, limit in vigil_rules.ROW_LIMITS.items():
            value = row.get(field)
            if value is None or (field == "cache" and value == ""):
                continue
            fitted = fit(value, limit)
            if fitted != value:
                shortened[f"rows.{row['id']}.{field}"] = value
                row[field] = fitted
    for field, limit in vigil_rules.LIMITS.items():
        value = series.get(field)
        if field == "note" or value is None:
            continue
        fitted = fit(value, limit)
        if fitted != value:
            shortened[field] = value
            series[field] = fitted
    note = series.get("note")
    limit = vigil_rules.LIMITS["note"]
    if has_provenance or shortened or (note is not None and fit(note, limit) != note):
        suffix = f"Full provenance: {sidecar_name}."
        body = fit(note, limit - len(suffix) - 1) if note else None
        if note is not None and body != note:
            shortened["note"] = note
        series["note"] = f"{body} {suffix}" if body else suffix
    return shortened


def atomic_write(directory, path, encoded):
    with NamedTemporaryFile(mode="w", encoding="utf-8", dir=directory, delete=False) as file:
        pending = Path(file.name)
        try:
            file.write(encoded)
            file.close()
            pending.replace(path)
        finally:
            pending.unlink(missing_ok=True)


def write_series(directory, series_id, at, metadata, groups, deferred=None, provenance=None,
                 cache=None):
    """Write `<series_id>.series.json`, and its provenance sidecar when there is
    provenance or a field had to be shortened. Refuses to write a series Vigil
    would refuse."""
    series = {
        "version": 1, "id": series_id, "at": at, **metadata,
        "rows": list(series_rows(groups, metadata["system"], metadata["target"], deferred,
                                 cache)),
    }
    sidecar_name = f"{series_id}.provenance.json"
    shortened = fit_series(series, sidecar_name, provenance is not None)
    problems = vigil_rules.problems(series)
    if problems:
        raise ValueError(f"series {series_id} would be refused by Vigil: {'; '.join(problems)}")
    directory.mkdir(parents=True, exist_ok=True)
    if provenance is not None or shortened:
        sidecar = {"version": 1, "series": series_id, "provenance": provenance or {},
                   "shortened": shortened}
        atomic_write(directory, directory / sidecar_name,
                     json.dumps(sidecar, indent=2, allow_nan=False) + "\n")
    path = directory / f"{series_id}.series.json"
    atomic_write(directory, path, json.dumps(series, indent=2, allow_nan=False) + "\n")
    return path
