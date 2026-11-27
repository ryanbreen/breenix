#!/usr/bin/env python3
"""Exercise the scalar string routines and check their generated x86 code."""
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
STRINGS = ROOT / "libs/libbreenix-libc/src/string"


def isolate(source):
    # Keep the host libc's symbols out of the test entry points.
    return re.sub(r'#\[no_mangle\]\s+pub unsafe extern "C" fn (\w+)',
                  r'#[export_name = "breenix_\1"] pub unsafe extern "C" fn \1', source)


with tempfile.TemporaryDirectory(prefix="breenix-string-check-") as temporary:
    directory = Path(temporary)
    source = (STRINGS / "mod.rs").read_text()
    start = source.index("use core::ptr::null_mut;")
    end = source.index('#[no_mangle]\npub unsafe extern "C" fn memrchr(')
    length = source.index('#[no_mangle]\npub unsafe extern "C" fn strlen(')
    finish = source.index("#[no_mangle]", length + len("#[no_mangle]"))
    (directory / "words.rs").write_text(isolate(source[start:end] + source[length:finish]))
    (directory / "mem_asm.rs").write_text(isolate((STRINGS / "mem_asm.rs").read_text()))
    harness = ROOT / "tests/libc-string-words-cases.rs"
    local = directory / "harness.rs"
    local.write_text(harness.read_text())
    binary = directory / "check"
    command = ["rustc", "--edition=2021", "--test", "-Dwarnings", "-Copt-level=3", str(local)]
    subprocess.run([*command, "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True, timeout=30)

    # The old routines return the same values. Inspect the generated code so
    # the red run detects byte loops and unconditional REP without timing.
    triple = subprocess.check_output(["rustc", "-vV"], text=True).split("host: ")[1].split()[0]
    target = "x86_64" + triple[triple.index("-"):]
    output = directory / "check.s"
    subprocess.run([*command, "--target", target, "--emit=asm", "-o", str(output)], check=True)
    assembly = output.read_text()

    def instructions(name):
        alias = re.search(rf"^\.set[ \t]+_?breenix_{name},[ \t]+_?breenix_(\w+)$",
                          assembly, flags=re.M)
        if alias:
            name = alias[1]
        label = re.search(rf"^_?breenix_{name}:[ \t]*(?:[#/].*)?$", assembly, flags=re.M)
        assert label, name
        return re.split(r"\.(?:cfi_endproc|section|globl)", assembly[label.end():], 1)[0]

    errors = []
    for name in ("memcmp", "memchr", "strlen"):
        if not re.search(r"\bmovq\s+[^\n,]*\([^\n]*\),\s*%r", instructions(name)):
            errors.append(f"{name} has no word load")
    copy = instructions("memcpy")
    if not re.search(r"\bj(?:b|be|ae|a|l|le|g|ge|e|ne)\b", copy):
        errors.append("memcpy uses REP for every size")
    if not re.search(r"\brep\s+movsb\b", copy):
        errors.append("memcpy has no REP bulk path")
    assert not errors, ", ".join(errors)
