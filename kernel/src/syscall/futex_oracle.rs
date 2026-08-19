//! Deterministic boot-test oracle for futex wait/wake handoff regressions.

#![cfg(feature = "boot_tests")]

use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    S1,
    S2,
    S3,
}

#[derive(Clone, Copy)]
pub enum OracleRet {
    Zero,
    Eagain,
    Eintr,
    Etimedout,
    Efault,
    Einval,
    Esrch,
    Rescued,
    Other,
}

const STAGE1_SENTINEL: u32 = 0x4655_5831;
const STAGE2_SENTINEL: u32 = 0x4655_5832;
const STAGE3_SENTINEL: u32 = 0x4655_5833;
const REPORT_SENTINEL: u32 = 0x4655_5852;
const BACKSTOP_NS: u64 = 1_000_000_000;

static STAGE1_TG_ID: AtomicU64 = AtomicU64::new(0);
static STAGE1_UADDR: AtomicU64 = AtomicU64::new(0);
static STAGE1_ARM_NS: AtomicU64 = AtomicU64::new(0);
static STAGE1_WAKE: AtomicU64 = AtomicU64::new(0);
static STAGE1_PARKED: AtomicU64 = AtomicU64::new(0);
static STAGE1_ENQUEUED: AtomicU64 = AtomicU64::new(0);
static STAGE1_LEFT: AtomicU64 = AtomicU64::new(0);
static STAGE1_RET: AtomicU64 = AtomicU64::new(8);
static STAGE1_ELAPSED_NS: AtomicU64 = AtomicU64::new(0);

static STAGE2_TG_ID: AtomicU64 = AtomicU64::new(0);
static STAGE2_UADDR: AtomicU64 = AtomicU64::new(0);
static STAGE2_ARM_NS: AtomicU64 = AtomicU64::new(0);
static STAGE2_WAKE: AtomicU64 = AtomicU64::new(0);
static STAGE2_PARKED: AtomicU64 = AtomicU64::new(0);
static STAGE2_ENQUEUED: AtomicU64 = AtomicU64::new(0);
static STAGE2_LEFT: AtomicU64 = AtomicU64::new(0);
static STAGE2_RET: AtomicU64 = AtomicU64::new(8);
static STAGE2_ELAPSED_NS: AtomicU64 = AtomicU64::new(0);

static STAGE3_TG_ID: AtomicU64 = AtomicU64::new(0);
static STAGE3_UADDR: AtomicU64 = AtomicU64::new(0);
static STAGE3_ARM_NS: AtomicU64 = AtomicU64::new(0);
static STAGE3_PARKED: AtomicU64 = AtomicU64::new(0);
static STAGE3_ENQUEUED: AtomicU64 = AtomicU64::new(0);
static STAGE3_LEFT: AtomicU64 = AtomicU64::new(0);
static STAGE3_RET: AtomicU64 = AtomicU64::new(8);
static STAGE3_ELAPSED_NS: AtomicU64 = AtomicU64::new(0);

static DRIVEN: AtomicU64 = AtomicU64::new(0);
static RESCUES: AtomicU64 = AtomicU64::new(0);

#[inline]
fn now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(nanos)
}

pub fn arm_from_val3(val3: u32) -> Option<Stage> {
    match val3 {
        STAGE1_SENTINEL => Some(Stage::S1),
        STAGE2_SENTINEL => Some(Stage::S2),
        STAGE3_SENTINEL => Some(Stage::S3),
        _ => None,
    }
}

pub fn is_report(val3: u32) -> bool {
    val3 == REPORT_SENTINEL
}

pub fn record_arm(stage: Stage, tg_id: u64, uaddr: u64) -> u64 {
    let started_at = now_ns();
    match stage {
        Stage::S1 => {
            STAGE1_TG_ID.store(tg_id, Ordering::Release);
            STAGE1_UADDR.store(uaddr, Ordering::Release);
            STAGE1_ARM_NS.store(started_at, Ordering::Release);
        }
        Stage::S2 => {
            STAGE2_TG_ID.store(tg_id, Ordering::Release);
            STAGE2_UADDR.store(uaddr, Ordering::Release);
            STAGE2_ARM_NS.store(started_at, Ordering::Release);
        }
        Stage::S3 => {
            STAGE3_TG_ID.store(tg_id, Ordering::Release);
            STAGE3_UADDR.store(uaddr, Ordering::Release);
            STAGE3_ARM_NS.store(started_at, Ordering::Release);
        }
    }
    started_at.saturating_add(BACKSTOP_NS)
}

pub fn stage1_drive(tg_id: u64, uaddr: u64, expected_val: u32) {
    let changed_value = expected_val.wrapping_add(1);
    let _ = crate::syscall::userptr::copy_to_user(uaddr as *mut u32, &changed_value);
    let woken = crate::syscall::futex::futex_wake_for_thread_group(tg_id, uaddr, u32::MAX);
    STAGE1_WAKE.store(woken as u64, Ordering::Release);
    DRIVEN.fetch_add(1, Ordering::AcqRel);
}

pub fn stage2_drive(tg_id: u64, uaddr: u64) {
    let woken = crate::syscall::futex::futex_wake_for_thread_group(tg_id, uaddr, u32::MAX);
    STAGE2_WAKE.store(woken as u64, Ordering::Release);
    DRIVEN.fetch_add(1, Ordering::AcqRel);
}

pub fn deadline_passed(deadline_ns: u64) -> bool {
    now_ns() >= deadline_ns
}

pub fn elapsed_since_arm(stage: Stage) -> u64 {
    now_ns().saturating_sub(match stage {
        Stage::S1 => STAGE1_ARM_NS.load(Ordering::Acquire),
        Stage::S2 => STAGE2_ARM_NS.load(Ordering::Acquire),
        Stage::S3 => STAGE3_ARM_NS.load(Ordering::Acquire),
    })
}

pub fn record_parked(stage: Stage) {
    match stage {
        Stage::S1 => STAGE1_PARKED.store(1, Ordering::Release),
        Stage::S2 => STAGE2_PARKED.store(1, Ordering::Release),
        Stage::S3 => STAGE3_PARKED.store(1, Ordering::Release),
    }
    RESCUES.fetch_add(1, Ordering::AcqRel);
}

pub fn record_enqueued(stage: Stage) {
    match stage {
        Stage::S1 => STAGE1_ENQUEUED.fetch_add(1, Ordering::AcqRel),
        Stage::S2 => STAGE2_ENQUEUED.fetch_add(1, Ordering::AcqRel),
        Stage::S3 => STAGE3_ENQUEUED.fetch_add(1, Ordering::AcqRel),
    };
}

pub fn record_left(stage: Stage) {
    match stage {
        Stage::S1 => STAGE1_LEFT.fetch_add(1, Ordering::AcqRel),
        Stage::S2 => STAGE2_LEFT.fetch_add(1, Ordering::AcqRel),
        Stage::S3 => STAGE3_LEFT.fetch_add(1, Ordering::AcqRel),
    };
}

fn ret_code(ret: OracleRet) -> u64 {
    match ret {
        OracleRet::Zero => 0,
        OracleRet::Eagain => 1,
        OracleRet::Eintr => 2,
        OracleRet::Etimedout => 3,
        OracleRet::Efault => 4,
        OracleRet::Einval => 5,
        OracleRet::Esrch => 6,
        OracleRet::Rescued => 7,
        OracleRet::Other => 8,
    }
}

pub fn record_return(stage: Stage, ret: OracleRet, elapsed_ns: u64) {
    let code = ret_code(ret);
    match stage {
        Stage::S1 => {
            STAGE1_RET.store(code, Ordering::Release);
            STAGE1_ELAPSED_NS.store(elapsed_ns, Ordering::Release);
        }
        Stage::S2 => {
            STAGE2_RET.store(code, Ordering::Release);
            STAGE2_ELAPSED_NS.store(elapsed_ns, Ordering::Release);
        }
        Stage::S3 => {
            STAGE3_RET.store(code, Ordering::Release);
            STAGE3_ELAPSED_NS.store(elapsed_ns, Ordering::Release);
        }
    }
}

fn ret_token(code: u64) -> &'static str {
    match code {
        0 => "0",
        1 => "EAGAIN",
        2 => "EINTR",
        3 => "ETIMEDOUT",
        4 => "EFAULT",
        5 => "EINVAL",
        6 => "ESRCH",
        7 => "RESCUED",
        _ => "OTHER",
    }
}

fn balance(enqueued: u64, left: u64) -> i64 {
    enqueued as i64 - left as i64
}

pub fn report() {
    let queue_residual = crate::syscall::futex::oracle_queue_residual([
        (
            STAGE1_TG_ID.load(Ordering::Acquire),
            STAGE1_UADDR.load(Ordering::Acquire),
        ),
        (
            STAGE2_TG_ID.load(Ordering::Acquire),
            STAGE2_UADDR.load(Ordering::Acquire),
        ),
        (
            STAGE3_TG_ID.load(Ordering::Acquire),
            STAGE3_UADDR.load(Ordering::Acquire),
        ),
    ]);
    let stage3_elapsed = STAGE3_ELAPSED_NS.load(Ordering::Acquire);
    let stage3_elapsed_ok = u64::from((50_000_000..=550_000_000).contains(&stage3_elapsed));
    let total_enqueued = STAGE1_ENQUEUED.load(Ordering::Acquire)
        + STAGE2_ENQUEUED.load(Ordering::Acquire)
        + STAGE3_ENQUEUED.load(Ordering::Acquire);
    let total_left = STAGE1_LEFT.load(Ordering::Acquire)
        + STAGE2_LEFT.load(Ordering::Acquire)
        + STAGE3_LEFT.load(Ordering::Acquire);

    crate::serial_println!(
        "[FUTEX_HANDOFF_ORACLE:{}:driven={}:stage1_ret={}:stage1_wake={}:stage1_parked={}:stage2_ret={}:stage2_wake={}:stage2_parked={}:stage3_ret={}:stage3_elapsed_ok={}:rescues={}:queue_residual={}:balance={}]",
        if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86" },
        DRIVEN.load(Ordering::Acquire),
        ret_token(STAGE1_RET.load(Ordering::Acquire)),
        STAGE1_WAKE.load(Ordering::Acquire),
        STAGE1_PARKED.load(Ordering::Acquire),
        ret_token(STAGE2_RET.load(Ordering::Acquire)),
        STAGE2_WAKE.load(Ordering::Acquire),
        STAGE2_PARKED.load(Ordering::Acquire),
        ret_token(STAGE3_RET.load(Ordering::Acquire)),
        stage3_elapsed_ok,
        RESCUES.load(Ordering::Acquire),
        queue_residual,
        balance(total_enqueued, total_left),
    );
}
