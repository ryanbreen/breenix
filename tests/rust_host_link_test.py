#!/usr/bin/env python3
"""Inspect a built hosted link probe, or compare ordinary release builds.

Before hosted_link_layout, build the probe for the same architecture with:
    userspace/programs/build.sh --arch ARCH --hosted-bin hosted_link_probe \
        --features hosted-link-probe

Run build.sh for each architecture at the base and use record_baseline to
save its digests. Retain the base release directories. Compare the ordinary
candidate builds with existing_links_load_identical.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PROGRAMS = ROOT / "userspace/programs"
ARCHES = ("x86_64", "aarch64")
PROBE = "hosted_link_probe"


def require(condition, message):
    if not condition:
        raise SystemExit(message)


def hosted_link_layout(arch, readelf, elf=None):
    if elf is None:
        elf = PROGRAMS / "target/hosted" / f"{arch}-breenix-hosted/release" / PROBE

    [report] = json.loads(subprocess.check_output([
        readelf, "--elf-output-style=JSON", "--file-header", "--sections", "--program-headers",
        "--symbols", elf,
    ]))
    # ELF gABI §2.1 e_machine and Appendix A: EM_X86_64 = 62, EM_AARCH64 = 183.
    require(report["ElfHeader"]["Machine"]["Value"] == {"x86_64": 62, "aarch64": 183}[arch],
            f"{elf}: expected an ELF for {arch}")
    sections = {row["Section"]["Name"]["Name"]: row["Section"] for row in report["Sections"]}
    headers = [row["ProgramHeader"] for row in report["ProgramHeaders"]]
    symbols = {row["Symbol"]["Name"]["Name"]: row["Symbol"] for row in report["Symbols"]}

    def unique_header(kind, citation):
        found = [header for header in headers if header["Type"]["Value"] == kind]
        require(len(found) == 1, f"{citation}: expected one header of type {kind:#x}")
        return found[0]

    def load_contains(section, flags):
        # ELF gABI §§7.1–7.2: PT_LOAD maps its file bytes starting at p_vaddr.
        # Tables 7.2–7.3: PF_R = 4, PF_R|PF_W = 6.
        return any(
            load["Type"]["Value"] == 1
            and load["Flags"]["Value"] == flags
            and 0 <= section["Offset"] - load["Offset"] <= load["FileSize"] - section["Size"]
            and 0 <= section["Address"] - load["VirtualAddress"] <= load["MemSize"] - section["Size"]
            and section["Offset"] - load["Offset"] == section["Address"] - load["VirtualAddress"]
            for load in headers
        )

    # LSB Core 5.0 §11.2/Table 11-1, GNU ld 2.45 §2.1 (--eh-frame-hdr).
    eh_header = unique_header(0x6474E550, "LSB Core 5.0 §11.2 PT_GNU_EH_FRAME")
    stack = unique_header(0x6474E551, "LSB Core 5.0 §11.2 PT_GNU_STACK")
    require(stack["Flags"]["Value"] == 6,
            "LSB Core 5.0 §11.2 and ELF gABI Table 7.3: PT_GNU_STACK must be RW")

    # LSB Core 5.0 §10.3.1.2/Table 10-4: SHT_PROGBITS, SHF_ALLOC.
    for name in (".eh_frame_hdr", ".eh_frame", ".gcc_except_table"):
        section = sections.get(name)
        require(
            section and section["Size"] > 0 and section["Type"]["Value"] == 1
            and section["Flags"]["Value"] == 2 and load_contains(section, 4),
            f"LSB Core 5.0 Table 10-4 and ELF gABI Table 7.3: {name} must be in an R load",
        )

    frame_header = sections[".eh_frame_hdr"]
    require(
        (eh_header["Offset"], eh_header["VirtualAddress"], eh_header["FileSize"], eh_header["MemSize"])
        == (frame_header["Offset"], frame_header["Address"], frame_header["Size"], frame_header["Size"]),
        "LSB Core 5.0 §11.2: PT_GNU_EH_FRAME must describe exactly .eh_frame_hdr",
    )

    # ELF gABI Tables 3.2, 3.3 and 3.8: array section types and SHF_ALLOC|SHF_WRITE.
    # The fixture supplies two eight-byte entries per array.
    for phase, kind in (("preinit", 16), ("init", 14), ("fini", 15)):
        name = f".{phase}_array"
        section = sections.get(name)
        require(
            section and section["Type"]["Value"] == kind and section["Flags"]["Value"] == 3
            and section["Size"] == 16 and section["Address"] % 8 == 0 and load_contains(section, 6),
            f"ELF gABI Tables 3.2, 3.3, 3.8 and 7.3: {name} must retain both pointers in an RW load",
        )
        # GNU ld 2.45 §3.5.4 (PROVIDE_HIDDEN) and ELF gABI §5.4 (STV_HIDDEN).
        for suffix, value in (("start", section["Address"]), ("end", section["Address"] + section["Size"])):
            symbol_name = f"__{phase}_array_{suffix}"
            symbol = symbols.get(symbol_name)
            require(
                symbol and symbol["Section"]["Value"] == section["Index"]
                and symbol["Value"] == value and symbol["Other"]["Value"] & 3 == 2,
                f"GNU ld §3.5.4: {symbol_name} must be hidden and at the array {suffix}",
            )
    print(f"hosted_link_layout ({arch}): PASS")


def hosted_targets():
    for arch in ARCHES:
        source = ROOT / ("x86_64-breenix-c.json" if arch == "x86_64" else "aarch64-breenix.json")
        expected = json.loads(source.read_text())
        expected["panic-strategy"] = "unwind"
        expected["eh-frame-header"] = True
        target = ROOT / f"{arch}-breenix-hosted.json"
        require(target.exists(), f"missing hosted target {target}")
        require(json.loads(target.read_text()) == expected, f"{target}: hosted target fields differ")
    print("hosted_targets: PASS")


def hosted_unwind_link(arch, readelf, nm, elf, link_map, archive):
    hosted_link_layout(arch, readelf, elf)
    undefined = subprocess.check_output([nm, "--undefined-only", str(elf)], text=True)
    require(not any("_Unwind_" in line for line in undefined.splitlines()),
            f"{elf}: undefined unwind symbols: {undefined}")
    map_text = link_map.read_text()
    require(str(archive.resolve()) in map_text, f"{link_map}: expected archive {archive}")
    digest = hashlib.file_digest(archive.open("rb"), "sha256").hexdigest()
    require(digest in map_text, f"{link_map}: missing selected libc digest {digest}")
    print(f"hosted_unwind_link ({arch}): PASS")


def normal_digests(arch, release=None):
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=PROGRAMS,
    ))
    [package] = metadata["packages"]
    features = {"framebuffer-v1"} if arch == "x86_64" else set()
    names = sorted(
        target["name"] for target in package["targets"]
        if "bin" in target["kind"] and features.issuperset(target.get("required-features", []))
    )
    require(names, "Cargo reported no ordinary bin targets")
    if release is None:
        release = PROGRAMS / "target" / f"{arch}-breenix/release"
    result = {}
    for name in names:
        with (release / name).open("rb") as binary:
            result[name] = hashlib.file_digest(binary, "sha256").hexdigest()
    return result


def normalized_symbol_strings(data):
    # Rust v0 Symbol Format, crate-root disambiguator and vendor-specific
    # suffix. Legacy mangling terminates its path with a 17-byte hash item.
    def normalized(name):
        name = re.sub(rb'(?<=\.llvm\.)[0-9]+\Z', lambda m: b'0' * len(m[0]), name)
        if name.startswith(b'_ZN'):
            return re.sub(rb'(?<=17h)[0-9a-f]{16}(?=E(?:\.llvm\.[0-9]+)?\Z)',
                          b'0' * 16, name)
        if name.startswith(b'_R'):
            return re.sub(rb'(?<=Cs)[0-9a-zA-Z]+(?=_u?[0-9])',
                          lambda m: b'0' * len(m[0]), name)
        return name

    return b'\0'.join(normalized(name) for name in data.split(b'\0'))


def elf_differences(reference, candidate, readelf):
    def layout(path):
        [elf] = json.loads(subprocess.check_output([
            readelf, "--elf-output-style=JSON", "--file-header", "--program-headers", "--sections", path,
        ]))
        return elf["ElfHeader"], elf["ProgramHeaders"], [row["Section"] for row in elf["Sections"]]

    before, after = reference.read_bytes(), candidate.read_bytes()
    base_header, base_segments, base_sections = layout(reference)
    header, segments, sections = layout(candidate)
    differences = []
    if base_header != header:
        differences.append("ELF header or entry point")
    if base_segments != segments:
        differences.append("segment layout")
    if base_sections != sections:
        for index in range(max(len(base_sections), len(sections))):
            left = base_sections[index:index + 1]
            right = sections[index:index + 1]
            if left != right:
                names = sorted({section["Name"]["Name"] for section in left + right})
                differences.append(f"section layout [{index}] {', '.join(names)}")

    # gABI §3.2: NOBITS occupies no file bytes. Its size and address are
    # checked above. Compare every other section, including all SHF_ALLOC
    # bytes, and permit hash changes only in non-allocatable string data.
    outside_before, outside_after = bytearray(before), bytearray(after)
    for left, right in zip(base_sections, sections):
        name = right["Name"]["Name"]
        if left["Name"]["Name"] != name or left["Type"] != right["Type"]:
            continue
        if right["Type"]["Value"] in (0, 8):  # SHT_NULL and SHT_NOBITS.
            continue
        offset, end = left["Offset"], left["Offset"] + left["Size"]
        new_offset, new_end = right["Offset"], right["Offset"] + right["Size"]
        old_data, new_data = before[offset:end], after[new_offset:new_end]
        allowed_strings = (
            not (left["Flags"]["Value"] | right["Flags"]["Value"]) & 2
            and name in (".strtab", ".comment", ".debug_str", ".debug_line_str")
        )
        if old_data != new_data and not (
            allowed_strings and normalized_symbol_strings(old_data) == normalized_symbol_strings(new_data)
        ):
            differences.append(name)
        outside_before[offset:end] = bytes(len(old_data))
        outside_after[new_offset:new_end] = bytes(len(new_data))
    if not differences and outside_before != outside_after:
        differences.append("bytes outside sections")
    return differences


def existing_links_load_identical(arch, baseline, baseline_release, readelf, candidate_release):
    receipt = json.loads(baseline.read_text())
    require(receipt["arch"] == arch, f"{baseline}: expected a baseline for {arch}")
    require(os.environ.get("BREENIX_STD_METADATA_SEED") == receipt["std_metadata_seed"],
            "ordinary builds must use the base pin as the std metadata seed")
    if candidate_release is None:
        candidate_release = PROGRAMS / "target" / f"{arch}-breenix/release"
    seed_file = candidate_release / "std-metadata-seed"
    require(seed_file.read_text().strip() == receipt["std_metadata_seed"],
            "ordinary build used a different std metadata seed")
    expected = receipt["sha256"]
    actual = normal_digests(arch, candidate_release)
    require(
        expected.keys() == actual.keys(),
        f"{arch}: ordinary target set changed: missing {sorted(expected.keys() - actual.keys())}, "
        f"extra {sorted(actual.keys() - expected.keys())}",
    )
    exact = 0
    failures = []
    for name, digest in expected.items():
        if digest == actual[name]:
            exact += 1
            continue
        require(baseline_release is not None, f"{arch}/{name}: supply --baseline-release to inspect changed bytes")
        reference = baseline_release / name
        with reference.open("rb") as binary:
            require(hashlib.file_digest(binary, "sha256").hexdigest() == digest,
                    f"{reference}: does not match the original-pin receipt")
        candidate = seed_file.parent / name
        differences = elf_differences(reference, candidate, readelf)
        if differences:
            failures.append(f"{name}: {', '.join(differences)}")
    require(not failures, f"{arch}: ordinary ELF differences:\n" + "\n".join(failures))
    print(f"existing_links_load_identical ({arch}): PASS "
          f"({len(expected)} bins, {exact} byte-identical, baseline {receipt['commit']})")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("check", choices=["hosted_targets", "hosted_unwind_link", "hosted_link_layout", "existing_links_load_identical", "record_baseline"])
    parser.add_argument("--arch", choices=ARCHES, required=True)
    parser.add_argument("--readelf", default=os.environ.get("LLVM_READELF", "llvm-readelf"))
    parser.add_argument("--elf", type=Path, help="inspect an already linked probe, such as the parent control")
    parser.add_argument("--baseline", type=Path, help="path to digests captured after building the parent")
    parser.add_argument("--baseline-release", type=Path, help="original release directory for nonidentical ELFs")
    parser.add_argument("--candidate-release", type=Path, help="retained ordinary release directory")
    parser.add_argument("--base-ref", default="HEAD", help="source revision for record_baseline")
    parser.add_argument("--nm", default=os.environ.get("LLVM_NM", "llvm-nm"))
    parser.add_argument("--link-map", type=Path)
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    if args.check == "hosted_targets":
        hosted_targets()
    elif args.check == "hosted_unwind_link":
        if not all((args.elf, args.link_map, args.archive)):
            parser.error("--elf, --link-map and --archive are required")
        expected = ROOT / "libs/libbreenix-libc/target" / ("x86_64-breenix-c" if args.arch == "x86_64" else "aarch64-breenix") / "release/libc.a"
        require(args.archive.resolve() == expected.resolve(), "incorrect libc archive")
        hosted_unwind_link(args.arch, args.readelf, args.nm, args.elf, args.link_map, args.archive)
    elif args.check == "hosted_link_layout":
        hosted_link_layout(args.arch, args.readelf, args.elf)
    else:
        if args.baseline is None:
            parser.error("--baseline is required for this check")
        if args.check == "record_baseline":
            rev = subprocess.check_output(
                ["git", "rev-parse", "--verify", args.base_ref + "^{commit}"], cwd=ROOT, text=True,
            ).strip()
            # The check-first commit can supply this script on top of the base.
            subprocess.run(
                ["git", "diff", "--quiet", rev, "--", ".", ":(exclude)tests"], cwd=ROOT, check=True,
            )
            digests = normal_digests(args.arch)
            receipt = {"commit": rev, "arch": args.arch, "std_metadata_seed": tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"], "sha256": digests}
            args.baseline.write_text(json.dumps(receipt, indent=2) + "\n")
            print(f"Recorded {len(digests)} {args.arch} bins at {rev}")
        else:
            existing_links_load_identical(args.arch, args.baseline, args.baseline_release, args.readelf,
                                          args.candidate_release)


if __name__ == "__main__":
    main()
