//! Shell shorthand word boundaries, from POSIX XCU 2.2 (quoting).
//! bsh does no shell expansion; quoted scripts are passed literally to sh -c.
pub fn words(line: &str) -> Result<Vec<String>, &'static str> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && quote != Some('\'') {
            let next = chars.next().ok_or("unfinished escape")?;
            if next == '\n' { continue; }
            if quote == Some('"') && !matches!(next, '$' | '`' | '"' | '\\') { word.push('\\'); }
            word.push(next);
            started = true;
        } else if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && matches!(c, '\'' | '"') {
            quote = Some(c);
            started = true;
        } else if quote.is_none() && c.is_whitespace() {
            if started { result.push(core::mem::take(&mut word)); started = false; }
        } else {
            word.push(c);
            started = true;
        }
    }
    if quote.is_some() { return Err("unterminated quote"); }
    if started { result.push(word); }
    Ok(result)
}

/// A JavaScript string literal, preserving shell words byte for byte.
pub fn js_string(word: &str) -> String {
    let mut out = String::from("\"");
    for c in word.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_script_and_empty_arguments() {
        assert_eq!(words(r#"/bin/sh -c 'set -e; printf "%s\n" "$1"' sh 'two words' ''"#).unwrap(),
            ["/bin/sh", "-c", "set -e; printf \"%s\\n\" \"$1\"", "sh", "two words", ""]);
        assert_eq!(words(r#"echo a"b c"'d' e\ f "a\qb""#).unwrap(), ["echo", "ab cd", "e f", "a\\qb"]);
        assert!(words("sh -c 'unfinished").is_err());
        assert!(words("echo \\").is_err());
        assert_eq!(js_string("a\\b\n\""), "\"a\\\\b\\n\\\"\"");
    }
}
