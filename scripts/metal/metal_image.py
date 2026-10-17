"""The x86-metal boot image and test payloads (docs/metal/README.md, "What is served").

boot.img is a GPT disk with one FAT32 EFI system partition holding the pinned bootloader's
efi/boot/bootx64.efi, kernel-x86_64 and boot.json (taken from the UEFI image the kernel build makes) plus the
bootloader's optional `ramdisk` file. Firmware HTTP boot registers the downloaded image as a RAM disk and starts its
bootloader, which then finds the kernel, its configuration and the RAM disk on its own device.
"""
import hashlib
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import uuid

SECTOR = 512
ESP_TYPE = uuid.UUID('c12a7328-f81f-11d2-ba4b-00a0c93ec93b')
ESP_START = 2048
ESP_FILES = ('efi/boot/bootx64.efi', 'kernel-x86_64', 'boot.json')
CONTENT_TYPES = {'img': 'application/vnd.efi-img', 'efi': 'application/efi', 'iso': 'application/vnd.efi-iso'}


def sha256(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as handle:
        while chunk := handle.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def esp_offset(image):
    """Byte offset and length of the first EFI system partition of a GPT disk image."""
    with open(image, 'rb') as disk:
        disk.seek(SECTOR)
        header = disk.read(92)
        if header[:8] != b'EFI PART':
            raise ValueError('no GPT header')
        entries_lba, count, size = struct.unpack_from('<QII', header, 72)
        disk.seek(entries_lba * SECTOR)
        table = disk.read(count * size)
    for index in range(count):
        entry = table[index * size:(index + 1) * size]
        if uuid.UUID(bytes_le=entry[:16]) == ESP_TYPE:
            first, last = struct.unpack_from('<QQ', entry, 32)
            return first * SECTOR, (last - first + 1) * SECTOR
    raise ValueError('no EFI system partition')


def run(*command):
    # mtools otherwise refuses images whose size is not a whole number of floppy-style tracks.
    subprocess.run(command, check=True, capture_output=True, env=dict(os.environ, MTOOLS_SKIP_CHECK='1'))


def extract_esp(uefi_image, directory):
    """Copies the bootloader, kernel and boot configuration out of the kernel build's UEFI image."""
    offset, _ = esp_offset(uefi_image)
    directory = Path(directory)
    for name in ESP_FILES:
        target = directory / name
        target.parent.mkdir(parents=True, exist_ok=True)
        run('mcopy', '-n', '-i', f'{uefi_image}@@{offset}', '::/' + name, str(target))
    return {name: directory / name for name in ESP_FILES}


def assemble(output, files):
    """Writes a GPT disk with one FAT32 ESP holding `files` ({path in the ESP: source}); returns the image facts."""
    output = Path(output)
    payload = sum(Path(source).stat().st_size for source in files.values())
    # FAT32 needs at least 65525 clusters; at 4 KiB clusters that is 256 MiB, so small payloads get that floor.
    esp_bytes = max(payload + payload // 32 + (32 << 20), 260 << 20)
    esp_bytes = (esp_bytes + (1 << 20) - 1) // (1 << 20) << 20
    total = ESP_START * SECTOR + esp_bytes + (1 << 20)
    output.unlink(missing_ok=True)
    with output.open('wb') as disk:
        disk.truncate(total)
    last = ESP_START + esp_bytes // SECTOR - 1
    run('sgdisk', '--clear', f'--new=1:{ESP_START}:{last}', '--typecode=1:EF00', '--change-name=1:EFI system',
        str(output))
    run('mkfs.fat', '-F', '32', '-S', str(SECTOR), '-s', '8', '-n', 'BREENIX', '--offset', str(ESP_START),
        str(output), str(esp_bytes // 1024))
    target = f'{output}@@{ESP_START * SECTOR}'
    made = set()
    for name in sorted(files):
        parent = ''
        for part in Path(name).parent.parts:
            parent = f'{parent}/{part}'
            if parent not in made:
                run('mmd', '-i', target, '::' + parent)
                made.add(parent)
        run('mcopy', '-i', target, str(files[name]), '::/' + name)
    return verify(output, files)


def verify(output, files):
    """Reads every file back out of the image and compares it with its source."""
    offset, _ = esp_offset(output)
    components = {}
    with tempfile.TemporaryDirectory() as scratch:
        for name, source in files.items():
            copy = Path(scratch) / 'check'
            run('mcopy', '-n', '-i', f'{output}@@{offset}', '::/' + name, str(copy))
            expected = sha256(source)
            if sha256(copy) != expected:
                raise ValueError(f'{name} reads back differently from its source')
            components[name] = {'size': Path(source).stat().st_size, 'sha256': expected}
            copy.unlink()
    return {'size': output.stat().st_size, 'sha256': sha256(output), 'components': components}


# metal-hang.efi: a UEFI application that writes one line to the COM1 UART, masks interrupts and halts forever.
# Its header layout follows the published PE/COFF specification; the code is position independent and its base
# relocation directory is present but holds no fixups.

HANG_LINE = b'\r\nmetal-hang: interrupts masked, halting\r\n'


def hang_code():
    code = bytearray()
    code += b'\x48\x8d\x35' + b'\0\0\0\0'      # lea rsi, [rip + message]
    lea_end = len(code)
    loop = len(code)
    code += b'\x8a\x06'                          # mov al, [rsi]
    code += b'\x84\xc0'                          # test al, al
    code += b'\x74\x00'                          # jz halt
    jz_halt = len(code) - 1
    wait = len(code)
    code += b'\x66\xba\xfd\x03'                  # mov dx, 0x3fd (line status)
    code += b'\xec'                              # in al, dx
    code += b'\xa8\x20'                          # test al, 0x20 (transmit holding register empty)
    code += b'\x74' + bytes([(wait - (len(code) + 2)) & 0xff])  # jz wait
    code += b'\x8a\x06'                          # mov al, [rsi]
    code += b'\x66\xba\xf8\x03'                  # mov dx, 0x3f8 (data)
    code += b'\xee'                              # out dx, al
    code += b'\x48\xff\xc6'                      # inc rsi
    code += b'\xeb' + bytes([(loop - (len(code) + 2)) & 0xff])  # jmp loop
    halt = len(code)
    code[jz_halt] = (halt - (jz_halt + 1)) & 0xff
    code += b'\xfa'                              # cli
    code += b'\xf4'                              # hlt
    code += b'\xeb\xfc'                          # jmp back to cli; an NMI's return lands here
    message = len(code)
    code += HANG_LINE + b'\0'
    struct.pack_into('<i', code, 3, message - lea_end)
    return bytes(code)


def pe32plus(code):
    """A PE32+ EFI application image: headers, .text (the code) and .reloc (one empty block)."""
    file_align, section_align = 0x200, 0x1000
    text_rva, reloc_rva = 0x1000, 0x2000
    reloc = struct.pack('<IIHH', text_rva, 12, 0, 0)  # one block, two IMAGE_REL_BASED_ABSOLUTE padding entries

    def padded(data):
        return data + b'\0' * (-len(data) % file_align)
    text_raw, reloc_raw = padded(code), padded(reloc)
    headers_size = file_align
    image_size = reloc_rva + section_align
    dos = bytearray(64)
    dos[0:2] = b'MZ'
    struct.pack_into('<I', dos, 0x3c, 64)
    coff = struct.pack('<HHIIIHH', 0x8664, 2, 0, 0, 0, 240, 0x0022)
    optional = struct.pack('<HBBIIIII', 0x20b, 0, 0, len(text_raw), len(reloc_raw), 0, text_rva, text_rva)
    optional += struct.pack('<QIIHHHHHHIIIIHHQQQQII', 0x10000000, section_align, file_align, 0, 0, 0, 0, 0, 0, 0,
                            image_size, headers_size, 0, 10, 0, 0x100000, 0x1000, 0x100000, 0x1000, 0, 16)
    directories = [(0, 0)] * 16
    directories[5] = (reloc_rva, len(reloc))
    optional += b''.join(struct.pack('<II', rva, size) for rva, size in directories)
    text = struct.pack('<8sIIIIIIHHI', b'.text', len(code), text_rva, len(text_raw), headers_size, 0, 0, 0, 0,
                       0x60000020)
    relocation = struct.pack('<8sIIIIIIHHI', b'.reloc', len(reloc), reloc_rva, len(reloc_raw),
                             headers_size + len(text_raw), 0, 0, 0, 0, 0x42000040)
    header = bytes(dos) + b'PE\0\0' + coff + optional + text + relocation
    return header + b'\0' * (headers_size - len(header)) + text_raw + reloc_raw


def hang_efi():
    return pe32plus(hang_code())
