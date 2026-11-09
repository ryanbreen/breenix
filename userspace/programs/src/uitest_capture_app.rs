//! Controlled producer used by the five forced capture cases.
use breengel::Window;
use libuitest::capture_marker::{self, Generation};
use std::{io::{BufRead, BufReader, Write}, os::unix::net::UnixStream,
    sync::{mpsc, Arc, Mutex}, thread, time::Duration};

#[derive(Clone, Copy)]
enum Command { Create, CreateTree, Commit, Resize, Destroy }
struct Producer { semantic: bool, window: Option<Window>, ordinal: u64, generation: Generation }
impl Producer {
    fn apply(&mut self, command: Command, title: &str) -> Result<String, String> {
        match command {
            Command::Create | Command::CreateTree => {
                self.semantic = matches!(command, Command::CreateTree);
                if self.window.is_some() { return Err("window already exists".into()); }
                self.window = Some(Window::new_without_fonts(title.as_bytes(), 320, 200).map_err(|e| e.to_string())?);
                self.ordinal = 0;
            }
            Command::Resize => self.window.as_mut().ok_or("no window")?.apply_resize(160, 100).map_err(|e| e.to_string())?,
            Command::Destroy => {
                let window = self.window.take().ok_or("no window")?;
                let reply = self.reply(window.id(), window.width(), window.height());
                // Observe DESTROY's result before Drop's defensive second call.
                libbreenix::window::destroy(window.id()).map_err(|e| e.to_string())?;
                drop(window);
                return Ok(reply);
            }
            Command::Commit => {}
        }
        let window = self.window.as_mut().ok_or("no window")?;
        let width = window.width(); let height = window.height();
        let generation = Generation { commit: self.ordinal + 1,
            producer: u32::try_from(libbreenix::process::gettid().map_err(|e| e.to_string())?.raw()).map_err(|e| e.to_string())? };
        // Use a bounded acquisition: an unexpected pool hold fails the command.
        window.acquire(500_000_000).map_err(|e| e.to_string())?;
        let surface = window.surface().map_err(|e| e.to_string())?;
        let marker = capture_marker::encode(generation);
        surface.batch(|surface| {
            surface.clear(libgfx::color::Color::rgb(0x18, 0x20, 0x30));
            for (x, pixel) in marker.chunks_exact(4).enumerate() {
                libgfx::shapes::fill_rect(surface, x as i32, 0, 1, height as i32,
                    libgfx::color::Color::rgb(pixel[2], pixel[1], pixel[0]));
            }
        });
        if self.semantic {
            use breengel::tree::{Tree, Node, Role, Flag};
            window.set_tree(Tree { root: Node {
                key: "root".into(), parent: None, role: Role::Window, name: "capture".into(),
                value: serde_json::json!({"commit": generation.commit, "producer": generation.producer}),
                bounds: [0, 0, width.into(), height.into()], clip: [0, 0, width.into(), height.into()],
                flags: vec![Flag::Enabled, Flag::Visible],
            }, children: Vec::new() });
        } else {
            window.write_tree(&libuitest::capture::root(generation, width, height)).map_err(|e| e.to_string())?;
        }
        let commit = window.commit(&[libbreenix::window::abi::Rect::new(0, 0, width as i32, height as i32)]).map_err(|e| e.to_string())?;
        if commit.commit_id != generation.commit { return Err("producer ordinal differs from commit id".into()); }
        let id = window.id();
        self.ordinal = commit.commit_id; self.generation = generation;
        Ok(self.reply(id, width, height))
    }
    fn reply(&self, id: u64, width: u32, height: u32) -> String {
        format!("ok {id} {} {} {width} {height}", self.generation.commit, self.generation.producer)
    }
}

pub fn serve(title: &str) -> Result<(), String> {
    let root = std::env::var_os("BREENIX_CONFIG_ROOT").ok_or("capture control requires a session root")?;
    let path = std::path::Path::new(&root).join("capture-control.sock");
    let socket = UnixStream::connect(path).map_err(|e| e.to_string())?;
    // The controller can be comparing a saved screen between commands. Its
    // enclosing runner owns the case deadline; waiting here is not app work.
    socket.set_read_timeout(None).map_err(|e| e.to_string())?;
    socket.set_write_timeout(Some(Duration::from_secs(2))).map_err(|e| e.to_string())?;
    let mut socket = BufReader::new(socket);
    let state = Arc::new(Mutex::new(Producer { semantic: false, window: None, ordinal: 0, generation: Generation { commit: 0, producer: 0 } }));
    let (replies, results) = mpsc::channel();
    let mut senders = Vec::new(); let mut workers = Vec::new();
    for _ in 0..2 {
        let (sender, jobs) = mpsc::channel::<Command>();
        let replies = replies.clone(); let state = state.clone(); let title = title.to_string();
        workers.push(thread::spawn(move || {
            while let Ok(command) = jobs.recv() {
                let reply = state.lock().map_err(|e| e.to_string()).and_then(|mut state| state.apply(command, &title));
                if replies.send(reply).is_err() { break; }
            }
        }));
        senders.push(sender);
    }
    drop(replies);
    let result = (|| {
        loop {
            let mut line = String::new();
            if socket.read_line(&mut line).map_err(|e| e.to_string())? == 0 { return Err("capture controller disconnected".into()); }
            let (worker, command) = match line.trim_end() {
                "create" => (0, Command::Create), "create-tree" => (0, Command::CreateTree), "commit 0" => (0, Command::Commit),
                "commit 1" => (1, Command::Commit), "resize" => (0, Command::Resize),
                "destroy" => (0, Command::Destroy),
                "quit" => {
                    let mut state = state.lock().map_err(|e| e.to_string())?;
                    drop(state.window.take());
                    writeln!(socket.get_mut(), "ok 0 0 0 0 0").map_err(|e| e.to_string())?;
                    return Ok(());
                }
                _ => return Err(format!("unknown capture command {line:?}")),
            };
            senders[worker].send(command).map_err(|e| e.to_string())?;
            let reply = results.recv_timeout(Duration::from_secs(2)).map_err(|e| e.to_string())?;
            match reply {
                Ok(reply) => writeln!(socket.get_mut(), "{reply}").map_err(|e| e.to_string())?,
                Err(error) => {
                    let _ = writeln!(socket.get_mut(), "error {}", error.replace(['\n', '\r'], " "));
                    return Err(error);
                }
            }
        }
    })();
    drop(senders);
    for worker in workers { worker.join().map_err(|_| "capture producer panicked")?; }
    result
}
