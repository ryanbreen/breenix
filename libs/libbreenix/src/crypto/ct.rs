//! Byte comparison without content-dependent early exits.

/// Compare byte slices, examining every byte when their lengths match.
/// The lengths themselves are not concealed.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::ct_eq;

    #[test]
    fn equal() {
        assert!(ct_eq(&[1, 2, 3], &[1, 2, 3]));
    }

    #[test]
    fn empty() {
        assert!(ct_eq(&[], &[]));
    }

    #[test]
    fn unequal_lengths() {
        assert!(!ct_eq(&[1, 2], &[1, 2, 3]));
    }

    #[test]
    fn first_byte_differs() {
        assert!(!ct_eq(&[0, 2, 3], &[1, 2, 3]));
    }

    #[test]
    fn last_byte_differs() {
        assert!(!ct_eq(&[1, 2, 0], &[1, 2, 3]));
    }
    #[test]
    fn test_ct_eq_equal() {
        let a = [0x01, 0x02, 0x03, 0x04];
        let b = [0x01, 0x02, 0x03, 0x04];
        assert!(ct_eq(&a, &b));
    }

    #[test]
    fn test_ct_eq_different() {
        let a = [0x01, 0x02, 0x03, 0x04];
        let b = [0x01, 0x02, 0x03, 0x05];
        assert!(!ct_eq(&a, &b));
    }

    #[test]
    fn test_ct_eq_different_lengths() {
        let a = [0x01, 0x02, 0x03];
        let b = [0x01, 0x02, 0x03, 0x04];
        assert!(!ct_eq(&a, &b));
    }

    #[test]
    fn test_ct_eq_empty() {
        let a: [u8; 0] = [];
        let b: [u8; 0] = [];
        assert!(ct_eq(&a, &b));
    }

    #[test]
    fn test_ct_eq_single_bit_difference() {
        let a = [0xFF; 32];
        let mut b = [0xFF; 32];
        // Flip a single bit in the last byte
        b[31] = 0xFE;
        assert!(!ct_eq(&a, &b));
    }
}
