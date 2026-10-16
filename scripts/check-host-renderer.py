#!/usr/bin/env python3
"""Judge the host renderer attestation of a GL QEMU run (docs/gui/qemu-gl.md).

    scripts/check-host-renderer.py ATTESTATION [--serial LOG]

ATTESTATION is the host-renderer.json the GL QEMU wrote for the run (the
BREENIX_HOST_RENDERER_JSON file of scripts/qemu-gl/host-renderer-attestation.patch).
The run is refused, with the reason, when:

  - the file is missing, is not format 1, or records no renderer context;
  - any record's event is not context-create or first-submit-3d, or its ctx_id is not
    a positive integer;
  - a first-submit-3d record has no context-create of the same ctx_id before it, or
    repeats one already recorded for that context;
  - no record is a first-submit-3d: the guest never submitted commands, so the file
    says nothing about the renderer that executed them;
  - any record lacks its renderer strings;
  - any record names a software renderer: llvmpipe, softpipe, lavapipe, SwiftShader,
    or any renderer string containing "Software";
  - any record names a renderer other than this target's declared one, the Apple GPU
    through ANGLE's Metal backend ("ANGLE Metal Renderer: Apple ...");
  - the serial log carries the guest's capset renderer string (a line
    VIRGL_CAPSET_RENDERER=<string>) and that string is not part of the host's.

Exit 0 accepts the run and prints the renderer; exit 1 refuses it.
"""
import argparse
import json
import re
import sys

SOFTWARE = re.compile(r"llvmpipe|softpipe|lavapipe|swiftshader|software", re.IGNORECASE)
DECLARED = re.compile(r"ANGLE Metal Renderer: Apple ")
DECLARED_NAME = "the Apple GPU through ANGLE's Metal backend"
EVENTS = ("context-create", "first-submit-3d")


def judge(attestation, serial):
    try:
        with open(attestation, encoding="utf-8") as f:
            doc = json.load(f)
    except FileNotFoundError:
        return "no host renderer attestation: the guest created no VirGL context"
    except (OSError, ValueError) as e:
        return f"unreadable host renderer attestation: {e}"
    if not isinstance(doc, dict) or doc.get("format") != 1:
        return "the host renderer attestation is not format 1"
    records = doc.get("records") or []
    if not isinstance(records, list) or not records:
        return "the host renderer attestation records no renderer context"
    renderers = set()
    created, submitted = set(), set()
    for rec in records:
        if not isinstance(rec, dict):
            return f"the host renderer attestation has a malformed record: {rec!r}"
        event, ctx = rec.get("event"), rec.get("ctx_id")
        where = f"{event} of context {ctx}"
        if event not in EVENTS:
            return f"the host renderer attestation has an unknown event {event!r}"
        if not isinstance(ctx, int) or isinstance(ctx, bool) or ctx < 1:
            return f"{where} has no valid ctx_id"
        if event == "context-create":
            created.add(ctx)
            submitted.discard(ctx)
        elif ctx not in created:
            return f"{where} has no context-create for that context before it"
        elif ctx in submitted:
            return f"{where} repeats that context's first submission"
        else:
            submitted.add(ctx)
        fields = [rec.get(k) for k in ("gl_vendor", "gl_renderer", "gl_version")]
        if not all(isinstance(v, str) and v for v in fields):
            return f"{where} has no GL renderer strings"
        text = " ".join(fields)
        software = SOFTWARE.search(text)
        if software:
            return f"{where} ran on a software renderer, {software.group(0)}: {rec['gl_renderer']}"
        if not DECLARED.search(rec["gl_renderer"]):
            return f"{where} ran on {rec['gl_renderer']}, not {DECLARED_NAME}"
        renderers.add(rec["gl_renderer"])
    if not any(rec["event"] == "first-submit-3d" for rec in records):
        return "the host renderer attestation records no first-submit-3d: the guest submitted no commands"
    if serial:
        try:
            with open(serial, encoding="utf-8", errors="replace") as f:
                guest = [m.group(1).strip() for m in re.finditer(r"VIRGL_CAPSET_RENDERER=(.*)", f.read())]
        except OSError as e:
            return f"unreadable serial log: {e}"
        for name in guest:
            if not name or not all(name in r for r in renderers):
                return f"the guest's capset renderer {name!r} does not match the host's {sorted(renderers)}"
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("attestation")
    parser.add_argument("--serial")
    args = parser.parse_args()
    reason = judge(args.attestation, args.serial)
    if reason:
        print(f"host renderer: REFUSED: {reason}")
        return 1
    with open(args.attestation, encoding="utf-8") as f:
        doc = json.load(f)
    names = sorted({r["gl_renderer"] for r in doc["records"]})
    print(f"host renderer: accepted: {'; '.join(names)} ({len(doc['records'])} records)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
