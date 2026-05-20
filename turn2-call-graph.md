# Turn 2: virtio-block and virtio-sound polling call graphs

## Verdict

Status: COMPLETE.

The audited PCI driver polling sites in `kernel/src/drivers/virtio/block.rs`
and `kernel/src/drivers/virtio/sound.rs` are not reached on current aarch64
Parallels production boots. Parallels takes the PCI branch in
`drivers::init()`, but that branch initializes virtio GPU, virtio net, USB,
and AHCI storage; it does not call PCI virtio-block or PCI virtio-sound init.
Saved Parallels logs confirm only virtio net/GPU/vsock-like devices plus AHCI
storage, and ext2 falls back to AHCI.

The polling sites are still real code paths:

- `block.rs` is production on x86_64 when PCI virtio-block disks are present,
  and it is used by x86_64 testing disk loaders.
- `sound.rs` is production on x86_64 when PCI virtio-sound is present and a
  userspace program invokes the Breenix audio syscalls.
- `block_mmio.rs` has additional polling loops and is the production aarch64
  QEMU/MMIO block path, not the Parallels path.
- `sound_mmio.rs` also has additional polling loops. It is not part of the
  requested file set, but it is the aarch64 audio syscall backend when a VirtIO
  MMIO sound device has been initialized.

## A. `block.rs` PCI call graph

### Polling site: `VirtioBlockDevice::read_sector`

Signature:

```rust
pub fn read_sector(&self, sector: u64, buffer: &mut [u8]) -> Result<(), &'static str>
```

Excerpt around the loop:

```rust
// Poll for completion (synchronous for now)
// Use a reasonable timeout with delays to give QEMU TCG time to process
let mut timeout = 100_000u32;
while !queue.has_used() && timeout > 0 {
    // Do a small spin delay - QEMU TCG needs CPU time to process I/O
```

Direct callers found with `rg`:

- `VirtioBlockDevice::read_sectors()` in `block.rs`
- `VirtioBlockDevice::test_read()` in `block.rs`
- `VirtioBlockWrapper::read_block()` in `kernel/src/block/virtio.rs`
- `userspace_test::load_test_binary_from_disk()` in `kernel/src/userspace_test.rs`

Call graph:

```text
VirtioBlockDevice::read_sector
  <- VirtioBlockDevice::test_read
     <- drivers::init() [x86_64 only]

VirtioBlockDevice::read_sector
  <- VirtioBlockWrapper::read_block [x86_64 BlockDevice impl]
     <- BlockDevice::read_blocks default
        <- fs::ext2::file::read_ext2_blocks/read_ext2_block
           <- fs::ext2 mount, inode, file, and syscall read/exec paths
           <- main.rs init_root_fs/init_home_fs [x86_64]

VirtioBlockDevice::read_sector
  <- VirtioBlockDevice::read_sectors
     <- userspace_test::load_test_binary_from_disk [feature/test loaders]
        <- userspace_test::get_test_binary / get_test_binary_static
        <- main.rs feature-gated blocking_recv_test/nonblock_eagain_test loaders
```

Classification:

- x86_64 production one-shot: `drivers::init() -> virtio::block::test_read()`.
- x86_64 production hot path: ext2 reads through `VirtioBlockWrapper` if a PCI
  virtio-block disk is mounted.
- x86_64 test-only/harness path: `userspace_test` disk loading under testing or
  feature-gated network tests.
- aarch64 Parallels: unreachable. The aarch64 Parallels driver branch never
  calls `virtio::block::init()`, and `block.rs` is not the aarch64 block wrapper
  backend.

### Polling site: `VirtioBlockDevice::write_sector`

Signature:

```rust
pub fn write_sector(&self, sector: u64, buffer: &[u8]) -> Result<(), &'static str>
```

Excerpt around the loop:

```rust
// Poll for completion (synchronous for now)
let mut timeout = 1_000_000u32;
while !queue.has_used() && timeout > 0 {
    core::hint::spin_loop();
    timeout -= 1;
```

Direct callers found with `rg`:

- `VirtioBlockWrapper::write_block()` in `kernel/src/block/virtio.rs`

Call graph:

```text
VirtioBlockDevice::write_sector
  <- VirtioBlockWrapper::write_block [x86_64 BlockDevice impl]
     <- fs::ext2::file::write_ext2_block
     <- fs::ext2::superblock::write_to
     <- fs::ext2::inode/block_group/file write paths
     <- syscall write/create/truncate/link/unlink-style filesystem paths
```

Classification:

- x86_64 production hot path if ext2 is mounted on a PCI virtio-block disk.
- aarch64 Parallels: unreachable for the same reason as `read_sector`.
- No direct `block.rs` write test caller was found; write coverage is through
  filesystem use on x86_64 or any future caller of the `BlockDevice` wrapper.

### Aarch64 Parallels PCI init result

For aarch64, `drivers::init()` has two major branches:

- QEMU/hybrid: calls `init_virtio_mmio()` and uses MMIO virtio block.
- PCI platform (Parallels): enumerates PCI, initializes virtio GPU PCI,
  virtio net PCI, VMware SVGA fallback, USB, and AHCI. It does not call
  `virtio::block::init()` or `virtio::sound::init()`.

The PCI discovery helpers do exist:

```text
pci::find_virtio_block_devices -> filter Device::is_virtio_block
pci::find_virtio_sound_devices -> filter Device::is_virtio_sound
```

But on aarch64 Parallels they are not used by the production init path.

## B. `block_mmio.rs` call graph

`block_mmio.rs` does contain polling loops. They are not `queue.has_used()`
loops, but they are synchronous used-ring polling loops over
`queue_mem.used.idx`.

### Polling site: `block_mmio::read_sector_inner`

Public wrapper signature:

```rust
pub fn read_sector(
    device_index: usize,
    sector: u64,
    buffer: &mut [u8; SECTOR_SIZE],
) -> Result<(), &'static str>
```

Inner polling excerpt:

```rust
let used_idx = unsafe { read_volatile(&(*bufs.queue_mem).used.idx) };
if used_idx != state.last_used_idx {
    state.last_used_idx = used_idx;
    break;
}
timeout -= 1;
```

Call graph:

```text
block_mmio::read_sector
  <- block_mmio::test_read
     <- init_virtio_mmio() [aarch64 QEMU/MMIO production one-shot]

block_mmio::read_sector
  <- VirtioBlockWrapper::read_block [aarch64 BlockDevice impl]
     <- BlockDevice::read_blocks default
        <- ext2 read/mount/syscall paths
        <- init_root_fs/init_home_fs

block_mmio::read_sector
  <- boot::test_disk::TestDisk::read / TestDisk::read_binary
     <- boot::test_disk::run_userspace_from_disk
     <- main_aarch64 fallback launch path if preloaded/ext2 init fails

block_mmio::read_sector
  <- block_mmio::test_multi_read / test_sequential_read / test_invalid_sector /
     test_uninitialized_read / test_write_read_verify
     <- test_framework::registry ARM64 virtio-blk tests
```

Classification:

- aarch64 QEMU/MMIO production one-shot: boot `test_read`.
- aarch64 QEMU/MMIO production hot path: ext2 reads and userspace binary reads
  if root/test disks are backed by VirtIO MMIO.
- aarch64 test-only: test framework registry functions.
- aarch64 Parallels: normally unreachable. Saved Parallels logs show the PCI
  branch, AHCI storage, and no VirtIO MMIO block init. If the fallback
  `run_userspace_from_disk()` is reached on Parallels, `read_sector_inner`
  should return before polling because no MMIO block device state exists.

### Polling site: `block_mmio::write_sector_inner`

Public wrapper signature:

```rust
pub fn write_sector(
    device_index: usize,
    sector: u64,
    buffer: &[u8; SECTOR_SIZE],
) -> Result<(), &'static str>
```

Inner polling excerpt:

```rust
let used_idx = unsafe { read_volatile(&(*bufs.queue_mem).used.idx) };
if used_idx != state.last_used_idx {
    state.last_used_idx = used_idx;
    break;
}
timeout -= 1;
```

Call graph:

```text
block_mmio::write_sector
  <- VirtioBlockWrapper::write_block [aarch64 BlockDevice impl]
     <- ext2 write paths
     <- syscall filesystem mutations/writes

block_mmio::write_sector
  <- block_mmio::test_write_read_verify
     <- test_framework::registry ARM64 virtio-blk write-read-verify test
```

Classification:

- aarch64 QEMU/MMIO production hot path if ext2 writes target VirtIO MMIO.
- aarch64 test-only for the registry write-read-verify test.
- aarch64 Parallels: normally unreachable because ext2 uses AHCI.

## C. `sound.rs` PCI call graph

### Polling site: `VirtioSoundDevice::send_ctrl`

Signature:

```rust
fn send_ctrl(&mut self, cmd_len: u32, resp_len: u32) -> Result<(), &'static str>
```

Excerpt around the loop:

```rust
// Poll for completion
let mut timeout = 100_000u32;
while !self.ctrl_queue.has_used() && timeout > 0 {
    for _ in 0..1000 {
        core::hint::spin_loop();
```

Call graph:

```text
VirtioSoundDevice::send_ctrl
  <- VirtioSoundDevice::do_setup_stream [SET_PARAMS, PREPARE, START]
     <- sound::setup_stream
        <- syscall::audio::sys_audio_init [x86_64 cfg]
        <- syscall dispatcher/handler paths for syscall 420
        <- libbreenix::audio::init
        <- userspace programs: tones, bsh, fart
```

Classification:

- x86_64 production hot path when PCI virtio-sound exists and userspace invokes
  audio init.
- x86_64 dormant/error path when no PCI virtio-sound exists:
  `sound::setup_stream()` returns `"Sound device not initialized"` before
  reaching `send_ctrl`.
- aarch64 Parallels: unreachable. Aarch64 `sys_audio_init()` calls
  `sound_mmio::setup_stream()`, and the aarch64 Parallels driver init branch
  does not call PCI `sound::init()`.

### Polling site: `VirtioSoundDevice::do_write_pcm`

Signature:

```rust
fn do_write_pcm(&mut self, data: &[u8]) -> Result<usize, &'static str>
```

Excerpt around the loop:

```rust
// Poll for completion
let mut timeout = 100_000u32;
while !self.tx_queue.has_used() && timeout > 0 {
    for _ in 0..1000 {
        core::hint::spin_loop();
```

Call graph:

```text
VirtioSoundDevice::do_write_pcm
  <- sound::write_pcm
     <- syscall::audio::sys_audio_write [x86_64 cfg]
     <- syscall dispatcher/handler paths for syscall 421
     <- libbreenix::audio::write_pcm / write_samples
     <- userspace programs: tones, bsh, fart
```

Classification:

- x86_64 production hot path when PCI virtio-sound exists, the stream has been
  started, and userspace writes PCM samples.
- x86_64 dormant/error path when no PCI virtio-sound exists or stream setup has
  not succeeded.
- aarch64 Parallels: unreachable through `sound.rs`.

### Extra finding: aarch64 sound backend

`kernel/src/syscall/audio.rs` selects `sound_mmio` on aarch64:

```text
sys_audio_init  -> drivers::virtio::sound_mmio::setup_stream()
sys_audio_write -> drivers::virtio::sound_mmio::write_pcm(data)
```

`sound_mmio.rs` has synchronous polling loops in:

- `send_ctrl_command()` around `CTRL_QUEUE.used.idx`
- `write_pcm()` around `TX_QUEUE.used.idx`

On current aarch64 Parallels boots, the PCI driver branch does not call
`sound_mmio::init()`, so these loops should return before polling with
`"Sound device not initialized"` unless a VirtIO MMIO sound device was
initialized on a QEMU/MMIO boot.

## D. Boot-time enumeration on Parallels

Existing serial log examined:

```text
/Users/wrb/fun/code/breenix.worktrees/scheduler-wake-atomic/turn10-artifacts/stress-gate/boot-1/serial.log
```

Relevant findings:

```text
[drivers] PCI ECAM at 0x2300000, enumerating PCI bus...
[drivers] Found 7 PCI devices
[drivers] Found 2 VirtIO PCI devices
[pci] 00:05.0 [1af4:1000] class=02/00
[pci] 00:0a.0 [1af4:1050] class=03/00
[pci] 00:0e.0 [1af4:1053] class=07/80
[drivers] VirtIO GPU (PCI) initialized
[drivers] VirtIO network (PCI) initialized
[drivers] AHCI initialized (platform MMIO): 2 SATA device(s)
[ext2] No VirtIO block device, trying AHCI...
[ext2] AHCI device 0: not ext2 (magic=0x0064)
[ext2] Found ext2 superblock on AHCI device 1
```

No PCI device ID for virtio-block (`1af4:1001` legacy or `1af4:1042` modern)
appears in the PCI dump. No PCI device ID for virtio-sound (`1af4:1019` legacy
or `1af4:1059` modern) appears either. The Linux probe VM in Turn 1 had the
same shape: SATA/AHCI storage and HDA audio, not virtio-blk/sound.

## E. Re-scoped fix proposal

The original audit correctly identified bad synchronous polling loops, but the
Parallels production impact is narrower than expected:

- The specific `block.rs` and `sound.rs` polling loops are not current aarch64
  Parallels production hot paths.
- They remain legitimate cleanup/fix targets because the contract success
  criteria explicitly require zero synchronous `while !queue.has_used()` loops
  in those two files, and they are production on x86_64 when matching PCI
  virtio devices exist.
- The aarch64 production-equivalent VirtIO block path is `block_mmio.rs` on
  QEMU/MMIO and hybrid QEMU, not Parallels. It has the same architectural
  problem: submitter spins on used-ring advancement.
- The aarch64 audio syscall backend is `sound_mmio.rs`, which also polls on
  used-ring indices when a MMIO sound device exists.

Recommended scope split:

1. Keep `block.rs` and `sound.rs` in scope to satisfy the stated contract and
   clean up x86_64 PCI/test behavior.
2. Add `block_mmio.rs` to the IRQ-completion scope if the operator wants to
   remove production aarch64 QEMU/MMIO block polling.
3. Add `sound_mmio.rs` to the sound scope if the operator wants aarch64 audio
   syscall behavior to match the no-polling standard.
4. Do not claim Parallels stress-gate improvement from changing `block.rs` or
   `sound.rs` alone; saved Parallels logs show those files are not reached for
   storage/audio on current Parallels boots.

## F. Turn 3 proposal

Turn 3 should map the existing interrupt/completion infrastructure for the
actual transport families before implementation:

- PCI path: `virtio::block::handle_interrupt`, static interrupt hookup,
  scheduler wait/wake APIs, and how `gpu_pci.rs` stores/wakes waiters.
- MMIO path: whether `block_mmio.rs` and `sound_mmio.rs` have real IRQ wiring
  today or only synchronous queue submission.
- Decision point for Claude/operator: implement only the contract-named PCI
  files first, or explicitly expand the code-change scope to include the MMIO
  drivers that are the real aarch64 QEMU production paths.

If the next turn authorizes code changes, start with the smallest demonstrable
slice: one block transport, one read request, IRQ drains used ring, blocked
caller wakes through the standard scheduler wake path, and no polling fallback.
