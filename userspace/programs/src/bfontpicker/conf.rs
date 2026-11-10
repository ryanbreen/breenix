//! Preserve configuration text while updating one setting.

pub fn set(contents: &str, key: &str, value: &str) -> String {
    let mut result = String::with_capacity(contents.len() + key.len() + value.len() + 2);
    let mut found = false;
    for line in contents.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        if let Some(eq) = body.find('=') {
            if !body.trim_start().starts_with('#') && body[..eq].trim() == key {
                let old = &body[eq + 1..];
                let leading = old.len() - old.trim_start().len();
                let trailing = old.trim_start().len() - old.trim().len();
                result.push_str(&body[..eq + 1 + leading]);
                result.push_str(value);
                result.push_str(&old[old.len() - trailing..]);
                result.push_str(&line[body.len()..]);
                found = true;
                continue;
            }
        }
        result.push_str(line);
    }
    if !found {
        if !result.is_empty() && !result.ends_with('\n') { result.push('\n'); }
        result.push_str(key);
        result.push('=');
        result.push_str(value);
        result.push('\n');
    }
    result
}
