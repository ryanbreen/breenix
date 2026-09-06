"""GDB driver for the BXCAP edge=LOCKUP oracle.

Loaded by scripts/run-lockup-capture-oracle.sh, which resolves every symbol
this file needs from the ELF's own symbol table and passes the ADDRESSES in
the environment. It does that rather than letting GDB look names up because
the aarch64 kernel is built without DWARF, so `kernel::...::SYMBOL` does not
resolve; the addresses do.

What this driver does, and deliberately all it does:
  * watches the coordinator's phase word;
  * at each PRE_ARM checkpoint, samples CPU0's deferred-requeue slot (the
    arming precondition -- a nonzero slot means exception return would take a
    BLOCKING scheduler acquisition while the peer holds the guard) and the
    exit-kick heartbeat, then writes the host acknowledgement that starts the
    episode;
  * at each RELEASED checkpoint and at DONE, reads the episode receipts.

It sets NO breakpoint in the detector, the dump or the emitter, so the
acceptance serial comes from an undisturbed run. The only guest memory it
writes is the oracle's own host-ack word.
"""
import os
import struct
import time

import gdb


RECEIPT_PATH = os.environ["BXCAP_LOCKUP_RECEIPTS"]
HOST_CEILING = float(os.environ.get("BXCAP_LOCKUP_HOST_CEILING", "60"))

PHASE_PRE_ARM = 1
PHASE_RELEASED = 4
PHASE_DONE = 5
PHASE_FAILED = 9

ADDR = dict()
for key, value in os.environ.items():
    if key.startswith("BXCAP_ADDR_"):
        ADDR[key[len("BXCAP_ADDR_"):]] = int(value, 16)

ROWS = []


def emit(key, value):
    ROWS.append("%s=%s" % (key, value))


def flush():
    handle = open(RECEIPT_PATH, "w")
    handle.write("\n".join(ROWS))
    handle.write("\n")
    handle.close()


def read_u64(addr):
    raw = gdb.selected_inferior().read_memory(addr, 8)
    return struct.unpack("<Q", bytes(raw))[0]


def write_u64(addr, value):
    gdb.selected_inferior().write_memory(addr, struct.pack("<Q", value))


def slot(name, index):
    return read_u64(ADDR[name] + 8 * index)


RECEIPT_SLOTS = [
    "RCPT_ACQUIRED",
    "RCPT_CPU",
    "RCPT_TICK_AT_ACQUIRE",
    "RCPT_TICK_AT_RELEASE",
    "RCPT_HELD_TICKS",
    "RCPT_CTX_AT_ACQUIRE",
    "RCPT_CTX_AT_RELEASE",
    "RCPT_SYSCALL_AT_ACQUIRE",
    "RCPT_SYSCALL_AT_RELEASE",
    "RCPT_HEARTBEAT_AT_ACQUIRE",
    "RCPT_HEARTBEAT_AT_RELEASE",
    "RCPT_PROGRESS_MOVED_DURING_HOLD",
    "RCPT_EXPIRED",
    "RCPT_TS_AT_ACQUIRE",
    "RCPT_TS_AT_RELEASE",
]


def record_episode(index):
    for name in RECEIPT_SLOTS:
        emit("episode%d.%s" % (index, name.lower()), slot(name, index))


def sample_arming(index):
    """Read the arming precondition and the pre-hold heartbeat.

    CPU0's deferred-requeue slot MUST be zero. If it is not, the exception
    return that follows would drain it under a blocking scheduler acquisition
    while CPU1 holds the guard. That is reported as a setup failure, never
    cleared by this driver.
    """
    deferred = read_u64(ADDR["DEFERRED_REQUEUE"])
    heartbeat = read_u64(ADDR["EXIT_KICK_HEARTBEAT"])
    emit("episode%d.arm.cpu0_deferred_requeue" % index, deferred)
    emit("episode%d.arm.exit_kick_heartbeat" % index, heartbeat)
    if deferred != 0:
        emit("episode%d.arm.verdict" % index, "SETUP_FAIL_DEFERRED_REQUEUE_NONEMPTY")
        return False
    emit("episode%d.arm.verdict" % index, "ARMED")
    return True


def sample_post_release(index):
    emit(
        "episode%d.post.exit_kick_heartbeat" % index,
        read_u64(ADDR["EXIT_KICK_HEARTBEAT"]),
    )
    emit(
        "episode%d.post.cpu0_deferred_requeue" % index,
        read_u64(ADDR["DEFERRED_REQUEUE"]),
    )
    record_episode(index)


def finish(verdict):
    emit("threshold_ticks", read_u64(ADDR["ORACLE_THRESHOLD_TICKS"]))
    emit("tsfreq_hz", read_u64(ADDR["ORACLE_TSFREQ_HZ"]))
    emit("setup_failure", read_u64(ADDR["ORACLE_SETUP_FAILURE"]))
    emit("final_phase", read_u64(ADDR["ORACLE_PHASE"]))
    emit("verdict", verdict)
    flush()


def main():
    port = os.environ["BXCAP_LOCKUP_GDB_PORT"]
    gdb.execute("set pagination off")
    gdb.execute("set confirm off")
    gdb.execute("set architecture aarch64")
    gdb.execute("target remote :%s" % port)
    gdb.execute("watch *(unsigned long *) %d" % ADDR["ORACLE_PHASE"])
    emit("gdb.watchpoint", "ORACLE_PHASE")
    emit("gdb.host_ceiling_secs", int(HOST_CEILING))
    started = time.time()
    acks = 0
    released = 0
    while True:
        gdb.execute("continue")
        phase = read_u64(ADDR["ORACLE_PHASE"])
        elapsed = time.time() - started
        if elapsed > HOST_CEILING:
            emit("gdb.elapsed_secs", round(elapsed, 3))
            finish("HOST_CEILING")
            return
        if phase == PHASE_PRE_ARM:
            index = read_u64(ADDR["ORACLE_EPISODE"])
            if not sample_arming(index):
                finish("SETUP_FAIL")
                return
            acks += 1
            write_u64(ADDR["ORACLE_HOST_ACK"], acks)
            continue
        if phase == PHASE_RELEASED:
            index = read_u64(ADDR["ORACLE_EPISODE"])
            sample_post_release(index)
            released += 1
            continue
        if phase == PHASE_DONE:
            acks += 1
            write_u64(ADDR["ORACLE_HOST_ACK"], acks)
            emit("gdb.episodes_released", released)
            emit("gdb.elapsed_secs", round(time.time() - started, 3))
            finish("DONE")
            return
        if phase == PHASE_FAILED:
            emit("gdb.elapsed_secs", round(time.time() - started, 3))
            finish("ORACLE_FAILED")
            return


try:
    main()
except Exception as error:
    emit("gdb.exception", repr(error))
    finish("GDB_ERROR")

gdb.execute("detach", to_string=True)
gdb.execute("quit")
