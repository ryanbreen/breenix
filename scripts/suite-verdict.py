#!/usr/bin/env python3
"""Score one effort-suite boot against its manifest (docs/suites/README.md).

    scripts/suite-verdict.py MANIFEST RECORDS_LOG [OTHER_LOG ...] [--disk IMAGE] [--milestone EFFORT/ID]

RECORDS_LOG is the serial the suite's stdout went to; every log given is also
searched for fatal kernel output. The boot passes when the suite's records, each
a whole line, are exactly: one START with the manifest's case count, one CASE line
per manifest case in manifest order, and one DONE whose counts match those CASE
lines with failed=0, skipped=0 and passed=total=the manifest's case count;
the suite's memory record shows its memory reclaimed; and no log shows a kernel
panic, soft lockup or EL1 abort (`fatal` in docs/boot-path.json, plus x86-64's
"KERNEL PANIC:"). VALUE, WAIT, PULSES, HOSTKEY, HOSTPOINTER and HOSTSSH records, which a case may print
while it runs, are accepted between START and DONE when they are well formed and
name a manifest case; they do not count.

The memory record (docs/suites/README.md, "Memory record") of a full run is one
`MEMORY before` line after START and before the first CASE, then after DONE one
`MEMORY after` line and one `MEMORY reclaimed=PASS|FAIL free_before= free_after=
allowance=` line whose figures are those two lines' free counts and whose allowance
equals MEMORY_RECLAIM_ALLOWANCE_PAGES read from libs/libbreenix/src/suite_memory.rs. reclaimed=FAIL,
or a reclaimed verdict its own figures contradict, fails the suite like a failing
case. Selected runs (`only`) may omit memory; when present, the same memory checks apply.

A manifest with diskChecks also requires --disk: debugfs reads the stopped
VM image directly and compares the persisted bytes, bypassing the guest cache.

A category marked "hostRunner" is run from the boot host against a persistent
boot (scripts/metal/server-suite), not by the suite binary: the boot's records
leave it out, and a milestone that measures it is scored on that persistent boot.

With --milestone, the commit's effort path supplies the required category set.
Missing or planned measured categories block acceptance. The full boot's records,
memory and fatal-output checks still apply; only the measured categories' cases
must pass. The verdict names the category set and does not claim a full-suite PASS.

Prints "PASS: <DONE line>" or "FAIL: <reason>" and exits 0 or 1.
"""

import argparse
import hashlib
import json
import shutil
import subprocess
import tempfile
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
# The records a case prints while it runs, which do not count.
RESULT = ("FRAME", "TREE", "INPUT", "COLLECT")
LIVE = ("VALUE", "WAIT", "PULSES", "HOSTKEY", "HOSTPOINTER", "HOSTSSH", "SMBRESTART", "SMBSTOP", "SMBSTART", "SCREEN", "SCREENPERTURB", "EVIDENCE") + RESULT
# The runner's memory record, around the cases.
MEMORY = ("MEMORY", "RECLAIMED")


def bundle_shape(suite):
    return (rf"SUITE {suite} EVIDENCE bundle ([a-z0-9/-]+) label=(uitest-[0-9a-f]{{16}})"
            r" bytes=([1-9][0-9]*) sha256=([0-9a-f]{64})"
            r" result=(?P<status>ok|disk-only:[a-z0-9]+(?:-[a-z0-9]+)*) path=(?P<path>/var/uitest/[^ ]+)")


def bundle_disk_problem(records, disk):
    """Disk-only anchors need bytes read from the stopped image, not a guest claim."""
    retained = [match for kind, match, _ in records
                if kind == 'EVIDENCE' and match['status'] != 'ok']
    if not retained:
        return None
    if not disk:
        return 'disk-only UI bundles require --disk for readback'
    debugfs = next((path for path in (
        'debugfs', '/usr/sbin/debugfs', '/sbin/debugfs',
        '/opt/homebrew/opt/e2fsprogs/sbin/debugfs',
        '/usr/local/opt/e2fsprogs/sbin/debugfs') if shutil.which(path)), None)
    if not debugfs:
        return 'UI bundle readback requires debugfs (e2fsprogs)'
    with tempfile.TemporaryDirectory(prefix='suite-bundles-') as directory:
        for index, match in enumerate(retained):
            output = Path(directory) / str(index)
            path = match['path'] + '/retention.bundle'
            try:
                read = subprocess.run([debugfs, '-R', f'dump {path} {output}', str(disk)],
                                      capture_output=True, timeout=30, check=False)
                if read.returncode or not output.is_file():
                    return f'UI bundle is missing from raw disk: {path}'
                if (output.stat().st_size != int(match[3])
                        or hashlib.sha256(output.read_bytes()).hexdigest() != match[4]):
                    return f'UI bundle raw disk size or digest differs: {path}'
            except (OSError, subprocess.TimeoutExpired) as error:
                return f'UI bundle raw disk read failed: {error}'
    return None


def upload_problem(records, serial, directory):
    """Verify receiver receipts and reassembled payloads against the serial anchors."""
    try:
        directory = Path(directory)
        route_info = json.loads((directory / 'route.json').read_text())
        intent = json.loads((directory / 'gt710-upload.json').read_text())
        token = route_info['token']
        if not isinstance(token, str) or not re.fullmatch(r'[0-9a-f]{64}', token):
            raise ValueError('invalid route token')
        route = '/gt710-upload/' + token
        boot = intent['id']
        if not isinstance(boot, str) or not boot or intent.get('route') != route or intent.get('suite') != 'ui-testing':
            raise ValueError('receiver intent differs from route')

        def received(filename, receipt_name, suffix):
            body = (directory / filename).read_bytes()
            receipt = json.loads((directory / receipt_name).read_text())
            if (not 0 < len(body) <= 2 << 20 or receipt.get('id') != boot
                    or receipt.get('route') != route + suffix or receipt.get('complete') is not True
                    or type(receipt.get('bytes')) is not int or receipt['bytes'] != len(body)
                    or receipt.get('sha256') != hashlib.sha256(body).hexdigest()):
                raise ValueError('upload receipt or payload differs')
            return body

        lines = read_lines(serial)
        part_index = 0
        total = 0
        for label, filename, receipt, suffix in (
                ('startup', 'gt710-pixels.rle', 'native-upload.json', ''),
                ('suite-final-panel', 'gt710-final.rle', 'native-final-upload.json', '/final')):
            prefix = 'SUITE_EVIDENCE ui-testing ' + label + ' '
            anchors = [(i, re.fullmatch(re.escape(prefix) + r'bytes=([1-9][0-9]*) sha256=([a-f0-9]{64})', line))
                       for i, line in enumerate(lines) if line.startswith(prefix)]
            if len(anchors) != 1 or anchors[0][1] is None:
                raise ValueError('missing, refused or malformed ' + label + ' panel')
            at, anchor = anchors[0]
            before = next((i for i, line in enumerate(lines) if line.startswith('SUITE ui-testing MEMORY before ')), None)
            after = next((i for i, line in enumerate(lines) if line.startswith('SUITE ui-testing MEMORY reclaimed=')), None)
            if before is not None and label == 'startup' and at >= before or after is not None and label == 'suite-final-panel' and at <= after:
                raise ValueError('panel outside its evidence slot')
            body = received(filename, receipt, suffix)
            total += len(body)
            parts = re.fullmatch(rb'SUITE_PARTS v=1 first=(0|[1-9]\d*) parts=([1-9]\d*) bytes=([1-9]\d*) sha256=([a-f0-9]{64})\n', body)
            if parts:
                first, count, size = map(int, parts.groups()[:3])
                if first != part_index or not (2 << 20) < size <= 64 << 20 or count != (size + (2 << 20) - 1) // (2 << 20):
                    raise ValueError('invalid multipart range')
                chunks = []
                for index in range(first, first + count):
                    chunk = received(f'gt710-breengel-{index:04}.rle', f'native-picture-upload-{index:04}.json', f'/pictures/{index}')
                    if len(chunk) != min(2 << 20, size - (index - first) * (2 << 20)):
                        raise ValueError('multipart chunk size differs')
                    chunks.append(chunk)
                body = b''.join(chunks)
                total += len(body)
                if hashlib.sha256(body).hexdigest() != parts[4].decode():
                    raise ValueError('multipart digest differs')
                part_index += count
            if len(body) != int(anchor[1]) or hashlib.sha256(body).hexdigest() != anchor[2]:
                raise ValueError('panel differs from serial anchor')
        if len(list(directory.glob('gt710-breengel-*.rle'))) != part_index:
            raise ValueError('unanchored picture parts')
        for kind, match, _ in records:
            if kind != 'EVIDENCE':
                continue
            if match['status'] != 'ok':
                raise ValueError('staged route requires bundle upload')
            label = match[2]
            body = received(f'diagnostic-{label}.bin', f'native-diagnostic-{label}.json', '/diagnostics/' + label)
            total += len(body)
            if len(body) != int(match[3]) or hashlib.sha256(body).hexdigest() != match[4]:
                raise ValueError('bundle differs from serial anchor')
        if total > 64 << 20:
            raise ValueError('uploads exceed evidence bound')
    except (OSError, ValueError, KeyError, TypeError) as error:
        return f'UI upload evidence invalid: {error}'
    return None


def read_lines(path):
    text = Path(path).read_bytes().decode("utf-8", errors="replace")
    return [line.rstrip("\r") for line in text.split("\n")]


def milestone_categories(path, milestone, suite):
    """Resolve a suite measure from the boot commit's effort path."""
    for effort in path["efforts"]:
        for item in effort["milestones"]:
            if effort["id"] + "/" + item["id"] == milestone:
                measure = item["measure"]
                if measure.get("suite") != suite or not measure.get("categories"):
                    raise ValueError(f"{milestone} does not measure categories of suite {suite}")
                return measure["categories"]
    raise ValueError(f"unknown milestone {milestone}")


def parse_result(line, suite, case, checks, collects):
    """The section 8.3 grammar, also exercised against suite_result.rs on the host.

    Returns (duplicate key, matched FRAME check or None, mismatch check or None).
    No image or reference bytes are read by this parser.
    """
    def name(value):
        return re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", value) is not None

    def hexdigits(value, size):
        return re.fullmatch(rf"[0-9a-f]{{{size}}}", value) is not None

    def field(token, key):
        if not token.startswith(key + "="):
            raise ValueError("malformed result field")
        return token[len(key) + 1:]

    def number(token, key):
        value = field(token, key)
        if re.fullmatch(r"[0-9]+", value) is None or len(value) > 20 or int(value) > 2**64 - 1:
            raise ValueError("malformed result number")
        return int(value)

    def artifacts(value):
        return value == "complete" or re.fullmatch(r"(?:partial|none):[a-z0-9]+(?:-[a-z0-9]+)*", value) is not None

    t = line.split(" ")
    if (len(line.encode("utf-8")) + 1 > 4096 or not line.isascii() or len(t) < 4
            or any(not token or any(ord(c) <= 32 or ord(c) == 127 for c in token) for token in t)
            or t[0] != "SUITE" or t[1] != suite or t[3] != case):
        raise ValueError("malformed or other-case result record")
    kind = t[2]
    if kind == "FRAME" and len(t) in (14, 15, 16, 20, 21):
        check = t[4]
        reference, source = field(t[6], "ref"), field(t[7], "source")
        if (not name(check) or check not in checks or not hexdigits(field(t[5], "def"), 8)
                or not (reference in ("formula", "structural") or hexdigits(reference, 8))
                or not (source in ("screen", "surface") or source.startswith("window:") and name(source[7:]))):
            raise ValueError("malformed or undeclared FRAME check")
        number(t[8], "commit")
        number(t[9], "epoch")
        width, height = number(t[10], "w"), number(t[11], "h")
        if not width or not height or not hexdigits(field(t[12], "sha256"), 64):
            raise ValueError("malformed FRAME dimensions or hash")
        result = field(t[13], "result")
        valid_path = lambda token: field(token, "path") == f"/var/uitest/{suite}/{case}"
        if result == "match" and (len(t) == 14
                or len(t) == 15 and artifacts(field(t[14], "artifacts")) and t[14] != "artifacts=complete"
                or len(t) == 16 and t[14] == "artifacts=complete" and valid_path(t[15])):
            return "FRAME:" + check, check, None
        if result == "mismatch" and len(t) in (20, 21):
            compared, differing = number(t[14], "compared"), number(t[15], "differing")
            first = field(t[16], "first").split(",")
            if len(first) != 2:
                raise ValueError("malformed first pixel")
            x, y = number("x=" + first[0], "x"), number("y=" + first[1], "y")
            if (not 0 < differing <= compared <= width * height <= 2**64 - 1 or x >= width or y >= height
                    or re.fullmatch(r"#[0-9a-f]{6}", field(t[17], "got")) is None
                    or re.fullmatch(r"#[0-9a-f]{6}", field(t[18], "want")) is None
                    or not artifacts(field(t[19], "artifacts"))
                    or len(t) == 21 and (t[19] != "artifacts=complete" or not valid_path(t[20]))):
                raise ValueError("malformed FRAME mismatch")
            return "FRAME:" + check, None, check
    elif kind == "TREE" and len(t) == 9:
        if not name(t[4]) or t[4] not in checks or not name(field(t[5], "window")) or not hexdigits(field(t[8], "sha256"), 64):
            raise ValueError("malformed or undeclared TREE check")
        number(t[6], "commit")
        number(t[7], "nodes")
        return "TREE:" + t[4], None, None
    elif kind == "INPUT" and len(t) in (9, 10):
        for token, key in zip(t[4:9], ("sent", "processed", "handled", "rejected", "lost")):
            number(token, key)
        if len(t) == 10:
            number(t[9], "missing")
        return "INPUT", None, None
    elif kind == "COLLECT" and len(t) == 9:
        screen = field(t[5], "screen")
        if (not collects or field(t[4], "trigger") not in ("fail", "crash", "timeout", "save-all")
                or not (screen in ("at-failure", "at-collection") or screen.startswith("none:") and name(screen[5:]))
                or not artifacts(field(t[8], "artifacts"))):
            raise ValueError("malformed or unrequested COLLECT")
        number(t[6], "windows")
        number(t[7], "remnants")
        return "COLLECT", None, None
    raise ValueError("malformed result record")


def result_problem(manifest, records, cases):
    """Bind records to the active case; a guest PASS must satisfy every FRAME obligation."""
    checks = {f"{cat['id']}/{row['id']}": [] for cat in manifest["categories"] for row in cat["cases"]}
    for entry in manifest.get("frameChecks", []):
        case, check = entry.get("case"), entry.get("check")
        if case not in checks:
            return "declared FRAME check names no manifest case"
        if (not isinstance(check, str) or re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", check) is None
                or check in checks[case]):
            return "invalid or duplicate declared FRAME check"
        checks[case].append(check)
        if len(checks[case]) > 256:
            return "more than 256 declared checks"
    collects = {f"{cat['id']}/{row['id']}": row.get("collects", False)
                for cat in manifest["categories"] for row in cat["cases"]}
    seen, matched, mismatched = set(), set(), set()
    count, index, started, done = 0, 0, False, False
    bundles = set()
    uploads, bundle_bytes = 0, 0
    for kind, match, line in records:
        if kind == "START":
            started = True
        elif kind == "DONE":
            done = True
        elif kind == "EVIDENCE":
            case = match.group(1)
            uploads += match['status'] == 'ok'
            bundle_bytes += int(match[3])
            if (manifest['id'] != 'ui-testing' or not started or done or index >= len(cases)
                    or case != cases[index] or case in bundles or uploads > 16 or bundle_bytes > 32 << 20
                    or match.group(2) != 'uitest-' + hashlib.sha256(case.encode()).hexdigest()[:16]
                    or int(match.group(3)) > 2 << 20
                    or match['path'] != '/var/uitest/ui-testing/' + case):
                return "UI bundle record binding, size or slot limit differs"
            bundles.add(case)
        elif kind in ("SMBSTOP", "SMBSTART"):
            if not started or done or index >= len(cases) or match.group(1) != cases[index]:
                return f"a {kind} record is not bound to the running case"
        elif kind in RESULT:
            if not started or done or index >= len(cases) or match.group(1) != cases[index]:
                return f"a {kind} record is not bound to the running case"
            case = cases[index]
            count += 1
            if count > 256:
                return "result record limit exceeded (256)"
            try:
                key, hit, miss = parse_result(line, manifest["id"], case, checks[case], collects[case])
            except ValueError as error:
                return str(error)
            if key in seen:
                return f"duplicate result record {key}"
            seen.add(key)
            if hit:
                matched.add(hit)
            if miss:
                mismatched.add(miss)
        elif kind in ("PASS", "FAIL", "SKIP"):
            if index < len(cases) and match.group(1) == cases[index] and kind == "PASS":
                if mismatched:
                    return f"frame check `{sorted(mismatched)[0]}` mismatched but CASE passed"
                missing = [check for check in checks[cases[index]] if check not in matched]
                if missing:
                    return f"frame check `{missing[0]}` never ran but CASE passed"
            index += 1
            count, seen, matched, mismatched = 0, set(), set(), set()
    return None


def verdict(manifest, records_log, logs, only=None, categories=None, disk=None, uploads=None):
    suite = manifest["id"]
    planned = [category["id"] for category in manifest["categories"] if not category["cases"]]
    hosted = [category["id"] for category in manifest["categories"] if category.get("hostRunner")]
    if categories is not None:
        if only or not categories:
            return False, "milestone acceptance needs categories and a full boot"
        known = {category["id"] for category in manifest["categories"]}
        missing = set(categories) - known
        if missing:
            return False, f"missing measured categories: {', '.join(sorted(missing))}"
        elsewhere = [category for category in categories if category in hosted]
        if elsewhere:
            return False, (f"host-run categories are scored on their persistent boot, not this one: {', '.join(elsewhere)}"
                           "; run scripts/metal/server-suite against it")
        planned = [category for category in planned if category in categories]
    if not only and planned:
        return False, f"planned categories have no cases: {', '.join(planned)}; acceptance is blocked"
    cases = [f"{category['id']}/{case['id']}" for category in manifest["categories"]
             if category["id"] not in hosted for case in category["cases"]]
    if only:
        selected = [case for case in cases if case == only or case.split("/", 1)[0] == only]
        if not selected:
            return False, f"unknown case {only}; valid ids: {', '.join(cases)}"
        cases = selected
    suffix = rf" only={re.escape(only)}" if only else ""
    fatal = json.loads((REPO / "docs/boot-path.json").read_text())["fatal"] + [r"KERNEL PANIC:"]
    for log in logs:
        for line in read_lines(log):
            for pattern in fatal:
                if re.search(pattern, line):
                    return False, f"fatal kernel output in {log}: {line.strip()[:200]}"

    prefix = f"SUITE {suite} "
    name = r"([a-z0-9]+(?:-[a-z0-9]+)*/[a-z0-9]+(?:-[a-z0-9]+)*)"
    picture = r"(?: sha256=[0-9a-f]{64})?"
    pace = r"(?: pace_ms=([1-9][0-9]{0,9}))?" if suite == "breengel" else ""
    proof = r"(?: screen_proof=on)?" if suite in ("gpu", "breengel") else ""
    shapes = {
        "START": re.compile(rf"^SUITE {suite} START cases=(\d+){suffix}{pace}{proof}$"),
        "PASS": re.compile(rf"^SUITE {suite} CASE {name} PASS ms=\d+{picture}$"),
        "FAIL": re.compile(rf"^SUITE {suite} CASE {name} FAIL ms=\d+{picture} msg=\S.*$"),
        "SKIP": re.compile(rf"^SUITE {suite} CASE {name} SKIP msg=\S.*$"),
        "DONE": re.compile(rf"^SUITE {suite} DONE passed=(\d+) failed=(\d+) skipped=(\d+) total=(\d+){suffix}$"),
        "ELAPSED": re.compile(rf"^SUITE {suite} ELAPSED ms=(\d+)$"),
        "VALUE": re.compile(rf"^SUITE {suite} VALUE {name} [a-z0-9]+(?:-[a-z0-9]+)*=(-?\d+(?:\.\d+)?)([a-z]*)"
                            r"(?: expect=(-?\d+(?:\.\d+)?)\.\.(-?\d+(?:\.\d+)?)([a-z]*))?$"),
        "SCREEN": re.compile(rf"^SUITE {suite} SCREEN {name} "
                             r"(?:end unobservable reason=(?:device-reset|target-invalidated)"
                             r"(?: errno=[1-9][0-9]* eio_reason=[0-9]+ failed_submission=[0-9]+)?"
                             r"|[a-z0-9]+(?:-[a-z0-9]+)* "
                             r"(?:sha256=[0-9a-f]{64} mode=\d+x\d+ binding=\d+x\d+(?: crc=[0-9a-f]{8} crc2=(?:[0-9a-f]{8}|none) raster=\d+x\d+ out=sor\d+)?"
                             r"|error=[a-z0-9]+(?:-[a-z0-9]+)*(?: errno=[1-9][0-9]* eio_reason=[0-9]+ failed_submission=[0-9]+)?))$"),
        "WAIT": re.compile(rf"^SUITE {suite} WAIT {name} until=\d+ for=\d+ result=[a-z0-9]+(?:-[a-z0-9]+)*$"),
        "HOSTKEY": re.compile(rf"^SUITE {suite} HOSTKEY {name} seq=\d+ keys=[a-z0-9]+(?:-[a-z0-9]+)*(?:,[a-z0-9]+(?:-[a-z0-9]+)*)*(?: ticket=\d+)?$"),
        "HOSTPOINTER": re.compile(rf"^SUITE {suite} HOSTPOINTER {name} seq=\d+ x=\d+ y=\d+ width=\d+ height=\d+ buttons=[0-7] wheel=-?[01](?: ticket=\d+)?$"),
        "HOSTSSH": re.compile(rf"^SUITE {suite} HOSTSSH {name} seq=\d+ try=(?:account|root) addr=\d+\.\d+\.\d+\.\d+"
                              rf" port=\d+ user=[a-z_][a-z0-9_-]* fp=SHA256:[A-Za-z0-9+/]+$"),
        "PULSES": re.compile(rf"^SUITE {suite} PULSES {name} every=\d+ limit=\d+ at=(?:-?\d+(?:,-?\d+)*)?$"),
        "MEMORY": re.compile(rf"^SUITE {suite} MEMORY (before|after) free=(\d+) heap_free_kb=\d+(?: [a-z_]+=\d+)+$"),
        "RECLAIMED": re.compile(rf"^SUITE {suite} MEMORY reclaimed=(PASS|FAIL) free_before=(\d+) free_after=(\d+) allowance=(\d+)$"),
    }
    if suite == "gui-apps":
        for action in ("SMBSTOP", "SMBSTART"):
            shapes[action] = re.compile(
                rf"^SUITE gui-apps {action} (guskit/smb-share-gone) seq=[0-9]{{1,20}}$")
    if suite == "network-shares":
        shapes['SMBRESTART'] = re.compile(
            r"^SUITE network-shares SMBRESTART (smb/server-restart) seq=[0-9]{1,20}$")
    if suite in ("gpu", "breengel"):
        # Same wire shape as screen-verdict.py; keep the suite scorer self-contained.
        fields = (r'(?:phase=[1-4] sha256=[0-9a-f]{64} mode=[1-9][0-9]*x[1-9][0-9]*'
                  r' binding=[1-9][0-9]*x[1-9][0-9]* crc=[0-9a-f]{8} crc2=(?:[0-9a-f]{8}|none)'
                  r' raster=[1-9][0-9]*x[1-9][0-9]* out=sor[0-9]+ resource=[1-9][0-9]*'
                  r' present_fence=[1-9][0-9]* fired=[0-9]+ disarmed=[01]|error=sequence-failed)')
        shapes['SCREENPERTURB'] = re.compile(rf"^SUITE {suite} SCREENPERTURB {name} end {fields}$")
    if suite == "ui-testing":
        shapes['EVIDENCE'] = re.compile('^' + bundle_shape(suite) + '$')
    shapes.update({kind: re.compile(rf"^SUITE {suite} {kind} {name} .+$") for kind in RESULT})
    records = []
    for line in read_lines(records_log):
        if not line.startswith(prefix):
            if prefix in line:
                return False, f"a SUITE record does not start its line: {line.strip()[:200]}"
            continue
        if not only and re.match(rf"{re.escape(prefix)}(?:START|DONE) .* only=\S+$", line):
            return False, "one-case run is not full-suite evidence"
        if line.startswith(prefix + "ERROR "):
            return False, line
        for kind, shape in shapes.items():
            match = shape.match(line)
            if match:
                records.append((kind, match, line))
                break
        else:
            return False, f"a SUITE line has none of the known shapes: {line[:200]}"

    if not records:
        return False, f"no 'SUITE {suite}' lines in {records_log}"
    problem = result_problem(manifest, records, cases)
    if problem:
        return False, problem
    problem = upload_problem(records, records_log, uploads) if suite == "ui-testing" and uploads else None
    if problem:
        return False, problem
    problem = bundle_disk_problem(records, disk)
    if problem:
        return False, problem
    for index, (kind, match, line) in enumerate(records):
        if kind not in LIVE:
            continue
        if match.group(1) not in cases:
            return False, f"a {kind} record names no manifest case: {line[:200]}"
        if kind == "VALUE" and match.group(6) is not None and match.group(6) != match.group(3):
            return False, f"a VALUE record's expected range is in another unit: {line[:200]}"
        if not any(k == "START" for k, _, _ in records[:index]) or any(k == "DONE" for k, _, _ in records[:index]):
            return False, f"a {kind} record is outside START..DONE: {line[:200]}"
    elapsed = [i for i, (kind, _, _) in enumerate(records) if kind == "ELAPSED"]
    done = [i for i, (kind, _, _) in enumerate(records) if kind == "DONE"]
    if elapsed and (len(elapsed) != 1 or len(done) != 1 or elapsed[0] <= done[0]):
        return False, "ELAPSED needs one record after DONE"
    memory = memory_problem(records) if not only or any(k in MEMORY for k, _, _ in records) else None
    records = [record for record in records if record[0] not in LIVE + MEMORY + ("ELAPSED",)]
    if not records:
        return False, "the records do not begin with exactly one START line"
    kind, match, line = records[0]
    if kind != "START" or [r[0] for r in records].count("START") != 1:
        return False, "the records do not begin with exactly one START line"
    if suite == "breengel" and match.group(2) is not None and int(match.group(2)) > 4294967295:
        return False, "pace_ms exceeds unsigned 32-bit milliseconds"
    if int(match.group(1)) != len(cases):
        return False, f"{line} but the manifest lists {len(cases)} cases"

    ran = [(kind, match.group(1)) for kind, match, _ in records if kind in ("PASS", "FAIL", "SKIP")]
    for index, expected in enumerate(cases):
        if index >= len(ran):
            stopped = f"after {ran[-1][1]}" if ran else "before its first case"
            return False, f"the suite stopped {stopped}: no CASE line for {expected}"
        if ran[index][1] != expected:
            return False, f"CASE line {index + 1} is {ran[index][1]}; the manifest's case {index + 1} is {expected}"
    if len(ran) > len(cases):
        return False, f"{len(ran)} CASE lines for the manifest's {len(cases)} cases"

    kind, match, line = records[-1]
    if kind != "DONE" or [r[0] for r in records].count("DONE") != 1:
        return False, "the records do not end with exactly one DONE line"
    passed, failed, skipped, total = (int(match.group(i)) for i in range(1, 5))
    tally = (sum(k == "PASS" for k, _ in ran), sum(k == "FAIL" for k, _ in ran), sum(k == "SKIP" for k, _ in ran))
    if (passed, failed, skipped) != tally or total != len(cases):
        return False, f"{line} disagrees with its CASE lines ({tally[0]} passed, {tally[1]} failed, {tally[2]} skipped of {len(cases)})"
    also = f"; {memory[1]}" if memory and not memory[0] else ""
    accepted = [(kind, name) for kind, name in ran if categories is None or name.split("/", 1)[0] in categories]
    failed = sum(kind == "FAIL" for kind, _ in accepted)
    skipped = sum(kind == "SKIP" for kind, _ in accepted)
    if categories is not None:
        line = f"milestone categories={','.join(categories)} passed={sum(k == 'PASS' for k, _ in accepted)} total={len(accepted)}"
    elif hosted:
        line = f"{line}; host-run categories {','.join(hosted)} are scored on their persistent boot"
    if failed:
        failures = "; ".join(l[len(prefix):] for k, m, l in records if k == "FAIL" and (categories is None or m.group(1).split("/", 1)[0] in categories))
        return False, f"{line}: {failures[:400]}{also}"
    if skipped:
        skips = "; ".join(l[len(prefix):] for k, m, l in records if k == "SKIP" and (categories is None or m.group(1).split("/", 1)[0] in categories))
        return False, f"{line}: suite acceptance requires skipped=0 and passed=total={len(accepted)}; {skips[:400]}{also}"
    if memory is None:
        return True, line
    return memory[0], f"{line}; {memory[1]}" if memory[0] else f"{line}: {memory[1]}"


def memory_reclaim_allowance_pages():
    """Read the runner's canonical allowance, including in a retained definition tree."""
    source = REPO / 'libs/libbreenix/src/suite_memory.rs'
    values = re.findall(r'^pub const MEMORY_RECLAIM_ALLOWANCE_PAGES: u64 = (\d+);$',
                        source.read_text(), re.M)
    if len(values) != 1:
        raise ValueError('memory allowance source must define MEMORY_RECLAIM_ALLOWANCE_PAGES once')
    return int(values[0])


def memory_problem(records):
    """(reclaimed, detail) for a full run's memory record, in serial order among its other records."""
    kinds = [kind for kind, _, _ in records]
    lines = {kind: [(index, match, line) for index, (k, match, line) in enumerate(records) if k == kind]
             for kind in ("MEMORY", "RECLAIMED")}
    before = [entry for entry in lines["MEMORY"] if entry[1].group(1) == "before"]
    after = [entry for entry in lines["MEMORY"] if entry[1].group(1) == "after"]
    if len(before) != 1 or len(after) != 1 or len(lines["RECLAIMED"]) != 1:
        return False, (f"the memory record needs one MEMORY before, one MEMORY after and one MEMORY reclaimed line; "
                       f"found {len(before)}, {len(after)} and {len(lines['RECLAIMED'])}")
    (b_at, b, _), (a_at, a, _), (r_at, r, reclaimed) = before[0], after[0], lines["RECLAIMED"][0]
    start = kinds.index("START") if "START" in kinds else len(kinds)
    first = next((i for i, k in enumerate(kinds) if k in ("PASS", "FAIL", "SKIP", "DONE")), len(kinds))
    done = kinds.index("DONE") if "DONE" in kinds else len(kinds)
    if not start < b_at < first:
        return False, "MEMORY before is not between START and the first CASE"
    if not done < a_at < r_at:
        return False, "MEMORY after and MEMORY reclaimed do not follow DONE in that order"
    passed, free_before, free_after, allowance = r.group(1) == "PASS", *(int(r.group(i)) for i in (2, 3, 4))
    try:
        expected_allowance = memory_reclaim_allowance_pages()
    except (OSError, ValueError) as error:
        return False, f"cannot read memory allowance: {error}"
    if allowance != expected_allowance:
        return False, f"memory allowance {allowance} differs from MEMORY_RECLAIM_ALLOWANCE_PAGES={expected_allowance}"
    if (free_before, free_after) != (int(b.group(2)), int(a.group(2))):
        return False, f"{reclaimed[len('SUITE '):]} does not repeat the free counts of its MEMORY before and after lines"
    if passed != (free_after + allowance >= free_before):
        return False, f"{reclaimed[len('SUITE '):]} contradicts its own figures"
    if not passed:
        return False, (f"memory not reclaimed: {free_before - free_after} pages short after the reap, "
                       f"allowance {allowance}: {a.string[len('SUITE '):]}")
    return True, reclaimed[len("SUITE "):]


def disk_verdict(checks, disk):
    if not checks:
        return True, ""
    if not disk:
        return False, "the manifest requires --disk for raw writeback checks"
    debugfs = next((shutil.which(path) for path in (
        "debugfs", "/usr/sbin/debugfs", "/sbin/debugfs",
        "/opt/homebrew/opt/e2fsprogs/sbin/debugfs",
        "/usr/local/opt/e2fsprogs/sbin/debugfs",
    ) if shutil.which(path)), None)
    if not debugfs:
        return False, "raw disk checks require debugfs (e2fsprogs)"
    with tempfile.TemporaryDirectory(prefix="suite-disk-") as directory:
        for index, check in enumerate(checks):
            output = Path(directory) / str(index)
            try:
                result = subprocess.run(
                    [debugfs, "-R", f"dump {check['path']} {output}", str(disk)],
                    capture_output=True, timeout=30, check=False,
                )
                if result.returncode or not output.exists():
                    return False, f"raw disk file is missing: {check['path']}"
                actual = output.read_bytes()
            except (OSError, subprocess.TimeoutExpired) as error:
                return False, f"raw disk read failed: {error}"
            expected = bytes([check["byte"]]) * check["length"]
            if actual != expected:
                return False, f"raw disk bytes differ: {check['path']}"
    return True, f"raw disk checks passed={len(checks)}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest")
    parser.add_argument("logs", nargs="+")
    parser.add_argument("--disk", help="stopped VM's raw ext2 image for diskChecks and disk-only UI bundles")
    parser.add_argument("--uploads", help="receiver evidence directory for UI upload verification")
    parser.add_argument("--milestone", help="effort/milestone whose category set this full boot must pass")
    args = parser.parse_args()
    manifest = json.loads(Path(args.manifest).read_text())
    try:
        categories = milestone_categories(json.loads((REPO / "docs/efforts/path.json").read_text()),
                                          args.milestone, manifest["id"]) if args.milestone else None
    except ValueError as error:
        parser.error(str(error))
    ok, reason = verdict(manifest, args.logs[0], args.logs, categories=categories, disk=args.disk, uploads=args.uploads)
    if ok:
        ok, disk_reason = disk_verdict(manifest.get("diskChecks", []), args.disk)
        if disk_reason:
            reason = f"{reason}; {disk_reason}"
    print(f"{'PASS' if ok else 'FAIL'}: {reason}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
