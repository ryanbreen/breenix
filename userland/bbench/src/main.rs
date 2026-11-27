mod io_work;

use bu_sys::bench;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::hint::black_box;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

type Metrics = BTreeMap<&'static str, u64>;

const MIB: usize = 1024 * 1024;

fn ns(start: Instant) -> u64 {
    start.elapsed().as_nanos() as u64
}

fn json_string(value: &str) -> String {
    use std::fmt::Write;
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            ch if ch <= '\u{1f}' => write!(&mut quoted, "\\u{:04x}", ch as u32).unwrap(),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

fn counters() -> BTreeMap<&'static str, String> {
    [
        "/proc/stat",
        "/proc/self/stat",
        "/proc/meminfo",
        "/proc/cowinfo",
        "/proc/block/cache",
        "/proc/trace/rx_counters",
        "/proc/uptime",
    ]
    .into_iter()
    .filter_map(|path| fs::read_to_string(path).ok().map(|value| (path, value)))
    .collect()
}

fn object<T>(map: &BTreeMap<&str, T>, encode: impl Fn(&T) -> String) -> String {
    let fields = map
        .iter()
        .map(|(key, value)| format!("{}:{}", json_string(key), encode(value)))
        .collect::<Vec<_>>();
    format!("{{{}}}", fields.join(","))
}

fn command(args: &[String], metrics: &mut Metrics) -> io::Result<()> {
    let [cwd, executable, args @ ..] = args else {
        return Err(invalid("command CWD EXECUTABLE [ARG ...]"));
    };
    let start = Instant::now();
    let mut child = Command::new(executable)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut byte = [0; 1];
    let read = stdout.read(&mut byte);
    if matches!(read, Ok(1)) {
        metrics.insert("first_output_ns", ns(start));
    }
    let tail = io::copy(&mut stdout, &mut io::sink());
    drop(stdout);
    let status = child.wait()?;
    metrics.insert("stdout_bytes", read? as u64 + tail?);
    if !status.success() {
        return Err(io::Error::other(status.to_string()));
    }
    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn count(value: &str, max: usize) -> io::Result<usize> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| (1..=max).contains(value))
        .ok_or_else(|| invalid("count outside allowed range"))
}

fn workload(dir: &Path, args: &[String], metrics: &mut Metrics) -> io::Result<()> {
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    match words.as_slice() {
        ["command", ..] => command(&args[1..], metrics),
        ["fork-exec", binary, loops] => {
            let loops = count(loops, 10_000)?;
            if !Path::new(binary).is_absolute() {
                return Err(invalid("fork-exec expects an absolute executable path"));
            }
            bench::fork_exec_wait(Path::new(binary), loops)?;
            metrics.insert("children", loops as u64);
            Ok(())
        }
        ["anonymous"] => {
            let size = 256 * MIB;
            let page = bench::page_size();
            bench::touch_anonymous(size, page)?;
            metrics.extend([
                ("bytes", size as u64),
                ("pages_touched", (size / page) as u64),
                ("page_bytes", page as u64),
            ]);
            Ok(())
        }
        ["file-map", path] => {
            let file = File::open(path)?;
            let len = file.metadata()?.len() as usize;
            let start = Instant::now();
            let mapping = bench::Mapping::new(len, Some(&file))?;
            metrics.insert("map_ns", ns(start));
            let start = Instant::now();
            let stride = 4096;
            metrics.insert("checksum", mapping.touch_read(stride));
            metrics.insert("touch_ns", ns(start));
            metrics.extend([
                ("bytes", len as u64),
                ("stride_bytes", stride as u64),
                ("page_bytes", bench::page_size() as u64),
            ]);
            Ok(())
        }
        ["copy"] => {
            let mut source = vec![0x55; 128 * MIB];
            let mut target = vec![0xaa; source.len()];
            black_box(&mut source);
            black_box(&mut target);
            let start = Instant::now();
            for _ in 0..8 {
                target.copy_from_slice(black_box(&source));
                black_box(&mut target);
            }
            metrics.extend([
                ("copy_ns", ns(start)),
                ("bytes_copied", (source.len() * 8) as u64),
            ]);
            Ok(())
        }
        ["sequential", bytes] => io_work::sequential(dir, count(bytes, 1024 * MIB)?, metrics),
        ["small-files", files] => io_work::small_files(dir, count(files, 10_000)?, metrics),
        ["walk", files] => io_work::walk(dir, count(files, 10_000)?, metrics),
        ["fsync", calls] => io_work::sync_latency(dir, count(calls, 1000)?, metrics),
        _ => Err(invalid("unknown workload or argument count")),
    }
}

fn run(args: &[String]) -> io::Result<bool> {
    let [data, reports, sample, workload_args @ ..] = args else {
        return Err(invalid(
            "usage: bbench DATA_DIR REPORT_DIR SAMPLE WORKLOAD [ARG ...]",
        ));
    };
    if sample.is_empty()
        || !sample
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(invalid(
            "sample must contain only ASCII letters, digits or hyphens",
        ));
    }
    let reports = Path::new(reports);
    let destination = reports.join(format!("{sample}.json"));
    let partial = destination.with_extension("partial");
    if destination.try_exists()? || partial.try_exists()? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "sample already exists",
        ));
    }
    let dir = Path::new(data).join(sample);
    fs::create_dir(&dir)?;
    let before = counters();
    let start = Instant::now();
    let mut metrics = Metrics::new();
    let result = workload(&dir, workload_args, &mut metrics);
    let wall = ns(start);
    let after = counters();
    let cleanup = fs::remove_dir_all(dir);
    let result = result.and(cleanup);
    let error = result
        .as_ref()
        .err()
        .map_or("null".into(), |err| json_string(&err.to_string()));
    let argv = workload_args
        .iter()
        .map(|arg| json_string(arg))
        .collect::<Vec<_>>();
    let report = format!(
        "{{\"sample\":{},\"argv\":[{}],\"wall_ns\":{wall},\"metrics\":{},\"error\":{error},\"before\":{},\"after\":{}}}\n",
        json_string(sample), argv.join(","), object(&metrics, u64::to_string),
        object(&before, |s| json_string(s)), object(&after, |s| json_string(s)),
    );
    let mut file = File::options()
        .create_new(true)
        .write(true)
        .open(&partial)?;
    file.write_all(report.as_bytes())?;
    file.sync_all()?;
    // Publish without replacing a completed run, even if a sample ID is reused.
    if destination.try_exists()? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "sample already exists",
        ));
    }
    fs::rename(partial, destination)?;
    File::open(reports)?.sync_all()?;
    io::stdout().lock().write_all(report.as_bytes())?;
    Ok(result.is_ok())
}

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("bbench: {error}");
            ExitCode::FAILURE
        }
    }
}
