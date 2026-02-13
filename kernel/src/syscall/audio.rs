//! Audio-related system calls.
//!
//! Provides syscalls for initializing audio playback and writing PCM data.

use super::SyscallResult;

/// Maximum valid userspace address
const USER_SPACE_MAX: u64 = crate::memory::layout::USER_STACK_REGION_END;

/// sys_audio_init - Initialize the audio stream for playback
///
/// Sets up the VirtIO sound device for S16_LE, 44100 Hz, stereo output.
///
/// # Returns
/// * 0 on success
/// * -EIO on failure
pub fn sys_audio_init() -> SyscallResult {
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
    if buf_ptr == 0 {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    if buf_ptr >= USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    let end = buf_ptr.saturating_add(buf_len);
    if end > USER_SPACE_MAX {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }

    let len = buf_len as usize;
    let data = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };

    #[cfg(target_arch = "aarch64")]
    let result = crate::drivers::virtio::sound_mmio::write_pcm(data);

    #[cfg(target_arch = "x86_64")]
    let result = crate::drivers::virtio::sound::write_pcm(data);

    match result {
        Ok(written) => SyscallResult::Ok(written as u64),
        Err(_) => SyscallResult::Err(super::ErrorCode::IoError as u64),
    }
}
