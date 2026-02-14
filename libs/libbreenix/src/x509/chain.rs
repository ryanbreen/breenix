//! X.509 certificate chain validation for TLS.
//!
//! Validates that a chain of certificates (leaf + intermediates) leads to a
//! trusted root CA, that hostnames match, that validity periods are current,
//! and that all signatures are cryptographically correct.

extern crate alloc;

use alloc::vec::Vec;

use super::ca_bundle::RootStore;
use super::cert::{Certificate, OID_SHA256_WITH_RSA, RsaPublicKeyInfo};
use crate::crypto::bignum::BigNum;
use crate::crypto::rsa::{rsa_verify_pkcs1_sha256, RsaPublicKey};
use crate::crypto::sha256::sha256;

/// Errors that can occur during certificate chain validation.
#[derive(Debug, Clone)]
pub enum ChainError {
    /// The certificate chain is empty (no certificates provided).
    EmptyChain,
    /// The leaf certificate's SAN DNS names do not match the requested hostname.
    HostnameMismatch,
    /// A certificate in the chain has expired (notAfter < now).
    Expired,
    /// A certificate in the chain is not yet valid (notBefore > now).
    NotYetValid,
    /// A certificate's signature could not be verified with its issuer's public key.
    SignatureVerificationFailed,
    /// No issuer could be found for a certificate in the chain.
    IssuerNotFound,
    /// An intermediate certificate does not have basicConstraints.cA = true.
    NotACa,
    /// The certificate uses a signature algorithm other than SHA-256 with RSA.
    UnsupportedSignatureAlgorithm,
    /// The issuer's public key could not be parsed or is malformed.
    InvalidPublicKey,
    /// A certificate could not be parsed from the DER data.
    ParseError,
}

/// Validate a certificate chain against a hostname and root store.
///
/// `chain[0]` is the leaf (server) certificate, and `chain[1..]` are
/// intermediate certificates provided by the server.
///
/// Validates:
/// 1. Hostname matches leaf certificate's SAN DNS names
/// 2. Each certificate is within its validity period
/// 3. Each certificate's signature is valid (verified with issuer's public key)
/// 4. Chain leads to a trusted root CA
/// 5. Intermediate certs have basicConstraints.cA = true
pub fn validate_chain(
    chain: &[Certificate],
    hostname: &str,
    root_store: &RootStore,
    now: u64,
) -> Result<(), ChainError> {
    if chain.is_empty() {
        return Err(ChainError::EmptyChain);
    }

    // Step 1: Verify hostname matches the leaf certificate.
    let leaf = &chain[0];
    if !hostname_matches(leaf, hostname) {
        return Err(ChainError::HostnameMismatch);
    }

    // Step 2: Walk the chain from leaf to root.
    // At each step, find the issuer of the current certificate, verify the
    // signature, check validity, and check CA constraints on intermediates.
    let mut current = leaf;
    let mut depth = 0;

    loop {
        // Check validity period for the current certificate.
        if now < current.not_before {
            return Err(ChainError::NotYetValid);
        }
        if now > current.not_after {
            return Err(ChainError::Expired);
        }

        // Try to find the issuer of the current certificate.
        // First, check the root store (this also handles the case where the
        // leaf or an intermediate is directly signed by a root).
        if let Some(root_cert) = root_store.find_issuer(&current.issuer) {
            // Verify the signature on current using the root's public key.
            verify_signature(current, &root_cert)?;
            // Chain terminates at a trusted root -- success.
            return Ok(());
        }

        // Next, search the remaining chain certificates for the issuer.
        let issuer = find_issuer_in_chain(current, chain, depth)?;

        // Verify the signature on current using the issuer's public key.
        verify_signature(current, issuer)?;

        // Intermediate certificates must have cA = true.
        if !issuer.is_ca {
            return Err(ChainError::NotACa);
        }

        // Move up the chain.
        current = issuer;
        depth += 1;

        // Safety check: prevent infinite loops in malformed chains.
        if depth > chain.len() {
            return Err(ChainError::IssuerNotFound);
        }
    }
}

/// Skip certificate validation (insecure mode).
///
/// This always returns `Ok(())` regardless of the chain contents, equivalent
/// to curl's `-k` / `--insecure` flag. Use only for debugging or when
/// certificate validation is handled out-of-band.
pub fn validate_chain_insecure(
    _chain: &[Certificate],
    _hostname: &str,
) -> Result<(), ChainError> {
    Ok(())
}

/// Check if a hostname matches any SAN DNS name in the certificate.
///
/// Supports exact match and wildcard matching (`*.example.com`).
/// Wildcards must be in the leftmost label only and match exactly one label
/// (i.e., `*.example.com` matches `foo.example.com` but not
/// `foo.bar.example.com`).
///
/// All comparisons are case-insensitive per RFC 6125.
fn hostname_matches(cert: &Certificate, hostname: &str) -> bool {
    let hostname_lower = hostname.to_ascii_lowercase();

    for san in &cert.san_dns_names {
        // Convert the SAN bytes to a lowercase string for comparison
        let san_str = match core::str::from_utf8(san) {
            Ok(s) => s.to_ascii_lowercase(),
            Err(_) => continue,
        };

        if san_str == hostname_lower {
            // Exact match.
            return true;
        }

        // Check for wildcard match: pattern must start with "*."
        if let Some(wildcard_suffix) = san_str.strip_prefix("*.") {
            // The wildcard covers exactly one label. The hostname must have
            // the form "<label>.<wildcard_suffix>" where <label> contains no
            // dots.
            if let Some(rest) = strip_first_label(&hostname_lower) {
                if rest == wildcard_suffix {
                    return true;
                }
            }
        }
    }

    false
}

/// Strip the first DNS label from a hostname, returning everything after the
/// first dot. Returns `None` if there is no dot (single-label hostname).
fn strip_first_label(hostname: &str) -> Option<&str> {
    let dot_pos = hostname.find('.')?;
    // The part before the dot must be non-empty (a valid label).
    if dot_pos == 0 {
        return None;
    }
    Some(&hostname[dot_pos + 1..])
}

/// Verify that `cert` was signed by `issuer`.
///
/// Checks that the signature algorithm is SHA-256 with RSA, hashes the
/// TBS (to-be-signed) portion of the certificate, extracts the issuer's
/// RSA public key, and verifies the PKCS#1 v1.5 signature.
fn verify_signature(cert: &Certificate, issuer: &Certificate) -> Result<(), ChainError> {
    // Only SHA-256 with RSA is supported.
    if cert.signature_algorithm != OID_SHA256_WITH_RSA {
        return Err(ChainError::UnsupportedSignatureAlgorithm);
    }

    // Hash the TBS (to-be-signed) certificate data with SHA-256.
    let tbs_hash = sha256(&cert.tbs_raw);

    // Extract the issuer's RSA public key.
    let pk_info = issuer.public_key.as_ref().ok_or(ChainError::InvalidPublicKey)?;
    let rsa_key = extract_rsa_public_key(pk_info)?;

    // Verify the signature.
    if !rsa_verify_pkcs1_sha256(&rsa_key, &cert.signature, &tbs_hash) {
        return Err(ChainError::SignatureVerificationFailed);
    }

    Ok(())
}

/// Extract an RSA public key from the certificate's SubjectPublicKeyInfo.
fn extract_rsa_public_key(pk_info: &RsaPublicKeyInfo) -> Result<RsaPublicKey, ChainError> {
    if pk_info.modulus.is_empty() || pk_info.exponent.is_empty() {
        return Err(ChainError::InvalidPublicKey);
    }

    let n = BigNum::from_be_bytes(&pk_info.modulus);
    let e = BigNum::from_be_bytes(&pk_info.exponent);

    if n.is_zero() {
        return Err(ChainError::InvalidPublicKey);
    }

    Ok(RsaPublicKey { n, e })
}

/// Find the issuer of `cert` among the chain certificates.
///
/// Searches `chain` for a certificate whose subject matches `cert.issuer`,
/// skipping the certificate at index `current_depth` (which is `cert` itself).
fn find_issuer_in_chain<'a>(
    cert: &Certificate,
    chain: &'a [Certificate],
    current_depth: usize,
) -> Result<&'a Certificate, ChainError> {
    for (i, candidate) in chain.iter().enumerate() {
        // Don't match a certificate against itself.
        if i == current_depth {
            continue;
        }
        if names_match(&cert.issuer, &candidate.subject) {
            return Ok(candidate);
        }
    }
    Err(ChainError::IssuerNotFound)
}

/// Check if two distinguished names (sequences of (OID, value) pairs) match.
///
/// Two names match if they contain the same number of (OID, value) pairs and
/// each pair is identical (same OID, same value bytes).
fn names_match(a: &[(Vec<u32>, Vec<u8>)], b: &[(Vec<u32>, Vec<u8>)]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (pair_a, pair_b) in a.iter().zip(b.iter()) {
        if pair_a.0 != pair_b.0 || pair_a.1 != pair_b.1 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // hostname_matches tests
    // -----------------------------------------------------------------------

    /// Helper to create a minimal Certificate with only san_dns_names populated.
    fn cert_with_sans(sans: &[&str]) -> Certificate {
        Certificate {
            tbs_raw: Vec::new(),
            version: 0,
            serial: Vec::new(),
            sig_algorithm: Vec::new(),
            subject: Vec::new(),
            issuer: Vec::new(),
            not_before: 0,
            not_after: u64::MAX,
            public_key: None,
            signature_algorithm: Vec::new(),
            signature: Vec::new(),
            san_dns_names: sans.iter().map(|s| s.as_bytes().to_vec()).collect(),
            is_ca: false,
        }
    }

    #[test]
    fn test_hostname_exact_match() {
        let cert = cert_with_sans(&["www.example.com"]);
        assert!(hostname_matches(&cert, "www.example.com"));
    }

    #[test]
    fn test_hostname_exact_match_case_insensitive() {
        let cert = cert_with_sans(&["WWW.Example.COM"]);
        assert!(hostname_matches(&cert, "www.example.com"));
    }

    #[test]
    fn test_hostname_no_match() {
        let cert = cert_with_sans(&["www.example.com"]);
        assert!(!hostname_matches(&cert, "mail.example.com"));
    }

    #[test]
    fn test_hostname_wildcard_match() {
        let cert = cert_with_sans(&["*.example.com"]);
        assert!(hostname_matches(&cert, "foo.example.com"));
        assert!(hostname_matches(&cert, "bar.example.com"));
        assert!(hostname_matches(&cert, "WWW.example.com"));
    }

    #[test]
    fn test_hostname_wildcard_no_subdomain_match() {
        // Wildcard should NOT match a deeper subdomain.
        let cert = cert_with_sans(&["*.example.com"]);
        assert!(!hostname_matches(&cert, "foo.bar.example.com"));
    }

    #[test]
    fn test_hostname_wildcard_no_bare_domain_match() {
        // Wildcard *.example.com should NOT match example.com itself.
        let cert = cert_with_sans(&["*.example.com"]);
        assert!(!hostname_matches(&cert, "example.com"));
    }

    #[test]
    fn test_hostname_wildcard_case_insensitive() {
        let cert = cert_with_sans(&["*.Example.COM"]);
        assert!(hostname_matches(&cert, "foo.example.com"));
    }

    #[test]
    fn test_hostname_multiple_sans() {
        let cert = cert_with_sans(&["example.com", "*.example.com", "example.org"]);
        assert!(hostname_matches(&cert, "example.com"));
        assert!(hostname_matches(&cert, "www.example.com"));
        assert!(hostname_matches(&cert, "example.org"));
        assert!(!hostname_matches(&cert, "example.net"));
    }

    #[test]
    fn test_hostname_empty_sans() {
        let cert = cert_with_sans(&[]);
        assert!(!hostname_matches(&cert, "anything.com"));
    }

    // -----------------------------------------------------------------------
    // names_match tests
    // -----------------------------------------------------------------------

    /// OID for commonName (2.5.4.3)
    fn oid_cn() -> Vec<u32> {
        vec![2, 5, 4, 3]
    }

    /// OID for organizationName (2.5.4.10)
    fn oid_o() -> Vec<u32> {
        vec![2, 5, 4, 10]
    }

    #[test]
    fn test_names_match_identical() {
        let a = vec![
            (oid_cn(), b"Example CA".to_vec()),
            (oid_o(), b"Example Inc".to_vec()),
        ];
        let b = vec![
            (oid_cn(), b"Example CA".to_vec()),
            (oid_o(), b"Example Inc".to_vec()),
        ];
        assert!(names_match(&a, &b));
    }

    #[test]
    fn test_names_match_different_values() {
        let a = vec![(oid_cn(), b"Example CA".to_vec())];
        let b = vec![(oid_cn(), b"Other CA".to_vec())];
        assert!(!names_match(&a, &b));
    }

    #[test]
    fn test_names_match_different_oids() {
        let a = vec![(oid_cn(), b"Example CA".to_vec())];
        let b = vec![(oid_o(), b"Example CA".to_vec())];
        assert!(!names_match(&a, &b));
    }

    #[test]
    fn test_names_match_different_lengths() {
        let a = vec![
            (oid_cn(), b"Example CA".to_vec()),
            (oid_o(), b"Example Inc".to_vec()),
        ];
        let b = vec![(oid_cn(), b"Example CA".to_vec())];
        assert!(!names_match(&a, &b));
    }

    #[test]
    fn test_names_match_empty() {
        let a: Vec<(Vec<u32>, Vec<u8>)> = Vec::new();
        let b: Vec<(Vec<u32>, Vec<u8>)> = Vec::new();
        assert!(names_match(&a, &b));
    }

    // -----------------------------------------------------------------------
    // strip_first_label tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_strip_first_label_normal() {
        assert_eq!(strip_first_label("foo.example.com"), Some("example.com"));
    }

    #[test]
    fn test_strip_first_label_single() {
        assert_eq!(strip_first_label("localhost"), None);
    }

    #[test]
    fn test_strip_first_label_leading_dot() {
        assert_eq!(strip_first_label(".example.com"), None);
    }

    #[test]
    fn test_strip_first_label_two_labels() {
        assert_eq!(strip_first_label("example.com"), Some("com"));
    }

    // -----------------------------------------------------------------------
    // validate_chain_insecure tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_chain_insecure_always_ok() {
        assert!(validate_chain_insecure(&[], "anything.com").is_ok());
    }

    // -----------------------------------------------------------------------
    // validate_chain error case tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_chain_empty_chain() {
        let store = RootStore::empty();
        let result = validate_chain(&[], "example.com", &store, 1_700_000_000);
        assert!(matches!(result, Err(ChainError::EmptyChain)));
    }

    #[test]
    fn test_validate_chain_hostname_mismatch() {
        let store = RootStore::empty();
        let cert = cert_with_sans(&["other.com"]);
        let result = validate_chain(&[cert], "example.com", &store, 1_700_000_000);
        assert!(matches!(result, Err(ChainError::HostnameMismatch)));
    }
}
