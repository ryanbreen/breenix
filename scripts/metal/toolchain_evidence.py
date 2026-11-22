"""Bind the guest's sysroot result to the host's retained image build record."""
import re


def build_identity(record, commit, arch):
    if (not re.fullmatch(r"[0-9a-f]{40}", commit or "") or not isinstance(record, dict)
            or record.get("commit") != commit or record.get("arch") != arch
            or not re.fullmatch(r"[0-9a-f]{64}", record.get("manifestSha256") or "")):
        raise ValueError("toolchain image build record differs from the requested commit or target")
    return record["manifestSha256"]


def verify_sysroot(serial, record, commit, arch):
    digest = build_identity(record, commit, arch)
    expected = f"TOOLCHAIN SYSROOT commit={commit} arch={arch} sha256={digest}"
    lines = [line.rstrip("\r") for line in serial.split("\n")[:-1]]
    starts = [index for index, line in enumerate(lines) if line.startswith("SUITE toolchain START ")]
    rows = [(index, line) for index, line in enumerate(lines) if line.startswith("TOOLCHAIN SYSROOT ")]
    passed = [index for index, line in enumerate(lines)
              if line.startswith("SUITE toolchain CASE inputs/sysroot-manifest PASS ")]
    if (len(starts) != 1 or len(rows) != 1 or len(passed) != 1
            or not starts[0] < rows[0][0] < passed[0] or rows[0][1] != expected):
        raise ValueError("guest sysroot digest differs from the retained toolchain image build record")
