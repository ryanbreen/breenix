#!/usr/bin/env python3
"""Generate/check the GT 710 judged-case set from a successful ARM BreenGL capture."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import runpy

ROOT = Path(__file__).resolve().parents[1]


def inventory(directory):
    directory = Path(directory)
    boot = json.loads((directory / 'run.json').read_text())
    commit = (directory / 'commit').read_text().strip()
    if (boot.get('arch') != 'aarch64' or boot.get('platform') != 'qemu'
            or boot.get('mode') != 'suite' or boot.get('suite') != 'breengel'
            or not re.fullmatch(r'\d{8}T\d{6}Z-qemu-\d+', boot.get('id', ''))
            or not re.fullmatch('[a-f0-9]{40}', commit)
            or (directory / 'dirty').read_text().strip() != '0'
            or (directory / 'exit-status').read_text().strip() != '0'):
        raise ValueError('inventory requires a completed clean ARM QEMU BreenGL capture')
    manifest = json.loads((ROOT / 'docs/suites/breengel.json').read_text())
    serial = directory / 'serial.txt'
    valid, why = runpy.run_path(str(ROOT / 'scripts/suite-verdict.py'))['verdict'](manifest, serial, [serial])
    if not valid:
        raise ValueError('inventory requires ARM 401/401: ' + why)
    body = serial.read_bytes()
    cases = re.findall(rb'^SUITE breengel CASE ([a-z0-9/-]+) PASS ms=\d+ sha256=[a-f0-9]{64}\r?$', body, re.M)
    if not cases:
        raise ValueError('ARM capture has no judged pictures')
    return dict(schema=1, suite='breengel', source=dict(boot=boot['id'], commit=commit,
                serialSha256=hashlib.sha256(body).hexdigest()), cases=[name.decode() for name in cases])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('capture', type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--output', type=Path)
    mode.add_argument('--check', type=Path)
    args = parser.parse_args()
    try:
        row = inventory(args.capture)
        if args.check:
            expected = json.loads(args.check.read_text())
            if expected.get('schema') != 1 or expected.get('suite') != 'breengel' or expected.get('cases') != row['cases']:
                raise ValueError('ARM judged-case set differs from the retained inventory')
            print(f"PASS: ARM judged set matches ({len(row['cases'])} cases)")
        else:
            text = json.dumps(row, indent=2) + '\n'
            if args.output:
                args.output.write_text(text)
            else:
                print(text, end='')
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f'inventory: {error}\n')


if __name__ == '__main__':
    main()
