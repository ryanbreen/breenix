#!/usr/bin/env python3
"""Patch GPT partition type from 'Microsoft Basic Data' to 'EFI System Partition'.

macOS hdiutil creates FAT32 partitions with the 'Microsoft Basic Data' type GUID.
UEFI firmware requires 'EFI System Partition' type to auto-discover and boot from
the ESP. This script patches both the primary and backup GPT headers with the
correct partition type GUID and updates CRC32 checksums.

Usage: patch-gpt-esp.py <raw-disk-image>
"""

import struct
import sys
import uuid
import zlib


# GPT partition type GUIDs
BASIC_DATA = uuid.UUID('EBD0A0A2-B9E5-4433-87C0-68B6B72699C7')
EFI_SYSTEM = uuid.UUID('C12A7328-F81F-11D2-BA4B-00A0C93EC93B')

SECTOR_SIZE = 512


def uuid_to_mixed_endian(u):
    """Convert a UUID to GPT's mixed-endian on-disk format.

    GPT stores UUIDs with the first three components in little-endian
    and the last two in big-endian (network byte order).
    """
    b = struct.pack('<IHH', u.time_low, u.time_mid, u.time_hi_version)
    b += u.clock_seq_hi_variant.to_bytes(1, 'big')
    b += u.clock_seq_low.to_bytes(1, 'big')
    b += u.node.to_bytes(6, 'big')
    return b


def crc32(data):
    """Compute CRC32 as used in GPT headers (unsigned)."""
    return zlib.crc32(data) & 0xFFFFFFFF


def patch_gpt(path):
    with open(path, 'r+b') as f:
        data = bytearray(f.read())

    basic_data_bytes = uuid_to_mixed_endian(BASIC_DATA)
    efi_system_bytes = uuid_to_mixed_endian(EFI_SYSTEM)

    # --- Patch primary GPT ---
    # Primary GPT header is at LBA 1 (offset 512)
    hdr_offset = SECTOR_SIZE
    signature = data[hdr_offset:hdr_offset + 8]
    if signature != b'EFI PART':
        print(f"ERROR: No GPT signature at offset {hdr_offset}", file=sys.stderr)
        sys.exit(1)

    # Header fields
    header_size = struct.unpack_from('<I', data, hdr_offset + 12)[0]
    part_entry_lba = struct.unpack_from('<Q', data, hdr_offset + 72)[0]
    num_parts = struct.unpack_from('<I', data, hdr_offset + 80)[0]
    part_entry_size = struct.unpack_from('<I', data, hdr_offset + 84)[0]
    backup_lba = struct.unpack_from('<Q', data, hdr_offset + 32)[0]

    # Patch partition entries (primary)
    entries_offset = part_entry_lba * SECTOR_SIZE
    entries_total_size = num_parts * part_entry_size
    patched = 0

    for i in range(num_parts):
        entry_off = entries_offset + i * part_entry_size
        type_guid = data[entry_off:entry_off + 16]
        if type_guid == basic_data_bytes:
            data[entry_off:entry_off + 16] = efi_system_bytes
            patched += 1

    if patched == 0:
        print("WARNING: No 'Microsoft Basic Data' partitions found to patch",
              file=sys.stderr)
        return

    # Update primary header's partition entries CRC32
    entries_crc = crc32(bytes(data[entries_offset:entries_offset + entries_total_size]))
    struct.pack_into('<I', data, hdr_offset + 88, entries_crc)

    # Update primary header CRC32 (zero the field first, then compute)
    struct.pack_into('<I', data, hdr_offset + 16, 0)
    hdr_crc = crc32(bytes(data[hdr_offset:hdr_offset + header_size]))
    struct.pack_into('<I', data, hdr_offset + 16, hdr_crc)

    # --- Patch backup GPT ---
    backup_hdr_offset = backup_lba * SECTOR_SIZE
    if backup_hdr_offset < len(data):
        backup_sig = data[backup_hdr_offset:backup_hdr_offset + 8]
        if backup_sig == b'EFI PART':
            # Backup partition entries are stored just before the backup header
            backup_part_lba = struct.unpack_from('<Q', data, backup_hdr_offset + 72)[0]
            backup_entries_offset = backup_part_lba * SECTOR_SIZE

            # Patch backup partition entries
            for i in range(num_parts):
                entry_off = backup_entries_offset + i * part_entry_size
                type_guid = data[entry_off:entry_off + 16]
                if type_guid == basic_data_bytes:
                    data[entry_off:entry_off + 16] = efi_system_bytes

            # Update backup header's partition entries CRC32
            backup_entries_crc = crc32(bytes(
                data[backup_entries_offset:backup_entries_offset + entries_total_size]
            ))
            struct.pack_into('<I', data, backup_hdr_offset + 88, backup_entries_crc)

            # Update backup header CRC32
            struct.pack_into('<I', data, backup_hdr_offset + 16, 0)
            backup_hdr_crc = crc32(bytes(
                data[backup_hdr_offset:backup_hdr_offset + header_size]
            ))
            struct.pack_into('<I', data, backup_hdr_offset + 16, backup_hdr_crc)

    with open(path, 'wb') as f:
        f.write(data)

    print(f"  Patched {patched} partition(s) to EFI System Partition type")


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <raw-disk-image>", file=sys.stderr)
        sys.exit(1)
    patch_gpt(sys.argv[1])
