use libsmb::auth::ntlm::{ntlmv2_response, ntowfv2};

#[test]
fn microsoft_ntlmv2_worked_example() {
    let key = ntowfv2("Password", "User", "Domain");
    assert_eq!(
        key,
        [
            0x0c, 0x86, 0x8a, 0x40, 0x3b, 0xfd, 0x7a, 0x93, 0xa3, 0x00, 0x1e, 0xf2, 0x2e, 0xf0,
            0x2e, 0x3f
        ]
    );
    let target = [
        2, 0, 12, 0, 68, 0, 111, 0, 109, 0, 97, 0, 105, 0, 110, 0, 1, 0, 12, 0, 83, 0, 101, 0, 114,
        0, 118, 0, 101, 0, 114, 0, 0, 0, 0, 0,
    ];
    let (response, session_key) = ntlmv2_response(
        &key,
        [1, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
        [0xaa; 8],
        0,
        &target,
    );
    assert_eq!(
        &response[..16],
        &[
            0x68, 0xcd, 0x0a, 0xb8, 0x51, 0xe5, 0x1c, 0x96, 0xaa, 0xbc, 0x92, 0x7b, 0xeb, 0xef,
            0x6a, 0x1c
        ]
    );
    assert_eq!(
        session_key,
        [
            0x8d, 0xe4, 0x0c, 0xca, 0xdb, 0xc1, 0x4a, 0x82, 0xf1, 0x5c, 0xb0, 0xad, 0x0d, 0xe9,
            0x5c, 0xa3
        ]
    );
}

#[test]
fn timestamp_authentication_has_mic_and_redacted_credentials() {
    use libbreenix::crypto::hmac::hmac_md5;
    use libsmb::auth::ntlm::{authenticate, decode_av_pairs, negotiate, Credentials};
    let creds = Credentials {
        user: "User",
        domain: "Domain",
        password: "secret password",
        workstation: "PC",
    };
    assert!(!format!("{creds:?}").contains(creds.password));
    let negotiation = negotiate();
    assert_eq!(
        u32::from_le_bytes(negotiation[12..16].try_into().unwrap()) & 0x4000_0000,
        0
    );
    let mut challenge = vec![0; 48];
    challenge[..12].copy_from_slice(b"NTLMSSP\0\x02\0\0\0");
    challenge[20..24].copy_from_slice(&0x0088_0201u32.to_le_bytes());
    challenge[24..32].copy_from_slice(&[1; 8]);
    let target = [7, 0, 8, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0];
    challenge[40..42].copy_from_slice(&(target.len() as u16).to_le_bytes());
    challenge[42..44].copy_from_slice(&(target.len() as u16).to_le_bytes());
    challenge[44..48].copy_from_slice(&48u32.to_le_bytes());
    challenge.extend_from_slice(&target);
    let (mut message, key) = authenticate(&creds, &negotiation, &challenge, [2; 8], 0).unwrap();
    let mic = message[64..80].to_vec();
    message[64..80].fill(0);
    let mut transcript = negotiation;
    transcript.extend_from_slice(&challenge);
    transcript.extend_from_slice(&message);
    assert_eq!(mic, hmac_md5(&key, &transcript));
    let nt_offset = u32::from_le_bytes(message[24..28].try_into().unwrap()) as usize;
    let nt_len = u16::from_le_bytes(message[20..22].try_into().unwrap()) as usize;
    let nt = &message[nt_offset..nt_offset + nt_len];
    assert_eq!(&nt[24..32], &target[4..12]);
    let pairs = decode_av_pairs(&nt[44..nt.len() - 4]).unwrap();
    assert_eq!(
        pairs.iter().find(|p| p.id == 6).unwrap().value,
        2u32.to_le_bytes()
    );
}
