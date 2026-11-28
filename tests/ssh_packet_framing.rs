//! SSH binary packet framing (RFC 4253 section 6) without a socket: each
//! sealed packet is one contiguous frame, one receive can hold several
//! packets, and chacha20-poly1305@openssh.com matches the worked example of
//! draft-ietf-sshm-chacha20-poly1305 appendix A.

use libbreenix::crypto::rand::Csprng;
use libbreenix::error::Error;
use libbreenix::ssh::cipher::{CipherKind, SshCipher};
use libbreenix::ssh::kex;
use libbreenix::ssh::keys::Ed25519Key;
use libbreenix::ssh::packet::{Framer, KexTransport};
use libbreenix::Errno;

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn rng(seed: u8) -> Csprng {
    Csprng::from_entropy(move |buf| {
        buf.fill(seed);
        Ok(buf.len())
    })
}

const DRAFT_KEY: &str = "8bbff6855fc102338c373e73aac0c914f076a905b2444a32eecaffeae22becc5
                         e9b7a7a5825a8249346ec1c28301cf394543fc7569887d76e168f37562ac0740";
const DRAFT_PLAIN: &str = "00000048065e00000000000000384c6f72656d20697073756d20646f6c6f7220
                           73697420616d65742c20636f6e7365637465747572206164697069736963696e
                           6720656c69744e43e804dc6c";
const DRAFT_WIRE: &str = "2c3ecce4a5bc05895bf07a7ba956b6c68829ac7c83b780b7000ecde745afc705
                          bbc378ce03a280236b87b53bed5839662302b164b6286a48cd1e097138e3cb90
                          9b8b2b829dd18d2a35ff82d995349e855bf02c298ef775f2d1a7e8b8";

#[test]
fn chacha20_poly1305_matches_the_draft_worked_example() {
    let key: [u8; 64] = hex(DRAFT_KEY).try_into().unwrap();
    let plain = hex(DRAFT_PLAIN);
    let wire = hex(DRAFT_WIRE);
    assert_eq!(plain.len() + 16, wire.len());

    let mut sender = SshCipher::new(CipherKind::ChaCha20Poly1305, &key);
    let mut packet = plain.clone();
    let mut tag = [0u8; 16];
    sender.seal(7, &mut packet, &mut tag).unwrap();
    assert_eq!([packet.as_slice(), &tag].concat(), wire);

    let mut receiver = SshCipher::new(CipherKind::ChaCha20Poly1305, &key);
    let mut packet = wire[..plain.len()].to_vec();
    assert_eq!(receiver.packet_length(7, &mut packet[..4]), 0x48);
    for index in [0, 5, plain.len() - 1] {
        let mut corrupt = packet.clone();
        corrupt[index] ^= 1;
        assert!(receiver.open(7, &mut corrupt, &wire[plain.len()..]).is_err());
    }
    assert!(receiver.open(8, &mut packet.clone(), &wire[plain.len()..]).is_err());
    receiver.open(7, &mut packet, &wire[plain.len()..]).unwrap();
    assert_eq!(packet, plain);
}

fn ciphers(kind: Option<CipherKind>) -> (Option<SshCipher>, Option<SshCipher>) {
    let key: Vec<u8> = (0..96u8).map(|i| i.wrapping_mul(29) ^ 0x5a).collect();
    match kind {
        None => (None, None),
        Some(kind) => (Some(SshCipher::new(kind, &key)), Some(SshCipher::new(kind, &key))),
    }
}

fn framers(kind: Option<CipherKind>) -> (Framer, Framer) {
    let (send, recv) = ciphers(kind);
    let mut sender = Framer::new(rng(3));
    let mut receiver = Framer::new(rng(4));
    if let Some(cipher) = send {
        sender.set_cipher_send(cipher);
    }
    if let Some(cipher) = recv {
        receiver.set_cipher_recv(cipher);
    }
    (sender, receiver)
}

fn feed(framer: &mut Framer, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let space = framer.receive_buffer();
        let n = space.len().min(bytes.len());
        space[..n].copy_from_slice(&bytes[..n]);
        framer.received(n);
        bytes = &bytes[n..];
    }
}

const KINDS: [Option<CipherKind>; 4] = [
    None,
    Some(CipherKind::Aes128Ctr),
    Some(CipherKind::Aes256Gcm),
    Some(CipherKind::ChaCha20Poly1305),
];

#[test]
fn every_cipher_round_trips_packets_in_any_fragmentation() {
    let payloads: Vec<Vec<u8>> = [0usize, 1, 15, 16, 17, 1000, 32 * 1024 + 9]
        .iter()
        .map(|&len| (0..len).map(|i| (i * 7 + len) as u8).collect())
        .collect();
    for kind in KINDS {
        for chunk in [1usize, 7, usize::MAX] {
            let (mut sender, mut receiver) = framers(kind);
            let mut stream = Vec::new();
            for payload in &payloads {
                let start = stream.len();
                sender.seal(payload, &mut stream).unwrap();
                // One contiguous frame per packet: length field, body and tag.
                let frame = &stream[start..];
                assert!(frame.len() >= 4 + 1 + payload.len() + 4, "{kind:?}");
            }
            let mut received = Vec::new();
            for piece in stream.chunks(chunk.min(stream.len())) {
                feed(&mut receiver, piece);
                while let Some(payload) = receiver.open().unwrap() {
                    received.push(payload);
                }
            }
            assert_eq!(received, payloads, "{kind:?}, chunk {chunk}");
            assert!(!receiver.packet_ready());
        }
    }
}

#[test]
fn packet_ready_reports_only_complete_buffered_packets() {
    for kind in KINDS {
        let (mut sender, mut receiver) = framers(kind);
        let mut stream = Vec::new();
        sender.seal(b"first", &mut stream).unwrap();
        let first = stream.len();
        sender.seal(b"second", &mut stream).unwrap();

        feed(&mut receiver, &stream[..first - 1]);
        assert!(!receiver.packet_ready(), "{kind:?}");
        assert_eq!(receiver.open().unwrap(), None);
        // The rest of the first packet and the whole second one arrive together.
        feed(&mut receiver, &stream[first - 1..]);
        assert!(receiver.packet_ready(), "{kind:?}");
        assert_eq!(receiver.open().unwrap().as_deref(), Some(&b"first"[..]));
        assert!(receiver.packet_ready(), "{kind:?}");
        assert_eq!(receiver.open().unwrap().as_deref(), Some(&b"second"[..]));
        assert!(!receiver.packet_ready(), "{kind:?}");
    }
}

#[test]
fn tampered_frames_are_rejected() {
    for kind in &KINDS[1..] {
        let (mut sender, mut receiver) = framers(*kind);
        let mut stream = Vec::new();
        sender.seal(b"payload bytes", &mut stream).unwrap();
        let last = stream.len() - 1;
        stream[last] ^= 0x80;
        feed(&mut receiver, &stream);
        assert!(receiver.open().is_err(), "{kind:?}");
    }
}

#[test]
fn negotiation_prefers_chacha20_and_detects_strict_kex() {
    let server = kex::build_kexinit(&mut rng(1));
    let client = kex::build_client_kexinit(&mut rng(2));
    assert_eq!(
        kex::negotiate_ciphers(&client, &server).unwrap(),
        (CipherKind::ChaCha20Poly1305, CipherKind::ChaCha20Poly1305)
    );
    assert!(kex::strict_kex(&client, &server));
    // A client that does not offer kex-strict-c gets the old sequence numbering.
    let mut legacy = client.clone();
    let needle = b"kex-strict-c";
    while let Some(at) = legacy.windows(needle.len()).position(|w| w == needle) {
        legacy[at..at + needle.len()].copy_from_slice(b"kex-strict-x");
    }
    assert!(!kex::strict_kex(&legacy, &server));
}

/// A KEXINIT payload offering `kex` and `ciphers` in both directions.
fn kexinit(kex: &str, ciphers: &str) -> Vec<u8> {
    let mut payload = vec![20u8];
    payload.extend_from_slice(&[0x11; 16]);
    let lists = [kex, "ssh-ed25519", ciphers, ciphers, "hmac-sha2-256", "hmac-sha2-256", "none", "none", "", ""];
    for list in lists {
        payload.extend((list.len() as u32).to_be_bytes());
        payload.extend_from_slice(list.as_bytes());
    }
    payload.push(0);
    payload.extend(0u32.to_be_bytes());
    payload
}

#[test]
fn chacha20_is_negotiated_only_under_strict_kex() {
    // Without strict KEX an attacker can delete packets from the start of
    // the encrypted stream undetected (the Terrapin attack), so a client
    // that prefers chacha20 but lacks kex-strict-c gets the next cipher.
    let server = kex::build_kexinit(&mut rng(1));
    let chacha_first = "chacha20-poly1305@openssh.com,aes256-gcm@openssh.com";
    let legacy = kexinit("curve25519-sha256", chacha_first);
    assert!(!kex::strict_kex(&legacy, &server));
    assert_eq!(
        kex::negotiate_ciphers(&legacy, &server).unwrap(),
        (CipherKind::Aes256Gcm, CipherKind::Aes256Gcm)
    );
    let chacha_only = kexinit("curve25519-sha256", "chacha20-poly1305@openssh.com");
    assert!(kex::negotiate_ciphers(&chacha_only, &server).is_err());
    // The same client with strict KEX keeps its first choice.
    let strict = kexinit("curve25519-sha256,kex-strict-c-v00@openssh.com", chacha_first);
    assert_eq!(
        kex::negotiate_ciphers(&strict, &server).unwrap(),
        (CipherKind::ChaCha20Poly1305, CipherKind::ChaCha20Poly1305)
    );
}

#[test]
fn no_sequence_number_is_used_twice_under_one_set_of_keys() {
    for kind in KINDS[1..].iter().copied() {
        let (mut sender, mut receiver) = framers(kind);
        // As if 2^32 - 1 packets had already passed each way under these keys.
        sender.advance_sequence_numbers(u32::MAX);
        receiver.advance_sequence_numbers(u32::MAX);
        let mut stream = Vec::new();
        sender.seal(b"last", &mut stream).unwrap();
        feed(&mut receiver, &stream);
        assert_eq!(receiver.open().unwrap().as_deref(), Some(&b"last"[..]), "{kind:?}");

        // The next number would be 0 again, whose keystream and Poly1305 key
        // were already used: the sender refuses to seal it.
        let mut wrapped = Vec::new();
        assert!(sender.seal(b"wrapped", &mut wrapped).is_err(), "{kind:?}");
        assert!(wrapped.is_empty(), "{kind:?}");
        // And the receiver refuses a packet sealed at 0 under the same keys,
        // which would otherwise replay the first packet of the connection.
        let (mut first, _) = framers(kind);
        let mut replay = Vec::new();
        first.seal(b"replayed", &mut replay).unwrap();
        feed(&mut receiver, &replay);
        assert!(receiver.open().is_err(), "{kind:?}");

        // New keys start a new count.
        let (mut sender, mut receiver) = framers(kind);
        sender.advance_sequence_numbers(u32::MAX);
        sender.seal(b"last", &mut Vec::new()).unwrap();
        let (send, recv) = ciphers(kind);
        sender.set_cipher_send(send.unwrap());
        receiver.set_cipher_recv(recv.unwrap());
        let mut stream = Vec::new();
        sender.seal(b"rekeyed", &mut stream).unwrap();
        feed(&mut receiver, &stream);
        assert_eq!(receiver.open().unwrap().as_deref(), Some(&b"rekeyed"[..]), "{kind:?}");
    }
}

/// A scripted client for the production server key exchange: the packets
/// it will receive, the message types it sent, and the key installation.
struct Script {
    incoming: std::collections::VecDeque<Vec<u8>>,
    sent: Vec<u8>,
    /// Some(strict) once keys were installed.
    installed: Option<bool>,
}

impl KexTransport for Script {
    fn send_packet(&mut self, payload: &[u8]) -> Result<(), Error> {
        self.sent.push(payload[0]);
        Ok(())
    }

    fn recv_packet(&mut self) -> Result<Vec<u8>, Error> {
        self.incoming.pop_front().ok_or(Error::Os(Errno::ECONNRESET))
    }

    fn install_keys(&mut self, _recv: SshCipher, _send: SshCipher, strict: bool) {
        assert!(self.installed.is_none(), "keys installed once");
        self.installed = Some(strict);
    }
}

fn ecdh_init() -> Vec<u8> {
    let public = libbreenix::crypto::x25519::x25519_basepoint(&[0x31; 32]);
    let mut init = vec![30u8];
    init.extend(32u32.to_be_bytes());
    init.extend_from_slice(&public);
    init
}

fn server_exchange(incoming: Vec<Vec<u8>>) -> (Result<(), libbreenix::ssh::SshError>, Script) {
    let host_key = Ed25519Key::from_seed(&[0x42; 32]);
    let mut script = Script { incoming: incoming.into(), sent: Vec::new(), installed: None };
    let mut state = kex::KexState::new();
    let result = kex::server_key_exchange(
        &mut script, &mut rng(6), &host_key, &mut state, "SSH-2.0-Script_1.0", "SSH-2.0-BreenixTest_1.0",
    );
    (result, script)
}

#[test]
fn the_server_key_exchange_admits_only_key_exchange_messages() {
    let client = kex::build_client_kexinit(&mut rng(2));
    let mut legacy = client.clone();
    let needle = b"kex-strict-c";
    while let Some(at) = legacy.windows(needle.len()).position(|w| w == needle) {
        legacy[at..at + needle.len()].copy_from_slice(b"kex-strict-x");
    }
    let (newkeys, ignore) = (vec![21u8], vec![2u8, 0, 0, 0, 0]);

    // Strict KEX: keys go in with both sequence numbers restarted.
    let (result, script) = server_exchange(vec![client.clone(), ecdh_init(), newkeys.clone()]);
    result.unwrap();
    assert_eq!(script.sent, [20, 31, 21], "KEXINIT, KEX_ECDH_REPLY, NEWKEYS");
    assert_eq!(script.installed, Some(true));
    // A client without strict KEX keeps counting from its KEXINIT.
    let (result, script) = server_exchange(vec![legacy, ecdh_init(), newkeys.clone()]);
    result.unwrap();
    assert_eq!(script.installed, Some(false));

    // Anything else in the initial exchange ends it before any keys exist:
    // a first packet other than KEXINIT, a message slipped in before
    // KEX_ECDH_INIT (it would shift the sequence numbers strict KEX
    // relies on), or a packet other than NEWKEYS after the reply.
    for incoming in [
        vec![ignore.clone(), client.clone(), ecdh_init(), newkeys.clone()],
        vec![client.clone(), ignore.clone(), ecdh_init(), newkeys.clone()],
        vec![client.clone(), ecdh_init(), ignore.clone(), newkeys.clone()],
    ] {
        let (result, script) = server_exchange(incoming);
        assert!(result.is_err());
        assert_eq!(script.installed, None);
    }
}
