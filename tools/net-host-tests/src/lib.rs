//! Host checks for network code, compiled from the kernel's own files: the
//! virtio-net transmit queue's publication order (virtio 1.2 section 2.7.13).
#![cfg(test)]

#[path = "../../../kernel/src/drivers/virtio/net_tx_queue.rs"]
mod net_tx_queue;

mod virtio_tx_checks;
