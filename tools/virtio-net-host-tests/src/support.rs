//! Frames and driver loops the scenarios share.
use crate::drivers::virtio::net_mmio;

/// An Ethernet frame carrying IPv4 and TCP between 10.0.2.15:`src_port` and
/// 10.0.2.2:`dst_port`, with `payload_len` bytes of payload, each byte its
/// offset modulo 251. Checksum fields are left as 0: the device does not read them.
pub fn tcp_frame(src_port: u16, dst_port: u16, payload_len: usize) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&[0x52, 0x55, 0x0a, 0x00, 0x02, 0x02]);
    frame.extend_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    frame.extend_from_slice(&0x0800u16.to_be_bytes());
    let total = (20 + 20 + payload_len) as u16;
    frame.extend_from_slice(&[0x45, 0]);
    frame.extend_from_slice(&total.to_be_bytes());
    frame.extend_from_slice(&[0, 1, 0x40, 0, 64, 6, 0, 0]);
    frame.extend_from_slice(&[10, 0, 2, 15, 10, 0, 2, 2]);
    frame.extend_from_slice(&src_port.to_be_bytes());
    frame.extend_from_slice(&dst_port.to_be_bytes());
    frame.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0x50, 0x18, 0xff, 0xff, 0, 0, 0, 0]);
    frame.extend((0..payload_len).map(|i| (i % 251) as u8));
    frame
}

/// A small frame whose payload carries `n`, for ordering checks.
pub fn numbered_frame(n: u32) -> Vec<u8> {
    let mut frame = tcp_frame(1000, 22, 8);
    let len = frame.len();
    frame[len - 8..len - 4].copy_from_slice(&n.to_be_bytes());
    frame
}

pub fn frame_number(frame: &[u8]) -> u32 {
    let len = frame.len();
    u32::from_be_bytes(frame[len - 8..len - 4].try_into().unwrap())
}

/// What the receive path delivers until it reports no further frame, with the
/// kernel's receive loop around it (net::process_rx): the buffers are given
/// back explicitly only when a frame was handed up, so a drain whose
/// completions were all dropped relies on the driver to give theirs back.
pub fn drain() -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    while let Some(frame) = net_mmio::receive() {
        frames.push(frame.to_vec());
    }
    if !frames.is_empty() {
        net_mmio::recycle_rx_buffers();
    }
    frames
}
