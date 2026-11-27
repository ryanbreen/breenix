use crate::{ns, Metrics};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Instant;

pub fn sequential(dir: &Path, size: usize, metrics: &mut Metrics) -> io::Result<()> {
    let path = dir.join("stream");
    let payload: Vec<_> = (0..1024 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    let start = Instant::now();
    let mut file = File::create(&path)?;
    let mut remaining = size;
    while remaining != 0 {
        let count = remaining.min(payload.len());
        file.write_all(&payload[..count])?;
        remaining -= count;
    }
    let sync = Instant::now();
    file.sync_all()?;
    metrics.insert("sync_ns", ns(sync));
    metrics.insert("write_ns", ns(start));
    metrics.insert("bytes_written", size as u64);
    drop(file);

    let mut block = vec![0; payload.len()];
    let start = Instant::now();
    let mut file = File::open(path)?;
    let mut bytes = 0;
    while bytes < size {
        let count = (size - bytes).min(block.len());
        file.read_exact(&mut block[..count])?;
        if block[0] != payload[0] || block[count - 1] != payload[count - 1] {
            return Err(io::Error::other("sequential read sample mismatch"));
        }
        bytes += count;
    }
    if file.read(&mut block[..1])? != 0 {
        return Err(io::Error::other("sequential file larger than requested"));
    }
    metrics.insert("read_ns", ns(start));
    metrics.insert("bytes_read", bytes as u64);
    Ok(())
}

fn make_paths(dir: &Path, count: usize, target: bool) -> io::Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::with_capacity(count);
    for shard in 0..count.div_ceil(100) {
        let parent = if target {
            let area = ["deps", "incremental", "build", ".fingerprint"][shard % 4];
            dir.join("target")
                .join("release")
                .join(area)
                .join(format!("pkg-{shard:04x}"))
        } else {
            dir.join(format!("{shard:04x}"))
        };
        fs::create_dir_all(&parent)?;
        for entry in 0..(count - shard * 100).min(100) {
            paths.push(parent.join(format!("artifact-{entry:04x}.o")));
        }
    }
    Ok(paths)
}

pub fn small_files(dir: &Path, count: usize, metrics: &mut Metrics) -> io::Result<()> {
    let paths = make_paths(dir, count, false)?;
    let start = Instant::now();
    for path in &paths {
        fs::write(path, [0xa5; 128])?;
    }
    metrics.insert("create_ns", ns(start));
    let start = Instant::now();
    for path in &paths {
        if fs::metadata(path)?.len() != 128 {
            return Err(io::Error::other("small-file size mismatch"));
        }
    }
    metrics.insert("stat_ns", ns(start));
    let start = Instant::now();
    for path in &paths {
        fs::remove_file(path)?;
    }
    metrics.insert("unlink_ns", ns(start));
    metrics.insert("files", paths.len() as u64);
    Ok(())
}

pub fn walk(dir: &Path, count: usize, metrics: &mut Metrics) -> io::Result<()> {
    for path in make_paths(dir, count, true)? {
        File::create(path)?;
    }
    let mut dirs = vec![dir.join("target")];
    let mut files = 0;
    let start = Instant::now();
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dirs.push(entry.path());
            } else {
                files += 1;
            }
        }
    }
    metrics.insert("walk_ns", ns(start));
    if files != count {
        return Err(io::Error::other(format!(
            "walk saw {files} of {count} files"
        )));
    }
    metrics.insert("files", files as u64);
    Ok(())
}

pub fn sync_latency(dir: &Path, count: usize, metrics: &mut Metrics) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .append(true)
        .open(dir.join("sync"))?;
    let mut total = 0;
    let mut min = u64::MAX;
    let mut max = 0;
    for i in 0..count {
        file.write_all(&[i as u8; 4096])?;
        let start = Instant::now();
        file.sync_all()?;
        let elapsed = ns(start);
        total += elapsed;
        min = min.min(elapsed);
        max = max.max(elapsed);
    }
    metrics.extend([
        ("fsync_ns", total / count as u64),
        ("fsync_min_ns", min),
        ("fsync_max_ns", max),
        ("calls", count as u64),
    ]);
    Ok(())
}
