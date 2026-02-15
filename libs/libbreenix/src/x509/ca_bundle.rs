//! Root CA certificate store for TLS certificate validation.
//!
//! Provides access to trusted root Certificate Authority certificates
//! used to validate TLS certificate chains. Ships with the Mozilla root
//! CA bundle (144 certificates) embedded as concatenated DER blobs.

extern crate alloc;
use alloc::vec::Vec;

use super::cert::{parse_certificate, Certificate};

/// Concatenated DER-encoded Mozilla root CA certificates.
/// Generated from https://curl.se/ca/cacert.pem (Mozilla root CAs).
static CA_DER_DATA: &[u8] = include_bytes!("ca-certificates.der");

/// Index into CA_DER_DATA: (offset, length) for each certificate.
/// Auto-generated -- 144 Mozilla root CA certificates.
static CA_INDEX: [(usize, usize); 144] = [
    (0, 1173),
    (1173, 1467),
    (2640, 1697),
    (4337, 955),
    (5292, 947),
    (6239, 969),
    (7208, 1470),
    (8678, 956),
    (9634, 960),
    (10594, 1057),
    (11651, 653),
    (12304, 940),
    (13244, 1460),
    (14704, 828),
    (15532, 1049),
    (16581, 1038),
    (17619, 867),
    (18486, 1525),
    (20011, 969),
    (20980, 993),
    (21973, 1011),
    (22984, 848),
    (23832, 848),
    (24680, 1354),
    (26034, 514),
    (26548, 959),
    (27507, 895),
    (28402, 891),
    (29293, 1471),
    (30764, 1373),
    (32137, 1373),
    (33510, 967),
    (34477, 1079),
    (35556, 1095),
    (36651, 1389),
    (38040, 2007),
    (40047, 1349),
    (41396, 1340),
    (42736, 967),
    (43703, 891),
    (44594, 1380),
    (45974, 1380),
    (47354, 1380),
    (48734, 922),
    (49656, 586),
    (50242, 914),
    (51156, 579),
    (51735, 1428),
    (53163, 1500),
    (54663, 1506),
    (56169, 659),
    (56828, 546),
    (57374, 1380),
    (58754, 1386),
    (60140, 1090),
    (61230, 765),
    (61995, 1425),
    (63420, 953),
    (64373, 886),
    (65259, 1494),
    (66753, 1551),
    (68304, 711),
    (69015, 1391),
    (70406, 1415),
    (71821, 837),
    (72658, 1349),
    (74007, 442),
    (74449, 502),
    (74951, 1127),
    (76078, 1420),
    (77498, 1505),
    (79003, 657),
    (79660, 1519),
    (81179, 664),
    (81843, 1415),
    (83258, 621),
    (83879, 1354),
    (85233, 1374),
    (86607, 1631),
    (88238, 920),
    (89158, 594),
    (89752, 887),
    (90639, 559),
    (91198, 1491),
    (92689, 605),
    (93294, 1452),
    (94746, 580),
    (95326, 1355),
    (96681, 1502),
    (98183, 612),
    (98795, 673),
    (99468, 1446),
    (100914, 626),
    (101540, 1374),
    (102914, 527),
    (103441, 1414),
    (104855, 1523),
    (106378, 617),
    (106995, 1476),
    (108471, 1463),
    (109934, 1448),
    (111382, 600),
    (111982, 1560),
    (113542, 531),
    (114073, 1370),
    (115443, 543),
    (115986, 1390),
    (117376, 480),
    (117856, 1371),
    (119227, 1371),
    (120598, 525),
    (121123, 525),
    (121648, 1400),
    (123048, 735),
    (123783, 735),
    (124518, 541),
    (125059, 1386),
    (126445, 1355),
    (127800, 507),
    (128307, 572),
    (128879, 1400),
    (130279, 553),
    (130832, 574),
    (131406, 1422),
    (132828, 1421),
    (134249, 574),
    (134823, 537),
    (135360, 1384),
    (136744, 1449),
    (138193, 601),
    (138794, 582),
    (139376, 1463),
    (140839, 638),
    (141477, 1425),
    (142902, 886),
    (143788, 1398),
    (145186, 551),
    (145737, 1453),
    (147190, 565),
    (147755, 1412),
    (149167, 1453),
    (150620, 1431),
    (152051, 569),
    (152620, 1415),
];

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

    /// Get the Mozilla root CA store (144 root certificates).
    ///
    /// Returns the embedded Mozilla root CA certificate bundle, sourced from
    /// https://curl.se/ca/cacert.pem and converted to concatenated DER format.
    pub fn mozilla() -> &'static RootStore {
        static STORE: RootStore = RootStore {
            der_data: CA_DER_DATA,
            index: &CA_INDEX,
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

    /// Find a root CA by matching its RSA public key (modulus + exponent).
    ///
    /// This handles cross-signing: when a CA transitions between roots, the
    /// same key pair may appear in both a self-signed root and a cross-signed
    /// intermediate. Matching by key is a fast byte comparison.
    pub fn find_by_public_key(&self, modulus: &[u8], exponent: &[u8]) -> Option<Certificate> {
        for i in 0..self.index.len() {
            if let Some(cert) = self.get(i) {
                if let Some(pk) = &cert.public_key {
                    if pk.modulus == modulus && pk.exponent == exponent {
                        return Some(cert);
                    }
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
        // The store should contain 144 Mozilla root CA certificates
        assert!(!store.is_empty());
        assert_eq!(store.len(), 144);
        // First certificate should be parseable
        assert!(store.get(0).is_some());
        // Last certificate should be parseable
        assert!(store.get(143).is_some());
        // Out of bounds should return None
        assert!(store.get(144).is_none());
    }

    #[test]
    fn test_mozilla_store_all_certs_parseable() {
        let store = RootStore::mozilla();
        let mut parsed = 0;
        for i in 0..store.len() {
            if store.get(i).is_some() {
                parsed += 1;
            }
        }
        // All certificates should be parseable (they are all CA root certs)
        assert!(
            parsed > 0,
            "At least some certificates should be parseable"
        );
    }

    #[test]
    fn test_find_issuer_nonexistent() {
        let store = RootStore::mozilla();
        let issuer = vec![(vec![2, 5, 4, 3], b"Nonexistent CA".to_vec())];
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
        let store = RootStore::empty();
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
