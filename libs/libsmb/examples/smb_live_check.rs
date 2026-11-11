//! Host integration check. Set SMB_TEST_HOST, optional SMB_TEST_PORT, and
//! SMB_TEST_ENV_FILE (path containing SMB_TEST_USER / SMB_TEST_PASSWORD and
//! optional SMB_TEST_DOMAIN). Credential values stay in the env file and are omitted from diagnostics.
//! At each AUDIT marker, the supervising process checks the server's session
//! table for the test account and sends NO_SESSIONS on stdin only if absent.
use libsmb::{
    auth::ntlm::Credentials,
    client::{Client, DirectTcp, Error},
    msg::*,
    session::{Request, Response, Session},
    DialectCap, SyncHeader,
};
use std::{
    collections::HashMap,
    env, fs,
    io::{self, ErrorKind, Write},
    time::{SystemTime, UNIX_EPOCH},
};

type Check = Result<(), String>;
fn operation<T>(result: Result<T, Error>, stage: &str) -> Result<T, String> {
    result.map_err(|e| match e {
        Error::Status(s) => format!("{stage}: NTSTATUS {:08x}", s.0),
        Error::Io(e) => format!("{stage}: transport {:?}", e.kind()),
        Error::Protocol => format!("{stage}: protocol failure"),
    })
}
fn expect(value: bool, message: &str) -> Check {
    if value {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn audit_sessions(stage: &str) -> Check {
    println!("AUDIT wrong-password {stage}");
    io::stdout().flush().map_err(|_| "audit output")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|_| "audit input")?;
    expect(
        answer.trim() == "NO_SESSIONS",
        "server session audit failed",
    )
}
fn root() -> CreateRequest<'static> {
    CreateRequest {
        name: "",
        oplock_level: 0,
        impersonation_level: 2,
        desired_access: access_mask::LIST_DIRECTORY,
        file_attributes: 0,
        share_access: 7,
        disposition: CreateDisposition::Open,
        options: create_options::DIRECTORY_FILE,
        contexts: &[],
    }
}
fn encrypted_exchange(
    tcp: &mut DirectTcp,
    session: &mut Session,
    request: Request<'_>,
) -> Result<Response, String> {
    let wire = session.build(request).map_err(|_| "encrypted build")?;
    expect(wire.starts_with(b"\xfdSMB"), "enc request is plaintext")?;
    operation(tcp.send(&wire), "encrypted send")?;
    let wire = operation(tcp.receive(), "encrypted receive")?;
    expect(wire.starts_with(b"\xfdSMB"), "enc response is plaintext")?;
    let response = session
        .consume(&wire)
        .map_err(|_| "encrypted integrity")?
        .ok_or("encrypted response incomplete")?;
    expect(response.header.status == 0, "encrypted operation failed")?;
    Ok(response)
}
// A timeout, malformed response, or unrelated status is NOT a refusal.
fn refused(result: Result<Vec<u8>, Error>) -> Check {
    match result {
        Ok(b) => {
            let h = SyncHeader::decode(&b).map_err(|_| "control: malformed response")?;
            expect(
                h.status == NtStatus::ACCESS_DENIED.0 || h.status == 0xc000_a000,
                "control: server did not refuse request",
            )
        }
        Err(Error::Io(e))
            if matches!(
                e.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
            ) =>
        {
            Ok(())
        }
        _ => Err("control: no explicit refusal or connection closure".into()),
    }
}
fn run() -> Check {
    let host = env::var("SMB_TEST_HOST").map_err(|_| "set SMB_TEST_HOST")?;
    let port = env::var("SMB_TEST_PORT")
        .unwrap_or_else(|_| "445".into())
        .parse::<u16>()
        .map_err(|_| "invalid port")?;
    let path = env::var_os("SMB_TEST_ENV_FILE").ok_or("set SMB_TEST_ENV_FILE")?;
    let file = fs::read_to_string(path).map_err(|_| "cannot read credential file")?;
    let fields: HashMap<_, _> = file
        .lines()
        .filter_map(|l| l.trim().split_once('='))
        .collect();
    let user = *fields
        .get("SMB_TEST_USER")
        .ok_or("credential file lacks user")?;
    let password = *fields
        .get("SMB_TEST_PASSWORD")
        .ok_or("credential file lacks password")?;
    let creds = Credentials {
        user,
        password,
        domain: fields.get("SMB_TEST_DOMAIN").copied().unwrap_or(""),
        workstation: "",
    };
    audit_sessions("before")?;
    let wrong = format!("{password}!wrong");
    let bad = Credentials {
        password: &wrong,
        ..creds
    };
    expect(
        matches!(
            Client::connect(&host, port, "rw", &bad, DialectCap::Default),
            Err(Error::Status(NtStatus::LOGON_FAILURE))
        ),
        "wrong password did not get LOGON_FAILURE",
    )?;
    println!("PASS wrong-password LOGON_FAILURE (0xc000006d)");
    audit_sessions("after")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "clock")?
        .as_nanos();
    let a = format!("ns3-{nonce}-{}.bin", std::process::id());
    let b = format!("ns3-{nonce}-{}-replace.bin", std::process::id());
    // Cross both 64 KiB credits and the server's usual 8 MiB transfer cap.
    let data: Vec<u8> = (0..9 * 1024 * 1024 + 17)
        .map(|i| ((i * 31 + i / 251) % 256) as u8)
        .collect();
    for (share, cap, dialect) in [
        ("rw", DialectCap::Default, 0x0311),
        ("rw302", DialectCap::Smb302, 0x0302),
        ("enc", DialectCap::Default, 0x0311),
        ("enc", DialectCap::Smb302, 0x0302),
    ] {
        let stage = format!("{share}/{dialect:04x}");
        let mut c = operation(
            Client::connect(&host, port, share, &creds, cap),
            &format!("{stage} connect"),
        )?;
        expect(c.dialect() == dialect, "unexpected negotiated dialect")?;
        operation(c.list(""), &format!("{stage} list"))?;
        operation(c.write(&a, &data), &format!("{stage} write"))?;
        expect(
            operation(c.read(&a), &format!("{stage} read"))? == data,
            "read bytes differ",
        )?;
        let opened = operation(
            c.open(
                &a,
                access_mask::READ_DATA | access_mask::WRITE_DATA,
                CreateDisposition::Open,
                create_options::NON_DIRECTORY_FILE,
            ),
            "open for offset I/O",
        )?;
        let offset = 65537;
        let patch = b"offset-check";
        operation(c.write_at(opened.file_id, offset, patch), "offset write")?;
        expect(
            operation(
                c.read_at(opened.file_id, offset, patch.len()),
                "offset read",
            )? == patch,
            "offset bytes differ",
        )?;
        let size = operation(c.query_fs_size(opened.file_id), "filesystem size")?;
        expect(
            size.total_allocation_units > 0
                && size.sectors_per_allocation_unit > 0
                && size.bytes_per_sector > 0,
            "invalid filesystem size",
        )?;
        operation(
            c.write_at(
                opened.file_id,
                offset,
                &data[offset as usize..offset as usize + patch.len()],
            ),
            "restore offset",
        )?;
        operation(c.close(opened.file_id), "close offset handle")?;
        println!("PASS {stage} offset write/read and filesystem size");
        operation(c.write(&b, b"old destination"), "write destination")?;
        operation(c.rename_replace(&a, &b), "rename-replace")?;
        expect(
            operation(c.read(&b), "read replacement")? == data,
            "replacement bytes differ",
        )?;
        let entries = operation(c.list(""), "list replacement")?;
        expect(
            !entries.iter().any(|e| e.name == a) && entries.iter().any(|e| e.name == b),
            "rename names differ",
        )?;
        operation(c.delete(&b), "delete")?;
        expect(
            !operation(c.list(""), "list deletion")?
                .iter()
                .any(|e| e.name == b),
            "deleted file remains",
        )?;
        let (mut tcp, mut session) = c.into_parts();
        let negotiated = session.negotiated().ok_or("missing negotiation")?;
        println!("NEGOTIATED {share} dialect=0x{:04x}", negotiated.dialect);
        if share == "enc" {
            let cipher = if dialect == 0x0302 {
                expect(negotiated.capabilities & 0x40 != 0, "CCM not negotiated")?;
                1
            } else {
                match negotiated.contexts.iter().find_map(|c| match c {
                    libsmb::NegotiateContext::Encryption { ciphers } => Some(ciphers.as_slice()),
                    _ => None,
                }) {
                    Some([cipher @ (1 | 2)]) => *cipher,
                    _ => return Err("missing selected encryption cipher".into()),
                }
            };
            let name = if cipher == 1 {
                "AES-128-CCM"
            } else {
                "AES-128-GCM"
            };
            println!("NEGOTIATED enc cipher=0x{cipher:04x} {name}");
            // Require actual transforms around file operations on this
            // connection, in addition to checking the negotiated cipher.
            let opened = encrypted_exchange(&mut tcp, &mut session, Request::Create(root()))?
                .decode(CreateResponse::decode)
                .map_err(|_| "encrypted create response")?;
            encrypted_exchange(
                &mut tcp,
                &mut session,
                Request::Close(CloseRequest {
                    file_id: opened.file_id,
                    postquery_attributes: false,
                }),
            )?
            .decode(CloseResponse::decode)
            .map_err(|_| "encrypted close response")?;
            println!("PASS {stage} encrypted CREATE/CLOSE request/response transform-on-wire");
        }
        println!("PASS {stage} list/write/read-exact/rename-replace/read-exact/delete");
    }
    let mut ro = operation(
        Client::connect(&host, port, "ro", &creds, DialectCap::Default),
        "ro connect",
    )?;
    expect(
        operation(ro.list(""), "ro list")?
            .iter()
            .any(|e| e.name == "README.txt"),
        "ro README absent",
    )?;
    expect(
        !operation(ro.read("README.txt"), "ro read")?.is_empty(),
        "ro README empty",
    )?;
    expect(
        matches!(
            ro.write(&a, b"refused"),
            Err(Error::Status(NtStatus::ACCESS_DENIED))
        ),
        "ro write did not get ACCESS_DENIED",
    )?;
    println!("NEGOTIATED ro dialect=0x{:04x}", ro.dialect());
    println!("PASS ro list/README.txt-read/write-ACCESS_DENIED (0xc0000022)");
    let c = operation(
        Client::connect(&host, port, "rw", &creds, DialectCap::Default),
        "signature control connect",
    )?;
    let (mut tcp, mut session) = c.into_parts();
    let mut wire = session
        .build(Request::Create(root()))
        .map_err(|_| "control build")?;
    expect(
        wire.starts_with(b"\xfeSMB") && wire[16] & 8 != 0,
        "control requires signed plaintext",
    )?;
    wire[48] ^= 1;
    operation(tcp.send(&wire), "control send")?;
    refused(tcp.receive())?;
    println!("PASS corrupted-signature refused");
    let c = operation(
        Client::connect(&host, port, "enc", &creds, DialectCap::Default),
        "encryption control connect",
    )?;
    let (mut tcp, session) = c.into_parts();
    let header = SyncHeader {
        credit_charge: 1,
        status: 0,
        command: 5,
        credits: 1,
        flags: 0,
        next_command: 0,
        message_id: session.next_message_id(),
        tree_id: session.tree_id().ok_or("no tree")?,
        session_id: session.session_id(),
        signature: [0; 16],
    };
    let raw = root().encode(&header).map_err(|_| "raw control build")?;
    operation(tcp.send(&raw), "raw control send")?;
    refused(tcp.receive())?;
    // This unsigned control checks refusal only; the transform exchange above
    // checks encryption independently of this ACCESS_DENIED result.
    println!("PASS unsigned plaintext enc request refused");
    Ok(())
}
fn main() {
    if let Err(message) = run() {
        eprintln!("FAIL {message}");
        std::process::exit(1);
    }
    println!("SMB LIVE CHECK PASS");
}
