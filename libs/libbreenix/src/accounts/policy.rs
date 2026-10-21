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

#[cfg(test)]
mod tests {
    use super::*;

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
