//! G4's probe input domains, from gpu-abi.md's PROBE table.

pub const COUNT: u32 = 25;
pub const STEPS: u64 = 65_536;

/// Check the selector, input domain, and unused operand words before device submission.
pub fn valid(routine: u32, word: u32, a: &[u32; 16]) -> bool {
    let (inputs, outputs) = match routine {
        0..=7 | 9 | 12..=14 | 17 => (2, if routine == 9 { 2 } else { 1 }),
        8 | 15 | 16 => (1, 1),
        10 => (3, 3),
        11 => (6, 6),
        18 | 19 => (0, 1),
        20 | 21 => (5, 1),
        22 => (7, 1),
        23 => (8, 1),
        24 => (6, 1),
        _ => return false,
    };
    if word >= outputs || a[inputs..].iter().any(|&v| v != 0) {
        return false;
    }
    match routine {
        3..=5 => a[1] < 32,
        8 => a[0] <= 65_025,
        11 => (a[3] | a[4] | a[5]) != 0,
        15 => (a[0] & 0x7fffffff) < 0x4f000000 || a[0] == 0xcf000000,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selectors_and_domains_are_bounded() {
        for r in 0..COUNT {
            let mut a = [0; 16];
            if r == 11 {
                a[3] = 1;
            }
            assert!(valid(r, 0, &a));
            assert!(!valid(r, 6, &a));
            a[15] = 1;
            assert!(!valid(r, 0, &a));
        }
        assert!(!valid(COUNT, 0, &[0; 16]));
        assert!(!valid(11, 0, &[0; 16]));
        for shift in [32, u32::MAX] {
            let mut a = [0; 16];
            a[1] = shift;
            for r in 3..=5 {
                assert!(!valid(r, 0, &a));
            }
        }
        for bad in [0x4f000000, 0xcf000001, 0x7f800000, 0x7fc00000] {
            let mut a = [0; 16];
            a[0] = bad;
            assert!(!valid(15, 0, &a));
        }
        let mut a = [0; 16];
        a[0] = 0xcf000000;
        assert!(valid(15, 0, &a));
        a[0] = 65_026;
        assert!(!valid(8, 0, &a));
    }
}
