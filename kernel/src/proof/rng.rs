//! Seeded, counter-derived draws.
//!
//! The root seed is a `u64` — `option_env!("BREENIX_COREPROOF_SEED")` when a
//! replay pins one, otherwise the monotonic clock at harness start. It is
//! printed in a `RUN` record BEFORE the first iteration, so a run that dies
//! mid-flight still has its seed on the wire.
//!
//! ## Counter-derived, not sequential
//!
//! `draw(root_seed, component, cpu, iteration)` is a pure function of its
//! arguments: iteration *I*'s vector does not depend on iterations `0..I-1`.
//! Three things follow, and they are why the construction is worth the extra
//! mixing. A violation record can name its own complete draw vector. A replay
//! can arm iteration *I* alone instead of re-executing half a million
//! predecessors. And concurrent CPUs cannot perturb each other's streams, so
//! `Adversarial` is exactly as reproducible as `Pen`.
//!
//! ## What a seed does and does not replay
//!
//! On the profile the gates actually run — `-smp 4` multi-threaded TCG — a seed
//! replays *what the harness did*, not *what the machine did in response*: host
//! scheduling of four vCPU threads, virtio timing and the throttled disk all
//! vary. The honest answer is a measurement, not a caveat, which is why the
//! pilot's pass bar requires a `replay_hit_rate` measured over ten replays of
//! each catching seed rather than an assertion that replay works.
//!
//! The generator is the same `xorshift64*` construction `syscall/random.rs`
//! already uses, deliberately re-implemented here rather than imported: that
//! instance is syscall-owned and reseeded on its own schedule.

use super::sites::{SiteId, ALL};
use super::stimulus::Action;

const NONZERO_FALLBACK: u64 = 0xdead_beef_cafe_1234;

const fn parse_digit(byte: u8, radix: u64) -> Option<u64> {
    let digit = match byte {
        b'0'..=b'9' => (byte - b'0') as u64,
        b'a'..=b'f' => (byte - b'a' + 10) as u64,
        b'A'..=b'F' => (byte - b'A' + 10) as u64,
        _ => return None,
    };
    if digit < radix {
        Some(digit)
    } else {
        None
    }
}

const fn parse_seed(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    let (radix, mut index) =
        if bytes.len() >= 2 && bytes[0] == b'0' && (bytes[1] == b'x' || bytes[1] == b'X') {
            (16u64, 2usize)
        } else {
            (10u64, 0usize)
        };
    if index == bytes.len() {
        return None;
    }

    let mut parsed = 0u64;
    while index < bytes.len() {
        let Some(digit) = parse_digit(bytes[index], radix) else {
            return None;
        };
        let Some(scaled) = parsed.checked_mul(radix) else {
            return None;
        };
        let Some(next) = scaled.checked_add(digit) else {
            return None;
        };
        parsed = next;
        index += 1;
    }
    Some(parsed)
}

const PINNED_SEED: Option<u64> = match option_env!("BREENIX_COREPROOF_SEED") {
    Some(value) => parse_seed(value),
    None => None,
};

pub struct Xorshift64Star {
    word: u64,
}

impl Xorshift64Star {
    fn new(seed: u64) -> Self {
        Self {
            word: if seed == 0 { NONZERO_FALLBACK } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.word;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.word = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

pub const fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Which side of a protocol's commit point a seam sits on.
///
/// Derived from the site (`SiteId::order()`), never drawn — see that method for
/// why an independent draw would let a record contradict itself.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Order {
    Before,
    After,
}

impl Order {
    pub fn name(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }

    pub(crate) fn from_u8(value: u8) -> Self {
        if value == Self::After as u8 {
            Self::After
        } else {
            Self::Before
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AntagonistOp {
    Unblock,
    Placement,
}

impl AntagonistOp {
    pub(crate) fn from_u8(value: u8) -> Self {
        if value == Self::Placement as u8 {
            Self::Placement
        } else {
            Self::Unblock
        }
    }
}

#[derive(Clone, Copy)]
pub struct DrawVector {
    pub site: SiteId,
    pub action: Action,
    pub ticks: u64,
    pub cycles: u32,
    pub antagonist_op: AntagonistOp,
    pub antagonist_cpu: u8,
    pub order: Order,
}

pub fn root_seed() -> u64 {
    PINNED_SEED.unwrap_or_else(|| {
        let (seconds, nanos) = crate::time::get_monotonic_time_ns();
        let seed = seconds.rotate_left(29) ^ nanos;
        if seed == 0 {
            NONZERO_FALLBACK
        } else {
            seed
        }
    })
}

/// Produce an iteration-local vector without consulting or mutating shared state.
pub fn draw(root_seed: u64, component: u8, cpu: u8, iteration: u64) -> DrawVector {
    let domain = root_seed ^ (u64::from(component) << 56) ^ (u64::from(cpu) << 48) ^ iteration;
    let mut rng = Xorshift64Star::new(splitmix64(domain));

    let site = ALL[(rng.next_u64() % ALL.len() as u64) as usize];
    let action_roll = rng.next_u64();
    let action = if action_roll & 7 == 0 {
        Action::None
    } else {
        match rng.next_u64() % 6 {
            0 => Action::Yield,
            1 => Action::ForceResched,
            2 => Action::TimerSqueeze,
            3 => Action::SgiFrom,
            4 => Action::SpinDelay,
            _ => Action::MaskWindow,
        }
    };

    DrawVector {
        site,
        action,
        ticks: rng.next_u64(),
        cycles: (rng.next_u64() as u32).saturating_add(1),
        antagonist_op: if rng.next_u64() & 1 == 0 {
            AntagonistOp::Unblock
        } else {
            AntagonistOp::Placement
        },
        antagonist_cpu: rng.next_u64() as u8,
        // Not drawn: the site already says which side of its commit point the
        // seam is on, and a second, independent bit could only disagree with it.
        order: site.order(),
    }
}
