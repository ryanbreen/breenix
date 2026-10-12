//! System V IPC state on the host: keys and identifiers, permissions, message
//! selection, semop's all-or-none rule, SEM_UNDO and the ABI structures.
extern crate alloc;
mod syscall {
    pub mod errno {
        pub const ENOENT: i32 = 2;
        pub const E2BIG: i32 = 7;
        pub const ENOMEM: i32 = 12;
        pub const EEXIST: i32 = 17;
        pub const EINVAL: i32 = 22;
        pub const ENOSPC: i32 = 28;
        pub const ERANGE: i32 = 34;
    }
}
#[path = "../kernel/src/ipc/sysv/state.rs"]
mod state;
use state::*;

fn perm(key: i32, mode: u32) -> IpcPerm {
    IpcPerm::new(key, 1000, 100, mode)
}

#[test]
fn keys_resolve_to_create_existing_or_errors() {
    let mut ids: IpcIds<u32> = IpcIds::new(8);
    assert_eq!(ids.resolve(77, 0), Err(syscall::errno::ENOENT));
    assert_eq!(ids.resolve(77, IPC_CREAT), Ok(None));
    let id = ids.insert(perm(77, 0o600), 1).unwrap();
    assert_eq!(ids.resolve(77, 0), Ok(Some(id)));
    assert_eq!(ids.resolve(77, IPC_CREAT), Ok(Some(id)));
    assert_eq!(ids.resolve(77, IPC_CREAT | IPC_EXCL), Err(syscall::errno::EEXIST));
    if let Some(entry) = ids.get_mut(id) {
        entry.obj = 5;
    }
    assert_eq!(ids.get(id).map(|e| e.obj), Some(5));
    assert_eq!(ids.resolve(IPC_PRIVATE, 0), Ok(None));
}

#[test]
fn private_objects_are_distinct_and_never_found_by_key() {
    let mut ids: IpcIds<u32> = IpcIds::new(8);
    let a = ids.insert(perm(IPC_PRIVATE, 0o600), 1).unwrap();
    let b = ids.insert(perm(IPC_PRIVATE, 0o600), 2).unwrap();
    assert_ne!(a, b);
    assert_eq!(ids.resolve(IPC_PRIVATE, IPC_CREAT | IPC_EXCL), Ok(None));
}

#[test]
fn a_removed_id_stays_dead_when_its_slot_is_reused() {
    let mut ids: IpcIds<u32> = IpcIds::new(8);
    let old = ids.insert(perm(5, 0o600), 1).unwrap();
    assert_eq!(ids.remove(old).map(|e| e.obj), Some(1));
    assert!(ids.get(old).is_none());
    assert_eq!(ids.resolve(5, 0), Err(syscall::errno::ENOENT));
    let new = ids.insert(perm(5, 0o600), 2).unwrap();
    assert_ne!(old, new);
    assert!(ids.get(old).is_none());
    assert_eq!(ids.get(new).map(|e| e.obj), Some(2));
    assert!(ids.get(-1).is_none());
}

#[test]
fn a_full_class_refuses_with_enospc() {
    let mut ids: IpcIds<u32> = IpcIds::new(2);
    ids.insert(perm(1, 0o600), 1).unwrap();
    ids.insert(perm(2, 0o600), 2).unwrap();
    assert_eq!(ids.insert(perm(3, 0o600), 3), Err(syscall::errno::ENOSPC));
}

#[test]
fn forget_key_keeps_the_id_and_frees_the_key() {
    let mut ids: IpcIds<u32> = IpcIds::new(8);
    let id = ids.insert(perm(9, 0o600), 1).unwrap();
    ids.forget_key(id);
    assert_eq!(ids.resolve(9, 0), Err(syscall::errno::ENOENT));
    assert_eq!(ids.get(id).map(|e| e.perm.key), Some(IPC_PRIVATE));
    let other = ids.insert(perm(9, 0o600), 2).unwrap();
    assert_eq!(ids.resolve(9, 0), Ok(Some(other)));
    assert_eq!(ids.iter_mut().count(), 2);
}

#[test]
fn permissions_follow_owner_creator_group_and_other_classes() {
    let mut p = perm(1, 0o640);
    let none = |_: u32| false;
    assert!(p.permits(1000, none, READ | WRITE));
    assert!(p.permits(0, none, READ | WRITE));
    assert!(p.permits(4242, |g| g == 100, READ));
    assert!(!p.permits(4242, |g| g == 100, WRITE));
    assert!(!p.permits(4242, none, READ));
    p.set(2000, 200, 0o600).unwrap();
    assert!(p.permits(1000, none, WRITE), "the creator keeps the owner class");
    assert!(p.permits(4242, |g| g == 100, 0), "asking for nothing is always granted");
    assert!(!p.permits(4242, |g| g == 100, READ));
    assert!(p.owned_by(1000) && p.owned_by(2000) && p.owned_by(0) && !p.owned_by(4242));
    assert_eq!(requested(0o640), 6);
    assert_eq!(requested(IPC_CREAT | 0o004), 4);
}

#[test]
fn ipc_set_changes_only_the_low_nine_mode_bits() {
    let mut p = perm(1, 0o644);
    p.mode |= SHM_DEST;
    p.set(1, 2, 0o7600).unwrap();
    assert_eq!(p.mode, SHM_DEST | 0o600);
    assert_eq!((p.uid, p.gid, p.cuid, p.cgid), (1, 2, 1000, 100));
}

#[test]
fn ipc_set_refuses_an_owner_of_minus_one_and_changes_nothing() {
    let mut p = perm(1, 0o644);
    assert_eq!(p.set(u32::MAX, 2, 0o600), Err(syscall::errno::EINVAL));
    assert_eq!(p.set(1, u32::MAX, 0o600), Err(syscall::errno::EINVAL));
    assert_eq!((p.uid, p.gid, p.mode), (1000, 100, 0o644));
}

fn msg(mtype: i64, text: &[u8]) -> Message {
    Message { mtype, text: text.to_vec() }
}

#[test]
fn msgrcv_takes_the_message_its_type_names() {
    let mut q = MsgQueue::new(0);
    for (t, s) in [(3, b"a"), (1, b"b"), (2, b"c"), (1, b"d")] {
        q.push(msg(t, s), 7, 0).unwrap();
    }
    let mut take = |t: i64| q.receive(Select::Type(t), 64, false, 8, 0).unwrap().map(|m| (m.mtype, m.text));
    assert_eq!(take(1), Some((1, b"b".to_vec())));
    assert_eq!(take(-2), Some((1, b"d".to_vec())));
    assert_eq!(take(0), Some((3, b"a".to_vec())));
    assert_eq!(take(5), None);
    assert_eq!(take(-1), None);
    assert_eq!(take(0), Some((2, b"c".to_vec())));
    assert_eq!(take(0), None);
    assert_eq!((q.qnum(), q.cbytes, q.lspid, q.lrpid), (0, 0, 7, 8));
}

#[test]
fn a_negative_type_takes_the_oldest_of_the_lowest_type() {
    let mut q = MsgQueue::new(0);
    for (t, s) in [(5, b"x"), (2, b"y"), (2, b"z"), (9, b"w")] {
        q.push(msg(t, s), 1, 0).unwrap();
    }
    assert_eq!(q.find(Select::Type(-4)), Some(1));
    assert_eq!(q.find(Select::Type(-1)), None);
    assert_eq!(q.find(Select::Type(i64::MIN)), Some(1));
}

#[test]
fn msg_except_takes_the_first_message_of_another_type() {
    let mut q = MsgQueue::new(0);
    for (t, s) in [(4, b"a"), (4, b"b"), (7, b"c"), (2, b"d")] {
        q.push(msg(t, s), 1, 0).unwrap();
    }
    assert_eq!(select(4, MSG_EXCEPT), Ok(Select::Except(4)));
    assert_eq!(select(0, MSG_EXCEPT), Ok(Select::Type(0)), "MSG_EXCEPT applies to a positive type only");
    assert_eq!(select(-3, MSG_EXCEPT), Ok(Select::Type(-3)));
    let m = q.receive(Select::Except(4), 8, false, 2, 0).unwrap().unwrap();
    assert_eq!((m.mtype, m.text), (7, b"c".to_vec()));
    assert_eq!(q.find(Select::Except(2)), Some(0));
}

#[test]
fn msg_copy_reads_by_position_and_leaves_the_queue_alone() {
    assert_eq!(select(0, MSG_COPY), Err(syscall::errno::EINVAL), "MSG_COPY needs IPC_NOWAIT");
    assert_eq!(select(0, MSG_COPY | IPC_NOWAIT | MSG_EXCEPT), Err(syscall::errno::EINVAL));
    assert_eq!(select(1, MSG_COPY | IPC_NOWAIT), Ok(Select::Copy(1)));
    let mut q = MsgQueue::new(0);
    for (t, s) in [(3, &b"one"[..]), (5, &b"twotwo"[..])] {
        q.push(msg(t, s), 1, 10).unwrap();
    }
    let m = q.receive(Select::Copy(1), 64, false, 9, 20).unwrap().unwrap();
    assert_eq!((m.mtype, m.text), (5, b"twotwo".to_vec()));
    assert_eq!((q.qnum(), q.cbytes, q.lrpid, q.rtime), (2, 9, 0, 0), "a copy changes nothing");
    assert_eq!(q.receive(Select::Copy(1), 3, false, 9, 20).err(), Some(syscall::errno::E2BIG));
    let m = q.receive(Select::Copy(1), 3, true, 9, 20).unwrap().unwrap();
    assert_eq!(m.text, b"two");
    assert!(q.receive(Select::Copy(2), 64, false, 9, 20).unwrap().is_none());
    assert!(q.receive(Select::Copy(-1), 64, false, 9, 20).unwrap().is_none());
}

#[test]
fn a_short_buffer_is_e2big_unless_msg_noerror_truncates() {
    let mut q = MsgQueue::new(0);
    q.push(msg(1, &[b'x'; 32]), 1, 0).unwrap();
    assert_eq!(q.receive(Select::Type(0), 8, false, 2, 0).err(), Some(syscall::errno::E2BIG));
    assert_eq!(q.qnum(), 1);
    let m = q.receive(Select::Type(0), 8, true, 2, 0).unwrap().unwrap();
    assert_eq!(m.text, [b'x'; 8]);
    assert_eq!((q.qnum(), q.cbytes), (0, 0));
}

#[test]
fn a_queue_holds_msg_qbytes_bytes() {
    let mut q = MsgQueue::new(0);
    q.qbytes = 256;
    for _ in 0..4 {
        assert!(q.has_room(64));
        q.push(msg(1, &[0; 64]), 1, 0).unwrap();
    }
    assert!(!q.has_room(64));
    assert!(q.has_room(0), "an empty message still fits in a queue at msg_qbytes bytes");
    q.qbytes = 3;
    assert!(!q.has_room(0), "Linux also caps the message count at msg_qbytes");
}

fn op(num: u16, op: i16, flg: i16) -> SemOp {
    SemOp { num, op, flg }
}

#[test]
fn semop_adds_takes_and_waits_for_zero() {
    let mut s = SemSet::try_new(1, 0).unwrap();
    let mut adj = [0i16; 1];
    for (o, after) in [(2, 2), (-1, 1), (-1, 0), (0, 0)] {
        assert_eq!(s.attempt(&[op(0, o, 0)], &mut adj, 5, 1), Ok(Attempt::Done));
        assert_eq!(s.vals[0], after);
    }
    assert_eq!((s.pids[0], s.otime), (5, 1));
    assert_eq!(s.attempt(&[op(0, -1, IPC_NOWAIT as i16)], &mut adj, 5, 1), Ok(Attempt::Block { num: 0, zero: false, nowait: true }));
    s.vals[0] = 1;
    assert_eq!(s.attempt(&[op(0, 0, 0)], &mut adj, 5, 1), Ok(Attempt::Block { num: 0, zero: true, nowait: false }));
}

#[test]
fn semop_applies_all_operations_or_none() {
    let mut s = SemSet::try_new(2, 0).unwrap();
    s.vals = vec![1, 0];
    let mut adj = [0i16; 2];
    let ops = [op(0, -1, SEM_UNDO), op(1, -1, 0)];
    assert!(matches!(s.attempt(&ops, &mut adj, 5, 1), Ok(Attempt::Block { num: 1, .. })));
    assert_eq!(s.vals, [1, 0]);
    assert_eq!(adj, [0, 0]);
    assert_eq!(s.pids, [0, 0]);
    s.vals = vec![1, 32767];
    assert_eq!(s.attempt(&[op(0, -1, 0), op(1, 1, 0)], &mut adj, 5, 1), Err(syscall::errno::ERANGE));
    assert_eq!(s.vals, [1, 32767]);
}

#[test]
fn semop_sees_its_own_earlier_operations() {
    let mut s = SemSet::try_new(1, 0).unwrap();
    let mut adj = [0i16; 1];
    assert_eq!(s.attempt(&[op(0, 1, 0), op(0, -1, 0), op(0, 0, 0)], &mut adj, 1, 1), Ok(Attempt::Done));
    assert_eq!(s.vals[0], 0);
}

#[test]
fn sem_undo_records_the_opposite_and_exit_restores_with_a_clamp() {
    let mut s = SemSet::try_new(1, 0).unwrap();
    let mut adj = [0i16; 1];
    s.attempt(&[op(0, 3, SEM_UNDO)], &mut adj, 9, 1).unwrap();
    assert_eq!((s.vals[0], adj[0]), (3, -3));
    assert!(s.apply_undo(&adj, 9, 4));
    assert_eq!((s.vals[0], s.otime), (0, 4), "exit's undo sets sem_otime, as on Linux");
    s.set_val(0, 5, 1, 2);
    let mut adj = [0i16; 1];
    s.attempt(&[op(0, -2, SEM_UNDO)], &mut adj, 9, 1).unwrap();
    assert_eq!((s.vals[0], adj[0]), (3, 2));
    s.apply_undo(&adj, 9, 4);
    assert_eq!(s.vals[0], 5);
    s.vals[0] = 1;
    assert!(s.apply_undo(&[-4], 9, 4));
    assert_eq!(s.vals[0], 0, "an undo that would go negative stops at zero");
    s.vals[0] = 32760;
    s.apply_undo(&[100], 9, 4);
    assert_eq!(s.vals[0], 32767);
    assert!(!s.apply_undo(&[0], 9, 5));
    assert_eq!(s.otime, 5);
}

#[test]
fn an_adjustment_out_of_range_is_erange() {
    let mut s = SemSet::try_new(1, 0).unwrap();
    let mut adj = [i16::MIN + 1; 1];
    assert_eq!(s.attempt(&[op(0, 2, SEM_UNDO)], &mut adj, 1, 1), Err(syscall::errno::ERANGE));
    assert_eq!((s.vals[0], adj[0]), (0, i16::MIN + 1));
}

#[test]
fn counts_set_val_and_set_all() {
    let mut s = SemSet::try_new(3, 0).unwrap();
    s.waiters.push(SemWaiter { tid: 1, num: 0, zero: false });
    s.waiters.push(SemWaiter { tid: 2, num: 1, zero: true });
    s.waiters.push(SemWaiter { tid: 3, num: 1, zero: false });
    assert_eq!(s.counts(0), (1, 0));
    assert_eq!(s.counts(1), (1, 1));
    assert_eq!(s.counts(2), (0, 0));
    s.waiters.retain(|w| w.tid != 3);
    assert_eq!(s.counts(1), (0, 1));
    assert!(!in_range(65536) && !in_range(-1) && in_range(0) && in_range(32767));
    s.set_val(0, 32767, 4, 7);
    assert_eq!((s.vals[0], s.pids[0], s.ctime), (32767, 4, 7));
    assert_eq!(s.set_all(&[1, 2, 40000], 4, 8), Err(syscall::errno::ERANGE));
    assert_eq!(s.vals, [32767, 0, 0]);
    assert_eq!(s.set_all(&[1, 2, 3], 4, 8), Ok(()));
    assert_eq!(s.vals, [1, 2, 3]);
}

#[test]
fn semop_without_sem_undo_takes_an_empty_adjustment_list() {
    let mut s = SemSet::try_new(2, 0).unwrap();
    assert_eq!(s.attempt(&[op(1, 4, 0), op(0, 1, 0)], &mut [], 3, 1), Ok(Attempt::Done));
    assert_eq!(s.vals, [1, 4]);
}

#[test]
fn undo_lists_belong_to_tasks_and_sets() {
    let mut u = UndoLists::new();
    let a = u.join(10).unwrap();
    assert_eq!(u.join(10), Ok(a), "a task keeps its list");
    let b = u.join(11).unwrap();
    assert_ne!(a, b, "a task of its own, as after fork, gets a list of its own");
    u.adjustments(a, 1, 2).unwrap()[0] = 3;
    u.adjustments(a, 2, 1).unwrap()[0] = -1;
    u.adjustments(b, 1, 2).unwrap()[1] = 4;
    assert_eq!(u.adjustments(a, 1, 2).unwrap().as_slice(), [3, 0]);
    u.clear(1, Some(0));
    assert_eq!(u.adjustments(a, 1, 2).unwrap().as_slice(), [0, 0]);
    assert_eq!(u.adjustments(b, 1, 2).unwrap().as_slice(), [0, 4]);
    u.drop_set(1);
    assert_eq!(u.leave(10), Some(a), "the last task to leave a list applies it");
    assert_eq!(u.take(a), Some((2, vec![-1])));
    assert_eq!(u.take(a), None);
    assert_eq!(u.leave(10), None, "leaving twice does nothing");
    assert_eq!(u.leave(11), Some(b));
    assert_eq!(u.take(b), None, "IPC_RMID dropped its only set");
}

#[test]
fn clone_sysvsem_shares_a_list_until_its_last_task_leaves() {
    let mut u = UndoLists::new();
    u.share(20, 21).unwrap();
    let list = u.join(20).unwrap();
    assert_eq!(u.join(21), Ok(list));
    u.adjustments(list, 5, 1).unwrap()[0] = 2;
    u.share(21, 22).unwrap();
    assert_eq!(u.leave(20), None, "the creator leaves while its threads still share the list");
    assert_eq!(u.leave(22), None);
    assert_eq!(u.leave(21), Some(list));
    assert_eq!(u.take(list), Some((5, vec![2])));
    assert_ne!(u.join(21), Ok(list), "a task rejoining after it left starts a new list");
}

#[test]
fn identifier_tables_count_and_find_by_index() {
    let mut ids: IpcIds<u32> = IpcIds::new(8);
    assert_eq!((ids.in_use(), ids.highest_index()), (0, 0));
    let a = ids.insert(perm(1, 0o600), 1).unwrap();
    let b = ids.insert(perm(2, 0o600), 2).unwrap();
    let c = ids.insert(perm(3, 0o600), 3).unwrap();
    ids.remove(b);
    assert_eq!((ids.in_use(), ids.highest_index()), (2, 2));
    assert_eq!(ids.at_index(0).map(|(id, e)| (id, e.obj)), Some((a, 1)));
    assert!(ids.at_index(1).is_none());
    assert_eq!(ids.at_index(2).map(|(id, _)| id), Some(c));
    assert_eq!(ids.at_index(c).map(|(id, _)| id), Some(c), "an identifier's index bits find it");
    assert!(ids.at_index(-1).is_none());
    assert_eq!(ids.iter().count(), 2);
}

#[test]
fn info_structures_report_limits_or_use() {
    let i32_at = |b: &[u8], at: usize| i32::from_ne_bytes(b[at..at + 4].try_into().unwrap());
    let m = msginfo(None);
    let fields: Vec<i32> = (0..7).map(|i| i32_at(&m, i * 4)).collect();
    assert_eq!(fields, [512000, 16384, 8192, 16384, 32000, 16, 16384]);
    assert_eq!(u16::from_ne_bytes([m[28], m[29]]), 0xffff);
    let m = msginfo(Some((2, 5, 99)));
    assert_eq!((i32_at(&m, 0), i32_at(&m, 4), i32_at(&m, 24)), (2, 5, 99));
    let s = seminfo(None);
    let fields: Vec<i32> = (0..10).map(|i| i32_at(&s, i * 4)).collect();
    assert_eq!(fields, [1024000000, 32000, 1024000000, 1024000000, 32000, 500, 500, 20, 32767, 32767]);
    let s = seminfo(Some((3, 7)));
    assert_eq!((i32_at(&s, 28), i32_at(&s, 36)), (3, 7));
    assert_eq!((MSGINFO_SIZE, SEMINFO_SIZE), (32, 40));
}

#[test]
fn shm_info_structures_report_limits_or_use() {
    let u64_at = |b: &[u8], at: usize| u64::from_ne_bytes(b[at..at + 8].try_into().unwrap());
    let i = shminfo();
    assert_eq!((u64_at(&i, 0), u64_at(&i, 8), u64_at(&i, 16), u64_at(&i, 24)), (SHMMAX, 1, 4096, 4096));
    assert_eq!(u64_at(&i, 32), u64::MAX - (1 << 24));
    assert!(i[40..].iter().all(|&b| b == 0));
    let u = shm_info(3, 10);
    assert_eq!(i32::from_ne_bytes(u[0..4].try_into().unwrap()), 3);
    assert_eq!((u64_at(&u, 8), u64_at(&u, 16), u64_at(&u, 24), u64_at(&u, 32), u64_at(&u, 40)), (10, 10, 0, 0, 0));
    assert_eq!((SHMINFO_SIZE, SHM_INFO_SIZE), (72, 48));
}

#[test]
fn the_highest_index_can_skip_objects_that_no_longer_exist() {
    let mut ids: IpcIds<bool> = IpcIds::new(8);
    ids.insert(perm(1, 0o600), true).unwrap();
    ids.insert(perm(2, 0o600), true).unwrap();
    ids.insert(perm(3, 0o600), false).unwrap();
    assert_eq!(ids.highest_index(), 2);
    assert_eq!(ids.highest_index_where(|e| e.obj), 1);
    assert_eq!(ids.highest_index_where(|_| false), 0);
}

#[test]
fn vec_map_keeps_keys_sorted_and_reports_enomem_instead_of_panicking() {
    let mut m: VecMap<i32, u8> = VecMap::new();
    for k in [5, 1, 3] {
        m.insert(k, k as u8).unwrap();
    }
    m.insert(3, 30).unwrap();
    assert_eq!(m.iter().map(|&(k, v)| (k, v)).collect::<Vec<_>>(), [(1, 1), (3, 30), (5, 5)]);
    assert_eq!(m.remove(1), Some(1));
    assert_eq!(m.take_first(|&k| k > 3), Some((5, 5)));
    assert_eq!(try_vec(usize::MAX / 2, 0u64).err(), Some(syscall::errno::ENOMEM));
    assert_eq!(try_vec(3, 7u8), Ok(vec![7, 7, 7]));
}

#[test]
fn abi_structures_put_each_field_where_linux_does() {
    let mut p = IpcPerm::new(0x4950_0011, 1, 2, 0o640);
    p.seq = 3;
    let mut q = MsgQueue::new(44);
    q.push(msg(1, &[0; 5]), 77, 55).unwrap();
    q.lrpid = 88;
    let b = msqid_ds(&p, &q);
    let i32_at = |b: &[u8], at: usize| i32::from_ne_bytes(b[at..at + 4].try_into().unwrap());
    let u64_at = |b: &[u8], at: usize| u64::from_ne_bytes(b[at..at + 8].try_into().unwrap());
    assert_eq!(i32_at(&b, 0), 0x4950_0011);
    assert_eq!((i32_at(&b, 4), i32_at(&b, 12), i32_at(&b, 20)), (1, 1, 0o640));
    assert_eq!(u16::from_ne_bytes([b[24], b[25]]), 3);
    assert_eq!((u64_at(&b, 48), u64_at(&b, 64)), (55, 44));
    assert_eq!((u64_at(&b, 72), u64_at(&b, 80), u64_at(&b, 88)), (5, 1, MSGMNB as u64));
    assert_eq!((i32_at(&b, 96), i32_at(&b, 100)), (77, 88));
    assert_eq!(perm_set_fields(&b), (1, 2, 0o640));
    assert_eq!(msqid_qbytes(&b), MSGMNB as u64);

    let mut s = SemSet::try_new(3, 66).unwrap();
    s.otime = 9;
    let x = semid_ds(&p, &s, SemLayout::X86_64);
    assert_eq!((u64_at(&x, 48), u64_at(&x, 64), u64_at(&x, 80)), (9, 66, 3));
    let a = semid_ds(&p, &s, SemLayout::Aarch64);
    assert_eq!((u64_at(&a, 48), u64_at(&a, 56), u64_at(&a, 64)), (9, 66, 3));
    assert_eq!((SemLayout::X86_64.size(), SemLayout::Aarch64.size()), (104, 88));

    let st = ShmStat { segsz: 4196, atime: 1, dtime: 2, ctime: 3, cpid: 4, lpid: 5, nattch: 6 };
    let h = shmid_ds(&p, &st);
    assert_eq!((u64_at(&h, 48), u64_at(&h, 56), u64_at(&h, 64), u64_at(&h, 72)), (4196, 1, 2, 3));
    assert_eq!((i32_at(&h, 80), i32_at(&h, 84), u64_at(&h, 88)), (4, 5, 6));
}

#[test]
fn sembuf_bytes_decode() {
    let mut b = [0u8; 6];
    b[..2].copy_from_slice(&5u16.to_ne_bytes());
    b[2..4].copy_from_slice(&(-2i16).to_ne_bytes());
    b[4..].copy_from_slice(&(SEM_UNDO | IPC_NOWAIT as i16).to_ne_bytes());
    let o = SemOp::from_bytes(b);
    assert_eq!((o.num, o.op, o.flg), (5, -2, SEM_UNDO | IPC_NOWAIT as i16));
}

#[test]
fn constants_and_limits_have_linux_values() {
    assert_eq!((IPC_PRIVATE, IPC_CREAT, IPC_EXCL, IPC_NOWAIT), (0, 0o1000, 0o2000, 0o4000));
    assert_eq!((IPC_RMID, IPC_SET, IPC_STAT, IPC_64), (0, 1, 2, 0x100));
    assert_eq!((MSG_NOERROR, MSG_EXCEPT, MSG_COPY), (0o10000, 0o20000, 0o40000));
    assert_eq!((IPC_INFO, MSG_STAT, MSG_INFO, MSG_STAT_ANY), (3, 11, 12, 13));
    assert_eq!((SEM_STAT, SEM_INFO, SEM_STAT_ANY), (18, 19, 20));
    assert_eq!((GETPID, GETVAL, GETALL, GETNCNT, GETZCNT, SETVAL, SETALL), (11, 12, 13, 14, 15, 16, 17));
    assert_eq!(SEM_UNDO, 0x1000);
    assert_eq!((SHM_RDONLY, SHM_RND, SHM_EXEC, SHM_DEST), (0o10000, 0o20000, 0o100000, 0o1000));
    assert_eq!((SHM_LOCKED, SHM_REMAP), (0o2000, 0o40000));
    assert_eq!((SHM_LOCK, SHM_UNLOCK, SHM_STAT, SHM_INFO, SHM_STAT_ANY), (11, 12, 13, 14, 15));
    assert_eq!((MSGMAX, MSGMNB, MSGMNI), (8192, 16384, 32000));
    assert_eq!((SEMMSL, SEMMNI, SEMOPM, SEMVMX), (32000, 32000, 500, 32767));
    assert_eq!((SHMMNI, SHMMIN, SHMMAX, SHMLBA), (4096, 1, u64::MAX - (1 << 24), 4096));
    assert_eq!((IPC_PERM_SIZE, MSQID_DS_SIZE, SHMID_DS_SIZE), (48, 120, 112));
    assert_eq!(SemLayout::NATIVE.size(), if cfg!(target_arch = "x86_64") { 104 } else { 88 });
}
