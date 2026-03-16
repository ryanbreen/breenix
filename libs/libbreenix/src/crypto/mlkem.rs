//! ML-KEM 768 (FIPS 203) — Post-quantum key encapsulation mechanism.
//!
//! Pure Rust implementation of ML-KEM 768 (formerly CRYSTALS-Kyber) for use
//! in SSH hybrid key exchange (`mlkem768x25519-sha256`). All arithmetic is
//! performed in Z_q where q = 3329, using Montgomery multiplication for
//! efficiency in NTT domain operations.
//!
//! Coefficients are stored as `i16` values in the range `(-q, q)` to allow
//! signed intermediate results during NTT butterflies without overflow. This
//! matches the approach used by the pqcrystals reference implementation.
//!
//! # Parameters (ML-KEM 768)
//!
//! | Parameter | Value |
//! |-----------|-------|
//! | n         | 256   |
//! | k         | 3     |
//! | q         | 3329  |
//! | eta1      | 2     |
//! | eta2      | 2     |
//! | du        | 10    |
//! | dv        | 4     |
//!
//! # Key sizes
//!
//! | Object        | Bytes |
//! |---------------|-------|
//! | Public key    | 1184  |
//! | Secret key    | 2400  |
//! | Ciphertext    | 1088  |
//! | Shared secret | 32    |

use super::keccak::{sha3_256, sha3_512, Shake128, Shake256};

// ---------------------------------------------------------------------------
// ML-KEM 768 parameters
// ---------------------------------------------------------------------------

/// Polynomial degree.
const N: usize = 256;

/// Module dimension.
const K: usize = 3;

/// Modulus.
const Q: i16 = 3329;

/// Noise distribution parameter for key generation.
const ETA1: usize = 2;

/// Noise distribution parameter for encryption.
const ETA2: usize = 2;

/// Compression bits for ciphertext component u.
const DU: u32 = 10;

/// Compression bits for ciphertext component v.
const DV: u32 = 4;

// ---------------------------------------------------------------------------
// Montgomery domain constants
// ---------------------------------------------------------------------------

/// q^{-1} mod 2^16, stored as a signed i16.
///
/// As unsigned: 62209 (since 3329 * 62209 = 1 mod 65536).
/// As signed i16: 62209 - 65536 = -3327.
/// This matches the pqcrystals reference implementation.
const QINV: i16 = -3327;

// ---------------------------------------------------------------------------
// NTT zeta tables (precomputed, in Montgomery domain)
// ---------------------------------------------------------------------------

/// Precomputed zetas for the NTT, in Montgomery domain (bit-reversed order).
///
/// zetas[i] = (17^{bitrev7(i)}) * 2^16 mod q, stored as i16 in (-q, q).
/// The primitive 256th root of unity in Z_3329 is 17.
///
/// These values are from the pqcrystals/kyber reference implementation.
const ZETAS: [i16; 128] = [
    2285, 2571, 2970, 1812, 1493, 1422, 287, 202, 3158, 622, 1577, 182, 962, 2127, 1855, 1468,
    573, 2004, 264, 383, 2500, 1458, 1727, 3199, 2648, 1017, 732, 608, 1787, 411, 3124, 1758,
    1223, 652, 2777, 1015, 2036, 1491, 3047, 1785, 516, 3321, 3009, 2663, 1711, 2167, 126, 1469,
    2476, 3239, 3058, 830, 107, 1908, 3082, 2378, 2931, 961, 1821, 2604, 448, 2264, 677, 2054,
    2226, 430, 555, 843, 2078, 871, 1550, 105, 422, 587, 177, 3094, 3038, 2869, 1574, 1653, 3083,
    778, 1159, 3182, 2552, 1483, 2727, 1119, 1739, 644, 2457, 349, 418, 329, 3173, 3254, 817,
    1097, 603, 610, 1322, 2044, 1864, 384, 2114, 3193, 1218, 1994, 2455, 220, 2142, 1670, 2144,
    1799, 2051, 794, 1819, 2475, 2459, 478, 3221, 3021, 996, 991, 958, 1869, 1522, 1628,
];

// ---------------------------------------------------------------------------
// Montgomery arithmetic (signed, matching reference implementation)
// ---------------------------------------------------------------------------

/// Montgomery reduction: given a 32-bit value `a`, compute a * R^{-1} mod q
/// where R = 2^16.
///
/// Uses the identity: t = (a mod R) * QINV mod R, then (a - t * q) / R
/// is congruent to a * R^{-1} mod q. Result is in the range (-q, q).
///
/// Matches the pqcrystals/kyber reference implementation.
#[inline(always)]
fn montgomery_reduce(a: i32) -> i16 {
    let t = (a as i16).wrapping_mul(QINV);
    ((a - (t as i32) * (Q as i32)) >> 16) as i16
}

/// Barrett reduction: reduce an i16 value to the range (-q/2, q/2).
///
/// For input in (-2^15, 2^15), produces output congruent to input mod q
/// with |output| <= q/2.
#[inline(always)]
fn barrett_reduce(a: i16) -> i16 {
    let v: i16 = ((1i32 << 26) / Q as i32 + 1) as i16; // 20159
    let t = ((v as i32 * a as i32 + (1 << 25)) >> 26) as i16;
    a - t * Q
}

/// Fully reduce to canonical range [0, q).
#[inline(always)]
fn to_positive(a: i16) -> u16 {
    let r = barrett_reduce(a);
    (r + ((r >> 15) & Q)) as u16
}

/// Multiply two Montgomery-domain values and reduce.
#[inline(always)]
fn mont_mul(a: i16, b: i16) -> i16 {
    montgomery_reduce(a as i32 * b as i32)
}

// ---------------------------------------------------------------------------
// Polynomial type
// ---------------------------------------------------------------------------

/// A polynomial in Z_q[X] / (X^256 + 1).
///
/// Coefficients are stored as i16 values. In normal domain, they represent
/// values in [0, q). In NTT domain, they are in Montgomery form.
#[derive(Clone)]
struct Poly {
    coeffs: [i16; N],
}

impl Poly {
    /// Create a zero polynomial.
    fn zero() -> Self {
        Poly { coeffs: [0i16; N] }
    }

    /// Forward NTT (Cooley-Tukey butterflies, bit-reversed to natural order).
    fn ntt(&mut self) {
        let mut k: usize = 1;
        let mut len = 128;
        while len >= 2 {
            let mut start = 0;
            while start < N {
                let zeta = ZETAS[k];
                k += 1;
                for j in start..start + len {
                    let t = mont_mul(zeta, self.coeffs[j + len]);
                    self.coeffs[j + len] = self.coeffs[j] - t;
                    self.coeffs[j] = self.coeffs[j] + t;
                }
                start += 2 * len;
            }
            len >>= 1;
        }
    }

    /// Inverse NTT (Gentleman-Sande butterflies, natural to bit-reversed order).
    ///
    /// Includes multiplication by n^{-1} = 3303 (since 256 * 3303 = 1 mod 3329).
    fn inv_ntt(&mut self) {
        let mut k: usize = 127;
        let mut len = 2;
        while len <= 128 {
            let mut start = 0;
            while start < N {
                let zeta = ZETAS[k];
                k = k.wrapping_sub(1);
                for j in start..start + len {
                    let t = self.coeffs[j];
                    self.coeffs[j] = barrett_reduce(t + self.coeffs[j + len]);
                    self.coeffs[j + len] = mont_mul(zeta, self.coeffs[j + len] - t);
                }
                start += 2 * len;
            }
            len <<= 1;
        }
        // Multiply by n^{-1} * R mod q = 3303 * 2^16 mod 3329 = montgomery form of 3303.
        // The reference implementation uses f = 1441 = mont(128^{-1}) for the last layer
        // combined. But we do all layers, so we use 1441 which equals
        // Normalization constant: f = R^2 / 128 mod q = 1441.
        //
        // The 7-layer butterfly structure needs a 1/128 normalization, and
        // pointwise multiplication introduces an extra R^{-1} factor (from
        // Montgomery multiplication). The inv_ntt compensates by outputting
        // in Montgomery form (values * R).
        //
        // mont_mul(f, x) = f * x * R^{-1} mod q.
        // With f = R^2/128 mod q: result = x * R / 128.
        // This is (x/128) in Montgomery form, which is correct when x comes
        // from pointwise multiplication that already divided by R.
        let f: i16 = 1441;
        for c in self.coeffs.iter_mut() {
            *c = mont_mul(f, *c);
        }
    }

    /// Pointwise multiplication of two polynomials in NTT domain.
    ///
    /// Multiplies 128 pairs of degree-1 polynomials mod (X^2 - zeta).
    fn pointwise_mul(&self, other: &Poly) -> Poly {
        let mut result = Poly::zero();
        for i in 0..64 {
            let idx = 4 * i;
            let zeta = ZETAS[64 + i];

            // First pair: mod (X^2 - zeta)
            basemul(
                &mut result.coeffs[idx..idx + 2],
                &self.coeffs[idx..idx + 2],
                &other.coeffs[idx..idx + 2],
                zeta,
            );

            // Second pair: mod (X^2 + zeta)
            basemul(
                &mut result.coeffs[idx + 2..idx + 4],
                &self.coeffs[idx + 2..idx + 4],
                &other.coeffs[idx + 2..idx + 4],
                -zeta,
            );
        }
        result
    }

    /// Add two polynomials coefficient-wise.
    fn add(&self, other: &Poly) -> Poly {
        let mut result = Poly::zero();
        for i in 0..N {
            result.coeffs[i] = self.coeffs[i] + other.coeffs[i];
        }
        result
    }

    /// Subtract two polynomials coefficient-wise.
    fn sub(&self, other: &Poly) -> Poly {
        let mut result = Poly::zero();
        for i in 0..N {
            result.coeffs[i] = self.coeffs[i] - other.coeffs[i];
        }
        result
    }

    /// Reduce all coefficients via Barrett reduction.
    fn reduce(&mut self) {
        for c in self.coeffs.iter_mut() {
            *c = barrett_reduce(*c);
        }
    }

    /// Convert all coefficients from Montgomery form to normal form.
    ///
    /// Multiplies each coefficient by R^{-1} mod q via Montgomery reduction.
    /// Used in NTT roundtrip tests where inv_ntt output needs de-Montgomerifying.
    /// In the normal encrypt/decrypt flow, pointwise_mul's R^{-1} cancels
    /// inv_ntt's R factor, so this method is not needed.
    #[cfg(test)]
    fn from_montgomery(&mut self) {
        for c in self.coeffs.iter_mut() {
            *c = montgomery_reduce(*c as i32);
        }
    }

    /// Convert all coefficients from normal form to Montgomery form.
    ///
    /// Multiplies each coefficient by R mod q. This is done by computing
    /// montgomery_reduce(coeff * R^2 mod q), since montgomery_reduce
    /// divides by R, giving coeff * R^2 / R = coeff * R.
    fn to_montgomery(&mut self) {
        const R2_MOD_Q: i32 = 1353; // R^2 mod q = (2^16)^2 mod 3329
        for c in self.coeffs.iter_mut() {
            *c = montgomery_reduce(*c as i32 * R2_MOD_Q);
        }
    }
}

/// Base multiplication of two degree-1 polynomials in NTT domain.
///
/// Computes (a0 + a1*X) * (b0 + b1*X) mod (X^2 - zeta).
fn basemul(result: &mut [i16], a: &[i16], b: &[i16], zeta: i16) {
    result[0] = mont_mul(a[1], b[1]);
    result[0] = mont_mul(result[0], zeta);
    result[0] += mont_mul(a[0], b[0]);
    result[1] = mont_mul(a[0], b[1]);
    result[1] += mont_mul(a[1], b[0]);
}

// ---------------------------------------------------------------------------
// Encoding / Decoding (FIPS 203, Section 4.2)
// ---------------------------------------------------------------------------

/// Encode a polynomial with d-bit coefficients into bytes (ByteEncode_d).
///
/// Coefficients are first reduced to [0, q) (or [0, 2^d) for compressed),
/// then packed into a byte stream.
fn byte_encode(poly: &Poly, d: u32, output: &mut [u8]) {
    let mask = (1u32 << d) - 1;
    let mut bit_pos: usize = 0;
    for b in output.iter_mut() {
        *b = 0;
    }
    for i in 0..N {
        let val = (to_positive(poly.coeffs[i]) as u32) & mask;
        for j in 0..d {
            if (val >> j) & 1 == 1 {
                let byte_idx = bit_pos / 8;
                let bit_idx = bit_pos % 8;
                output[byte_idx] |= 1u8 << bit_idx;
            }
            bit_pos += 1;
        }
    }
}

/// Decode bytes into a polynomial with d-bit coefficients (ByteDecode_d).
fn byte_decode(input: &[u8], d: u32) -> Poly {
    let mut poly = Poly::zero();
    let mask = (1u32 << d) - 1;
    let mut bit_pos: usize = 0;
    for i in 0..N {
        let mut val: u32 = 0;
        for j in 0..d {
            let byte_idx = bit_pos / 8;
            let bit_idx = bit_pos % 8;
            if (input[byte_idx] >> bit_idx) & 1 == 1 {
                val |= 1 << j;
            }
            bit_pos += 1;
        }
        poly.coeffs[i] = (val & mask) as i16;
    }
    poly
}

/// Encode a polynomial with 12-bit coefficients (for public key encoding).
fn byte_encode_12(poly: &Poly, output: &mut [u8]) {
    byte_encode(poly, 12, output);
}

/// Decode a polynomial with 12-bit coefficients.
fn byte_decode_12(input: &[u8]) -> Poly {
    byte_decode(input, 12)
}

// ---------------------------------------------------------------------------
// Compression / Decompression (FIPS 203, Section 4.2.1)
// ---------------------------------------------------------------------------

/// Compress a coefficient: round(2^d / q * x) mod 2^d.
#[inline]
fn compress_coeff(x: i16, d: u32) -> u16 {
    let x = to_positive(x) as u32;
    let shifted = (x << d) + (Q as u32 / 2);
    let result = shifted / (Q as u32);
    (result & ((1u32 << d) - 1)) as u16
}

/// Decompress a coefficient: round(q / 2^d * y).
#[inline]
fn decompress_coeff(y: u16, d: u32) -> i16 {
    let val = (Q as u32) * (y as u32) + (1u32 << (d - 1));
    (val >> d) as i16
}

/// Compress a polynomial: apply compress_coeff to each coefficient.
fn compress_poly(poly: &Poly, d: u32) -> Poly {
    let mut result = Poly::zero();
    for i in 0..N {
        result.coeffs[i] = compress_coeff(poly.coeffs[i], d) as i16;
    }
    result
}

/// Decompress a polynomial.
fn decompress_poly(poly: &Poly, d: u32) -> Poly {
    let mut result = Poly::zero();
    for i in 0..N {
        result.coeffs[i] = decompress_coeff(poly.coeffs[i] as u16, d);
    }
    result
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// Sample a polynomial from a uniform distribution using SHAKE-128 (XOF).
///
/// This implements the rejection sampling from FIPS 203 (SampleNTT).
/// Absorbs seed || i || j and rejection-samples coefficients in [0, q).
fn sample_ntt(seed: &[u8; 32], i: u8, j: u8) -> Poly {
    let mut xof = Shake128::new();
    xof.absorb(seed);
    xof.absorb(&[i, j]);

    let mut poly = Poly::zero();
    let mut idx = 0;
    let mut buf = [0u8; 3];

    while idx < N {
        xof.squeeze(&mut buf);
        let d1 = (buf[0] as u16) | ((buf[1] as u16 & 0x0F) << 8);
        let d2 = ((buf[1] as u16) >> 4) | ((buf[2] as u16) << 4);

        if d1 < Q as u16 {
            poly.coeffs[idx] = d1 as i16;
            idx += 1;
        }
        if idx < N && d2 < Q as u16 {
            poly.coeffs[idx] = d2 as i16;
            idx += 1;
        }
    }
    poly
}

/// Sample a polynomial from the Centered Binomial Distribution (CBD) with eta.
///
/// FIPS 203, Algorithm 7 (SamplePolyCBD). Consumes 64*eta bytes of randomness.
fn sample_cbd(bytes: &[u8], eta: usize) -> Poly {
    let mut poly = Poly::zero();

    match eta {
        2 => {
            // CBD with eta=2: each coefficient uses 4 bits (2+2)
            for i in 0..N {
                let byte_idx = i / 2;
                let nibble = if i % 2 == 0 {
                    bytes[byte_idx] & 0x0F
                } else {
                    bytes[byte_idx] >> 4
                };
                let a = ((nibble & 1) + ((nibble >> 1) & 1)) as i16;
                let b = (((nibble >> 2) & 1) + ((nibble >> 3) & 1)) as i16;
                poly.coeffs[i] = a - b;
            }
        }
        3 => {
            // CBD with eta=3: each coefficient uses 6 bits (3+3)
            let mut bit_pos = 0;
            for i in 0..N {
                let mut a: i16 = 0;
                for _j in 0..eta {
                    let byte_idx = bit_pos / 8;
                    let bit_idx = bit_pos % 8;
                    a += ((bytes[byte_idx] >> bit_idx) & 1) as i16;
                    bit_pos += 1;
                }
                let mut b: i16 = 0;
                for _j in 0..eta {
                    let byte_idx = bit_pos / 8;
                    let bit_idx = bit_pos % 8;
                    b += ((bytes[byte_idx] >> bit_idx) & 1) as i16;
                    bit_pos += 1;
                }
                poly.coeffs[i] = a - b;
            }
        }
        _ => panic!("unsupported eta value"),
    }

    poly
}

/// Generate CBD noise bytes using SHAKE-256 (PRF).
///
/// FIPS 203: PRF_eta(s, b) = SHAKE-256(s || b, 64*eta).
fn prf(seed: &[u8; 32], nonce: u8, eta: usize) -> Vec<u8> {
    let out_len = 64 * eta;
    let mut output = vec![0u8; out_len];
    let mut xof = Shake256::new();
    xof.absorb(seed);
    xof.absorb(&[nonce]);
    xof.squeeze(&mut output);
    output
}

// ---------------------------------------------------------------------------
// Key types
// ---------------------------------------------------------------------------

/// ML-KEM 768 public key (1184 bytes).
///
/// Contains the encoded NTT-domain vector t_hat (3 * 384 = 1152 bytes)
/// followed by the 32-byte seed rho for regenerating matrix A.
pub struct MlKemPublicKey {
    /// Raw encoded public key bytes.
    pub bytes: [u8; 1184],
}

/// ML-KEM 768 secret key (2400 bytes).
///
/// Contains:
/// - Encoded secret vector s_hat in NTT domain (3 * 384 = 1152 bytes)
/// - Encoded public key (1184 bytes)
/// - Hash of public key H(pk) (32 bytes)
/// - Implicit rejection seed z (32 bytes)
pub struct MlKemSecretKey {
    /// Raw encoded secret key bytes.
    pub bytes: [u8; 2400],
}

/// ML-KEM 768 ciphertext (1088 bytes).
///
/// Contains:
/// - Compressed vector u (3 * 320 = 960 bytes, du=10)
/// - Compressed polynomial v (128 bytes, dv=4)
pub struct MlKemCiphertext {
    /// Raw encoded ciphertext bytes.
    pub bytes: [u8; 1088],
}

// ---------------------------------------------------------------------------
// K-PKE (internal CPA-secure PKE)
// ---------------------------------------------------------------------------

/// K-PKE key generation (FIPS 203, Algorithm 12).
///
/// Returns (encryption key ek, decryption key dk).
fn k_pke_keygen(d: &[u8; 32]) -> (Vec<u8>, Vec<u8>) {
    // (rho, sigma) = G(d || k)
    let mut g_input = [0u8; 33];
    g_input[..32].copy_from_slice(d);
    g_input[32] = K as u8;
    let g_output = sha3_512(&g_input);
    let rho: [u8; 32] = g_output[..32].try_into().unwrap();
    let sigma: [u8; 32] = g_output[32..64].try_into().unwrap();

    // Generate matrix A_hat (in NTT domain) from rho
    let mut a_hat: [[Poly; 3]; 3] =
        core::array::from_fn(|_| core::array::from_fn(|_| Poly::zero()));
    for i in 0..K {
        for j in 0..K {
            a_hat[i][j] = sample_ntt(&rho, i as u8, j as u8);
        }
    }

    // Generate secret vector s
    let mut s: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    let mut nonce: u8 = 0;
    for i in 0..K {
        let noise_bytes = prf(&sigma, nonce, ETA1);
        s[i] = sample_cbd(&noise_bytes, ETA1);
        nonce += 1;
    }

    // Generate error vector e
    let mut e: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        let noise_bytes = prf(&sigma, nonce, ETA1);
        e[i] = sample_cbd(&noise_bytes, ETA1);
        nonce += 1;
    }

    // NTT(s), NTT(e)
    let mut s_hat: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    let mut e_hat: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        s_hat[i] = s[i].clone();
        s_hat[i].ntt();
        s_hat[i].reduce();
        e_hat[i] = e[i].clone();
        e_hat[i].ntt();
        e_hat[i].reduce();
    }

    // t_hat = A_hat * s_hat + e_hat
    // Following the reference: accumulate pointwise products, then apply
    // to_montgomery to cancel the R^{-1} from basemul, then add e_hat.
    let mut t_hat: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        for j in 0..K {
            let product = a_hat[i][j].pointwise_mul(&s_hat[j]);
            t_hat[i] = t_hat[i].add(&product);
        }
        t_hat[i].to_montgomery();
        t_hat[i] = t_hat[i].add(&e_hat[i]);
        t_hat[i].reduce();
    }

    // Encode encryption key: ek = ByteEncode_12(t_hat) || rho
    let mut ek = vec![0u8; K * 384 + 32];
    for i in 0..K {
        byte_encode_12(&t_hat[i], &mut ek[i * 384..(i + 1) * 384]);
    }
    ek[K * 384..].copy_from_slice(&rho);

    // Encode decryption key: dk = ByteEncode_12(s_hat)
    let mut dk = vec![0u8; K * 384];
    for i in 0..K {
        byte_encode_12(&s_hat[i], &mut dk[i * 384..(i + 1) * 384]);
    }

    (ek, dk)
}

/// K-PKE encryption (FIPS 203, Algorithm 13).
fn k_pke_encrypt(ek: &[u8], msg: &[u8; 32], randomness: &[u8; 32]) -> Vec<u8> {
    // Decode encryption key
    let mut t_hat: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        t_hat[i] = byte_decode_12(&ek[i * 384..(i + 1) * 384]);
    }
    let rho: [u8; 32] = ek[K * 384..K * 384 + 32].try_into().unwrap();

    // Regenerate matrix A_hat^T (transposed for encryption)
    let mut a_hat_t: [[Poly; 3]; 3] =
        core::array::from_fn(|_| core::array::from_fn(|_| Poly::zero()));
    for i in 0..K {
        for j in 0..K {
            a_hat_t[i][j] = sample_ntt(&rho, j as u8, i as u8);
        }
    }

    // Sample r, e1, e2
    let mut r: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    let mut nonce: u8 = 0;
    for i in 0..K {
        let noise_bytes = prf(randomness, nonce, ETA1);
        r[i] = sample_cbd(&noise_bytes, ETA1);
        nonce += 1;
    }

    let mut e1: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        let noise_bytes = prf(randomness, nonce, ETA2);
        e1[i] = sample_cbd(&noise_bytes, ETA2);
        nonce += 1;
    }

    let noise_bytes = prf(randomness, nonce, ETA2);
    let e2 = sample_cbd(&noise_bytes, ETA2);

    // NTT(r)
    let mut r_hat: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        r_hat[i] = r[i].clone();
        r_hat[i].ntt();
        r_hat[i].reduce();
    }

    // u = NTT^{-1}(A_hat^T * r_hat) + e1
    let mut u: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        let mut acc = Poly::zero();
        for j in 0..K {
            let product = a_hat_t[i][j].pointwise_mul(&r_hat[j]);
            acc = acc.add(&product);
        }
        acc.inv_ntt();
        // Note: no from_montgomery needed here because pointwise_mul's
        // R^{-1} factor cancels with inv_ntt's R factor (f=1441).
        acc.reduce();
        u[i] = acc.add(&e1[i]);
        u[i].reduce();
    }

    // v = NTT^{-1}(t_hat^T * r_hat) + e2 + Decompress_1(Decode_1(msg))
    let mut v_acc = Poly::zero();
    for j in 0..K {
        let product = t_hat[j].pointwise_mul(&r_hat[j]);
        v_acc = v_acc.add(&product);
    }
    v_acc.inv_ntt();
    v_acc.reduce();
    v_acc = v_acc.add(&e2);

    // Decode message as polynomial
    let msg_poly = decode_message(msg);
    let mut v = v_acc.add(&msg_poly);
    v.reduce();

    // Encode ciphertext: c1 = Compress_du(u), c2 = Compress_dv(v)
    let c1_len = K * (N * DU as usize / 8); // 960
    let c2_len = N * DV as usize / 8; // 128
    let mut ct = vec![0u8; c1_len + c2_len];

    for i in 0..K {
        let compressed = compress_poly(&u[i], DU);
        byte_encode(&compressed, DU, &mut ct[i * 320..(i + 1) * 320]);
    }

    let compressed_v = compress_poly(&v, DV);
    byte_encode(&compressed_v, DV, &mut ct[c1_len..]);

    ct
}

/// K-PKE decryption (FIPS 203, Algorithm 14).
fn k_pke_decrypt(dk: &[u8], ct: &[u8]) -> [u8; 32] {
    // Decode secret key
    let mut s_hat: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        s_hat[i] = byte_decode_12(&dk[i * 384..(i + 1) * 384]);
    }

    // Decode ciphertext
    let mut u: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        let compressed = byte_decode(&ct[i * 320..(i + 1) * 320], DU);
        u[i] = decompress_poly(&compressed, DU);
    }

    let c2_start = K * 320;
    let compressed_v = byte_decode(&ct[c2_start..], DV);
    let v = decompress_poly(&compressed_v, DV);

    // NTT(u)
    let mut u_hat: [Poly; 3] = core::array::from_fn(|_| Poly::zero());
    for i in 0..K {
        u_hat[i] = u[i].clone();
        u_hat[i].ntt();
    }

    // w = v - NTT^{-1}(s_hat^T * NTT(u))
    let mut inner = Poly::zero();
    for j in 0..K {
        let product = s_hat[j].pointwise_mul(&u_hat[j]);
        inner = inner.add(&product);
    }
    inner.inv_ntt();
    // No from_montgomery: pointwise_mul's R^{-1} cancels inv_ntt's R.
    inner.reduce();
    let mut w = v.sub(&inner);
    w.reduce();

    // Encode message
    encode_message(&w)
}

/// Decode a 32-byte message into a polynomial.
///
/// Each bit i of the message maps to coefficient i:
/// 0 -> 0, 1 -> round(q/2) = 1665.
fn decode_message(msg: &[u8; 32]) -> Poly {
    let mut poly = Poly::zero();
    for i in 0..N {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        let bit = ((msg[byte_idx] >> bit_idx) & 1) as i16;
        // round(q/2) = (3329 + 1) / 2 = 1665
        poly.coeffs[i] = bit * 1665;
    }
    poly
}

/// Encode a polynomial back to a 32-byte message.
///
/// Each coefficient is compressed to 1 bit: round(2x/q) mod 2.
fn encode_message(poly: &Poly) -> [u8; 32] {
    let mut msg = [0u8; 32];
    for i in 0..N {
        let val = to_positive(poly.coeffs[i]) as u32;
        // Compress to 1 bit: round(2 * val / q) mod 2
        let bit = ((val * 2 + Q as u32 / 2) / Q as u32) & 1;
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        msg[byte_idx] |= (bit as u8) << bit_idx;
    }
    msg
}

// ---------------------------------------------------------------------------
// ML-KEM 768 public API
// ---------------------------------------------------------------------------

/// Generate an ML-KEM 768 key pair (FIPS 203, Algorithm 15).
///
/// # Arguments
///
/// * `rng` - 64 bytes of random data: first 32 bytes (d) for K-PKE keygen,
///   last 32 bytes (z) for implicit rejection seed.
///
/// # Returns
///
/// A tuple `(public_key, secret_key)`.
pub fn keygen(rng: &[u8; 64]) -> (MlKemPublicKey, MlKemSecretKey) {
    let d: [u8; 32] = rng[..32].try_into().unwrap();
    let z: [u8; 32] = rng[32..64].try_into().unwrap();

    let (ek, dk) = k_pke_keygen(&d);

    // Public key = ek
    let mut pk = MlKemPublicKey {
        bytes: [0u8; 1184],
    };
    pk.bytes.copy_from_slice(&ek);

    // Secret key = dk || ek || H(ek) || z
    let h_ek = sha3_256(&ek);
    let mut sk = MlKemSecretKey {
        bytes: [0u8; 2400],
    };
    sk.bytes[..1152].copy_from_slice(&dk);
    sk.bytes[1152..2336].copy_from_slice(&ek);
    sk.bytes[2336..2368].copy_from_slice(&h_ek);
    sk.bytes[2368..2400].copy_from_slice(&z);

    (pk, sk)
}

/// Encapsulate a shared secret using an ML-KEM 768 public key (FIPS 203, Algorithm 16).
///
/// # Arguments
///
/// * `pk` - The recipient's public key.
/// * `rng` - 32 bytes of random data (m).
///
/// # Returns
///
/// A tuple `(ciphertext, shared_secret)` where `shared_secret` is 32 bytes.
pub fn encapsulate(pk: &MlKemPublicKey, rng: &[u8; 32]) -> (MlKemCiphertext, [u8; 32]) {
    let m = *rng;

    // (K, r) = G(m || H(ek))
    let h_ek = sha3_256(&pk.bytes);
    let mut g_input = [0u8; 64];
    g_input[..32].copy_from_slice(&m);
    g_input[32..64].copy_from_slice(&h_ek);
    let g_output = sha3_512(&g_input);
    let shared_secret: [u8; 32] = g_output[..32].try_into().unwrap();
    let r: [u8; 32] = g_output[32..64].try_into().unwrap();

    // c = K-PKE.Encrypt(ek, m, r)
    let ct_bytes = k_pke_encrypt(&pk.bytes, &m, &r);

    let mut ct = MlKemCiphertext {
        bytes: [0u8; 1088],
    };
    ct.bytes.copy_from_slice(&ct_bytes);

    (ct, shared_secret)
}

/// Decapsulate a shared secret from an ML-KEM 768 ciphertext (FIPS 203, Algorithm 17).
///
/// Uses implicit rejection: if the ciphertext is invalid, returns a
/// pseudorandom value derived from the secret key and ciphertext rather
/// than an error, preventing chosen-ciphertext attacks.
///
/// # Arguments
///
/// * `sk` - The recipient's secret key.
/// * `ct` - The ciphertext to decapsulate.
///
/// # Returns
///
/// The 32-byte shared secret.
pub fn decapsulate(sk: &MlKemSecretKey, ct: &MlKemCiphertext) -> [u8; 32] {
    // Parse secret key components
    let dk_pke = &sk.bytes[..1152];
    let ek = &sk.bytes[1152..2336];
    let h_ek: [u8; 32] = sk.bytes[2336..2368].try_into().unwrap();
    let z: [u8; 32] = sk.bytes[2368..2400].try_into().unwrap();

    // m' = K-PKE.Decrypt(dk, c)
    let m_prime = k_pke_decrypt(dk_pke, &ct.bytes);

    // (K', r') = G(m' || h)
    let mut g_input = [0u8; 64];
    g_input[..32].copy_from_slice(&m_prime);
    g_input[32..64].copy_from_slice(&h_ek);
    let g_output = sha3_512(&g_input);
    let k_prime: [u8; 32] = g_output[..32].try_into().unwrap();
    let r_prime: [u8; 32] = g_output[32..64].try_into().unwrap();

    // K_bar = J(z || c) (implicit rejection value)
    let mut j_xof = Shake256::new();
    j_xof.absorb(&z);
    j_xof.absorb(&ct.bytes);
    let mut k_bar = [0u8; 32];
    j_xof.squeeze(&mut k_bar);

    // c' = K-PKE.Encrypt(ek, m', r')
    let ct_prime = k_pke_encrypt(ek, &m_prime, &r_prime);

    // Constant-time comparison: if c == c', return K'; else return K_bar
    let mut diff: u8 = 0;
    for i in 0..1088 {
        diff |= ct.bytes[i] ^ ct_prime[i];
    }
    // diff == 0 iff ct == ct_prime
    // Constant-time select: mask is 0xFF if equal, 0x00 if not
    let mask = (((diff as u16).wrapping_sub(1)) >> 8) as u8;
    let neg_mask = !mask;

    let mut shared_secret = [0u8; 32];
    for i in 0..32 {
        shared_secret[i] = (k_prime[i] & mask) | (k_bar[i] & neg_mask);
    }

    shared_secret
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_montgomery_reduce_basic() {
        // montgomery_reduce(a) should give a * R^{-1} mod q
        // For a = 0, result should be 0
        assert_eq!(montgomery_reduce(0), 0);

        // For a = Q (= 3329), result should be Q * R^{-1} mod Q = 0
        let r = montgomery_reduce(Q as i32);
        assert_eq!(to_positive(r), 0);
    }

    #[test]
    fn test_montgomery_roundtrip() {
        // to_mont(a) then from_mont should give back a
        for &a in &[0i16, 1, 2, 100, 1000, 3000, 3328] {
            let mont_a = montgomery_reduce(a as i32 * 1353); // 1353 = R^2 mod q
            let back = to_positive(montgomery_reduce(mont_a as i32));
            assert_eq!(
                back, a as u16,
                "roundtrip failed for {}: mont={}, back={}",
                a, mont_a, back
            );
        }
    }

    #[test]
    fn test_barrett_reduce() {
        for &a in &[0i16, 1, 3328, -1, -3328, 10000, -10000] {
            let r = barrett_reduce(a);
            // Result should be congruent to a mod Q
            let pos_a = ((a as i32 % Q as i32) + Q as i32) as u16 % Q as u16;
            let pos_r = to_positive(r);
            assert_eq!(
                pos_r, pos_a,
                "barrett_reduce({}) = {}, expected {} mod q",
                a, pos_r, pos_a
            );
        }
    }

    #[test]
    fn test_ntt_inverse_ntt_roundtrip() {
        // NTT followed by inverse NTT should give back the original polynomial
        let mut poly = Poly::zero();
        for i in 0..N {
            poly.coeffs[i] = (i as i16) % Q;
        }
        let original = poly.clone();

        poly.ntt();
        poly.inv_ntt();
        poly.from_montgomery();

        for i in 0..N {
            let a = to_positive(poly.coeffs[i]);
            let b = to_positive(original.coeffs[i]);
            assert_eq!(
                a, b,
                "NTT roundtrip failed at index {}: got {}, expected {}",
                i, a, b
            );
        }
    }

    #[test]
    fn test_pointwise_mul_ntt() {
        // Test that pointwise multiplication in NTT domain corresponds to
        // polynomial multiplication in normal domain (simple case)
        let mut a = Poly::zero();
        a.coeffs[0] = 1; // a(x) = 1

        let mut b = Poly::zero();
        b.coeffs[0] = 2; // b(x) = 2

        a.ntt();
        b.ntt();
        let mut c = a.pointwise_mul(&b);
        c.inv_ntt();
        // No from_montgomery: pointwise_mul's R^{-1} cancels inv_ntt's R.

        // Result should be the constant polynomial 2
        assert_eq!(to_positive(c.coeffs[0]), 2);
        for i in 1..N {
            assert_eq!(to_positive(c.coeffs[i]), 0, "nonzero at index {}", i);
        }
    }

    #[test]
    fn test_encode_decode_12_roundtrip() {
        let mut poly = Poly::zero();
        for i in 0..N {
            poly.coeffs[i] = ((i * 13 + 7) as i16) % Q;
        }

        let mut encoded = [0u8; 384];
        byte_encode_12(&poly, &mut encoded);
        let decoded = byte_decode_12(&encoded);

        for i in 0..N {
            assert_eq!(
                to_positive(poly.coeffs[i]),
                to_positive(decoded.coeffs[i]),
                "encode/decode roundtrip failed at {}",
                i
            );
        }
    }

    #[test]
    fn test_compress_decompress_roundtrip() {
        // Compression is lossy, but decompress(compress(x)) should be close to x
        for d in [4u32, 10] {
            for &x in &[0i16, 1, 100, 1664, 1665, 3000, 3328] {
                let c = compress_coeff(x, d);
                let dc = to_positive(decompress_coeff(c, d));
                let xp = to_positive(x);
                let diff = if xp > dc {
                    core::cmp::min(xp - dc, Q as u16 - xp + dc)
                } else {
                    core::cmp::min(dc - xp, Q as u16 - dc + xp)
                };
                let max_error = (Q as u32 + (1 << d)) / (1 << (d + 1));
                assert!(
                    diff <= max_error as u16,
                    "compress/decompress error too large for x={}, d={}: diff={}, max={}",
                    x, d, diff, max_error
                );
            }
        }
    }

    #[test]
    fn test_message_encode_decode_roundtrip() {
        let msg: [u8; 32] = [
            0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC,
            0xBA, 0x98, 0x76, 0x54, 0x32, 0x10, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x11,
            0x22, 0x33, 0x44, 0x55,
        ];
        let poly = decode_message(&msg);
        let recovered = encode_message(&poly);
        assert_eq!(msg, recovered, "message encode/decode roundtrip failed");
    }

    #[test]
    fn test_keygen_sizes() {
        let mut rng = [0u8; 64];
        for i in 0..64 {
            rng[i] = i as u8;
        }
        let (pk, sk) = keygen(&rng);
        assert_eq!(pk.bytes.len(), 1184, "public key should be 1184 bytes");
        assert_eq!(sk.bytes.len(), 2400, "secret key should be 2400 bytes");
    }

    #[test]
    fn test_encapsulate_decapsulate_roundtrip() {
        let mut keygen_rng = [0u8; 64];
        for i in 0..64 {
            keygen_rng[i] = (i as u8).wrapping_mul(7).wrapping_add(13);
        }
        let (pk, sk) = keygen(&keygen_rng);

        let mut encap_rng = [0u8; 32];
        for i in 0..32 {
            encap_rng[i] = (i as u8).wrapping_mul(11).wrapping_add(3);
        }
        let (ct, shared_secret_enc) = encapsulate(&pk, &encap_rng);

        assert_eq!(ct.bytes.len(), 1088, "ciphertext should be 1088 bytes");
        assert_eq!(
            shared_secret_enc.len(),
            32,
            "shared secret should be 32 bytes"
        );

        let shared_secret_dec = decapsulate(&sk, &ct);

        assert_eq!(
            shared_secret_enc, shared_secret_dec,
            "encapsulate and decapsulate must produce the same shared secret"
        );
    }

    #[test]
    fn test_decapsulate_wrong_ciphertext_gives_implicit_rejection() {
        let mut keygen_rng = [0u8; 64];
        for i in 0..64 {
            keygen_rng[i] = (i as u8).wrapping_mul(3).wrapping_add(42);
        }
        let (pk, sk) = keygen(&keygen_rng);

        let mut encap_rng = [0u8; 32];
        for i in 0..32 {
            encap_rng[i] = (i as u8).wrapping_mul(5).wrapping_add(17);
        }
        let (mut ct, shared_secret_enc) = encapsulate(&pk, &encap_rng);

        // Corrupt the ciphertext
        ct.bytes[0] ^= 0xFF;
        ct.bytes[100] ^= 0xFF;

        let shared_secret_dec = decapsulate(&sk, &ct);

        assert_ne!(
            shared_secret_enc, shared_secret_dec,
            "corrupted ciphertext should trigger implicit rejection"
        );

        // Should be deterministic
        let shared_secret_dec2 = decapsulate(&sk, &ct);
        assert_eq!(
            shared_secret_dec, shared_secret_dec2,
            "implicit rejection should be deterministic"
        );
    }

    #[test]
    fn test_different_keys_different_secrets() {
        let mut rng1 = [0u8; 64];
        let mut rng2 = [0u8; 64];
        for i in 0..64 {
            rng1[i] = i as u8;
            rng2[i] = (i as u8).wrapping_add(100);
        }
        let (pk1, sk1) = keygen(&rng1);
        let (pk2, _sk2) = keygen(&rng2);

        let mut encap_rng = [0u8; 32];
        for i in 0..32 {
            encap_rng[i] = (i as u8).wrapping_mul(7);
        }

        let (ct1, ss1) = encapsulate(&pk1, &encap_rng);
        let (_, ss2) = encapsulate(&pk2, &encap_rng);

        assert_ne!(ss1, ss2, "different keys should produce different shared secrets");

        let ss1_dec = decapsulate(&sk1, &ct1);
        assert_eq!(ss1, ss1_dec);
    }

    #[test]
    fn test_cbd_distribution() {
        let seed = [0x42u8; 32];
        let noise = prf(&seed, 0, ETA1);
        let poly = sample_cbd(&noise, ETA1);

        for i in 0..N {
            let c = poly.coeffs[i];
            // Coefficient should be in {-2, -1, 0, 1, 2} for eta=2
            assert!(
                (-2..=2).contains(&c),
                "CBD coefficient {} at index {} is out of range for eta=2",
                c,
                i
            );
        }
    }

    #[test]
    fn test_sample_ntt_range() {
        let seed = [0xABu8; 32];
        let poly = sample_ntt(&seed, 0, 0);
        for i in 0..N {
            assert!(
                poly.coeffs[i] >= 0 && poly.coeffs[i] < Q,
                "sample_ntt coefficient {} at index {} out of range",
                poly.coeffs[i],
                i
            );
        }
    }

    #[test]
    fn test_multiple_roundtrips() {
        let mut keygen_rng = [0u8; 64];
        for i in 0..64 {
            keygen_rng[i] = (i as u8).wrapping_mul(23).wrapping_add(1);
        }
        let (pk, sk) = keygen(&keygen_rng);

        for trial in 0u8..5 {
            let mut encap_rng = [0u8; 32];
            for i in 0..32 {
                encap_rng[i] = (i as u8)
                    .wrapping_mul(trial.wrapping_add(1))
                    .wrapping_add(trial);
            }
            let (ct, ss_enc) = encapsulate(&pk, &encap_rng);
            let ss_dec = decapsulate(&sk, &ct);
            assert_eq!(ss_enc, ss_dec, "roundtrip failed on trial {}", trial);
        }
    }
}
