//! Slow-client input delivery regression for #2163; run with rustc --test.

#[path = "../kernel/src/window/input_ring.rs"]
mod input_ring;
#[path = "../userspace/programs/src/bwm/input_backlog.rs"]
mod input_backlog;
use input_backlog::{Backlog, Delivery};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Event { kind: u16, value: u16, scroll: i16 }
fn motion(old: &Event, new: &Event) -> bool {
    old.kind == 3 && new.kind == 3 && old.scroll == 0 && new.scroll == 0
}

#[test]
fn a_slow_client_receives_discrete_events_in_order_and_motion_keeps_barriers() {
    let mut backlog = Backlog::new();
    let mut expected = Vec::new();
    for value in 0..201 {
        let event = Event { kind:1,value,scroll:0 };
        expected.push(event);
        backlog.push(1,event,motion);
        if value == 100 {
            for kind in [2,4,5,6,8] {
                let event = Event { kind,value,scroll:0 };
                expected.push(event);
                backlog.push(1,event,motion);
            }
        }
    }
    let mut ring = [Event { kind:0,value:0,scroll:0 };64];
    let (mut head, mut tail) = (0,0);
    let mut received = Vec::new();
    while !backlog.is_empty() || head != tail {
        backlog.flush(|id,event| {
            assert_eq!(id,1);
            if input_ring::push(&mut ring,&mut head,tail,*event) { Delivery::Accepted } else { Delivery::Full }
        });
        for _ in 0..16 {
            if head == tail { break; }
            received.push(ring[tail]);
            tail = (tail+1)%ring.len();
        }
    }
    assert_eq!(received,expected);

    for value in 0..200 { backlog.push(1,Event { kind:3,value,scroll:0 },motion); }
    backlog.push(1,Event { kind:1,value:30,scroll:0 },motion);
    backlog.push(1,Event { kind:3,value:201,scroll:0 },motion);
    backlog.push(1,Event { kind:3,value:202,scroll:1 },motion);
    backlog.push(1,Event { kind:3,value:203,scroll:0 },motion);
    backlog.push(2,Event { kind:1,value:99,scroll:0 },motion);
    let mut other = Vec::new();
    backlog.flush(|id,event| if id == 1 { Delivery::Full } else { other.push(*event); Delivery::Accepted });
    assert_eq!(other,[Event { kind:1,value:99,scroll:0 }]);
    let mut moves = Vec::new();
    backlog.flush(|_,event| { moves.push(*event); Delivery::Accepted });
    assert_eq!(moves.iter().map(|e|(e.kind,e.value,e.scroll)).collect::<Vec<_>>(),
        [(3,199,0),(1,30,0),(3,201,0),(3,202,1),(3,203,0)]);

    backlog.push(3,Event { kind:1,value:1,scroll:0 },motion);
    backlog.flush(|_,_|Delivery::Gone);
    assert!(backlog.is_empty());
}
