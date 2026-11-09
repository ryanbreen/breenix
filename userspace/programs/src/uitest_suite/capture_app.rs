//! Socket controller for the test app's commanded captures.
use libuitest::{Session, capture_marker::Generation};
use std::{io::{BufRead, BufReader, Write}, os::unix::net::{UnixListener, UnixStream},
    path::Path, thread, time::{Duration, Instant}};

pub(crate) struct App { socket: BufReader<UnixStream>, pub pid: u64 }
#[derive(Clone, Copy)]
pub(crate) struct Commit { pub id: u64, pub generation: Generation, pub width: u32, pub height: u32 }
impl App {
    pub(crate) fn launch(session: &mut Session) -> Result<Self, String> {
        let path = Path::new(session.config_root()).join("capture-control.sock");
        let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let pid = session.launch_app(&[b"/usr/libexec/breenix/uitest-app_test", b"capture races", b"--capture-control"], 0)?;
        let end = Instant::now() + Duration::from_secs(2);
        let socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < end => thread::sleep(Duration::from_millis(1)),
                Err(e) => return Err(format!("capture app connection: {e}")),
            }
        };
        socket.set_read_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
        socket.set_write_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        Ok(Self { socket: BufReader::new(socket), pid })
    }
    pub(crate) fn send(&mut self, command: &str) -> Result<(), String> {
        writeln!(self.socket.get_mut(), "{command}").map_err(|e| e.to_string())
    }
    pub(crate) fn reply(&mut self) -> Result<Commit, String> {
        let mut line = String::new();
        self.socket.read_line(&mut line).map_err(|e| e.to_string())?;
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 6 || fields[0] != "ok" { return Err(format!("capture app: {line:?}")); }
        let number = |i: usize| fields[i].parse::<u64>().map_err(|e| e.to_string());
        Ok(Commit { id: number(1)?, generation: Generation { commit: number(2)?,
            producer: u32::try_from(number(3)?).map_err(|e| e.to_string())? },
            width: u32::try_from(number(4)?).map_err(|e| e.to_string())?,
            height: u32::try_from(number(5)?).map_err(|e| e.to_string())? })
    }
    pub(crate) fn command(&mut self, command: &str) -> Result<Commit, String> { self.send(command)?; self.reply() }
}
