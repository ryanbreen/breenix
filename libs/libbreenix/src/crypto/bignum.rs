//! Big integer arithmetic for RSA verification.
//!
//! Provides arbitrary-precision unsigned integers using a `Vec<u64>` limb
//! representation (little-endian: least significant limb first). Implements
//! the operations needed for RSA signature verification: addition,
//! subtraction, multiplication, division, and modular exponentiation.
//!
//! # Example
//!
//! ```
//! use libbreenix::crypto::bignum::BigNum;
//!
//! // Compute 4^13 mod 497 = 445
//! let base = BigNum::from_u64(4);
//! let exp = BigNum::from_u64(13);
//! let modulus = BigNum::from_u64(497);
//! let result = base.mod_exp(&exp, &modulus);
//! assert_eq!(result, BigNum::from_u64(445));
//! ```

use core::cmp::Ordering;

/// Arbitrary-precision unsigned integer.
///
/// Internally stored as a vector of `u64` limbs in little-endian order
/// (the least significant limb is at index 0). The representation is
/// always trimmed so that the most significant limb is nonzero, except
/// for the value zero which is represented as a single zero limb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BigNum {
    /// Little-endian limbs (least significant first).
    limbs: Vec<u64>,
}

impl BigNum {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a `BigNum` with the value zero.
    pub fn zero() -> Self {
        Self { limbs: vec![0] }
    }

    /// Create a `BigNum` with the value one.
    pub fn one() -> Self {
        Self { limbs: vec![1] }
    }

    /// Create a `BigNum` from a single `u64` value.
    pub fn from_u64(val: u64) -> Self {
        Self { limbs: vec![val] }
    }

    /// Parse a big-endian byte slice into a `BigNum`.
    ///
    /// Leading zero bytes are skipped. The bytes are grouped into 64-bit
    /// limbs (big-endian within each limb, little-endian limb order).
    ///
    /// # Example
    ///
    /// ```
    /// use libbreenix::crypto::bignum::BigNum;
    ///
    /// let n = BigNum::from_be_bytes(&[0x01, 0x00]);
    /// assert_eq!(n, BigNum::from_u64(256));
    /// ```
    pub fn from_be_bytes(bytes: &[u8]) -> Self {
        // Skip leading zeros.
        let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
        let bytes = &bytes[start..];

        if bytes.is_empty() {
            return Self::zero();
        }

        // Number of full 8-byte groups and the size of the partial leading group.
        let num_full = bytes.len() / 8;
        let partial = bytes.len() % 8;
        let num_limbs = num_full + if partial > 0 { 1 } else { 0 };

        let mut limbs = Vec::with_capacity(num_limbs);

        // Process full 8-byte groups from the end (least significant first).
        for i in 0..num_full {
            let offset = bytes.len() - (i + 1) * 8;
            let mut limb = 0u64;
            for j in 0..8 {
                limb = (limb << 8) | bytes[offset + j] as u64;
            }
            limbs.push(limb);
        }

        // Process the remaining partial group (most significant limb).
        if partial > 0 {
            let mut limb = 0u64;
            for j in 0..partial {
                limb = (limb << 8) | bytes[j] as u64;
            }
            limbs.push(limb);
        }

        let mut result = Self { limbs };
        result.trim();
        result
    }

    /// Convert to a big-endian byte vector, stripping leading zeros.
    ///
    /// The result always contains at least one byte (0x00 for zero).
    pub fn to_be_bytes(&self) -> Vec<u8> {
        if self.is_zero() {
            return vec![0];
        }

        let mut bytes = Vec::new();

        // Process from the most significant limb to the least significant.
        for i in (0..self.limbs.len()).rev() {
            let limb = self.limbs[i];
            for shift in (0..8).rev() {
                bytes.push((limb >> (shift * 8)) as u8);
            }
        }

        // Strip leading zeros.
        let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len().saturating_sub(1));
        bytes[start..].to_vec()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Remove trailing zero limbs, keeping at least one limb.
    fn trim(&mut self) {
        while self.limbs.len() > 1 && *self.limbs.last().unwrap() == 0 {
            self.limbs.pop();
        }
    }

    /// Returns `true` if this value is zero.
    pub fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// Returns the number of significant bits.
    ///
    /// For zero, returns 0.
    pub fn bit_len(&self) -> usize {
        if self.is_zero() {
            return 0;
        }
        let top = *self.limbs.last().unwrap();
        (self.limbs.len() - 1) * 64 + (64 - top.leading_zeros() as usize)
    }

    /// Get the bit at position `i` (0 = least significant).
    fn get_bit(&self, i: usize) -> bool {
        let limb_idx = i / 64;
        let bit_idx = i % 64;
        if limb_idx >= self.limbs.len() {
            return false;
        }
        (self.limbs[limb_idx] >> bit_idx) & 1 == 1
    }

    // -----------------------------------------------------------------------
    // Comparison
    // -----------------------------------------------------------------------

    /// Compare two `BigNum` values.
    pub fn cmp(&self, other: &BigNum) -> Ordering {
        // Compare by number of limbs first.
        match self.limbs.len().cmp(&other.limbs.len()) {
            Ordering::Greater => return Ordering::Greater,
            Ordering::Less => return Ordering::Less,
            Ordering::Equal => {}
        }
        // Same number of limbs: compare from most significant.
        for i in (0..self.limbs.len()).rev() {
            match self.limbs[i].cmp(&other.limbs[i]) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }
        Ordering::Equal
    }

    // -----------------------------------------------------------------------
    // Arithmetic
    // -----------------------------------------------------------------------

    /// Schoolbook addition with carry.
    pub fn add(&self, other: &BigNum) -> BigNum {
        let max_len = core::cmp::max(self.limbs.len(), other.limbs.len());
        let mut result = Vec::with_capacity(max_len + 1);
        let mut carry = 0u64;

        for i in 0..max_len {
            let a = if i < self.limbs.len() { self.limbs[i] } else { 0 };
            let b = if i < other.limbs.len() { other.limbs[i] } else { 0 };

            let (sum1, c1) = a.overflowing_add(b);
            let (sum2, c2) = sum1.overflowing_add(carry);
            result.push(sum2);
            carry = (c1 as u64) + (c2 as u64);
        }

        if carry > 0 {
            result.push(carry);
        }

        let mut r = BigNum { limbs: result };
        r.trim();
        r
    }

    /// Schoolbook subtraction with borrow.
    ///
    /// # Panics
    ///
    /// Panics in debug mode if `self < other`.
    pub fn sub(&self, other: &BigNum) -> BigNum {
        debug_assert!(
            self.cmp(other) != Ordering::Less,
            "BigNum::sub: self must be >= other"
        );

        let mut result = Vec::with_capacity(self.limbs.len());
        let mut borrow = 0u64;

        for i in 0..self.limbs.len() {
            let a = self.limbs[i];
            let b = if i < other.limbs.len() { other.limbs[i] } else { 0 };

            let (diff1, b1) = a.overflowing_sub(b);
            let (diff2, b2) = diff1.overflowing_sub(borrow);
            result.push(diff2);
            borrow = (b1 as u64) + (b2 as u64);
        }

        let mut r = BigNum { limbs: result };
        r.trim();
        r
    }

    /// O(n^2) schoolbook multiplication using `u128` intermediates.
    pub fn mul(&self, other: &BigNum) -> BigNum {
        if self.is_zero() || other.is_zero() {
            return BigNum::zero();
        }

        let n = self.limbs.len();
        let m = other.limbs.len();
        let mut result = vec![0u64; n + m];

        for i in 0..n {
            let mut carry = 0u128;
            for j in 0..m {
                let product =
                    (self.limbs[i] as u128) * (other.limbs[j] as u128) + result[i + j] as u128 + carry;
                result[i + j] = product as u64;
                carry = product >> 64;
            }
            result[i + m] += carry as u64;
        }

        let mut r = BigNum { limbs: result };
        r.trim();
        r
    }

    /// Long division returning `(quotient, remainder)`.
    ///
    /// # Panics
    ///
    /// Panics if `divisor` is zero.
    pub fn div_rem(&self, divisor: &BigNum) -> (BigNum, BigNum) {
        assert!(!divisor.is_zero(), "BigNum::div_rem: division by zero");

        // Fast path: dividend < divisor.
        if self.cmp(divisor) == Ordering::Less {
            return (BigNum::zero(), self.clone());
        }

        // Fast path: single-limb divisor.
        if divisor.limbs.len() == 1 {
            return self.div_rem_single(divisor.limbs[0]);
        }

        // Binary long division (bit-by-bit).
        self.div_rem_binary(divisor)
    }

    /// Division by a single `u64` limb.
    fn div_rem_single(&self, d: u64) -> (BigNum, BigNum) {
        let d = d as u128;
        let mut quotient = vec![0u64; self.limbs.len()];
        let mut rem = 0u128;

        for i in (0..self.limbs.len()).rev() {
            let cur = (rem << 64) | self.limbs[i] as u128;
            quotient[i] = (cur / d) as u64;
            rem = cur % d;
        }

        let mut q = BigNum { limbs: quotient };
        q.trim();
        (q, BigNum::from_u64(rem as u64))
    }

    /// Binary long division (bit-by-bit).
    fn div_rem_binary(&self, divisor: &BigNum) -> (BigNum, BigNum) {
        let nbits = self.bit_len();
        let mut quotient = BigNum::zero();
        let mut remainder = BigNum::zero();

        // Pre-allocate quotient limbs.
        let q_limbs = (nbits + 63) / 64;
        quotient.limbs.resize(q_limbs, 0);

        for i in (0..nbits).rev() {
            // Shift remainder left by 1 bit.
            remainder = remainder.shl_one();

            // Bring down next bit from self.
            if self.get_bit(i) {
                remainder.limbs[0] |= 1;
            }

            // If remainder >= divisor, subtract and set quotient bit.
            if remainder.cmp(divisor) != Ordering::Less {
                remainder = remainder.sub(divisor);
                let limb_idx = i / 64;
                let bit_idx = i % 64;
                quotient.limbs[limb_idx] |= 1u64 << bit_idx;
            }
        }

        quotient.trim();
        remainder.trim();
        (quotient, remainder)
    }

    /// Left-shift by one bit (internal helper for division).
    fn shl_one(&self) -> BigNum {
        let mut result = Vec::with_capacity(self.limbs.len() + 1);
        let mut carry = 0u64;

        for &limb in &self.limbs {
            result.push((limb << 1) | carry);
            carry = limb >> 63;
        }

        if carry > 0 {
            result.push(carry);
        }

        BigNum { limbs: result }
    }

    /// Compute `self % modulus`.
    pub fn mod_val(&self, modulus: &BigNum) -> BigNum {
        let (_, rem) = self.div_rem(modulus);
        rem
    }

    // -----------------------------------------------------------------------
    // Modular exponentiation
    // -----------------------------------------------------------------------

    /// Compute `self^exponent mod modulus` using the right-to-left binary
    /// (square-and-multiply) method.
    ///
    /// For RSA-2048 with the common public exponent e = 65537 = 2^16 + 1,
    /// this performs only 16 squarings and 1 extra multiplication since
    /// 65537 has just two set bits.
    ///
    /// # Panics
    ///
    /// Panics if `modulus` is zero.
    pub fn mod_exp(&self, exponent: &BigNum, modulus: &BigNum) -> BigNum {
        assert!(!modulus.is_zero(), "BigNum::mod_exp: modulus must be nonzero");

        if exponent.is_zero() {
            return BigNum::one();
        }

        let nbits = exponent.bit_len();
        let mut result = BigNum::one();
        let mut base = self.mod_val(modulus);

        for i in 0..nbits {
            if exponent.get_bit(i) {
                result = result.mul(&base).mod_val(modulus);
            }
            // Don't bother squaring on the last iteration.
            if i + 1 < nbits {
                base = base.mul(&base).mod_val(modulus);
            }
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Trait implementations
// ---------------------------------------------------------------------------

impl PartialOrd for BigNum {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigNum {
    fn cmp(&self, other: &Self) -> Ordering {
        BigNum::cmp(self, other)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Construction roundtrip tests --

    #[test]
    fn test_from_be_bytes_to_be_bytes_roundtrip() {
        let original = vec![
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let n = BigNum::from_be_bytes(&original);
        let result = n.to_be_bytes();
        assert_eq!(result, original);
    }

    #[test]
    fn test_from_be_bytes_leading_zeros() {
        let with_zeros = vec![0x00, 0x00, 0x00, 0x42];
        let without_zeros = vec![0x42];
        let a = BigNum::from_be_bytes(&with_zeros);
        let b = BigNum::from_be_bytes(&without_zeros);
        assert_eq!(a, b);
        assert_eq!(a.to_be_bytes(), vec![0x42]);
    }

    #[test]
    fn test_from_be_bytes_empty() {
        let n = BigNum::from_be_bytes(&[]);
        assert_eq!(n, BigNum::zero());
        assert_eq!(n.to_be_bytes(), vec![0x00]);
    }

    #[test]
    fn test_from_be_bytes_single_byte() {
        let n = BigNum::from_be_bytes(&[0xFF]);
        assert_eq!(n, BigNum::from_u64(255));
    }

    #[test]
    fn test_from_be_bytes_large() {
        // A 24-byte value (3 limbs worth).
        let bytes = vec![
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
            0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
        ];
        let n = BigNum::from_be_bytes(&bytes);
        let roundtrip = n.to_be_bytes();
        assert_eq!(roundtrip, bytes);
    }

    // -- Addition tests --

    #[test]
    fn test_add_simple() {
        let a = BigNum::from_u64(100);
        let b = BigNum::from_u64(200);
        assert_eq!(a.add(&b), BigNum::from_u64(300));
    }

    #[test]
    fn test_add_carry_propagation() {
        // 0xFFFFFFFFFFFFFFFF + 1 = 0x1_0000000000000000
        let a = BigNum::from_u64(u64::MAX);
        let b = BigNum::from_u64(1);
        let result = a.add(&b);
        assert_eq!(result.limbs, vec![0, 1]);
        assert_eq!(result.to_be_bytes(), vec![0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_add_zero() {
        let a = BigNum::from_u64(42);
        let z = BigNum::zero();
        assert_eq!(a.add(&z), a);
        assert_eq!(z.add(&a), a);
    }

    // -- Subtraction tests --

    #[test]
    fn test_sub_simple() {
        let a = BigNum::from_u64(300);
        let b = BigNum::from_u64(100);
        assert_eq!(a.sub(&b), BigNum::from_u64(200));
    }

    #[test]
    fn test_sub_borrow() {
        // 0x1_0000000000000000 - 1 = 0xFFFFFFFFFFFFFFFF
        let a = BigNum {
            limbs: vec![0, 1],
        };
        let b = BigNum::from_u64(1);
        assert_eq!(a.sub(&b), BigNum::from_u64(u64::MAX));
    }

    #[test]
    fn test_sub_self_is_zero() {
        let a = BigNum::from_u64(12345);
        assert_eq!(a.sub(&a), BigNum::zero());
    }

    // -- Multiplication tests --

    #[test]
    fn test_mul_simple() {
        let a = BigNum::from_u64(12345);
        let b = BigNum::from_u64(67890);
        assert_eq!(a.mul(&b), BigNum::from_u64(838_102_050));
    }

    #[test]
    fn test_mul_by_zero() {
        let a = BigNum::from_u64(12345);
        assert_eq!(a.mul(&BigNum::zero()), BigNum::zero());
    }

    #[test]
    fn test_mul_by_one() {
        let a = BigNum::from_u64(9999);
        assert_eq!(a.mul(&BigNum::one()), a);
    }

    #[test]
    fn test_mul_large() {
        // (2^64 - 1) * (2^64 - 1) = 2^128 - 2^65 + 1
        let a = BigNum::from_u64(u64::MAX);
        let result = a.mul(&a);
        // u64::MAX * u64::MAX = 0xFFFFFFFFFFFFFFFE_0000000000000001
        assert_eq!(result.limbs, vec![1, 0xFFFF_FFFF_FFFF_FFFE]);
    }

    // -- Division tests --

    #[test]
    fn test_div_rem_simple() {
        let a = BigNum::from_u64(17);
        let b = BigNum::from_u64(5);
        let (q, r) = a.div_rem(&b);
        assert_eq!(q, BigNum::from_u64(3));
        assert_eq!(r, BigNum::from_u64(2));
    }

    #[test]
    fn test_div_rem_exact() {
        let a = BigNum::from_u64(100);
        let b = BigNum::from_u64(10);
        let (q, r) = a.div_rem(&b);
        assert_eq!(q, BigNum::from_u64(10));
        assert_eq!(r, BigNum::zero());
    }

    #[test]
    fn test_div_rem_dividend_smaller() {
        let a = BigNum::from_u64(3);
        let b = BigNum::from_u64(7);
        let (q, r) = a.div_rem(&b);
        assert_eq!(q, BigNum::zero());
        assert_eq!(r, BigNum::from_u64(3));
    }

    #[test]
    fn test_mod_val() {
        let a = BigNum::from_u64(100);
        let m = BigNum::from_u64(7);
        assert_eq!(a.mod_val(&m), BigNum::from_u64(2)); // 100 % 7 = 2
    }

    // -- Modular exponentiation tests --

    #[test]
    fn test_mod_exp_standard() {
        // 4^13 mod 497 = 445 (classic textbook example).
        let base = BigNum::from_u64(4);
        let exp = BigNum::from_u64(13);
        let modulus = BigNum::from_u64(497);
        assert_eq!(base.mod_exp(&exp, &modulus), BigNum::from_u64(445));
    }

    #[test]
    fn test_mod_exp_zero_exponent() {
        let base = BigNum::from_u64(42);
        let exp = BigNum::zero();
        let modulus = BigNum::from_u64(100);
        assert_eq!(base.mod_exp(&exp, &modulus), BigNum::one());
    }

    #[test]
    fn test_mod_exp_one_exponent() {
        let base = BigNum::from_u64(42);
        let exp = BigNum::one();
        let modulus = BigNum::from_u64(100);
        assert_eq!(base.mod_exp(&exp, &modulus), BigNum::from_u64(42));
    }

    #[test]
    fn test_mod_exp_rsa_e65537() {
        // Verify that mod_exp handles the RSA public exponent 65537 correctly.
        // Compute 2^65537 mod 1000000007 (a prime).
        //
        // We verify by checking that (result^inverse) mod p gives back the
        // original base, but it is simpler to just verify the known result.
        // 2^65537 mod 1000000007 can be verified: 65537 = 2^16 + 1.
        let base = BigNum::from_u64(2);
        let exp = BigNum::from_u64(65537);
        let modulus = BigNum::from_u64(1_000_000_007);
        let result = base.mod_exp(&exp, &modulus);

        // Verify by computing independently: 2^65537 mod 1000000007.
        // 65537 mod (p-1) = 65537 mod 1000000006. Since 65537 < 1000000006,
        // Fermat's little theorem doesn't simplify further.
        // Known value (computed externally): 2^65537 mod 1000000007 = 947173645.
        assert_eq!(result, BigNum::from_u64(947_173_645));
    }

    #[test]
    fn test_mod_exp_256bit() {
        // Use 256-bit values to exercise multi-limb modular exponentiation.
        //
        // base  = 2^128 + 1
        // exp   = 3
        // mod   = 2^256 - 189 (a known prime)
        //
        // We compute (2^128 + 1)^3 mod (2^256 - 189) and verify the roundtrip.

        // base = 2^128 + 1 (limbs: [1, 0, 1] in little-endian 64-bit)
        let base = BigNum {
            limbs: vec![1, 0, 1],
        };

        let exp = BigNum::from_u64(3);

        // modulus = 2^256 - 189
        // = [u64::MAX - 188, u64::MAX, u64::MAX, u64::MAX] in little-endian
        let modulus = BigNum {
            limbs: vec![u64::MAX - 188, u64::MAX, u64::MAX, u64::MAX],
        };

        let result = base.mod_exp(&exp, &modulus);

        // Verify by computing base^3 directly and taking mod.
        let base_cubed = base.mul(&base).mul(&base);
        let expected = base_cubed.mod_val(&modulus);
        assert_eq!(result, expected);
    }

    // -- Comparison tests --

    #[test]
    fn test_cmp() {
        let a = BigNum::from_u64(100);
        let b = BigNum::from_u64(200);
        assert_eq!(a.cmp(&b), Ordering::Less);
        assert_eq!(b.cmp(&a), Ordering::Greater);
        assert_eq!(a.cmp(&a), Ordering::Equal);
    }

    #[test]
    fn test_cmp_different_lengths() {
        let small = BigNum::from_u64(u64::MAX);
        let big = BigNum {
            limbs: vec![0, 1],
        }; // 2^64
        assert_eq!(small.cmp(&big), Ordering::Less);
    }

    #[test]
    fn test_ord_trait() {
        let a = BigNum::from_u64(10);
        let b = BigNum::from_u64(20);
        assert!(a < b);
        assert!(b > a);
        assert!(a <= a);
        assert!(a >= a);
    }

    // -- Bit operations tests --

    #[test]
    fn test_bit_len() {
        assert_eq!(BigNum::zero().bit_len(), 0);
        assert_eq!(BigNum::one().bit_len(), 1);
        assert_eq!(BigNum::from_u64(255).bit_len(), 8);
        assert_eq!(BigNum::from_u64(256).bit_len(), 9);
        assert_eq!(BigNum::from_u64(u64::MAX).bit_len(), 64);
    }

    #[test]
    fn test_get_bit() {
        let n = BigNum::from_u64(0b1010);
        assert!(!n.get_bit(0));
        assert!(n.get_bit(1));
        assert!(!n.get_bit(2));
        assert!(n.get_bit(3));
        assert!(!n.get_bit(4));
        assert!(!n.get_bit(100)); // Out of range.
    }
}
