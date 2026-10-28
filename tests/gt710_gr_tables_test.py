#!/usr/bin/env python3
"""Independently compare the GK208 GR data with its pinned MIT C sources.

No generator import: tokenize only C array initializers, then read Rust literals.
Digest wire format: LE u32 pack length, then for each ordered pack member its
class ID, record length, and addr/count/pitch/data u32 records. C terminators
are omitted; count means stored records, not expanded register writes.
"""
import hashlib
import json
from pathlib import Path
import re
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / 'vendor/kepler/nouveau/gr'
TABLES = ROOT / 'kernel/src/gpu/gk208/gr/tables.rs'
COMMIT = 'e8f897f4afef0031fe618a8e94127a0934896aba'
UPSTREAM = 'drivers/gpu/drm/nouveau/nvkm/engine/gr/'
KINDS = ('mmio', 'icmd', 'mthd', 'hub', 'gpc_0', 'gpc_1', 'tpc', 'ppc', 'zcull')


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def without_comments(text):
    return re.sub(r'/\*.*?\*/|//[^\n]*', '', text, flags=re.S)


def c_arrays():
    """Use token boundaries/braces, independently of the emitter's line parser."""
    arrays, texts = {}, {}
    for path in sorted(VENDOR.glob('*.c')):
        text = texts[path.name] = path.read_text()
        tokens = re.findall(r'0x[0-9a-fA-F]+|[0-9]+|[A-Za-z_]\w*|[^\s]', without_comments(text))
        for i in range(len(tokens) - 8):
            if tokens[i:i+3] != ['const', 'struct', 'gf100_gr_init'] and tokens[i:i+3] != ['const', 'struct', 'gf100_gr_pack']:
                continue
            typ, name = tokens[i+2], tokens[i+3]
            if tokens[i+4:i+8] != ['[', ']', '=', '{']:
                continue
            j, rows = i + 8, []
            while tokens[j] != '}':
                require(tokens[j] == '{', f'{name}: expected entry')
                j += 1
                row = []
                while tokens[j] != '}':
                    if row:
                        require(tokens[j] == ',', f'{name}: missing comma')
                        j += 1
                    row.append(tokens[j])
                    j += 1
                rows.append(row)
                j += 1
                if tokens[j] == ',':
                    j += 1
            require(rows and rows.pop() == [], f'{name}: missing terminator')
            require(all(rows), f'{name}: early terminator')
            require(name not in arrays, f'duplicate {name}')
            if typ == 'gf100_gr_init':
                require(all(len(r) == 4 for r in rows), f'{name}: init layout')
                rows = [tuple(int(x, 0) for x in r) for r in rows]
                require(all(r[1] > 0 and r[2] > 0 and all(0 <= v <= 0xffffffff for v in r) for r in rows), f'{name}: invalid record')
            else:
                require(all(len(r) in (1, 2) for r in rows), f'{name}: pack layout')
                rows = [(r[0], int(r[1], 0) if len(r) == 2 else 0) for r in rows]
            arrays[name] = (path.name, typ, rows)
    return arrays, texts


def reference():
    arrays, texts = c_arrays()
    require(texts, 'missing vendored GR sources')
    roots = {}
    for kind in KINDS:
        file = 'gk208.c' if kind == 'mmio' else 'ctxgk208.c'
        field = re.search(r'\.' + kind + r'\s*=\s*(\w+)\s*,', without_comments(texts[file]))
        require(field is not None, f'missing GK208 {kind} binding')
        roots[kind] = field[1]
    used, lists = set(), []
    for kind, root in roots.items():
        file, typ, members = arrays[root]
        require(typ == 'gf100_gr_pack', root)
        used.add(file)
        words = [len(members)]
        count = 0
        for name, cls in members:
            source, typ, rows = arrays[name]
            require(typ == 'gf100_gr_init', name)
            used.add(source)
            words.extend((cls, len(rows)))
            count += len(rows)
            for row in rows:
                words.extend(row)
        digest = hashlib.sha256(b''.join(v.to_bytes(4, 'little') for v in words)).hexdigest()
        lists.append(dict(name=kind, count=count, sha256=digest))
    require(used == set(texts), f'source closure mismatch {used ^ set(texts)}')
    return arrays, texts, roots, lists


def rust_data(path):
    text = path.read_text()
    clean = without_comments(text)
    inits, packs = {}, {}
    for name, typ, length, body in re.findall(r'static\s+(\w+):\s*\[(Init|Pack);\s*(\d+)\]\s*=\s*\[(.*?)\];', clean, re.S):
        require(name not in inits and name not in packs, 'duplicate Rust array')
        if typ == 'Init':
            pattern = r'Init\s*\{\s*addr:\s*(\w+),\s*count:\s*(\w+),\s*pitch:\s*(\w+),\s*data:\s*(\w+)\s*\},'
            rows = [tuple(int(v, 0) for v in row) for row in re.findall(pattern, body)]
            inits[name] = rows
        else:
            pattern = r'Pack\s*\{\s*class_id:\s*(\w+),\s*init:\s*&(\w+)\s*\},'
            rows = [(n, int(c, 0)) for c, n in re.findall(pattern, body)]
            packs[name] = rows
        require(len(rows) == int(length), f'{name}: Rust length')
        require(not re.sub(pattern, '', body).strip(), f'{name}: unparsed Rust')
    pattern = r'InitList\s*\{\s*name:\s*"(\w+)",\s*packs:\s*&(\w+),\s*count:\s*(\d+),\s*sha256:\s*"([0-9a-f]{64})"\s*\},'
    lists = [dict(name=k, root=r, count=int(c), sha256=h) for k, r, c, h in re.findall(pattern, clean)]
    return text, inits, packs, lists


def compare(path):
    arrays, texts, roots, expected = reference()
    text, inits, packs, lists = rust_data(path)
    require([x['name'] for x in lists] == list(KINDS), 'Rust list order')
    require(set(packs) == {v.upper() for v in roots.values()}, 'Rust pack closure')
    required_inits = set()
    for actual, want in zip(lists, expected):
        root = roots[want['name']]
        require(actual == dict(want, root=root.upper()), f'{root}: metadata mismatch')
        require(packs[root.upper()] == [(n.upper(), c) for n, c in arrays[root][2]], f'{root}: pack order/class mismatch')
        require(f'source: {arrays[root][0]}:{root}' in text, f'{root}: missing attribution')
        for name, cls in arrays[root][2]:
            required_inits.add(name.upper())
            require(inits[name.upper()] == arrays[name][2], f'{name}: entry mismatch')
            require(f'source: {arrays[name][0]}:{name}' in text, f'{name}: missing attribution')
    require(set(inits) == required_inits, 'Rust init closure')
    entries = json.loads((ROOT / 'vendor/kepler/sources.json').read_text())['files']
    entries = [e for e in entries if e['project'] == 'nouveau' and e['path'].startswith('gr/')]
    require({e['path'] for e in entries} == {'gr/' + f for f in texts} and len(entries) == len(texts), 'manifest closure')
    notices = (ROOT / 'THIRD_PARTY_NOTICES').read_text()
    for e in entries:
        file = e['path'][3:]
        source = texts[file]
        header = source[:source.index('*/') + 2]
        require(e['license'] == 'MIT' and e['license_line'] == 'Permission is hereby granted, free of charge, to any person obtaining a', f'{file}: MIT license line')
        require(e['license_line'] in header and 'Copyright' in header and 'THE SOFTWARE IS PROVIDED "AS IS"' in header, f'{file}: MIT header')
        require(e['notice'] == header and header in text[:text.index('pub struct')], f'{file}: full generated notice')
        require(e['commit'] == COMMIT and e['upstream_path'] == UPSTREAM + file and e['adapted'] is False, f'{file}: provenance')
        digest = hashlib.sha256((VENDOR / file).read_bytes()).hexdigest()
        require(e['upstream_sha256'] == digest == e['vendored_sha256'], f'{file}: SHA-256')
        require('vendor/kepler/nouveau/gr/' + file in notices, f'{file}: third-party notice')
    require('kernel/src/gpu/gk208/gr/tables.rs' in notices and 'tools/kepler/gr_tables.py' in notices, 'notice consumers')
    scorer = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
    require(scorer['GR_INIT_LISTS'] == expected, 'comparator literals differ')
    return expected


class GrTablesTest(unittest.TestCase):
    def test_sources_entries_packs_notices_and_comparator(self):
        lists = compare(TABLES)
        print('GR tables: ' + ', '.join(f"{e['name']}={e['count']}:{e['sha256']}" for e in lists))

    def test_entry_mutation_is_rejected(self):
        text = TABLES.read_text()
        pattern = r'(Init \{ addr: 0x[0-9a-f]+, count: \d+, pitch: 0x[0-9a-f]+, data: )0x([0-9a-f]+)'
        changed, n = re.subn(pattern, lambda m: m[1] + f'0x{int(m[2], 16) ^ 1:08x}', text, count=1)
        self.assertEqual(n, 1)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'tables.rs'
            path.write_text(changed)
            with self.assertRaisesRegex(AssertionError, 'entry mismatch'):
                compare(path)
        print('GR tables: temporary entry mutation rejected')

    def test_method_class_and_pack_order_mutations_are_rejected(self):
        text = TABLES.read_text()
        changed = text.replace('class_id: 0x0000a197', 'class_id: 0x0000a196', 1)
        self.assertNotEqual(changed, text)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'tables.rs'
            path.write_text(changed)
            with self.assertRaisesRegex(AssertionError, 'pack order/class mismatch'):
                compare(path)
            first, second = re.findall(r'    Pack \{[^\n]+', text)[:2]
            path.write_text(text.replace(first, '\x01', 1).replace(second, first, 1).replace('\x01', second, 1))
            with self.assertRaisesRegex(AssertionError, 'pack order/class mismatch'):
                compare(path)
        print('GR tables: method class and pack order mutations rejected')


if __name__ == '__main__':
    unittest.main()
