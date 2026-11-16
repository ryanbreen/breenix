"""Verify the converted root against an ext2 image with varied metadata."""
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, errors='surrogateescape',
                          capture_output=True, **kwargs)


def root_image_matches_the_ext2_image():
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        tree = tmp / 'tree'
        tree.mkdir()
        (tree / 'owned').mkdir()
        (tree / 'setid').write_bytes(b'root image content\n')
        (tree / 'setid').chmod(0o4755)
        os.link(tree / 'setid', tree / 'alias')
        os.symlink('setid', tree / 'sym')
        image, result = tmp / 'ext2.img', tmp / 'breenfs.img'
        with image.open('wb') as f:
            f.truncate(64 * 1024 * 1024)
        run('mke2fs', '-q', '-t', 'ext2', '-F', '-d', str(tree), str(image))
        owners = tmp / 'owners'
        owners.write_text(f'2000:2000\t{tree / "owned"}\n')
        run('python3', str(ROOT / 'scripts/ext2-owners.py'), str(image), str(owners), str(tree))
        # The production image carries a non-UTF-8 helper alias. Create it in
        # the image directly: macOS cannot materialize this POSIX filename.
        subprocess.run(['debugfs', '-w', '-f', '-', str(image)],
                       input=b'ln /setid /raw-\xff\nset_inode_field /setid links_count 3\n',
                       check=True, capture_output=True)
        run('bash', str(ROOT / 'scripts/create-breenfs-root.sh'), str(image), str(result))
        extract = ROOT / 'tools/breenfs/target/debug/breenfs-extract'
        lines = run(str(extract), str(result), 'manifest').stdout.splitlines()
        rows = {bytes.fromhex(c[0]).decode(errors='surrogateescape'): c
                for c in (line.split('\t') for line in lines)}
        digest = hashlib.sha256(b'root image content\n').hexdigest()
        group = b'/alias'.hex()
        listing = run('debugfs', '-R', 'ls -p -l /', str(image)).stdout
        sym_mode = next(line.split('/')[2] for line in listing.splitlines()
                        if line.split('/')[5:6] == ['sym'])
        expected = {
            '/': ('dir', '040755', '0', '0', '6', '-', '-'),
            '/lost+found': ('dir', '040700', '0', '0', '0', '-', '-'),
            '/owned': ('dir', '040755', '2000', '2000', '0', '-', '-'),
            '/alias': ('reg', '104755', '0', '0', '19', digest, group),
            '/setid': ('reg', '104755', '0', '0', '19', digest, group),
            '/raw-\udcff': ('reg', '104755', '0', '0', '19', digest, group),
            '/sym': ('symlink', sym_mode, '0', '0', '5',
                     hashlib.sha256(b'setid').hexdigest(), '-'),
        }
        actual = {path: tuple(row[1:6] + row[9:11]) for path, row in rows.items()}
        assert actual == expected



def test_root_image_matches_the_ext2_image():
    root_image_matches_the_ext2_image()
