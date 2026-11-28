"""The checks `vigil-cli breenix bench record` applies to a schema-v1 series.

A port of Vigil's BreenixBenchSeries.validate() and BreenixProjects.text(), so a
series can be checked on this side before it is recorded. Vigil refuses the whole
file when one text field breaks a rule, so the series writer keeps every field
inside them.
"""
from datetime import datetime
import re
import unicodedata

# Field limits, in characters (Vigil BreenixBench.swift, validate()).
LIMITS = {"hardware": 200, "kernelCommit": 64, "userlandCommit": 64, "toolchain": 200,
          "session": 120, "note": 400}
ROW_LIMITS = {"title": 200, "cache": 300, "reason": 400}

IPV4 = re.compile(r"(?<![\w.])([0-9]{1,3}(?:\.[0-9]{1,3}){3})\b(?!\.[0-9])")
VERSION_PREFIX = re.compile(
    r"\b(?:versions?|ver\.?|releases?|built|build|revision|firmware|v|app)\s+$", re.IGNORECASE)
CREDENTIALS = re.compile(
    r"\b(?:password|passwd|secret|api[_-]?key|access[_-]?token|token)\s*[:=]\s*\S"
    r"|\bbearer\s+[a-z0-9._~-]+|-----begin [a-z ]*private key-----"
    r"|\b(?:gh[pousr]_|github_pat_)[a-z0-9_]+|\b[a-z0-9._+-]+@[a-z0-9.-]+", re.IGNORECASE)
HOST = re.compile(
    r"(?:\w+://|\bwww\.|\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:local|localhost|lan|internal|test|"
    r"example|invalid|com|net|org|io|dev|edu|gov|mil|cloud)\b|\blocalhost\b"
    r"|\[[a-f0-9:]*:[a-f0-9:]*\])", re.IGNORECASE)
APP_HOST = re.compile(r"\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.app\b")
SERIES_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
ROW_ID = re.compile(r"[a-z0-9]+(?:[._-][a-z0-9]+)*")


def text_problem(value, limit):
    """Why Vigil would refuse `value` as a text field of `limit` characters, or None."""
    if value is None:
        return None
    if (not value.strip() or len(value) > limit
            or any(unicodedata.category(ch) == "Cc" for ch in value)):
        return f"must be single-line text of 1-{limit} characters"
    if "." in value:
        for match in IPV4.finditer(value):
            if (not VERSION_PREFIX.search(value[:match.start()])
                    and all(int(part) <= 255 for part in match.group(0).split("."))):
                return "contains an IPv4 literal"
    if CREDENTIALS.search(value):
        return "contains credentials or a host address"
    if HOST.search(value) or (".app" in value and APP_HOST.search(value)):
        return "contains a host address"
    return None


def problems(series):
    """Every rule the series breaks, as text; empty when Vigil would record it."""
    found = []
    if series.get("version") != 1:
        found.append("version must be 1")
    series_id = series.get("id", "")
    if not (1 <= len(series_id) <= 120 and SERIES_ID.fullmatch(series_id)):
        found.append("id is not a series id")
    try:
        datetime.fromisoformat(series.get("at", "").replace("Z", "+00:00"))
    except ValueError:
        found.append("at is not an ISO 8601 time")
    if (series.get("system") == "mac") != (series.get("target") == "mac"):
        found.append("only a mac series runs on target mac")
    if series.get("hardware") is None:
        found.append("hardware is required")
    for field, limit in LIMITS.items():
        problem = text_problem(series.get(field), limit)
        if problem:
            found.append(f"{field} {problem}")
    if series.get("pr") is not None and series["pr"] <= 0:
        found.append("pr must be positive")
    rows = series.get("rows", [])
    if not 1 <= len(rows) <= 500:
        found.append("must have 1-500 rows")
    seen = set()
    for row in rows:
        name = f"row {row.get('id')}"
        if row.get("id") in seen:
            found.append(f"{name} is listed twice")
        seen.add(row.get("id"))
        if not (1 <= len(row.get("id", "")) <= 120 and ROW_ID.fullmatch(row.get("id", ""))):
            found.append(f"{name}: not a row id")
        for field, limit in ROW_LIMITS.items():
            value = row.get(field)
            if field == "cache" and value == "":
                continue
            problem = text_problem(value, limit)
            if problem:
                found.append(f"{name} {field} {problem}")
        if not 0 <= row.get("completed", -1) <= row.get("attempts", -1):
            found.append(f"{name}: completed must be at most attempts")
        values = [row.get(key) for key in ("median", "min", "max")]
        if row.get("state") == "measured":
            if (None in values or not all(value > 0 for value in values)
                    or not values[1] <= values[0] <= values[2] or row["completed"] < 1):
                found.append(f"{name}: a measured row needs 0 < min <= median <= max")
        elif values != [None, None, None]:
            found.append(f"{name}: a {row.get('state')} row has no median, min or max")
    return found
