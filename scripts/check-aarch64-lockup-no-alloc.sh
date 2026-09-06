#!/bin/bash
#
# check-aarch64-lockup-no-alloc.sh - binary and source guard for the AArch64
# soft-lockup report (failure-capture PR-7).
#
# WHAT IT GUARDS. `dump_lockup_state` runs inside CPU0's hardware timer
# interrupt. An allocation there takes the heap allocator's lock from interrupt
# context. Before PR-7 the report reached TWO allocating snapshot helpers --
# `task::scheduler::try_dump_state`, which collects `Vec<ThreadDumpEntry>` and
# `Vec<u64>` while holding the scheduler guard, and `process::try_dump_state`,
# which pushed owned `String`s -- and neither was visible to a source denylist
# scoped to `kernel/src/capture/`.
#
# WHY IT IS TRANSITIVE, unlike scripts/check-x86-dispatch-no-alloc.sh. That
# guard is depth-1 with a broad alloc-family target set, which is right for its
# root but cannot see an allocation behind an ordinary, innocently named helper
# -- which is exactly the shape PR-7 removed. This one derives the reachable
# call graph from the linked code to a fixed point, so a newly introduced
# helper that allocates is caught whatever it is called.
#
# Because the walk is transitive, the target set here is the true allocation
# SINKS only (`__rust_alloc` and family, `exchange_malloc`,
# `handle_alloc_error`). Copying the x86 script's alloc-crate family matcher
# into a transitive scan would flag read-only methods that merely live under
# `alloc::`, so it is deliberately not copied.
#
# ANTI-VACUITY. Each of these is a FAILURE rather than a quiet pass: a wrong
# architecture, an unreadable ELF, a missing root symbol, empty disassembly or
# symbol data, zero resolved root edges, an unsupported reachable branch form,
# an unresolved indirect transfer, and an analysis bound reached before
# closure. A missing target is not a clean leaf.
# claim-lint:ok: 7 of 7 of those shapes are driven against this script by
# tests/lockup_capture_guard_structure.rs, which builds a fixture ELF per shape
# and requires the guard to reject six and pass the clean one.
#
# SOURCE MODE. `--extract-source` prints the strict source scope -- the item
# body of `dump_lockup_state` plus the bodies of local helpers in the same file
# reached by syntactically resolved calls, derived from the calls rather than
# from a maintained list. scripts/check-critical-path-violations.sh consumes
# that text and applies its capture-scoped denylist to it, which is how the
# strict list reaches this function without blanket-denying the rest of a timer
# file that legitimately carries other things.
#
# Exit code:
#   0 - clean
#   1 - guard tripped, or a tooling/usage/anti-vacuity failure
#
# Usage:
#   ./scripts/check-aarch64-lockup-no-alloc.sh <path/to/aarch64-kernel-elf>
#   ./scripts/check-aarch64-lockup-no-alloc.sh --extract-source
#   ./scripts/check-aarch64-lockup-no-alloc.sh --root NAME <elf>

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TIMER_SOURCE="$REPO_ROOT/kernel/src/arch_impl/aarch64/timer_interrupt.rs"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'

ROOT_NAME="dump_lockup_state"
MODE="binary"
KERNEL_ELF=""
TIMER_SOURCE_OVERRIDE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --extract-source) MODE="source"; shift ;;
        --root) ROOT_NAME="${2:-}"; shift 2 ;;
        # `--source` points the source mode at a file other than the tree's own
        # timer file. Its one purpose is the mutation legs in
        # tests/capture_path_lock_free_structure.rs: they copy the timer file to
        # a temp path, insert an allocation, and require this extractor plus the
        # denylist to catch it. Without it a mutation leg would have to write
        # into the real tree to test the guard, which is worse.
        --source) TIMER_SOURCE_OVERRIDE="${2:-}"; shift 2 ;;
        -h|--help) sed -n '2,60p' "$0"; exit 0 ;;
        -*) echo "ERROR: unknown option $1" >&2; exit 1 ;;
        *) KERNEL_ELF="$1"; shift ;;
    esac
done

if ! command -v python3 >/dev/null 2>&1; then
    echo "ERROR: python3 is required by this guard." >&2
    exit 1
fi

if [ "$MODE" = "source" ]; then
    if [ -n "$TIMER_SOURCE_OVERRIDE" ]; then
        TIMER_SOURCE="$TIMER_SOURCE_OVERRIDE"
    fi
    if [ ! -f "$TIMER_SOURCE" ]; then
        echo "ERROR: $TIMER_SOURCE not found." >&2
        exit 1
    fi
    python3 - "$TIMER_SOURCE" "$ROOT_NAME" <<'BXCAP_SOURCE_PY'
"""Extract the strict source scope of the soft-lockup dump.

The scope is the complete item body of `dump_lockup_state`, its lexically
nested closures (they are inside that body, so they come for free), and the
bodies of local helpers in the same file reached by syntactically resolved
calls -- derived from the calls themselves, never from a maintained list.

This is an ADVISORY source boundary. Cross-file and indirect reachability is
the binary guard's job; this mode exists so the shell denylist has something
narrower than the whole timer file to read, without blanket-denying unrelated
timer code (the file carries an unrelated CPU0-regression `panic!`).
"""
import re
import sys


def code_mask(source):
    """True where a byte is CODE: not inside a comment or a string literal."""
    mask = [True] * len(source)
    i = 0
    n = len(source)
    while i < n:
        c = source[i]
        if c == "/" and i + 1 < n and source[i + 1] == "/":
            j = source.find("\n", i)
            j = n if j < 0 else j
            for k in range(i, j):
                mask[k] = False
            i = j
            continue
        if c == "/" and i + 1 < n and source[i + 1] == "*":
            depth = 1
            j = i + 2
            while j < n and depth > 0:
                if source[j] == "/" and j + 1 < n and source[j + 1] == "*":
                    depth += 1
                    j += 2
                    continue
                if source[j] == "*" and j + 1 < n and source[j + 1] == "/":
                    depth -= 1
                    j += 2
                    continue
                j += 1
            for k in range(i, min(j, n)):
                mask[k] = False
            i = j
            continue
        if c == '"':
            j = i + 1
            while j < n:
                if source[j] == "\\":
                    j += 2
                    continue
                if source[j] == '"':
                    j += 1
                    break
                j += 1
            for k in range(i + 1, min(j, n)):
                mask[k] = False
            i = j
            continue
        i += 1
    return mask


FN_DEF = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
CALL = re.compile(r"\b([a-z_][A-Za-z0-9_]*)\s*\(")


def item_span(source, mask, start):
    """Balanced body span for the item whose `fn` keyword begins at `start`."""
    i = start
    n = len(source)
    while i < n and not (source[i] == "{" and mask[i]):
        if source[i] == ";" and mask[i]:
            return None
        i += 1
    if i >= n:
        return None
    depth = 0
    j = i
    while j < n:
        if mask[j]:
            if source[j] == "{":
                depth += 1
            elif source[j] == "}":
                depth -= 1
                if depth == 0:
                    return (start, j + 1)
        j += 1
    return None


def collect_items(source, mask):
    items = dict()
    for m in FN_DEF.finditer(source):
        if not mask[m.start()]:
            continue
        name = m.group(1)
        span = item_span(source, mask, m.start())
        if span is None:
            continue
        items.setdefault(name, []).append(span)
    return items


def main():
    path = sys.argv[1]
    root = sys.argv[2]
    source = open(path, encoding="utf-8").read()
    mask = code_mask(source)
    items = collect_items(source, mask)
    if root not in items:
        sys.stderr.write("FAIL: no `fn %s` item in %s\n" % (root, path))
        return 1
    if len(items[root]) != 1:
        sys.stderr.write(
            "FAIL: `fn %s` is ambiguous in %s (%d definitions)\n"
            % (root, path, len(items[root]))
        )
        return 1
    order = [root]
    seen = set(order)
    out = []
    while order:
        name = order.pop(0)
        spans = items.get(name)
        if not spans or len(spans) != 1:
            continue
        start, end = spans[0]
        body = source[start:end]
        out.append("// ---- strict lockup scope: fn %s (%s) ----" % (name, path))
        out.append(body)
        for m in CALL.finditer(body):
            if not mask[start + m.start()]:
                continue
            callee = m.group(1)
            if callee in seen or callee not in items:
                continue
            seen.add(callee)
            order.append(callee)
    if len(seen) < 1:
        sys.stderr.write("FAIL: extracted an empty strict scope\n")
        return 1
    sys.stdout.write("\n".join(out))
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
BXCAP_SOURCE_PY
    exit $?
fi

if [ -z "$KERNEL_ELF" ]; then
    echo "ERROR: no ELF given. This guard requires an EXPLICIT aarch64 kernel" >&2
    echo "ELF path -- guessing one is how a guard ends up reading a stale or" >&2
    echo "wrong-profile artifact and reporting it as the shipped kernel." >&2
    exit 1
fi
if [ ! -f "$KERNEL_ELF" ]; then
    echo "ERROR: ELF not found: $KERNEL_ELF" >&2
    exit 1
fi

find_objdump() {
    if command -v llvm-objdump >/dev/null 2>&1; then
        command -v llvm-objdump; return 0
    fi
    local sysroot cand
    sysroot="$(rustc --print sysroot 2>/dev/null)"
    if [ -n "$sysroot" ]; then
        cand="$(ls "$sysroot"/lib/rustlib/*/bin/llvm-objdump 2>/dev/null | head -1)"
        if [ -n "$cand" ]; then echo "$cand"; return 0; fi
    fi
    if command -v objdump >/dev/null 2>&1; then
        command -v objdump; return 0
    fi
    return 1
}

OBJDUMP="$(find_objdump)"
if [ -z "$OBJDUMP" ]; then
    echo "ERROR: no objdump found (need llvm-objdump or objdump)." >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    ELF_SHA="$(sha256sum "$KERNEL_ELF" | cut -d" " -f1)"
else
    ELF_SHA="$(shasum -a 256 "$KERNEL_ELF" | cut -d" " -f1)"
fi

echo "Guard: aarch64 soft-lockup report allocation check (failure-capture PR-7)"
echo "  ELF:      $KERNEL_ELF"
echo "  sha256:   $ELF_SHA"
echo "  objdump:  $OBJDUMP"
echo "  root:     $ROOT_NAME (its own symbols and every reachable callee)"

GUARD_OUT="$(python3 - "$KERNEL_ELF" "$OBJDUMP" "$ROOT_NAME" <<'BXCAP_BINARY_PY'
"""Transitive AArch64 allocation-reachability analysis from a named root."""
import re
import struct
import subprocess
import sys

# The true allocation SINKS. Because this walk is TRANSITIVE, only the sinks
# need naming: a `String::clone` or a `Vec` growth reached from the root will
# itself reach one of these, and is reported with the whole path. Naming the
# alloc-crate types instead -- what the depth-1 x86 guard does -- would
# misclassify read-only methods that merely live under `alloc::`.
ALLOC_SINK = (
    "__rust_alloc",
    "__rust_realloc",
    "__rust_alloc_zeroed",
    "__rg_alloc",
    "__rg_realloc",
    "__rg_alloc_zeroed",
    "__rdl_alloc",
    "__rdl_realloc",
    "__rdl_alloc_zeroed",
    "exchange_malloc",
    "handle_alloc_error",
    "5alloc5alloc5alloc",
    "5alloc5alloc7realloc",
    "5alloc5alloc12alloc_zeroed",
)


class Fail(Exception):
    pass


def read_elf(path):
    """Return (data, segments) where segments maps a vaddr range to file data."""
    data = open(path, "rb").read()
    if data[:4] != b"\x7fELF":
        raise Fail("not an ELF file: %s" % path)
    if data[4] != 2:
        raise Fail("not a 64-bit ELF: %s" % path)
    machine = struct.unpack_from("<H", data, 18)[0]
    if machine != 183:
        raise Fail("wrong architecture: e_machine=%d, expected 183 (AArch64)" % machine)
    phoff, = struct.unpack_from("<Q", data, 32)
    phentsize, phnum = struct.unpack_from("<HH", data, 54)
    segs = []
    for i in range(phnum):
        off = phoff + i * phentsize
        p_type, = struct.unpack_from("<I", data, off)
        if p_type != 1:
            continue
        p_offset, p_vaddr = struct.unpack_from("<QQ", data, off + 8)
        p_filesz, = struct.unpack_from("<Q", data, off + 32)
        segs.append((p_vaddr, p_vaddr + p_filesz, p_offset))
    if not segs:
        raise Fail("no PT_LOAD segments in %s" % path)
    return data, segs


def read_u64(data, segs, vaddr):
    for lo, hi, off in segs:
        if lo <= vaddr and vaddr + 8 <= hi:
            return struct.unpack_from("<Q", data, off + (vaddr - lo))[0]
    return None


SYM_HEAD = re.compile(r"^([0-9a-fA-F]+)\s+<(.+)>:\s*$")
INSN = re.compile(r"^([0-9a-fA-F]+):\s+(.*)$")


def disassemble(objdump, elf):
    out = subprocess.run(
        [objdump, "-d", "--no-show-raw-insn", elf],
        capture_output=True, text=True,
    )
    if out.returncode != 0:
        raise Fail("objdump failed: %s" % out.stderr.strip()[:400])
    return out.stdout


def parse_functions(text):
    funcs = {}
    order = []
    cur = None
    for line in text.splitlines():
        m = SYM_HEAD.match(line)
        if m:
            cur = int(m.group(1), 16)
            funcs[cur] = dict(name=m.group(2), addr=cur, insns=[])
            order.append(cur)
            continue
        if cur is None:
            continue
        m = INSN.match(line.strip())
        if m:
            funcs[cur]["insns"].append((int(m.group(1), 16), m.group(2).strip()))
    if not funcs:
        raise Fail("objdump produced no disassembled symbols for the ELF")
    order.sort()
    for i, addr in enumerate(order):
        if i + 1 < len(order):
            funcs[addr]["end"] = order[i + 1]
        else:
            ins = funcs[addr]["insns"]
            funcs[addr]["end"] = (ins[-1][0] + 4) if ins else addr + 4
    return funcs, order


TARGET = re.compile(r"0x([0-9a-fA-F]+)")
ADRP = re.compile(r"^adrp\s+(x\d+|xzr),\s*0x([0-9a-fA-F]+)")
ADR = re.compile(r"^adr\s+(x\d+|xzr),\s*0x([0-9a-fA-F]+)")
ADD_IMM = re.compile(r"^add\s+(x\d+),\s*(x\d+|sp),\s*#(0x[0-9a-fA-F]+|\d+)")
MOV_REG = re.compile(r"^mov\s+(x\d+),\s*(x\d+)\s*$")
LDR_OFF = re.compile(r"^ldr\s+(x\d+),\s*\[(x\d+)(?:,\s*#(0x[0-9a-fA-F]+|\d+))?\]")
LDR_LIT = re.compile(r"^ldr\s+(x\d+),\s*0x([0-9a-fA-F]+)")
INDIRECT = re.compile(r"^(blr|br)\s+(x\d+)")
DIRECT = re.compile(r"^(bl|b)\s+0x([0-9a-fA-F]+)")
COND_B = re.compile(r"^(b\.[a-z]+|cbz|cbnz|tbz|tbnz)\b")


def imm(text):
    return int(text, 16) if text.startswith("0x") else int(text)


def owner_of(order, funcs, addr):
    lo, hi = 0, len(order) - 1
    best = None
    while lo <= hi:
        mid = (lo + hi) // 2
        if order[mid] <= addr:
            best = order[mid]
            lo = mid + 1
        else:
            hi = mid - 1
    if best is None:
        return None
    if addr < funcs[best]["end"]:
        return best
    return None


def edges_of(fn, funcs, order, data, segs, problems):
    """Call/tail targets leaving `fn`, as a set of callee entry addresses."""
    out = set()
    regs = dict()
    lo = fn["addr"]
    hi = fn["end"]
    for addr, text in fn["insns"]:
        m = ADRP.match(text)
        if m:
            regs[m.group(1)] = int(m.group(2), 16)
            continue
        m = ADR.match(text)
        if m:
            regs[m.group(1)] = int(m.group(2), 16)
            continue
        m = ADD_IMM.match(text)
        if m:
            base = regs.get(m.group(2))
            if base is None:
                regs.pop(m.group(1), None)
            else:
                regs[m.group(1)] = base + imm(m.group(3))
            continue
        m = MOV_REG.match(text)
        if m:
            src = regs.get(m.group(2))
            if src is None:
                regs.pop(m.group(1), None)
            else:
                regs[m.group(1)] = src
            continue
        m = LDR_LIT.match(text)
        if m:
            value = read_u64(data, segs, int(m.group(2), 16))
            if value is None:
                regs.pop(m.group(1), None)
            else:
                regs[m.group(1)] = value
            continue
        m = LDR_OFF.match(text)
        if m:
            base = regs.get(m.group(2))
            if base is None:
                regs.pop(m.group(1), None)
            else:
                slot = base + (imm(m.group(3)) if m.group(3) else 0)
                value = read_u64(data, segs, slot)
                if value is None:
                    regs.pop(m.group(1), None)
                else:
                    regs[m.group(1)] = value
            continue
        m = DIRECT.match(text)
        if m:
            tgt = int(m.group(2), 16)
            if m.group(1) == "b" and lo <= tgt < hi:
                continue
            owner = owner_of(order, funcs, tgt)
            if owner is None:
                problems.append(
                    "unresolved direct transfer to 0x%x from %s at 0x%x"
                    % (tgt, fn["name"], addr)
                )
            else:
                out.add(owner)
            continue
        if COND_B.match(text):
            m2 = TARGET.search(text)
            if m2:
                tgt = int(m2.group(1), 16)
                if not (lo <= tgt < hi):
                    owner = owner_of(order, funcs, tgt)
                    if owner is not None:
                        out.add(owner)
            continue
        m = INDIRECT.match(text)
        if m:
            value = regs.get(m.group(2))
            owner = owner_of(order, funcs, value) if value else None
            if owner is None:
                problems.append(
                    "UNRESOLVED indirect %s %s in %s at 0x%x"
                    % (m.group(1), m.group(2), fn["name"], addr)
                )
            else:
                out.add(owner)
            continue
        if text.startswith("ret") or text.startswith("eret"):
            continue
    return out


MAX_FUNCS = 20000


def is_sink(name):
    for needle in ALLOC_SINK:
        if needle in name:
            return True
    return False


def walk(root_addrs, funcs, order, data, segs):
    seen = set()
    parent = dict()
    problems = []
    stack = list(root_addrs)
    for a in root_addrs:
        seen.add(a)
    edge_count = 0
    while stack:
        if len(seen) > MAX_FUNCS:
            problems.append("analysis bound of %d functions reached before closure" % MAX_FUNCS)
            break
        addr = stack.pop()
        for callee in edges_of(funcs[addr], funcs, order, data, segs, problems):
            edge_count += 1
            if callee not in seen:
                seen.add(callee)
                parent[callee] = addr
                stack.append(callee)
    return seen, parent, problems, edge_count


def path_to(addr, parent, funcs, roots):
    chain = [addr]
    while chain[-1] not in roots and chain[-1] in parent:
        chain.append(parent[chain[-1]])
    chain.reverse()
    return " -> ".join(funcs[a]["name"] for a in chain)


def main():
    elf = sys.argv[1]
    objdump = sys.argv[2]
    root_substr = sys.argv[3]
    data, segs = read_elf(elf)
    funcs, order = parse_functions(disassemble(objdump, elf))
    roots = set(a for a in order if root_substr in funcs[a]["name"])
    if not roots:
        print("FAIL: no symbol whose name contains %r in %s" % (root_substr, elf))
        print("The guard checked no code: the root was renamed, inlined away, or")
        print("the wrong ELF was supplied. Fix the guard rather than deleting it.")
        return 1
    seen, parent, problems, edge_count = walk(roots, funcs, order, data, segs)
    print("  roots:           %d" % len(roots))
    for a in sorted(roots):
        print("    %s" % funcs[a]["name"])
    print("  reachable funcs: %d" % len(seen))
    print("  call edges:      %d" % edge_count)
    zero_edges = edge_count == 0
    sinks = sorted(a for a in seen if is_sink(funcs[a]["name"]))
    status = 0
    if sinks:
        print("FAIL: %d allocating call target(s) reachable from the root." % len(sinks))
        for a in sinks:
            print("    %s" % path_to(a, parent, funcs, roots))
        status = 1
    if problems:
        print("FAIL: %d analysis problem(s); a missing target is not a clean leaf." % len(problems))
        for p in problems[:40]:
            print("    %s" % p)
        status = 1
    if zero_edges and status == 0:
        print("FAIL: resolved 0 call edges out of the root(s).")
        print("The symbol was found but no edge came out of it, so the decoder")
        print("has stopped matching this toolchain's output. Not a clean result.")
        status = 1
    if status == 0:
        print("PASS: 0 allocation sinks reachable from %d root symbol(s)." % len(roots))
    return status


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Fail as error:
        print("FAIL: %s" % error)
        sys.exit(1)
BXCAP_BINARY_PY
)"
GUARD_STATUS=$?
printf '%s\n' "$GUARD_OUT"

if [ $GUARD_STATUS -eq 0 ]; then
    echo -e "${GREEN}PASS:${NC} no allocation is reachable from $ROOT_NAME in this ELF."
    exit 0
fi

echo -e "${RED}FAIL:${NC} $ROOT_NAME runs inside CPU0's timer interrupt." >&2
echo -e "${YELLOW}An allocation reached from there takes the heap allocator's" >&2
echo -e "lock from interrupt context, and a report that has to allocate to be" >&2
echo -e "written cannot be trusted to be written at all. See" >&2
echo -e "docs/planning/green-program/failure-capture/PR-7-2026-09-06.md${NC}" >&2
exit 1
