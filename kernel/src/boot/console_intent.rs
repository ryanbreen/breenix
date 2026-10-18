//! Per-boot console intent for disk-root boots, carried in the bootloader's
//! RAM payload instead of modifying the installed filesystem's boot target.
use core::sync::atomic::{AtomicU8, Ordering};

static INTENT: AtomicU8 = AtomicU8::new(0);

fn parse(bytes: &[u8]) -> Option<bool> {
    match bytes {
        b"BREENIX-CONSOLE 1\nsession\n" => Some(false),
        b"BREENIX-CONSOLE 1\nmeasure\n" => Some(true),
        _ => None,
    }
}

/// Consume a console-intent payload. Filesystem RAM disks are not consumed.
pub fn capture(bytes: &[u8]) -> bool {
    let Some(replug) = parse(bytes) else { return false; };
    INTENT.store(if replug { 2 } else { 1 }, Ordering::Release);
    true
}

/// An explicit per-boot intent overrides an installed target's measurement flag.
pub fn replug_or(target: bool) -> bool {
    match INTENT.load(Ordering::Acquire) {
        1 => false,
        2 => true,
        _ => target,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_boot_intent_overrides_both_kinds_of_installed_target() {
        assert!(!replug_or(false));
        assert!(replug_or(true));
        assert!(capture(b"BREENIX-CONSOLE 1\nsession\n"));
        assert!(!replug_or(true));
        assert!(!replug_or(false));
        assert!(capture(b"BREENIX-CONSOLE 1\nmeasure\n"));
        assert!(replug_or(false));
        assert!(replug_or(true));
        for bytes in [&[0; 2048][..], b"BREENIX-CONSOLE 1\nunknown\n", b"BREENIX-CONSOLE 1\nsession\nextra"] {
            assert!(!capture(bytes));
            assert!(replug_or(false));
        }
    }
}
