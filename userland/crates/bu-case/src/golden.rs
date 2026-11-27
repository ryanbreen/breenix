use std::fmt::Write;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub const MAX_OUTPUT: usize = 1024 * 1024;

pub struct Goldens {
    pub out: Vec<u8>,
    pub err: Vec<u8>,
    pub status: String,
}

impl Goldens {
    pub fn load(suite: &Path, case: &str) -> Result<Self, String> {
        let read = |extension| {
            let path = suite.join("golden").join(format!("{case}.{extension}"));
            read_output(&path).map_err(|e| format!("{}: {e}", path.display()))
        };
        let out = read("out")?;
        let err = read("err")?;
        let raw_status = read("status")?;
        let status = std::str::from_utf8(&raw_status)
            .ok()
            .and_then(|text| text.strip_suffix('\n'))
            .filter(|status| valid_status(status))
            .ok_or_else(|| {
                format!("{case}: expected an exit code or signal:N followed by a newline")
            })?;
        Ok(Self {
            out,
            err,
            status: status.into(),
        })
    }
}

fn valid_status(status: &str) -> bool {
    let number = |value: &str| value.parse::<u8>().ok().filter(|n| n.to_string() == value);
    if let Some(signal) = status.strip_prefix("signal:") {
        return number(signal).is_some_and(|n| n > 0);
    }
    number(status).is_some()
}

fn read_output(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_OUTPUT as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_OUTPUT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "golden exceeds 1 MiB",
        ));
    }
    Ok(bytes)
}

pub fn first_difference(
    expected_out: &[u8],
    actual_out: &[u8],
    expected_err: &[u8],
    actual_err: &[u8],
    expected_status: &str,
    actual_status: &str,
) -> Option<String> {
    let status_expected = format!("{expected_status}\n");
    let status_actual = format!("{actual_status}\n");
    let difference = [
        ("out", expected_out, actual_out),
        ("err", expected_err, actual_err),
        (
            "status",
            status_expected.as_bytes(),
            status_actual.as_bytes(),
        ),
    ]
    .into_iter()
    .find(|(_, want, got)| want != got)
    .map(|(stream, want, got)| diff(stream, want, got));
    difference
}

fn diff(stream: &str, want: &[u8], got: &[u8]) -> String {
    let mut diff = format!("--- expected.{stream}\n+++ actual.{stream}\n");
    let (Ok(want), Ok(got)) = (std::str::from_utf8(want), std::str::from_utf8(got)) else {
        let offset = want.iter().zip(got).take_while(|(a, b)| a == b).count();
        let end = offset + 16;
        let _ = writeln!(
            diff,
            "Binary mismatch at byte {offset}\n-{:02x?}\n+{:02x?}",
            &want[offset..want.len().min(end)],
            &got[offset..got.len().min(end)]
        );
        return diff;
    };
    let want: Vec<_> = want.split_inclusive('\n').collect();
    let got: Vec<_> = got.split_inclusive('\n').collect();
    let prefix = want.iter().zip(&got).take_while(|(a, b)| a == b).count();
    let suffix = want[prefix..]
        .iter()
        .rev()
        .zip(got[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let start = prefix.saturating_sub(2);
    let trailing = suffix.min(2);
    let old_end = want.len() - suffix;
    let new_end = got.len() - suffix;
    let old_range = range(start, old_end + trailing - start);
    let new_range = range(start, new_end + trailing - start);
    let _ = writeln!(diff, "@@ -{old_range} +{new_range} @@");
    lines(&mut diff, ' ', &want[start..prefix]);
    lines(&mut diff, '-', &want[prefix..old_end]);
    lines(&mut diff, '+', &got[prefix..new_end]);
    lines(&mut diff, ' ', &want[old_end..old_end + trailing]);
    diff
}

fn range(start: usize, count: usize) -> String {
    match count {
        0 => format!("{start},0"),
        1 => (start + 1).to_string(),
        _ => format!("{},{}", start + 1, count),
    }
}

fn lines(diff: &mut String, marker: char, lines: &[&str]) {
    for line in lines {
        diff.push(marker);
        diff.push_str(line);
        if !line.ends_with('\n') {
            diff.push_str("\n\\ No newline at end of file\n");
        }
    }
}
