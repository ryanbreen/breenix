#![allow(dead_code)]
extern crate alloc;
extern crate self as libbui;
#[path = "../libs/libbui/src/tree.rs"] pub mod tree;
#[path = "../userspace/programs/src/guskit/canvas.rs"] mod canvas;
#[path = "../userspace/programs/src/guskit/collab.rs"] mod collab;
#[path = "../userspace/programs/src/guskit/node.rs"] mod node;
use canvas::{Canvas, Radius};
use collab::Core;
use libcollab::{queue::{SendError, Transport, QUEUE_LIMIT}, wire::{encode, DrawOp, Message}};
use serde_json::json;

fn pencil(x: i16, color: [u8; 3]) -> DrawOp {
    DrawOp::Pencil { x0:x, y0:0, x1:x, y1:3, color }
}
fn join(host: &mut Core, conn: u32, name: &str) {
    host.connect(conn);
    host.receive(conn, &encode(&Message::Hello { version:1, name:name.as_bytes().to_vec() }).unwrap());
}
fn remote(host: &mut Core, conn: u32, local: u32, op: DrawOp) {
    host.receive(conn, &encode(&Message::Op { local, op }).unwrap());
}
#[test]
fn replay_draws_live_operations_with_the_canvas_renderer() {
    let base = Canvas::new(20, 20);
    let mut direct = base.clone();
    let mut host = Core::host("Host", base, 20, 20);
    let ops = [pencil(0,[1,2,3]),
        DrawOp::Brush { x0:4,y0:4,x1:5,y1:5,radius:2,color:[4,5,6] },
        DrawOp::Eraser { x0:4,y0:4,x1:5,y1:5,radius:2 },
        DrawOp::Line { x0:0,y0:8,x1:12,y1:10,color:[7,8,9] },
        DrawOp::Rectangle { x:15,y:15,width:-5,height:-3,color:[10,11,12] },
        DrawOp::Circle { x:10,y:5,radius:3,color:[13,14,15] },
        DrawOp::Fill { x:19,y:19,color:[16,17,18] }];
    direct.pencil(&[(0,0),(0,3)],[1,2,3]);
    direct.brush(&[(4,4),(5,5)],Radius::Small,[4,5,6]);
    direct.erase(&[(4,4),(5,5)],Radius::Small);
    direct.line((0,8),(12,10),[7,8,9]);
    direct.rectangle((15,15),(10,12),[10,11,12]);
    direct.circle((10,5),(13,5),[13,14,15]);
    direct.fill((19,19),[16,17,18]);
    for op in ops { host.submit(op); }
    assert_eq!(host.model.canvas.pixels(), direct.pixels());
    host.submit(DrawOp::Clear);
    assert_eq!(host.model.canvas.pixels(), Canvas::new(20,20).pixels());
}
#[test]
fn undo_removes_only_its_owners_target_and_node_reports_answers() {
    let mut host = Core::host("Host", Canvas::new(4,4), 4,4);
    join(&mut host, 7, "Guest");
    host.submit(pencil(0,[1,2,3]));
    remote(&mut host,7,1,pencil(1,[4,5,6]));
    host.submit(pencil(2,[7,8,9]));
    host.submit(DrawOp::Undo);
    let mut expected=Canvas::new(4,4);
    expected.pencil(&[(0,0),(0,3)],[1,2,3]);
    expected.pencil(&[(1,0),(1,3)],[4,5,6]);
    assert_eq!(host.model.canvas.pixels(),expected.pixels());
    let n=node::collab([0,0,4,4],&host.model);
    assert_eq!(n.key,"collab"); assert_eq!(n.role,tree::Role::Status);
    assert_eq!(n.value["role"],"host"); assert_eq!(n.value["name"],"Host");
    assert_eq!(n.value["peers"],json!([{"id":1,"name":"Guest"}]));
    assert_eq!(n.value["state"],"connected"); assert!(n.value["reason"].is_null());
    assert_eq!(n.value["submitted"],json!([{"id":1,"global":1},{"id":2,"global":3},{"id":3,"global":4}]));
    assert_eq!(n.value["applied"],json!([1,2,3,4]));
    host.submit(DrawOp::Undo); host.submit(DrawOp::Undo);
    assert_eq!(node::collab([0;4],&host.model).value["submitted"][4],json!({"id":5,"refused":"nothing-to-undo"}));
    host.closed(7,"disconnected");
    assert_eq!(host.model.status,"Guest left");
}
struct Slow(Vec<u8>, bool);
impl Transport for Slow {
    type Error=();
    fn send(&mut self,_:u32,bytes:&[u8])->Result<usize,SendError<()>> {
        if self.1 { return Err(SendError::WouldBlock); }
        let n=bytes.len().min(4096); self.0.extend_from_slice(&bytes[..n]); self.1=true; Ok(n)
    }
}
#[test]
fn lazy_sync_keeps_live_operations_ordered_and_publishes_peer_identity() {
    let mut base=Canvas::new(800,424);
    base.pencil(&[(2,2),(7,4)],[18,29,40]);
    let mut host=Core::host("Host",base,800,424);
    host.submit(pencil(0,[1,2,3]));
    join(&mut host,7,"Guest");
    let mut peer=Core::join("Guest");
    let mut sink=Slow(Vec::new(),false);
    for i in 0..400 {
        if i==2 { host.submit(pencil(1,[4,5,6])); }
        sink.1=false; host.flush(7,&mut sink);
        assert!(host.queues.queued(7)<=QUEUE_LIMIT);
        for part in sink.0.chunks(137) { peer.receive(1,part); }
        sink.0.clear();
        if !host.pending(7) { break; }
    }
    assert!(!host.pending(7));
    assert_eq!(peer.model.canvas.pixels(),host.model.canvas.pixels());
    let n=node::collab([0;4],&peer.model);
    assert_eq!(n.value["peers"],json!([{"id":0,"name":"Host"}]));
    assert_eq!(n.value["applied"],json!([1,2]));
    assert_eq!(n.value["state"],"connected");
    peer.submit(pencil(3,[7,8,9])); peer.closed(1,"disconnected");
    assert_eq!(peer.model.status,"disconnected: disconnected");
    assert_eq!(node::collab([0;4],&peer.model).value["submitted"],json!([{"id":1,"refused":"disconnected"}]));
}

#[test]
fn undo_removes_a_whole_freehand_gesture_and_preserves_other_peers() {
    for tool in [0, 1, 6] {
        let mut host = Core::host("Host", Canvas::new(80,20),80,20);
        join(&mut host,7,"Guest");
        remote(&mut host,7,1,pencil(70,[1,2,3]));
        let before=host.model.canvas.clone();
        let points: Vec<_>=(0..60).map(|x|(x,10)).collect();
        host.submit_stroke(collab::stroke(tool,0,[4,5,6],&points));
        assert_eq!(host.model.undo_depth(),1,"one gesture is one undo entry");
        host.submit(DrawOp::Undo);
        assert_eq!(host.model.canvas.pixels(),before.pixels(),"whole gesture undone");
        host.submit_stroke(collab::stroke(tool,0,[7,8,9],&points));
        host.submit(DrawOp::Undo);
        assert_eq!(host.model.canvas.pixels(),before.pixels(),"next gesture is independent");
    }
}
