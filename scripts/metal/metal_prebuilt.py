"""Portable payload inventory. Verification completes before service staging begins."""
import hashlib
import fcntl
import contextlib
import json
import os
from pathlib import Path
import re


def facts(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        while chunk := stream.read(1 << 20):
            digest.update(chunk)
    return dict(size=path.stat().st_size, sha256=digest.hexdigest())


def inventory(root, staged=False):
    result = {}
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise ValueError('artifact symlink refused: ' + str(path.relative_to(root)))
        if path.is_file() and path != root / 'manifest.json' and not (staged and path == root / 'ready'):
            result[str(path.relative_to(root))] = facts(path)
    return result


def create(root, tools_commit, mode, toolchain, rust_pin, policy_commit):
    build = json.loads((root / 'build.json').read_text())
    manifest = dict(version=1, builder='mac', builderHost='apple-silicon-macos', commit=build['commit'],
                    toolsCommit=tools_commit, mode=mode, profile=build['profile'], features=build['features'],
                    suite=build.get('suite'), toolchain=toolchain, rustPin=rust_pin,
                    policyCommit=policy_commit, catchup=bool(build.get('catchup', False)), artifacts=inventory(root))
    (root / 'manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
    return manifest


def verify(root, *, commit, tools_commit, mode, features, suite, staged=False):
    root = Path(root)
    # The upload receiver holds LOCK_EX while pruning. Refresh
    # last-use time before releasing it, protecting queued deliveries/retries.
    with contextlib.ExitStack() as stack:
        if root.parent.name == 'prebuilt':
            lock = stack.enter_context((root.parent / '.upload.lock').open('a'))
            fcntl.flock(lock, fcntl.LOCK_SH)
        return _verify(root, commit=commit, tools_commit=tools_commit, mode=mode,
                       features=features, suite=suite, staged=staged)


def _verify(root, *, commit, tools_commit, mode, features, suite, staged=False):
    root = Path(root)
    manifest = json.loads((root / 'manifest.json').read_text())
    if manifest.get('version') != 1 or manifest.get('builder') != 'mac':
        raise ValueError('unsupported prebuilt manifest')
    for field, expected in dict(commit=commit, toolsCommit=tools_commit, mode=mode,
                                features=features).items():
        if manifest.get(field) != expected:
            raise ValueError(f'prebuilt {field} mismatch: expected {expected!r}, got {manifest.get(field)!r}')
    # Retries reuse the same payload, but run only suites without completed
    # evidence. They must remain an ordered selection of the built suites.
    built = (manifest.get('suite') or '').split(',') if manifest.get('suite') else []
    requested = suite.split(',') if suite else []
    remaining = iter(built)
    if bool(built) != bool(requested) or len(set(requested)) != len(requested) or any(
            name not in remaining for name in requested):
        raise ValueError(f'prebuilt suite mismatch: requested {suite!r}, built {manifest.get("suite")!r}')
    for field in ('commit', 'toolsCommit', 'policyCommit'):
        if not re.fullmatch('[0-9a-f]{40}', manifest.get(field, '')):
            raise ValueError('invalid prebuilt ' + field)
    if not manifest.get('toolchain') or not manifest.get('rustPin') or not manifest.get('builderHost'):
        raise ValueError('missing prebuilt compiler/builder identity')
    build = json.loads((root / 'build.json').read_text())
    required = {'build.json', 'uefi.img', 'ext2.img', 'stub.efi', 'manager.efi', 'sha512-crypt'}
    required.update('definition/' + name for name in build['definition'])
    if mode == 'tests' or 'testing' in ','.join(features).split(','):
        required.add('test-binaries.img')
    listed = manifest.get('artifacts', {})
    for name in required:
        if name not in listed or not (root / name).is_file():
            raise ValueError('missing prebuilt artifact: ' + name)
    for name, expected in listed.items():
        if Path(name).is_absolute() or '..' in Path(name).parts or not (root / name).is_file():
            raise ValueError('missing or invalid prebuilt artifact: ' + name)
        if (root / name).is_symlink() or facts(root / name) != expected:
            raise ValueError('prebuilt artifact hash/size mismatch: ' + name)
    if inventory(root, staged) != listed:
        raise ValueError('prebuilt artifact inventory mismatch')
    for field in ('commit', 'profile', 'features', 'suite'):
        if build.get(field) != manifest.get(field):
            raise ValueError('prebuilt build record mismatch: ' + field)
    # Retention follows recent use, including detached suite retries.
    os.utime(root, None)
    return manifest


def record_builder(run, manifest=None):
    """Host-built records retain their existing schema and bytes."""
    if manifest is not None:
        run['builder'] = manifest['builder']
        run['prebuilt'] = dict(commit=manifest['commit'], toolsCommit=manifest['toolsCommit'],
                              policyCommit=manifest['policyCommit'])
    return run
