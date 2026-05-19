set pagination off
set confirm off
set architecture aarch64
set remotetimeout 10
set mem inaccessible-by-default off
set logging file gdb_softlock_state.out
set logging overwrite on
set logging enabled on

echo === TURN2 GDB SOFTLOCK STATE: COMPOSITOR WAIT ===\n
echo Target: Parallels guestdebugger on 127.0.0.1:9600\n
target remote 127.0.0.1:9600

set $SCHEDULER = 0xffff00004022d000
set $COMPOSITOR_FRAME_WQ = 0xffff00004022d2b0
set $WINDOW_REGISTRY = 0xffff00004022d2d8
set $CLIENT_FRAME_WQ = 0xffff000040231258
set $GPU_PCI_STATE = 0xffff000040231ba8
set $PROCESS_MANAGER = 0xffff0000402416f0
set $PROCESS_MANAGER_OWNER_CPU = 0xffff000040241668
set $PROCESS_MANAGER_OWNER_TID = 0xffff000040241670
set $NEED_RESCHED = 0xffff000040242000
set $CONTEXT_SWITCH_COUNT = 0xffff000040242008
set $CPU_IS_IDLE = 0xffff000040242010
set $COMPOSITOR_DIRTY_WAKE = 0xffff00004024afdc
set $GPU_PCI_LOCK = 0xffff00004024b034
set $AHCI_IRQ = 0xffff000041200f80
set $DEFERRED_REQUEUE = 0xffff000041bc7840
set $LAST_DEFER_REQUEUE_INFO = 0xffff000041bc7880
set $LAST_DEFER_REQUEUE_SP = 0xffff000041bc78c0
set $LAST_DEFER_REQUEUE_ELR = 0xffff000041bc7900
set $LAST_DEFER_REQUEUE_X30 = 0xffff000041bc7940
set $INLINE_SCHEDULE_STATE = 0xffff000041bc7980

echo === SYMBOL ADDRESS MAP ===\n
printf "SCHEDULER=0x%lx\n", $SCHEDULER
printf "COMPOSITOR_FRAME_WQ=0x%lx\n", $COMPOSITOR_FRAME_WQ
printf "WINDOW_REGISTRY=0x%lx\n", $WINDOW_REGISTRY
printf "CLIENT_FRAME_WQ=0x%lx\n", $CLIENT_FRAME_WQ
printf "COMPOSITOR_DIRTY_WAKE=0x%lx\n", $COMPOSITOR_DIRTY_WAKE
printf "NEED_RESCHED=0x%lx CONTEXT_SWITCH_COUNT=0x%lx CPU_IS_IDLE=0x%lx\n", $NEED_RESCHED, $CONTEXT_SWITCH_COUNT, $CPU_IS_IDLE
printf "DEFERRED_REQUEUE=0x%lx INLINE_SCHEDULE_STATE=0x%lx\n", $DEFERRED_REQUEUE, $INLINE_SCHEDULE_STATE

echo === RAW BINARY SNAPSHOTS ===\n
dump binary memory scheduler.bin $SCHEDULER $SCHEDULER+0x2b0
dump binary memory compositor_frame_wq.bin $COMPOSITOR_FRAME_WQ $COMPOSITOR_FRAME_WQ+0x40
dump binary memory client_frame_wq.bin $CLIENT_FRAME_WQ $CLIENT_FRAME_WQ+0x40
dump binary memory window_registry.bin $WINDOW_REGISTRY $WINDOW_REGISTRY+0x4000
dump binary memory deferred_requeue.bin $DEFERRED_REQUEUE $DEFERRED_REQUEUE+0x180
dump binary memory gpu_pci_lock.bin $GPU_PCI_LOCK $GPU_PCI_LOCK+0x80

echo === LIVENESS LOCK SNAPSHOT ===\n
printf "scheduler_lock_byte=%u scheduler_word=0x%lx\n", *(unsigned char*)$SCHEDULER, *(unsigned long*)$SCHEDULER
printf "process_manager_lock_byte=%u process_owner_cpu=0x%lx process_owner_tid=0x%lx\n", *(unsigned char*)$PROCESS_MANAGER, *(unsigned long*)$PROCESS_MANAGER_OWNER_CPU, *(unsigned long*)$PROCESS_MANAGER_OWNER_TID
printf "gpu_pci_lock_byte=%u gpu_pci_word=0x%lx\n", *(unsigned char*)$GPU_PCI_LOCK, *(unsigned long*)$GPU_PCI_LOCK
printf "ahci_irq=%u\n", *(unsigned int*)$AHCI_IRQ
printf "need_resched_byte=%u context_switch_count=%lu\n", *(unsigned char*)$NEED_RESCHED, *(unsigned long*)$CONTEXT_SWITCH_COUNT
printf "cpu_is_idle bytes: "
x/8xb $CPU_IS_IDLE

python
import gdb
import struct

inf = gdb.selected_inferior()

SCHEDULER = 0xffff00004022d000
COMPOSITOR_FRAME_WQ = 0xffff00004022d2b0
WINDOW_REGISTRY = 0xffff00004022d2d8
CLIENT_FRAME_WQ = 0xffff000040231258
COMPOSITOR_DIRTY_WAKE = 0xffff00004024afdc
NEED_RESCHED = 0xffff000040242000
CONTEXT_SWITCH_COUNT = 0xffff000040242008
CPU_IS_IDLE = 0xffff000040242010
DEFERRED_REQUEUE = 0xffff000041bc7840
LAST_DEFER_REQUEUE_INFO = 0xffff000041bc7880
LAST_DEFER_REQUEUE_SP = 0xffff000041bc78c0
LAST_DEFER_REQUEUE_ELR = 0xffff000041bc7900
LAST_DEFER_REQUEUE_X30 = 0xffff000041bc7940
INLINE_SCHEDULE_STATE = 0xffff000041bc7980

def read(addr, n):
    return bytes(inf.read_memory(addr, n))

def u8(addr):
    return read(addr, 1)[0]

def u32(addr):
    return struct.unpack_from("<I", read(addr, 4))[0]

def i32(addr):
    return struct.unpack_from("<i", read(addr, 4))[0]

def u64(addr):
    return struct.unpack_from("<Q", read(addr, 8))[0]

def maybe_option_u64(tag, value):
    if tag == 0:
        return "None"
    if tag == 1:
        return "Some(%d)" % value
    return "tag%d(0x%x)" % (tag, value)

def safe_u64(addr):
    try:
        return u64(addr)
    except Exception as exc:
        return "ERR:%s" % exc

def decode_waitqueue(name, base):
    print("=== WAITQUEUE %s DECODE ===" % name)
    try:
        lock = u8(base)
        cap = u64(base + 8)
        ptr = u64(base + 16)
        head = u64(base + 24)
        length = u64(base + 32)
        print("%s.lock_byte=%u cap=%u ptr=0x%x head=%u len=%u" % (name, lock, cap, ptr, head, length))
        logical = []
        if cap > 0 and cap < 4096 and length < 4096 and ptr != 0:
            for i in range(length):
                idx = (head + i) % cap
                logical.append(u64(ptr + idx * 8))
        print("%s.logical_waiters=%s" % (name, logical))
        if cap > 0 and cap <= 32 and ptr != 0:
            raw = [safe_u64(ptr + i * 8) for i in range(cap)]
            print("%s.raw_ring=%s" % (name, raw))
    except Exception as exc:
        print("%s.decode_error=%s" % (name, exc))

def decode_window_registry(base):
    print("=== WINDOW REGISTRY DECODE ===")
    slot_size = 984
    data = base + 8
    print("window_registry.lock_byte=%u data=0x%x slot_size=%u max_slots=16" % (u8(base), data, slot_size))
    active = 0
    for idx in range(16):
        slot = data + idx * slot_size
        tag = u64(slot + 0)
        wait_value = u64(slot + 8)
        waiting = maybe_option_u64(tag, wait_value)
        generation = u64(slot + 144)
        last_uploaded = u64(slot + 152)
        last_read = u64(slot + 160)
        owner = u64(slot + 40)
        size = u64(slot + 56)
        mapped = u64(slot + 64)
        input_head = u64(slot + 936)
        input_tail = u64(slot + 944)
        buf_id = u32(slot + 952)
        width = u32(slot + 956)
        height = u32(slot + 960)
        x = i32(slot + 964)
        y = i32(slot + 968)
        z = u32(slot + 972)
        resource = u32(slot + 976)
        registered = u8(slot + 980)
        virgl_init = u8(slot + 981)
        plausible = (
            tag != 2
            or buf_id != 0
            or generation != 0
            or last_uploaded != 0
            or registered != 0
            or width != 0
            or height != 0
        )
        if plausible:
            active += 1
            pending = generation > last_uploaded
            print(
                "window[%02d] slot=0x%x tag=%u id=%u owner=%u size=%u mapped=0x%x "
                "width=%u height=%u x=%d y=%d z=%u registered=%u resource=%u virgl_init=%u "
                "generation=%u last_uploaded_gen=%u last_read_gen=%u pending=%s "
                "waiting_thread_id=%s input_head=%u input_tail=%u"
                % (
                    idx,
                    slot,
                    tag,
                    buf_id,
                    owner,
                    size,
                    mapped,
                    width,
                    height,
                    x,
                    y,
                    z,
                    registered,
                    resource,
                    virgl_init,
                    generation,
                    last_uploaded,
                    last_read,
                    str(pending),
                    waiting,
                    input_head,
                    input_tail,
                )
            )
    print("window_registry.active_or_nonempty_slots=%u" % active)
    print("window_registry.next_id_candidate=%u" % u32(data + 16 * slot_size))

def decode_scheduler(base):
    print("=== SCHEDULER DECODE ===")
    data = base + 8
    print("scheduler.lock_byte=%u data=0x%x" % (u8(base), data))
    current_cpus = {13: [], 16: []}
    previous_cpus = {13: [], 16: []}
    for cpu in range(8):
        cbase = data + cpu * 40
        cur_tag = u64(cbase + 0)
        cur_val = u64(cbase + 8)
        prev_tag = u64(cbase + 16)
        prev_val = u64(cbase + 24)
        idle_tid = u64(cbase + 32)
        if cur_tag == 1 and cur_val in current_cpus:
            current_cpus[cur_val].append(cpu)
        if prev_tag == 1 and prev_val in previous_cpus:
            previous_cpus[prev_val].append(cpu)
        print(
            "cpu_state[%u] current=%s previous=%s idle_tid=%u"
            % (cpu, maybe_option_u64(cur_tag, cur_val), maybe_option_u64(prev_tag, prev_val), idle_tid)
        )

    threads_cap = u64(data + 320)
    threads_ptr = u64(data + 328)
    threads_len = u64(data + 336)
    print("threads_vec cap=%u ptr=0x%x len=%u" % (threads_cap, threads_ptr, threads_len))
    if threads_ptr != 0 and threads_len < 128:
        for i in range(threads_len):
            tptr = u64(threads_ptr + i * 8)
            prefix = []
            try:
                for j in range(8):
                    prefix.append(u64(tptr + j * 8))
            except Exception as exc:
                prefix = ["ERR:%s" % exc]
            print("thread_box[%02u]=0x%x first8q=%s" % (i, tptr, prefix))

    queue_locations = {13: [], 16: []}
    qbase = data + 344
    for cpu in range(8):
        q = qbase + cpu * 32
        cap = u64(q + 0)
        ptr = u64(q + 8)
        head = u64(q + 16)
        length = u64(q + 24)
        ids = []
        if cap > 0 and cap < 4096 and length < 4096 and ptr != 0:
            for i in range(length):
                tid = u64(ptr + ((head + i) % cap) * 8)
                ids.append(tid)
                if tid in queue_locations:
                    queue_locations[tid].append(cpu)
        print("ready_queue[%u] cap=%u ptr=0x%x head=%u len=%u ids=%s" % (cpu, cap, ptr, head, length, ids))

    heap_cap = u64(data + 600)
    heap_ptr = u64(data + 608)
    heap_len = u64(data + 616)
    print("timer_heap cap=%u ptr=0x%x len=%u" % (heap_cap, heap_ptr, heap_len))
    for tid in (13, 16):
        print(
            "tid_location tid=%u current_cpus=%s previous_cpus=%s ready_queues=%s"
            % (tid, current_cpus[tid], previous_cpus[tid], queue_locations[tid])
        )

def decode_percpu_and_deferred():
    print("=== PER-CPU STATIC STATE ===")
    print("need_resched_byte=%u context_switch_count=%u" % (u8(NEED_RESCHED), u64(CONTEXT_SWITCH_COUNT)))
    print("cpu_is_idle=%s" % ([u8(CPU_IS_IDLE + i) for i in range(8)],))
    deferred = [u64(DEFERRED_REQUEUE + i * 8) for i in range(8)]
    info = [u64(LAST_DEFER_REQUEUE_INFO + i * 8) for i in range(8)]
    sp = [u64(LAST_DEFER_REQUEUE_SP + i * 8) for i in range(8)]
    elr = [u64(LAST_DEFER_REQUEUE_ELR + i * 8) for i in range(8)]
    x30 = [u64(LAST_DEFER_REQUEUE_X30 + i * 8) for i in range(8)]
    print("deferred_requeue=%s" % deferred)
    print("last_defer_info=%s" % [hex(x) for x in info])
    print("last_defer_sp=%s" % [hex(x) for x in sp])
    print("last_defer_elr=%s" % [hex(x) for x in elr])
    print("last_defer_x30=%s" % [hex(x) for x in x30])
    for cpu in range(8):
        s = INLINE_SCHEDULE_STATE + cpu * 32
        scheduler_ptr = u64(s + 0)
        old_tid = u64(s + 8)
        new_tid = u64(s + 16)
        should = u8(s + 24)
        print(
            "inline_schedule_state[%u] scheduler_ptr=0x%x old_tid=%u new_tid=%u should_requeue_old=%u"
            % (cpu, scheduler_ptr, old_tid, new_tid, should)
        )
    for tid in (13, 16):
        print(
            "tid_deferred_membership tid=%u deferred_cpus=%s inline_old_cpus=%s inline_new_cpus=%s"
            % (
                tid,
                [cpu for cpu, val in enumerate(deferred) if val == tid],
                [cpu for cpu in range(8) if u64(INLINE_SCHEDULE_STATE + cpu * 32 + 8) == tid],
                [cpu for cpu in range(8) if u64(INLINE_SCHEDULE_STATE + cpu * 32 + 16) == tid],
            )
        )

print("=== COMPOSITOR DOMAIN DECODE ===")
print("COMPOSITOR_DIRTY_WAKE.byte=%u" % u8(COMPOSITOR_DIRTY_WAKE))
decode_waitqueue("COMPOSITOR_FRAME_WQ", COMPOSITOR_FRAME_WQ)
decode_waitqueue("CLIENT_FRAME_WQ", CLIENT_FRAME_WQ)
decode_window_registry(WINDOW_REGISTRY)
decode_scheduler(SCHEDULER)
decode_percpu_and_deferred()
end

echo === RAW MEMORY TEXT DUMPS ===\n
echo --- COMPOSITOR_FRAME_WQ raw ---\n
x/8xg $COMPOSITOR_FRAME_WQ
echo --- CLIENT_FRAME_WQ raw ---\n
x/8xg $CLIENT_FRAME_WQ
echo --- WINDOW_REGISTRY first 256 qwords raw ---\n
x/256xg $WINDOW_REGISTRY
echo --- SCHEDULER first 96 qwords raw ---\n
x/96xg $SCHEDULER
echo --- DEFERRED/INLINE raw ---\n
x/48xg $DEFERRED_REQUEUE

echo === TID SEARCH IN KEY RAW RANGES ===\n
echo find tid 13 in window registry\n
find /g $WINDOW_REGISTRY, $WINDOW_REGISTRY+0x4000, 13
echo find tid 16 in window registry\n
find /g $WINDOW_REGISTRY, $WINDOW_REGISTRY+0x4000, 16
echo find tid 13 in scheduler static\n
find /g $SCHEDULER, $SCHEDULER+0x2b0, 13
echo find tid 16 in scheduler static\n
find /g $SCHEDULER, $SCHEDULER+0x2b0, 16

echo === DETACH ===\n
detach
quit
