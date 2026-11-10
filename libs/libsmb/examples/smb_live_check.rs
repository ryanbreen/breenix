//! Host integration check. Set SMB_TEST_HOST, optional SMB_TEST_PORT, and
//! SMB_TEST_ENV_FILE (path containing SMB_TEST_USER / SMB_TEST_PASSWORD and
//! optional SMB_TEST_DOMAIN). Credential values stay in the env file and are omitted from diagnostics.
use libsmb::{
    auth::ntlm::Credentials,
    client::{Client, Error},
    msg::*,
    session::Request,
    DialectCap, SyncHeader,
};
use std::{
    collections::HashMap,
    env, fs,
    io::ErrorKind,
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
        println!("PASS {stage} list/write/read/rename-replace/delete");
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
            Err(Error::Status(
                NtStatus::ACCESS_DENIED | NtStatus::MEDIA_WRITE_PROTECTED
            ))
        ),
        "ro write was not refused",
    )?;
    println!("PASS ro list/read/write-refused");
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
    println!("PASS wrong-password LOGON_FAILURE");
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
    println!("PASS unencrypted enc request refused");
    Ok(())
}
fn main() {
    if let Err(message) = run() {
        eprintln!("FAIL {message}");
        std::process::exit(1);
    }
    println!("SMB LIVE CHECK PASS");
}
