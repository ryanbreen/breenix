"""Build the E25 reference inventory from Clang and lld's build records."""

import hashlib
import os
from pathlib import Path
import re


def file_record(path):
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(128 * 1024), b""):
            digest.update(chunk)
    return {"path": str(path), "sha256": digest.hexdigest()}


def input_path(path, roots):
    resolved = path.resolve(strict=True)
    if not any(resolved.is_relative_to(root) for root in roots):
        raise ValueError(f"input outside the allowed roots: {resolved}")
    return resolved


def dep_paths(depfile):
    # Both tools write the input list in the first make rule. lld follows
    # it with empty rules for the dependencies.
    text = os.fsdecode(depfile.read_bytes())
    first_rule = text.replace("\\\n", " ").split("\n", 1)[0]
    _, colon, prerequisites = first_rule.partition(":")
    if not colon:
        raise ValueError(f"missing dependency rule in {depfile}")
    for token in re.findall(r"(?:\\[ \t#]|[^\s])+", prerequisites):
        name = re.sub(r"\\([ \t#])", r"\1", token).replace("$$", "$")
        yield Path(name)


def include_records(depfile, source, roots):
    dependencies = {input_path(path, roots) for path in dep_paths(depfile)}
    if source not in dependencies:
        raise ValueError(f"missing source in Clang dependencies: {source}")
    return [file_record(path) for path in sorted(dependencies)]


def archive_record(path, selected):
    # An ordinary ar archive stores each member in a 60-byte header followed
    # by its bytes and even-byte alignment padding. Handle the two extended
    # name forms emitted by the producer's archiver.
    data = path.read_bytes()
    if data[:8] != b"!<arch>\n":
        raise ValueError(f"expected an ordinary archive: {path}")
    names = b""
    hashes = {}
    offset = 8
    while offset < len(data):
        header = data[offset:offset + 60]
        size_field = header[48:58].strip()
        if header[58:60] != b"\x60\n" or not size_field.isdigit():
            raise ValueError(f"invalid archive header at {offset} in {path}")
        size = int(size_field)
        start = offset + 60
        end = start + size
        offset = end + size % 2
        if offset > len(data):
            raise ValueError(f"truncated archive member in {path}")
        name = header[:16].rstrip(b" ")
        if name == b"//":
            names = data[start:end]
            continue
        if name in {b"/", b"/SYM64/"}:
            continue
        if name.startswith(b"#1/"):
            length = int(name[3:])
            if length < 0 or length > size:
                raise ValueError(f"invalid archive member name length in {path}")
            name = data[start:start + length].rstrip(b"\0")
            start += length
        elif name.startswith(b"/"):
            index = int(name[1:])
            limit = names.find(b"/\n", index)
            if index < 0 or limit < 0:
                raise ValueError(f"invalid archive member name offset in {path}")
            name = names[index:limit]
        else:
            name = name.removesuffix(b"/")
        member = os.fsdecode(name)
        if member in selected:
            digest = hashlib.sha256(memoryview(data)[start:end]).hexdigest()
            if hashes.setdefault(member, digest) != digest:
                raise ValueError(f"ambiguous extracted member {member} in {path}")
    if missing := selected - hashes.keys():
        raise ValueError(f"extracted members missing from {path}: {', '.join(sorted(missing))}")
    return {
        "path": str(path),
        "sha256": hashlib.sha256(data).hexdigest(),
        "members": [{"name": name, "sha256": digest} for name, digest in sorted(hashes.items())],
    }


MAP_INPUT = re.compile(r"^\s*[0-9a-fA-F]+\s+[0-9a-fA-F]+\s+[0-9a-fA-F]+\s+\d+ {9}(.+):\(")


def link_records(object_file, archives, link_script, depfile, link_map, trace):
    # The dependency file covers the script and archives even when no member
    # is extracted. The trace covers members that section GC leaves off the map.
    expected = {object_file, link_script, *archives}
    deps = {path.resolve(strict=True) for path in dep_paths(depfile)}
    if unexpected := deps - expected:
        raise ValueError(f"unexpected linker input: {min(unexpected)}")
    if missing := expected - deps:
        raise ValueError(f"input missing from lld dependencies: {min(missing)}")

    traced = set(trace.splitlines())
    if str(object_file) not in traced:
        raise ValueError(f"compiled object missing from lld trace: {object_file}")
    selected = {path: set() for path in archives}
    for entry in traced - {str(object_file), str(link_script), *map(str, archives)}:
        matches = [path for path in selected if entry.startswith(f"{path}(")]
        if len(matches) != 1 or not entry.endswith(")"):
            raise ValueError(f"unexpected linker trace input: {entry}")
        archive = matches[0]
        selected[archive].add(entry[len(str(archive)) + 1:-1])

    for line in os.fsdecode(link_map.read_bytes()).splitlines():
        if match := MAP_INPUT.match(line):
            if match[1] not in traced and match[1] != "<internal>":
                raise ValueError(f"link map input missing from trace: {match[1]}")
    return [archive_record(path, names) for path, names in selected.items()]


def build_inventory(
    arch, producer, flags, includes, elf, object_file, archives,
    link_script, link_deps, link_map, trace,
):
    return {
        "arch": arch,
        "producer": producer,
        "flags": flags,
        "includes": includes,
        "object": file_record(object_file),
        "archives": link_records(object_file, archives, link_script, link_deps, link_map, trace),
        "link_script": file_record(link_script),
        "elf": file_record(elf),
    }
