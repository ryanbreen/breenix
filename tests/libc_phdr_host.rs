//! Run with `rustc --edition=2021 --test tests/libc_phdr_host.rs`.

#[path = "../libs/libbreenix-libc/src/link.rs"]
mod link;

mod crt {
    use std::cell::Cell;

    thread_local! {
        pub(super) static AUXV: Cell<[u64; 8]> = const { Cell::new([0; 8]) };
    }

    pub(crate) fn auxv_entry(kind: u64) -> Option<u64> {
        AUXV.get()
            .chunks_exact(2)
            .take_while(|pair| pair[0] != 0)
            .find(|pair| pair[0] == kind)
            .map(|pair| pair[1])
    }
}

mod pthread {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::ptr::null_mut;

    thread_local! {
        pub(super) static TLS: Cell<*mut c_void> = const { Cell::new(null_mut()) };
    }

    pub(crate) fn static_tls_data() -> *mut c_void {
        TLS.get()
    }
}

mod libc_host {
    use super::link::{dl_iterate_phdr, DlPhdrInfo};
    use std::ffi::{c_void, CStr};
    use std::ptr;

    #[derive(Default)]
    struct Call {
        result: i32,
        count: usize,
        info: Option<DlPhdrInfo>,
        size: usize,
    }

    unsafe extern "C" fn visit(info: *mut DlPhdrInfo, size: usize, data: *mut c_void) -> i32 {
        // SAFETY: run passes a live Call and the iterator passes a live record.
        let call = unsafe { &mut *data.cast::<Call>() };
        call.count += 1;
        // SAFETY: the callback may read the record for the duration of the call.
        call.info = Some(unsafe { info.read() });
        call.size = size;
        call.result
    }

    fn run(headers: &[[u64; 7]], tls: *mut u8, result: i32) -> Call {
        super::crt::AUXV.set([
            5,
            headers.len() as u64,
            3,
            headers.as_ptr() as u64,
            4,
            56,
            0,
            0,
        ]);
        super::pthread::TLS.set(tls.cast());
        let mut call = Call {
            result,
            ..Call::default()
        };

        // SAFETY: the synthetic auxiliary vector names the live ELF64 headers,
        // and visit uses the context pointer only during the call.
        let returned = unsafe { dl_iterate_phdr(visit, ptr::from_mut(&mut call).cast()) };
        assert_eq!(returned, if headers.is_empty() { 0 } else { result });
        call
    }

    #[test]
    fn dl_iterate_phdr_reports_auxv_headers() {
        // The gABI gives Elf64_Phdr seven 8-byte words, starting with
        // p_type and p_flags. The TLS header has p_filesz=0 and p_memsz=48.
        let headers = [
            [1, 0, 0x4000_0000, 0, 4096, 4096, 4096],
            [0x6474_e550, 900, 0x4000_0384, 0, 16, 16, 4],
            [7, 4096, 0x4000_1000, 0, 0, 48, 32],
        ];
        let mut first_tls = [0u8; 48];
        let mut second_tls = [0u8; 48];

        for (tls, result) in [
            (first_tls.as_mut_ptr(), 0),
            (second_tls.as_mut_ptr(), -23),
            (ptr::null_mut(), 7),
        ] {
            let call = run(&headers, tls, result);
            assert_eq!(call.count, 1);
            assert_eq!(call.size, 64);
            let info = call.info.unwrap();
            assert_eq!(info.dlpi_addr, 0);
            // SAFETY: dlpi_name for the executable is a static C string.
            assert_eq!(unsafe { CStr::from_ptr(info.dlpi_name) }, c"");
            assert_eq!(info.dlpi_phdr.cast::<[u64; 7]>(), headers.as_ptr());
            assert_eq!(info.dlpi_phnum, 3);
            assert_eq!((info.dlpi_adds, info.dlpi_subs), (1, 0));
            assert_eq!(info.dlpi_tls_modid, 1);
            assert_eq!(info.dlpi_tls_data, tls.cast());
        }

        let no_tls = run(&headers[..2], first_tls.as_mut_ptr(), 0).info.unwrap();
        assert_eq!(no_tls.dlpi_tls_modid, 0);
        assert!(no_tls.dlpi_tls_data.is_null());
        assert_eq!(run(&[], ptr::null_mut(), 7).count, 0);
    }
}
