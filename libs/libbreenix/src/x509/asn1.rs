//! Minimal DER (Distinguished Encoding Rules) parser for X.509 certificate parsing.
//!
//! This module implements just enough ASN.1 DER decoding to parse X.509 certificates.
//! It does not attempt to be a general-purpose ASN.1 library.

extern crate alloc;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// ASN.1 tag constants
// ---------------------------------------------------------------------------

pub const TAG_INTEGER: u8 = 0x02;
pub const TAG_BIT_STRING: u8 = 0x03;
pub const TAG_OCTET_STRING: u8 = 0x04;
pub const TAG_NULL: u8 = 0x05;
pub const TAG_OID: u8 = 0x06;
pub const TAG_UTF8_STRING: u8 = 0x0C;
pub const TAG_PRINTABLE_STRING: u8 = 0x13;
pub const TAG_IA5_STRING: u8 = 0x16;
pub const TAG_UTC_TIME: u8 = 0x17;
pub const TAG_GENERALIZED_TIME: u8 = 0x18;
pub const TAG_SEQUENCE: u8 = 0x30;
pub const TAG_SET: u8 = 0x31;

// Context-specific tags
pub const TAG_CONTEXT_0: u8 = 0xA0; // [0] EXPLICIT
pub const TAG_CONTEXT_1: u8 = 0xA1;
pub const TAG_CONTEXT_2: u8 = 0xA2;
pub const TAG_CONTEXT_3: u8 = 0xA3;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asn1Error {
    UnexpectedEnd,
    InvalidLength,
    UnexpectedTag { expected: u8, got: u8 },
    InvalidOid,
    InvalidInteger,
    InvalidTime,
}

// ---------------------------------------------------------------------------
// DER Parser
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct DerParser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> DerParser<'a> {
    /// Create a new DER parser over the given byte slice.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Return the number of unconsumed bytes remaining.
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// Check if there are no more bytes to parse.
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Return the current read position within the input.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Peek at the next tag byte without advancing the parser.
    pub fn peek_tag(&self) -> Result<u8, Asn1Error> {
        if self.pos >= self.data.len() {
            return Err(Asn1Error::UnexpectedEnd);
        }
        Ok(self.data[self.pos])
    }

    /// Read one byte as a tag and advance the parser.
    pub fn read_tag(&mut self) -> Result<u8, Asn1Error> {
        if self.pos >= self.data.len() {
            return Err(Asn1Error::UnexpectedEnd);
        }
        let tag = self.data[self.pos];
        self.pos += 1;
        Ok(tag)
    }

    /// Read a DER-encoded length value.
    ///
    /// Short form (< 128): single byte is the length.
    /// Long form: first byte has bit 7 set; the lower 7 bits give the number
    /// of subsequent bytes that encode the length in big-endian.
    pub fn read_length(&mut self) -> Result<usize, Asn1Error> {
        if self.pos >= self.data.len() {
            return Err(Asn1Error::UnexpectedEnd);
        }
        let first = self.data[self.pos];
        self.pos += 1;

        if first < 0x80 {
            // Short form
            return Ok(first as usize);
        }

        // Long form: lower 7 bits = number of length bytes
        let num_bytes = (first & 0x7F) as usize;
        if num_bytes == 0 || num_bytes > 4 {
            // Indefinite length (0) or too large
            return Err(Asn1Error::InvalidLength);
        }
        if self.pos + num_bytes > self.data.len() {
            return Err(Asn1Error::UnexpectedEnd);
        }

        let mut length: usize = 0;
        for i in 0..num_bytes {
            length = length
                .checked_shl(8)
                .ok_or(Asn1Error::InvalidLength)?
                | (self.data[self.pos + i] as usize);
        }
        self.pos += num_bytes;

        // DER requires minimal encoding: reject if short form would have sufficed
        if length < 0x80 {
            return Err(Asn1Error::InvalidLength);
        }

        Ok(length)
    }

    /// Read a complete TLV (tag-length-value) element.
    ///
    /// Returns `(tag, value_bytes)` where `value_bytes` is a slice over the
    /// value portion of the TLV.
    pub fn read_tlv(&mut self) -> Result<(u8, &'a [u8]), Asn1Error> {
        let tag = self.read_tag()?;
        let length = self.read_length()?;
        if self.pos + length > self.data.len() {
            return Err(Asn1Error::UnexpectedEnd);
        }
        let value = &self.data[self.pos..self.pos + length];
        self.pos += length;
        Ok((tag, value))
    }

    /// Expect a SEQUENCE tag and return a sub-parser positioned over its contents.
    pub fn read_sequence(&mut self) -> Result<DerParser<'a>, Asn1Error> {
        let (tag, value) = self.read_tlv()?;
        if tag != TAG_SEQUENCE {
            return Err(Asn1Error::UnexpectedTag {
                expected: TAG_SEQUENCE,
                got: tag,
            });
        }
        Ok(DerParser::new(value))
    }

    /// Expect a SET tag and return a sub-parser positioned over its contents.
    pub fn read_set(&mut self) -> Result<DerParser<'a>, Asn1Error> {
        let (tag, value) = self.read_tlv()?;
        if tag != TAG_SET {
            return Err(Asn1Error::UnexpectedTag {
                expected: TAG_SET,
                got: tag,
            });
        }
        Ok(DerParser::new(value))
    }

    /// Expect an INTEGER tag and return the raw value bytes.
    ///
    /// The value may include a leading zero byte to indicate a positive number
    /// whose high bit would otherwise be set.
    pub fn read_integer(&mut self) -> Result<&'a [u8], Asn1Error> {
        let (tag, value) = self.read_tlv()?;
        if tag != TAG_INTEGER {
            return Err(Asn1Error::UnexpectedTag {
                expected: TAG_INTEGER,
                got: tag,
            });
        }
        if value.is_empty() {
            return Err(Asn1Error::InvalidInteger);
        }
        Ok(value)
    }

    /// Expect an INTEGER tag and parse it as a `u64`.
    ///
    /// Returns `InvalidInteger` if the value is negative or too large.
    pub fn read_integer_as_u64(&mut self) -> Result<u64, Asn1Error> {
        let bytes = self.read_integer()?;

        // Check for negative (high bit set without leading zero)
        if bytes[0] & 0x80 != 0 {
            return Err(Asn1Error::InvalidInteger);
        }

        // Strip the leading zero byte if present
        let bytes = if bytes.len() > 1 && bytes[0] == 0x00 {
            &bytes[1..]
        } else {
            bytes
        };

        if bytes.len() > 8 {
            return Err(Asn1Error::InvalidInteger);
        }

        let mut result: u64 = 0;
        for &b in bytes {
            result = result << 8 | b as u64;
        }
        Ok(result)
    }

    /// Expect an OID tag and decode its components.
    ///
    /// The first encoded byte yields components 1 and 2 as `first / 40` and
    /// `first % 40`. Subsequent components use base-128 encoding where the
    /// MSB is a continuation bit.
    pub fn read_oid(&mut self) -> Result<Vec<u32>, Asn1Error> {
        let (tag, value) = self.read_tlv()?;
        if tag != TAG_OID {
            return Err(Asn1Error::UnexpectedTag {
                expected: TAG_OID,
                got: tag,
            });
        }
        if value.is_empty() {
            return Err(Asn1Error::InvalidOid);
        }

        let mut components = Vec::new();

        // First byte encodes components 1 and 2
        let first = value[0] as u32;
        components.push(first / 40);
        components.push(first % 40);

        // Remaining bytes are base-128 encoded components
        let mut i = 1;
        while i < value.len() {
            let mut component: u32 = 0;
            loop {
                if i >= value.len() {
                    return Err(Asn1Error::InvalidOid);
                }
                let byte = value[i];
                i += 1;
                component = component
                    .checked_shl(7)
                    .ok_or(Asn1Error::InvalidOid)?
                    | (byte & 0x7F) as u32;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            components.push(component);
        }

        Ok(components)
    }

    /// Expect a BIT STRING tag, skip the unused-bits byte, and return the
    /// content bytes.
    pub fn read_bit_string(&mut self) -> Result<&'a [u8], Asn1Error> {
        let (tag, value) = self.read_tlv()?;
        if tag != TAG_BIT_STRING {
            return Err(Asn1Error::UnexpectedTag {
                expected: TAG_BIT_STRING,
                got: tag,
            });
        }
        if value.is_empty() {
            return Err(Asn1Error::UnexpectedEnd);
        }
        // First byte is the number of unused bits in the last byte -- skip it
        Ok(&value[1..])
    }

    /// Expect an OCTET STRING tag and return the value bytes.
    pub fn read_octet_string(&mut self) -> Result<&'a [u8], Asn1Error> {
        self.expect_tag(TAG_OCTET_STRING)
    }

    /// Expect a UTF8String tag and return the value as a `&str`.
    pub fn read_utf8_string(&mut self) -> Result<&'a str, Asn1Error> {
        let value = self.expect_tag(TAG_UTF8_STRING)?;
        core::str::from_utf8(value).map_err(|_| Asn1Error::UnexpectedEnd)
    }

    /// Expect a PrintableString tag and return the value as a `&str`.
    ///
    /// PrintableString is a restricted subset of ASCII, so UTF-8 decoding is
    /// always valid.
    pub fn read_printable_string(&mut self) -> Result<&'a str, Asn1Error> {
        let value = self.expect_tag(TAG_PRINTABLE_STRING)?;
        core::str::from_utf8(value).map_err(|_| Asn1Error::UnexpectedEnd)
    }

    /// Expect an IA5String tag and return the value as a `&str`.
    pub fn read_ia5_string(&mut self) -> Result<&'a str, Asn1Error> {
        let value = self.expect_tag(TAG_IA5_STRING)?;
        core::str::from_utf8(value).map_err(|_| Asn1Error::UnexpectedEnd)
    }

    /// Read any string type (UTF8String, PrintableString, IA5String) and
    /// return the raw value bytes.
    pub fn read_any_string(&mut self) -> Result<&'a [u8], Asn1Error> {
        let tag = self.peek_tag()?;
        match tag {
            TAG_UTF8_STRING | TAG_PRINTABLE_STRING | TAG_IA5_STRING => {
                let (_, value) = self.read_tlv()?;
                Ok(value)
            }
            _ => Err(Asn1Error::UnexpectedTag {
                expected: TAG_UTF8_STRING,
                got: tag,
            }),
        }
    }

    /// Parse a UTCTime value (`"YYMMDDHHMMSSZ"`) into a Unix timestamp.
    ///
    /// Per RFC 5280, years 00-49 are interpreted as 2000-2049 and years
    /// 50-99 as 1950-1999.
    pub fn read_utc_time(&mut self) -> Result<u64, Asn1Error> {
        let value = self.expect_tag(TAG_UTC_TIME)?;
        if value.len() < 13 {
            return Err(Asn1Error::InvalidTime);
        }

        let s = core::str::from_utf8(value).map_err(|_| Asn1Error::InvalidTime)?;

        let year = parse_digits(s, 0, 2)?;
        let year = if year >= 50 { 1900 + year } else { 2000 + year };
        let month = parse_digits(s, 2, 2)?;
        let day = parse_digits(s, 4, 2)?;
        let hour = parse_digits(s, 6, 2)?;
        let min = parse_digits(s, 8, 2)?;
        let sec = parse_digits(s, 10, 2)?;

        // Must end with 'Z'
        if s.as_bytes().get(12) != Some(&b'Z') {
            return Err(Asn1Error::InvalidTime);
        }

        Ok(date_to_unix(year, month, day, hour, min, sec))
    }

    /// Parse a GeneralizedTime value (`"YYYYMMDDHHMMSSZ"`) into a Unix
    /// timestamp.
    pub fn read_generalized_time(&mut self) -> Result<u64, Asn1Error> {
        let value = self.expect_tag(TAG_GENERALIZED_TIME)?;
        if value.len() < 15 {
            return Err(Asn1Error::InvalidTime);
        }

        let s = core::str::from_utf8(value).map_err(|_| Asn1Error::InvalidTime)?;

        let year = parse_digits(s, 0, 4)?;
        let month = parse_digits(s, 4, 2)?;
        let day = parse_digits(s, 6, 2)?;
        let hour = parse_digits(s, 8, 2)?;
        let min = parse_digits(s, 10, 2)?;
        let sec = parse_digits(s, 12, 2)?;

        // Must end with 'Z'
        if s.as_bytes().get(14) != Some(&b'Z') {
            return Err(Asn1Error::InvalidTime);
        }

        Ok(date_to_unix(year, month, day, hour, min, sec))
    }

    /// Read either a UTCTime or GeneralizedTime value and return a Unix
    /// timestamp.
    pub fn read_time(&mut self) -> Result<u64, Asn1Error> {
        let tag = self.peek_tag()?;
        match tag {
            TAG_UTC_TIME => self.read_utc_time(),
            TAG_GENERALIZED_TIME => self.read_generalized_time(),
            _ => Err(Asn1Error::UnexpectedTag {
                expected: TAG_UTC_TIME,
                got: tag,
            }),
        }
    }

    /// Skip one complete TLV element.
    pub fn skip(&mut self) -> Result<(), Asn1Error> {
        let _tag = self.read_tag()?;
        let length = self.read_length()?;
        if self.pos + length > self.data.len() {
            return Err(Asn1Error::UnexpectedEnd);
        }
        self.pos += length;
        Ok(())
    }

    /// Read one complete TLV element and return the raw bytes including the
    /// tag and length encoding.
    pub fn read_raw_tlv(&mut self) -> Result<&'a [u8], Asn1Error> {
        let start = self.pos;
        let _tag = self.read_tag()?;
        let length = self.read_length()?;
        if self.pos + length > self.data.len() {
            return Err(Asn1Error::UnexpectedEnd);
        }
        self.pos += length;
        Ok(&self.data[start..self.pos])
    }

    /// Read any TLV element, returning its tag and value.
    ///
    /// This is an alias for [`read_tlv`] for convenience.
    pub fn read_any(&mut self) -> Result<(u8, &'a [u8]), Asn1Error> {
        self.read_tlv()
    }

    /// Read a context-specific tagged element (e.g., `[0] EXPLICIT`, `[3] EXPLICIT`).
    ///
    /// Expects the tag to be `0xA0 | context_number` and returns the inner value bytes.
    pub fn read_context(&mut self, context_number: u8) -> Result<&'a [u8], Asn1Error> {
        let expected_tag = 0xA0 | context_number;
        self.expect_tag(expected_tag)
    }

    /// Expect a specific tag and return its value bytes.
    ///
    /// Returns `UnexpectedTag` if the next tag does not match.
    pub fn expect_tag(&mut self, tag: u8) -> Result<&'a [u8], Asn1Error> {
        let (actual, value) = self.read_tlv()?;
        if actual != tag {
            return Err(Asn1Error::UnexpectedTag {
                expected: tag,
                got: actual,
            });
        }
        Ok(value)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse `count` ASCII decimal digits from `s` starting at byte offset
/// `offset`, returning the decoded integer value.
fn parse_digits(s: &str, offset: usize, count: usize) -> Result<u32, Asn1Error> {
    let bytes = s.as_bytes();
    if offset + count > bytes.len() {
        return Err(Asn1Error::InvalidTime);
    }
    let mut value: u32 = 0;
    for &b in &bytes[offset..offset + count] {
        if !b.is_ascii_digit() {
            return Err(Asn1Error::InvalidTime);
        }
        value = value * 10 + (b - b'0') as u32;
    }
    Ok(value)
}

/// Convert date components to an approximate Unix timestamp (seconds since
/// 1970-01-01 00:00:00 UTC).
///
/// Accounts for leap years using the standard Gregorian calendar rules.
fn date_to_unix(year: u32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> u64 {
    // Cumulative days before each month in a non-leap year (index 0 = unused,
    // index 1 = Jan, ...).
    const DAYS_BEFORE_MONTH: [u32; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

    let is_leap = |y: u32| -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 };

    // Days from the epoch year (1970) to the start of `year`.
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    // Days within the year.
    if month >= 1 && month <= 12 {
        days += DAYS_BEFORE_MONTH[month as usize] as u64;
    }
    // Add leap day if past February in a leap year.
    if month > 2 && is_leap(year) {
        days += 1;
    }
    // `day` is 1-based.
    if day > 0 {
        days += (day - 1) as u64;
    }

    days * 86400 + hour as u64 * 3600 + min as u64 * 60 + sec as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequence_containing_integer() {
        // SEQUENCE { INTEGER 42 }
        // 30 05         -- SEQUENCE, length 5
        //   02 03       -- INTEGER, length 3
        //     00 00 2a  -- value 42 (with leading zero padding for illustration)
        let data = [0x30, 0x05, 0x02, 0x03, 0x00, 0x00, 0x2a];
        let mut parser = DerParser::new(&data);

        let mut seq = parser.read_sequence().expect("should parse SEQUENCE");
        let val = seq.read_integer_as_u64().expect("should parse INTEGER");
        assert_eq!(val, 42);
        assert_eq!(seq.remaining(), 0);
        assert_eq!(parser.remaining(), 0);
    }

    #[test]
    fn test_oid_sha256_with_rsa() {
        // OID 1.2.840.113549.1.1.11 (sha256WithRSAEncryption)
        // Encoded value bytes: 2a 86 48 86 f7 0d 01 01 0b
        let oid_bytes: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
        // Wrap in a proper TLV: tag 0x06, length 9
        let mut data = Vec::new();
        data.push(TAG_OID);
        data.push(oid_bytes.len() as u8);
        data.extend_from_slice(oid_bytes);

        let mut parser = DerParser::new(&data);
        let components = parser.read_oid().expect("should parse OID");
        assert_eq!(components, vec![1, 2, 840, 113549, 1, 1, 11]);
    }

    #[test]
    fn test_length_short_form() {
        // Short form: length = 3
        let data = [0x03];
        let mut parser = DerParser::new(&data);
        let len = parser.read_length().expect("should parse short form length");
        assert_eq!(len, 3);
    }

    #[test]
    fn test_length_long_form() {
        // Long form: 0x82 means 2 subsequent bytes encode the length
        // 0x01 0x00 = 256
        let data = [0x82, 0x01, 0x00];
        let mut parser = DerParser::new(&data);
        let len = parser.read_length().expect("should parse long form length");
        assert_eq!(len, 256);
    }

    #[test]
    fn test_utc_time_parsing() {
        // UTCTime "230615120000Z" = 2023-06-15 12:00:00 UTC
        let time_str = b"230615120000Z";
        let mut data = Vec::new();
        data.push(TAG_UTC_TIME);
        data.push(time_str.len() as u8);
        data.extend_from_slice(time_str);

        let mut parser = DerParser::new(&data);
        let timestamp = parser.read_utc_time().expect("should parse UTCTime");

        // Verify by computing expected value:
        // 2023-06-15 12:00:00 UTC
        let expected = date_to_unix(2023, 6, 15, 12, 0, 0);
        assert_eq!(timestamp, expected);

        // Sanity-check the timestamp is in the right ballpark:
        // 2023-06-15 should be around 1686816000
        assert!(timestamp > 1_686_000_000);
        assert!(timestamp < 1_687_000_000);
    }

    #[test]
    fn test_generalized_time_parsing() {
        // GeneralizedTime "20301231235959Z" = 2030-12-31 23:59:59 UTC
        let time_str = b"20301231235959Z";
        let mut data = Vec::new();
        data.push(TAG_GENERALIZED_TIME);
        data.push(time_str.len() as u8);
        data.extend_from_slice(time_str);

        let mut parser = DerParser::new(&data);
        let timestamp = parser
            .read_generalized_time()
            .expect("should parse GeneralizedTime");

        let expected = date_to_unix(2030, 12, 31, 23, 59, 59);
        assert_eq!(timestamp, expected);
    }

    #[test]
    fn test_bit_string() {
        // BIT STRING with 0 unused bits, content [0xDE, 0xAD]
        let data = [TAG_BIT_STRING, 0x03, 0x00, 0xDE, 0xAD];
        let mut parser = DerParser::new(&data);
        let content = parser.read_bit_string().expect("should parse BIT STRING");
        assert_eq!(content, &[0xDE, 0xAD]);
    }

    #[test]
    fn test_skip() {
        // Two consecutive INTEGERs: skip the first, read the second.
        let data = [
            TAG_INTEGER, 0x01, 0x01, // INTEGER 1
            TAG_INTEGER, 0x01, 0x02, // INTEGER 2
        ];
        let mut parser = DerParser::new(&data);
        parser.skip().expect("should skip first TLV");
        let val = parser.read_integer_as_u64().expect("should read second INTEGER");
        assert_eq!(val, 2);
    }

    #[test]
    fn test_read_raw_tlv() {
        let data = [TAG_INTEGER, 0x02, 0x00, 0xFF];
        let mut parser = DerParser::new(&data);
        let raw = parser.read_raw_tlv().expect("should read raw TLV");
        assert_eq!(raw, &[TAG_INTEGER, 0x02, 0x00, 0xFF]);
        assert_eq!(parser.remaining(), 0);
    }

    #[test]
    fn test_unexpected_tag_error() {
        let data = [TAG_INTEGER, 0x01, 0x05]; // An INTEGER, not a SEQUENCE
        let mut parser = DerParser::new(&data);
        let err = parser.read_sequence().unwrap_err();
        assert_eq!(
            err,
            Asn1Error::UnexpectedTag {
                expected: TAG_SEQUENCE,
                got: TAG_INTEGER,
            }
        );
    }

    #[test]
    fn test_unexpected_end_error() {
        let data: &[u8] = &[];
        let mut parser = DerParser::new(data);
        assert_eq!(parser.read_tag().unwrap_err(), Asn1Error::UnexpectedEnd);
    }

    #[test]
    fn test_date_to_unix_epoch() {
        // 1970-01-01 00:00:00 should be 0
        assert_eq!(date_to_unix(1970, 1, 1, 0, 0, 0), 0);
    }

    #[test]
    fn test_date_to_unix_known_date() {
        // 2000-01-01 00:00:00 UTC = 946684800
        assert_eq!(date_to_unix(2000, 1, 1, 0, 0, 0), 946684800);
    }

    #[test]
    fn test_printable_string() {
        let s = b"US";
        let mut data = Vec::new();
        data.push(TAG_PRINTABLE_STRING);
        data.push(s.len() as u8);
        data.extend_from_slice(s);

        let mut parser = DerParser::new(&data);
        let val = parser
            .read_printable_string()
            .expect("should parse PrintableString");
        assert_eq!(val, "US");
    }

    #[test]
    fn test_any_string_reads_utf8() {
        let s = b"example";
        let mut data = Vec::new();
        data.push(TAG_UTF8_STRING);
        data.push(s.len() as u8);
        data.extend_from_slice(s);

        let mut parser = DerParser::new(&data);
        let val = parser
            .read_any_string()
            .expect("should parse any string type");
        assert_eq!(val, b"example");
    }

    #[test]
    fn test_nested_sequence() {
        // SEQUENCE { SEQUENCE { INTEGER 7 } }
        // Inner: 30 03 02 01 07
        // Outer: 30 05 30 03 02 01 07
        let data = [0x30, 0x05, 0x30, 0x03, 0x02, 0x01, 0x07];
        let mut parser = DerParser::new(&data);
        let mut outer = parser.read_sequence().expect("outer SEQUENCE");
        let mut inner = outer.read_sequence().expect("inner SEQUENCE");
        let val = inner.read_integer_as_u64().expect("INTEGER");
        assert_eq!(val, 7);
    }
}
