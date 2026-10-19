//! The virtual-terminal and KD requests on a virtual terminal's descriptor
//! (`docs/console/contract.md` section 4.1). Numbers and layouts are the
//! published `linux/vt.h` and `linux/kd.h`; the behaviour is the contract's.
//!
//! "The descriptor's VT" is resolved before dispatch: VT *n* for `/dev/ttyN`,
//! tty1 for `/dev/console`, the foreground for `/dev/tty0`.

use crate::console::vt::{self, Source};
use crate::syscall::errno::{EFAULT, EINVAL, ENOTTY, EPERM, ESRCH};
use crate::syscall::userptr::{copy_from_user, copy_to_user};

pub const VT_OPENQRY: u64 = 0x5600;
pub const VT_GETMODE: u64 = 0x5601;
pub const VT_SETMODE: u64 = 0x5602;
pub const VT_GETSTATE: u64 = 0x5603;
pub const VT_RELDISP: u64 = 0x5605;
pub const VT_ACTIVATE: u64 = 0x5606;
pub const VT_WAITACTIVE: u64 = 0x5607;
pub const KDSETMODE: u64 = 0x4B3A;
pub const KDGETMODE: u64 = 0x4B3B;
/// Breenix's own request: move the terminal's scrollback view (contract 1.6).
pub const VT_SCROLLVIEW: u64 = 0x56B0;

const KD_TEXT: u64 = 0;
const KD_GRAPHICS: u64 = 1;

/// Whether `request` is one of this module's.
pub fn handles(request: u64) -> bool {
    matches!(
        request,
        VT_OPENQRY | VT_GETMODE | VT_SETMODE | VT_GETSTATE | VT_RELDISP | VT_ACTIVATE | VT_WAITACTIVE | KDSETMODE
            | KDGETMODE | VT_SCROLLVIEW
    )
}

/// The calling process as these requests see it.
struct Caller {
    /// Its process (the thread group's ID).
    pid: u64,
    root: bool,
    /// Its controlling terminal when that is a virtual terminal (1-based).
    ctty_vt: Option<usize>,
    /// Its command is `chvt`, which names its switches so.
    chvt: bool,
}

fn caller() -> Result<Caller, i32> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH)?;
    let guard = crate::process::manager();
    let pm = guard.as_ref().ok_or(ESRCH)?;
    let (pid, row) = pm.find_process_by_thread(tid).ok_or(ESRCH)?;
    let group = pm.thread_group_of(pid).unwrap_or(pid.as_u64());
    Ok(Caller {
        pid: group,
        root: row.cred.euid == 0,
        ctty_vt: match row.ctty {
            Some(super::ctty::TtyId::Vt(n)) => Some(usize::from(n)),
            _ => None,
        },
        chvt: row.name.rsplit('/').next() == Some("chvt"),
    })
}

/// The changing requests need root or a virtual terminal as the controlling
/// terminal; with `own`, without root, that terminal must be `vt`.
fn permit(caller: &Caller, vt: usize, own: bool) -> Result<(), i32> {
    match caller.ctty_vt {
        _ if caller.root => Ok(()),
        Some(ctty) if !own || ctty == vt => Ok(()),
        _ => Err(EPERM),
    }
}

/// The terminals some process has open: bit *n* for VT *n*. A descriptor for
/// `/dev/ttyN`, for `/dev/console` (or the console's standard streams) or for
/// `/dev/tty` resolved to a terminal counts; `/dev/tty0` and the screens do not.
fn open_terminals() -> u16 {
    use crate::fs::devfs::DeviceType;
    use crate::ipc::fd::FdKind;
    let guard = crate::process::manager();
    let Some(pm) = guard.as_ref() else {
        return 0;
    };
    let mut open = 0u16;
    for (_, row) in pm.iter_processes() {
        for kind in row.fd_table.kinds() {
            match kind {
                FdKind::Device(DeviceType::Console | DeviceType::Tty) | FdKind::StdIo(_) => open |= 1 << 1,
                FdKind::Device(DeviceType::Vt(n)) => open |= 1 << n,
                _ => {}
            }
        }
    }
    open
}

/// Run request `request` on virtual terminal `vt` (1-based).
pub fn ioctl(vt: usize, request: u64, arg: u64) -> Result<i32, i32> {
    match request {
        VT_OPENQRY => {
            let open = open_terminals();
            let free = (1..=vt::VT_COUNT).find(|n| open & (1 << n) == 0).map_or(-1, |n| n as i32);
            copy_to_user(arg as *mut i32, &free).map_err(|_| EFAULT)?;
            Ok(0)
        }
        VT_GETSTATE => {
            let state = [vt::foreground() as u16, 0, open_terminals() & !1];
            copy_to_user(arg as *mut [u16; 3], &state).map_err(|_| EFAULT)?;
            Ok(0)
        }
        VT_GETMODE => {
            let (mode, relsig, acqsig) = vt::mode(vt);
            let mut bytes = [0u8; 8];
            bytes[0] = mode;
            bytes[2..4].copy_from_slice(&relsig.to_le_bytes());
            bytes[4..6].copy_from_slice(&acqsig.to_le_bytes());
            copy_to_user(arg as *mut [u8; 8], &bytes).map_err(|_| EFAULT)?;
            Ok(0)
        }
        VT_SETMODE => {
            let caller = caller()?;
            permit(&caller, vt, true)?;
            let bytes: [u8; 8] = copy_from_user(arg as *const [u8; 8]).map_err(|_| EFAULT)?;
            let relsig = i16::from_le_bytes([bytes[2], bytes[3]]);
            let acqsig = i16::from_le_bytes([bytes[4], bytes[5]]);
            vt::set_mode(vt, caller.pid, bytes[0], relsig, acqsig).map(|()| 0)
        }
        VT_RELDISP => vt::reldisp(vt, caller()?.pid, arg).map(|()| 0),
        VT_ACTIVATE => {
            let caller = caller()?;
            permit(&caller, vt, false)?;
            let source = if caller.chvt { Source::Chvt } else { Source::Ioctl };
            vt::activate(arg, source).map(|()| 0)
        }
        VT_WAITACTIVE => vt::wait_active(arg).map(|()| 0),
        KDGETMODE => {
            let splash = vt == 1 && crate::console::splash::is_up() && crate::console::owns_screen();
            let mode = if splash || vt::kd_graphics(vt) { KD_GRAPHICS as i32 } else { KD_TEXT as i32 };
            copy_to_user(arg as *mut i32, &mode).map_err(|_| EFAULT)?;
            Ok(0)
        }
        KDSETMODE => {
            let caller = caller()?;
            permit(&caller, vt, true)?;
            let graphics = match arg {
                KD_TEXT => false,
                KD_GRAPHICS => true,
                _ => return Err(EINVAL),
            };
            // The display owner's own KD_TEXT gives the display back.
            if !graphics && vt::display_owner() == Some((caller.pid, vt)) {
                return match crate::syscall::handlers::sys_give_back_display() {
                    crate::syscall::SyscallResult::Ok(_) => Ok(0),
                    crate::syscall::SyscallResult::Err(errno) => Err(errno as i32),
                };
            }
            vt::set_kd(vt, caller.pid, graphics)?;
            // tty1's splash gives way to either mode (contract section 7).
            if vt == 1 {
                use crate::console::splash::{request, Reason};
                request(if graphics { Reason::DisplayTaken } else { Reason::KdText });
            }
            Ok(0)
        }
        VT_SCROLLVIEW => {
            let caller = caller()?;
            permit(&caller, vt, false)?;
            Ok(vt::scroll_view(vt, arg as i64 as isize) as i32)
        }
        _ => Err(ENOTTY),
    }
}
