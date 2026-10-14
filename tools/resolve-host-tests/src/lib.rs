//! Execute production address/resolver code with explicit user-copy and file shims.
#![cfg(test)]
extern crate alloc;
use std::{
    alloc::{alloc_zeroed, dealloc, Layout},
    cell::RefCell,
    collections::BTreeMap,
};
#[path = "../../../kernel/src/socket/addr.rs"]
pub mod addr;
#[path = "../../../libs/libbreenix-libc/src/inet.rs"]
pub mod inet;
#[path = "../../../libs/libbreenix-libc/src/netdb.rs"]
pub mod netdb;

pub mod syscall {
    pub mod errno {
        pub const EINVAL: i32 = 22;
        pub const EAFNOSUPPORT: i32 = 97;
        pub const ENOMEM: i32 = 12;
        pub const EFAULT: i32 = 14;
    }
    pub mod userptr {
        // These shims validate the copy boundary's results, not kernel page permissions.
        pub fn read_user_bytes(dst: *mut u8, src: u64, len: usize) -> Result<(), u64> {
            if src == 0 {
                return Err(14);
            }
            unsafe {
                core::ptr::copy_nonoverlapping(src as *const u8, dst, len);
            }
            Ok(())
        }
        pub fn write_user_prefix(dst: u64, src: *const u8, len: usize) -> usize {
            if dst == 0 {
                return 0;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(src, dst as *mut u8, len);
            }
            len
        }
        pub fn copy_from_user<T: Copy>(ptr: *const T) -> Result<T, u64> {
            if ptr.is_null() {
                Err(14)
            } else {
                Ok(unsafe { ptr.read_unaligned() })
            }
        }
        pub fn copy_to_user<T: Copy>(ptr: *mut T, val: &T) -> Result<(), u64> {
            if ptr.is_null() {
                Err(14)
            } else {
                unsafe {
                    ptr.write_unaligned(*val);
                }
                Ok(())
            }
        }
    }
}
struct State {
    errno: i32,
    hosts: Vec<u8>,
    services: Vec<u8>,
    hosts_errno: Option<i32>,
    service_opens: usize,
    files: BTreeMap<i32, (Vec<u8>, usize)>,
    allocations: BTreeMap<usize, Layout>,
    fail_after: Option<usize>,
}
impl Default for State {
    fn default() -> Self {
        Self { services: b"http 80/tcp www\ndomain 53/udp\n".to_vec(), hosts_errno: None, service_opens: 0, errno: 0, hosts: b"127.0.0.1 localhost\n192.0.2.7 canonical.test alias.test # comment\n192.0.2.8 canonical.test alias.test\n".to_vec(), files: BTreeMap::new(), allocations: BTreeMap::new(), fail_after: None }
    }
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
#[repr(C)]
pub struct utsname {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}
pub unsafe fn uname(identity: *mut utsname) -> i32 {
    let identity = &mut *identity;
    identity.nodename[..20].copy_from_slice(b"breenix.example.test");
    0
}
pub fn __errno_location() -> *mut i32 {
    STATE.with(|s| &mut s.borrow_mut().errno as *mut i32)
}
pub unsafe fn open(path: *const u8, flags: i32, mode: u32) -> i32 {
    assert_eq!(flags, 0x80000);
    assert_eq!(mode, 0);
    let text = std::ffi::CStr::from_ptr(path.cast()).to_bytes();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let data = match text {
            b"/etc/hosts" => {
                if let Some(e) = s.hosts_errno {
                    s.errno = e;
                    return -1;
                }
                s.hosts.clone()
            }
            b"/etc/services" => {
                s.service_opens += 1;
                s.services.clone()
            }
            b"/etc/resolv.conf" => b"nameserver 10.0.2.3\n".to_vec(),
            _ => {
                s.errno = 2;
                return -1;
            }
        };
        let fd = (3..).find(|fd| !s.files.contains_key(fd)).unwrap();
        s.files.insert(fd, (data, 0));
        fd
    })
}
pub unsafe fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let (data, offset) = s.files.get_mut(&fd).unwrap();
        let n = count.min(data.len() - *offset);
        std::ptr::copy_nonoverlapping(data.as_ptr().add(*offset), buf, n);
        *offset += n;
        n as isize
    })
}
pub fn close(fd: i32) -> i32 {
    STATE.with(|s| {
        assert!(s.borrow_mut().files.remove(&fd).is_some());
    });
    0
}
pub unsafe fn calloc(count: usize, size: usize) -> *mut u8 {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(left) = &mut s.fail_after {
            if *left == 0 {
                return std::ptr::null_mut();
            }
            *left -= 1;
        }
        let layout = Layout::from_size_align(count.checked_mul(size).unwrap(), 16).unwrap();
        let p = alloc_zeroed(layout);
        if !p.is_null() {
            s.allocations.insert(p as usize, layout);
        }
        p
    })
}
pub unsafe fn free(p: *mut u8) {
    if !p.is_null() {
        STATE.with(|s| {
            let layout = s.borrow_mut().allocations.remove(&(p as usize)).unwrap();
            dealloc(p, layout);
        });
    }
}
fn hints(kind: i32, flags: i32) -> netdb::AddrInfo {
    netdb::AddrInfo {
        flags,
        family: 2,
        socktype: kind,
        protocol: 0,
        addrlen: 0,
        addr: std::ptr::null_mut(),
        canonname: std::ptr::null_mut(),
        next: std::ptr::null_mut(),
    }
}
unsafe fn lookup(node: &[u8], service: &[u8], h: &netdb::AddrInfo) -> (i32, *mut netdb::AddrInfo) {
    let mut out = std::ptr::null_mut();
    (
        netdb::getaddrinfo(
            if node.is_empty() {
                std::ptr::null()
            } else {
                node.as_ptr()
            },
            if service.is_empty() {
                std::ptr::null()
            } else {
                service.as_ptr()
            },
            h,
            &mut out,
        ),
        out,
    )
}
#[test]
fn numeric_list_has_linux_layout_and_two_transports() {
    unsafe {
        assert_eq!(std::mem::size_of::<netdb::AddrInfo>(), 48);
        let (code, p) = lookup(b"192.0.2.7\0", b"8080\0", &hints(0, 0));
        assert_eq!(code, 0);
        let q = (*p).next;
        assert!(!q.is_null());
        assert!((*q).next.is_null());
        assert_eq!(((*p).socktype, (*p).protocol), (1, 6));
        assert_eq!(((*q).socktype, (*q).protocol), (2, 17));
        assert_eq!(
            std::slice::from_raw_parts((*p).addr, 16),
            &[2, 0, 0x1f, 0x90, 192, 0, 2, 7, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        netdb::freeaddrinfo(p);
        STATE.with(|s| assert!(s.borrow().allocations.is_empty()));
    }
}
#[test]
fn validation_and_reserved_names_return_errors_without_results() {
    unsafe {
        for (h, node, serv, expected) in [
            (hints(1, 0), b"".as_slice(), b"".as_slice(), -2),
            (hints(1, 4), b"localhost\0", b"", -2),
            (hints(1, 0x400), b"127.0.0.1\0", b"http\0", -2),
            (hints(12345, 0), b"127.0.0.1\0", b"", -7),
            (hints(1, 0x10000000), b"127.0.0.1\0", b"", -1),
            (hints(1, 0), b"NO-SUCH-HOST.INVALID.\0", b"", -2),
        ] {
            let (code, p) = lookup(node, serv, &h);
            assert_eq!(code, expected);
            assert!(p.is_null());
        }
        let mut h = hints(1, 0);
        h.family = 12345;
        assert_eq!(lookup(b"127.0.0.1\0", b"", &h).0, -6);
        h.family = 2;
        h.protocol = 17;
        assert_eq!(lookup(b"127.0.0.1\0", b"", &h).0, -7);
    }
}
#[test]
fn passive_loopback_and_named_service() {
    unsafe {
        for (flags, ip) in [(1, [0; 4]), (0, [127, 0, 0, 1])] {
            let (code, p) = lookup(b"", b"80\0", &hints(1, flags));
            assert_eq!(code, 0);
            assert_eq!(std::slice::from_raw_parts((*p).addr.add(4), 4), &ip);
            netdb::freeaddrinfo(p);
        }
        let (code, p) = lookup(b"127.0.0.1\0", b"www\0", &hints(0, 0));
        assert_eq!(code, 0);
        assert_eq!(std::slice::from_raw_parts((*p).addr.add(2), 2), &[0, 80]);
        assert!((*p).next.is_null());
        netdb::freeaddrinfo(p);
    }
}
#[test]
fn hosts_alias_multiple_addresses_canon_and_reverse() {
    unsafe {
        let (code, p) = lookup(b"ALIAS.TEST\0", b"", &hints(1, 2));
        assert_eq!(code, 0);
        assert_eq!(
            std::ffi::CStr::from_ptr((*p).canonname.cast()).to_bytes(),
            b"canonical.test"
        );
        let q = (*p).next;
        assert!(!q.is_null());
        assert!((*q).canonname.is_null());
        let mut host = [0; 256];
        let mut serv = [0; 32];
        assert_eq!(
            netdb::getnameinfo(
                (*q).addr,
                16,
                host.as_mut_ptr(),
                256,
                serv.as_mut_ptr(),
                32,
                8 | 2
            ),
            0
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(host.as_ptr().cast()).to_bytes(),
            b"canonical.test"
        );
        netdb::freeaddrinfo(p);
    }
}
#[test]
fn allocation_failure_frees_partial_list() {
    unsafe {
        STATE.with(|s| s.borrow_mut().fail_after = Some(1));
        let (code, p) = lookup(b"127.0.0.1\0", b"80\0", &hints(0, 0));
        assert_eq!(code, -10);
        assert!(p.is_null());
        STATE.with(|s| {
            let s = s.borrow();
            assert!(s.allocations.is_empty());
            assert!(s.files.is_empty());
        });
    }
}
#[test]
fn reverse_bounds_family_and_unknown_name() {
    unsafe {
        let addr = [2, 0, 0, 80, 127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut host = [0xcc; 16];
        let mut serv = [0; 32];
        assert_eq!(
            netdb::getnameinfo(
                addr.as_ptr(),
                16,
                host.as_mut_ptr(),
                16,
                serv.as_mut_ptr(),
                32,
                3
            ),
            0
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(host.as_ptr().cast()).to_bytes(),
            b"127.0.0.1"
        );
        host.fill(0xcc);
        assert_eq!(
            netdb::getnameinfo(
                addr.as_ptr(),
                16,
                host.as_mut_ptr(),
                4,
                serv.as_mut_ptr(),
                32,
                3
            ),
            -12
        );
        assert_eq!(host, [0xcc; 16]);
        assert_eq!(
            netdb::getnameinfo(
                addr.as_ptr(),
                2,
                host.as_mut_ptr(),
                16,
                serv.as_mut_ptr(),
                32,
                3
            ),
            -6
        );
        let mut unknown = addr;
        unknown[4..8].copy_from_slice(&[192, 0, 2, 99]);
        assert_eq!(
            netdb::getnameinfo(
                unknown.as_ptr(),
                16,
                host.as_mut_ptr(),
                16,
                serv.as_mut_ptr(),
                32,
                8
            ),
            -2
        );
    }
}
#[test]
fn inet_errno_and_untouched_output() {
    unsafe {
        let mut out = [0xccu8; 16];
        assert_eq!(
            inet::inet_pton(2, b"1.2.3.4x\0".as_ptr(), out.as_mut_ptr()),
            0
        );
        assert_eq!(out, [0xcc; 16]);
        assert_eq!(
            inet::inet_pton(12345, b"1.2.3.4\0".as_ptr(), out.as_mut_ptr()),
            -1
        );
        assert_eq!(*__errno_location(), 97);
        assert!(inet::inet_ntop(2, [127, 0, 0, 1].as_ptr(), out.as_mut_ptr(), 8).is_null());
        assert_eq!(*__errno_location(), 28);
        assert_eq!(out, [0xcc; 16]);
    }
}
#[test]
fn address_prefix_and_checked_copy_errors() {
    let a = addr::SockAddr::Inet {
        addr: [127, 0, 0, 1],
        port: 80,
    };
    let mut out = [0xccu8; 16];
    let mut n = 4u32;
    addr::write_sockaddr(&a, out.as_mut_ptr() as u64, &mut n as *mut u32 as u64).unwrap();
    assert_eq!(n, 16);
    assert_eq!(&out[..4], &[2, 0, 0, 80]);
    assert_eq!(&out[4..], &[0xcc; 12]);
    assert_eq!(addr::read_sockaddr(0, 16, 2), Err(14));
    assert_eq!(addr::write_sockaddr(&a, 0, 0), Err(14));
}

#[test]
fn nofqdn_only_shortens_the_local_domain_and_hosts_are_not_cached() {
    unsafe {
        STATE.with(|s| {
            s.borrow_mut().hosts = b"192.0.2.7 node.example.test
192.0.2.8 remote.other.test
"
            .to_vec()
        });
        for (ip, expected) in [
            (7, b"node".as_slice()),
            (8, b"remote.other.test".as_slice()),
        ] {
            let addr = [2, 0, 0, 80, 192, 0, 2, ip, 0, 0, 0, 0, 0, 0, 0, 0];
            let mut out = [0; 256];
            assert_eq!(
                netdb::getnameinfo(
                    addr.as_ptr(),
                    16,
                    out.as_mut_ptr(),
                    256,
                    std::ptr::null_mut(),
                    0,
                    4
                ),
                0
            );
            assert_eq!(
                std::ffi::CStr::from_ptr(out.as_ptr().cast()).to_bytes(),
                expected
            );
        }
        let (code, p) = lookup(b"node.example.test\0", b"", &hints(1, 0));
        assert_eq!(code, 0);
        netdb::freeaddrinfo(p);
        STATE.with(|s| s.borrow_mut().hosts = b"192.0.2.99 node.example.test".to_vec());
        let (code, p) = lookup(b"node.example.test\0", b"", &hints(1, 0));
        assert_eq!(code, 0);
        assert_eq!(
            std::slice::from_raw_parts((*p).addr.add(4), 4),
            &[192, 0, 2, 99]
        );
        netdb::freeaddrinfo(p);
    }
}

#[test]
fn legacy_numeric_forms_keep_inet_pton_strict() {
    for (name, ip) in [
        (b"127.1\0".as_slice(), [127, 0, 0, 1]),
        (b"0x7f.0.0.1\0", [127, 0, 0, 1]),
        (b"010.0.0.1\0", [8, 0, 0, 1]),
    ] {
        unsafe {
            let (code, p) = lookup(name, b"80\0", &hints(1, 4));
            assert_eq!(code, 0);
            assert_eq!(std::slice::from_raw_parts((*p).addr.add(4), 4), ip);
            netdb::freeaddrinfo(p);
            let mut addr = [0; 4];
            assert_eq!(inet::inet_pton(2, name.as_ptr(), addr.as_mut_ptr()), 0);
        }
    }
}
#[test]
fn validation_precedence_protocols_and_address_bounds() {
    unsafe {
        let mut p = std::ptr::null_mut();
        assert_eq!(
            netdb::getaddrinfo(
                std::ptr::null(),
                std::ptr::null(),
                &hints(1, 0x8000),
                &mut p
            ),
            -2
        );
        let mut h = hints(1, 0);
        h.protocol = 17;
        assert_eq!(lookup(b"127.0.0.1\0", b"80\0", &h).0, -7);
        h.protocol = 132;
        let (code, p) = lookup(b"127.0.0.1\0", b"80\0", &h);
        assert_eq!(code, 0);
        assert_eq!((*p).protocol, 132);
        netdb::freeaddrinfo(p);
    }
    let bytes = [0u8; 128];
    assert_eq!(addr::read_sockaddr(bytes.as_ptr() as u64, 129, 2), Err(22));
    for len in 2..16 {
        assert_eq!(
            addr::read_sockaddr(bytes.as_ptr() as u64, len, 2),
            Ok(addr::SockAddr::Unspec)
        );
    }
    assert_eq!(
        addr::read_sockaddr(bytes.as_ptr() as u64, 2, 1),
        Ok(addr::SockAddr::Unspec)
    );
    let mut out = [0u8; 16];
    let mut len = -1i32;
    assert_eq!(
        addr::write_sockaddr(
            &addr::SockAddr::Inet {
                addr: [0; 4],
                port: 80
            },
            out.as_mut_ptr() as u64,
            &mut len as *mut i32 as u64
        ),
        Err(22)
    );
}

#[test]
fn services_are_first_match_and_read_once_for_multiple_addresses() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.services = b"http 80/tcp www\nhttp 81/tcp\nhttp bad/tcp\nhttp 82/udp\n".to_vec();
        s.service_opens = 0;
    });
    unsafe {
        let (code, p) = lookup(b"alias.test\0", b"http\0", &hints(0, 0));
        assert_eq!(code, 0);
        let mut at = p;
        let mut count = 0;
        while !at.is_null() {
            let bytes = std::slice::from_raw_parts((*at).addr.add(2), 2);
            assert_eq!(
                u16::from_be_bytes([bytes[0], bytes[1]]),
                if (*at).socktype == 1 { 80 } else { 82 }
            );
            count += 1;
            at = (*at).next;
        }
        assert_eq!(count, 4);
        netdb::freeaddrinfo(p);
    }
    STATE.with(|s| assert_eq!(s.borrow().service_opens, 1));
}
#[test]
fn oversized_hosts_name_falls_back_and_unreadable_hosts_do_not_fail() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.hosts = b"192.0.2.7 ".to_vec();
        s.hosts.extend_from_slice(&[b'a'; 256]);
        s.hosts.extend_from_slice(b" short.test\n");
    });
    unsafe {
        let (code, p) = lookup(b"short.test\0", b"80\0", &hints(1, 2));
        assert_eq!(code, 0);
        assert_eq!(
            std::ffi::CStr::from_ptr((*p).canonname.cast()).to_bytes(),
            b"short.test"
        );
        let mut text = [0u8; 32];
        assert_eq!(
            netdb::getnameinfo(
                (*p).addr,
                16,
                text.as_mut_ptr(),
                32,
                std::ptr::null_mut(),
                0,
                0
            ),
            0
        );
        assert_eq!(
            std::ffi::CStr::from_ptr(text.as_ptr().cast()).to_bytes(),
            b"192.0.2.7"
        );
        netdb::freeaddrinfo(p);
        STATE.with(|s| s.borrow_mut().hosts_errno = Some(13));
        let (code, p) = lookup(b"localhost\0", b"80\0", &hints(1, 0));
        assert_eq!(code, 0);
        netdb::freeaddrinfo(p);
    }
}

// pthread-specific storage shims for the production inet_ntoa implementation.
pub mod pthread {
    use std::{cell::{Cell, UnsafeCell}, sync::Once};
    static ONCE: Once = Once::new();
    thread_local! { static H_ERRNO: UnsafeCell<i32> = const { UnsafeCell::new(0) }; }
    pub fn __h_errno_location() -> *mut i32 { H_ERRNO.with(|value| value.get()) }
    thread_local! { static VALUE: Cell<*mut u8> = const { Cell::new(core::ptr::null_mut()) }; }
    pub unsafe fn pthread_once(_p: *mut i32, init: extern "C" fn()) -> i32 {
        ONCE.call_once(|| init());
        0
    }
    pub unsafe fn pthread_key_create(key: *mut u32, _destructor: Option<unsafe extern "C" fn(*mut u8)>) -> i32 {
        *key = 0;
        0
    }
    pub unsafe fn pthread_getspecific(_key: u32) -> *mut u8 { VALUE.with(Cell::get) }
    pub unsafe fn pthread_setspecific(_key: u32, value: *mut u8) -> i32 {
        VALUE.with(|slot| slot.set(value));
        0
    }
    pub unsafe fn cleanup() {
        VALUE.with(|slot| crate::free(slot.replace(core::ptr::null_mut())));
    }
}

#[test]
fn inet_ntoa_preserves_other_threads_buffers() {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (1..=8).map(|last| {
        let barrier = barrier.clone();
        std::thread::spawn(move || unsafe {
            let address = [192, 0, 2, last];
            let p = inet::inet_ntoa(u32::from_ne_bytes(address));
            assert!(!p.is_null());
            barrier.wait();
            assert_eq!(std::ffi::CStr::from_ptr(p.cast()).to_bytes(), format!("192.0.2.{last}").as_bytes());
            assert_eq!(inet::inet_ntoa(u32::from_ne_bytes([127, 0, 0, 1])), p);
            assert_eq!(std::ffi::CStr::from_ptr(p.cast()).to_bytes(), b"127.0.0.1");
            pthread::cleanup();
        })
    }).collect();
    for thread in threads { thread.join().unwrap(); }
}

#[path = "../../../libs/libbreenix-libc/src/string/errtext.rs"]
mod errtext;
#[test]
fn network_errors_have_descriptions() {
    for error in [64, 94, 112] {
        assert!(errtext::error_text(error).is_some());
        assert!(errtext::error_cstr(error).unwrap().ends_with('\0'));
    }
    assert!(errtext::signal_text(13).is_some());
}
