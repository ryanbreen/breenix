#!/usr/bin/env python3
"""Judge the host renderer attestation of a GL QEMU run (docs/gui/qemu-gl.md).

    scripts/check-host-renderer.py ATTESTATION [--serial LOG]

ATTESTATION is the host-renderer.json the GL QEMU wrote for the run (the
BREENIX_HOST_RENDERER_JSON file of scripts/qemu-gl/host-renderer-attestation.patch).
The run is refused, with the reason, when:

  - the file is missing or records no renderer context;
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


def judge(attestation, serial):
    try:
        with open(attestation, encoding="utf-8") as f:
            doc = json.load(f)
    except FileNotFoundError:
        return "no host renderer attestation: the guest created no VirGL context"
    except (OSError, ValueError) as e:
        return f"unreadable host renderer attestation: {e}"
    records = doc.get("records") or []
    if not records:
        return "the host renderer attestation records no renderer context"
    renderers = set()
    for rec in records:
        where = f"{rec.get('event')} of context {rec.get('ctx_id')}"
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
