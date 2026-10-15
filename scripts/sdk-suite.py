#!/usr/bin/env python3
"""The host side of the Developer SDK suite (docs/suites/README.md, "Developer SDK").

    scripts/sdk-suite.py --arch aarch64|x86_64 [--image IMAGE] [--rebuild]

Builds the SDK from a clean clone of HEAD into a prefix outside the checkout, checks
the toolchain, copies each test app in userspace/sdk-suite/ into a fresh directory with
no path into the checkout and builds it with only the SDK, then, with --image, stages
the results onto that ext2 disk image under /usr/local/sdk-suite, where
/sbin/suite-sdk finds them. With --image it also installs apps with the SDK's disk
tool and follows the docs' quick start, both onto the same image.

Every host step leaves one record, /usr/local/sdk-suite/host/<category>/<case>, holding
"PASS" or "FAIL <reason>"; a step that cannot run (the SDK does not exist yet) is a FAIL
that says why, and the guest case that reads it fails with that reason.

Without --image it builds and checks only, keeping the results in a cache outside the
checkout keyed by the commit and this script, so a later --image run of the same commit
stages them without building again (--rebuild discards the cache). The script exits 0
when it recorded every step, and non-zero only when it could not stage the image.
It needs git, bash and debugfs/e2fsck from e2fsprogs (on PATH or Homebrew's keg).
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
ARCHES = ('aarch64', 'x86_64')
ELF_MACHINE = {'aarch64': 183, 'x86_64': 62}
GUEST_DIR = '/usr/local/sdk-suite'
BIN_DIR = '/usr/local/bin'
APP_PATH = '/usr/local/bin:/usr/bin:/bin'
REQUIRED_TEMPLATES = {'c': 'c', 'rust': 'rust', 'rust-no-std': 'nostd'}
INSTALLED = ('sdk-c-app', 'sdk-rust-app', 'sdk-nostd-app', 'sdk-private', 'sdk-replaced')
MUSL_MARKS = (b'__NEED_', b'ld-musl', b'__MUSL__', b'musl libc', b'Rich Felker')
GLIBC_MARKS = (b'__GLIBC__', b'GNU C Library', b'__THROW', b'libc.so.6', b'ld-linux', b'glibc')
OTHER_LIBC_MARKS = (b'_NEWLIB_VERSION', b'newlib', b'__BIONIC__', b'__UCLIBC__', b'uClibc',
                    b'dietlibc', b'__PICOLIBC__', b'picolibc')
# compiler_builtins' math is the libm crate, a port of musl's libm; scripts/libc-clean-archive.py
# removes it from libc.a by these symbol names, and the pinned fork builds none of it into the
# std rlibs. MUSL_LIBM_BYTES also finds it in an archive's rlib metadata, bitcode or data: its
# module path, or a path of its sources (libm's support traits, which compiler_builtins' integer
# and float helpers use, are not math).
MUSL_LIBM_SYMBOLS = ('libm_math', 'compiler_builtins4math', 'compiler_builtins..math..')
MUSL_LIBM_BYTES = re.compile(rb'libm_math|compiler_builtins4math|compiler_builtins\.\.math\.\.|libm/src/math/(?!support/)')

# Every case this script records, in manifest order. tests/suite_manifests.rs checks that
# each names a case of docs/suites/sdk.json and that the guest reads each one.
HOST_CASES = (
    'toolchain/build-script', 'toolchain/clean-build', 'toolchain/build-quiet',
    'toolchain/version-commit', 'toolchain/reproducible', 'toolchain/cc-aarch64',
    'toolchain/cc-x86-64', 'toolchain/cargo-aarch64', 'toolchain/cargo-x86-64',
    'toolchain/libc-archives', 'toolchain/rust-std', 'toolchain/std-fork-pin',
    'toolchain/target-specs', 'toolchain/headers-c17', 'toolchain/headers-posix',
    'toolchain/header-symbols', 'toolchain/headers-self-contained', 'toolchain/no-host-paths',
    'toolchain/no-musl', 'toolchain/no-glibc', 'toolchain/no-other-libc', 'toolchain/relocatable',
    'c-app/built', 'c-app/outside-tree', 'c-app/warnings',
    'rust-app/built', 'rust-app/nostd-built', 'rust-app/lto', 'rust-app/outside-tree',
    'install/tool', 'install/installed', 'install/removed', 'install/list',
    'install/fsck', 'install/arch-refused', 'install/remove-missing',
    'templates/tool', 'templates/c-create', 'templates/c-build', 'templates/rust-create',
    'templates/rust-build', 'templates/nostd-create', 'templates/nostd-build',
    'templates/every-template', 'templates/outside-tree', 'templates/no-clobber',
    'templates/quick-start-doc', 'templates/quick-start-host', 'templates/sdk-docs',
)
# Recorded while staging the image; the rest are recorded by the build.
STAGE_CASES = {c for c in HOST_CASES if c.startswith('install/')} | {'templates/quick-start-host'}

TIMEOUT_SDK = 3600
TIMEOUT_BUILD = 900
TIMEOUT_TOOL = 120


def say(text):
    print(f'[sdk-suite] {text}', flush=True)


# Local paths a record must not carry (records reach serial logs and reports), longest
# first, each with what it stands for.
SCRUB = []


def one_line(text, limit=400):
    text = ' '.join(str(text).split())
    for path, name in SCRUB:
        text = text.replace(path, name)
    return text if len(text) <= limit else text[:limit - 3] + '...'


class Records:
    def __init__(self, data=None):
        self.data = dict(data or {})

    def set(self, case, ok, why=''):
        assert case in HOST_CASES, case
        self.data[case] = 'PASS' if ok else 'FAIL ' + one_line(why or 'failed')

    def fail(self, case, why):
        self.set(case, False, why)

    def ok(self, case):
        return self.data.get(case) == 'PASS'


class Ran:
    def __init__(self, code, out, err, timed_out=False):
        self.code, self.out, self.err, self.timed_out = code, out, err, timed_out

    @property
    def ok(self):
        return self.code == 0 and not self.timed_out

    def why(self, what):
        if self.timed_out:
            return f'{what} did not finish in time and was killed'
        tail = (self.err.strip() or self.out.strip()).splitlines()[-3:]
        return f'{what} exited {self.code}' + (': ' + ' | '.join(tail) if tail else '')


def run(argv, cwd, env, timeout, log=None, stdin=None):
    """Run argv in its own process group, killing the group on timeout."""
    try:
        proc = subprocess.Popen([str(a) for a in argv], cwd=str(cwd), env=env, stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    except OSError as error:
        return Ran(127, '', f'could not start {Path(str(argv[0])).name}: {error.strerror}')
    try:
        out, err = proc.communicate(input=stdin, timeout=timeout)
        result = Ran(proc.returncode, out.decode(errors='replace'), err.decode(errors='replace'))
    except subprocess.TimeoutExpired:
        os.killpg(proc.pid, signal.SIGKILL)
        out, err = proc.communicate()
        result = Ran(-9, out.decode(errors='replace'), err.decode(errors='replace'), timed_out=True)
    if log is not None:
        with open(log, 'a') as handle:
            handle.write(f'$ {" ".join(str(a) for a in argv)}\n# cwd {cwd}\n{result.out}{result.err}# exit {result.code}\n\n')
    return result


def tool_dirs():
    """Directories holding the host prerequisites the SDK documents: cargo/rustup."""
    dirs = []
    for name in ('cargo', 'rustup'):
        found = shutil.which(name)
        if found and str(Path(found).parent) not in dirs:
            dirs.append(str(Path(found).parent))
    return dirs


def real_home():
    return Path(os.path.expanduser('~')).resolve()


def sdk_env(sdk, home, extra=None):
    """The whole environment an app build gets: the SDK's bin, the Rust prerequisites
    and the system's own directories, a home of its own, and nothing from the checkout."""
    path = ([str(sdk / 'bin')] if sdk else []) + tool_dirs() + ['/usr/bin', '/bin', '/usr/sbin', '/sbin']
    env = {
        'PATH': ':'.join(path),
        'HOME': str(home),
        'RUSTUP_HOME': os.environ.get('RUSTUP_HOME', str(real_home() / '.rustup')),
        'CARGO_HOME': os.environ.get('CARGO_HOME', str(real_home() / '.cargo')),
        'TMPDIR': str(home),
        'LANG': 'C',
        'CARGO_BUILD_JOBS': os.environ.get('CARGO_BUILD_JOBS', '4'),
        'CARGO_NET_OFFLINE': 'true',
    }
    env.update(extra or {})
    return env


def commit_of(repo):
    return subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip()


def export(repo, commit, dest):
    """A clean checkout: a fresh local clone of the repository at the commit, detached."""
    subprocess.run(['git', 'clone', '--quiet', '--no-checkout', str(repo), str(dest)], check=True)
    subprocess.run(['git', '-C', str(dest), 'switch', '--quiet', '--detach', commit], check=True)


def has_file(repo, commit, path):
    return subprocess.run(['git', '-C', str(repo), 'cat-file', '-e', f'{commit}:{path}'], capture_output=True).returncode == 0


def files_under(directory):
    if not directory.exists():
        return []
    return sorted(p for p in directory.rglob('*') if p.is_file() and not p.is_symlink())


def scan(directory, needles):
    """Map each needle found to the relative paths of the files holding it."""
    found = {}
    for path in files_under(directory):
        try:
            data = path.read_bytes()
        except OSError:
            continue
        for needle in needles:
            if needle and needle in data:
                found.setdefault(needle, []).append(str(path.relative_to(directory)))
    return found


def describe_found(found, names):
    parts = []
    for needle, paths in found.items():
        shown = ', '.join(paths[:3]) + (f' and {len(paths) - 3} more' if len(paths) > 3 else '')
        parts.append(f'{names.get(needle, needle.decode(errors="replace"))} in {shown}')
    return '; '.join(parts)


def elf_machine(data):
    if len(data) < 20 or data[:4] != b'\x7fELF':
        return None
    endian = '<' if data[5] == 1 else '>'
    return struct.unpack(endian + 'H', data[18:20])[0]


def ar_members(data):
    """(name, bytes) of each member of a System V/GNU ar archive."""
    if not data.startswith(b'!<arch>\n'):
        return None
    members, offset, names = [], 8, b''
    while offset + 60 <= len(data):
        header = data[offset:offset + 60]
        name = header[:16].decode(errors='replace').strip()
        size = int(header[48:58].decode().strip() or 0)
        body = data[offset + 60:offset + 60 + size]
        if name == '//':
            names = body
        elif name.startswith('/') and name[1:].isdigit() and names:
            start = int(name[1:])
            name = names[start:names.index(b'/\n', start)].decode(errors='replace')
        members.append((name.rstrip('/'), body))
        offset += 60 + size + (size & 1)
    return members


def archive_machines(path):
    members = ar_members(path.read_bytes()) or []
    return {elf_machine(body) for _, body in members if elf_machine(body) is not None}


def parse_quick_start(text):
    """The quick start's machine-read blocks: (shell commands, run line, expected output)."""
    blocks, current, info = [], None, None
    for line in text.splitlines(keepends=True):
        fence = re.match(r'^```\s*(.*?)\s*$', line)
        if fence and current is None:
            current, info = [], fence.group(1)
        elif fence and current is not None and not fence.group(1):
            blocks.append((info, ''.join(current)))
            current = None
        elif current is not None:
            current.append(line)
    shell = [body for info, body in blocks if info == 'sh quick-start']
    run_lines = [body.strip() for info, body in blocks if info == 'breenix quick-start-run']
    outputs = [body for info, body in blocks if info == 'text quick-start-output']
    return shell, run_lines, outputs


def find_e2fsprogs(name):
    for candidate in (name, f'/sbin/{name}', f'/usr/sbin/{name}', f'/opt/homebrew/opt/e2fsprogs/sbin/{name}',
                      f'/usr/local/opt/e2fsprogs/sbin/{name}'):
        found = shutil.which(candidate)
        if found:
            return found
    return None


class Harness:
    def __init__(self, arch, rebuild):
        self.arch = arch
        self.other = 'x86_64' if arch == 'aarch64' else 'aarch64'
        self.commit = commit_of(ROOT)
        script = Path(__file__).read_bytes()
        self.key = hashlib.sha256(f'{self.commit}\0{arch}\0'.encode() + script).hexdigest()
        tag = hashlib.sha256(str(ROOT).encode()).hexdigest()[:12]
        self.cache = Path(tempfile.gettempdir()).resolve() / 'breenix-sdk-suite' / tag / arch
        if rebuild and self.cache.exists():
            shutil.rmtree(self.cache)
        self.sdk = self.cache / 'sdk'
        aliases = [(self.sdk, 'SDK'), (self.cache, '<work>'), (ROOT, '<checkout>'), (real_home(), '~')]
        for path, name in aliases:
            for form in {str(path), str(path).removeprefix('/private')}:
                SCRUB.append((form, name))
        SCRUB.sort(key=lambda pair: -len(pair[0]))
        self.stage = self.cache / 'stage'
        self.logs = self.cache / 'logs'
        self.work = self.cache / 'work'
        self.records = Records()
        fork = ROOT / 'rust-fork' / 'library'
        override = os.environ.get('BREENIX_RUST_FORK_LIBRARY')
        self.fork = Path(override).resolve() if override else (fork.resolve() if fork.exists() else None)
        self.apps = {}

    # ----------------------------------------------------------------- build phase

    def cached(self):
        state = self.cache / 'state.json'
        if not state.exists():
            return False
        data = json.loads(state.read_text())
        if data.get('key') != self.key:
            return False
        self.records = Records(data['records'])
        self.apps = {k: Path(v) for k, v in data['apps'].items()}
        return True

    def build(self):
        if self.cached():
            say(f'reusing the build of {self.commit[:12]} for {self.arch}')
            return
        if self.cache.exists():
            shutil.rmtree(self.cache)
        for directory in (self.stage, self.logs, self.work):
            directory.mkdir(parents=True)
        self.home = self.work / 'home'
        self.home.mkdir()
        say(f'building the SDK of {self.commit[:12]} for {self.arch} in {self.cache}')
        exported = self.work / 'export'
        if has_file(ROOT, self.commit, 'scripts/build-sdk.sh'):
            export(ROOT, self.commit, exported)
            self.step(lambda: self.build_sdk(exported), 'toolchain/')
            if self.sdk_built():
                self.step(lambda: self.check_toolchain(exported), 'toolchain/')
            # Nothing built from here on may need the checkout or its clone.
            shutil.rmtree(exported)
        else:
            self.records.fail('toolchain/build-script', 'the commit has no scripts/build-sdk.sh')
            self.records.fail('toolchain/clean-build', 'there is no SDK build script to run (scripts/build-sdk.sh)')
        if not self.sdk_built():
            self.not_built_toolchain()
        self.step(self.build_c_app, 'c-app/')
        self.step(self.build_rust_apps, 'rust-app/')
        self.step(self.build_other_arch, 'install/arch-refused')
        self.step(self.build_templates, 'templates/')
        self.step(self.quick_start_doc, 'templates/quick-start-doc', 'templates/sdk-docs')
        state = {'key': self.key, 'records': self.records.data, 'apps': {k: str(v) for k, v in self.apps.items()}}
        (self.cache / 'state.json').write_text(json.dumps(state, indent=1))

    def step(self, action, *owned):
        """Run one step; if it raises, fail the cases it owns that it had not recorded.

        `owned` are case ids or category prefixes ending in '/'."""
        try:
            action()
        except Exception as error:  # a broken step is a recorded failure, not a crash
            say(f'a host step raised {type(error).__name__}: {error}')
            for case in HOST_CASES:
                mine = any(case == o or (o.endswith('/') and case.startswith(o)) for o in owned)
                if mine and case not in self.records.data:
                    self.records.fail(case, f'the host step raised {type(error).__name__}: {error}')

    def sdk_built(self):
        return self.records.ok('toolchain/clean-build')

    def leak_needles(self):
        needles = {str(ROOT).encode(): 'the checkout\'s path', str(self.work / 'export').encode(): 'the clean clone\'s path',
                   str(self.work / 'export2').encode(): 'the second clone\'s path',
                   str(real_home()).encode(): 'the builder\'s home directory'}
        if self.fork:
            needles[str(self.fork).encode()] = 'the Rust fork\'s path'
        return needles

    def build_env(self):
        env = dict(os.environ, CARGO_BUILD_JOBS=os.environ.get('CARGO_BUILD_JOBS', '4'), CARGO_NET_OFFLINE='true',
                   HOME=str(self.home))
        env.setdefault('RUSTUP_HOME', str(real_home() / '.rustup'))
        env.setdefault('CARGO_HOME', str(real_home() / '.cargo'))
        for name in ('CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET_DIR', 'RUSTFLAGS', 'BREENIX_USERSPACE_REMAP'):
            env.pop(name, None)
        if self.fork:
            env['BREENIX_RUST_FORK_LIBRARY'] = str(self.fork)
        return env

    def build_sdk(self, exported):
        script = exported / 'scripts' / 'build-sdk.sh'
        r = self.records
        r.set('toolchain/build-script', os.access(script, os.X_OK), 'scripts/build-sdk.sh is not executable')
        say('running scripts/build-sdk.sh from a clean clone of the commit')
        ran = run(['bash', script, '--prefix', self.sdk], exported, self.build_env(), TIMEOUT_SDK, self.logs / 'build-sdk.log')
        if not ran.ok:
            r.fail('toolchain/clean-build', ran.why('scripts/build-sdk.sh --prefix DIR'))
            return
        if not (self.sdk / 'VERSION').is_file():
            r.fail('toolchain/clean-build', 'scripts/build-sdk.sh exited 0 but the prefix has no VERSION')
            return
        r.set('toolchain/clean-build', True)
        warnings = [line.strip() for line in (ran.out + ran.err).splitlines()
                    if re.match(r'^\s*warning[:\[]', line) and 'neon' not in line.lower()]
        r.set('toolchain/build-quiet', not warnings, f'{len(warnings)} warnings, the first: {warnings[0] if warnings else ""}')

    def not_built_toolchain(self):
        why = 'the SDK was not built: ' + self.records.data['toolchain/clean-build'][5:]
        for case in HOST_CASES:
            if case.startswith('toolchain/') and case not in self.records.data:
                self.records.fail(case, why)

    def tool(self, arch, kind):
        return self.sdk / 'bin' / f'{arch}-breenix-{kind}'

    def check_toolchain(self, exported):
        r = self.records
        line = (self.sdk / 'VERSION').read_text(errors='replace')
        match = re.fullmatch(r'breenix-sdk (\S+) ([0-9a-f]{40})\n?', line)
        if not match:
            r.fail('toolchain/version-commit', f'VERSION reads {line.strip()[:80]!r}, not "breenix-sdk <version> <40-hex commit>"')
        else:
            r.set('toolchain/version-commit', match.group(2) == self.commit,
                  f'VERSION names commit {match.group(2)[:12]}, but it was built from {self.commit[:12]}')
        self.check_reproducible()
        for arch in ARCHES:
            slug = arch.replace('_', '-')
            for kind in ('cc', 'cargo'):
                self.check_version_line(f'toolchain/{kind}-{slug}', self.tool(arch, kind), arch)
        self.check_libc_archives()
        self.check_rust_std()
        pin = self.sdk / 'share' / 'breenix-sdk' / 'rust-fork.toml'
        r.set('toolchain/std-fork-pin', pin.is_file() and pin.read_bytes() == (exported / 'rust-fork.toml').read_bytes(),
              'share/breenix-sdk/rust-fork.toml is missing or differs from the commit\'s rust-fork.toml')
        self.check_target_specs()
        self.check_headers(exported)
        self.check_scans()
        self.check_relocatable(exported)

    def check_reproducible(self):
        second = self.work / 'export2'
        export(ROOT, self.commit, second)
        prefix = self.work / 'sdk2'
        say('building the SDK a second time from a clone at another path, in a hostile environment')
        ran = run(['bash', second / 'scripts' / 'build-sdk.sh', '--prefix', prefix], second, self.hostile_env(),
                  TIMEOUT_SDK, self.logs / 'build-sdk-2.log')
        if not ran.ok:
            self.records.fail('toolchain/reproducible', ran.why('the second build'))
        else:
            differences = []
            left = {str(p.relative_to(self.sdk)): p for p in self.sdk.rglob('*')}
            right = {str(p.relative_to(prefix)): p for p in prefix.rglob('*')}
            for name in sorted(set(left) | set(right)):
                a, b = left.get(name), right.get(name)
                if a is None or b is None:
                    differences.append(f'{name} is only in one build')
                elif a.is_symlink() or b.is_symlink():
                    if not (a.is_symlink() and b.is_symlink() and os.readlink(a) == os.readlink(b)):
                        differences.append(f'{name} links differ')
                elif a.is_file() and (not b.is_file() or a.read_bytes() != b.read_bytes()
                                      or (a.stat().st_mode & 0o7777) != (b.stat().st_mode & 0o7777)):
                    differences.append(f'{name} differs')
            self.records.set('toolchain/reproducible', not differences,
                             f'{len(differences)} paths differ between the two builds, the first: {", ".join(differences[:3])}')
        shutil.rmtree(prefix, ignore_errors=True)
        shutil.rmtree(second, ignore_errors=True)
        shutil.rmtree(self.work / 'hostile', ignore_errors=True)

    def hostile_env(self):
        """The build's environment plus a builder's settings that must not reach the SDK: a
        cargo home whose config.toml moves the target directory and changes the release
        profile and rustc's flags, the same through CARGO_* and RUSTFLAGS, and C compiler
        settings. The cargo home shares the real one's download caches."""
        env = self.build_env()
        real = Path(env['CARGO_HOME'])
        hostile = self.work / 'hostile'
        home = hostile / 'cargo-home'
        home.mkdir(parents=True)
        for entry in ('registry', 'git', '.package-cache', '.package-cache-mutate', '.global-cache'):
            if (real / entry).exists():
                (home / entry).symlink_to(real / entry)
        (home / 'config.toml').write_text(
            '[build]\n'
            f'target-dir = "{hostile / "target"}"\n'
            'rustflags = ["-Copt-level=1", "-Cdebuginfo=2"]\n'
            '[profile.release]\nopt-level = 1\ndebug = 2\ncodegen-units = 7\n')
        (hostile / 'include').mkdir()
        env.update({
            'CARGO_HOME': str(home),
            'CARGO_PROFILE_RELEASE_OPT_LEVEL': '1',
            'CARGO_PROFILE_RELEASE_DEBUG': '2',
            'CARGO_BUILD_TARGET_DIR': str(hostile / 'target2'),
            'CARGO_TARGET_DIR': str(hostile / 'target3'),
            'RUSTFLAGS': '-Copt-level=1',
            'CARGO_ENCODED_RUSTFLAGS': '-Cdebuginfo=2',
            'CFLAGS': '-O0 -g',
            'CPATH': str(hostile / 'include'),
        })
        return env

    def check_version_line(self, case, tool, arch):
        if not tool.is_file():
            self.records.fail(case, f'the SDK has no bin/{tool.name}')
            return
        ran = run([tool, '--version'], self.home, sdk_env(self.sdk, self.home), TIMEOUT_TOOL)
        first = (ran.out.splitlines() or [''])[0]
        missing = [w for w in ('breenix-sdk', arch, self.commit) if w not in first]
        if not ran.ok:
            self.records.fail(case, ran.why(f'{tool.name} --version'))
        else:
            self.records.set(case, not missing, f'{tool.name} --version printed {first[:120]!r}, which does not name {", ".join(m[:12] for m in missing)}')

    def check_libc_archives(self):
        archives = [p for p in files_under(self.sdk) if p.name == 'libc.a']
        problems, have = [], set()
        for path in archives:
            machines = archive_machines(path)
            rel = str(path.relative_to(self.sdk))
            if len(machines) != 1:
                problems.append(f'{rel} holds objects for {len(machines)} machines')
            have |= machines
        for arch in ARCHES:
            if ELF_MACHINE[arch] not in have:
                problems.append(f'no libc.a for {arch}')
        self.records.set('toolchain/libc-archives', not problems, '; '.join(problems))

    def check_rust_std(self):
        have = set()
        for path in files_under(self.sdk):
            if path.name.startswith('libstd-') and path.suffix == '.rlib':
                have |= archive_machines(path)
        missing = [arch for arch in ARCHES if ELF_MACHINE[arch] not in have]
        self.records.set('toolchain/rust-std', not missing, f'the SDK has no prebuilt std (libstd-*.rlib) for {", ".join(missing)}')

    def check_target_specs(self):
        have = set()
        for path in sorted((self.sdk / 'share' / 'breenix-sdk' / 'targets').glob('*.json')):
            try:
                spec = json.loads(path.read_text())
            except ValueError:
                continue
            if spec.get('os') == 'breenix':
                have.add(spec.get('arch'))
        missing = [arch for arch in ARCHES if arch not in have]
        self.records.set('toolchain/target-specs', not missing,
                         f'share/breenix-sdk/targets has no rustc target spec with os breenix for {", ".join(missing)}')

    def compile(self, arch, args, cwd, log, extra=None):
        tool = self.tool(arch, 'cc')
        if not tool.is_file():
            return Ran(127, '', f'the SDK has no bin/{tool.name}')
        return run([tool, *args], cwd, sdk_env(self.sdk, self.home, extra), TIMEOUT_TOOL, log)

    def check_headers(self, exported):
        lists = exported / 'userspace' / 'sdk-suite' / 'headers'
        scratch = self.work / 'headers'
        scratch.mkdir()
        log = self.logs / 'headers.log'
        for case, listing, std in (('toolchain/headers-c17', 'c17-headers.txt', ''),
                                   ('toolchain/headers-posix', 'posix-headers.txt', '#define _POSIX_C_SOURCE 202405L\n')):
            headers = (lists / listing).read_text().split()
            problems = []
            for arch in ARCHES:
                failing = []
                for header in headers:
                    source = scratch / 'one.c'
                    source.write_text(f'{std}#include <{header}>\n')
                    if not self.compile(arch, ['-std=c17', '-fsyntax-only', source.name], scratch, log).ok:
                        failing.append(header)
                source = scratch / 'all.c'
                source.write_text(std + ''.join(f'#include <{h}>\n' for h in headers))
                together = self.compile(arch, ['-std=c17', '-fsyntax-only', source.name], scratch, log).ok
                if failing:
                    problems.append(f'{arch}: {len(failing)} of {len(headers)} fail alone ({", ".join(failing[:12])}{", ..." if len(failing) > 12 else ""})')
                elif not together:
                    problems.append(f'{arch}: every header compiles alone but not all {len(headers)} together')
            self.records.set(case, not problems, '; '.join(problems))
        problems = []
        shutil.copy(lists / 'symbols.c', scratch / 'symbols.c')
        for arch in ARCHES:
            ran = self.compile(arch, ['-std=c17', '-o', 'symbols', 'symbols.c', '-lm', '-lpthread'], scratch, log)
            if not ran.ok:
                undefined = sorted(set(re.findall(r'undefined symbol: (\S+)', ran.err)))
                undeclared = sorted(set(re.findall(r"undeclared (?:function|identifier) '(\w+)'", ran.err)))
                headers = sorted(set(re.findall(r"'([\w/]+\.h)' file not found", ran.err)))
                detail = (f'missing header {", ".join(headers)}' if headers else
                          f'{len(undeclared)} undeclared ({", ".join(undeclared[:10])})' if undeclared else
                          f'{len(undefined)} undefined ({", ".join(undefined[:10])})' if undefined else ran.why('the compile'))
                problems.append(f'{arch}: {detail}')
        self.records.set('toolchain/header-symbols', not problems, '; '.join(problems))
        problems = []
        for arch in ARCHES:
            resource = self.compile(arch, ['-print-resource-dir'], scratch, log)
            source = scratch / 'all.c'
            source.write_text('#define _POSIX_C_SOURCE 202405L\n' + ''.join(
                f'#include <{h}>\n' for h in (lists / 'posix-headers.txt').read_text().split()))
            ran = self.compile(arch, ['-std=c17', '-M', '-MG', source.name], scratch, log)
            if not ran.ok or not resource.ok:
                problems.append(f'{arch}: ' + (ran if not ran.ok else resource).why('listing the headers a compile reads'))
                continue
            allowed = [str(self.sdk.resolve()), str(self.sdk), str(Path(resource.out.strip()).resolve()), resource.out.strip()]
            # -MG lists a missing header by its bare name; only paths say where one was read.
            paths = [p for p in ran.out.replace('\\\n', ' ').split()[1:] if p != source.name and '/' in p
                     and os.path.isabs(p)]
            foreign = sorted({str(Path(p).parent) for p in paths
                              if not any(os.path.realpath(p).startswith(a + '/') or p.startswith(a + '/') for a in allowed)})
            if foreign:
                problems.append(f'{arch}: headers read from {len(foreign)} directories outside the SDK and the compiler\'s resource directory')
        self.records.set('toolchain/headers-self-contained', not problems, '; '.join(problems))
        shutil.rmtree(scratch)

    def check_scans(self):
        r = self.records
        names = self.leak_needles()
        found = scan(self.sdk, list(names))
        r.set('toolchain/no-host-paths', not found, 'the SDK holds ' + describe_found(found, names))
        found = scan(self.sdk, list(MUSL_MARKS))
        libm = self.libm_symbols()
        why = []
        if found:
            why.append('musl marks: ' + describe_found(found, {}))
        if libm:
            why.append('compiler_builtins\' libm math (a port of musl\'s libm) in ' + ', '.join(libm[:3]))
        r.set('toolchain/no-musl', not why, '; '.join(why))
        found = scan(self.sdk, list(GLIBC_MARKS))
        r.set('toolchain/no-glibc', not found, 'glibc marks: ' + describe_found(found, {}))
        found = scan(self.sdk, list(OTHER_LIBC_MARKS))
        r.set('toolchain/no-other-libc', not found, 'other C libraries\' marks: ' + describe_found(found, {}))

    def libm_symbols(self):
        """SDK archives whose symbol tables define compiler_builtins' musl-derived math, whose
        bytes (rlib metadata, bitcode, data) hold its module or source paths, or that could not
        be read (named so, which fails the case rather than passing it unread).

        The archives are read with the pinned toolchain's own llvm-nm: rlibs carry rustc's
        LLVM bitcode, which an older host LLVM cannot read."""
        sysroot = run(['rustc', '--print', 'sysroot'], ROOT, dict(os.environ), TIMEOUT_TOOL)
        host = run(['rustc', '-vV'], ROOT, dict(os.environ), TIMEOUT_TOOL)
        triple = re.search(r'^host: (\S+)$', host.out, re.MULTILINE)
        nm = Path(sysroot.out.strip()) / 'lib' / 'rustlib' / (triple.group(1) if triple else '') / 'bin' / 'llvm-nm'
        if not sysroot.ok or not triple or not nm.exists():
            return ['(the pinned toolchain has no llvm-nm, so no archive was read)']
        holding = []
        for path in files_under(self.sdk):
            if path.suffix not in ('.a', '.rlib'):
                continue
            ran = run([nm, '--defined-only', '--format=posix', path], self.home, dict(os.environ), TIMEOUT_TOOL)
            if not ran.ok:
                holding.append(f'{path.relative_to(self.sdk)} (llvm-nm could not read it: {ran.why("llvm-nm")})')
            elif any(mark in line for line in ran.out.splitlines() for mark in MUSL_LIBM_SYMBOLS):
                holding.append(str(path.relative_to(self.sdk)))
            elif MUSL_LIBM_BYTES.search(path.read_bytes()):
                holding.append(f'{path.relative_to(self.sdk)} (in its metadata, bitcode or data)')
        return holding

    def check_relocatable(self, exported):
        moved = self.work / 'moved-sdk'
        shutil.copytree(self.sdk, moved, symlinks=True)
        scratch = self.work / 'moved-app'
        scratch.mkdir()
        shutil.copy(exported / 'userspace' / 'sdk-suite' / 'hello.c', scratch)
        problems = []
        for arch in ARCHES:
            tool = moved / 'bin' / f'{arch}-breenix-cc'
            ran = run([tool, '-o', f'hello-{arch}', 'hello.c'], scratch, sdk_env(moved, self.home), TIMEOUT_TOOL, self.logs / 'relocatable.log')
            built = scratch / f'hello-{arch}'
            if not ran.ok:
                problems.append(f'{arch}: ' + ran.why(f'{tool.name} from the copied SDK'))
            elif not built.is_file() or elf_machine(built.read_bytes()) != ELF_MACHINE[arch]:
                problems.append(f'{arch}: the copied SDK did not produce an {arch} program')
        self.records.set('toolchain/relocatable', not problems, '; '.join(problems))
        shutil.rmtree(moved)
        shutil.rmtree(scratch)

    # -------------------------------------------------------------- the test apps

    def fresh_copy(self, name):
        """The app's sources from the commit in a new directory outside the checkout."""
        dest = self.work / 'apps' / name
        dest.parent.mkdir(exist_ok=True)
        archive = subprocess.Popen(['git', '-C', str(ROOT), 'archive', '--format=tar', self.commit, f'userspace/sdk-suite/apps/{name}'],
                                   stdout=subprocess.PIPE)
        unpack = self.work / 'apps' / f'.{name}'
        unpack.mkdir()
        subprocess.run(['tar', '-xf', '-', '-C', str(unpack)], stdin=archive.stdout, check=True)
        archive.stdout.close()
        archive.wait()
        shutil.move(str(unpack / 'userspace' / 'sdk-suite' / 'apps' / name), str(dest))
        shutil.rmtree(unpack)
        return dest

    def stage_app(self, name, program):
        dest = self.stage / 'apps' / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(program, dest)
        dest.chmod(0o755)
        self.apps[name] = dest

    def outside_tree(self, directories):
        names = {k: v for k, v in self.leak_needles().items() if v in ('the checkout\'s path', 'the clean clone\'s path',
                                                                    'the second clone\'s path')}
        found = {}
        for directory in directories:
            for needle, paths in scan(directory, list(names)).items():
                found.setdefault(needle, []).extend(str((directory / p).relative_to(self.work)) for p in paths)
        return found, names

    def build_c_app(self):
        r = self.records
        if not self.sdk_built():
            why = 'the SDK was not built, so the C app could not be: ' + r.data['toolchain/clean-build'][5:]
            for case in ('c-app/built', 'c-app/outside-tree', 'c-app/warnings'):
                r.fail(case, why)
            return
        say('building the C app outside the checkout with the SDK')
        app = self.fresh_copy('c-app')
        log = self.logs / 'c-app.log'
        sources = sorted(p.name for p in app.glob('*.c'))
        # A builder's environment may name its own header directories, which clang searches
        # ahead of the SDK's: the app builds with CPATH and C_INCLUDE_PATH naming a directory
        # where every header it includes is an #error.
        poison = self.work / 'host-include'
        poison.mkdir()
        for path in sorted(app.glob('*.[ch]')):
            for name in re.findall(r'^\s*#\s*include\s*<([^>]+)>', path.read_text(errors='replace'), re.M):
                (poison / name).parent.mkdir(parents=True, exist_ok=True)
                (poison / name).write_text(f'#error the host header {name} was used\n')
        hostile = {'CPATH': str(poison), 'C_INCLUDE_PATH': str(poison)}
        warnings, failed = [], None
        for source in sources:
            ran = self.compile(self.arch, ['-std=c17', '-O2', '-Wall', '-Wextra', '-MD', '-c', source,
                                           '-o', source[:-2] + '.o'], app, log, hostile)
            warnings += [l for l in ran.err.splitlines() if 'warning:' in l]
            if not ran.ok:
                failed = ran.why(f'compiling {source}')
                break
        if failed is None:
            ran = self.compile(self.arch, ['-o', 'c-app', *[s[:-2] + '.o' for s in sources], '-lm', '-lpthread'], app, log)
            if not ran.ok:
                failed = ran.why('linking c-app')
        program = app / 'c-app'
        if failed is None and (not program.is_file() or elf_machine(program.read_bytes()) != ELF_MACHINE[self.arch]):
            failed = f'the link produced no {self.arch} ELF program'
        r.set('c-app/built', failed is None, failed)
        r.set('c-app/warnings', failed is None and not warnings,
              failed and 'the C app did not build' or f'{len(warnings)} warnings, the first: {warnings[0] if warnings else ""}')
        if failed is None:
            found, names = self.outside_tree([app])
            r.set('c-app/outside-tree', not found, 'the build holds ' + describe_found(found, names))
            self.stage_app('c-app', program)
        else:
            r.fail('c-app/outside-tree', 'the C app did not build')

    def cargo_build(self, app, log, extra=None):
        """Build a cargo project with the SDK's wrapper; returns (Ran, program path or None)."""
        tool = self.tool(self.arch, 'cargo')
        if not tool.is_file():
            return Ran(127, '', f'the SDK has no bin/{tool.name}'), None
        ran = run([tool, 'build', '--release', '--offline', '--message-format=json-render-diagnostics'], app,
                  sdk_env(self.sdk, self.home, extra), TIMEOUT_BUILD, log)
        program = None
        for line in ran.out.splitlines():
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if message.get('reason') == 'compiler-artifact' and message.get('executable'):
                program = Path(message['executable'])
        return ran, program

    def build_rust_apps(self):
        r = self.records
        if not self.sdk_built():
            why = 'the SDK was not built, so the Rust apps could not be: ' + r.data['toolchain/clean-build'][5:]
            for case in ('rust-app/built', 'rust-app/nostd-built', 'rust-app/lto', 'rust-app/outside-tree'):
                r.fail(case, why)
            return
        built = []
        for name, case in (('rust-app', 'rust-app/built'), ('rust-nostd-app', 'rust-app/nostd-built')):
            say(f'building {name} outside the checkout with the SDK')
            app = self.fresh_copy(name)
            ran, program = self.cargo_build(app, self.logs / f'{name}.log')
            if not ran.ok:
                r.fail(case, ran.why(f'{self.arch}-breenix-cargo build --release --offline'))
            elif program is None or not program.is_file() or elf_machine(program.read_bytes()) != ELF_MACHINE[self.arch]:
                r.fail(case, f'cargo reported no {self.arch} executable')
            else:
                r.set(case, True)
                self.stage_app(name, program)
                built.append(app)
        if built and built[0].name == 'rust-app':
            # Fat LTO reads every crate's bitcode, std's and compiler_builtins' included.
            say('building rust-app again with cross-crate fat LTO')
            lto = {'CARGO_PROFILE_RELEASE_LTO': 'fat', 'CARGO_TARGET_DIR': str(built[0] / 'target-lto')}
            ran, program = self.cargo_build(built[0], self.logs / 'rust-app-lto.log', lto)
            if not ran.ok:
                r.fail('rust-app/lto', ran.why(f'{self.arch}-breenix-cargo build --release with lto = "fat"'))
            else:
                r.set('rust-app/lto', program is not None and program.is_file()
                      and elf_machine(program.read_bytes()) == ELF_MACHINE[self.arch],
                      f'cargo reported no {self.arch} executable for the LTO build')
        else:
            r.fail('rust-app/lto', 'the Rust std app did not build, so its LTO build was not tried')
        if built:
            found, names = self.outside_tree(built)
            r.set('rust-app/outside-tree', not found, 'the build holds ' + describe_found(found, names))
        else:
            r.fail('rust-app/outside-tree', 'neither Rust app built')

    def build_other_arch(self):
        """The other architecture's hello, which the disk tool must refuse to install."""
        if not self.sdk_built():
            return
        scratch = self.work / 'other-arch'
        scratch.mkdir()
        (scratch / 'hello.c').write_text('#include <stdio.h>\nint main(void) { puts("wrong arch"); return 0; }\n')
        if self.compile(self.other, ['-o', 'hello', 'hello.c'], scratch, self.logs / 'other-arch.log').ok:
            self.apps['other-arch'] = scratch / 'hello'

    def build_templates(self):
        r = self.records
        template_cases = [c for c in HOST_CASES if c.startswith('templates/') and c not in STAGE_CASES
                          and not c.startswith('templates/quick-start') and c != 'templates/sdk-docs']
        if not self.sdk_built():
            for case in template_cases:
                r.fail(case, 'the SDK was not built: ' + r.data['toolchain/clean-build'][5:])
            return
        new = self.sdk / 'bin' / 'breenix-new'
        env = sdk_env(self.sdk, self.home)
        log = self.logs / 'templates.log'
        if not new.is_file():
            for case in template_cases:
                r.fail(case, 'the SDK has no bin/breenix-new')
            return
        listing = run([new, '--list'], self.home, env, TIMEOUT_TOOL, log)
        names = listing.out.split() if listing.ok else []
        missing = [t for t in REQUIRED_TEMPLATES if t not in names]
        listing_why = listing.why('breenix-new --list') if not listing.ok else f'breenix-new --list does not name {", ".join(missing)}'
        r.set('templates/tool', listing.ok and not missing, listing_why)
        projects = self.work / 'templates'
        projects.mkdir()
        everyone, made = [], []
        for name, slug in REQUIRED_TEMPLATES.items():
            if name not in names:
                r.fail(f'templates/{slug}-create', f'breenix-new --list does not name the {name} template')
                r.fail(f'templates/{slug}-build', f'breenix-new --list does not name the {name} template')
        for name in names:
            project = f'tmpl-{name}'
            slug = REQUIRED_TEMPLATES.get(name)
            created = run([new, name, project], projects, env, TIMEOUT_TOOL, log)
            if slug:
                r.set(f'templates/{slug}-create', created.ok and (projects / project).is_dir(),
                      created.why(f'breenix-new {name} {project}') if not created.ok else 'breenix-new made no project directory')
            if not created.ok or not (projects / project).is_dir():
                everyone.append(f'{name} was not created')
                if slug:
                    r.fail(f'templates/{slug}-build', f'the {name} template made no project')
                continue
            made.append(projects / project)
            build = run([self.sdk / 'bin' / 'breenix-build', '--arch', self.arch], projects / project, env, TIMEOUT_BUILD, log)
            lines = build.out.strip().splitlines()
            program = Path(lines[-1].strip()) if build.ok and lines else None
            if program is not None and not program.is_absolute():
                program = projects / project / program
            if not build.ok:
                why = build.why(f'breenix-build --arch {self.arch}')
            elif program is None or not program.is_file() or elf_machine(program.read_bytes()) != ELF_MACHINE[self.arch]:
                why = 'breenix-build did not print the path of an ELF program for this architecture as its last line'
            else:
                why = None
                self.stage_app(f'templates/{project}', program)
            if slug:
                r.set(f'templates/{slug}-build', why is None, why)
            if why:
                everyone.append(f'{name}: {why}')
        if not listing.ok or missing:
            r.fail('templates/every-template', listing_why)
        else:
            r.set('templates/every-template', not everyone, '; '.join(everyone))
        (self.stage / 'templates').write_text(''.join(f'tmpl-{n}\n' for n in names))
        if made:
            found, leak_names = self.outside_tree(made)
            r.set('templates/outside-tree', not found, 'the projects hold ' + describe_found(found, leak_names))
        else:
            r.fail('templates/outside-tree', 'no template made a project')
        self.no_clobber(new, env, log)

    def no_clobber(self, new, env, log):
        target = self.work / 'occupied'
        target.mkdir()
        (target / 'keep.txt').write_text('mine\n')
        if not new.is_file():
            self.records.fail('templates/no-clobber', 'the SDK has no bin/breenix-new')
            return
        ran = run([new, 'c', target], self.home, env, TIMEOUT_TOOL, log)
        unchanged = [p.name for p in target.iterdir()] == ['keep.txt'] and (target / 'keep.txt').read_text() == 'mine\n'
        self.records.set('templates/no-clobber', ran.code != 0 and unchanged,
                         'breenix-new c into a directory holding a file exited 0' if ran.code == 0 else
                         'breenix-new c changed a directory that already held a file')

    def quick_start_doc(self):
        r = self.records
        doc = ROOT / 'docs' / 'sdk' / 'quick-start.md'
        shown = subprocess.run(['git', '-C', str(ROOT), 'show', f'{self.commit}:docs/sdk/quick-start.md'],
                               capture_output=True)
        if shown.returncode != 0:
            r.fail('templates/quick-start-doc', 'the commit has no docs/sdk/quick-start.md')
            r.fail('templates/sdk-docs', 'the commit has no docs/sdk/quick-start.md to ship')
            return
        text = shown.stdout.decode(errors='replace')
        shell, run_lines, outputs = parse_quick_start(text)
        problems = []
        if not shell:
            problems.append('no ```sh quick-start block')
        if len(run_lines) != 1 or len(run_lines[0].splitlines()) != 1:
            problems.append('not exactly one one-line ```breenix quick-start-run block')
        if len(outputs) != 1:
            problems.append('not exactly one ```text quick-start-output block')
        r.set('templates/quick-start-doc', not problems, f'{doc.relative_to(ROOT)} has ' + ', '.join(problems))
        if not problems:
            (self.stage / 'quick-start').mkdir()
            (self.stage / 'quick-start' / 'run').write_text(run_lines[0] + '\n')
            (self.stage / 'quick-start' / 'expected').write_text(outputs[0])
            (self.cache / 'quick-start.sh').write_text('\n'.join(shell))
        shipped = self.sdk / 'share' / 'doc' / 'breenix-sdk' / 'quick-start.md'
        if not self.sdk_built():
            r.fail('templates/sdk-docs', 'the SDK was not built: ' + r.data['toolchain/clean-build'][5:])
        else:
            r.set('templates/sdk-docs', shipped.is_file() and shipped.read_bytes() == shown.stdout,
                  'share/doc/breenix-sdk/quick-start.md is missing or differs from docs/sdk/quick-start.md')

    # --------------------------------------------------------------- stage phase

    def stage_image(self, image):
        debugfs, e2fsck = find_e2fsprogs('debugfs'), find_e2fsprogs('e2fsck')
        if not debugfs or not e2fsck:
            sys.exit('sdk-suite: debugfs and e2fsck (e2fsprogs) are needed to stage the disk image')
        image = Path(image).resolve()
        present = subprocess.run([debugfs, '-R', f'stat {GUEST_DIR}', str(image)], capture_output=True, text=True)
        if 'File not found' not in present.stdout + present.stderr:
            sys.exit(f'sdk-suite: {image.name} already holds {GUEST_DIR}; stage a fresh copy of the disk')
        self.home = self.work / 'home'
        self.home.mkdir(parents=True, exist_ok=True)
        clean_before = run([e2fsck, '-fn', image], self.home, dict(os.environ), TIMEOUT_TOOL)
        (self.stage / 'run').write_text(f'commit {self.commit}\narch {self.arch}\n')
        self.write_tree(debugfs, image, self.stage, GUEST_DIR)
        self.step(lambda: self.install(image), *sorted(c for c in STAGE_CASES if c.startswith('install/') and c != 'install/fsck'))
        self.step(lambda: self.quick_start(image), 'templates/quick-start-host')
        after = run([e2fsck, '-fn', image], self.home, dict(os.environ), TIMEOUT_TOOL, self.logs / 'fsck.log')
        if not self.sdk_built():
            self.records.fail('install/fsck', 'the SDK was not built, so its disk tool wrote nothing to check')
        elif not clean_before.ok:
            self.records.fail('install/fsck', 'e2fsck -fn found errors on the disk before the SDK\'s tool touched it')
        else:
            found = [l.strip() for l in after.out.splitlines() if l.strip() and not l.startswith(('e2fsck ', 'Pass '))]
            self.records.set('install/fsck', after.ok, f'e2fsck -fn exited {after.code} after the installs: ' + ' | '.join(found[:3]))
        records = self.work / 'records'
        if records.exists():
            shutil.rmtree(records)
        for case in HOST_CASES:
            path = records / case
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(self.records.data.get(case, 'FAIL the host harness did not reach this step') + '\n')
        self.write_tree(debugfs, image, records, GUEST_DIR + '/host')
        back = subprocess.run([debugfs, '-R', f'cat {GUEST_DIR}/run', str(image)], capture_output=True, text=True)
        if back.stdout != (self.stage / 'run').read_text():
            sys.exit(f'sdk-suite: {GUEST_DIR}/run did not read back from {image.name}')
        passed = sum(1 for v in self.records.data.values() if v == 'PASS')
        say(f'staged {len(HOST_CASES)} host records ({passed} PASS) and {len(self.apps)} programs onto {image.name}')

    def write_tree(self, debugfs, image, source, guest):
        """Copy source's tree to guest on the image, owned by root, as one debugfs batch."""
        # debugfs's mkdir of a directory that already exists leaks an inode, leaving the
        # disk inconsistent, so only missing directories are made.
        commands = []
        parts = guest.strip('/').split('/')
        for i in range(1, len(parts) + 1):
            directory = '/' + '/'.join(parts[:i])
            found = subprocess.run([debugfs, '-R', f'stat {directory}', str(image)], capture_output=True, text=True)
            if 'File not found' in found.stdout + found.stderr:
                commands += [f'mkdir {directory}', f'sif {directory} mode 040755', f'sif {directory} uid 0', f'sif {directory} gid 0']
        for path in sorted(source.rglob('*')):
            target = f'{guest}/{path.relative_to(source)}'
            if path.is_dir():
                commands += [f'mkdir {target}', f'sif {target} mode 040755']
            else:
                mode = '0100755' if os.access(path, os.X_OK) else '0100644'
                commands += [f'write {path} {target}', f'sif {target} mode {mode}']
            commands += [f'sif {target} uid 0', f'sif {target} gid 0']
        script = self.work / 'debugfs-commands'
        script.write_text('\n'.join(commands) + '\n')
        subprocess.run([debugfs, '-w', '-f', str(script), str(image)], capture_output=True)

    def image_tool(self, image, *args):
        tool = self.sdk / 'bin' / 'breenix-image'
        return run([tool, *args[:1], '--image', image, *args[1:]], self.home, sdk_env(self.sdk, self.home),
                   TIMEOUT_TOOL, self.logs / 'install.log')

    def install(self, image):
        r = self.records
        tool = self.sdk / 'bin' / 'breenix-image'
        if not self.sdk_built() or not tool.is_file():
            why = ('the SDK was not built: ' + r.data['toolchain/clean-build'][5:]) if not self.sdk_built() else 'the SDK has no bin/breenix-image'
            for case in HOST_CASES:
                if case.startswith('install/') and case != 'install/fsck':
                    r.fail(case, why)
            return
        say('installing apps onto the image with the SDK\'s breenix-image')
        version = run([tool, '--version'], self.home, sdk_env(self.sdk, self.home), TIMEOUT_TOOL)
        r.set('install/tool', version.ok and self.commit in version.out,
              version.why('breenix-image --version') if not version.ok else 'breenix-image --version does not name the commit')
        c_app, rust_app, nostd = (self.apps.get(n) for n in ('c-app', 'rust-app', 'rust-nostd-app'))
        steps = [('sdk-c-app', c_app, []), ('sdk-rust-app', rust_app, []), ('sdk-nostd-app', nostd, []),
                 ('sdk-private', c_app, ['--mode', '0750']), ('sdk-replaced', nostd, []), ('sdk-replaced', c_app, []),
                 ('sdk-removed', c_app, [])]
        failures = []
        for name, program, extra in steps:
            if program is None:
                failures.append(f'{name}: its program was not built')
                continue
            ran = self.image_tool(image, 'install', *extra, '--name', name, program)
            if not ran.ok:
                failures.append(f'{name}: ' + ran.why('breenix-image install'))
        r.set('install/installed', not failures, '; '.join(failures))
        removed = self.image_tool(image, 'remove', 'sdk-removed')
        r.set('install/removed', removed.ok, removed.why('breenix-image remove sdk-removed'))
        listing = self.image_tool(image, 'list')
        expected = sorted(INSTALLED)
        r.set('install/list', listing.ok and listing.out.split() == expected,
              listing.why('breenix-image list') if not listing.ok else f'breenix-image list printed {listing.out.split()}, expected {expected}')
        wrong = self.apps.get('other-arch')
        if wrong is None:
            r.fail('install/arch-refused', f'the SDK could not build an {self.other} program to offer it')
        else:
            refused = self.image_tool(image, 'install', '--name', 'sdk-wrong-arch', wrong)
            after = self.image_tool(image, 'list')
            r.set('install/arch-refused', refused.code != 0 and 'sdk-wrong-arch' not in after.out.split(),
                  f'breenix-image installed an {self.other} program onto an {self.arch} disk' if refused.code == 0
                  else 'the refused install still left sdk-wrong-arch on the disk')
        missing = self.image_tool(image, 'remove', 'sdk-never-installed')
        r.set('install/remove-missing', missing.code == 1 and not missing.timed_out
              and 'breenix-image: program is not installed: sdk-never-installed' in missing.err
              and 'Traceback' not in missing.err, missing.why('remove of an unregistered name'))

    def quick_start(self, image):
        r = self.records
        script = self.cache / 'quick-start.sh'
        if not r.ok('templates/quick-start-doc'):
            r.fail('templates/quick-start-host', 'the quick start could not be read: ' + r.data['templates/quick-start-doc'][5:])
            return
        if not self.sdk_built():
            r.fail('templates/quick-start-host', 'the SDK was not built: ' + r.data['toolchain/clean-build'][5:])
            return
        say('following the docs\' quick start literally')
        directory = self.work / 'quick-start'
        home = self.work / 'quick-start-home'
        for d in (directory, home):
            shutil.rmtree(d, ignore_errors=True)
            d.mkdir()
        env = sdk_env(None, home, {'BREENIX_SDK': str(self.sdk), 'BREENIX_IMAGE': str(image), 'BREENIX_ARCH': self.arch})
        ran = run(['bash', '-euo', 'pipefail', script], directory, env, TIMEOUT_BUILD, self.logs / 'quick-start.log')
        r.set('templates/quick-start-host', ran.ok, ran.why('the quick start\'s commands'))


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--arch', required=True, choices=ARCHES)
    parser.add_argument('--image', help='an ext2 disk image (a copy) to stage the results onto')
    parser.add_argument('--rebuild', action='store_true', help='discard a cached build of this commit')
    args = parser.parse_args()
    # The builds share the Mac with its user: run this script's whole process tree at
    # background priority. The boot that follows it is a separate process and keeps its own.
    if sys.platform == 'darwin' and os.environ.get('BREENIX_SDK_SUITE_BACKGROUND') != '1' and shutil.which('taskpolicy'):
        os.environ['BREENIX_SDK_SUITE_BACKGROUND'] = '1'
        os.execvp('taskpolicy', ['taskpolicy', '-b', sys.executable, *sys.argv])
    harness = Harness(args.arch, args.rebuild)
    harness.build()
    if args.image:
        harness.stage_image(args.image)
    else:
        passed = sum(1 for case, v in harness.records.data.items() if v == 'PASS')
        say(f'built: {passed} of {len(harness.records.data)} build-time host records PASS (stage with --image)')


if __name__ == '__main__':
    main()
