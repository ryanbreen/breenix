//! Audio-related system calls.
//!
//! Provides syscalls for initializing audio playback and writing PCM data.

use alloc::vec::Vec;

use super::{userptr::read_user_bytes, SyscallResult};
use crate::memory::layout::is_valid_user_range;

/// Maximum PCM payload accepted by either VirtIO sound backend per write.
const MAX_PCM_WRITE: usize = 16 * 1024;

/// sys_audio_init - Initialize the audio stream for playback
///
/// Sets up the VirtIO sound device for S16_LE, 44100 Hz, stereo output.
///
/// # Returns
/// * 0 on success
/// * -EIO on failure
pub fn sys_audio_init() -> SyscallResult {
    if let Err(errno) = crate::process::privilege::require(crate::process::privilege::Operation::Audio) {
        return SyscallResult::Err(errno);
    }
    #[cfg(target_arch = "aarch64")]
    let result = crate::drivers::virtio::sound_mmio::setup_stream();

    #[cfg(target_arch = "x86_64")]
    let result = crate::drivers::virtio::sound::setup_stream();

    match result {
        Ok(()) => SyscallResult::Ok(0),
        Err(_) => SyscallResult::Err(super::ErrorCode::IoError as u64),
    }
}

/// sys_audio_write - Write PCM data to the audio device
///
/// # Arguments
/// * `buf_ptr` - Pointer to PCM data buffer in userspace
/// * `buf_len` - Length of data in bytes
///
/// # Returns
/// * Number of bytes written on success
/// * -EFAULT if pointer is invalid
/// * -EIO on device error
pub fn sys_audio_write(buf_ptr: u64, buf_len: u64) -> SyscallResult {
    if let Err(errno) = crate::process::privilege::require(crate::process::privilege::Operation::Audio) {
        return SyscallResult::Err(errno);
    }
    if !is_valid_user_range(buf_ptr, buf_len.max(1) as usize) {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    // The device accepts a short write for larger buffers. Only the bytes
    // submitted in this call need to be mapped and copied.
    let len = (buf_len as usize).min(MAX_PCM_WRITE);
    let mut data = Vec::new();
    if data.try_reserve_exact(len).is_err() {
        return SyscallResult::Err(super::ErrorCode::OutOfMemory as u64);
    }
    data.resize(len, 0);
    if let Err(errno) = read_user_bytes(data.as_mut_ptr(), buf_ptr, len) {
        return SyscallResult::Err(errno);
    }

    #[cfg(target_arch = "aarch64")]
    let result = crate::drivers::virtio::sound_mmio::write_pcm(&data);

    #[cfg(target_arch = "x86_64")]
    let result = crate::drivers::virtio::sound::write_pcm(&data);

    match result {
        Ok(written) => SyscallResult::Ok(written as u64),
        Err(_) => SyscallResult::Err(super::ErrorCode::IoError as u64),
    }
}
