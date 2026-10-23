"""Exercise native fence assignment in the production VirtIO queue sender."""
from pathlib import Path
import os
import re
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]


def sender():
    source = (ROOT / "kernel/src/gpu/virgl/mod.rs").read_text()
    match = re.search(r"(?m)^    pub fn send\(", source)
    if match is None:
        raise AssertionError("production VirtIO sender is missing")
    start = match.start()
    opening = source.index("{", start)
    depth = 1
    for end in range(opening + 1, len(source)):
        depth += (source[end] == "{") - (source[end] == "}")
        if depth == 0:
            return textwrap.dedent(source[start:end + 1])
    raise AssertionError("production VirtIO sender has no closing brace")


class NativeFence(unittest.TestCase):
    def test_native_fences_follow_queue_order_not_preparation_order(self):
        harness = r'''
mod wire { pub const HDR_FENCE: usize = 8; }
struct Header([u8; 24]);
impl Header {
    fn put_u64(&mut self, at: usize, value: u64) { self.0[at..at+8].copy_from_slice(&value.to_le_bytes()); }
    fn u64_at(&self, at: usize) -> u64 { u64::from_le_bytes(self.0[at..at+8].try_into().unwrap()) }
}
struct Request { fence: Option<u64>, hdr: Header }
impl Request {
    fn new(fence: Option<u64>) -> Self {
        let mut hdr = Header([0; 24]);
        if let Some(fence) = fence { hdr.put_u64(wire::HDR_FENCE, fence); }
        Self { fence, hdr }
    }
    fn segments(&self) -> Vec<()> { vec![(); 2] }
    fn publish(&self) {}
}
struct Queue { free: usize, next: u16 }
impl Queue {
    fn free_count(&self) -> usize { self.free }
    fn add(&mut self, segments: &[()]) -> Option<u16> {
        if segments.len() > self.free { return None; }
        self.free -= segments.len();
        let head = self.next;
        self.next += 1;
        Some(head)
    }
}
struct Device { queue: Queue, inflight: Vec<Option<Request>>, made: u64, made_at: Vec<u64> }
impl Device {
    // PRODUCTION_SENDER
    fn new() -> Self { Self { queue: Queue { free: 16, next: 0 },
        inflight: (0..8).map(|_| None).collect(), made: 0, made_at: vec![0; 8] } }
}
#[test]
fn delayed_preparation_cannot_reverse_native_fences() {
    let mut d = Device::new();
    // Admission 9 was delayed while admission 44 finished preparing. Queue order
    // is authoritative for the device timeline, even across contexts and batches.
    for token in [Some(44), None, Some(9), Some(88)] {
        assert!(d.send(Request::new(token)).is_ok());
    }
    let wire: Vec<_> = d.inflight.iter().flatten().map(|r| r.hdr.u64_at(wire::HDR_FENCE)).collect();
    assert_eq!(wire, [1, 0, 3, 4], "native fence numbers must increase in queue order");
    let tokens: Vec<_> = d.inflight.iter().flatten().map(|r| r.fence).collect();
    assert_eq!(tokens, [Some(44), None, Some(9), Some(88)], "admission identities must survive native numbering");
    assert_eq!(&d.made_at[..4], [1, 2, 3, 4]);
}
#[test]
fn backpressure_does_not_consume_a_native_fence() {
    let mut d = Device::new();
    d.queue.free = 0;
    let request = d.send(Request::new(Some(90))).err().unwrap();
    assert_eq!(d.made, 0);
    assert_eq!(request.hdr.u64_at(wire::HDR_FENCE), 90);
    d.queue.free = 2;
    assert!(d.send(request).is_ok());
    assert_eq!(d.inflight[0].as_ref().unwrap().hdr.u64_at(wire::HDR_FENCE), 1);
}
'''
        harness = harness.replace("// PRODUCTION_SENDER", sender())
        with tempfile.TemporaryDirectory(prefix="gpu-native-fence-") as directory:
            source = Path(directory) / "native.rs"
            binary = Path(directory) / "native"
            source.write_text(harness)
            subprocess.run([os.environ.get("RUSTC", "rustc"), "--edition=2021", "--test", "-Dwarnings", str(source), "-o", str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    unittest.main()
