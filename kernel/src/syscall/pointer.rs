//! Pointer and compositor keyboard state (syscall 413).
use super::SyscallResult;

/// sys_get_mouse_pos - The pointer's state, or the compositor's keyboard state
/// (`docs/gui/breengel.md` section 17.6)
///
/// # Arguments
/// * `out_ptr` - The userspace buffer
/// * `len` - 16: `[x, y, buttons, scroll_delta]` as `u32`s (buttons: bit 0 = left button
///   pressed; scroll_delta: accumulated wheel ticks since the last read, positive = up).
///   8: `[modifiers, super_taps]` (modifiers: bit 0 Shift, 1 Ctrl, 2 Alt, 3 Super;
///   super_taps: the Super presses latched since the last read)
///
/// # Returns
/// * 0 on success
/// * -EINVAL for any other length
/// * -EFAULT if the buffer cannot be written; the state stays unread then
#[cfg(target_arch = "aarch64")]
pub fn sys_get_mouse_pos(out_ptr: u64, len: u64) -> SyscallResult {
    if len != 16 && len != 8 {
        return SyscallResult::Err(super::ErrorCode::InvalidArgument as u64);
    }
    // The state below is consumed as it is read: check the buffer first, so a bad one
    // leaves it unread.
    if crate::window::mem::probe_writable(out_ptr, len).is_err() {
        return SyscallResult::Err(super::ErrorCode::Fault as u64);
    }
    if len == 8 {
        let keys = [
            crate::input::poll_modifier_state() as u32,
            crate::input::take_super_tap_count().min(u32::MAX as u64) as u32,
        ];
        return match crate::syscall::userptr::copy_to_user(out_ptr as *mut [u32; 2], &keys) {
            Ok(()) => SyscallResult::Ok(0),
            Err(_) => SyscallResult::Err(super::ErrorCode::Fault as u64),
        };
    }

    let tablet = crate::drivers::virtio::input_mmio::is_tablet_initialized();
    // Test point 17, `move-pointer`: this read finds the device at the armed position, as if
    // it had moved there (docs/gui/breengel.md section 21).
    if let Some(at) = crate::test_control::take(breengel_abi::tc_point::MOVE_POINTER, 0) {
        let (x, y) = (at as u32, (at >> 32) as u32);
        if tablet {
            crate::drivers::virtio::input_mmio::move_to(x, y);
        } else {
            crate::input::pointer::move_to(x, y);
        }
    }
    let (mx, my, buttons) = if tablet {
        crate::drivers::virtio::input_mmio::mouse_state(
            crate::window::Caller::current().map_or(0, |caller| caller.tgid),
        )
    } else {
        crate::input::pointer::mouse_state_consume()
    };

    let scroll = crate::input::pointer::mouse_wheel_consume() as u32;

    // An unmapped or read-only buffer is EFAULT.
    match crate::syscall::userptr::copy_to_user(out_ptr as *mut [u32; 4], &[mx, my, buttons, scroll]) {
        Ok(()) => SyscallResult::Ok(0),
        Err(_) => SyscallResult::Err(super::ErrorCode::Fault as u64),
    }
}
