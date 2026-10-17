"""Write boot-specific files onto a staged ext2 RAM disk and verify their bytes."""
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


def stage_root(image, mode, suite, site):
    if mode not in ('default', 'shell', 'suite'):
        raise ValueError('unsupported RAM-disk boot mode')
    if mode == 'suite' and not re.fullmatch(r'[a-z0-9]+(?:-[a-z0-9]+)*', suite or ''):
        raise ValueError('suite mode needs a suite id')
    if mode != 'suite' and suite:
        raise ValueError('a suite needs suite mode')
    target = f'suite {suite}' if mode == 'suite' else mode
    # Only explicitly guest-designated values cross from the host's site file.
    lines = []
    for key, value in sorted(site.items()):
        if not key.startswith('GUEST_'):
            continue
        if (not re.fullmatch(r'GUEST_[A-Z][A-Z0-9_]*', key)
                or re.search(r'PASS|SECRET|TOKEN|CREDENTIAL|PRIVATE_KEY', key)
                or any(char in value for char in '\r\n\x00')):
            raise ValueError('invalid guest site field')
        lines.append(key.removeprefix('GUEST_') + '=' + value + '\n')
    debugfs = shutil.which('debugfs')
    if not debugfs:
        candidate = Path('/opt/homebrew/opt/e2fsprogs/sbin/debugfs')
        if candidate.is_file():
            debugfs = str(candidate)
    if not debugfs:
        raise ValueError('staging needs debugfs')
    image = Path(image).resolve()
    with tempfile.TemporaryDirectory(prefix='metal-root-') as scratch:
        scratch = Path(scratch)
        files = {'boot-target': target + '\n', 'site.env': ''.join(lines)}
        commands = ['mkdir /etc/breenix', 'rm /etc/breenix/suite-sequence']
        for name, text in files.items():
            source = scratch / name
            source.write_text(text)
            commands.extend([f'rm /etc/breenix/{name}', f'write {source} /etc/breenix/{name}'])
        script = scratch / 'commands'
        script.write_text('\n'.join(commands) + '\n')
        subprocess.run([debugfs, '-w', '-f', str(script), str(image)], check=True, capture_output=True)
        for name, text in files.items():
            result = subprocess.run([debugfs, '-R', f'cat /etc/breenix/{name}', str(image)],
                                    check=True, capture_output=True)
            if result.stdout != text.encode():
                raise ValueError('RAM-disk boot configuration readback differs')
        binary = f'/sbin/suite-{suite}' if mode == 'suite' else '/sbin/init'
        installed = scratch / 'pid1'
        subprocess.run([debugfs, '-R', f'dump {binary} {installed}', str(image)],
                       check=True, capture_output=True)
        if not installed.is_file() or installed.read_bytes()[:4] != b'\x7fELF':
            raise ValueError('RAM disk has no selected PID 1 ELF')
    return {'mode': mode, 'suite': suite or None, 'siteFields': len(lines)}
