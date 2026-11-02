#![cfg(test)]
#[path = "../../../kernel/src/syscall/power_token_validation.rs"]
mod power_token_validation;
use power_token_validation::{validate, File, Kind, Invalid};

fn file<'a>(data: &'a [u8], mode: u16) -> File<'a> {
    File { regular: true, uid: 0, mode, size: data.len() as u64, data }
}

#[test]
fn validates_token_and_nonce() {
    let token = [b'a'; 64];
    let mut nonce = [b'b'; 65];
    nonce[64] = b'\n';
    assert_eq!(validate(Kind::Token, Some(file(&token, 0o400))), Ok(()));
    assert_eq!(validate(Kind::Nonce, Some(file(&nonce, 0o600))), Ok(()));
    assert_eq!(validate(Kind::Token, None), Err(Invalid::Absent));
    assert_eq!(validate(Kind::Token, Some(file(&token, 0o600))), Err(Invalid::Metadata));
    let mut bad = token;
    bad[0] = b'z';
    assert_eq!(validate(Kind::Token, Some(file(&bad, 0o400))), Err(Invalid::Content));
    nonce[64] = b'x';
    assert_eq!(validate(Kind::Nonce, Some(file(&nonce, 0o600))), Err(Invalid::Content));
    let mut wrong = file(&token, 0o400);
    wrong.uid = 1;
    assert_eq!(validate(Kind::Token, Some(wrong)), Err(Invalid::Metadata));
    assert_eq!(validate(Kind::Token, Some(file(&token[..63], 0o400))), Err(Invalid::Metadata));
}
