//! Curve25519 ECDHE key exchange (RFC 7748)
//!
//! Pure Rust implementation of X25519 Diffie-Hellman key exchange using
//! the Montgomery curve Curve25519. All operations are constant-time to
//! prevent timing side-channel attacks.
//!
//! Field elements are in GF(2^255 - 19), represented as 5 limbs of 51 bits.

// ---------------------------------------------------------------------------
// Field element: GF(2^255 - 19)
// ---------------------------------------------------------------------------

/// Field element in GF(2^255-19), represented as 5 limbs of 51 bits each.
///
/// value = limbs[0] + limbs[1]*2^51 + limbs[2]*2^102 + limbs[3]*2^153 + limbs[4]*2^204
#[derive(Clone, Copy, Debug)]
struct Fe([u64; 5]);

/// Mask for the lower 51 bits.
const MASK51: u64 = (1u64 << 51) - 1;

/// The curve constant a24 = (A - 2) / 4 where A = 486662 for Curve25519.
/// a24 = (486662 - 2) / 4 = 121665.
const A24: u64 = 121665;

impl Fe {
    /// The zero element.
    fn zero() -> Fe {
        Fe([0u64; 5])
    }

    /// The one element.
    fn one() -> Fe {
        Fe([1, 0, 0, 0, 0])
    }
}

// ---------------------------------------------------------------------------
// Field arithmetic
// ---------------------------------------------------------------------------

/// Carry-propagate and reduce modulo 2^255 - 19.
///
/// After operations like addition or subtraction, limbs may exceed 51 bits.
/// This function propagates carries from each limb to the next, and wraps
/// overflow from limb 4 back to limb 0 multiplied by 19 (since 2^255 = 19
/// mod p).
fn fe_reduce(a: &Fe) -> Fe {
    let mut h = a.0;

    // First pass: propagate carries upward
    let mut carry: u64;
    carry = h[0] >> 51;
    h[0] &= MASK51;
    h[1] += carry;

    carry = h[1] >> 51;
    h[1] &= MASK51;
    h[2] += carry;

    carry = h[2] >> 51;
    h[2] &= MASK51;
    h[3] += carry;

    carry = h[3] >> 51;
    h[3] &= MASK51;
    h[4] += carry;

    // Wrap overflow from limb 4 back to limb 0 (2^255 = 19 mod p)
    carry = h[4] >> 51;
    h[4] &= MASK51;
    h[0] += carry * 19;

    // Second pass: one more carry from limb 0 in case carry*19 overflowed
    carry = h[0] >> 51;
    h[0] &= MASK51;
    h[1] += carry;

    Fe(h)
}

/// Field addition: a + b (mod p).
fn fe_add(a: &Fe, b: &Fe) -> Fe {
    let h = Fe([
        a.0[0] + b.0[0],
        a.0[1] + b.0[1],
        a.0[2] + b.0[2],
        a.0[3] + b.0[3],
        a.0[4] + b.0[4],
    ]);
    fe_reduce(&h)
}

/// Field subtraction: a - b (mod p).
///
/// Uses the ref10 approach: add 2*p to `a` before subtracting `b` to
/// guarantee every limb stays non-negative.
///
/// 2*p in 5-limb representation:
///   limb 0: 2*(2^51 - 19) = 2^52 - 38 = 0xFFFFFFFFFFFDA
///   limb i (1..4): 2*(2^51 - 1) = 2^52 - 2 = 0xFFFFFFFFFFFFE
fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    let h = Fe([
        (a.0[0] + 0xFFFFFFFFFFFDA) - b.0[0],
        (a.0[1] + 0xFFFFFFFFFFFFE) - b.0[1],
        (a.0[2] + 0xFFFFFFFFFFFFE) - b.0[2],
        (a.0[3] + 0xFFFFFFFFFFFFE) - b.0[3],
        (a.0[4] + 0xFFFFFFFFFFFFE) - b.0[4],
    ]);
    fe_reduce(&h)
}

/// Field multiplication: a * b (mod p).
///
/// Schoolbook 5x5 multiplication using u128 intermediates. The reduction
/// uses the identity 2^255 = 19 (mod p): any overflow past limb 4 is
/// multiplied by 19 and folded back into the lower limbs.
fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    let a0 = a.0[0] as u128;
    let a1 = a.0[1] as u128;
    let a2 = a.0[2] as u128;
    let a3 = a.0[3] as u128;
    let a4 = a.0[4] as u128;

    let b0 = b.0[0] as u128;
    let b1 = b.0[1] as u128;
    let b2 = b.0[2] as u128;
    let b3 = b.0[3] as u128;
    let b4 = b.0[4] as u128;

    // Pre-multiply by 19 for the reduction step.
    // When a_i * b_j lands in limb >= 5, we need to multiply by 19.
    let b1_19 = (b.0[1] * 19) as u128;
    let b2_19 = (b.0[2] * 19) as u128;
    let b3_19 = (b.0[3] * 19) as u128;
    let b4_19 = (b.0[4] * 19) as u128;

    // Accumulate products for each output limb.
    // Limb k gets contributions from a_i * b_j where (i+j) mod 5 == k,
    // with a factor of 19 when i+j >= 5.
    let t0 = a0 * b0 + a1 * b4_19 + a2 * b3_19 + a3 * b2_19 + a4 * b1_19;
    let t1 = a0 * b1 + a1 * b0 + a2 * b4_19 + a3 * b3_19 + a4 * b2_19;
    let t2 = a0 * b2 + a1 * b1 + a2 * b0 + a3 * b4_19 + a4 * b3_19;
    let t3 = a0 * b3 + a1 * b2 + a2 * b1 + a3 * b0 + a4 * b4_19;
    let t4 = a0 * b4 + a1 * b3 + a2 * b2 + a3 * b1 + a4 * b0;

    // Carry propagation
    let mut r0 = t0 as u64 & MASK51;
    let c = (t0 >> 51) as u64;

    let t1 = t1 + c as u128;
    let mut r1 = t1 as u64 & MASK51;
    let c = (t1 >> 51) as u64;

    let t2 = t2 + c as u128;
    let mut r2 = t2 as u64 & MASK51;
    let c = (t2 >> 51) as u64;

    let t3 = t3 + c as u128;
    let r3 = t3 as u64 & MASK51;
    let c = (t3 >> 51) as u64;

    let t4 = t4 + c as u128;
    let r4 = t4 as u64 & MASK51;
    let c = (t4 >> 51) as u64;

    // Wrap carry from limb 4 back to limb 0
    r0 += c * 19;
    let c = r0 >> 51;
    r0 &= MASK51;
    r1 += c;
    let c = r1 >> 51;
    r1 &= MASK51;
    r2 += c;

    Fe([r0, r1, r2, r3, r4])
}

/// Field squaring: a^2 (mod p).
///
/// Optimized version of fe_mul(a, a) that takes advantage of the symmetry
/// in squaring: cross terms appear twice, so we double them instead of
/// computing both i*j and j*i.
fn fe_sq(a: &Fe) -> Fe {
    let a0 = a.0[0] as u128;
    let a1 = a.0[1] as u128;
    let a2 = a.0[2] as u128;
    let a3 = a.0[3] as u128;
    let a4 = a.0[4] as u128;

    // Doubled terms for cross products
    let a0_2 = 2 * a0;
    let a1_2 = 2 * a1;
    let a2_2 = 2 * a2;

    // Pre-multiplied by 19 for reduction (terms that wrap around)
    let a3_19 = (a.0[3] * 19) as u128;
    let a4_19 = (a.0[4] * 19) as u128;

    // t0: a0*a0 + 2*(a1*a4 + a2*a3) * 19
    let t0 = a0 * a0 + a1_2 * a4_19 + a2_2 * a3_19;
    // t1: 2*a0*a1 + 2*a2*a4*19 + a3*a3*19
    let t1 = a0_2 * a1 + a2_2 * a4_19 + a3 * a3_19;
    // t2: 2*a0*a2 + a1*a1 + 2*a3*a4*19
    let t2 = a0_2 * a2 + a1 * a1 + 2 * a3_19 * a4;
    // t3: 2*a0*a3 + 2*a1*a2 + a4*a4*19
    let t3 = a0_2 * a3 + a1_2 * a2 + a4 * a4_19;
    // t4: 2*a0*a4 + 2*a1*a3 + a2*a2
    let t4 = a0_2 * a4 + a1_2 * a3 + a2 * a2;

    // Carry propagation (identical to fe_mul)
    let mut r0 = t0 as u64 & MASK51;
    let c = (t0 >> 51) as u64;

    let t1 = t1 + c as u128;
    let mut r1 = t1 as u64 & MASK51;
    let c = (t1 >> 51) as u64;

    let t2 = t2 + c as u128;
    let mut r2 = t2 as u64 & MASK51;
    let c = (t2 >> 51) as u64;

    let t3 = t3 + c as u128;
    let r3 = t3 as u64 & MASK51;
    let c = (t3 >> 51) as u64;

    let t4 = t4 + c as u128;
    let r4 = t4 as u64 & MASK51;
    let c = (t4 >> 51) as u64;

    r0 += c * 19;
    let c = r0 >> 51;
    r0 &= MASK51;
    r1 += c;
    let c = r1 >> 51;
    r1 &= MASK51;
    r2 += c;

    Fe([r0, r1, r2, r3, r4])
}

/// Repeated squaring: compute a^(2^n) by squaring n times.
fn fe_sq_n(a: &Fe, n: u32) -> Fe {
    let mut result = fe_sq(a);
    for _ in 1..n {
        result = fe_sq(&result);
    }
    result
}

/// Field inversion: a^(-1) (mod p) via Fermat's little theorem.
///
/// Computes a^(p-2) where p = 2^255 - 19, so p-2 = 2^255 - 21.
///
/// Uses the standard ref10/donna addition chain:
///
/// z2  = a^2
/// z9  = z2^4 * a       => a^9
/// z11 = z9 * z2         => a^11
/// z_5_0 = z11^2 * z9    => a^31 = a^(2^5 - 1)
/// z_10_0 = z_5_0^(2^5) * z_5_0   => a^(2^10 - 1)
/// z_20_0 = z_10_0^(2^10) * z_10_0 => a^(2^20 - 1)
/// z_40_0 = z_20_0^(2^20) * z_20_0 => a^(2^40 - 1)
/// z_50_0 = z_40_0^(2^10) * z_10_0 => a^(2^50 - 1)
/// z_100_0 = z_50_0^(2^50) * z_50_0 => a^(2^100 - 1)
/// z_200_0 = z_100_0^(2^100) * z_100_0 => a^(2^200 - 1)
/// z_250_0 = z_200_0^(2^50) * z_50_0 => a^(2^250 - 1)
/// result = z_250_0^(2^5) * z11 => a^(2^255 - 21)
fn fe_inv(a: &Fe) -> Fe {
    let a1 = *a;

    // z2 = a^2
    let z2 = fe_sq(&a1);

    // t = a^4, then a^8
    let t = fe_sq(&z2);
    let t = fe_sq(&t);

    // z9 = a^8 * a = a^9
    let z9 = fe_mul(&t, &a1);

    // z11 = a^9 * a^2 = a^11
    let z11 = fe_mul(&z9, &z2);

    // t = a^22
    let t = fe_sq(&z11);

    // z_5_0 = a^22 * a^9 = a^31 = a^(2^5 - 1)
    let z_5_0 = fe_mul(&t, &z9);

    // z_10_0 = a^(2^10 - 1)
    let t = fe_sq_n(&z_5_0, 5);
    let z_10_0 = fe_mul(&t, &z_5_0);

    // z_20_0 = a^(2^20 - 1)
    let t = fe_sq_n(&z_10_0, 10);
    let z_20_0 = fe_mul(&t, &z_10_0);

    // z_40_0 = a^(2^40 - 1)
    let t = fe_sq_n(&z_20_0, 20);
    let z_40_0 = fe_mul(&t, &z_20_0);

    // z_50_0 = a^(2^50 - 1)
    let t = fe_sq_n(&z_40_0, 10);
    let z_50_0 = fe_mul(&t, &z_10_0);

    // z_100_0 = a^(2^100 - 1)
    let t = fe_sq_n(&z_50_0, 50);
    let z_100_0 = fe_mul(&t, &z_50_0);

    // z_200_0 = a^(2^200 - 1)
    let t = fe_sq_n(&z_100_0, 100);
    let z_200_0 = fe_mul(&t, &z_100_0);

    // z_250_0 = a^(2^250 - 1)
    let t = fe_sq_n(&z_200_0, 50);
    let z_250_0 = fe_mul(&t, &z_50_0);

    // Final: a^(2^255 - 32 + 11) = a^(2^255 - 21) = a^(p-2)
    let t = fe_sq_n(&z_250_0, 5);
    fe_mul(&t, &z11)
}

// ---------------------------------------------------------------------------
// Byte encoding / decoding
// ---------------------------------------------------------------------------

/// Decode 32 little-endian bytes into a field element.
///
/// The top bit (bit 255) is masked off per RFC 7748.
fn fe_from_bytes(bytes: &[u8; 32]) -> Fe {
    let mut s = *bytes;
    // Mask the top bit (bit 255 = bit 7 of byte 31)
    s[31] &= 0x7f;

    // Read as a 256-bit little-endian integer split into 51-bit limbs.
    // Each limb spans certain byte boundaries.
    let load8 = |src: &[u8]| -> u64 {
        let mut v = 0u64;
        for i in 0..core::cmp::min(8, src.len()) {
            v |= (src[i] as u64) << (8 * i);
        }
        v
    };

    let mut h = [0u64; 5];
    h[0] = load8(&s[0..]) & MASK51;
    h[1] = (load8(&s[6..]) >> 3) & MASK51;
    h[2] = (load8(&s[12..]) >> 6) & MASK51;
    h[3] = (load8(&s[19..]) >> 1) & MASK51;
    h[4] = (load8(&s[24..]) >> 12) & MASK51;

    Fe(h)
}

/// Fully reduce a field element and encode as 32 little-endian bytes.
///
/// Performs a full canonical reduction to ensure the output is in [0, p).
fn fe_to_bytes(f: &Fe) -> [u8; 32] {
    // First, reduce carries
    let mut h = fe_reduce(f).0;

    // Fully reduce: if h >= p, subtract p.
    // To check: add 19 to h[0]. If this causes a carry all the way
    // through to overflow limb 4, then h >= p.
    let mut q = (h[0] + 19) >> 51;
    q = (h[1] + q) >> 51;
    q = (h[2] + q) >> 51;
    q = (h[3] + q) >> 51;
    q = (h[4] + q) >> 51;
    // q is now 1 if h >= p, 0 otherwise

    // Conditionally subtract p by adding q*19 and masking to 255 bits
    h[0] += q * 19;
    let c = h[0] >> 51;
    h[0] &= MASK51;
    h[1] += c;
    let c = h[1] >> 51;
    h[1] &= MASK51;
    h[2] += c;
    let c = h[2] >> 51;
    h[2] &= MASK51;
    h[3] += c;
    let c = h[3] >> 51;
    h[3] &= MASK51;
    h[4] += c;
    h[4] &= MASK51;

    // Pack limbs into 32 little-endian bytes using store_limb helper.
    // Each limb is 51 bits starting at bit offset i*51.
    let mut out = [0u8; 32];
    store_limb(&mut out, h[0], 0);
    store_limb(&mut out, h[1], 51);
    store_limb(&mut out, h[2], 102);
    store_limb(&mut out, h[3], 153);
    store_limb(&mut out, h[4], 204);

    out
}

/// Store a 51-bit limb value into a byte array at the given bit offset.
fn store_limb(out: &mut [u8; 32], val: u64, bit_offset: usize) {
    let byte_start = bit_offset / 8;
    let bit_shift = bit_offset % 8;

    // The limb, shifted, can span up to 8 bytes (51 bits + 7 bit shift = 58 bits)
    let shifted = (val as u128) << bit_shift;
    for i in 0..8 {
        if byte_start + i < 32 {
            out[byte_start + i] |= (shifted >> (8 * i)) as u8;
        }
    }
}

// ---------------------------------------------------------------------------
// Constant-time operations
// ---------------------------------------------------------------------------

/// Constant-time conditional swap.
///
/// If `swap` is 1, swap the values of `a` and `b`. If `swap` is 0, do nothing.
/// The `swap` parameter MUST be 0 or 1. This function runs in constant time
/// regardless of the value of `swap`.
fn fe_cswap(a: &mut Fe, b: &mut Fe, swap: u64) {
    // Expand swap to a full mask: 0 -> 0x0000..., 1 -> 0xFFFF...
    let mask = 0u64.wrapping_sub(swap);
    for i in 0..5 {
        let t = mask & (a.0[i] ^ b.0[i]);
        a.0[i] ^= t;
        b.0[i] ^= t;
    }
}

/// Multiply a field element by a small constant (used for a24 = 121665).
fn fe_mul_small(a: &Fe, c: u64) -> Fe {
    let c128 = c as u128;
    let t0 = a.0[0] as u128 * c128;
    let t1 = a.0[1] as u128 * c128;
    let t2 = a.0[2] as u128 * c128;
    let t3 = a.0[3] as u128 * c128;
    let t4 = a.0[4] as u128 * c128;

    let mut r0 = t0 as u64 & MASK51;
    let carry = (t0 >> 51) as u64;

    let t1 = t1 + carry as u128;
    let mut r1 = t1 as u64 & MASK51;
    let carry = (t1 >> 51) as u64;

    let t2 = t2 + carry as u128;
    let mut r2 = t2 as u64 & MASK51;
    let carry = (t2 >> 51) as u64;

    let t3 = t3 + carry as u128;
    let r3 = t3 as u64 & MASK51;
    let carry = (t3 >> 51) as u64;

    let t4 = t4 + carry as u128;
    let r4 = t4 as u64 & MASK51;
    let carry = (t4 >> 51) as u64;

    r0 += carry * 19;
    let carry = r0 >> 51;
    r0 &= MASK51;
    r1 += carry;
    let carry = r1 >> 51;
    r1 &= MASK51;
    r2 += carry;

    Fe([r0, r1, r2, r3, r4])
}

// ---------------------------------------------------------------------------
// X25519 scalar multiplication
// ---------------------------------------------------------------------------

/// X25519 scalar multiplication (RFC 7748).
///
/// Computes `scalar * point` on Curve25519 using the Montgomery ladder.
/// Both `scalar` and `point` are 32-byte little-endian encodings.
///
/// Returns the 32-byte little-endian encoding of the resulting x-coordinate.
pub fn x25519(scalar: &[u8; 32], point: &[u8; 32]) -> [u8; 32] {
    // Clamp the scalar per RFC 7748
    let mut k = *scalar;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;

    // Decode the u-coordinate
    let u = fe_from_bytes(point);

    // Montgomery ladder
    let x_1 = u;
    let mut x_2 = Fe::one();
    let mut z_2 = Fe::zero();
    let mut x_3 = u;
    let mut z_3 = Fe::one();

    let mut swap: u64 = 0;

    // Process bits from 254 down to 0 (bit 255 is always 0 after clamping)
    for pos in (0..255).rev() {
        let byte_idx = pos / 8;
        let bit_idx = pos % 8;
        let k_t = ((k[byte_idx] >> bit_idx) & 1) as u64;

        swap ^= k_t;
        fe_cswap(&mut x_2, &mut x_3, swap);
        fe_cswap(&mut z_2, &mut z_3, swap);
        swap = k_t;

        // Montgomery ladder step (RFC 7748 differential addition + doubling)
        let a = fe_add(&x_2, &z_2);
        let aa = fe_sq(&a);
        let b = fe_sub(&x_2, &z_2);
        let bb = fe_sq(&b);
        let e = fe_sub(&aa, &bb);
        let c = fe_add(&x_3, &z_3);
        let d = fe_sub(&x_3, &z_3);
        let da = fe_mul(&d, &a);
        let cb = fe_mul(&c, &b);
        x_3 = fe_sq(&fe_add(&da, &cb));
        z_3 = fe_mul(&x_1, &fe_sq(&fe_sub(&da, &cb)));
        x_2 = fe_mul(&aa, &bb);
        z_2 = fe_mul(&e, &fe_add(&aa, &fe_mul_small(&e, A24)));
    }

    // Final conditional swap
    fe_cswap(&mut x_2, &mut x_3, swap);
    fe_cswap(&mut z_2, &mut z_3, swap);

    // Compute result = x_2 * z_2^(-1)
    let result = fe_mul(&x_2, &fe_inv(&z_2));
    fe_to_bytes(&result)
}

/// X25519 with the standard base point (9).
///
/// Computes `scalar * G` where G is the Curve25519 base point with
/// u-coordinate 9.
pub fn x25519_basepoint(scalar: &[u8; 32]) -> [u8; 32] {
    let mut basepoint = [0u8; 32];
    basepoint[0] = 9;
    x25519(scalar, &basepoint)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a hex string into a 32-byte array (little-endian as given).
    fn hex_to_bytes(hex: &str) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        for i in 0..32 {
            let hi = hex_digit(hex.as_bytes()[2 * i]);
            let lo = hex_digit(hex.as_bytes()[2 * i + 1]);
            bytes[i] = (hi << 4) | lo;
        }
        bytes
    }

    fn hex_digit(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("invalid hex digit"),
        }
    }

    fn bytes_to_hex(bytes: &[u8; 32]) -> String {
        let mut s = String::with_capacity(64);
        for b in bytes {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }

    #[test]
    fn test_fe_roundtrip() {
        // Test that encoding then decoding a field element is the identity
        let bytes: [u8; 32] = [
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
        ];
        let fe = fe_from_bytes(&bytes);
        let out = fe_to_bytes(&fe);
        assert_eq!(bytes, out);
    }

    #[test]
    fn test_fe_zero_roundtrip() {
        let bytes = [0u8; 32];
        let fe = fe_from_bytes(&bytes);
        let out = fe_to_bytes(&fe);
        assert_eq!(bytes, out);
    }

    #[test]
    fn test_fe_add_sub() {
        let a = Fe::one();
        let b = Fe::one();
        let sum = fe_add(&a, &b);
        let diff = fe_sub(&sum, &b);
        let out = fe_to_bytes(&diff);
        let expected = fe_to_bytes(&a);
        assert_eq!(out, expected);
    }

    #[test]
    fn test_fe_mul_identity() {
        // a * 1 = a
        let a = Fe([12345, 67890, 11111, 22222, 33333]);
        let one = Fe::one();
        let product = fe_mul(&a, &one);
        assert_eq!(fe_to_bytes(&product), fe_to_bytes(&a));
    }

    #[test]
    fn test_fe_inv() {
        // a * a^(-1) = 1
        let a = Fe([12345, 67890, 11111, 22222, 33333]);
        let a_inv = fe_inv(&a);
        let product = fe_mul(&a, &a_inv);
        let out = fe_to_bytes(&product);
        let one = fe_to_bytes(&Fe::one());
        assert_eq!(out, one);
    }

    #[test]
    fn test_fe_sq_vs_mul() {
        // a^2 should equal a * a
        let a = Fe([12345, 67890, 11111, 22222, 33333]);
        let sq = fe_sq(&a);
        let mul = fe_mul(&a, &a);
        assert_eq!(fe_to_bytes(&sq), fe_to_bytes(&mul));
    }

    #[test]
    fn test_rfc7748_vector_1() {
        let scalar = hex_to_bytes(
            "a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4",
        );
        let u_coord = hex_to_bytes(
            "e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c",
        );
        let expected = hex_to_bytes(
            "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552",
        );

        let result = x25519(&scalar, &u_coord);
        assert_eq!(
            bytes_to_hex(&result),
            bytes_to_hex(&expected),
            "RFC 7748 test vector 1 failed"
        );
    }

    #[test]
    fn test_rfc7748_vector_2() {
        let scalar = hex_to_bytes(
            "4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d",
        );
        let u_coord = hex_to_bytes(
            "e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493",
        );
        let expected = hex_to_bytes(
            "95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957",
        );

        let result = x25519(&scalar, &u_coord);
        assert_eq!(
            bytes_to_hex(&result),
            bytes_to_hex(&expected),
            "RFC 7748 test vector 2 failed"
        );
    }

    #[test]
    fn test_basepoint() {
        // Test that x25519_basepoint produces the same result as x25519 with point=9
        let scalar = hex_to_bytes(
            "a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4",
        );
        let mut basepoint = [0u8; 32];
        basepoint[0] = 9;

        let r1 = x25519(&scalar, &basepoint);
        let r2 = x25519_basepoint(&scalar);
        assert_eq!(r1, r2, "basepoint helper should match x25519 with point=9");
    }

    #[test]
    fn test_rfc7748_iterated_1() {
        // RFC 7748 Section 5.2: After 1 iteration
        let mut k = [0u8; 32];
        k[0] = 9;
        let mut u = [0u8; 32];
        u[0] = 9;

        let result = x25519(&k, &u);

        let expected = hex_to_bytes(
            "422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079",
        );
        assert_eq!(
            bytes_to_hex(&result),
            bytes_to_hex(&expected),
            "RFC 7748 iterated test (1 iteration) failed"
        );
    }

    #[test]
    fn test_rfc7748_iterated_1000() {
        // RFC 7748 Section 5.2: After 1000 iterations
        let mut k = [0u8; 32];
        k[0] = 9;
        let mut u = [0u8; 32];
        u[0] = 9;

        for _ in 0..1000 {
            let new_k = x25519(&k, &u);
            u = k;
            k = new_k;
        }

        let expected = hex_to_bytes(
            "684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51",
        );
        assert_eq!(
            bytes_to_hex(&k),
            bytes_to_hex(&expected),
            "RFC 7748 iterated test (1000 iterations) failed"
        );
    }

    #[test]
    fn test_cswap_does_swap() {
        let mut a = Fe([1, 2, 3, 4, 5]);
        let mut b = Fe([6, 7, 8, 9, 10]);
        fe_cswap(&mut a, &mut b, 1);
        assert_eq!(a.0, [6, 7, 8, 9, 10]);
        assert_eq!(b.0, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_cswap_no_swap() {
        let mut a = Fe([1, 2, 3, 4, 5]);
        let mut b = Fe([6, 7, 8, 9, 10]);
        fe_cswap(&mut a, &mut b, 0);
        assert_eq!(a.0, [1, 2, 3, 4, 5]);
        assert_eq!(b.0, [6, 7, 8, 9, 10]);
    }
}
