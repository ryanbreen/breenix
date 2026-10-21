//! SSH authentication layer (RFC 4252)
//!
//! The server accepts only the publickey method (RFC 4252 section 7): the key
//! must be one the account's authorized keys list, as the `Authorizer` decides,
//! and the request must carry a valid signature by that key. Each other
//! method, password included, receives USERAUTH_FAILURE.

use super::keys;
use super::packet::PacketIo;
use super::{SshBuf, SshError};
use super::{SSH_MSG_DEBUG, SSH_MSG_EXT_INFO, SSH_MSG_IGNORE};
use super::{SSH_MSG_SERVICE_ACCEPT, SSH_MSG_SERVICE_REQUEST};
use super::{
    SSH_MSG_USERAUTH_BANNER, SSH_MSG_USERAUTH_FAILURE, SSH_MSG_USERAUTH_REQUEST,
    SSH_MSG_USERAUTH_SUCCESS,
};

/// SSH_MSG_USERAUTH_PK_OK (RFC 4252 §7)
const SSH_MSG_USERAUTH_PK_OK: u8 = 60;
/// SSH_MSG_USERAUTH_INFO_REQUEST / INFO_RESPONSE (RFC 4256 §3)
const SSH_MSG_USERAUTH_INFO_REQUEST: u8 = 60;
const SSH_MSG_USERAUTH_INFO_RESPONSE: u8 = 61;

/// Authentication requests one connection may make before it is disconnected.
const MAX_AUTH_REQUESTS: usize = 20;

/// Decides whether an account may log in with a public key.
pub trait Authorizer {
    /// Whether `user` names an account whose authorized keys include `key_blob`.
    /// Unknown accounts and unreadable or unsafe key files are refusals.
    fn authorized(&mut self, user: &str, key_blob: &[u8]) -> bool;
}

/// Whether `algorithm` is a signature algorithm for keys of the blob's type.
fn algorithm_fits_key(algorithm: &[u8], key_blob: &[u8]) -> bool {
    let key_type = keys::key_type_from_blob(key_blob);
    match algorithm {
        b"ssh-ed25519" => key_type.as_deref() == Some(keys::ED25519),
        b"rsa-sha2-256" | b"rsa-sha2-512" => key_type.as_deref() == Some(keys::RSA),
        _ => false,
    }
}

/// Handle the "ssh-userauth" service request (server side).
///
/// Skips any SSH_MSG_EXT_INFO (7), SSH_MSG_IGNORE (2), or SSH_MSG_DEBUG (4)
/// messages that the client may send after NEWKEYS before the SERVICE_REQUEST.
pub fn server_accept_service(io: &mut PacketIo) -> Result<(), SshError> {
    let msg = loop {
        let pkt = io.recv_packet().map_err(|_| SshError::Io)?;
        if pkt.is_empty() {
            return Err(SshError::Protocol(
                "empty packet waiting for SERVICE_REQUEST",
            ));
        }
        // Skip informational messages that arrive before SERVICE_REQUEST
        match pkt[0] {
            2 | 4 | 7 => continue, // IGNORE, DEBUG, EXT_INFO
            _ => break pkt,
        }
    };
    if msg[0] != SSH_MSG_SERVICE_REQUEST {
        return Err(SshError::Protocol("expected SERVICE_REQUEST"));
    }

    let mut pos = 1;
    let service =
        SshBuf::get_string(&msg, &mut pos).ok_or(SshError::Protocol("bad SERVICE_REQUEST"))?;

    if service != b"ssh-userauth" {
        return Err(SshError::Protocol("unknown service requested"));
    }

    let mut reply = Vec::with_capacity(20);
    reply.push(SSH_MSG_SERVICE_ACCEPT);
    SshBuf::put_string(&mut reply, b"ssh-userauth");
    io.send_packet(&reply).map_err(|_| SshError::Io)?;

    Ok(())
}

/// What the server answers one USERAUTH_REQUEST with.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// USERAUTH_FAILURE: the method, account, key or signature is refused.
    Failure,
    /// USERAUTH_PK_OK for a key query (no signature) with an authorized key.
    PublicKeyOk { algorithm: Vec<u8>, key_blob: Vec<u8> },
    /// USERAUTH_SUCCESS: the authenticated user name.
    Success(String),
}

/// Decide one USERAUTH_REQUEST payload (`msg[0]` is the message number).
///
/// Only the publickey method can succeed: a key query is answered with PK_OK
/// only for a key the account authorizes, and a signed request succeeds only
/// when the key is authorized and the signature over the RFC 4252 section 7
/// data verifies. Each other method, password included, is a failure.
pub fn decide_request(msg: &[u8], session_id: &[u8], authorizer: &mut dyn Authorizer) -> Result<Decision, SshError> {
    let mut pos = 1;
    let username = SshBuf::get_string(msg, &mut pos).ok_or(SshError::Protocol("bad username"))?;
    let service = SshBuf::get_string(msg, &mut pos).ok_or(SshError::Protocol("bad service"))?;
    let method = SshBuf::get_string(msg, &mut pos).ok_or(SshError::Protocol("bad method"))?;

    if service != b"ssh-connection" {
        return Err(SshError::Protocol("unsupported service"));
    }

    let Ok(username_str) = core::str::from_utf8(username) else {
        return Ok(Decision::Failure);
    };

    if method != b"publickey" {
        println!(
            "bsshd: refused method '{}' for '{}'",
            String::from_utf8_lossy(method),
            username_str
        );
        return Ok(Decision::Failure);
    }

    let has_signature = SshBuf::get_bool(msg, &mut pos)
        .ok_or(SshError::Protocol("bad publickey has_signature"))?;
    let algo = SshBuf::get_string(msg, &mut pos)
        .ok_or(SshError::Protocol("bad publickey algorithm"))?;
    let key_blob = SshBuf::get_string(msg, &mut pos)
        .ok_or(SshError::Protocol("bad publickey blob"))?;

    if !algorithm_fits_key(algo, key_blob) || !authorizer.authorized(username_str, key_blob) {
        println!(
            "bsshd: key refused for '{}' algo={} fingerprint={}",
            username_str,
            String::from_utf8_lossy(algo),
            keys::fingerprint_text(key_blob)
        );
        return Ok(Decision::Failure);
    }

    if !has_signature {
        return Ok(Decision::PublicKeyOk { algorithm: algo.to_vec(), key_blob: key_blob.to_vec() });
    }

    let signature = SshBuf::get_string(msg, &mut pos)
        .ok_or(SshError::Protocol("bad publickey signature"))?;

    // The data that was signed (RFC 4252 section 7):
    //   string    session_id
    //   byte      SSH_MSG_USERAUTH_REQUEST (50)
    //   string    user name
    //   string    "ssh-connection"
    //   string    "publickey"
    //   boolean   TRUE
    //   string    algorithm name
    //   string    public key blob
    let signed_data = publickey_signed_data(session_id, username, algo, key_blob);

    let mut signature_pos = 0;
    let signature_algo = SshBuf::get_string(signature, &mut signature_pos);
    if pos == msg.len()
        && signature_algo == Some(algo)
        && keys::verify_signature(algo, key_blob, signature, &signed_data)
    {
        return Ok(Decision::Success(username_str.to_string()));
    }

    println!("bsshd: signature refused for '{}'", username_str);
    Ok(Decision::Failure)
}

/// The data a publickey request's signature covers (RFC 4252 section 7).
pub fn publickey_signed_data(session_id: &[u8], username: &[u8], algo: &[u8], key_blob: &[u8]) -> Vec<u8> {
    let mut signed_data = Vec::with_capacity(128 + session_id.len() + key_blob.len());
    SshBuf::put_string(&mut signed_data, session_id);
    signed_data.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut signed_data, username);
    SshBuf::put_string(&mut signed_data, b"ssh-connection");
    SshBuf::put_string(&mut signed_data, b"publickey");
    SshBuf::put_bool(&mut signed_data, true);
    SshBuf::put_string(&mut signed_data, algo);
    SshBuf::put_string(&mut signed_data, key_blob);
    signed_data
}

/// Handle authentication (server side): answer each USERAUTH_REQUEST as
/// `decide_request` decides, until one succeeds. Returns the user name.
pub fn server_authenticate(
    io: &mut PacketIo,
    session_id: &[u8],
    authorizer: &mut dyn Authorizer,
) -> Result<String, SshError> {
    let mut requests = 0usize;
    loop {
        let msg = io.recv_packet().map_err(|_| SshError::Io)?;
        if msg.is_empty() {
            return Err(SshError::Protocol("empty auth message"));
        }

        if msg[0] != SSH_MSG_USERAUTH_REQUEST {
            continue;
        }
        requests += 1;
        if requests > MAX_AUTH_REQUESTS {
            return Err(SshError::Auth);
        }

        match decide_request(&msg, session_id, authorizer)? {
            Decision::Failure => send_auth_failure(io, false)?,
            Decision::PublicKeyOk { algorithm, key_blob } => {
                let mut pk_ok = Vec::with_capacity(4 + algorithm.len() + 4 + key_blob.len() + 1);
                pk_ok.push(SSH_MSG_USERAUTH_PK_OK);
                SshBuf::put_string(&mut pk_ok, &algorithm);
                SshBuf::put_string(&mut pk_ok, &key_blob);
                io.send_packet(&pk_ok).map_err(|_| SshError::Io)?;
            }
            Decision::Success(user) => {
                io.send_packet(&[SSH_MSG_USERAUTH_SUCCESS])
                    .map_err(|_| SshError::Io)?;
                return Ok(user);
            }
        }
    }
}

/// Send USERAUTH_FAILURE with the list of supported methods.
fn send_auth_failure(io: &mut PacketIo, partial_success: bool) -> Result<(), SshError> {
    let mut failure = Vec::with_capacity(32);
    failure.push(SSH_MSG_USERAUTH_FAILURE);
    SshBuf::put_string(&mut failure, b"publickey");
    SshBuf::put_bool(&mut failure, partial_success);
    io.send_packet(&failure).map_err(|_| SshError::Io)?;
    Ok(())
}

// ==========================================================================
// Client-side authentication
// ==========================================================================

/// Request the ssh-userauth service (client side).
pub fn client_request_service(io: &mut PacketIo) -> Result<(), SshError> {
    let mut req = Vec::with_capacity(20);
    req.push(SSH_MSG_SERVICE_REQUEST);
    SshBuf::put_string(&mut req, b"ssh-userauth");
    io.send_packet(&req).map_err(|_| SshError::Io)?;

    let reply = recv_client_reply(io)?;
    if reply.is_empty() || reply[0] != SSH_MSG_SERVICE_ACCEPT {
        return Err(SshError::Protocol("service request rejected"));
    }

    Ok(())
}

/// Authenticate with a password (client side).
pub fn client_auth_password(
    io: &mut PacketIo,
    username: &str,
    password: &str,
) -> Result<(), SshError> {
    println!("bssh: userauth request method=password user='{}'", username);

    let mut req = Vec::with_capacity(64);
    req.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut req, username.as_bytes());
    SshBuf::put_string(&mut req, b"ssh-connection");
    SshBuf::put_string(&mut req, b"password");
    SshBuf::put_bool(&mut req, false);
    SshBuf::put_string(&mut req, password.as_bytes());
    io.send_packet(&req).map_err(|_| SshError::Io)?;

    let reply = recv_client_reply(io)?;
    if reply.is_empty() {
        return Err(SshError::Protocol("empty auth response"));
    }

    match reply[0] {
        SSH_MSG_USERAUTH_SUCCESS => Ok(()),
        SSH_MSG_USERAUTH_FAILURE => {
            let failure = parse_auth_failure(&reply)?;
            println!(
                "bssh: userauth failure methods='{}' partial={}",
                failure.methods, failure.partial_success
            );
            if failure.method_allowed("keyboard-interactive") {
                println!("bssh: retrying auth with keyboard-interactive");
                client_auth_keyboard_interactive(io, username, password)
            } else {
                Err(SshError::Auth)
            }
        }
        _ => Err(SshError::Protocol("unexpected auth response")),
    }
}

/// Authenticate with keyboard-interactive using the supplied password for prompts.
pub fn client_auth_keyboard_interactive(
    io: &mut PacketIo,
    username: &str,
    password: &str,
) -> Result<(), SshError> {
    println!(
        "bssh: userauth request method=keyboard-interactive user='{}'",
        username
    );

    let mut req = Vec::with_capacity(96);
    req.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut req, username.as_bytes());
    SshBuf::put_string(&mut req, b"ssh-connection");
    SshBuf::put_string(&mut req, b"keyboard-interactive");
    SshBuf::put_string(&mut req, b"");
    SshBuf::put_string(&mut req, b"");
    io.send_packet(&req).map_err(|_| SshError::Io)?;

    for _ in 0..8 {
        let reply = recv_client_reply(io)?;
        if reply.is_empty() {
            return Err(SshError::Protocol("empty keyboard-interactive response"));
        }

        match reply[0] {
            SSH_MSG_USERAUTH_SUCCESS => return Ok(()),
            SSH_MSG_USERAUTH_FAILURE => {
                let failure = parse_auth_failure(&reply)?;
                println!(
                    "bssh: userauth failure methods='{}' partial={}",
                    failure.methods, failure.partial_success
                );
                return Err(SshError::Auth);
            }
            SSH_MSG_USERAUTH_INFO_REQUEST => {
                let prompt_count = parse_keyboard_interactive_request(&reply)?;
                println!(
                    "bssh: keyboard-interactive info-request prompts={}",
                    prompt_count
                );

                let mut response = Vec::with_capacity(16 + password.len() * prompt_count);
                response.push(SSH_MSG_USERAUTH_INFO_RESPONSE);
                SshBuf::put_u32(&mut response, prompt_count as u32);
                for _ in 0..prompt_count {
                    SshBuf::put_string(&mut response, password.as_bytes());
                }
                io.send_packet(&response).map_err(|_| SshError::Io)?;
            }
            _ => {
                return Err(SshError::Protocol(
                    "unexpected keyboard-interactive response",
                ))
            }
        }
    }

    Err(SshError::Protocol("too many keyboard-interactive rounds"))
}

/// Authenticate with the SSH publickey method using `identity`.
///
/// With `wrong_signer`, the request still names the identity's public key but
/// is signed by a fresh random Ed25519 key, so a refusal comes from signature
/// verification rather than from the key lookup.
pub fn client_auth_publickey(
    io: &mut PacketIo,
    username: &str,
    session_id: &[u8],
    identity: &keys::Identity,
    wrong_signer: bool,
) -> Result<(), SshError> {
    let key_blob = identity.public_key_blob();
    let algo = identity.algorithm();
    if !wrong_signer {
        return client_auth_publickey_with_signer(io, username, session_id, algo, key_blob, |data| {
            identity.sign(data)
        });
    }
    let mut random = crate::crypto::rand::Csprng::new();
    let wrong_identity = keys::Ed25519Key::generate(|seed| {
        random.fill(seed);
        Ok(())
    })
    .map_err(SshError::Protocol)?;
    client_auth_publickey_with_signer(io, username, session_id, algo, key_blob, |data| {
        let signature = wrong_identity.sign(data);
        if algo == keys::ED25519 {
            signature
        } else {
            // Keep the request's algorithm name, so the server reaches verification.
            let mut pos = 0;
            let _ = SshBuf::get_string(&signature, &mut pos);
            let raw = SshBuf::get_string(&signature, &mut pos).unwrap_or(&[]);
            let mut blob = Vec::new();
            SshBuf::put_string(&mut blob, algo.as_bytes());
            SshBuf::put_string(&mut blob, raw);
            blob
        }
    })
}

fn client_auth_publickey_with_signer<F>(
    io: &mut PacketIo,
    username: &str,
    session_id: &[u8],
    algo: &str,
    key_blob: Vec<u8>,
    sign: F,
) -> Result<(), SshError>
where
    F: FnOnce(&[u8]) -> Vec<u8>,
{
    let algo = algo.as_bytes();
    println!(
        "bssh: userauth request method=publickey user='{}' algo={} has_signature=false key_blob_len={}",
        username,
        String::from_utf8_lossy(algo),
        key_blob.len()
    );

    let mut query = Vec::with_capacity(128 + key_blob.len());
    query.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut query, username.as_bytes());
    SshBuf::put_string(&mut query, b"ssh-connection");
    SshBuf::put_string(&mut query, b"publickey");
    SshBuf::put_bool(&mut query, false);
    SshBuf::put_string(&mut query, algo);
    SshBuf::put_string(&mut query, &key_blob);
    io.send_packet(&query).map_err(|_| SshError::Io)?;

    let reply = recv_client_reply(io)?;
    if reply.is_empty() {
        return Err(SshError::Protocol("empty publickey query response"));
    }
    match reply[0] {
        SSH_MSG_USERAUTH_PK_OK => {}
        SSH_MSG_USERAUTH_FAILURE => {
            let failure = parse_auth_failure(&reply)?;
            println!(
                "bssh: userauth failure methods='{}' partial={}",
                failure.methods, failure.partial_success
            );
            return Err(SshError::Auth);
        }
        _ => return Err(SshError::Protocol("unexpected publickey query response")),
    }

    let mut pos = 1;
    let accepted_algo = SshBuf::get_string(&reply, &mut pos)
        .ok_or(SshError::Protocol("bad publickey pk_ok algorithm"))?;
    let accepted_key = SshBuf::get_string(&reply, &mut pos)
        .ok_or(SshError::Protocol("bad publickey pk_ok key"))?;
    if accepted_algo != algo || accepted_key != key_blob.as_slice() {
        return Err(SshError::Protocol("publickey pk_ok mismatch"));
    }

    let signed_data = publickey_signed_data(session_id, username.as_bytes(), algo, &key_blob);
    let signature = sign(&signed_data);

    println!(
        "bssh: userauth request method=publickey user='{}' algo={} has_signature=true signature_len={}",
        username,
        String::from_utf8_lossy(algo),
        signature.len()
    );

    let mut req = Vec::with_capacity(128 + key_blob.len() + signature.len());
    req.push(SSH_MSG_USERAUTH_REQUEST);
    SshBuf::put_string(&mut req, username.as_bytes());
    SshBuf::put_string(&mut req, b"ssh-connection");
    SshBuf::put_string(&mut req, b"publickey");
    SshBuf::put_bool(&mut req, true);
    SshBuf::put_string(&mut req, algo);
    SshBuf::put_string(&mut req, &key_blob);
    SshBuf::put_string(&mut req, &signature);
    io.send_packet(&req).map_err(|_| SshError::Io)?;

    let reply = recv_client_reply(io)?;
    if reply.is_empty() {
        return Err(SshError::Protocol("empty publickey auth response"));
    }

    match reply[0] {
        SSH_MSG_USERAUTH_SUCCESS => Ok(()),
        SSH_MSG_USERAUTH_FAILURE => {
            let failure = parse_auth_failure(&reply)?;
            println!(
                "bssh: userauth failure methods='{}' partial={}",
                failure.methods, failure.partial_success
            );
            Err(SshError::Auth)
        }
        _ => Err(SshError::Protocol("unexpected publickey auth response")),
    }
}

struct AuthFailure {
    methods: String,
    partial_success: bool,
}

impl AuthFailure {
    fn method_allowed(&self, method: &str) -> bool {
        self.methods.split(',').any(|candidate| candidate == method)
    }
}

fn parse_auth_failure(reply: &[u8]) -> Result<AuthFailure, SshError> {
    let mut pos = 1;
    let methods = SshBuf::get_string(reply, &mut pos)
        .ok_or(SshError::Protocol("bad userauth failure methods"))?;
    let partial_success = SshBuf::get_bool(reply, &mut pos)
        .ok_or(SshError::Protocol("bad userauth failure partial flag"))?;
    Ok(AuthFailure {
        methods: String::from_utf8_lossy(methods).into_owned(),
        partial_success,
    })
}

fn parse_keyboard_interactive_request(reply: &[u8]) -> Result<usize, SshError> {
    let mut pos = 1;
    let _name = SshBuf::get_string(reply, &mut pos)
        .ok_or(SshError::Protocol("bad keyboard-interactive name"))?;
    let _instruction = SshBuf::get_string(reply, &mut pos)
        .ok_or(SshError::Protocol("bad keyboard-interactive instruction"))?;
    let _language = SshBuf::get_string(reply, &mut pos)
        .ok_or(SshError::Protocol("bad keyboard-interactive language"))?;
    let prompt_count = SshBuf::get_u32(reply, &mut pos)
        .ok_or(SshError::Protocol("bad keyboard-interactive prompt count"))?
        as usize;

    for _ in 0..prompt_count {
        let _prompt = SshBuf::get_string(reply, &mut pos)
            .ok_or(SshError::Protocol("bad keyboard-interactive prompt"))?;
        let _echo = SshBuf::get_bool(reply, &mut pos)
            .ok_or(SshError::Protocol("bad keyboard-interactive echo flag"))?;
    }

    Ok(prompt_count)
}

fn recv_client_reply(io: &mut PacketIo) -> Result<Vec<u8>, SshError> {
    loop {
        let reply = io.recv_packet().map_err(|_| SshError::Io)?;
        if reply.is_empty() {
            return Ok(reply);
        }
        match reply[0] {
            SSH_MSG_IGNORE | SSH_MSG_DEBUG | SSH_MSG_EXT_INFO | SSH_MSG_USERAUTH_BANNER => continue,
            _ => return Ok(reply),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::keys::{Ed25519Key, Identity};

    const SESSION: &[u8] = b"exchange hash of this session";

    /// Authorizes exactly one account and key.
    struct One {
        user: &'static str,
        key: Vec<u8>,
        asked: usize,
    }

    impl Authorizer for One {
        fn authorized(&mut self, user: &str, key_blob: &[u8]) -> bool {
            self.asked += 1;
            user == self.user && key_blob == self.key.as_slice()
        }
    }

    fn header(user: &[u8], method: &[u8]) -> Vec<u8> {
        let mut msg = vec![SSH_MSG_USERAUTH_REQUEST];
        SshBuf::put_string(&mut msg, user);
        SshBuf::put_string(&mut msg, b"ssh-connection");
        SshBuf::put_string(&mut msg, method);
        msg
    }

    fn password(user: &[u8], password: &[u8]) -> Vec<u8> {
        let mut msg = header(user, b"password");
        SshBuf::put_bool(&mut msg, false);
        SshBuf::put_string(&mut msg, password);
        msg
    }

    /// A signed publickey request for `blob`, signed by `signer`.
    fn signed(user: &[u8], identity: &Identity, blob: &[u8], signer: &Identity) -> Vec<u8> {
        let algo = identity.algorithm().as_bytes();
        let mut msg = header(user, b"publickey");
        SshBuf::put_bool(&mut msg, true);
        SshBuf::put_string(&mut msg, algo);
        SshBuf::put_string(&mut msg, blob);
        SshBuf::put_string(&mut msg, &signer.sign(&publickey_signed_data(SESSION, user, algo, blob)));
        msg
    }

    fn setup() -> (Identity, One) {
        let identity = Identity::Ed25519(Ed25519Key::from_seed(&[11; 32]));
        let authorizer = One { user: "operator", key: identity.public_key_blob(), asked: 0 };
        (identity, authorizer)
    }

    #[test]
    fn authorized_key_with_a_valid_signature_succeeds() {
        let (identity, mut authorizer) = setup();
        let blob = identity.public_key_blob();
        let msg = signed(b"operator", &identity, &blob, &identity);
        assert_eq!(decide_request(&msg, SESSION, &mut authorizer).unwrap(), Decision::Success("operator".into()));
        let mut query = header(b"operator", b"publickey");
        SshBuf::put_bool(&mut query, false);
        SshBuf::put_string(&mut query, b"ssh-ed25519");
        SshBuf::put_string(&mut query, &blob);
        assert!(matches!(decide_request(&query, SESSION, &mut authorizer).unwrap(), Decision::PublicKeyOk { .. }));
    }

    #[test]
    fn password_attempts_are_refused_whatever_the_password() {
        let (_, mut authorizer) = setup();
        for (user, secret) in [(&b"root"[..], &b"breenix"[..]), (b"operator", b"breenix"), (b"operator", b""), (b"nobody", b"x")] {
            assert_eq!(decide_request(&password(user, secret), SESSION, &mut authorizer).unwrap(), Decision::Failure);
        }
        for method in [&b"none"[..], b"keyboard-interactive", b"hostbased"] {
            assert_eq!(decide_request(&header(b"operator", method), SESSION, &mut authorizer).unwrap(), Decision::Failure);
        }
        assert_eq!(authorizer.asked, 0, "only publickey requests reach the authorizer");
    }

    #[test]
    fn wrong_key_unknown_user_and_bad_signatures_are_refused() {
        let (identity, mut authorizer) = setup();
        let blob = identity.public_key_blob();
        let other = Identity::Ed25519(Ed25519Key::from_seed(&[12; 32]));
        // A key the account does not list, even when correctly signed.
        let msg = signed(b"operator", &other, &other.public_key_blob(), &other);
        assert_eq!(decide_request(&msg, SESSION, &mut authorizer).unwrap(), Decision::Failure);
        // The listed key, for a user that is not the account.
        let msg = signed(b"admin", &identity, &blob, &identity);
        assert_eq!(decide_request(&msg, SESSION, &mut authorizer).unwrap(), Decision::Failure);
        // The listed key, signed by another key.
        let msg = signed(b"operator", &identity, &blob, &other);
        assert_eq!(decide_request(&msg, SESSION, &mut authorizer).unwrap(), Decision::Failure);
        // A signature over another session.
        let mut msg = header(b"operator", b"publickey");
        SshBuf::put_bool(&mut msg, true);
        SshBuf::put_string(&mut msg, b"ssh-ed25519");
        SshBuf::put_string(&mut msg, &blob);
        SshBuf::put_string(&mut msg, &identity.sign(&publickey_signed_data(b"another session", b"operator", b"ssh-ed25519", &blob)));
        assert_eq!(decide_request(&msg, SESSION, &mut authorizer).unwrap(), Decision::Failure);
        // An algorithm that does not fit the key.
        let mut msg = header(b"operator", b"publickey");
        SshBuf::put_bool(&mut msg, false);
        SshBuf::put_string(&mut msg, b"rsa-sha2-256");
        SshBuf::put_string(&mut msg, &blob);
        assert_eq!(decide_request(&msg, SESSION, &mut authorizer).unwrap(), Decision::Failure);
    }
}
