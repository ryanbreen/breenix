//! Run with rustc --edition=2021 --test tests/libc_dl_host.rs.

#[path = "../libs/libbreenix-libc/src/dl_abi.rs"]
pub mod dl_abi;
#[path = "../libs/libbreenix-libc/src/link_hosted.rs"]
mod link_hosted;

mod libc_host {
    use crate::dl_abi::{LoadedObject, ObjectInfo};
    use crate::link_hosted::{visit_objects, DlPhdrInfo};
    use std::ffi::{c_void, CStr};
    use std::ptr;

    #[derive(Default)]
    struct Visit {
        seen: Vec<(u64, &'static CStr, usize, u16, u64, u64)>,
        stop: usize,
    }

    unsafe extern "C" fn visit(info: *mut DlPhdrInfo, size: usize, data: *mut c_void) -> i32 {
        let visited = unsafe { &mut *data.cast::<Visit>() };
        let info = unsafe { &*info };
        assert_eq!(size, 80);
        visited.seen.push((
            info.dlpi_addr,
            unsafe { CStr::from_ptr(info.dlpi_name) },
            info.dlpi_phdr as usize,
            info.dlpi_phnum,
            info.text_base,
            info.got_base,
        ));
        if visited.seen.len() == visited.stop {
            23
        } else {
            0
        }
    }

    #[test]
    fn dl_iterate_phdr_lists_every_object() {
        let headers = [[0u64; 7]; 3];
        let names = [c"", c"/lib/driver.so", c"/lib/libc.so"];
        let biases = [0, 0x8000_0000, 0xa000_0000];
        let records: [ObjectInfo; 3] = std::array::from_fn(|i| ObjectInfo {
            object: LoadedObject {
                name: names[i].as_ptr(),
                bias: biases[i],
                phdr: headers[i].as_ptr().cast(),
                phnum: 1,
                dynamic: ptr::null(),
                tls_module_id: 0,
            },
            text_base: biases[i] + 0x1000,
            got_base: biases[i] + 0x7000,
        });
        let expected: Vec<_> = (0..3)
            .map(|i| {
                (
                    biases[i],
                    names[i],
                    headers[i].as_ptr() as usize,
                    1,
                    biases[i] + 0x1000,
                    biases[i] + 0x7000,
                )
            })
            .collect();
        for (stop, count, result) in [(0, 3, 0), (2, 2, 23)] {
            let mut visited = Visit {
                stop,
                ..Visit::default()
            };
            let returned = unsafe {
                visit_objects(
                    records.len(),
                    |i| Some(records[i]),
                    visit,
                    ptr::from_mut(&mut visited).cast(),
                )
            };
            assert_eq!(returned, result);
            assert_eq!(visited.seen, expected[..count]);
        }
    }
}
