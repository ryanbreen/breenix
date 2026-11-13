//! Gus Kit's sequenced canvas and bounded session driver.
use super::canvas::{Canvas, Point, Radius};
use libcollab::{
    host::{Event, HostCore, Output},
    peer::{PeerCore, Response},
    queue::{OutputQueues, Transport, SYNC_LOW_WATER},
    wire::{self, DrawOp, Message},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub struct Model {
    pub role: &'static str,
    pub name: String,
    pub peers: BTreeMap<u8, String>,
    pub state: &'static str,
    pub reason: Option<String>,
    pub submitted: Vec<u32>,
    pub peer: PeerCore,
    pub canvas: Canvas,
    pub status: String,
    pub id: Option<u8>,
    width: usize,
    height: usize,
}
impl Model {
    fn new(role: &'static str, name: &str, canvas: Canvas, width: usize, height: usize) -> Self {
        Self {
            role,
            name: name.into(),
            peers: BTreeMap::new(),
            state: if role == "host" {
                "connected"
            } else {
                "connecting"
            },
            reason: None,
            submitted: Vec::new(),
            peer: PeerCore::new(),
            canvas,
            status: String::new(),
            id: if role == "host" { Some(0) } else { None },
            width,
            height,
        }
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    fn receive(&mut self, msg: Message) -> Result<(), &'static str> {
        match &msg {
            Message::Welcome {
                peer,
                width,
                height,
            } => {
                if *width == 0 || *height == 0 || *width > 800 || *height > 424 {
                    return Err("canvas-size");
                }
                self.id = Some(*peer);
                self.width = *width as usize;
                self.height = *height as usize;
                self.state = "syncing";
            }
            Message::SyncChunk { offset, data } => {
                if *offset as usize + data.len() > self.width * self.height * 3 {
                    return Err("canvas-size");
                }
            }
            Message::SyncEnd => {
                if self.peer.base_canvas().len() != self.width * self.height * 3 {
                    return Err("canvas-size");
                }
                self.state = "connected";
            }
            Message::PeerJoined { peer, name } => {
                if Some(*peer) != self.id {
                    self.peers
                        .insert(*peer, String::from_utf8_lossy(name).into_owned());
                }
            }
            Message::PeerLeft { peer, .. } => {
                let name = self
                    .peers
                    .remove(peer)
                    .unwrap_or_else(|| format!("Peer {peer}"));
                self.status = format!("{name} left");
            }
            Message::JoinRefused { reason } => {
                self.disconnect(&String::from_utf8_lossy(reason));
                return Ok(());
            }
            Message::Bye => {
                self.disconnect("disconnected");
                return Ok(());
            }
            _ => {}
        }
        let apply = match &msg { Message::Apply { op, .. } => Some(op.clone()), _ => None };
        let replay = matches!(msg, Message::SyncEnd) || apply == Some(DrawOp::Undo);
        self.peer.receive(msg).map_err(|_| "protocol")?;
        if replay {
            self.replay()?;
        } else if let Some(op) = apply {
            draw(&mut self.canvas, &op);
        }
        Ok(())
    }
    fn replay(&mut self) -> Result<(), &'static str> {
        let base = self.peer.base_canvas();
        if base.len() != self.width * self.height * 3 {
            return Err("canvas-size");
        }
        let mut canvas = Canvas::new(self.width, self.height);
        canvas.pixels_mut().copy_from_slice(base);
        for msg in libcollab::live_ops(self.peer.log()) {
            if let Message::Apply { op, .. } = msg {
                draw(&mut canvas, &op);
            }
        }
        self.canvas = canvas;
        Ok(())
    }
    pub fn undo_depth(&self) -> usize {
        libcollab::live_ops(self.peer.log())
            .iter()
            .filter(|msg| matches!(msg,Message::Apply {peer,..} if Some(*peer)==self.id))
            .filter_map(libcollab::gesture_key)
            .collect::<BTreeSet<_>>()
            .len()
    }
    fn disconnect(&mut self, reason: &str) {
        self.state = "disconnected";
        self.reason = Some(reason.into());
        self.status = format!("disconnected: {reason}");
        self.peer.disconnect();
    }
}

fn radius(value: u8) -> Radius {
    match value {
        2 => Radius::Small,
        5 => Radius::Medium,
        _ => Radius::Large,
    }
}
fn draw(c: &mut Canvas, op: &DrawOp) {
    let p = |x: i16, y: i16| (i32::from(x), i32::from(y));
    match *op {
        DrawOp::StrokeSegment { tool, x0, y0, x1, y1, radius: r, color, .. } => {
            let points = [p(x0,y0),p(x1,y1)];
            match tool {
                0 => c.pencil(&points,color),
                1 => c.brush(&points,radius(r),color),
                _ => c.erase(&points,radius(r)),
            }
        }
        DrawOp::Pencil {
            x0,
            y0,
            x1,
            y1,
            color,
        } => c.pencil(&[p(x0, y0), p(x1, y1)], color),
        DrawOp::Brush {
            x0,
            y0,
            x1,
            y1,
            radius: r,
            color,
        } => c.brush(&[p(x0, y0), p(x1, y1)], radius(r), color),
        DrawOp::Eraser {
            x0,
            y0,
            x1,
            y1,
            radius: r,
        } => c.erase(&[p(x0, y0), p(x1, y1)], radius(r)),
        DrawOp::Line {
            x0,
            y0,
            x1,
            y1,
            color,
        } => c.line(p(x0, y0), p(x1, y1), color),
        DrawOp::Rectangle {
            x,
            y,
            width,
            height,
            color,
        } => c.rectangle(
            p(x, y),
            (
                i32::from(x) + i32::from(width),
                i32::from(y) + i32::from(height),
            ),
            color,
        ),
        DrawOp::Circle {
            x,
            y,
            radius,
            color,
        } => c.circle(
            p(x, y),
            (i32::from(x) + i32::from(radius), i32::from(y)),
            color,
        ),
        DrawOp::Fill { x, y, color } => c.fill(p(x, y), color),
        DrawOp::Clear => c.pixels_mut().fill(255),
        DrawOp::Undo => {}
    }
}
/// Convert a completed gesture to wire operations, one per polyline segment.
pub fn stroke(tool: usize, size: usize, color: [u8; 3], points: &[Point]) -> Vec<DrawOp> {
    let Some(&(x, y)) = points.first() else {
        return Vec::new();
    };
    let &(last_x, last_y) = points.last().unwrap();
    let (x0, y0, x1, y1) = (x as i16, y as i16, last_x as i16, last_y as i16);
    let radius = [2, 5, 10][size];
    match tool {
        2 => vec![DrawOp::Line {
            x0,
            y0,
            x1,
            y1,
            color,
        }],
        3 => vec![DrawOp::Rectangle {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
            color,
        }],
        4 => {
            let n = (last_x - x).pow(2) + (last_y - y).pow(2);
            let mut r = 0;
            while (r + 1) * (r + 1) <= n {
                r += 1;
            }
            vec![DrawOp::Circle {
                x: x0,
                y: y0,
                radius: r as i16,
                color,
            }]
        }
        5 => vec![DrawOp::Fill {
            x: x0,
            y: y0,
            color,
        }],
        _ => {
            let segments: Vec<_> = if points.len() == 1 {
                vec![(points[0], points[0])]
            } else {
                points.windows(2).map(|p| (p[0], p[1])).collect()
            };
            segments
                .into_iter()
                .map(|(a, b)| {
                    let (x0, y0, x1, y1) = (a.0 as i16, a.1 as i16, b.0 as i16, b.1 as i16);
                    match tool {
                        0 => DrawOp::Pencil {
                            x0,
                            y0,
                            x1,
                            y1,
                            color,
                        },
                        1 => DrawOp::Brush {
                            x0,
                            y0,
                            x1,
                            y1,
                            radius,
                            color,
                        },
                        _ => DrawOp::Eraser {
                            x0,
                            y0,
                            x1,
                            y1,
                            radius,
                        },
                    }
                })
                .collect()
        }
    }
}

pub struct Core {
    pub model: Model,
    pub queues: OutputQueues,
    host: Option<HostCore>,
    replay: BTreeMap<u32, usize>,
    input: Vec<u8>,
    pub closing: BTreeSet<u32>,
}
impl Core {
    pub fn host(name: &str, canvas: Canvas, width: usize, height: usize) -> Self {
        let host = HostCore::session(
            width as u16,
            height as u16,
            canvas.pixels().to_vec(),
            name.as_bytes().to_vec(),
        );
        let mut core = Self {
            model: Model::new("host", name, canvas, width, height),
            queues: OutputQueues::new(),
            host: Some(host),
            replay: BTreeMap::new(),
            input: Vec::new(),
            closing: BTreeSet::new(),
        };
        let base = core.host.as_ref().unwrap().base_canvas().to_vec();
        core.model
            .receive(Message::SyncChunk {
                offset: 0,
                data: base,
            })
            .expect("host base");
        core
    }
    pub fn join(name: &str) -> Self {
        let mut queues = OutputQueues::new();
        queues.connect(1);
        queues
            .enqueue(
                1,
                &wire::encode(&Message::Hello {
                    version: 1,
                    name: name.as_bytes().to_vec(),
                })
                .expect("bounded name"),
            )
            .expect("empty queue");
        Self {
            model: Model::new("peer", name, Canvas::new(800, 424), 800, 424),
            queues,
            host: None,
            replay: BTreeMap::new(),
            input: Vec::new(),
            closing: BTreeSet::new(),
        }
    }
    pub fn connect(&mut self, conn: u32) {
        self.queues.connect(conn);
        if let Some(host) = &mut self.host {
            host.connect(conn);
        }
    }
    pub fn submit_stroke(&mut self, ops: Vec<DrawOp>) {
        let gesture = self.model.submitted.len() as u32 + 1;
        for op in ops {
            let segment = match op {
                DrawOp::Pencil { x0,y0,x1,y1,color } =>
                    DrawOp::StrokeSegment { gesture,tool:0,x0,y0,x1,y1,radius:2,color },
                DrawOp::Brush { x0,y0,x1,y1,radius,color } =>
                    DrawOp::StrokeSegment { gesture,tool:1,x0,y0,x1,y1,radius,color },
                DrawOp::Eraser { x0,y0,x1,y1,radius } =>
                    DrawOp::StrokeSegment { gesture,tool:6,x0,y0,x1,y1,radius,color:[255;3] },
                other => other,
            };
            self.submit(segment);
        }
    }
    pub fn submit(&mut self, op: DrawOp) -> u32 {
        let id = self.model.peer.submit(op.clone());
        self.model.submitted.push(id);
        if self.model.state == "disconnected" {
            self.model
                .peer
                .receive(Message::Refused {
                    local: id,
                    reason: b"disconnected".to_vec(),
                })
                .expect("new submission");
        } else if let Some(host) = &mut self.host {
            let out = host.submit(op);
            self.outputs(out);
        } else {
            self.enqueue(
                1,
                &wire::encode(&Message::Op { local: id, op }).expect("op fits"),
            );
        }
        id
    }
    pub fn receive(&mut self, conn: u32, bytes: &[u8]) {
        if let Some(host) = &mut self.host {
            let out = host.bytes(conn, bytes);
            self.outputs(out);
        } else {
            self.input.extend_from_slice(bytes);
            loop {
                match wire::decode(&self.input) {
                    Ok(Some((msg, used))) => {
                        self.input.drain(..used);
                        if let Err(reason) = self.model.receive(msg) {
                            self.closed(conn, reason);
                            break;
                        }
                        if self.model.state == "disconnected" {
                            self.queues.remove(conn);
                            self.closing.insert(conn);
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        self.closed(conn, "decode");
                        break;
                    }
                }
            }
        }
    }
    fn outputs(&mut self, out: Vec<Output>) {
        let mut pending: VecDeque<_> = out.into();
        while let Some(output) = pending.pop_front() {
            match output {
                Output::Sync { conn } => {
                    let base = self.host.as_ref().unwrap().base_canvas().to_vec();
                    if let Err(reason) = self.queues.start_sync(conn, base) {
                        self.drop_connection(conn, reason, &mut pending);
                    } else {
                        self.replay.insert(conn, 0);
                    }
                }
                Output::Frame { conn, data } => {
                    // Apply is streamed from the admitted log until catch-up completes.
                    if self.replay.contains_key(&conn)
                        && matches!(wire::decode(&data), Ok(Some((Message::Apply { .. }, _))))
                    {
                        continue;
                    }
                    if let Err(reason) = self.queues.enqueue(conn, &data) {
                        self.drop_connection(conn, reason, &mut pending);
                    }
                }
                Output::Event(Event::Apply(msg)) => {
                    if let Message::Apply {
                        peer: 0,
                        local,
                        global,
                        ..
                    } = &msg
                    {
                        self.model
                            .receive(Message::Ack {
                                local: *local,
                                global: *global,
                            })
                            .expect("host answer");
                    }
                    self.model.receive(msg).expect("host global order");
                }
                Output::Event(Event::Refused { local, reason }) => {
                    self.model
                        .receive(Message::Refused { local, reason })
                        .expect("host refusal");
                }
                Output::Event(Event::PeerJoined { peer, name }) => {
                    self.model
                        .receive(Message::PeerJoined { peer, name })
                        .expect("peer name");
                }
                Output::Event(Event::PeerLeft { peer, reason }) => {
                    self.model
                        .receive(Message::PeerLeft { peer, reason })
                        .expect("peer left");
                }
                Output::Event(Event::Close { conn, .. }) => {
                    if self.replay.remove(&conn).is_some() {
                        self.queues.remove(conn);
                    }
                    self.closing.insert(conn);
                }
            }
        }
    }
    fn drop_connection(&mut self, conn: u32, reason: &[u8], pending: &mut VecDeque<Output>) {
        self.queues.remove(conn);
        self.replay.remove(&conn);
        self.closing.insert(conn);
        if let Some(host) = &mut self.host {
            pending.extend(host.close_with_reason(conn, reason));
        }
    }
    fn enqueue(&mut self, conn: u32, bytes: &[u8]) {
        if let Err(reason) = self.queues.enqueue(conn, bytes) {
            self.closed(conn, &String::from_utf8_lossy(reason));
        }
    }
    pub fn closed(&mut self, conn: u32, reason: &str) {
        self.queues.remove(conn);
        self.replay.remove(&conn);
        self.closing.insert(conn);
        if let Some(host) = &mut self.host {
            let out = host.close_with_reason(conn, reason.as_bytes());
            self.outputs(out);
        } else if self.model.state != "disconnected" {
            self.model.disconnect(reason);
        }
    }
    pub fn pending(&self, conn: u32) -> bool {
        self.queues.wants_write(conn)
            || self.queues.syncing(conn)
            || self.replay.contains_key(&conn)
    }
    pub fn flush<T: Transport>(&mut self, conn: u32, transport: &mut T) {
        if self.queues.fill_sync(conn).is_err() {
            return;
        }
        if !self.queues.syncing(conn) {
            if let Some(cursor) = self.replay.get_mut(&conn) {
                let log = self.host.as_ref().unwrap().log();
                while *cursor < log.len() && self.queues.queued(conn) < SYNC_LOW_WATER {
                    let bytes = wire::encode(&log[*cursor]).expect("admitted op");
                    if self.queues.enqueue(conn, &bytes).is_err() {
                        self.closed(conn, "queue-full");
                        return;
                    }
                    *cursor += 1;
                }
                if *cursor == log.len() {
                    self.replay.remove(&conn);
                }
            }
        }
        if self.queues.flush(conn, transport).is_err() {
            self.closed(conn, "send");
        }
    }
}

#[cfg(any(test, target_os = "breenix"))]
pub struct Session {
    pub core: Core,
    transport: libcollab::net::TcpTransport,
}
#[cfg(any(test, target_os = "breenix"))]
impl Session {
    pub fn host(
        address: [u8; 4],
        port: u16,
        name: &str,
        canvas: Canvas,
        width: usize,
        height: usize,
    ) -> Result<Self, libbreenix::error::Error> {
        Ok(Self {
            core: Core::host(name, canvas, width, height),
            transport: libcollab::net::TcpTransport::bind(address, port)?,
        })
    }
    pub fn join(address: [u8; 4], port: u16, name: &str) -> Result<Self, libbreenix::error::Error> {
        let mut transport = libcollab::net::TcpTransport::client();
        transport.connect(address, port)?;
        Ok(Self {
            core: Core::join(name),
            transport,
        })
    }
    pub fn poll(&mut self, timeout: i32) -> bool {
        use libbreenix::{
            errno::Errno,
            error::Error,
            io::{self, poll_events},
        };
        let before = (
            self.core.model.peer.log().len(),
            self.core
                .model
                .submitted
                .iter()
                .filter(|id| self.core.model.peer.response(**id).is_some())
                .count(),
            self.core.model.peers.clone(),
            self.core.model.state,
            self.core.model.status.clone(),
        );
        let ids: Vec<_> = self
            .transport
            .poll_fds(&self.core.queues)
            .into_iter()
            .filter_map(|(id, _)| id)
            .collect();
        for id in ids {
            if self.core.pending(id) {
                self.core.flush(id, &mut self.transport);
            }
        }
        let entries = self.transport.poll_fds(&self.core.queues);
        let mut fds: Vec<_> = entries.iter().map(|(_, fd)| *fd).collect();
        if io::poll(&mut fds, timeout.clamp(0, 10)).is_ok() {
            for ((id, _), fd) in entries.into_iter().zip(fds) {
                let Some(id) = id else {
                    if fd.revents & poll_events::POLLIN != 0 {
                        // Bound work per dispatch turn even under a connection flood.
                        for _ in 0..16 {
                            match self.transport.accept() {
                                Ok(id) => self.core.connect(id),
                                Err(_) => break,
                            }
                        }
                    }
                    continue;
                };
                if fd.revents & (poll_events::POLLIN | poll_events::POLLHUP | poll_events::POLLERR)
                    != 0
                {
                    let mut bytes = [0u8; 8192];
                    for _ in 0..32 {
                        match self.transport.recv(id, &mut bytes) {
                            Ok(0) => {
                                self.core.closed(id, "disconnected");
                                break;
                            }
                            Ok(n) => self.core.receive(id, &bytes[..n]),
                            Err(Error::Os(Errno::EAGAIN)) => break,
                            Err(_) => {
                                self.core.closed(id, "receive");
                                break;
                            }
                        }
                    }
                }
                self.core.flush(id, &mut self.transport);
            }
        }
        // Drain refusal frames before a protocol-requested close; send errors and
        // queue overflow discard the affected queue and close immediately.
        let closed: Vec<_> = self
            .core
            .closing
            .iter()
            .copied()
            .filter(|id| !self.core.queues.wants_write(*id))
            .collect();
        for id in closed {
            self.transport.close(id);
            self.core.queues.remove(id);
            self.core.closing.remove(&id);
        }
        let after = (
            self.core.model.peer.log().len(),
            self.core
                .model
                .submitted
                .iter()
                .filter(|id| self.core.model.peer.response(**id).is_some())
                .count(),
            self.core.model.peers.clone(),
            self.core.model.state,
            self.core.model.status.clone(),
        );
        before != after
    }
}

#[cfg(test)]
pub(super) fn session_fixture(core: Core) -> Session {
    Session { core, transport: libcollab::net::TcpTransport::client() }
}

impl super::node::Collaboration for Model {
    fn collab_value(&self) -> serde_json::Value {
        use serde_json::json;
        let submitted: Vec<_> = self
            .submitted
            .iter()
            .map(|id| match self.peer.response(*id) {
                Some(Response::Ack { global }) => json!({"id":id,"global":global}),
                Some(Response::Refused(reason)) => {
                    json!({"id":id,"refused":String::from_utf8_lossy(reason)})
                }
                None => json!({"id":id}),
            })
            .collect();
        let applied: Vec<_> = self
            .peer
            .log()
            .iter()
            .filter_map(|m| match m {
                Message::Apply { global, .. } => Some(*global),
                _ => None,
            })
            .collect();
        let peers: Vec<_> = self
            .peers
            .iter()
            .map(|(id, name)| json!({"id":id,"name":name}))
            .collect();
        json!({"role":self.role,"name":self.name,"peers":peers,"state":self.state,"reason":self.reason,"submitted":submitted,"applied":applied})
    }
}
