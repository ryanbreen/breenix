#!/bin/bash
# Build BusyBox against Breenix's own C library, applet by applet.
#
#   scripts/build-libc-busybox.sh --arch aarch64|x86_64
#
# Extracts the checksummed vendor/busybox/busybox-1.37.0.tar.bz2 under
# target/libc-busybox/<arch>/, configures it with vendor/busybox/breenix-libc.config
# on top of allnoconfig, and compiles it with scripts/breenix-cc, continuing past
# objects that do not compile. Then, for each configured applet, it links the
# applet's own code (entry <applet>_main) and BusyBox's common start-up path (entry
# main) against libc.a and records what does not resolve:
#
#   target/libc-busybox/<arch>/applets.json
#       { "core": {"links": bool, "missing": [...]},
#         "objects_failed": [...],
#         "applets": { "<name>": {"compiles": bool, "links": bool, "missing": [...]} } }
#
# Applets that do not compile or link are switched off and BusyBox is rebuilt and
# linked with the rest, as userspace/programs/{,aarch64/}libc-busybox.elf with
# libc-busybox.links naming its applets; the disk installs it as
# /usr/local/libc/bin/busybox with a hard link per applet. A switched-off applet's
# case fails with BusyBox's "applet not found", and applets.json says why. When the
# start-up path itself does not link, there is no libc-busybox.elf and every case
# fails saying BusyBox is not installed.
#
# The build is skipped when none of its inputs changed (libc.a, the headers, the
# config fragment, the source archive, breenix-cc and this script). It fails only
# when BusyBox cannot be built at all for reasons of its own (the archive, the
# configuration); missing library pieces are results, not failures.
set -euo pipefail
export LC_ALL=C

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARCHIVE="$ROOT/vendor/busybox/busybox-1.37.0.tar.bz2"
SOURCE_SHA256="3311dff32e746499f4df0d5df04d7eb396382d7e108bb9250e7b519b837043a4"
FRAGMENT="$ROOT/vendor/busybox/breenix-libc.config"
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
        shasum -a 256 "$LIBC" "$ARCHIVE" "$FRAGMENT" "$CC" "${BASH_SOURCE[0]}"
        find "$ROOT/libs/libbreenix-libc/include" -type f -name '*.h' | sort | xargs shasum -a 256
        "$TOOLS/clang" --version
    } | shasum -a 256 | cut -d' ' -f1
}
DIGEST="$(inputs_digest)"
if [ -f "$WORK/inputs.sha256" ] && [ "$(cat "$WORK/inputs.sha256")" = "$DIGEST" ] && [ -f "$WORK/applets.json" ] && [ -f "$ELF" ] && [ -f "$LINKS" ]; then
    echo "  libc-busybox ($ARCH): up to date"
    exit 0
fi
rm -f "$WORK/inputs.sha256"

# Cross tools BusyBox's Kbuild finds by prefix.
mkdir -p "$WORK/bin"
PREFIX="$WORK/bin/breenix-$ARCH-"
printf '#!/bin/sh\nexec "%s" --arch %s "$@"\n' "$CC" "$ARCH" > "${PREFIX}clang"
for tool in ar nm strip objcopy objdump; do
    printf '#!/bin/sh\nexec "%s/llvm-%s" "$@"\n' "$TOOLS" "$tool" > "${PREFIX}$tool"
done
chmod +x "${PREFIX}"*

if [ ! -f "$SRC/.extracted" ]; then
    python3 - "$ARCHIVE" "$SOURCE_SHA256" <<'PYVERIFY'
import hashlib, pathlib, sys
if hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest() != sys.argv[2]:
    sys.exit("libc-busybox: BusyBox source checksum mismatch")
PYVERIFY
    rm -rf "$SRC"
    tar -xjf "$ARCHIVE" -C "$WORK"
    touch "$SRC/.extracted"
fi

# BB_GLOBAL_CONST empty: BusyBox's "*const" globals pointers are assigned once through
# a cast, and clang moves loads of them ahead of that store (libbb.h documents the
# switch for toolchains that do).
# Name the compiler clang so Kbuild excludes GCC-only optimization flags.
MAKE_ARGS=(-C "$SRC" CROSS_COMPILE="$PREFIX" CC="${PREFIX}clang" HOSTCC=cc
           "EXTRA_CFLAGS=-std=gnu11 -Wno-error -DBB_GLOBAL_CONST="
           SKIP_STRIP=y)
JOBS="${CARGO_BUILD_JOBS:-6}"

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

# Compile everything that compiles. Kbuild's own link step needs libraries Breenix
# does not have (-lm, -lcrypt); the link below replaces it.
compile() {
    make "${MAKE_ARGS[@]}" -k -j"$JOBS" busybox_unstripped > "$WORK/$1.log" 2>&1 || true
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
compile compile
archive_objects

# Per-applet analysis.
python3 - "$SRC" "$WORK" "$CC" "$ARCH" "$FRAGMENT" <<'PYANALYZE'
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
result = {'core': {'links': core_links, 'missing': core_missing}, 'objects_failed': failed_objects,
          'compile_errors': errors[:60], 'applets': {}}
for name in sorted(applets):
    a = applets[name]
    entry = {'config': a['config'], 'compiles': compiled(a['source']), 'links': False, 'missing': []}
    if entry['compiles']:
        entry['links'], entry['missing'] = link(a['main'])
    result['applets'][name] = entry
(work / 'applets.json').write_text(json.dumps(result, indent=1, sort_keys=True) + '\n')
PYANALYZE

# Switch off what failed, rebuild, and link the rest.
python3 - "$WORK/applets.json" "$WORK/disable.config" <<'PYDISABLE'
import json, sys
result = json.load(open(sys.argv[1]))
off = sorted({a['config'] for a in result['applets'].values() if not (a['compiles'] and a['links'])})
lines = [f'CONFIG_{c}=n' for c in off]
# "sh" is a Kconfig choice: switching it off means choosing no shell.
if 'SH_IS_ASH' in off:
    lines += ['CONFIG_SH_IS_NONE=y', 'CONFIG_SHELL_ASH=n']
open(sys.argv[2], 'w').write(''.join(l + '\n' for l in lines))
PYDISABLE

rm -f "$ELF" "$LINKS"
core_links="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["core"]["links"])' "$WORK/applets.json")"
if [ "$core_links" = True ]; then
    if [ -s "$WORK/disable.config" ]; then
        configure "$WORK/disable.config"
        still_on="$(sed -n 's/^\(CONFIG_[A-Z0-9_]*\)=n$/\1/p' "$WORK/disable.config" \
            | while read -r option; do grep -q "^$option=y" "$SRC/.config" && echo "$option"; done || true)"
        if [ -n "$still_on" ]; then
            echo "libc-busybox: Kconfig kept on what was switched off: $still_on" >&2
            exit 1
        fi
        compile rebuild
        archive_objects
    fi
    if "$CC" --arch "$ARCH" -Wl,--error-limit=0 "$WORK/objects.a" -o "$WORK/busybox.elf" > "$WORK/link.log" 2>&1; then
        "$TOOLS/llvm-strip" -o "$ELF" "$WORK/busybox.elf"
        python3 - "$WORK/applets.json" "$LINKS" <<'PYLINKS'
import json, sys
result = json.load(open(sys.argv[1]))
names = [n for n, a in sorted(result['applets'].items()) if a['compiles'] and a['links']]
open(sys.argv[2], 'w').write(''.join(n + '\n' for n in names))
PYLINKS
    else
        echo "  libc-busybox ($ARCH): the final link failed; see $WORK/link.log" >&2
        tail -5 "$WORK/link.log" >&2
    fi
fi

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
