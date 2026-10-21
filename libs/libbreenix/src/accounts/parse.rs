//! Strict, borrowed records: a bad line is unusable, its neighbours still parse.

pub fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && (name[0].is_ascii_lowercase() || name[0] == b'_')
        && name
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(b))
}

pub fn id(text: &[u8]) -> Option<u32> {
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    text.iter()
        .try_fold(0u32, |n, b| {
            n.checked_mul(10)?.checked_add((b - b'0') as u32)
        })
        .filter(|n| *n != u32::MAX)
}

pub fn fields<const N: usize>(line: &[u8]) -> Option<[&[u8]; N]> {
    if line.iter().any(|b| b.is_ascii_control()) {
        return None;
    }
    core::str::from_utf8(line).ok()?;
    let mut out = [&[][..]; N];
    let mut parts = line.split(|b| *b == b':');
    for slot in &mut out {
        *slot = parts.next()?;
    }
    if parts.next().is_some() {
        None
    } else {
        Some(out)
    }
}

pub fn absolute(path: &[u8]) -> bool {
    path.starts_with(b"/")
        && !path.is_empty()
        && (path == b"/"
            || path
                .split(|b| *b == b'/')
                .skip(1)
                .all(|p| !p.is_empty() && p != b"." && p != b".."))
}

#[derive(Debug)]
pub struct Passwd<'a> {
    pub fields: [&'a [u8]; 7],
    pub uid: u32,
    pub gid: u32,
}
impl<'a> Passwd<'a> {
    pub fn parse(line: &'a [u8]) -> Option<Self> {
        let f = fields::<7>(line)?;
        if !valid_name(f[0]) || f[1] != b"x" || !absolute(f[5]) || !absolute(f[6]) {
            return None;
        }
        Some(Self {
            uid: id(f[2])?,
            gid: id(f[3])?,
            fields: f,
        })
    }
}

#[derive(Debug)]
pub struct Group<'a> {
    pub fields: [&'a [u8]; 4],
    pub gid: u32,
}
impl<'a> Group<'a> {
    pub fn parse(line: &'a [u8]) -> Option<Self> {
        let f = fields::<4>(line)?;
        if !valid_name(f[0])
            || f[1] != b"x"
            || (!f[3].is_empty() && !f[3].split(|b| *b == b',').all(valid_name))
        {
            return None;
        }
        Some(Self {
            gid: id(f[2])?,
            fields: f,
        })
    }
}

#[derive(Debug)]
pub struct Shadow<'a> {
    pub fields: [&'a [u8]; 9],
    pub last_change: Option<u32>,
    pub minimum: Option<u32>,
    pub maximum: Option<u32>,
    pub inactive: Option<u32>,
    pub expire: Option<u32>,
}
fn days(text: &[u8]) -> Option<Option<u32>> {
    if text.is_empty() {
        Some(None)
    } else {
        id(text).map(Some)
    }
}
impl<'a> Shadow<'a> {
    pub fn parse(line: &'a [u8]) -> Option<Self> {
        let f = fields::<9>(line)?;
        if !valid_name(f[0]) || !f[8].is_empty() {
            return None;
        }
        // An empty hash is syntactically valid; authentication must refuse it.
        let hash = f[1].strip_prefix(b"!").unwrap_or(f[1]);
        if !hash.is_empty() && hash != b"*" && hash != b"!" && !valid_hash(hash) {
            return None;
        }
        days(f[5])?;
        Some(Self {
            last_change: days(f[2])?,
            minimum: days(f[3])?,
            maximum: days(f[4])?,
            inactive: days(f[6])?,
            expire: days(f[7])?,
            fields: f,
        })
    }
    pub fn expired(&self, today: u32) -> bool {
        self.expire.is_some_and(|day| day < today)
    }
    pub fn password_allowed(&self, today: u32) -> bool {
        let hash = self.fields[1];
        !self.expired(today)
            && hash.starts_with(b"$6$")
            && !self
                .last_change
                .zip(self.maximum)
                .is_some_and(|(last, max)| today > last.saturating_add(max))
    }
}

fn valid_hash(hash: &[u8]) -> bool {
    let Some(mut body) = hash.strip_prefix(b"$6$") else {
        return false;
    };
    if let Some(rest) = body.strip_prefix(b"rounds=") {
        let Some(end) = rest.iter().position(|b| *b == b'$') else {
            return false;
        };
        if !id(&rest[..end]).is_some_and(|n| (1000..=999_999_999).contains(&n)) {
            return false;
        }
        body = &rest[end + 1..];
    }
    let Some(end) = body.iter().position(|b| *b == b'$') else {
        return false;
    };
    let alphabet = |b: &u8| b.is_ascii_alphanumeric() || b"./".contains(b);
    (1..=16).contains(&end)
        && body[..end].iter().all(alphabet)
        && body[end + 1..].len() == 86
        && body[end + 1..].iter().all(alphabet)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_records_disable_only_the_bad_line() {
        let records = b"ok:x:1000:1000::/home/ok:/bin/bsh\nBad:x:1001:1001::/home/Bad:/bin/bsh\nno:x:4294967296:0::/:/bin/sh\nyes:x:1002:1002::/:/bin/sh";
        let names: std::vec::Vec<_> = records
            .split(|b| *b == b'\n')
            .filter_map(Passwd::parse)
            .map(|p| p.fields[0])
            .collect();
        assert_eq!(names, [b"ok".as_slice(), b"yes"]);
        assert!(Group::parse(b"group:x:2100:ok,yes").is_some());
        assert!(Group::parse(b"group:x:2100:ok,,yes").is_none());
        assert!(Passwd::parse(b"bad:x:10:10::/home/../root:/bin/sh").is_none());
        assert!(Passwd::parse(b"bad:x:10:10::/home/bad:/bin/sh\r").is_none());
    }
    #[test]
    fn shadow_expiration_and_password_locks_are_independent() {
        let locked = Shadow::parse(b"ok:!:10:0:99999:7::20:").unwrap();
        assert!(!locked.expired(20));
        assert!(locked.expired(21));
        assert!(!locked.password_allowed(15));
        assert!(Shadow::parse(b"ok::10:0:99999:7:::").is_some());
        assert!(Shadow::parse(b"ok:$6$broken:10:0:99999:7:::").is_none());
        assert!(Shadow::parse(b"ok:!:10:0:x:7:::").is_none());
    }
}
