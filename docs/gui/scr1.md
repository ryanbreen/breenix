# SCR1 capture disk

Create a sparse 8 GiB raw disk and enable capture on the disposable root with:

```sh
scripts/boot-interactive.sh --mode suite --suite gpu,breengel --gpu-gl --display --screen-proof --screen-capture /absolute/new-disk.img
```

The path must be absent.
The launch uses the normal Mac boot lease and Vigil registration. After QEMU
stops, run:

```
cargo run --release --manifest-path libs/screen-capture/Cargo.toml --bin verify -- CAPTURE_DISK SERIAL_LOG
```

The verifier requires disk pictures to match their serial SCREEN digest, mode
and first occurrence's suite/case/checkpoint. It also requires the serial's
set of distinct picture digests to be present on disk. It rejects corrupt
records, duplicate disk digests, missing pictures and non-picture SCREEN lines.
This command does not approve pictures or write the known-good table. The
Vigil candidates exporter consumes this format in the gallery unit.

The guest retains the canonical bytes used for its digest, then sends an
encoded record through the root-only suite evidence endpoint. A `SCRWRITE`
request contains an eight-byte tag followed by a little-endian u64 userspace
address and u64 byte length. On ARM QEMU, the endpoint additionally requires
`opt/breenix/screen-capture=on` and exactly one disk carrying the marker below.
It validates sector data with bounded buffers, commits the first sector after
the digest matches, flushes before success,
and remembers digests across case forks and suite execs. A validation, payload
read, write or flush failure during streaming poisons subsequent capture writes
for that boot, since payload sectors may already have been written. Capture errors become additive SCREEN
errors, leaving CASE expectations and suite scoring unchanged.

## Disk and records

The first sector is `SCRDISK1` followed by 504 bytes of 0x00. Records start at
byte 512, align to 512 bytes and use little-endian fields. An unwritten 0x00
tail ends the stream; the disk has no filesystem.

| Offset | Bytes | Meaning |
|---|---|---|
| 0 | 4 | `SCR1` |
| 4 | 4 | Total record length, including padding |
| 8 | 4 | Encoding: 0 raw BGRX, 1 pixel RLE |
| 12 | 4 | Pixel payload byte length |
| 16 | 4 | Width |
| 20 | 4 | Height |
| 24 | 2 | Suite name byte length |
| 26 | 2 | Case name byte length |
| 28 | 2 | Checkpoint name byte length |
| 30 | 2 | Reserved, 0x00 |
| 32 | 32 | SHA-256 of canonical decoded pixels |
| 64 | 4 | Decoded pixel byte length |
| 68 | 12 | Reserved, 0x00 |
| 80 | variable | UTF-8 suite, case and checkpoint, without separators |
| following | variable | Pixel payload |
| following | variable | 0x00 padding to the sector boundary |

Pixels are B, G, R, 255, in row order. RLE consists of eight-byte runs:
u32 nonzero pixel count followed by one four-byte pixel. The encoder selects
RLE only when its payload is smaller than raw; ties use raw. Names are at
most 255 bytes. The decoded picture limit is 2560 × 1600 × 4 bytes. The
encoder and decoder are shared by the guest, kernel sink and host verifier.
