#!/usr/bin/env python3
"""Author the 256 declared client inputs for breengel.md section 24."""
from pathlib import Path
import struct
import zlib


def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))


def image(seed):
    filtered = bytearray()
    for y in range(256):
        row = bytearray()
        for x in range(256):
            row.extend(((x ^ y ^ seed) & 255, (5 * y + 29 * seed) & 255, (7 * x + 13 * seed) & 255))
        filtered.append(1)
        filtered.extend((value - (row[i - 3] if i >= 3 else 0)) & 255 for i, value in enumerate(row))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 256, 256, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(filtered, 9)) + chunk(b'IEND', b''))


if __name__ == '__main__':
    for seed in range(256):
        Path(__file__).with_name(f'{seed:02x}.png').write_bytes(image(seed))
