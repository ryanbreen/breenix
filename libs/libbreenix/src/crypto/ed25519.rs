//! Ed25519 signatures (RFC 8032, section 5.1)
//!
//! Key generation, signing and verification over edwards25519, written from
//! RFC 8032. Points use extended homogeneous coordinates (X:Y:Z:T) with
//! x = X/Z, y = Y/Z and x*y = T/Z (section 5.1.4). The field arithmetic is
//! the GF(2^255 - 19) code that X25519 uses.
//!
//! Signing runs a fixed sequence of point operations and selects with masks,
//! so its timing does not depend on the secret scalar or nonce. Point
//! decoding and verification handle public data only.

use super::sha512::Sha512;
use super::x25519::{fe_add, fe_from_bytes, fe_inv, fe_mul, fe_sq, fe_sub, fe_to_bytes, Fe};

/// The group order L = 2^252 + 27742317777372353535851937790883648493,
/// as 32 little-endian bytes.
const L: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10,
];

/// The base point B's encoding: y = 4/5 with x even (section 5.1).
const BASE: [u8; 32] = [
    0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
];

pub const PUBLIC_KEY_LEN: usize = 32;
pub const SEED_LEN: usize = 32;
pub const SIGNATURE_LEN: usize = 64;

fn fe_small(value: u64) -> Fe {
    Fe([value, 0, 0, 0, 0])
}

fn fe_neg(a: &Fe) -> Fe {
    fe_sub(&Fe::zero(), a)
}

fn fe_eq(a: &Fe, b: &Fe) -> bool {
    fe_to_bytes(a) == fe_to_bytes(b)
}

fn fe_is_odd(a: &Fe) -> bool {
    fe_to_bytes(a)[0] & 1 == 1
}

/// a^e for an exponent given as little-endian bytes. Variable time: used only
/// for constants and for decoding public points.
fn fe_pow(a: &Fe, exponent: &[u8; 32]) -> Fe {
    let mut result = Fe::one();
    for byte in exponent.iter().rev() {
        for bit in (0..8).rev() {
            result = fe_sq(&result);
            if (byte >> bit) & 1 == 1 {
                result = fe_mul(&result, a);
            }
        }
    }
    result
}

/// p - 2^k for a small k, as little-endian bytes, minus `minus`: the exponents
/// (p-5)/8 = 2^252 - 3 and (p-1)/4 = 2^253 - 5 used below.
fn power_of_two_minus(bit: usize, minus: u8) -> [u8; 32] {
    let mut out = [0xffu8; 32];
    // 2^bit - 1 has `bit` low ones.
    for i in bit..256 {
        out[i / 8] &= !(1 << (i % 8));
    }
    // 2^bit - minus = (2^bit - 1) - (minus - 1).
    out[0] -= minus - 1;
    out
}

/// d = -121665 / 121666.
fn curve_d() -> Fe {
    fe_mul(&fe_neg(&fe_small(121665)), &fe_inv(&fe_small(121666)))
}

/// sqrt(-1) = 2^((p-1)/4).
fn sqrt_m1() -> Fe {
    fe_pow(&fe_small(2), &power_of_two_minus(253, 5))
}

#[derive(Clone, Copy)]
struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

impl Point {
    fn identity() -> Self {
        Point { x: Fe::zero(), y: Fe::one(), z: Fe::one(), t: Fe::zero() }
    }

    /// Point addition (section 5.1.4); the formula is complete, so it also doubles.
    fn add(&self, other: &Point, d2: &Fe) -> Point {
        let a = fe_mul(&fe_sub(&self.y, &self.x), &fe_sub(&other.y, &other.x));
        let b = fe_mul(&fe_add(&self.y, &self.x), &fe_add(&other.y, &other.x));
        let c = fe_mul(&fe_mul(&self.t, d2), &other.t);
        let d = fe_mul(&fe_add(&self.z, &self.z), &other.z);
        let e = fe_sub(&b, &a);
        let f = fe_sub(&d, &c);
        let g = fe_add(&d, &c);
        let h = fe_add(&b, &a);
        Point { x: fe_mul(&e, &f), y: fe_mul(&g, &h), t: fe_mul(&e, &h), z: fe_mul(&f, &g) }
    }

    fn negate(&self) -> Point {
        Point { x: fe_neg(&self.x), y: self.y, z: self.z, t: fe_neg(&self.t) }
    }

    /// Replace `self` with `other` when `choose` is 1, without branching on it.
    fn select(&mut self, other: &Point, choose: u64) {
        let mask = 0u64.wrapping_sub(choose);
        for (mine, theirs) in [
            (&mut self.x, &other.x),
            (&mut self.y, &other.y),
            (&mut self.z, &other.z),
            (&mut self.t, &other.t),
        ] {
            for i in 0..5 {
                mine.0[i] ^= mask & (mine.0[i] ^ theirs.0[i]);
            }
        }
    }

    /// [scalar]self, for a 256-bit little-endian scalar, with a fixed operation sequence.
    fn multiply(&self, scalar: &[u8; 32], d2: &Fe) -> Point {
        let mut result = Point::identity();
        for byte in scalar.iter().rev() {
            for bit in (0..8).rev() {
                result = result.add(&result, d2);
                let sum = result.add(self, d2);
                result.select(&sum, ((byte >> bit) & 1) as u64);
            }
        }
        result
    }

    /// Encoding (section 5.1.2): y with the sign of x in the top bit.
    fn encode(&self) -> [u8; 32] {
        let zinv = fe_inv(&self.z);
        let x = fe_mul(&self.x, &zinv);
        let y = fe_mul(&self.y, &zinv);
        let mut out = fe_to_bytes(&y);
        out[31] |= (fe_is_odd(&x) as u8) << 7;
        out
    }

    /// Decoding (section 5.1.3). Refuses a non-canonical y and points not on the curve.
    fn decode(bytes: &[u8; 32], d: &Fe) -> Option<Point> {
        let sign = bytes[31] >> 7;
        let mut y_bytes = *bytes;
        y_bytes[31] &= 0x7f;
        let y = fe_from_bytes(&y_bytes);
        if fe_to_bytes(&y) != y_bytes {
            return None;
        }
        let y2 = fe_sq(&y);
        let u = fe_sub(&y2, &Fe::one());
        let v = fe_add(&fe_mul(d, &y2), &Fe::one());
        let v3 = fe_mul(&fe_sq(&v), &v);
        let v7 = fe_mul(&fe_sq(&v3), &v);
        let mut x = fe_mul(&fe_mul(&u, &v3), &fe_pow(&fe_mul(&u, &v7), &power_of_two_minus(252, 3)));
        let vx2 = fe_mul(&v, &fe_sq(&x));
        if !fe_eq(&vx2, &u) {
            if fe_eq(&vx2, &fe_neg(&u)) {
                x = fe_mul(&x, &sqrt_m1());
            } else {
                return None;
            }
        }
        let zero = fe_to_bytes(&x) == [0u8; 32];
        if zero && sign == 1 {
            return None;
        }
        if fe_is_odd(&x) as u8 != sign {
            x = fe_neg(&x);
        }
        Some(Point { x, y, z: Fe::one(), t: fe_mul(&x, &y) })
    }
}

// ---------------------------------------------------------------------------
// Scalars modulo L
// ---------------------------------------------------------------------------

fn limbs(bytes: &[u8]) -> [u64; 8] {
    let mut out = [0u64; 8];
    for (i, byte) in bytes.iter().enumerate() {
        out[i / 8] |= (*byte as u64) << (8 * (i % 8));
    }
    out
}

/// `value` (up to 512 bits, little-endian) mod L, by shifting in one bit at a
/// time and subtracting L under a mask. The sequence does not depend on the value.
fn reduce(value: &[u8]) -> [u8; 32] {
    let l = limbs(&L);
    let mut r = [0u64; 5];
    for i in (0..value.len() * 8).rev() {
        let bit = ((value[i / 8] >> (i % 8)) & 1) as u64;
        // r = 2r + bit; r < L < 2^253 keeps this inside five limbs.
        let mut carry = bit;
        for limb in r.iter_mut() {
            let next = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = next;
        }
        // Subtract L, then keep the difference unless it borrowed.
        let mut difference = [0u64; 5];
        let mut borrow = 0u64;
        for j in 0..5 {
            let (step, b1) = r[j].overflowing_sub(l[j]);
            let (step, b2) = step.overflowing_sub(borrow);
            difference[j] = step;
            borrow = (b1 | b2) as u64;
        }
        let keep = borrow.wrapping_sub(1);
        for j in 0..5 {
            r[j] = (difference[j] & keep) | (r[j] & !keep);
        }
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = (r[i / 8] >> (8 * (i % 8))) as u8;
    }
    out
}

/// (a * b + c) mod L, for 256-bit little-endian a, b and c.
fn multiply_add(a: &[u8; 32], b: &[u8; 32], c: &[u8; 32]) -> [u8; 32] {
    let (a, b, c) = (limbs(a), limbs(b), limbs(c));
    let mut wide = [0u128; 9];
    for i in 0..4 {
        for j in 0..4 {
            wide[i + j] += a[i] as u128 * b[j] as u128 % (1u128 << 64);
            wide[i + j + 1] += (a[i] as u128 * b[j] as u128) >> 64;
        }
        wide[i] += c[i] as u128;
    }
    let mut out = [0u8; 72];
    let mut carry = 0u128;
    for (i, limb) in wide.iter().enumerate() {
        let total = *limb + carry;
        out[i * 8..i * 8 + 8].copy_from_slice(&(total as u64).to_le_bytes());
        carry = total >> 64;
    }
    reduce(&out)
}

fn less_than_l(scalar: &[u8; 32]) -> bool {
    for i in (0..32).rev() {
        if scalar[i] != L[i] {
            return scalar[i] < L[i];
        }
    }
    false
}

fn hash(parts: &[&[u8]]) -> [u8; 64] {
    let mut hasher = Sha512::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize()
}

// ---------------------------------------------------------------------------
// Keys and signatures
// ---------------------------------------------------------------------------

/// An Ed25519 private key: the 32-byte seed and what RFC 8032 derives from it.
pub struct SigningKey {
    seed: [u8; SEED_LEN],
    scalar: [u8; 32],
    prefix: [u8; 32],
    public: [u8; PUBLIC_KEY_LEN],
}

impl SigningKey {
    /// Derive the key from its seed (section 5.1.5).
    pub fn from_seed(seed: &[u8; SEED_LEN]) -> Self {
        let digest = hash(&[seed]);
        let mut scalar = [0u8; 32];
        scalar.copy_from_slice(&digest[..32]);
        scalar[0] &= 248;
        scalar[31] &= 127;
        scalar[31] |= 64;
        let mut prefix = [0u8; 32];
        prefix.copy_from_slice(&digest[32..]);
        let d = curve_d();
        let d2 = fe_add(&d, &d);
        let base = Point::decode(&BASE, &d).expect("the base point decodes");
        let public = base.multiply(&scalar, &d2).encode();
        SigningKey { seed: *seed, scalar, prefix, public }
    }

    pub fn seed(&self) -> &[u8; SEED_LEN] {
        &self.seed
    }

    pub fn public_key(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.public
    }

    /// Sign `message` (section 5.1.6).
    pub fn sign(&self, message: &[u8]) -> [u8; SIGNATURE_LEN] {
        let d = curve_d();
        let d2 = fe_add(&d, &d);
        let base = Point::decode(&BASE, &d).expect("the base point decodes");
        let r = reduce(&hash(&[&self.prefix, message]));
        let big_r = base.multiply(&r, &d2).encode();
        let k = reduce(&hash(&[&big_r, &self.public, message]));
        let s = multiply_add(&k, &self.scalar, &r);
        let mut signature = [0u8; SIGNATURE_LEN];
        signature[..32].copy_from_slice(&big_r);
        signature[32..].copy_from_slice(&s);
        signature
    }
}

impl Drop for SigningKey {
    fn drop(&mut self) {
        for byte in self.seed.iter_mut().chain(self.scalar.iter_mut()).chain(self.prefix.iter_mut()) {
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
    }
}

/// Verify `signature` over `message` by `public_key` (section 5.1.7): S must be
/// below L, R and A must decode, and [S]B must equal R + [k]A.
pub fn verify(public_key: &[u8; PUBLIC_KEY_LEN], message: &[u8], signature: &[u8; SIGNATURE_LEN]) -> bool {
    let d = curve_d();
    let d2 = fe_add(&d, &d);
    let mut r_bytes = [0u8; 32];
    r_bytes.copy_from_slice(&signature[..32]);
    let mut s = [0u8; 32];
    s.copy_from_slice(&signature[32..]);
    if !less_than_l(&s) {
        return false;
    }
    let (Some(a), Some(_)) = (Point::decode(public_key, &d), Point::decode(&r_bytes, &d)) else {
        return false;
    };
    let base = Point::decode(&BASE, &d).expect("the base point decodes");
    let k = reduce(&hash(&[&r_bytes, public_key, message]));
    let check = base.multiply(&s, &d2).add(&a.multiply(&k, &d2).negate(), &d2);
    check.encode() == r_bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    fn array<const N: usize>(text: &str) -> [u8; N] {
        hex(text).try_into().unwrap()
    }

    // RFC 8032 section 7.1, TEST 1 to TEST 3.
    const VECTORS: [(&str, &str, &str, &str); 3] = [
        (
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "",
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        ),
        (
            "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
            "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
            "72",
            "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
        ),
        (
            "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
            "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
            "af82",
            "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
        ),
    ];

    #[test]
    fn rfc8032_vectors_derive_sign_and_verify() {
        for (seed, public, message, signature) in VECTORS {
            let key = SigningKey::from_seed(&array(seed));
            assert_eq!(key.public_key().to_vec(), hex(public));
            assert_eq!(key.sign(&hex(message)).to_vec(), hex(signature));
            assert!(verify(&array(public), &hex(message), &array(signature)));
        }
    }

    #[test]
    fn altered_message_key_or_signature_is_refused() {
        let (_, public, message, signature) = VECTORS[2];
        let (public, signature) = (array::<32>(public), array::<64>(signature));
        let mut changed = hex(message);
        changed[0] ^= 1;
        assert!(!verify(&public, &changed, &signature));
        let other = SigningKey::from_seed(&array(VECTORS[0].0)).public_key();
        assert!(!verify(&other, &hex(message), &signature));
        for index in [0, 31, 32, 63] {
            let mut bad = signature;
            bad[index] ^= 0x01;
            assert!(!verify(&public, &hex(message), &bad), "byte {index}");
        }
    }

    #[test]
    fn signature_scalar_at_or_above_the_order_is_refused() {
        let (_, public, message, signature) = VECTORS[0];
        let mut bad = array::<64>(signature);
        // S + L satisfies the group equation but is not the canonical S.
        let mut carry = 0u16;
        for i in 0..32 {
            let total = bad[32 + i] as u16 + L[i] as u16 + carry;
            bad[32 + i] = total as u8;
            carry = total >> 8;
        }
        assert_eq!(carry, 0);
        assert!(!verify(&array(public), &hex(message), &bad));
    }

    #[test]
    fn order_times_base_is_the_identity_and_reduction_matches() {
        let d = curve_d();
        let d2 = fe_add(&d, &d);
        let base = Point::decode(&BASE, &d).unwrap();
        assert_eq!(base.multiply(&L, &d2).encode(), Point::identity().encode());
        assert_eq!(base.encode(), BASE);
        let mut wide = [0u8; 64];
        wide[..32].copy_from_slice(&L);
        assert_eq!(reduce(&wide), [0u8; 32]);
        wide[0] = wide[0].wrapping_add(5);
        let mut five = [0u8; 32];
        five[0] = 5;
        assert_eq!(reduce(&wide), five);
    }

    #[test]
    fn invalid_points_are_refused() {
        // y = p is not canonical; y = 2 is not on the curve.
        let mut p = [0xffu8; 32];
        p[0] = 0xed;
        p[31] = 0x7f;
        assert!(Point::decode(&p, &curve_d()).is_none());
        let mut two = [0u8; 32];
        two[0] = 2;
        assert!(Point::decode(&two, &curve_d()).is_none());
    }
}
