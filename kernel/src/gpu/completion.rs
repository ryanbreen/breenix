//! Target publication after device completion (gpu contract section 1.4.1).

/// Record a failed writer, or recover only from a later successful whole write.
pub fn finish_write(invalid: &mut bool, failed_at: &mut u64, serial: u64, ok: bool, whole: bool) {
    if !ok {
        *invalid = true;
        *failed_at = (*failed_at).max(serial);
    } else if whole && serial > *failed_at {
        *invalid = false;
    }
}
