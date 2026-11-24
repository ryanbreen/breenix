"""Check the K1 symbol inventory against sealed reference and runtime artifacts."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
DEV = Path("opt/breenix-dev/25.2")
CORPUS = Path("usr/share/breenix/toolchain")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def symbols(nm, path, *selection):
    output = subprocess.check_output(
        [nm, *selection, "--format=posix", path], text=True
    )
    return sorted({
        row.split()[0] for row in output.splitlines()
        if row.strip() and not row.endswith(":")
    })


def compiler_builtins_cover_x_inventory(arch, bundle, libc, builtins, nm):
    inventory = json.loads((HERE / f"25.2-{arch}.json").read_text())
    inputs = (ROOT / "toolchain/corpus/cases").read_text().splitlines()
    objects = inventory["objects"]
    assert sorted(objects) == sorted(inputs), "inventory differs from the 25.2 corpus"

    root = bundle / "root"
    manifest = root / DEV / "manifest.sha256"
    build = json.loads((bundle / "build.json").read_text())
    assert build["arch"] == arch, "wrong bundle architecture"
    assert build["manifestSha256"] == sha256(manifest), "bundle manifest digest mismatch"
    sealed = {path: digest for digest, path in
              (line.split("  ", 1) for line in manifest.read_text().splitlines())}

    required = set()
    for name, (digest, expected) in objects.items():
        obj = CORPUS / "objs" / f"{name}.o"
        receipt = CORPUS / f"{name}.inventory.json"
        assert sealed[str(receipt)] == sha256(root / receipt), f"{name}: unsealed receipt"
        metadata = json.loads((root / receipt).read_text())
        assert metadata["arch"] == arch, f"{name}: wrong receipt target"
        assert {
            tool: record["sha256"] for tool, record in metadata["producer"].items()
        } == inventory["producerSha256"], f"{name}: wrong sealed tool pins"
        assert (sha256(root / obj) == digest == sealed[str(obj)]
                == metadata["object"]["sha256"]), f"{name}: reference object digest mismatch"
        undefined = symbols(nm, root / obj, "-u")
        assert undefined == expected, f"{name}: expected {expected}, got {undefined}"
        required.update(undefined)

    libc_exports = symbols(nm, libc, "--defined-only", "--extern-only")
    builtins_exports = symbols(nm, builtins, "--defined-only", "--extern-only")
    assert libc_exports and builtins_exports, "empty runtime export set"
    needs_builtins = required.difference(libc_exports)
    missing = needs_builtins.difference(builtins_exports)
    assert not missing, f"{arch}: missing runtime symbols: {', '.join(sorted(missing))}"
    print(f"libc_host::compiler_builtins_cover_x_inventory {arch}: "
          f"{len(objects)} objects, {len(required)} undefined symbols, "
          f"{len(needs_builtins)} supplied by compiler_builtins")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("arch", choices=("x86_64", "aarch64"))
    parser.add_argument("bundle", type=Path)
    parser.add_argument("libc", type=Path)
    parser.add_argument("builtins", type=Path)
    parser.add_argument("--llvm-nm", type=Path, required=True)
    args = parser.parse_args()
    compiler_builtins_cover_x_inventory(
        args.arch, args.bundle, args.libc, args.builtins, args.llvm_nm
    )
