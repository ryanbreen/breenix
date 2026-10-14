#!/bin/bash
# Build every configured BusyBox applet against Breenix libc, recording per-applet
# compile/link results in target/libc-busybox/<arch>/applets.json. Missing applets
# fail the build. Patches in vendor/busybox/patches/series apply after extraction;
# every patch is part of the build digest. Outputs are libc-busybox.elf and
# libc-busybox.links, installed in /bin and /usr/local/libc/bin as hard links.
# Usage: scripts/build-libc-busybox.sh --arch aarch64|x86_64
set -euo pipefail
export LC_ALL=C

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCHIVE="$ROOT/vendor/busybox/busybox-1.37.0.tar.bz2"
SOURCE_SHA256="3311dff32e746499f4df0d5df04d7eb396382d7e108bb9250e7b519b837043a4"
FRAGMENT="$ROOT/vendor/busybox/breenix-libc.config"
PATCHES="$ROOT/vendor/busybox/patches"
export SOURCE_DATE_EPOCH=1725148800
export KBUILD_BUILD_TIMESTAMP="2024-09-01 00:00:00 UTC"
export KBUILD_BUILD_USER=breenix KBUILD_BUILD_HOST=breenix

ARCH=""
while [ $# -gt 0 ]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        *) echo "usage: $0 --arch aarch64|x86_64" >&2; exit 2 ;;
    esac
done
case "$ARCH" in
    aarch64)
        OUT="$ROOT/userspace/programs/aarch64"
        LIBC="$ROOT/libs/libbreenix-libc/target/aarch64-breenix/release/libc.a"
        ;;
    x86_64)
        OUT="$ROOT/userspace/programs"
        LIBC="$ROOT/libs/libbreenix-libc/target/x86_64-breenix-c/release/libc.a"
        ;;
    *) echo "usage: $0 --arch aarch64|x86_64" >&2; exit 2 ;;
esac
[ -f "$LIBC" ] || { echo "libc-busybox: $LIBC is missing" >&2; exit 1; }

WORK="$ROOT/target/libc-busybox/$ARCH"
SRC="$WORK/busybox-1.37.0"
ELF="$OUT/libc-busybox.elf"
LINKS="$OUT/libc-busybox.links"
CC="$ROOT/scripts/breenix-cc"
TOOLS="$("$CC" --arch "$ARCH" --tool-dir)"
mkdir -p "$WORK" "$OUT"

inputs_digest() {
    {
        shasum -a 256 "$LIBC" "$ARCHIVE" "$FRAGMENT" "$CC" "${BASH_SOURCE[0]}" "$ROOT/vendor/busybox/required-applets" "$ROOT/scripts/install-libc-busybox.sh"
        find "$PATCHES" -type f | sort | xargs shasum -a 256
        find "$ROOT/libs/libbreenix-libc/include" -type f -name '*.h' | sort | xargs shasum -a 256
        "$TOOLS/clang" --version
    } | shasum -a 256 | cut -d' ' -f1
}
DIGEST="$(inputs_digest)"
if [ -f "$WORK/inputs.sha256" ] && [ "$(cat "$WORK/inputs.sha256")" = "$DIGEST" ] && [ -f "$WORK/applets.json" ] && [ -f "$ELF" ] && [ -f "$LINKS" ]; then
    echo "  libc-busybox ($ARCH): up to date"
    exit 0
fi
rm -f "$WORK/inputs.sha256" "$WORK/applets.json" "$ELF" "$LINKS"

# Cross tools BusyBox's Kbuild finds by prefix.
mkdir -p "$WORK/bin"
PREFIX="$WORK/bin/breenix-$ARCH-"
printf '#!/bin/sh\nexec "%s" --arch %s "$@"\n' "$CC" "$ARCH" > "${PREFIX}clang"
for tool in ar nm strip objcopy objdump; do
    printf '#!/bin/sh\nexec "%s/llvm-%s" "$@"\n' "$TOOLS" "$tool" > "${PREFIX}$tool"
done
chmod +x "${PREFIX}"*

# Re-extract when patches change, so removed/reordered patches never survive.
PATCH_DIGEST="$(find "$PATCHES" -type f | sort | xargs shasum -a 256 | shasum -a 256 | cut -d' ' -f1)"
if [ ! -f "$SRC/.extracted" ] || [ "$(cat "$SRC/.extracted")" != "$PATCH_DIGEST" ]; then
    python3 - "$ARCHIVE" "$SOURCE_SHA256" <<'PYVERIFY'
import hashlib, pathlib, sys
if hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest() != sys.argv[2]:
    sys.exit("libc-busybox: BusyBox source checksum mismatch")
PYVERIFY
    rm -rf "$SRC"
    tar -xjf "$ARCHIVE" -C "$WORK"
    while IFS= read -r patch_name || [ -n "$patch_name" ]; do
        case "$patch_name" in ''|'#'*) continue ;; esac
        case "$patch_name" in */*|*..*|*[!a-zA-Z0-9_.-]*) echo "Invalid patch name: $patch_name" >&2; exit 1 ;; esac
        [ -f "$PATCHES/$patch_name" ] || { echo "Missing BusyBox patch: $patch_name" >&2; exit 1; }
        patch -d "$SRC" -p1 < "$PATCHES/$patch_name"
    done < "$PATCHES/series"
    echo "$PATCH_DIGEST" > "$SRC/.extracted"
fi

# BB_GLOBAL_CONST empty: BusyBox's "*const" globals pointers are assigned once through
# a cast, and clang moves loads of them ahead of that store (libbb.h documents the
# switch for toolchains that do).
# BusyBox's disabled features leave unused upstream helpers and variables. Keep
# those warning exceptions in third-party compiler flags, not source patches or
# extra applets; our C programs still build with unqualified -Werror.
# Name the compiler clang so Kbuild excludes GCC-only optimization flags.
MAKE_ARGS=(-C "$SRC" CROSS_COMPILE="$PREFIX" CC="${PREFIX}clang" HOSTCC="$TOOLS/clang" "HOSTCFLAGS=-Wall -Wstrict-prototypes -O2 -fomit-frame-pointer -Wno-string-plus-int -Wno-self-assign"
           "EXTRA_CFLAGS=-std=gnu11 -Werror -Wno-unused-variable -Wno-unused-but-set-variable -Wno-unused-function -DBB_GLOBAL_CONST="
           SKIP_STRIP=y)
JOBS="${CARGO_BUILD_JOBS:-4}"

# Apply `fragment` (CONFIG_X=value lines; =n switches an option off) to .config.
apply_config() {
    python3 - "$SRC/.config" "$@" <<'PYCONFIG'
import re, sys
path, fragments = sys.argv[1], sys.argv[2:]
config = open(path).read()
for fragment in fragments:
    for line in open(fragment).read().splitlines():
        line = line.strip()
        m = re.match(r'^(CONFIG_[A-Z0-9_]+)=(.*)$', line)
        if not m:
            continue
        key, value = m.groups()
        new = f'# {key} is not set' if value == 'n' else f'{key}={value}'
        pattern = re.compile(rf'^(# {key} is not set|{key}=.*)$', re.MULTILINE)
        config = pattern.sub(new, config) if pattern.search(config) else config + new + '\n'
open(path, 'w').write(config)
PYCONFIG
}

configure() {
    make "${MAKE_ARGS[@]}" allnoconfig > "$WORK/config.log" 2>&1
    apply_config "$FRAGMENT" "$@"
    { yes "" || true; } | make "${MAKE_ARGS[@]}" oldconfig >> "$WORK/config.log" 2>&1
}

# This top-level prerequisite builds every applet/library directory without
# Kbuild's host-library link step. Link with Breenix's C driver below instead.
compile() {
    if ! make "${MAKE_ARGS[@]}" -k -j"$JOBS" applets/built-in.o > "$WORK/$1.log" 2>&1; then
        cat "$WORK/$1.log" >&2
        return 1
    fi
}

# Every target object the build produced (ELF, so not the host tools'), in one archive.
archive_objects() {
    rm -f "$WORK/objects.a"
    (cd "$SRC" && find . -name '*.o' -not -path './scripts/*' -not -name 'built-in.o' | sort \
        | python3 -c 'import sys; [print(p) for p in sys.stdin.read().split() if open(p, "rb").read(4) == b"\x7fELF"]') \
        > "$WORK/objects.txt"
    if [ -s "$WORK/objects.txt" ]; then
        (cd "$SRC" && xargs "$TOOLS/llvm-ar" rcs "$WORK/objects.a" < "$WORK/objects.txt")
    fi
}

echo "  libc-busybox ($ARCH): configuring"
configure
echo "  libc-busybox ($ARCH): compiling"
make "${MAKE_ARGS[@]}" clean > "$WORK/clean.log" 2>&1
compile_ok=1
compile compile || compile_ok=0
archive_objects

# Per-applet analysis.
python3 - "$SRC" "$WORK" "$CC" "$ARCH" "$FRAGMENT" "$compile_ok" <<'PYANALYZE'
import json, os, re, subprocess, sys
from pathlib import Path

src, work, cc, arch, fragment = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3], sys.argv[4], sys.argv[5]
config = (src / '.config').read_text()
enabled = set(re.findall(r'^CONFIG_([A-Z0-9_]+)=y$', config, re.M))

# Applets: //applet:IF_<CONFIG>(APPLET...(name, main?, ...)) lines in the sources.
applets = {}
pattern = re.compile(r'//applet:IF_([A-Z0-9_]+)\(\s*(APPLET(?:_NOEXEC|_NOFORK|_ODDNAME|_SCRIPTED)?)\(\s*([^,\s]+)\s*,\s*([^,\s]+)?')
for path in src.rglob('*.c'):
    if 'scripts' in path.relative_to(src).parts:
        continue
    for line in path.read_text(errors='replace').splitlines():
        m = pattern.search(line)
        if not m:
            continue
        cfg, kind, name, second = m.groups()
        if cfg not in enabled:
            continue
        main = name if kind in ('APPLET', 'APPLET_SCRIPTED') else second
        applets.setdefault(name, {'config': cfg, 'main': f'{main}_main', 'source': str(path.relative_to(src))})

objects = set((work / 'objects.txt').read_text().split())

def compiled(source):
    return './' + source[:-2] + '.o' in objects

def link(entry, defsyms=()):
    out = work / 'probe.elf'
    args = [cc, '--arch', arch, f'-Wl,-e,{entry}', f'-Wl,-u,{entry}', '-Wl,--error-limit=0',
            *[f'-Wl,--defsym={s}=0' for s in defsyms], str(work / 'objects.a'), '-o', str(out)]
    r = subprocess.run(args, capture_output=True, text=True)
    missing = set(re.findall(r'undefined (?:hidden )?symbol: (\S+)', r.stderr))
    # lld only warns when the entry itself is not defined.
    if re.search(rf'cannot find entry symbol {re.escape(entry)}\b', r.stderr):
        missing.add(entry)
    missing = sorted(missing)
    ok = r.returncode == 0 and not missing
    if r.returncode != 0 and not missing:
        errors = [l for l in r.stderr.splitlines() if 'error:' in l]
        missing = ['(link failed: ' + (errors[0] if errors else 'no message')[:200] + ')']
    out.unlink(missing_ok=True)
    return ok, missing

log = (work / 'compile.log').read_text(errors='replace')
failed_objects = sorted(set(re.findall(r"\[([A-Za-z0-9_./-]+\.o)\] Error", log)))
errors = []
for line in log.splitlines():
    m = re.search(r'(\S+\.[ch]):\d+:\d+: (?:fatal )?error: (.*)', line)
    if m and m.group(2) not in [e.split(': ', 1)[1] for e in errors]:
        errors.append(f'{m.group(1)}: {m.group(2)}')
mains = sorted({a['main'] for a in applets.values()})
if objects:
    core_links, core_missing = link('main', defsyms=mains)
else:
    core_links, core_missing = False, ['(nothing compiled)']
result = {'compile_ok': sys.argv[6] == '1', 'core': {'links': core_links, 'missing': core_missing}, 'objects_failed': failed_objects,
          'compile_errors': errors[:60], 'applets': {}}
for name in sorted(applets):
    a = applets[name]
    entry = {'config': a['config'], 'compiles': compiled(a['source']), 'links': False, 'missing': []}
    if entry['compiles']:
        entry['links'], entry['missing'] = link(a['main'])
    result['applets'][name] = entry
(work / 'applets.json').write_text(json.dumps(result, indent=1, sort_keys=True) + '\n')
PYANALYZE

# Reject configured applets that failed to compile or link.
rm -f "$ELF" "$LINKS"
python3 - "$WORK/applets.json" <<'PYCHECK'
import json, sys
result = json.load(open(sys.argv[1]))
failed = [name for name, app in result['applets'].items() if not (app['compiles'] and app['links'])]
if not result['compile_ok'] or not result['core']['links'] or failed or result['objects_failed']:
    if not result['compile_ok']:
        print('libc-busybox: compilation failed; see compile.log and applets.json', file=sys.stderr)
    if not result['core']['links']:
        print('libc-busybox: unresolved core: ' + ' '.join(result['core']['missing']), file=sys.stderr)
    if failed:
        print('libc-busybox: failed applets: ' + ' '.join(failed), file=sys.stderr)
    if result['objects_failed']:
        print('libc-busybox: failed objects: ' + ' '.join(result['objects_failed']), file=sys.stderr)
    for name in failed:
        print(f"  {name}: {result['applets'][name]}", file=sys.stderr)
    for error in result['compile_errors']:
        print('  ' + error, file=sys.stderr)
    sys.exit(1)
PYCHECK
if ! "$CC" --arch "$ARCH" -Wl,--error-limit=0 "$WORK/objects.a" -o "$WORK/busybox.elf" > "$WORK/link.log" 2>&1; then
    echo "libc-busybox: final link failed ($ARCH)" >&2
    cat "$WORK/link.log" >&2
    exit 1
fi
"$TOOLS/llvm-strip" -o "$ELF" "$WORK/busybox.elf"
python3 - "$WORK/applets.json" "$LINKS" <<'PYLINKS'
import json, sys
result = json.load(open(sys.argv[1]))
open(sys.argv[2], 'w').write(''.join(name + '\n' for name in sorted(result['applets'])))
PYLINKS
"$ROOT/scripts/install-libc-busybox.sh" --check "$OUT" "$ROOT/vendor/busybox/required-applets"

python3 - "$WORK/applets.json" "$ARCH" "$ELF" <<'PYREPORT'
import json, os, sys
result, arch, elf = json.load(open(sys.argv[1])), sys.argv[2], sys.argv[3]
apps = result['applets']
good = [n for n, a in apps.items() if a['compiles'] and a['links']]
print(f"  libc-busybox ({arch}): {len(good)}/{len(apps)} applets compile and link"
      + ("" if os.path.exists(elf) else "; no libc-busybox.elf"))
if not result['core']['links']:
    print(f"    start-up path does not link; missing: {' '.join(result['core']['missing'][:40])}")
if result['objects_failed']:
    print(f"    objects that did not compile: {' '.join(result['objects_failed'][:40])}")
for error in result['compile_errors']:
    print(f"    compile error: {error}")
for name, a in sorted(apps.items()):
    if not a['compiles']:
        print(f"    {name}: does not compile")
    elif not a['links']:
        print(f"    {name}: missing {' '.join(a['missing'][:30])}")
PYREPORT

echo "$DIGEST" > "$WORK/inputs.sha256"
