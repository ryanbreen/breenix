//! Run with `rustc --edition=2021 --test tests/libc_tls_host.rs`.

#[path = "../libs/libbreenix-libc/src/pthread/hosted_tls.rs"]
mod hosted_tls;
#[path = "../libs/libbreenix-libc/src/tls_module.rs"]
mod tls_module;

mod stdlib {
    pub mod malloc {
        use std::alloc::{alloc, dealloc, Layout};
        use std::collections::BTreeMap;
        use std::sync::Mutex;

        pub static LIVE: Mutex<BTreeMap<usize, Layout>> = Mutex::new(BTreeMap::new());

        pub unsafe fn posix_memalign(out: *mut *mut u8, alignment: usize, size: usize) -> i32 {
            let layout = Layout::from_size_align(size, alignment).unwrap();
            let block = unsafe { alloc(layout) };
            assert!(!block.is_null());
            // Ensure the TLS implementation must initialize both tdata and tbss.
            unsafe { block.write_bytes(0xcd, size) };
            unsafe { out.write(block) };
            assert!(LIVE
                .lock()
                .unwrap()
                .insert(block as usize, layout)
                .is_none());
            0
        }

        pub unsafe fn free(block: *mut u8) {
            let layout = LIVE.lock().unwrap().remove(&(block as usize)).unwrap();
            unsafe { dealloc(block, layout) };
        }
    }
}

mod libc_host {
    use super::hosted_tls::{Dtv, TlsIndex};
    use super::stdlib::malloc::LIVE;
    use super::tls_module::TlsModule;
    use std::ptr;

    fn templates() -> [TlsModule; 2] {
        static TDATA: &[u8] = &[1, 2, 3, 4, 5];
        [
            TlsModule {
                image: ptr::null(),
                file_size: 0,
                mem_size: 48,
                align: 8,
            },
            TlsModule {
                image: TDATA.as_ptr(),
                file_size: TDATA.len(),
                mem_size: 16,
                align: 32,
            },
        ]
    }

    #[test]
    fn tls_get_addr_per_thread_block() {
        let driver = TlsIndex {
            module: 1,
            offset: 0,
        };
        let mut main_dtv = Dtv::new();
        let modules = templates();
        assert!(LIVE.lock().unwrap().is_empty());
        let main_block = unsafe { main_dtv.get_addr(&modules, &driver) };
        assert!(!main_block.is_null());
        assert_eq!(main_block as usize % 8, 0);
        assert_eq!(unsafe { main_block.read() }, 0);
        unsafe { main_block.write(42) };
        assert_eq!(unsafe { main_dtv.get_addr(&modules, &driver) }, main_block);

        let main_address = main_block as usize;
        let (driver_address, tdata_address) = std::thread::spawn(move || {
            let mut dtv = Dtv::new();
            let modules = templates();
            let block = unsafe { dtv.get_addr(&modules, &driver) };
            assert!(!block.is_null());
            assert_eq!(block as usize % 8, 0);
            assert_ne!(block as usize, main_address);
            assert_eq!(unsafe { std::slice::from_raw_parts(block, 48) }, [0; 48]);
            assert_eq!(LIVE.lock().unwrap()[&(block as usize)].size(), 48);

            let tdata = TlsIndex {
                module: 2,
                offset: 0,
            };
            let initialized = unsafe { dtv.get_addr(&modules, &tdata) };
            assert!(!initialized.is_null());
            assert_eq!(initialized as usize % 32, 0);
            assert_eq!(
                unsafe { std::slice::from_raw_parts(initialized, 16) },
                [1, 2, 3, 4, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            );
            let offset = TlsIndex {
                module: 2,
                offset: 4,
            };
            assert_eq!(unsafe { dtv.get_addr(&modules, &offset) }, unsafe {
                initialized.add(4)
            });
            (block as usize, initialized as usize)
        })
        .join()
        .unwrap();
        let live = LIVE.lock().unwrap();
        assert!(!live.contains_key(&driver_address));
        assert!(!live.contains_key(&tdata_address));
        assert!(live.contains_key(&main_address));
        drop(live);
        assert_eq!(unsafe { main_block.read() }, 42);
        drop(main_dtv);
        assert!(LIVE.lock().unwrap().is_empty());
    }
}
