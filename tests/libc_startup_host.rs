//! Run with `rustc --edition=2021 --test tests/libc_startup_host.rs`.

#[path = "../libs/libbreenix-libc/src/lifecycle.rs"]
mod lifecycle;

mod libc_host {
    use super::lifecycle::{
        run_exit, run_handlers, run_main, run_thread_exit, Handler, HandlerList, HandlerNode,
        ProcessHandlers,
    };
    use std::cell::RefCell;
    use std::ptr::null_mut;
    use std::sync::Barrier;

    thread_local! {
        static TRACE: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    }

    fn record(event: usize) {
        TRACE.with(|trace| trace.borrow_mut().push(event));
    }

    fn trace() -> Vec<usize> {
        TRACE.with(|trace| std::mem::take(&mut *trace.borrow_mut()))
    }

    extern "C" fn first() {
        record(1);
    }

    extern "C" fn second() {
        record(2);
    }

    extern "C" fn third() {
        record(3);
    }

    extern "C" fn fourth() {
        record(4);
    }

    unsafe extern "C" fn record_arg(arg: *mut u8) {
        record(arg.addr());
    }

    fn cxa(event: usize, dso: *mut u8) -> Handler {
        Handler::Cxa {
            function: record_arg,
            argument: std::ptr::without_provenance_mut(event),
            dso,
        }
    }

    fn node(handler: Handler) -> &'static mut HandlerNode {
        Box::leak(Box::new(HandlerNode::new(handler)))
    }

    fn pop(node: Option<&'static mut HandlerNode>) -> Option<Handler> {
        node.map(|node| {
            // SAFETY: the node came from Box::leak and has been unlinked.
            let node = unsafe { Box::from_raw(node) };
            node.handler()
        })
    }

    #[test]
    fn init_array_runs_before_main() {
        let status = unsafe {
            run_main(&[first, second], &[third, fourth], || {
                record(5);
                42
            })
        };
        assert_eq!(status, 42);
        assert_eq!(trace(), [1, 2, 3, 4, 5]);

        assert_eq!(unsafe { run_main(&[], &[], || 23) }, 23);
        assert!(trace().is_empty());
    }

    unsafe extern "C" fn register_during_exit(arg: *mut u8) {
        let handlers = unsafe { &*arg.cast::<ProcessHandlers>() };
        record(6);
        handlers.push(node(cxa(7, null_mut())));
    }

    #[test]
    fn exit_runs_cxa_atexit_then_fini_reverse() {
        let handlers = ProcessHandlers::new();
        let dso = std::ptr::without_provenance_mut(1);
        handlers.push(node(cxa(9, dso)));
        handlers.push(node(Handler::C(first)));
        handlers.push(node(cxa(8, dso)));
        handlers.push(node(cxa(5, null_mut())));
        handlers.push(node(Handler::C(second)));
        handlers.push(node(Handler::Cxa {
            function: register_during_exit,
            argument: std::ptr::from_ref(&handlers).cast_mut().cast(),
            dso: null_mut(),
        }));

        unsafe {
            run_handlers(|| pop(handlers.take(dso)));
            run_handlers(|| pop(handlers.take(dso)));
            run_exit(
                || pop(handlers.take(null_mut())),
                &[third, fourth],
                || record(10),
            );
            run_handlers(|| pop(handlers.take(null_mut())));
        }
        assert_eq!(trace(), [8, 9, 6, 7, 2, 5, 1, 4, 3, 10]);

        unsafe { run_exit(|| pop(handlers.take(null_mut())), &[], || record(10)) };
        assert_eq!(trace(), [10]);
    }

    #[test]
    fn thread_exit_runs_cxa_thread_atexit_before_key_dtors() {
        let mut current = HandlerList::new();
        let mut other = HandlerList::new();
        current.push(node(cxa(1, null_mut())));
        current.push(node(cxa(2, null_mut())));
        other.push(node(cxa(5, null_mut())));

        unsafe {
            run_thread_exit(
                || pop(current.take(null_mut())),
                || {
                    record(3); // pthread key destructor
                    record(4); // Rust TLS destructor reached through a pthread key
                },
            );
        }
        assert_eq!(trace(), [2, 1, 3, 4]);
        assert!(current.take(null_mut()).is_none());
        unsafe { run_handlers(|| pop(other.take(null_mut()))) };
        assert_eq!(trace(), [5]);
    }

    #[test]
    fn concurrent_registration_keeps_every_handler() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 16;
        let handlers = ProcessHandlers::new();
        let start = Barrier::new(THREADS);
        std::thread::scope(|scope| {
            for worker in 0..THREADS {
                let handlers = &handlers;
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    for entry in 0..PER_THREAD {
                        handlers.push(node(cxa(worker * PER_THREAD + entry + 1, null_mut())));
                    }
                });
            }
        });

        unsafe { run_handlers(|| pop(handlers.take(null_mut()))) };
        let mut observed = trace();
        for worker in 0..THREADS {
            let first = worker * PER_THREAD + 1;
            let last = first + PER_THREAD;
            assert!(observed
                .iter()
                .copied()
                .filter(|n| (first..last).contains(n))
                .eq((first..last).rev()));
        }
        observed.sort_unstable();
        assert!(observed.into_iter().eq(1..=THREADS * PER_THREAD));
    }
}
