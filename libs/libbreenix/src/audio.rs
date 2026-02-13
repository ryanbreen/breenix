//! Audio playback API
//!
//! Provides userspace access to the VirtIO sound device for PCM audio output.

use crate::error::Error;
use crate::syscall::{nr, raw};

/// Audio sample rate (Hz)
pub const SAMPLE_RATE: u32 = 44100;

/// Number of audio channels (stereo)
pub const CHANNELS: u32 = 2;

/// Initialize the audio device for playback.
///
/// Must be called before `write_pcm()` or `write_samples()`.
pub fn init() -> Result<(), Error> {
    let ret = unsafe { raw::syscall0(nr::AUDIO_INIT) as i64 };
    Error::from_syscall(ret)?;
    Ok(())
}

/// Write raw PCM data to the audio device.
///
/// Data must be S16_LE stereo at 44100 Hz.
/// Maximum 16384 bytes per call.
pub fn write_pcm(data: &[u8]) -> Result<usize, Error> {
    let ret = unsafe {
        raw::syscall2(nr::AUDIO_WRITE, data.as_ptr() as u64, data.len() as u64) as i64
    };
    Error::from_syscall(ret).map(|v| v as usize)
}

/// Write 16-bit audio samples to the device.
///
/// Samples should be interleaved stereo (L, R, L, R, ...).
pub fn write_samples(samples: &[i16]) -> Result<usize, Error> {
    let byte_len = samples.len() * 2;
    let ptr = samples.as_ptr() as *const u8;
    let data = unsafe { core::slice::from_raw_parts(ptr, byte_len) };
    write_pcm(data)
}
