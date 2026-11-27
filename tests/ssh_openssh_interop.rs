//! A stock OpenSSH client (/usr/bin/ssh) against a minimal server built on
//! this crate's packet Framer, cipher negotiation, key derivation and strict
//! KEX, over loopback TCP. For each cipher the client pipes 200 KiB through
//! an exec channel that echoes it back, so both directions carry bulk data
//! in full-size packets. Host-key checking stays strict, against a known
//! hosts file holding the test's own key.

use libbreenix::crypto::{rand::Csprng, sha256::sha256, x25519};
use libbreenix::ssh::cipher::CipherKind;
use libbreenix::ssh::keys::Ed25519Key;
use libbreenix::ssh::packet::Framer;
use libbreenix::ssh::{kex, SshBuf};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::Duration;

const SERVER_VERSION: &str = "SSH-2.0-BreenixInterop_1.0";

fn rng(seed: u8) -> Csprng {
    Csprng::from_entropy(move |buf| {
        buf.fill(seed);
        Ok(buf.len())
    })
}

struct Wire {
    stream: TcpStream,
    framer: Framer,
    out: Vec<u8>,
}

impl Wire {
    fn send(&mut self, payload: &[u8]) {
        self.out.clear();
        self.framer.seal(payload, &mut self.out).unwrap();
        self.stream.write_all(&self.out).unwrap();
    }

    fn recv(&mut self) -> Vec<u8> {
        loop {
            if let Some(payload) = self.framer.open().unwrap() {
                return payload;
            }
            let n = self.stream.read(self.framer.receive_buffer()).unwrap();
            assert!(n > 0, "client closed the connection");
            self.framer.received(n);
        }
    }
}

fn u32_at(message: &[u8], pos: &mut usize) -> u32 {
    SshBuf::get_u32(message, pos).unwrap()
}

fn string_at<'a>(message: &'a [u8], pos: &mut usize) -> &'a [u8] {
    SshBuf::get_string(message, pos).unwrap()
}

/// What the server saw: the negotiated ciphers, whether strict KEX applied,
/// and the exec command.
struct Transcript {
    kinds: (CipherKind, CipherKind),
    strict: bool,
    command: Vec<u8>,
}

fn serve(stream: TcpStream, host_key: &Ed25519Key) -> Transcript {
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut wire = Wire { stream, framer: Framer::new(rng(9)), out: Vec::new() };
    wire.stream.write_all(format!("{SERVER_VERSION}\r\n").as_bytes()).unwrap();
    let mut client_version = Vec::new();
    let mut byte = [0u8];
    while !client_version.ends_with(b"\r\n") {
        wire.stream.read_exact(&mut byte).unwrap();
        client_version.push(byte[0]);
    }
    client_version.truncate(client_version.len() - 2);

    let server_kexinit = kex::build_kexinit(&mut rng(5));
    wire.send(&server_kexinit);
    let client_kexinit = wire.recv();
    assert_eq!(client_kexinit[0], 20, "KEXINIT first");
    let init = wire.recv();
    assert_eq!(init[0], 30, "KEX_ECDH_INIT");
    let client_public: [u8; 32] = string_at(&init, &mut 1).try_into().unwrap();

    let secret = [0x77u8; 32];
    let server_public = x25519::x25519_basepoint(&secret);
    let shared = x25519::x25519(&secret, &client_public);
    let host_blob = host_key.public_key_blob();
    let mut hashed = Vec::new();
    SshBuf::put_string(&mut hashed, &client_version);
    SshBuf::put_string(&mut hashed, SERVER_VERSION.as_bytes());
    SshBuf::put_string(&mut hashed, &client_kexinit);
    SshBuf::put_string(&mut hashed, &server_kexinit);
    SshBuf::put_string(&mut hashed, &host_blob);
    SshBuf::put_string(&mut hashed, &client_public);
    SshBuf::put_string(&mut hashed, &server_public);
    SshBuf::put_mpint(&mut hashed, &shared);
    let exchange_hash = sha256(&hashed);

    let mut reply = vec![31];
    SshBuf::put_string(&mut reply, &host_blob);
    SshBuf::put_string(&mut reply, &server_public);
    SshBuf::put_string(&mut reply, &host_key.sign(&exchange_hash));
    wire.send(&reply);
    wire.send(&[21]);
    assert_eq!(wire.recv(), [21], "NEWKEYS");

    let kinds = kex::negotiate_ciphers(&client_kexinit, &server_kexinit).unwrap();
    let (c2s, s2c) = kex::derive_keys(&shared, &exchange_hash, &exchange_hash, false, kinds.0, kinds.1);
    wire.framer.set_cipher_recv(c2s);
    wire.framer.set_cipher_send(s2c);
    let strict = kex::strict_kex(&client_kexinit, &server_kexinit);
    if strict {
        wire.framer.reset_sequence_numbers();
    }

    // The client answers the server's ext-info-s with its own EXT_INFO.
    let mut request = wire.recv();
    while request[0] == 7 {
        request = wire.recv();
    }
    assert_eq!(request[0], 5, "SERVICE_REQUEST");
    let mut accept = vec![6];
    SshBuf::put_string(&mut accept, string_at(&request, &mut 1));
    wire.send(&accept);
    assert_eq!(wire.recv()[0], 50, "USERAUTH_REQUEST");
    wire.send(&[52]);

    let mut client_channel = None;
    let mut client_max_packet = 0usize;
    let mut command = Vec::new();
    let mut input = Vec::new();
    loop {
        let message = wire.recv();
        let mut pos = 1;
        match message[0] {
            80 => {
                string_at(&message, &mut pos);
                if SshBuf::get_bool(&message, &mut pos).unwrap() {
                    wire.send(&[82]);
                }
            }
            90 => {
                assert_eq!(string_at(&message, &mut pos), b"session");
                let sender = u32_at(&message, &mut pos);
                let _window = u32_at(&message, &mut pos);
                client_max_packet = u32_at(&message, &mut pos) as usize;
                client_channel = Some(sender);
                let mut confirm = vec![91];
                for value in [sender, 0, 4 << 20, 32 * 1024] {
                    SshBuf::put_u32(&mut confirm, value);
                }
                wire.send(&confirm);
            }
            98 => {
                u32_at(&message, &mut pos);
                let name = string_at(&message, &mut pos).to_vec();
                let want_reply = SshBuf::get_bool(&message, &mut pos).unwrap();
                let exec = name == b"exec";
                if exec {
                    command = string_at(&message, &mut pos).to_vec();
                }
                if want_reply {
                    let mut answer = vec![if exec { 99 } else { 100 }];
                    SshBuf::put_u32(&mut answer, client_channel.unwrap());
                    wire.send(&answer);
                }
            }
            94 => {
                u32_at(&message, &mut pos);
                input.extend_from_slice(string_at(&message, &mut pos));
            }
            96 => break,
            2 | 4 | 93 => {}
            kind => panic!("unexpected message {kind}"),
        }
    }

    let channel = client_channel.unwrap();
    for chunk in input.chunks(client_max_packet.min(32 * 1024)) {
        let mut data = vec![94];
        SshBuf::put_u32(&mut data, channel);
        SshBuf::put_string(&mut data, chunk);
        wire.send(&data);
    }
    let mut status = vec![98];
    SshBuf::put_u32(&mut status, channel);
    SshBuf::put_string(&mut status, b"exit-status");
    SshBuf::put_bool(&mut status, false);
    SshBuf::put_u32(&mut status, 0);
    wire.send(&status);
    for kind in [96, 97] {
        let mut message = vec![kind];
        SshBuf::put_u32(&mut message, channel);
        wire.send(&message);
    }
    loop {
        let message = wire.recv();
        if message[0] == 97 {
            break;
        }
    }
    Transcript { kinds, strict, command }
}

fn exchange(cipher: CipherKind) {
    let host_key = Ed25519Key::from_seed(&[0x42; 32]);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let known = std::env::temp_dir().join(format!("bssh-interop-{}-{port}", std::process::id()));
    std::fs::write(&known, format!("[127.0.0.1]:{port} {}\n", host_key.public_line("interop"))).unwrap();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        serve(stream, &host_key)
    });
    let input: Vec<u8> = (0..200 * 1024).map(|i: usize| (i * 31 % 251) as u8).collect();
    let mut options = vec![
        "-F".into(), "/dev/null".into(),
        "-p".into(), port.to_string(),
        "-o".into(), format!("UserKnownHostsFile={}", known.display()),
        "-o".into(), "StrictHostKeyChecking=yes".into(),
        "-o".into(), "BatchMode=yes".into(),
        "-o".into(), "IdentityAgent=none".into(),
        "-o".into(), "PubkeyAuthentication=no".into(),
        "-o".into(), "KexAlgorithms=curve25519-sha256".into(),
        "-o".into(), "HostKeyAlgorithms=ssh-ed25519".into(),
        "-o".into(), format!("Ciphers={}", cipher.name()),
    ];
    if !cipher.is_aead() {
        options.extend(["-o".into(), "MACs=hmac-sha2-256".into()]);
    }
    let mut client = Command::new("/usr/bin/ssh")
        .args(&options)
        .args(["-T", "interop@127.0.0.1", "echo-back"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = client.stdin.take().unwrap();
    let sent = input.clone();
    let writer = std::thread::spawn(move || stdin.write_all(&sent));
    let output = client.wait_with_output().unwrap();
    writer.join().unwrap().unwrap();
    let transcript = server.join().unwrap();
    let _ = std::fs::remove_file(&known);

    assert!(output.status.success(), "{cipher:?}: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, input, "{cipher:?}: echoed bytes");
    assert_eq!(transcript.kinds, (cipher, cipher));
    assert!(transcript.strict, "OpenSSH offers strict KEX and the server accepted it");
    assert_eq!(transcript.command, b"echo-back");
}

#[test]
fn openssh_round_trips_bulk_data_with_chacha20_poly1305() {
    exchange(CipherKind::ChaCha20Poly1305);
}

#[test]
fn openssh_round_trips_bulk_data_with_aes256_gcm() {
    exchange(CipherKind::Aes256Gcm);
}

#[test]
fn openssh_round_trips_bulk_data_with_aes128_ctr() {
    exchange(CipherKind::Aes128Ctr);
}
