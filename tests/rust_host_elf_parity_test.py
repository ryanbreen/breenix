"""Ruling 98 controls for comparing ordinary Breenix ELFs."""

import os
from pathlib import Path
import struct
import tempfile
import unittest

from rust_host_link_test import elf_differences


def fixture(names, text=b"\x90"):
    # ELF gABI sections 2, 3, 5 and 6: ELF64 header, sections, symbols and strings.
    strings = bytearray(b"\0")
    symbols = bytearray(24)
    for name, value in names:
        symbols.extend(struct.pack("<IBBHQQ", len(strings), 2, 0, 1, value, 1))
        strings.extend(name + b"\0")
    section_names = b"\0.text\0.symtab\0.strtab\0.shstrtab\0"
    image = bytearray(128)
    image[64:65] = text
    sections = [
        bytes(64),
        struct.pack("<IIQQQQIIQQ", 1, 1, 6, 0x40000040, 64, 1, 0, 0, 1, 0),
    ]
    for name, payload, kind, link, info, alignment, entry in (
        (b".symtab", symbols, 2, 3, len(names) + 1, 8, 24),
        (b".strtab", strings, 3, 0, 0, 1, 0),
        (b".shstrtab", section_names, 3, 0, 0, 1, 0),
    ):
        image.extend(bytes(-len(image) % alignment))
        sections.append(struct.pack("<IIQQQQIIQQ", section_names.index(name), kind, 0, 0,
                                    len(image), len(payload), link, info, alignment, entry))
        image.extend(payload)
    image.extend(bytes(-len(image) % 8))
    image[:64] = struct.pack("<16sHHIQQQIHHHHHH", b"\x7fELF\x02\x01\x01", 2, 62, 1,
                             0x40000040, 0, len(image), 0, 64, 56, 0, 64, len(sections), 4)
    image.extend(b"".join(sections))
    return image


class ElfParityTest(unittest.TestCase):
    def compare(self, before, after):
        with tempfile.TemporaryDirectory() as temp:
            reference = Path(temp) / "reference"
            candidate = Path(temp) / "candidate"
            reference.write_bytes(before)
            candidate.write_bytes(after)
            return elf_differences(reference, candidate, os.environ.get("LLVM_READELF", "llvm-readelf"))

    def test_digest_width_changes_preserve_parity(self):
        before = fixture([
            (b"function.llvm.123456789", 1),
            (b"anon.0123456789abcdef0123456789abcdef.8", 2),
            (b"_RNvCs12345_5crate3run", 3),
            (b"_ZN5crate3run17h0123456789abcdefE", 4),
        ])
        after = fixture([
            (b"_ZN5crate3run17hfedcba9876543210E", 4),
            (b"_RNvCs6_5crate3run", 3),
            (b"function.llvm.5", 1),
            (b"anon.fedcba9876543210fedcba9876543210.8", 2),
        ])
        self.assertEqual(self.compare(before, after), [])

    def test_renamed_symbol_breaks_parity(self):
        before = fixture([(b"function.llvm.123456", 1)])
        after = fixture([(b"renamed_.llvm.123456", 1)])
        self.assertTrue(self.compare(before, after))

    def test_changed_allocated_byte_breaks_parity(self):
        names = [(b"function.llvm.123456", 1)]
        self.assertIn(".text", self.compare(fixture(names), fixture(names, b"\x91")))


if __name__ == "__main__":
    unittest.main()
