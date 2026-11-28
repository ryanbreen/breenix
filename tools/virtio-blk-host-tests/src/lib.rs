//! Host checks for the virtio-blk request queue (E4.1), compiled from the
//! kernel's own file: several chains in flight over fragmented frames, returned
//! out of order across index wrap, with exact data, descriptor credits and
//! per-head status; a stopped queue that keeps an abandoned chain's memory and
//! completes no chain a stale used entry names; and the same-range ordering.
#![cfg(test)]

#[path = "../../../kernel/src/drivers/virtio/blk_queue.rs"]
mod blk_queue;

mod device;
mod queue_checks;
