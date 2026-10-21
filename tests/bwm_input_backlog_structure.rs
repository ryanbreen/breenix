//! Slow-client input delivery regression for #2163; run with rustc --test.

#[path = "../userspace/programs/src/bwm/input_backlog.rs"]
mod input_backlog;
#[path = "../kernel/src/syscall/graphics/input_ring.rs"]
mod input_ring;
use input_backlog::{Backlog, Delivery};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Event {
    kind: u16,
    value: u16,
    scroll: i16,
}
fn motion(old: &Event, new: &Event) -> bool {
    old.kind == 3 && new.kind == 3 && old.scroll == 0 && new.scroll == 0
}

#[test]
fn a_slow_client_receives_discrete_events_in_order_and_motion_keeps_barriers() {
    let mut backlog = Backlog::new();
    let mut expected = Vec::new();
    for value in 0..201 {
        let event = Event {
            kind: 1,
            value,
            scroll: 0,
        };
        expected.push(event);
        backlog.push(1, event, motion);
        if value == 100 {
            for kind in [2, 4, 5, 6, 8] {
                let event = Event {
                    kind,
                    value,
                    scroll: 0,
                };
                expected.push(event);
                backlog.push(1, event, motion);
            }
        }
    }
    let mut ring = [Event {
        kind: 0,
        value: 0,
        scroll: 0,
    }; 64];
    let (mut head, mut tail) = (0, 0);
    let mut received = Vec::new();
    while !backlog.is_empty() || head != tail {
        backlog.flush(|id, event| {
            assert_eq!(id, 1);
            if input_ring::push(&mut ring, &mut head, tail, *event) {
                Delivery::Accepted
            } else {
                Delivery::Full
            }
        });
        for _ in 0..16 {
            if head == tail {
                break;
            }
            received.push(ring[tail]);
            tail = (tail + 1) % ring.len();
        }
    }
    assert_eq!(received, expected);

    for value in 0..200 {
        backlog.push(
            1,
            Event {
                kind: 3,
                value,
                scroll: 0,
            },
            motion,
        );
    }
    backlog.push(
        1,
        Event {
            kind: 1,
            value: 30,
            scroll: 0,
        },
        motion,
    );
    backlog.push(
        1,
        Event {
            kind: 3,
            value: 201,
            scroll: 0,
        },
        motion,
    );
    backlog.push(
        1,
        Event {
            kind: 3,
            value: 202,
            scroll: 1,
        },
        motion,
    );
    backlog.push(
        1,
        Event {
            kind: 3,
            value: 203,
            scroll: 0,
        },
        motion,
    );
    backlog.push(
        2,
        Event {
            kind: 1,
            value: 99,
            scroll: 0,
        },
        motion,
    );
    let mut other = Vec::new();
    backlog.flush(|id, event| {
        if id == 1 {
            Delivery::Full
        } else {
            other.push(*event);
            Delivery::Accepted
        }
    });
    assert_eq!(
        other,
        [Event {
            kind: 1,
            value: 99,
            scroll: 0
        }]
    );
    let mut moves = Vec::new();
    backlog.flush(|_, event| {
        moves.push(*event);
        Delivery::Accepted
    });
    assert_eq!(
        moves
            .iter()
            .map(|e| (e.kind, e.value, e.scroll))
            .collect::<Vec<_>>(),
        [
            (3, 199, 0),
            (1, 30, 0),
            (3, 201, 0),
            (3, 202, 1),
            (3, 203, 0)
        ]
    );

    backlog.push(
        3,
        Event {
            kind: 1,
            value: 1,
            scroll: 0,
        },
        motion,
    );
    backlog.flush(|_, _| Delivery::Gone);
    assert!(backlog.is_empty());
}

#[test]
fn backlog_limit_disables_only_the_overloaded_window_until_registry_removal() {
    use input_backlog::{Admission, MAX_PENDING};
    let mut backlog = Backlog::new();
    for value in 0..MAX_PENDING {
        assert_eq!(backlog.push(1, value, |_, _| false), Admission::Queued);
    }
    let mut attempts = 0;
    backlog.flush(|_, _| {
        attempts += 1;
        Delivery::Full
    });
    assert_eq!(attempts, 1);
    assert_eq!(
        backlog.push(1, MAX_PENDING, |_, _| false),
        Admission::Overflow
    );
    assert_eq!(
        backlog.push(1, MAX_PENDING + 1, |_, _| false),
        Admission::Disabled
    );
    assert_eq!(backlog.push(2, 42, |_, _| false), Admission::Queued);
    let mut delivered = Vec::new();
    backlog.flush(|id, value| {
        delivered.push((id, *value));
        Delivery::Accepted
    });
    assert_eq!(delivered, [(2, 42)]);
    backlog.retain_windows(|id| id != 1);
    assert_eq!(backlog.push(1, 99, |_, _| false), Admission::Queued);
    backlog.flush(|_, value| {
        assert_eq!(*value, 99);
        Delivery::Accepted
    });
    assert!(backlog.is_empty());
}

#[test]
fn permanent_delivery_error_is_not_retried_or_overtaken() {
    use input_backlog::Admission;
    let mut backlog = Backlog::new();
    backlog.push(1, 1, |_, _| false);
    backlog.push(1, 2, |_, _| false);
    let mut attempts = 0;
    backlog.flush(|_, event| {
        attempts += 1;
        assert_eq!(*event, 1);
        Delivery::Failed
    });
    assert_eq!(attempts, 1);
    assert_eq!(backlog.push(1, 3, |_, _| false), Admission::Disabled);
    backlog.flush(|_, _| panic!("permanent delivery failure was retried"));
    assert!(backlog.is_empty());
}

#[test]
fn adjacent_motion_can_coalesce_at_the_limit_without_losing_discrete_events() {
    use input_backlog::{Admission, MAX_PENDING};
    let mut backlog = Backlog::new();
    for value in 0..MAX_PENDING - 1 {
        assert_eq!(
            backlog.push(
                1,
                Event {
                    kind: 1,
                    value: value as u16,
                    scroll: 0
                },
                motion
            ),
            Admission::Queued
        );
    }
    for value in [50, 51] {
        assert_eq!(
            backlog.push(
                1,
                Event {
                    kind: 3,
                    value,
                    scroll: 0
                },
                motion
            ),
            Admission::Queued
        );
    }
    let mut received = Vec::new();
    backlog.flush(|_, event| {
        received.push(*event);
        Delivery::Accepted
    });
    assert_eq!(received.len(), MAX_PENDING);
    for (value, event) in received[..MAX_PENDING - 1].iter().enumerate() {
        assert_eq!(
            *event,
            Event {
                kind: 1,
                value: value as u16,
                scroll: 0
            }
        );
    }
    assert_eq!(
        received[MAX_PENDING - 1],
        Event {
            kind: 3,
            value: 51,
            scroll: 0
        }
    );
}

#[test]
fn close_survives_overflow_and_retries_until_the_client_drains() {
    use input_backlog::{Admission, MAX_PENDING};
    let mut backlog = Backlog::new();
    for value in 0..MAX_PENDING {
        assert_eq!(backlog.push(1, value, |_, _| false), Admission::Queued);
    }
    assert_eq!(backlog.push(1, MAX_PENDING, |_, _| false), Admission::Overflow);
    // Both the close hotkey and launcher toggle use this reserved request when
    // ordinary admission reports Overflow or Disabled.
    for _ in 0..20 {
        backlog.request_close(1, 7);
    }
    assert_eq!(backlog.push(1, 99, |_, _| false), Admission::Disabled);
    let mut ring = [0; 64];
    let (mut head, mut tail) = (0, 0);
    for value in 0..63 {
        assert!(input_ring::push(&mut ring, &mut head, tail, value));
    }
    let mut attempts = 0;
    backlog.flush(|id, event| {
        assert_eq!((id, *event), (1, 7));
        attempts += 1;
        if input_ring::push(&mut ring, &mut head, tail, *event) {
            Delivery::Accepted
        } else {
            Delivery::Full
        }
    });
    assert_eq!(attempts, 1);
    assert!(!backlog.is_empty());
    tail = (tail + 16) % ring.len();
    backlog.flush(|id, event| {
        assert_eq!((id, *event), (1, 7));
        assert!(input_ring::push(&mut ring, &mut head, tail, *event));
        Delivery::Accepted
    });
    assert_eq!(ring[63], 7);
    assert!(backlog.is_empty());
    backlog.flush(|_, _| panic!("close was duplicated"));
    backlog.request_close(1, 7);
    backlog.retain_windows(|_| false);
    assert!(backlog.is_empty());
}
