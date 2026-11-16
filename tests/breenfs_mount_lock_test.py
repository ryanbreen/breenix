#!/usr/bin/env python3
"""C1 (E24 coordinator ledger 309, issue #3033): no spin lock in breenfs is
held across a block read.

ext2's #728 showed the failure: a spin-lock holder sleeps on block I/O while
contenders spin with preemption disabled (x86 syscalls also run with
interrupts masked), so the holder's completion can be denied every CPU. That
livelock needs the kernel scheduler, so this host check is a structural
census, as ledger 309 permits, in the style of tests/ext2_lock_structure.rs.

Census: every spin::Mutex or spin::RwLock declared in kernel/src/fs/breenfs/
(a struct field or a static) is found, each of its acquisitions is located,
and the guard's live range is derived from the source (a let-bound guard
lives to its drop() or the end of its block; a temporary lives to the end of
its statement, including a for/if/match body). No live range may contain a
call that reaches the buffer cache or a device read. Spin locks declared
outside fs/breenfs (for example VNode fields) and guards a function returns
to its caller are not followed by this census.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
BREENFS = ROOT / 'kernel/src/fs/breenfs'

# Calls that read nodes or data through the buffer cache or a device, or that
# wait. Files methods read the node store; the MountState helpers below read
# inodes through it.
IO_CALLS = (
    'files.', 'read_verified(', 'read_block(', 'read_blocks(', 'self.get(',
    'recover::recover(', 'recover(', '.inode(', '.attrs(', '.regular(',
    '.directory(', 'drain_dirty(', 'drain_pages(', 'prepare_files(',
    'write_cached(', 'open_locked(', 'apply_touches(', 'commit_generation(',
    'sync_generation(', 'commit_due(', 'sync_range(', '.wait(',
)


def strip_comments(source):
    """Blank comments and string/char literals, keeping offsets."""
    out = list(source)
    i, n = 0, len(source)
    while i < n:
        if source.startswith('//', i):
            j = source.find('\n', i)
            j = n if j < 0 else j
            out[i:j] = ' ' * (j - i)
            i = j
        elif source.startswith('/*', i):
            j = source.find('*/', i + 2)
            j = n if j < 0 else j + 2
            out[i:j] = [c if c == '\n' else ' ' for c in source[i:j]]
            i = j
        elif source[i] == '"':
            j = i + 1
            while j < n and source[j] != '"':
                j += 2 if source[j] == '\\' else 1
            out[i + 1:j] = ' ' * (j - i - 1)
            i = j + 1
        elif source[i] == "'" and re.match(r"'(\\.|[^\\'])'", source[i:i + 4]):
            j = source.index("'", i + 1 + (source[i + 1] == '\\'))
            out[i + 1:j] = ' ' * (j - i - 1)
            i = j + 1
        else:
            i += 1
    return ''.join(out)


def spin_locks(source):
    """Names of fields and statics whose type is a spin lock."""
    imported = set()
    for group in re.findall(r'use\s+spin::\{([^}]*)\}', source):
        imported |= {name.strip() for name in group.split(',')}
    imported |= set(re.findall(r'use\s+spin::(\w+)\s*;', source))
    kinds = {name for name in imported if name in ('Mutex', 'RwLock')}
    spellings = [rf'spin::{kind}' for kind in ('Mutex', 'RwLock')] + sorted(kinds)
    names = set()
    for spelling in spellings:
        names |= set(re.findall(rf'\b(\w+)\s*:\s*{re.escape(spelling)}\s*<', source))
    return names


def statement_end(code, start):
    """End of the statement containing `start` (a ';' at depth 0, or the
    close of the enclosing block)."""
    depth = 0
    i = start
    while i < len(code):
        c = code[i]
        if c in '([{':
            depth += 1
        elif c in ')]}':
            if depth == 0:
                return i
            depth -= 1
            if depth == 0 and c == '}':
                # A block-bodied statement (for/if/match/while) ends at its
                # brace unless an else/chain continues it.
                rest = code[i + 1:].lstrip()
                if not rest.startswith(('else', '.', '?', ';', ')')):
                    return i + 1
        elif c == ';' and depth == 0:
            return i
        i += 1
    return i


def block_end(code, start):
    depth = 0
    for i in range(start, len(code)):
        if code[i] in '([{':
            depth += 1
        elif code[i] in ')]}':
            if depth == 0:
                return i
            depth -= 1
    return len(code)


def guard_ranges(code, names):
    """(name, line, live-range text) for each acquisition of a named lock."""
    ranges = []
    pattern = re.compile(r'\b(' + '|'.join(map(re.escape, sorted(names))) +
                         r')\s*\.\s*(lock|read|write|upgradeable_read)\s*\(\s*\)')
    for match in pattern.finditer(code) if names else ():
        start = match.end()
        line = code.count('\n', 0, match.start()) + 1
        head = max(code.rfind(c, 0, match.start()) for c in ';{}') + 1
        end = statement_end(code, head)
        binding = re.fullmatch(r'\s*let\s+(?:mut\s+)?(\w+)\s*(?::[^=]*)?=\s*[\w.&*\s]*',
                               code[head:match.start()])
        if binding and re.fullmatch(r'[\s?]*', code[start:end]):
            # A let-bound guard lives to its drop() or the end of its block.
            guard = binding.group(1)
            close = block_end(code, start)
            dropped = re.search(rf'\bdrop\(\s*{guard}\s*\)', code[start:close])
            text = code[start:start + dropped.start() if dropped else close]
        else:
            # A temporary lives to the end of its whole statement.
            text = code[head:end]
        ranges.append((match.group(1), line, text))
    return ranges


def violations(source, path='<source>'):
    code = strip_comments(source)
    found = []
    for name, line, text in guard_ranges(code, spin_locks(code)):
        text = re.sub(r'\s+', '', text)
        calls = sorted({call for call in IO_CALLS if call.replace(' ', '') in text})
        if calls:
            found.append(f'{path}:{line}: spin lock `{name}` held across {", ".join(calls)}')
    return found


def census():
    found, acquisitions = [], 0
    for path in sorted(BREENFS.glob('*.rs')):
        source = path.read_text()
        code = strip_comments(source)
        acquisitions += len(guard_ranges(code, spin_locks(code)))
        found += violations(source, path.relative_to(ROOT))
    return found, acquisitions


class MountLockCensus(unittest.TestCase):
    def test_no_spin_guard_spans_a_block_read(self):
        found, acquisitions = census()
        # WRITABLE_OWNERS (vfs.rs) and the decoded-node index (node_cache.rs)
        # are spin locks with short sections; fewer than three acquisitions
        # means the scanner stopped seeing the tree.
        self.assertGreaterEqual(acquisitions, 3, 'census found no spin-lock acquisitions')
        self.assertEqual(found, [], '\n'.join(found))

    def test_scanner_reddens_on_a_guard_held_across_a_read(self):
        held = '''use spin::{Mutex, MutexGuard};
struct Mount { state: Mutex<State> }
impl Mount {
    fn read(&self) { let mut state = self.state.lock(); state.files.read(1); }
    fn tail(&self) -> u64 { self.state.lock().inode(2) }
    fn each(&self) { for n in self.state.lock().nodes.iter() { n.files.read(3); } }
}'''
        self.assertEqual(len(violations(held)), 3, violations(held))

    def test_scanner_accepts_sections_that_end_before_io(self):
        scoped = '''use spin::Mutex;
static OWNERS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
struct Store { decoded: Mutex<Vec<u64>> }
impl Store {
    fn get(&self) {
        { let cache = self.decoded.lock(); cache.len(); }
        let node = self.get(1);
        let guard = OWNERS.lock();
        drop(guard);
        files.read(2);
        let n = self.decoded.lock().len();
    }
}'''
        self.assertEqual(violations(scoped), [])


if __name__ == '__main__':
    unittest.main()
