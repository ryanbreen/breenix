//! X.509 v3 certificate parsing (RFC 5280).
//!
//! Parses DER-encoded X.509 certificates, extracting the fields needed for
//! TLS certificate chain validation: subject, issuer, public key, extensions
//! (SAN, Basic Constraints), and signature.
//!
//! Only RSA public keys are currently supported.

extern crate alloc;

use alloc::vec::Vec;

use super::asn1::{Asn1Error, DerParser, TAG_CONTEXT_0, TAG_CONTEXT_3};

// ---------------------------------------------------------------------------
// OID constants
// ---------------------------------------------------------------------------

// Signature algorithms
pub const OID_SHA256_WITH_RSA: &[u32] = &[1, 2, 840, 113549, 1, 1, 11];
pub const OID_SHA384_WITH_RSA: &[u32] = &[1, 2, 840, 113549, 1, 1, 12];
pub const OID_SHA1_WITH_RSA: &[u32] = &[1, 2, 840, 113549, 1, 1, 5];

// Key types
pub const OID_RSA_ENCRYPTION: &[u32] = &[1, 2, 840, 113549, 1, 1, 1];

// Extensions
pub const OID_SUBJECT_ALT_NAME: &[u32] = &[2, 5, 29, 17];
pub const OID_BASIC_CONSTRAINTS: &[u32] = &[2, 5, 29, 19];
pub const OID_KEY_USAGE: &[u32] = &[2, 5, 29, 15];

// Attribute types
pub const OID_COMMON_NAME: &[u32] = &[2, 5, 4, 3];
pub const OID_ORGANIZATION: &[u32] = &[2, 5, 4, 10];
pub const OID_COUNTRY: &[u32] = &[2, 5, 4, 6];

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum CertParseError {
    Asn1(Asn1Error),
    UnsupportedKeyType,
    InvalidCertificate,
}

impl From<Asn1Error> for CertParseError {
    fn from(e: Asn1Error) -> Self {
        CertParseError::Asn1(e)
    }
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Certificate {
    /// Raw TBS (To Be Signed) certificate bytes -- for signature verification
    pub tbs_raw: Vec<u8>,
    /// Certificate version (0 = v1, 2 = v3)
    pub version: u8,
    /// Serial number (raw bytes)
    pub serial: Vec<u8>,
    /// Signature algorithm OID
    pub sig_algorithm: Vec<u32>,
    /// Issuer distinguished name (list of (OID, value) pairs)
    pub issuer: Vec<(Vec<u32>, Vec<u8>)>,
    /// Subject distinguished name
    pub subject: Vec<(Vec<u32>, Vec<u8>)>,
    /// Not Before (Unix timestamp)
    pub not_before: u64,
    /// Not After (Unix timestamp)
    pub not_after: u64,
    /// Subject public key (RSA modulus n, exponent e)
    pub public_key: Option<RsaPublicKeyInfo>,
    /// Subject Alternative Names (DNS names)
    pub san_dns_names: Vec<Vec<u8>>,
    /// Is this a CA certificate?
    pub is_ca: bool,
    /// Signature algorithm OID (from outer Certificate SEQUENCE)
    pub signature_algorithm: Vec<u32>,
    /// Signature value
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RsaPublicKeyInfo {
    /// RSA modulus (big-endian bytes, may have leading zero)
    pub modulus: Vec<u8>,
    /// RSA exponent (big-endian bytes)
    pub exponent: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Name parsing
// ---------------------------------------------------------------------------

/// Parse a Name (SEQUENCE OF SET OF AttributeTypeAndValue).
///
/// Each SET contains a SEQUENCE of { OID, ANY (string value) }.
/// We store each attribute as `(oid_components, raw_value_bytes)`.
fn parse_name(parser: &mut DerParser) -> Result<Vec<(Vec<u32>, Vec<u8>)>, CertParseError> {
    let mut name_parser = parser.read_sequence()?;
    let mut attrs: Vec<(Vec<u32>, Vec<u8>)> = Vec::new();

    while !name_parser.is_empty() {
        // Each RDN is a SET
        let mut set_parser = name_parser.read_set()?;

        while !set_parser.is_empty() {
            // Each AttributeTypeAndValue is a SEQUENCE { OID, ANY }
            let mut atv_parser = set_parser.read_sequence()?;

            let oid = atv_parser.read_oid()?;
            // Read the value as raw bytes regardless of its string type
            let (_tag, value) = atv_parser.read_any()?;
            attrs.push((oid, value.to_vec()));
        }
    }

    Ok(attrs)
}

// ---------------------------------------------------------------------------
// Time parsing
// ---------------------------------------------------------------------------

/// Parse a Time value (UTCTime or GeneralizedTime) into a Unix timestamp.
///
/// UTCTime (tag 0x17): YYMMDDHHMMSSZ
///   - If YY >= 50 then 19YY, otherwise 20YY (per RFC 5280 Section 4.1.2.5.1)
///
/// GeneralizedTime (tag 0x18): YYYYMMDDHHMMSSZ
fn parse_time(parser: &mut DerParser) -> Result<u64, CertParseError> {
    let (tag, data) = parser.read_any()?;

    let s = core::str::from_utf8(data).map_err(|_| CertParseError::InvalidCertificate)?;

    let (year, rest) = match tag {
        0x17 => {
            // UTCTime: YYMMDDHHMMSSZ
            if s.len() < 13 {
                return Err(CertParseError::InvalidCertificate);
            }
            let yy: u64 = s[0..2]
                .parse()
                .map_err(|_| CertParseError::InvalidCertificate)?;
            let year = if yy >= 50 { 1900 + yy } else { 2000 + yy };
            (year, &s[2..])
        }
        0x18 => {
            // GeneralizedTime: YYYYMMDDHHMMSSZ
            if s.len() < 15 {
                return Err(CertParseError::InvalidCertificate);
            }
            let year: u64 = s[0..4]
                .parse()
                .map_err(|_| CertParseError::InvalidCertificate)?;
            (year, &s[4..])
        }
        _ => return Err(CertParseError::InvalidCertificate),
    };

    let month: u64 = rest[0..2]
        .parse()
        .map_err(|_| CertParseError::InvalidCertificate)?;
    let day: u64 = rest[2..4]
        .parse()
        .map_err(|_| CertParseError::InvalidCertificate)?;
    let hour: u64 = rest[4..6]
        .parse()
        .map_err(|_| CertParseError::InvalidCertificate)?;
    let minute: u64 = rest[6..8]
        .parse()
        .map_err(|_| CertParseError::InvalidCertificate)?;
    let second: u64 = rest[8..10]
        .parse()
        .map_err(|_| CertParseError::InvalidCertificate)?;

    // Convert to Unix timestamp (seconds since 1970-01-01 00:00:00 UTC)
    Ok(datetime_to_unix(year, month, day, hour, minute, second))
}

/// Convert a date/time to a Unix timestamp.
///
/// Uses a straightforward calculation with no leap-second awareness, which is
/// sufficient for certificate validity checking.
fn datetime_to_unix(year: u64, month: u64, day: u64, hour: u64, minute: u64, second: u64) -> u64 {
    // Days in each month (non-leap year)
    const DAYS_BEFORE_MONTH: [u64; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

    let mut days: u64 = 0;

    // Years since epoch
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }

    // Months in current year
    if month > 1 {
        days += DAYS_BEFORE_MONTH[month as usize];
        if month > 2 && is_leap_year(year) {
            days += 1;
        }
    }

    // Days in current month (1-indexed)
    days += day - 1;

    days * 86400 + hour * 3600 + minute * 60 + second
}

/// Check if a year is a leap year.
fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// ---------------------------------------------------------------------------
// Algorithm identifier parsing
// ---------------------------------------------------------------------------

/// Parse an AlgorithmIdentifier SEQUENCE { OID, optional parameters }.
/// Returns just the OID components.
fn parse_algorithm_identifier(parser: &mut DerParser) -> Result<Vec<u32>, CertParseError> {
    let mut alg_parser = parser.read_sequence()?;
    let oid = alg_parser.read_oid()?;
    // Skip optional parameters (NULL, or algorithm-specific)
    Ok(oid)
}

// ---------------------------------------------------------------------------
// Public key parsing
// ---------------------------------------------------------------------------

/// Parse SubjectPublicKeyInfo and extract RSA key material.
///
/// SubjectPublicKeyInfo ::= SEQUENCE {
///   algorithm  AlgorithmIdentifier,
///   subjectPublicKey  BIT STRING
/// }
///
/// For RSA, the BIT STRING contains the DER encoding of:
///   RSAPublicKey ::= SEQUENCE {
///     modulus          INTEGER,
///     publicExponent   INTEGER
///   }
fn parse_subject_public_key_info(
    parser: &mut DerParser,
) -> Result<Option<RsaPublicKeyInfo>, CertParseError> {
    let mut spki_parser = parser.read_sequence()?;

    // Algorithm identifier
    let key_alg = parse_algorithm_identifier(&mut spki_parser)?;

    // BIT STRING containing the public key
    let bit_string = spki_parser.read_bit_string()?;

    // Only RSA keys are supported
    if key_alg != OID_RSA_ENCRYPTION {
        return Ok(None);
    }

    // The bit string content (after the unused-bits byte, which read_bit_string
    // already strips) is the DER-encoded RSAPublicKey SEQUENCE.
    let mut key_parser = DerParser::new(bit_string);
    let mut seq_parser = key_parser.read_sequence()?;

    let modulus = seq_parser.read_integer()?.to_vec();
    let exponent = seq_parser.read_integer()?.to_vec();

    Ok(Some(RsaPublicKeyInfo { modulus, exponent }))
}

// ---------------------------------------------------------------------------
// Extension parsing
// ---------------------------------------------------------------------------

/// Parse the extensions section of a v3 certificate.
///
/// Extensions ::= SEQUENCE SIZE (1..MAX) OF Extension
/// Extension ::= SEQUENCE {
///   extnID     OID,
///   critical   BOOLEAN DEFAULT FALSE,
///   extnValue  OCTET STRING (contains DER-encoded extension value)
/// }
fn parse_extensions(
    data: &[u8],
    san_dns_names: &mut Vec<Vec<u8>>,
    is_ca: &mut bool,
) -> Result<(), CertParseError> {
    let mut ext_seq_parser = DerParser::new(data);
    let mut parser = ext_seq_parser.read_sequence()?;

    while !parser.is_empty() {
        let mut ext_parser = parser.read_sequence()?;

        let oid = ext_parser.read_oid()?;

        // Optional critical BOOLEAN -- peek at the next tag
        if !ext_parser.is_empty() && ext_parser.peek_tag()? == 0x01 {
            // BOOLEAN tag -- read and discard the critical flag
            let _critical = ext_parser.read_any()?;
        }

        // extnValue is an OCTET STRING wrapping the actual extension DER
        let value_data = ext_parser.read_octet_string()?;

        if oid == OID_SUBJECT_ALT_NAME {
            parse_san_extension(value_data, san_dns_names)?;
        } else if oid == OID_BASIC_CONSTRAINTS {
            parse_basic_constraints(value_data, is_ca)?;
        }
        // Other extensions are silently ignored.
    }

    Ok(())
}

/// Parse Subject Alternative Name extension value.
///
/// SubjectAltName ::= GeneralNames
/// GeneralNames ::= SEQUENCE SIZE (1..MAX) OF GeneralName
/// GeneralName ::= CHOICE {
///   ...
///   dNSName  [2] IA5String,
///   ...
/// }
fn parse_san_extension(
    data: &[u8],
    san_dns_names: &mut Vec<Vec<u8>>,
) -> Result<(), CertParseError> {
    let mut parser = DerParser::new(data);
    let mut seq_parser = parser.read_sequence()?;

    while !seq_parser.is_empty() {
        let (tag, value) = seq_parser.read_any()?;
        // dNSName is [2] IMPLICIT IA5String — context-specific primitive tag
        // 0x82 = context class (10) + primitive (0) + tag number 2 (00010)
        if tag == 0x82 {
            san_dns_names.push(value.to_vec());
        }
        // Other GeneralName types are ignored.
    }

    Ok(())
}

/// Parse Basic Constraints extension value.
///
/// BasicConstraints ::= SEQUENCE {
///   cA                 BOOLEAN DEFAULT FALSE,
///   pathLenConstraint  INTEGER (0..MAX) OPTIONAL
/// }
fn parse_basic_constraints(data: &[u8], is_ca: &mut bool) -> Result<(), CertParseError> {
    let mut parser = DerParser::new(data);
    let mut seq_parser = parser.read_sequence()?;

    if seq_parser.is_empty() {
        // Empty sequence means cA defaults to false
        *is_ca = false;
        return Ok(());
    }

    // Check if first element is a BOOLEAN
    if !seq_parser.is_empty() && seq_parser.peek_tag()? == 0x01 {
        let (_tag, bool_data) = seq_parser.read_any()?;
        *is_ca = !bool_data.is_empty() && bool_data[0] != 0x00;
    } else {
        *is_ca = false;
    }
    // pathLenConstraint is optional and we don't need it.

    Ok(())
}

// ---------------------------------------------------------------------------
// Certificate parsing
// ---------------------------------------------------------------------------

/// Parse a DER-encoded X.509 certificate.
///
/// Follows RFC 5280 structure:
///
/// ```text
/// Certificate ::= SEQUENCE {
///   tbsCertificate      TBSCertificate,
///   signatureAlgorithm  AlgorithmIdentifier,
///   signatureValue      BIT STRING
/// }
/// ```
pub fn parse_certificate(der: &[u8]) -> Result<Certificate, CertParseError> {
    let mut outer = DerParser::new(der);
    let mut cert_parser = outer.read_sequence()?;

    // -----------------------------------------------------------------------
    // 1. Capture the raw TBS bytes (tag + length + value) for signature
    //    verification. We read the raw TLV first, then parse its contents.
    // -----------------------------------------------------------------------
    let tbs_raw = cert_parser.read_raw_tlv()?.to_vec();
    let mut tbs = {
        let mut tbs_outer = DerParser::new(&tbs_raw);
        tbs_outer.read_sequence()?
    };

    // -----------------------------------------------------------------------
    // 2. Parse TBSCertificate fields
    // -----------------------------------------------------------------------

    // Version: [0] EXPLICIT INTEGER (optional, default v1 = 0)
    let version = if !tbs.is_empty() && tbs.peek_tag()? == TAG_CONTEXT_0 {
        let ver_data = tbs.read_context(0)?;
        let mut ver_parser = DerParser::new(ver_data);
        let ver_bytes = ver_parser.read_integer()?;
        if ver_bytes.is_empty() {
            0u8
        } else {
            ver_bytes[ver_bytes.len() - 1]
        }
    } else {
        0u8 // Default: v1
    };

    // Serial number
    let serial = tbs.read_integer()?.to_vec();

    // Signature algorithm (inside TBS)
    let sig_algorithm = parse_algorithm_identifier(&mut tbs)?;

    // Issuer
    let issuer = parse_name(&mut tbs)?;

    // Validity
    let mut validity_parser = tbs.read_sequence()?;
    let not_before = parse_time(&mut validity_parser)?;
    let not_after = parse_time(&mut validity_parser)?;

    // Subject
    let subject = parse_name(&mut tbs)?;

    // SubjectPublicKeyInfo
    let public_key = parse_subject_public_key_info(&mut tbs)?;

    // Extensions: [3] EXPLICIT SEQUENCE OF Extension (only in v3)
    let mut san_dns_names: Vec<Vec<u8>> = Vec::new();
    let mut is_ca = false;

    if version == 2 {
        // There may be optional issuerUniqueID [1] and subjectUniqueID [2]
        // before the extensions. Skip them if present.
        while !tbs.is_empty() {
            let tag = tbs.peek_tag()?;
            if tag == TAG_CONTEXT_3 {
                let ext_wrapper = tbs.read_context(3)?;
                parse_extensions(ext_wrapper, &mut san_dns_names, &mut is_ca)?;
                break;
            } else if tag == (0x80 | 0x01) || tag == (0x80 | 0x02) {
                // issuerUniqueID [1] or subjectUniqueID [2] -- skip
                let _skipped = tbs.read_any()?;
            } else {
                break;
            }
        }
    }

    // -----------------------------------------------------------------------
    // 3. Outer signature algorithm and value
    // -----------------------------------------------------------------------
    let signature_algorithm = parse_algorithm_identifier(&mut cert_parser)?;

    let signature_bits = cert_parser.read_bit_string()?;
    // The signature BIT STRING content is the raw signature bytes.
    let signature = signature_bits.to_vec();

    Ok(Certificate {
        tbs_raw,
        version,
        serial,
        sig_algorithm,
        issuer,
        subject,
        not_before,
        not_after,
        public_key,
        san_dns_names,
        is_ca,
        signature_algorithm,
        signature,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the Common Name from a distinguished name.
///
/// Searches the list of (OID, value) pairs for OID 2.5.4.3 (id-at-commonName)
/// and returns the raw bytes of its value.
pub fn get_common_name(name: &[(Vec<u32>, Vec<u8>)]) -> Option<&[u8]> {
    for (oid, value) in name {
        if oid.as_slice() == OID_COMMON_NAME {
            return Some(value.as_slice());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_oid_sha256_with_rsa() {
        assert_eq!(OID_SHA256_WITH_RSA, &[1, 2, 840, 113549, 1, 1, 11]);
    }

    #[test]
    fn test_oid_sha384_with_rsa() {
        assert_eq!(OID_SHA384_WITH_RSA, &[1, 2, 840, 113549, 1, 1, 12]);
    }

    #[test]
    fn test_oid_sha1_with_rsa() {
        assert_eq!(OID_SHA1_WITH_RSA, &[1, 2, 840, 113549, 1, 1, 5]);
    }

    #[test]
    fn test_oid_rsa_encryption() {
        assert_eq!(OID_RSA_ENCRYPTION, &[1, 2, 840, 113549, 1, 1, 1]);
    }

    #[test]
    fn test_oid_subject_alt_name() {
        assert_eq!(OID_SUBJECT_ALT_NAME, &[2, 5, 29, 17]);
    }

    #[test]
    fn test_oid_basic_constraints() {
        assert_eq!(OID_BASIC_CONSTRAINTS, &[2, 5, 29, 19]);
    }

    #[test]
    fn test_oid_key_usage() {
        assert_eq!(OID_KEY_USAGE, &[2, 5, 29, 15]);
    }

    #[test]
    fn test_oid_common_name() {
        assert_eq!(OID_COMMON_NAME, &[2, 5, 4, 3]);
    }

    #[test]
    fn test_oid_organization() {
        assert_eq!(OID_ORGANIZATION, &[2, 5, 4, 10]);
    }

    #[test]
    fn test_oid_country() {
        assert_eq!(OID_COUNTRY, &[2, 5, 4, 6]);
    }

    /// Hand-craft a DER-encoded Name with a single RDN containing a Common Name
    /// attribute, then verify parse_name extracts it correctly.
    ///
    /// The DER structure is:
    ///   SEQUENCE {                -- Name (SEQUENCE OF SET)
    ///     SET {                   -- RelativeDistinguishedName
    ///       SEQUENCE {            -- AttributeTypeAndValue
    ///         OID 2.5.4.3        -- id-at-commonName
    ///         UTF8String "test"   -- value
    ///       }
    ///     }
    ///   }
    #[test]
    fn test_parse_name_simple() {
        // OID 2.5.4.3 encodes as: 55 04 03
        let oid_bytes: &[u8] = &[0x06, 0x03, 0x55, 0x04, 0x03];
        // UTF8String "test" = tag 0x0C, length 4, "test"
        let value_bytes: &[u8] = &[0x0C, 0x04, b't', b'e', b's', b't'];

        // SEQUENCE { OID, UTF8String }
        let atv_content_len = oid_bytes.len() + value_bytes.len();
        let mut atv = vec![0x30, atv_content_len as u8];
        atv.extend_from_slice(oid_bytes);
        atv.extend_from_slice(value_bytes);

        // SET { SEQUENCE }
        let set_content_len = atv.len();
        let mut set = vec![0x31, set_content_len as u8];
        set.extend_from_slice(&atv);

        // Outer SEQUENCE { SET }
        let seq_content_len = set.len();
        let mut name_der = vec![0x30, seq_content_len as u8];
        name_der.extend_from_slice(&set);

        let mut parser = DerParser::new(&name_der);
        let name = parse_name(&mut parser).expect("parse_name should succeed");

        assert_eq!(name.len(), 1);
        assert_eq!(name[0].0, vec![2, 5, 4, 3]);
        assert_eq!(name[0].1, b"test");
    }

    #[test]
    fn test_get_common_name_found() {
        let name = vec![
            (vec![2, 5, 4, 6], b"US".to_vec()),
            (vec![2, 5, 4, 10], b"Acme".to_vec()),
            (vec![2, 5, 4, 3], b"example.com".to_vec()),
        ];
        let cn = get_common_name(&name);
        assert_eq!(cn, Some(b"example.com".as_slice()));
    }

    #[test]
    fn test_get_common_name_not_found() {
        let name = vec![
            (vec![2, 5, 4, 6], b"US".to_vec()),
            (vec![2, 5, 4, 10], b"Acme".to_vec()),
        ];
        let cn = get_common_name(&name);
        assert!(cn.is_none());
    }

    #[test]
    fn test_get_common_name_empty() {
        let name: Vec<(Vec<u32>, Vec<u8>)> = vec![];
        let cn = get_common_name(&name);
        assert!(cn.is_none());
    }

    #[test]
    fn test_datetime_to_unix_epoch() {
        // 1970-01-01 00:00:00 UTC = 0
        assert_eq!(datetime_to_unix(1970, 1, 1, 0, 0, 0), 0);
    }

    #[test]
    fn test_datetime_to_unix_known_date() {
        // 2024-01-01 00:00:00 UTC
        // From 1970 to 2024 is 54 years.
        // Leap years in [1970..2023]: 1972,1976,...,2020 = 14 leap years
        // Total days = 54*365 + 14 = 19710 + 14 = 19724 -- but let's
        // verify with the well-known value: 1704067200
        let ts = datetime_to_unix(2024, 1, 1, 0, 0, 0);
        assert_eq!(ts, 1704067200);
    }

    #[test]
    fn test_datetime_to_unix_leap_year() {
        // 2000-03-01 00:00:00 UTC -- 2000 is a leap year
        // Known Unix timestamp: 951868800
        let ts = datetime_to_unix(2000, 3, 1, 0, 0, 0);
        assert_eq!(ts, 951868800);
    }

    #[test]
    fn test_is_leap_year() {
        assert!(is_leap_year(2000)); // Divisible by 400
        assert!(!is_leap_year(1900)); // Divisible by 100 but not 400
        assert!(is_leap_year(2024)); // Divisible by 4 but not 100
        assert!(!is_leap_year(2023)); // Not divisible by 4
    }

    /// Build a DER-encoded Name with two RDNs (Country + Common Name) and
    /// verify both are extracted.
    #[test]
    fn test_parse_name_multiple_rdns() {
        // RDN 1: Country = "US"
        // OID 2.5.4.6 encodes as: 55 04 06
        let oid_c: &[u8] = &[0x06, 0x03, 0x55, 0x04, 0x06];
        let val_c: &[u8] = &[0x13, 0x02, b'U', b'S']; // PrintableString "US"
        let mut atv_c = vec![0x30, (oid_c.len() + val_c.len()) as u8];
        atv_c.extend_from_slice(oid_c);
        atv_c.extend_from_slice(val_c);
        let mut set_c = vec![0x31, atv_c.len() as u8];
        set_c.extend_from_slice(&atv_c);

        // RDN 2: Common Name = "example.com"
        let oid_cn: &[u8] = &[0x06, 0x03, 0x55, 0x04, 0x03];
        let cn_val = b"example.com";
        let mut val_cn = vec![0x0C, cn_val.len() as u8];
        val_cn.extend_from_slice(cn_val);
        let mut atv_cn = vec![0x30, (oid_cn.len() + val_cn.len()) as u8];
        atv_cn.extend_from_slice(oid_cn);
        atv_cn.extend_from_slice(&val_cn);
        let mut set_cn = vec![0x31, atv_cn.len() as u8];
        set_cn.extend_from_slice(&atv_cn);

        // Outer SEQUENCE
        let inner_len = set_c.len() + set_cn.len();
        let mut name_der = vec![0x30, inner_len as u8];
        name_der.extend_from_slice(&set_c);
        name_der.extend_from_slice(&set_cn);

        let mut parser = DerParser::new(&name_der);
        let name = parse_name(&mut parser).expect("parse_name should succeed");

        assert_eq!(name.len(), 2);
        assert_eq!(name[0].0, vec![2, 5, 4, 6]);
        assert_eq!(name[0].1, b"US");
        assert_eq!(name[1].0, vec![2, 5, 4, 3]);
        assert_eq!(name[1].1, b"example.com");
    }
}
