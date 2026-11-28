//! Test-only candidate counts and reservation faults shared by TCP host fixtures.

use super::*;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Index {
    Established,
    Listener,
    Bind,
}

std::thread_local! {
    pub(crate) static VISITS: Cell<[usize; 3]> = const { Cell::new([0; 3]) };
    pub(crate) static FAIL: Cell<Option<Index>> = const { Cell::new(None) };
}

pub(super) fn visit(index: Index) {
    VISITS.with(|visits| {
        let mut counts = visits.get();
        counts[index as usize] += 1;
        visits.set(counts);
    });
}

pub(super) fn reserve(index: Index) -> Result<(), i32> {
    if FAIL.get() == Some(index) {
        FAIL.set(None);
        return Err(ENOMEM);
    }
    Ok(())
}

/// Isolate the global table from tests in the harness that keep their flows.
pub(crate) struct Fixture(TcpTables);

impl Fixture {
    pub(crate) fn new() -> Self {
        Self(with_tcp(|tables| {
            core::mem::replace(tables, TcpTables::new())
        }))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        with_tcp(|tables| core::mem::swap(tables, &mut self.0));
        FAIL.set(None);
    }
}

pub(crate) fn seed_wildcard(
    port: u16,
    owner_pid: crate::process::process::ProcessId,
) -> ListenerId {
    let id = ListenerId {
        addr: [0; 4],
        port,
        generation: 0,
    };
    with_tcp(|tables| {
        tables
            .insert_listener(ListenSocket {
                id,
                backlog: 4,
                owner_pid,
                reuse: false,
                ttl: 64,
                rcv_cap: DEFAULT_RCVBUF / 2,
                owner: None,
            })
            .unwrap()
    });
    id
}

pub(crate) fn parent(id: &ConnectionId) -> Option<ListenerId> {
    with_tcp_connections(|conns| conns.get(id).and_then(|conn| conn.parent))
}

pub(crate) fn has_listener(id: ListenerId) -> bool {
    with_tcp(|tables| tables.listener(id).is_some())
}
