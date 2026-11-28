//! Attribution workloads: system calls, the C allocator, raw TCP and the SSH
//! crypto primitives, each timed in-process so both systems run the same code.
use crate::{ns, Metrics};
use libbreenix::crypto::gcm::AesGcm;
use libbreenix::crypto::hmac::HmacSha256;
use std::hint::black_box;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Barrier;
use std::thread;
use std::time::Instant;

/// SSH carries at most 32 KiB of payload per packet (RFC 4253 6.1).
const PACKET: usize = 32 * 1024;
const TCP_CHUNK: usize = 64 * 1024;

pub fn getpid(loops: usize, metrics: &mut Metrics) -> io::Result<()> {
    let start = Instant::now();
    let mut sum = 0u64;
    for _ in 0..loops {
        sum = sum.wrapping_add(u64::from(black_box(std::process::id())));
    }
    metrics.insert("getpid_ns", ns(start));
    metrics.insert("calls", loops as u64);
    black_box(sum);
    Ok(())
}

/// Allocate and free one 64-byte block `loops` times on each of `threads`
/// threads, started together. The time runs from the release to the last join.
pub fn malloc(threads: usize, loops: usize, metrics: &mut Metrics) -> io::Result<()> {
    let barrier = Barrier::new(threads + 1);
    let elapsed = thread::scope(|scope| {
        let workers = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    for _ in 0..loops {
                        let block: Vec<u8> = Vec::with_capacity(64);
                        black_box(&block);
                    }
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let start = Instant::now();
        let joined = workers.into_iter().all(|worker| worker.join().is_ok());
        joined.then(|| ns(start))
    });
    let elapsed = elapsed.ok_or_else(|| io::Error::other("an allocator thread panicked"))?;
    metrics.extend([
        ("malloc_ns", elapsed),
        ("threads", threads as u64),
        ("pairs_per_thread", loops as u64),
    ]);
    Ok(())
}

/// Raw TCP to the rig's host endpoint. The guest opens the connection and
/// sends a 9-byte request: `S` (guest sends) or `R` (guest receives), then the
/// byte count as a big-endian u64. For `S` the host answers with the count it
/// received once that count has arrived, so the time covers delivery.
pub fn tcp(direction: &str, address: &str, bytes: usize, metrics: &mut Metrics) -> io::Result<()> {
    let tag = match direction {
        "send" => b'S',
        "recv" => b'R',
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "tcp send|recv")),
    };
    let mut stream = TcpStream::connect(address)?;
    stream.set_nodelay(true)?;
    let mut request = [0u8; 9];
    request[0] = tag;
    request[1..].copy_from_slice(&(bytes as u64).to_be_bytes());
    let chunk: Vec<u8> = (0..TCP_CHUNK).map(|i| (i * 31 % 251) as u8).collect();
    let mut buffer = vec![0u8; TCP_CHUNK];
    let start = Instant::now();
    stream.write_all(&request)?;
    let moved = if tag == b'S' {
        let mut remaining = bytes;
        while remaining != 0 {
            let count = remaining.min(chunk.len());
            stream.write_all(&chunk[..count])?;
            remaining -= count;
        }
        let mut answer = [0u8; 8];
        stream.read_exact(&mut answer)?;
        u64::from_be_bytes(answer) as usize
    } else {
        let mut received = 0;
        while received < bytes {
            match stream.read(&mut buffer)? {
                0 => break,
                count => received += count,
            }
        }
        received
    };
    metrics.insert("tcp_ns", ns(start));
    let _ = stream.shutdown(Shutdown::Both);
    if moved != bytes {
        return Err(io::Error::other(format!("tcp {direction} moved {moved} of {bytes} bytes")));
    }
    metrics.insert("bytes", bytes as u64);
    Ok(())
}

/// AES-256-GCM sealing of `bytes` in SSH-sized packets, as aes256-gcm@openssh.com
/// uses it: a 4-byte length as associated data and a per-packet nonce.
pub fn aes_gcm(bytes: usize, metrics: &mut Metrics) -> io::Result<()> {
    let cipher = AesGcm::new256(&[0x42; 32]);
    let plaintext: Vec<u8> = (0..PACKET).map(|i| i as u8).collect();
    let mut sealed = vec![0u8; PACKET];
    let mut tag = [0u8; 16];
    let mut nonce = [0u8; 12];
    let mut folded = 0u8;
    let start = Instant::now();
    let mut remaining = bytes;
    let mut sequence = 0u64;
    while remaining != 0 {
        let count = remaining.min(PACKET);
        nonce[4..].copy_from_slice(&sequence.to_be_bytes());
        let aad = (count as u32).to_be_bytes();
        cipher.encrypt(&nonce, &aad, &plaintext[..count], &mut sealed[..count], &mut tag);
        folded ^= black_box(tag[0]);
        remaining -= count;
        sequence += 1;
    }
    metrics.insert("crypto_ns", ns(start));
    metrics.insert("bytes", bytes as u64);
    metrics.insert("tag_fold", u64::from(folded));
    Ok(())
}

/// HMAC-SHA256 over `bytes` in SSH-sized packets, one MAC per packet.
pub fn hmac_sha256(bytes: usize, metrics: &mut Metrics) -> io::Result<()> {
    let message: Vec<u8> = (0..PACKET).map(|i| (i * 7) as u8).collect();
    let mut folded = 0u8;
    let start = Instant::now();
    let mut remaining = bytes;
    let mut sequence = 0u32;
    while remaining != 0 {
        let count = remaining.min(PACKET);
        let mut mac = HmacSha256::new(&[0x24; 32]);
        mac.update(&sequence.to_be_bytes());
        mac.update(&message[..count]);
        folded ^= black_box(mac.finalize()[0]);
        remaining -= count;
        sequence = sequence.wrapping_add(1);
    }
    metrics.insert("crypto_ns", ns(start));
    metrics.insert("bytes", bytes as u64);
    metrics.insert("tag_fold", u64::from(folded));
    Ok(())
}

/// Write a deterministic file of `bytes` and sync it, for the file-map rows on
/// systems without the R240's compiler driver.
pub fn fixture(path: &str, bytes: usize) -> io::Result<()> {
    let block: Vec<u8> = (0..1024 * 1024u32)
        .map(|i| (i.wrapping_mul(2_246_822_519) >> 24) as u8)
        .collect();
    let mut file = std::fs::File::create(path)?;
    let mut remaining = bytes;
    while remaining != 0 {
        let count = remaining.min(block.len());
        file.write_all(&block[..count])?;
        remaining -= count;
    }
    file.sync_all()
}
