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
    files: BTreeMap<i32, (Vec<u8>, usize)>,
    allocations: BTreeMap<usize, Layout>,
    fail_after: Option<usize>,
}
impl Default for State {
    fn default() -> Self {
        Self { errno: 0, hosts: b"127.0.0.1 localhost\n192.0.2.7 canonical.test alias.test # comment\n192.0.2.8 canonical.test alias.test\n".to_vec(), files: BTreeMap::new(), allocations: BTreeMap::new(), fail_after: None }
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
    assert_eq!(flags, 0);
    assert_eq!(mode, 0);
    let text = std::ffi::CStr::from_ptr(path.cast()).to_bytes();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let data = match text {
            b"/etc/hosts" => s.hosts.clone(),
            b"/etc/services" => b"http 80/tcp www\ndomain 53/udp\n".to_vec(),
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
        assert_eq!(lookup(b"127.0.0.1\0", b"", &h).0, -8);
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
