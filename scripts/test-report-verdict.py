"""Validate a testing-kernel report against the boot commit's test contracts."""
from pathlib import Path
import re
import runpy


def completion(serial, root):
    root = Path(root)
    # x86 logs report records through the kernel logger; ARM prints them directly.
    lines = [re.sub(r"^\[[^\]]+\]\s+[\w:]+:\s*", "", line.rstrip("\r"))
             for line in serial.split("\n")[:-1]]
    if not any(line == "USERSPACE TEST REPORT DONE" for line in lines):
        return None
    tally = [line for line in lines if line.startswith("TEST_TALLY: ")]
    # Child programs deliberately test signal deaths and nonzero exits;
    # their supervising canonical tests must succeed after verifying them.
    canonical = set(re.findall(r'^    "([^"]+)",',
                     (root / "kernel/src/boot/test_list.rs").read_text(), re.MULTILINE))
    for line in lines:
        if re.search(r"\bSKIP\b", line):
            return False, f"skipped test: {line}"
    loaded_lines = [line for line in lines if line.startswith("[test] Loaded ")
                    and "test binaries" in line]
    if len(loaded_lines) != 1:
        return False, "expected exactly one test-binary loader summary"
    loaded = re.fullmatch(r"\[test\] Loaded (\d+)/(\d+) test binaries \((\d+) failed, (\d+) not found\)", loaded_lines[0])
    if not loaded or tuple(map(int, loaded.groups())) != (len(canonical), len(canonical), 0, 0):
        return False, f"unloaded test programs: {loaded_lines[0]}"
    for line in tally:
        failures = re.search(r"failed=\[(.*?)\]", line)
        if failures:
            for failure in failures[1].split(","):
                name, _, status = failure.rpartition(":")
                if name in canonical and status != "0":
                    return False, f"canonical test failed: {failure}"
    try:
        classify = runpy.run_path(str(root / "scripts/x86-gate-exits.py"))["classify_serial"]
        if not classify(serial, root / "scripts/x86-gate-allowlist.txt"):
            return False, "unexpected process exit"
    except (OSError, ValueError) as error:
        return False, f"incomplete exit accounting: {error}"
    return True, tally[0]
