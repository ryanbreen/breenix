#!/usr/bin/env python3
"""Report suite memory ownership changes from retained boot directories as TSV."""
import argparse
from collections import Counter
import csv
import json
from pathlib import Path
import re
import sys

MEMORY = re.compile(r'^SUITE (\S+) MEMORY (?:(before|after) |(reclaimed)=)(.*)$')
FIELDS = re.compile(r'([a-zA-Z_][a-zA-Z_0-9]*)=(-?\d+)')
NON_OWNERS = {'free', 'heap_free_kb', 'settle_ms'}


def fields(text):
    return {name: int(value) for name, value in FIELDS.findall(text)}


def parse_breengel_tables(serial):
    records = {}
    pattern = r'BREENGEL TABLES (before|finale|retired|after) runner_pte_kb=(\d+) processes=(\d+) all_pte_kb=(\d+) top_pte=([^ ]+)'
    for line in serial.splitlines():
        if not line.startswith('BREENGEL TABLES '):
            continue
        match = re.fullmatch(pattern, line)
        if not match or match[1] in records:
            raise ValueError('invalid BreenGL table census')
        entries = []
        for item in match[5].split(','):
            entry = re.fullmatch(r'(\d+):([^,:\s]+):(\d+)', item)
            if not entry:
                raise ValueError('invalid BreenGL table owner')
            entries.append((int(entry[1]), entry[2], int(entry[3])))
        if (int(match[3]) < 1 or len(entries) != min(3, int(match[3]))
                or len({entry[0] for entry in entries}) != len(entries)
                or any(entries[i][2] < entries[i+1][2] for i in range(len(entries) - 1))
                or int(match[4]) < sum(entry[2] for entry in entries)):
            raise ValueError('invalid BreenGL table ranking')
        records[match[1]] = dict(runner_pte_kb=int(match[2]), processes=int(match[3]), all_pte_kb=int(match[4]), top_pte=entries)
    if not {'before', 'after'} <= records.keys():
        raise ValueError('incomplete BreenGL table census')
    return records


def optional_evidence(boot, serial):
    births = ''
    census_info = ''
    artifact = boot / 'gt710-final.rle'
    if artifact.exists():
        records = re.findall(r'^GT710_BREENGL_ALLOCATIONS (.+)$', artifact.read_text(errors='replace'), re.M)
        if len(records) > 1:
            raise ValueError(f'{artifact}: multiple allocation censuses')
        if records:
            census = json.loads(records[0])
            if len(census['entries']) != census['count']:
                raise ValueError(f'{artifact}: incomplete allocation census')
            census_info = json.dumps({k: census[k] for k in ('phase', 'count', 'dropped')}, separators=(',', ':'))
            counts = Counter(str(entry[3]) for entry in census['entries'])
            births = json.dumps(dict(sorted(counts.items(), key=lambda item: int(item[0]))), separators=(',', ':'))
    table = ''
    if re.search(r'^BREENGEL TABLES ', serial, re.M):
        phases = parse_breengel_tables(serial)
        table = json.dumps({**{k: phases['after'][k] - phases['before'][k]
                               for k in ('runner_pte_kb', 'all_pte_kb')},
                            'top_before': phases['before']['top_pte'],
                            'top_after': phases['after']['top_pte']}, separators=(',', ':'))
    return births, census_info, table


def read_boot(boot):
    serial = (boot / 'serial.txt').read_text(errors='replace')
    suites = {}
    for line in serial.splitlines():
        match = MEMORY.match(line)
        if not match:
            continue
        suite, phase, reclaimed, payload = match.groups()
        phase = phase or reclaimed
        record = suites.setdefault(suite, {})
        if phase in record:
            raise ValueError(f'{boot}: duplicate {suite} MEMORY {phase}')
        record[phase] = payload
    births, census_info, table = optional_evidence(boot, serial)
    rows = []
    for suite, record in suites.items():
        if not {'before', 'after', 'reclaimed'} <= record.keys():
            continue
        before, after = fields(record['before']), fields(record['after'])
        owners = (before.keys() | after.keys()) - NON_OWNERS
        if not owners <= before.keys() or not owners <= after.keys():
            raise ValueError(f'{boot}: mismatched {suite} owner fields')
        verdict = re.search(r'\breclaimed=(PASS|FAIL)\b', 'reclaimed=' + record['reclaimed'])
        if not verdict:
            raise ValueError(f'{boot}: missing {suite} memory verdict')
        row = {'boot': boot.name, 'suite': suite, 'retained': before['free'] - after['free'],
               'reclaimed': verdict.group(1), 'allocation_births': births if suite == 'breengel' else '',
               'allocation_census': census_info if suite == 'breengel' else '',
               'runner_tables_delta': table if suite == 'breengel' else ''}
        row.update({owner: after[owner] - before[owner] for owner in owners})
        rows.append(row)
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('boot', nargs='+', type=Path, help='boot or evidence directories containing serial.txt')
    args = parser.parse_args()
    try:
        rows = [row for boot in args.boot for row in read_boot(boot)]
        owners = sorted(set().union(*(row.keys() for row in rows)) -
                        {'boot', 'suite', 'retained', 'reclaimed', 'allocation_births', 'allocation_census', 'runner_tables_delta'})
        writer = csv.DictWriter(sys.stdout, fieldnames=['boot', 'suite', 'retained', 'reclaimed', *owners,
                                                        'allocation_births', 'allocation_census', 'runner_tables_delta'], delimiter='\t', lineterminator='\n')
        writer.writeheader()
        writer.writerows(rows)
    except (OSError, ValueError, KeyError, IndexError, TypeError) as error:
        parser.exit(1, f'{error}\n')


if __name__ == '__main__':
    main()
