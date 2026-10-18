#!/usr/bin/env python3
"""Prune missing runner VMs from Fusion's library only while its app is closed."""
import os
from pathlib import Path
import re
import subprocess
import tempfile


GROUP = re.compile(r'^(vmlist|index)(\d+)\.(\w+(?:\.\w+)*)\s*=\s*"(.*)"\s*$')
COUNT = re.compile(r'^index\.count\s*=')


def fusion_running():
    # AppKit queries the application list without opening Fusion or asking
    # System Events for automation permission. Any query failure forbids edits.
    result = subprocess.run(
        ['/usr/bin/osascript', '-l', 'JavaScript', '-e',
         "ObjC.import('AppKit'); "
         "$.NSRunningApplication.runningApplicationsWithBundleIdentifier('com.vmware.fusion').count"],
        capture_output=True, text=True, check=True, timeout=10)
    return int(result.stdout.strip()) != 0


def prune_inventory(source, machines):
    """Keep other VMs and all unknown fields; renumber surviving groups."""
    lines = source.splitlines(keepends=True)
    groups = {}
    for line in lines:
        match = GROUP.match(line)
        if match:
            kind, number, field, value = match.groups()
            groups.setdefault((kind, int(number)), {})[field] = value

    dead = set()
    for group, fields in groups.items():
        value = fields.get('config' if group[0] == 'vmlist' else 'id', '')
        path = Path(value)
        try:
            relative = path.relative_to(machines)
        except ValueError:
            continue
        if (len(relative.parts) < 2 or not relative.parts[0].startswith('breenix-')
                or '..' in relative.parts or path.suffix != '.vmx'):
            continue
        try:
            path.stat()
        except FileNotFoundError:
            dead.add(group)

    if not dead:
        return source
    names = {}
    for kind, start in (('vmlist', 1), ('index', 0)):
        kept = sorted(group for group in groups if group[0] == kind and group not in dead)
        names.update((group, f'{kind}{number}') for number, group in enumerate(kept, start))
    count = sum(group[0] == 'index' for group in names)
    output = []
    for line in lines:
        match = GROUP.match(line)
        if match:
            group = (match[1], int(match[2]))
            if group in dead:
                continue
            line = re.sub(r'^(?:vmlist|index)\d+', names[group], line, count=1)
        if COUNT.match(line):
            continue
        output.append(line)
    # A missing trailing newline must not concatenate index.count onto a field.
    body = ''.join(output).rstrip('\n')
    return (body + '\n' if body else '') + f'index.count = "{count}"\n'


def prune_file(inventory, machines):
    if fusion_running() or not inventory.exists():
        return False
    original = inventory.read_text()
    pruned = prune_inventory(original, machines)
    if pruned == original:
        return False
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode='w', dir=inventory.parent, delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(pruned)
            os.fchmod(stream.fileno(), inventory.stat().st_mode & 0o777)
        # Recheck immediately before the atomic replacement: Fusion might have
        # opened, or another writer might have changed the inventory meanwhile.
        if fusion_running() or inventory.read_text() != original:
            return False
        temporary.replace(inventory)
        return True
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    home = Path.home()
    try:
        changed = prune_file(home / 'Library/Application Support/VMware Fusion/vmInventory',
                             home / 'Virtual Machines.localized')
        if changed:
            print('Pruned missing Breenix boot VMs from VMware library')
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        raise SystemExit(f'VMware inventory left unchanged: {error}')


if __name__ == '__main__':
    main()
