#!/usr/bin/env python3
"""Add rows for unowned allocation sites to docs/plans/e1-oom-inventory.json.

The E1 plan's table (section 11, "E1-11 to E1-20") assigns each part of
kernel/src to the PR that converts it, with the error that PR surfaces and the
rollback point it fails before. This script applies that table to the sites
the inventory scan reports and keeps every existing row as it is, so a row a
conversion PR has refined is not overwritten.

    BREENIX_OOM_INVENTORY_DUMP=/path/sites.json \\
        cargo test --manifest-path tools/kmem-host-tests/Cargo.toml --test oom_inventory
    scripts/kmem-oom-inventory-fill.py /path/sites.json

A site whose file the table does not name is printed and makes the script exit 1.
"""
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
INVENTORY = REPO / "docs/plans/e1-oom-inventory.json"

OWNERS = {
    "boot": ("E1-4",
             "panic with the OOM record; the site becomes try_*().expect(\"boot: <what>\") (7.4 boot rule)",
             "none: before PID 1 runs a failed boot allocation stops the boot"),
    "test": ("test",
             "outside the runtime rule: built only with a test feature",
             "none: converted when a test feature builds alloc with no_global_oom_handling"),
    "E1-11": ("E1-11", "ENOMEM", "before any user-visible effect of the call"),
    "E1-11-irq": ("E1-11",
                  "none: an interrupt handler does not allocate; the site moves to state preallocated outside it",
                  "before the handler publishes any state"),
    "E1-12": ("E1-12", "ENOMEM", "before the VMA or page-table change is published"),
    "E1-13": ("E1-13",
              "EAGAIN for fork, clone and thread creation; ENOMEM for exec and the rest; EAGAIN for sigqueue",
              "before the process, thread or signal is published"),
    "E1-14": ("E1-14", "ENOMEM", "before the dentry, fd or lock is published"),
    "E1-15": ("E1-15", "ENOMEM",
              "before any disk change, by the reservation rule (file_map.rs, fallible-allocation discipline)"),
    "E1-16": ("E1-16", "ENOBUFS on send; a counted drop on receive", "before the segment is queued"),
    "E1-17": ("E1-17", "ENOMEM or ENOSPC per the call's man page", "before the object is linked"),
    "E1-18": ("E1-18", "an I/O error to the request", "before the request is submitted; no partial request"),
    "E1-19-window": ("E1-19", "ENOMEM from the window commit",
                     "before the new buffer is published; the last committed frame stays on screen"),
    "E1-19-input": ("E1-19", "the input batch is dropped whole and counted",
                    "before the batch is published to readers"),
    "E1-19": ("E1-19", "a counted drop, or the call's errno", "before the driver publishes the unit of work"),
    "E1-20": ("E1-20", "ENOMEM on a read; logging does not allocate",
              "before the read consumes its source; a /proc/kmsg read keeps its position"),
}

# (prefix under kernel/src, owner key); the first match wins.
TABLE = [
    ("contracts/", "test"), ("proof/", "test"), ("test_framework/", "test"),
    ("gdt_tests.rs", "test"), ("contract_runner.rs", "test"), ("gpu_fault_acceptance.rs", "test"),
    ("capture_oracle.rs", "test"), ("capture_lockup_oracle.rs", "test"),
    ("boot/", "boot"), ("main.rs", "boot"), ("main_aarch64.rs", "boot"), ("platform_config.rs", "boot"),
    ("syscall/mmap.rs", "E1-12"), ("syscall/memory.rs", "E1-12"), ("memory/", "E1-12"),
    ("syscall/signal.rs", "E1-13"), ("process/", "E1-13"), ("task/", "E1-13"), ("arch_impl/", "E1-13"),
    ("signal/", "E1-13"), ("exec_elf.rs", "E1-13"), ("elf.rs", "E1-13"), ("tls.rs", "E1-13"),
    ("gdt.rs", "E1-13"), ("per_cpu.rs", "E1-13"), ("per_cpu_aarch64.rs", "E1-13"),
    ("stack_switch.rs", "E1-13"), ("pm_reference.rs", "E1-13"), ("spinlock.rs", "E1-13"),
    ("irq_safe_mutex.rs", "E1-13"),
    ("interrupts/", "E1-11-irq"), ("interrupts.rs", "E1-11-irq"), ("interrupts_fix.rs", "E1-11-irq"),
    ("syscall/", "E1-11"), ("time/", "E1-11"), ("random/", "E1-11"),
    ("fs/breenfs/", "E1-15"), ("fs/ext2/", "E1-15"), ("fs/procfs/", "E1-20"), ("fs/", "E1-14"),
    ("ipc/fd.rs", "E1-14"), ("ipc/", "E1-17"),
    ("net/", "E1-16"), ("socket/", "E1-16"),
    ("block/", "E1-18"), ("drivers/ahci/", "E1-18"), ("drivers/nvme/", "E1-18"),
    ("drivers/virtio/block", "E1-18"),
    ("window/", "E1-19-window"), ("input/", "E1-19-input"), ("keyboard/", "E1-19-input"),
    ("keyboard.rs", "E1-19-input"),
    ("drivers/", "E1-19"), ("gpu/", "E1-19"), ("console/", "E1-19"), ("tty/", "E1-19"),
    ("graphics/", "E1-19"), ("framebuffer.rs", "E1-19"),
    ("capture/", "E1-20"), ("tracing/", "E1-20"), ("serial/", "E1-20"), ("test_control/", "E1-20"),
    ("test_control.rs", "E1-20"), ("log_buffer.rs", "E1-20"), ("log_notice.rs", "E1-20"),
    ("log_ring.rs", "E1-20"), ("logger.rs", "E1-20"), ("irq_log.rs", "E1-20"), ("serial", "E1-20"),
    ("trace.rs", "E1-20"), ("output_accounting.rs", "E1-20"), ("stage_lines.rs", "E1-20"),
    ("uart_owner.rs", "E1-20"), ("macros.rs", "E1-20"), ("test_exec.rs", "E1-20"),
    ("test_userspace.rs", "E1-20"), ("test_checkpoints.rs", "E1-20"), ("userspace_test.rs", "E1-20"),
    ("userspace_fault_tests.rs", "E1-20"), ("clock_gettime_test.rs", "E1-20"), ("time_test.rs", "E1-20"),
    ("rtc_test.rs", "E1-20"), ("preempt_count_test.rs", "E1-20"),
]


def owner_key(file):
    rel = file.removeprefix("kernel/src/")
    for prefix, key in TABLE:
        if rel.startswith(prefix):
            return key
    return None


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    sites = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    inventory = json.loads(INVENTORY.read_text(encoding="utf-8"))
    rows = inventory["rows"]
    unmapped = sorted({site["file"] for site in sites if owner_key(site["file"]) is None})
    if unmapped:
        print("no owner in the table for:", *unmapped, sep="\n  ")
        return 1
    grouped = {}
    for site in sites:
        owner, error, rollback = OWNERS[owner_key(site["file"])]
        grouped.setdefault((site["file"], owner, error, rollback), set()).add(site["function"])
    for (file, owner, error, rollback), functions in grouped.items():
        row = next((r for r in rows if (r["file"], r["owner"], r["error"], r["rollback"])
                    == (file, owner, error, rollback)), None)
        if row is None:
            row = {"file": file, "owner": owner, "error": error, "rollback": rollback, "functions": []}
            rows.append(row)
        row["functions"] = sorted(set(row["functions"]) | functions)
    rows.sort(key=lambda r: (r["file"], r["owner"]))
    INVENTORY.write_text(json.dumps(inventory, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{len(sites)} sites added to {sum(1 for _ in grouped)} rows; the inventory has {len(rows)} rows")
    return 0


if __name__ == "__main__":
    sys.exit(main())
