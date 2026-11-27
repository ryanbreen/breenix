//! The sudo policy, `/etc/sudo.policy` (`docs/users/design.md`, section 3,
//! "sudo-style admin"). The grammar is Breenix's own and claims no
//! compatibility with any other program's:
//!
//! ```text
//! permit (USER | %GROUP) [as TARGET] [cmd /ABSOLUTE/PATH] [nopass]
//! ```
//!
//! Blank lines and `#` comments are ignored; each other line must parse, or
//! the whole policy is refused. Without `as` a rule permits root only; without
//! `cmd`, any command. The first rule that matches the caller, the target and
//! the command decides, and `nopass` lets it run without the caller's password.
use super::parse;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const MAX_POLICY_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    User(String),
    Group(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub who: Who,
    pub target: String,
    pub command: Option<String>,
    pub nopass: bool,
}

fn trusted_policy(uid: u32, gid: u32, mode: u32, links: u64) -> bool {
    uid == 0 && gid == 0 && mode == 0o100440 && links == 1
}

/// Check the path and the opened inode, then parse the bytes from that inode.
pub fn read_policy(path: &Path) -> Result<Vec<Rule>, String> {
    fn read(path: &Path) -> Result<Vec<Rule>, String> {
        let trusted = |meta: &fs::Metadata| {
            trusted_policy(meta.uid(), meta.gid(), meta.mode(), meta.nlink())
        };
        let before = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !trusted(&before) {
            return Err("expected a regular root:root file at mode 0440 with one link".into());
        }
        let file = File::open(path).map_err(|e| e.to_string())?;
        let opened = file.metadata().map_err(|e| e.to_string())?;
        if !trusted(&opened) || (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
            return Err("policy changed during open".into());
        }
        let mut text = String::new();
        file.take(MAX_POLICY_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        if text.len() as u64 > MAX_POLICY_BYTES {
            return Err("policy is too large".into());
        }
        parse_policy(&text)
    }
    read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parse a whole policy; the error names the first line that does not parse.
pub fn parse_policy(text: &str) -> Result<Vec<Rule>, String> {
    let mut rules = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        rules.push(parse_rule(line).ok_or_else(|| format!("line {} does not parse", number + 1))?);
    }
    Ok(rules)
}

fn parse_rule(line: &str) -> Option<Rule> {
    let mut words = line.split_whitespace().peekable();
    if words.next()? != "permit" {
        return None;
    }
    let who = words.next()?;
    let who = match who.strip_prefix('%') {
        Some(group) if parse::valid_name(group.as_bytes()) => Who::Group(group.into()),
        None if parse::valid_name(who.as_bytes()) => Who::User(who.into()),
        _ => return None,
    };
    let mut rule = Rule { who, target: "root".into(), command: None, nopass: false };
    if words.peek() == Some(&"as") {
        words.next();
        let target = words.next()?;
        if !parse::valid_name(target.as_bytes()) {
            return None;
        }
        rule.target = target.into();
    }
    if words.peek() == Some(&"cmd") {
        words.next();
        let command = words.next()?;
        if !parse::absolute(command.as_bytes()) {
            return None;
        }
        rule.command = Some(command.into());
    }
    if words.peek() == Some(&"nopass") {
        words.next();
        rule.nopass = true;
    }
    words.next().is_none().then_some(rule)
}

/// The first rule permitting `caller` (in `groups`, by name) to run
/// `command` (an absolute path) as `target`.
pub fn decide<'a>(rules: &'a [Rule], caller: &str, groups: &[String], target: &str, command: &str) -> Option<&'a Rule> {
    rules.iter().find(|rule| {
        let who = match &rule.who {
            Who::User(name) => name == caller,
            Who::Group(group) => groups.iter().any(|g| g == group),
        };
        who && rule.target == target && rule.command.as_deref().is_none_or(|c| c == command)
    })
}

/// Named primary and supplementary memberships from the account database.
pub fn group_names(group_file: &str, caller: &str, primary_gid: u32) -> Vec<String> {
    group_file
        .lines()
        .filter_map(|line| parse::Group::parse(line.as_bytes()))
        .filter(|group| {
            group.gid == primary_gid
                || group.fields[3].split(|b| *b == b',').any(|member| member == caller.as_bytes())
        })
        .filter_map(|group| std::str::from_utf8(group.fields[0]).ok().map(str::to_owned))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authentication {
    Passwordless,
    Password,
    Denied,
}

impl Authentication {
    pub fn requires_password(self) -> bool {
        self != Self::Passwordless
    }
}

/// Only a matching rule from a successfully loaded policy grants authority.
pub fn authentication(
    rules: Option<&[Rule]>,
    caller: &str,
    groups: &[String],
    target: &str,
    command: &str,
) -> Authentication {
    let Some(rules) = rules else {
        return Authentication::Denied;
    };
    match decide(rules, caller, groups, target, command) {
        Some(rule) if rule.nopass => Authentication::Passwordless,
        Some(_) => Authentication::Password,
        None => Authentication::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudo_policy_requires_a_password_unless_a_matching_rule_waives_it() {
        let rules = parse_policy("permit %wheel as root nopass\npermit fixture cmd /usr/bin/id\n").unwrap();
        let wheel = group_names("wheel:x:10:member\n", "member", 100);
        assert_eq!(authentication(Some(&rules), "member", &wheel, "root", "/usr/bin/id"), Authentication::Passwordless);
        let other = group_names("wheel:x:10:member\nalias:x:10:other\n", "other", 100);
        let denied = authentication(Some(&rules), "other", &other, "root", "/usr/bin/id");
        assert_eq!(denied, Authentication::Denied);
        assert!(denied.requires_password());
        assert_eq!(authentication(Some(&rules), "fixture", &[], "root", "/usr/bin/id"), Authentication::Password);
        assert_eq!(authentication(Some(&rules), "fixture", &[], "root", "/bin/sh"), Authentication::Denied);
    }

    #[test]
    fn sudo_policy_ignores_untrusted_metadata() {
        assert!(trusted_policy(0, 0, 0o100440, 1));
        for (uid, gid, mode, links) in [
            (1000, 0, 0o100440, 1),
            (0, 10, 0o100440, 1),
            (0, 0, 0o100460, 1),
            (0, 0, 0o100442, 1),
            (0, 0, 0o120440, 1),
            (0, 0, 0o100440, 2),
        ] {
            assert!(!trusted_policy(uid, gid, mode, links), "{uid}:{gid} {mode:o} {links}");
        }
    }

    #[test]
    fn sudo_policy_missing_or_invalid_grants_no_authority() {
        let missing = std::path::Path::new("/dev/null").join("sudo.policy");
        let rules = read_policy(&missing);
        assert!(rules.is_err());
        let wheel = vec!["wheel".to_string()];
        let malformed = parse_policy("# defaults\npermit %wheel nopass\n\npermit %wheel as\n");
        assert_eq!(malformed.as_ref().unwrap_err(), "line 4 does not parse");
        for unusable in [rules, malformed] {
            assert_eq!(
                authentication(unusable.as_deref().ok(), "member", &wheel, "root", "/usr/bin/id"),
                Authentication::Denied,
            );
        }
        assert_eq!(authentication(None, "other", &[], "root", "/usr/bin/id"), Authentication::Denied);
        assert_eq!(authentication(None, "member", &wheel, "fixture", "/usr/bin/id"), Authentication::Denied);
    }

    #[test]
    fn rules_parse_strictly() {
        let rules = parse_policy("# defaults\npermit %wheel as root\n\npermit ua as ub cmd /usr/bin/id nopass\n").unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0], Rule { who: Who::Group("wheel".into()), target: "root".into(), command: None, nopass: false });
        assert_eq!(rules[1], Rule { who: Who::User("ua".into()), target: "ub".into(), command: Some("/usr/bin/id".into()), nopass: true });
        for bad in ["permit", "allow ua", "permit ua as", "permit ua cmd id", "permit ua nopass extra",
                    "permit Ua", "permit %", "permit ua nopass as root"] {
            assert!(parse_policy(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn first_matching_rule_decides() {
        let rules = parse_policy("permit ua as root cmd /usr/bin/id nopass\npermit %wheel\n").unwrap();
        let wheel = vec!["wheel".to_string()];
        // A caller both rules permit: the first decides, in either order, so
        // neither last-match nor any-match evaluation passes.
        assert!(decide(&rules, "ua", &wheel, "root", "/usr/bin/id").is_some_and(|r| r.nopass));
        let reversed = parse_policy("permit %wheel\npermit ua as root cmd /usr/bin/id nopass\n").unwrap();
        assert!(decide(&reversed, "ua", &wheel, "root", "/usr/bin/id").is_some_and(|r| !r.nopass));
        assert!(decide(&rules, "ua", &[], "root", "/usr/bin/id").is_some_and(|r| r.nopass));
        assert!(decide(&rules, "ua", &[], "root", "/bin/sh").is_none());
        assert!(decide(&rules, "uw", &wheel, "root", "/bin/sh").is_some_and(|r| !r.nopass));
        // A rule without `as` permits root only.
        assert!(decide(&rules, "uw", &wheel, "ub", "/bin/sh").is_none());
    }
}
