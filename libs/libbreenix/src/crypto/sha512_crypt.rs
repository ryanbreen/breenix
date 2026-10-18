//! SHA-512-crypt, independently written from the specification prose v0.6,
//! steps 1-22 and its SHA-512 vectors. No sample implementation was consulted.
use super::sha512::Sha512;

pub const MAX_OUTPUT: usize = 123;
const ALPHABET: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Hash a byte password with a `$6$[rounds=N$]salt` setting. The result excludes NUL.
/// Invalid settings are refused; salt bytes after the first 16 are ignored.
pub fn sha512_crypt(password: &[u8], setting: &str, out: &mut [u8; MAX_OUTPUT]) -> Result<usize, &'static str> {
    let mut tail = setting.strip_prefix("$6$").ok_or("unsupported hash")?;
    let mut rounds = 5000u32;
    let explicit = tail.starts_with("rounds=");
    if explicit {
        let (number, rest) = tail[7..].split_once('$').ok_or("missing salt")?;
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) { return Err("invalid rounds"); }
        let mut n = 0u32;
        for b in number.bytes() { n = n.saturating_mul(10).saturating_add((b - b'0') as u32); }
        rounds = n.clamp(1000, 999_999_999);
        tail = rest;
    }
    let salt = tail.split('$').next().unwrap_or("").as_bytes();
    let salt = &salt[..salt.len().min(16)];
    if !salt.iter().all(|b| ALPHABET.contains(b)) { return Err("invalid salt"); }
    let mut a = Sha512::new();
    a.update(password); a.update(salt);
    let mut b = Sha512::new();
    b.update(password); b.update(salt); b.update(password);
    let alternate = b.finalize();
    repeated(&mut a, &alternate, password.len());
    let mut bits = password.len();
    while bits != 0 {
        a.update(if bits & 1 != 0 { &alternate } else { password });
        bits >>= 1;
    }
    let mut digest = a.finalize();
    let mut dp = Sha512::new();
    for _ in 0..password.len() { dp.update(password); }
    let p = dp.finalize();
    let mut ds = Sha512::new();
    for _ in 0..16 + digest[0] as usize { ds.update(salt); }
    let s = ds.finalize();
    for round in 0..rounds {
        let mut c = Sha512::new();
        if round & 1 != 0 { repeated(&mut c, &p, password.len()); } else { c.update(&digest); }
        if round % 3 != 0 { repeated(&mut c, &s, salt.len()); }
        if round % 7 != 0 { repeated(&mut c, &p, password.len()); }
        if round & 1 != 0 { c.update(&digest); } else { repeated(&mut c, &p, password.len()); }
        digest = c.finalize();
    }
    let mut at = 0;
    let mut put = |bytes: &[u8]| { out[at..at + bytes.len()].copy_from_slice(bytes); at += bytes.len(); };
    put(b"$6$");
    if explicit {
        put(b"rounds=");
        let mut digits = [0u8; 10];
        let mut n = rounds;
        let mut start = digits.len();
        while n != 0 { start -= 1; digits[start] = b'0' + (n % 10) as u8; n /= 10; }
        put(&digits[start..]); put(b"$");
    }
    put(salt); put(b"$");
    // Step 22's table lists high, middle, low byte indices.
    const GROUPS: [[usize; 3]; 21] = [
        [0,21,42],[22,43,1],[44,2,23],[3,24,45],[25,46,4],[47,5,26],
        [6,27,48],[28,49,7],[50,8,29],[9,30,51],[31,52,10],[53,11,32],
        [12,33,54],[34,55,13],[56,14,35],[15,36,57],[37,58,16],[59,17,38],
        [18,39,60],[40,61,19],[62,20,41],
    ];
    for [high, middle, low] in GROUPS {
        let mut word = ((digest[high] as u32) << 16) | ((digest[middle] as u32) << 8) | digest[low] as u32;
        for _ in 0..4 { put(&[ALPHABET[(word & 63) as usize]]); word >>= 6; }
    }
    put(&[ALPHABET[(digest[63] & 63) as usize], ALPHABET[(digest[63] >> 6) as usize]]);
    Ok(at)
}

fn repeated(hash: &mut Sha512, digest: &[u8; 64], mut length: usize) {
    while length >= 64 { hash.update(digest); length -= 64; }
    hash.update(&digest[..length]);
}

/// Compare a computed hash without a content-dependent early exit.
pub fn verify(password: &[u8], stored: &str) -> bool {
    let mut out = [0; MAX_OUTPUT];
    let Ok(len) = sha512_crypt(password, stored, &mut out) else { return false; };
    if len != stored.len() { return false; }
    let mut difference = 0u8;
    for (a, b) in out[..len].iter().zip(stored.as_bytes()) { difference |= a ^ b; }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn specification_vectors() {
        let vectors = [
            ("$6$saltstring", "Hello world!", "$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1"),
            ("$6$rounds=10000$saltstringsaltstring", "Hello world!", "$6$rounds=10000$saltstringsaltst$OW1/O6BYHV6BcXZu8QVeXbDWra3Oeqh0sbHbbMCVNSnCM/UrjmM0Dp8vOuZeHBy/YTBmSK6H9qs/y3RnOaw5v."),
            ("$6$rounds=5000$toolongsaltstring", "This is just a test", "$6$rounds=5000$toolongsaltstrin$lQ8jolhgVRVhY4b5pZKaysCLi0QBxGoNeKQzQ3glMhwllF7oGDZxUhx1yxdYcz/e1JSbq3y6JMxxl8audkUEm0"),
            ("$6$rounds=1400$anotherlongsaltstring", "a very much longer text to encrypt.  This one even stretches over morethan one line.", "$6$rounds=1400$anotherlongsalts$POfYwTEok97VWcjxIiSOjiykti.o/pQs.wPvMxQ6Fm7I6IoYN3CmLs66x9t0oSwbtEW7o7UmJEiDwGqd8p4ur1"),
            ("$6$rounds=77777$short", "we have a short salt string but not a short password", "$6$rounds=77777$short$WuQyW2YR.hBNpjjRhpYD/ifIw05xdfeEyQoMxIXbkvr0gge1a1x3yRULJ5CCaUeOxFmtlcGZelFl5CxtgfiAc0"),
            ("$6$rounds=123456$asaltof16chars..", "a short string", "$6$rounds=123456$asaltof16chars..$BtCwjqMJGx5hrJhZywWvt0RLE8uZ4oPwcelCjmw2kSYu.Ec6ycULevoBK25fs2xXgMNrCzIMVcgEJAstJeonj1"),
            ("$6$rounds=10$roundstoolow", "the minimum number is still observed", "$6$rounds=1000$roundstoolow$kUMsbe306n21p9R.FRkW3IGn.S9NPN0x50YhH1xhLsPuWGsUSklZt58jaTfF4ZEQpyUNGc0dqbpBYYBaHHrsX."),
        ];
        for (setting, password, expected) in vectors {
            let mut output = [0; MAX_OUTPUT];
            let length = sha512_crypt(password.as_bytes(), setting, &mut output).unwrap();
            assert_eq!(&output[..length], expected.as_bytes());
        }
    }
    #[test]
    fn password_verification_refuses_wrong_locked_and_missing_hashes() {
        let mut output = [0; MAX_OUTPUT];
        let n = sha512_crypt(b"fixture password", "$6$rounds=1000$fixture", &mut output).unwrap();
        let hash = core::str::from_utf8(&output[..n]).unwrap();
        assert!(verify(b"fixture password", hash));
        assert!(!verify(b"wrong password", hash));
        assert!(!verify(b"", ""));
        assert!(!verify(b"fixture password", "!"));
        assert!(!verify(b"fixture password", "$1$fixture$unsupported"));
        assert!(sha512_crypt(b"x", "$6$rounds=no$salt", &mut output).is_err());
    }
}
