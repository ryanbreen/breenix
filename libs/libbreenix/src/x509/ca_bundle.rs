//! Root CA certificate store for TLS certificate validation.
//!
//! Provides access to trusted root Certificate Authority certificates
//! used to validate TLS certificate chains. Currently ships with an
//! empty store that can be populated with the Mozilla root CAs.

extern crate alloc;
use alloc::vec::Vec;

use super::cert::{parse_certificate, Certificate};

/// Root CA certificate store
pub struct RootStore {
    /// DER-encoded certificates concatenated together
    der_data: &'static [u8],
    /// Index entries: (offset, length) for each certificate in der_data
    index: &'static [(usize, usize)],
}

impl RootStore {
    /// Create an empty root store (useful for testing)
    pub fn empty() -> RootStore {
        RootStore {
            der_data: &[],
            index: &[],
        }
    }

    /// Get the Mozilla root CA store
    ///
    /// For now this returns an empty store. To populate it:
    /// 1. Download Mozilla's certdata.txt or a PEM bundle
    /// 2. Convert each root CA to DER format
    /// 3. Concatenate into a single blob
    /// 4. Generate the index table
    /// 5. Use include_bytes! to embed the blob
    pub fn mozilla() -> &'static RootStore {
        static STORE: RootStore = RootStore {
            der_data: &[],
            index: &[],
        };
        &STORE
    }

    /// Create a root store from a slice of DER-encoded certificates
    /// (for testing purposes)
    pub fn from_der_certs(
        certs_der: &'static [u8],
        index: &'static [(usize, usize)],
    ) -> RootStore {
        RootStore {
            der_data: certs_der,
            index,
        }
    }

    /// Get the number of root certificates
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Check if the store is empty
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Get a certificate by index
    pub fn get(&self, idx: usize) -> Option<Certificate> {
        let (offset, length) = self.index.get(idx)?;
        let der = self.der_data.get(*offset..*offset + *length)?;
        parse_certificate(der).ok()
    }

    /// Find a root CA certificate that matches the given issuer name
    ///
    /// Searches through all root certificates to find one whose subject
    /// matches the given issuer name.
    pub fn find_issuer(&self, issuer: &[(Vec<u32>, Vec<u8>)]) -> Option<Certificate> {
        for i in 0..self.index.len() {
            if let Some(cert) = self.get(i) {
                if names_equal(&cert.subject, issuer) {
                    return Some(cert);
                }
            }
        }
        None
    }
}

/// Compare two distinguished names for equality
fn names_equal(a: &[(Vec<u32>, Vec<u8>)], b: &[(Vec<u32>, Vec<u8>)]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (av, bv) in a.iter().zip(b.iter()) {
        if av.0 != bv.0 || av.1 != bv.1 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mozilla_store_is_valid() {
        let store = RootStore::mozilla();
        // The store should be valid (currently empty)
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert!(store.get(0).is_none());
    }

    #[test]
    fn test_find_issuer_empty_store() {
        let store = RootStore::mozilla();
        let issuer = vec![(vec![2, 5, 4, 3], b"Example CA".to_vec())];
        assert!(store.find_issuer(&issuer).is_none());
    }

    #[test]
    fn test_from_der_certs_empty() {
        let store = RootStore::from_der_certs(&[], &[]);
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_get_out_of_bounds() {
        let store = RootStore::mozilla();
        assert!(store.get(0).is_none());
        assert!(store.get(100).is_none());
    }

    #[test]
    fn test_names_equal_both_empty() {
        let a: Vec<(Vec<u32>, Vec<u8>)> = vec![];
        let b: Vec<(Vec<u32>, Vec<u8>)> = vec![];
        assert!(names_equal(&a, &b));
    }

    #[test]
    fn test_names_equal_matching() {
        let a = vec![(vec![2, 5, 4, 3], b"Test".to_vec())];
        let b = vec![(vec![2, 5, 4, 3], b"Test".to_vec())];
        assert!(names_equal(&a, &b));
    }

    #[test]
    fn test_names_equal_different_oid() {
        let a = vec![(vec![2, 5, 4, 3], b"Test".to_vec())];
        let b = vec![(vec![2, 5, 4, 6], b"Test".to_vec())];
        assert!(!names_equal(&a, &b));
    }

    #[test]
    fn test_names_equal_different_value() {
        let a = vec![(vec![2, 5, 4, 3], b"Test".to_vec())];
        let b = vec![(vec![2, 5, 4, 3], b"Other".to_vec())];
        assert!(!names_equal(&a, &b));
    }

    #[test]
    fn test_names_equal_different_length() {
        let a = vec![(vec![2, 5, 4, 3], b"Test".to_vec())];
        let b = vec![
            (vec![2, 5, 4, 3], b"Test".to_vec()),
            (vec![2, 5, 4, 6], b"Extra".to_vec()),
        ];
        assert!(!names_equal(&a, &b));
    }
}
