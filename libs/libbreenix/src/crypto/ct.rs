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
}
