//! Connection setup as the kernel runs it (`tcp_ident::IsnKey` and
//! `tcp_ident::bind_ephemeral`, called by `generate_isn` and
//! `tcp_bind_ephemeral`), against a pool whose readiness and output the test
//! controls.
//!
//! - No ISN key is drawn while the pool is unready (RFC 6528 section 3 needs a
//!   secret the attacker cannot predict); the key in use after readiness is the
//!   one drawn after it.
//! - A handshake that interrupts the key's setup on the same CPU (the ARM64
//!   IRQ-exit softirq taking a SYN during connect()) is answered at once: it
//!   does not wait for the setup it interrupted.
//! - Each ephemeral-port search starts at its own draw from the pool.

use crate::tcp_ident::{bind_ephemeral, isn, IsnKey, Pool, EPHEMERAL_FIRST, EPHEMERAL_LAST, ISN_SECRET_LEN};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::sync::mpsc;
use std::time::Duration;

const LOCAL: [u8; 4] = [10, 0, 2, 15];
const REMOTE: [u8; 4] = [10, 0, 2, 2];

/// One draw: whether the pool was ready, and the bytes it gave.
#[derive(Clone)]
struct Draw {
    ready: bool,
    bytes: Vec<u8>,
}

type Hook = Box<dyn FnOnce()>;

struct TestPool {
    ready: Cell<bool>,
    state: Cell<u64>,
    draws: RefCell<Vec<Draw>>,
    /// Runs inside the first key-sized draw, as an interrupt arriving while
    /// the key is being set up.
    during_key_draw: RefCell<Option<Hook>>,
}

impl TestPool {
    fn new(ready: bool) -> Self {
        Self {
            ready: Cell::new(ready),
            state: Cell::new(0x0123_4567_89ab_cdef),
            draws: RefCell::new(Vec::new()),
            during_key_draw: RefCell::new(None),
        }
    }

    fn key_draws(&self, ready: bool) -> Vec<[u8; ISN_SECRET_LEN]> {
        self.draws
            .borrow()
            .iter()
            .filter(|d| d.ready == ready && d.bytes.len() == ISN_SECRET_LEN)
            .map(|d| d.bytes.as_slice().try_into().unwrap())
            .collect()
    }
}

impl Pool for TestPool {
    fn fill(&self, out: &mut [u8]) -> bool {
        let ready = self.ready.get();
        if out.len() == ISN_SECRET_LEN {
            let hook = self.during_key_draw.borrow_mut().take();
            if let Some(hook) = hook {
                hook();
            }
        }
        for byte in out.iter_mut() {
            // splitmix64, one byte per step: a fixed stream standing in for the pool.
            let s = self.state.get().wrapping_add(0x9e37_79b9_7f4a_7c15);
            self.state.set(s);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            *byte = (z ^ (z >> 31)) as u8;
        }
        self.draws.borrow_mut().push(Draw { ready, bytes: out.to_vec() });
        ready
    }
}

#[test]
fn no_isn_key_is_drawn_before_the_pool_is_ready() {
    let pool = TestPool::new(false);
    let key = IsnKey::new();
    let now = 1_000_000;
    let a = key.isn(&pool, LOCAL, 40000, REMOTE, 22, now);
    let b = key.isn(&pool, LOCAL, 40000, REMOTE, 22, now);
    assert!(pool.key_draws(false).is_empty(), "an ISN key was drawn from an unready pool");
    // Before readiness nothing outlives one connection: two connections on one
    // 4-tuple at one instant do not share an offset.
    assert_ne!(a, b, "two ISNs drawn before readiness share a stored offset");

    pool.ready.set(true);
    let now = 9_000_000;
    let after = key.isn(&pool, LOCAL, 40000, REMOTE, 22, now);
    let keys = pool.key_draws(true);
    assert_eq!(keys.len(), 1, "one key drawn once the pool is ready");
    assert_eq!(after, isn(&keys[0], LOCAL, 40000, REMOTE, 22, now));
    // It is kept: the same 4-tuple advances with the 4-microsecond clock.
    assert_eq!(key.isn(&pool, LOCAL, 40000, REMOTE, 22, now + 400), after.wrapping_add(100));
    assert_eq!(pool.key_draws(true).len(), 1, "the key was drawn again");
}

#[test]
fn a_handshake_interrupting_key_setup_does_not_wait_for_it() {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let pool: &'static TestPool = Box::leak(Box::new(TestPool::new(true)));
        let key: &'static IsnKey = Box::leak(Box::new(IsnKey::new()));
        let nested: &'static Cell<Option<u32>> = Box::leak(Box::new(Cell::new(None)));
        // connect() is setting the key up when a SYN for a listener arrives on
        // the same CPU and its handler asks for an ISN.
        *pool.during_key_draw.borrow_mut() = Some(Box::new(move || {
            nested.set(Some(key.isn(pool, LOCAL, 22, REMOTE, 40000, 5_000_000)));
        }));
        let outer = key.isn(pool, LOCAL, 40000, REMOTE, 22, 5_000_000);
        let keys = pool.key_draws(true);
        let later = key.isn(pool, LOCAL, 40000, REMOTE, 22, 5_000_004);
        let _ = tx.send((outer, nested.get(), keys, later, pool.key_draws(true).len()));
    });
    let (outer, nested, keys, later, key_draws) = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the interrupting handshake waited for the key setup it interrupted");
    assert!(nested.is_some(), "the interrupting handshake got no ISN");
    assert_eq!(keys.len(), 1);
    assert_eq!(outer, isn(&keys[0], LOCAL, 40000, REMOTE, 22, 5_000_000));
    assert_eq!(later, outer.wrapping_add(1), "the key set up was kept");
    assert_eq!(key_draws, 1);
}

#[test]
fn each_ephemeral_search_starts_at_its_own_draw() {
    let span = u64::from(EPHEMERAL_LAST - EPHEMERAL_FIRST) + 1;
    let start_of = |draw: &Draw| EPHEMERAL_FIRST + (u64::from_le_bytes(draw.bytes.as_slice().try_into().unwrap()) % span) as u16;
    let pool = TestPool::new(true);
    let taken = RefCell::new(BTreeSet::new());
    let mut ports = Vec::new();
    for n in 0..64 {
        let port = bind_ephemeral(&pool, |p| Ok(taken.borrow_mut().insert(p))).unwrap().expect("a free port");
        let draws = pool.draws.borrow();
        assert_eq!(draws.len(), n + 1, "search {n} did not draw once from the pool");
        assert_eq!(port, start_of(&draws[n]), "search {n} did not start at its draw");
        ports.push(port);
    }
    assert_eq!(ports.iter().collect::<BTreeSet<_>>().len(), ports.len());

    // A taken port moves the search on by one, wrapping; an error ends it.
    let mut seen = Vec::new();
    let result = bind_ephemeral(&pool, |p| {
        seen.push(p);
        if seen.len() < 3 { Ok(false) } else { Err(-98) }
    });
    assert_eq!(result, Err(-98));
    let first = start_of(&pool.draws.borrow()[64]);
    let next = |p: u16| if p == EPHEMERAL_LAST { EPHEMERAL_FIRST } else { p + 1 };
    assert_eq!(seen, vec![first, next(first), next(next(first))]);

    // Every port taken: no port, after one pass over the range.
    let mut tries = 0u64;
    assert_eq!(bind_ephemeral(&pool, |_| { tries += 1; Ok(false) }), Ok(None));
    assert_eq!(tries, span);
}
