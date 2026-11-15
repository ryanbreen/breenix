# Buffer cache counters

`/proc/block/cache` is read-only. Its decimal output has one line per counter,
in the order below; each line contains the name, count, and last sequence,
separated by single spaces and terminated by a newline. A sequence of zero
means that counter or gauge has not changed. <!-- claim-lint:ok: tools/block-host-tests/src/report.rs -->

<!-- claim-lint:ok: normative format verified by tools/block-host-tests/src/report.rs -->
| Name | Count | Last sequence |
| --- | --- | --- |
| hits | 11 | 12 |
| misses | 21 | 22 |
| device_reads | 31 | 32 |
| device_writes | 41 | 42 |
| pending_writes | 0 | 0 |
| flushes | 51 | 52 |

The values in the table are an example snapshot. Sequence numbers come from
the cache's shared clock and identify the latest event for each counter.

`device_reads`, `device_writes` and `flushes` count completed device calls,
including failures, and stamp their sequence when the call returns. Hits and
misses stamp their cache lookup result. `pending_writes` is a gauge of device
writes issued and not yet returned: it rises just before `write_block` and
falls on return, on both the WriteThrough and Hold submission paths. Its last
sequence records its last change. The write completion and gauge decrease
share a sequence and are published together. <!-- claim-lint:ok: kernel/src/block/cache.rs device_write and tools/block-host-tests/src/cache.rs write_counters_stamp_on_completion -->
