//! A boot seed for the kernel's random pool, taken from the firmware before
//! ExitBootServices.
//!
//! Sources, in order:
//! - EFI_RNG_PROTOCOL (UEFI 2.10 section 37.5), where the firmware has one.
//! - The TPM's TPM2_GetRandom (TPM 2.0 Library, Part 3, section 16.1), sent
//!   through EFI_TCG2_PROTOCOL.SubmitCommand (TCG EFI Protocol Specification,
//!   section 6.6.5). A hypervisor's virtual TPM answers it from the host.
//! - A one-use host seed on this loader's ESP, consumed before handoff (S43).
//!
//! Neither VM the loader boots offers CPU RNG instructions or a virtio entropy
//! device, so without this the kernel's pool has no credited source there.

use crate::hw_config::HardwareConfig;
use arm64_boot_contract::{
    boot_seed_file, BOOT_SEED_BYTES, BOOT_SEED_EFI_RNG, BOOT_SEED_HOST_FILE, BOOT_SEED_TPM,
};
use uefi::boot::{self, SearchType};
use uefi::proto::media::file::{File, FileAttribute, FileMode, RegularFile};
use uefi::proto::rng::Rng;
use uefi::proto::tcg::v2::Tcg;

/// TPM_ST_NO_SESSIONS (Part 2, 6.9).
const TPM_ST_NO_SESSIONS: u16 = 0x8001;
/// TPM_CC_GetRandom (Part 2, 6.5.2).
const TPM_CC_GET_RANDOM: u32 = 0x0000_017B;
/// TPM2_GetRandom's command: header (10) and bytesRequested (2).
const GET_RANDOM_COMMAND: usize = 12;
/// Response header: tag (2), responseSize (4), responseCode (4).
const RESPONSE_HEADER: usize = 10;
/// GetRandom calls before giving up on a TPM that returns short answers.
const TPM_ATTEMPTS: usize = 8;

/// Fill `config`'s boot seed from the first source that supplies the full
/// seed. Returns the source, or 0 if unavailable.
pub fn collect(config: &mut HardwareConfig) -> u32 {
    let mut seed = [0u8; BOOT_SEED_BYTES];
    let source = if efi_rng(&mut seed) {
        BOOT_SEED_EFI_RNG
    } else if tpm_get_random(&mut seed) {
        BOOT_SEED_TPM
    } else if host_seed(&mut seed) {
        BOOT_SEED_HOST_FILE
    } else {
        0
    };
    if source != 0 {
        config.boot_seed = seed;
        config.boot_seed_source = source;
    }
    wipe(&mut seed);
    source
}

/// Open the ESP identified by the loaded image, using an existing read/write
/// file. Distributed images are seed-free; a missing seed fails closed.
fn host_seed(out: &mut [u8; BOOT_SEED_BYTES]) -> bool {
    let Ok(mut fs) = boot::get_image_file_system(boot::image_handle()) else {
        return false;
    };
    let Ok(mut root) = fs.open_volume() else {
        return false;
    };
    let Ok(handle) = root.open(
        uefi::cstr16!("\\EFI\\BREENIX\\random-seed"),
        FileMode::ReadWrite,
        FileAttribute::empty(),
    ) else {
        return false;
    };
    let Some(file) = handle.into_regular_file() else {
        return false;
    };
    boot_seed_file::consume(Some(&mut HostSeedFile(file)), out)
}

struct HostSeedFile(RegularFile);

impl boot_seed_file::SeedFile for HostSeedFile {
    fn read(&mut self, bytes: &mut [u8]) -> Option<usize> {
        self.0.read(bytes).ok()
    }
    fn rewind(&mut self) -> bool {
        self.0.set_position(0).is_ok()
    }
    fn write(&mut self, bytes: &[u8]) -> bool {
        self.0.write(bytes).is_ok()
    }
    fn flush(&mut self) -> bool {
        self.0.flush().is_ok()
    }
}

fn efi_rng(out: &mut [u8]) -> bool {
    let Ok(handles) = boot::locate_handle_buffer(SearchType::from_proto::<Rng>()) else {
        return false;
    };
    handles.iter().any(|&handle| {
        boot::open_protocol_exclusive::<Rng>(handle)
            .map(|mut rng| rng.get_rng(None, out).is_ok())
            .unwrap_or(false)
    })
}

fn tpm_get_random(out: &mut [u8]) -> bool {
    let Ok(handles) = boot::locate_handle_buffer(SearchType::from_proto::<Tcg>()) else {
        return false;
    };
    let Some(&handle) = handles.first() else {
        return false;
    };
    let Ok(mut tcg) = boot::open_protocol_exclusive::<Tcg>(handle) else {
        return false;
    };
    let mut filled = 0;
    let mut response = [0u8; RESPONSE_HEADER + 2 + 64];
    for _ in 0..TPM_ATTEMPTS {
        if filled == out.len() {
            break;
        }
        let wanted = (out.len() - filled).min(64) as u16;
        let mut command = [0u8; GET_RANDOM_COMMAND];
        command[0..2].copy_from_slice(&TPM_ST_NO_SESSIONS.to_be_bytes());
        command[2..6].copy_from_slice(&(GET_RANDOM_COMMAND as u32).to_be_bytes());
        command[6..10].copy_from_slice(&TPM_CC_GET_RANDOM.to_be_bytes());
        command[10..12].copy_from_slice(&wanted.to_be_bytes());
        if tcg.submit_command(&command, &mut response).is_err() {
            break;
        }
        let Some(random) = get_random_bytes(&response) else {
            break;
        };
        let take = random.len().min(out.len() - filled);
        out[filled..filled + take].copy_from_slice(&random[..take]);
        filled += take;
    }
    wipe(&mut response);
    filled == out.len()
}

/// The randomBytes of a successful TPM2_GetRandom response: a TPM2B_DIGEST
/// after the header, its size in the first two bytes.
fn get_random_bytes(response: &[u8]) -> Option<&[u8]> {
    let tag = u16::from_be_bytes(response.get(0..2)?.try_into().ok()?);
    let size = u32::from_be_bytes(response.get(2..6)?.try_into().ok()?) as usize;
    let code = u32::from_be_bytes(response.get(6..10)?.try_into().ok()?);
    if tag != TPM_ST_NO_SESSIONS || code != 0 || size > response.len() {
        return None;
    }
    let len = u16::from_be_bytes(response.get(10..12)?.try_into().ok()?) as usize;
    let random = response.get(12..12 + len)?;
    (12 + len <= size && len > 0).then_some(random)
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` is a valid, exclusive reference.
        unsafe { core::ptr::write_volatile(byte, 0) };
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
