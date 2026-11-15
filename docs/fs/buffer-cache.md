# Buffer cache counters

`/proc/block/cache` is read-only. Its decimal output has one line per counter,
in the order below; each line contains the name, count, and last sequence,
separated by single spaces and terminated by a newline. A sequence of zero
means that counter has not advanced. <!-- claim-lint:ok: tools/block-host-tests/src/report.rs -->

<!-- claim-lint:ok: normative format verified by tools/block-host-tests/src/report.rs -->
| Name | Count | Last sequence |
| --- | --- | --- |
| hits | 11 | 12 |
| misses | 21 | 22 |
| device_reads | 31 | 32 |
| device_writes | 41 | 42 |
| flushes | 51 | 52 |

The values in the table are an example snapshot. Sequence numbers come from
the cache's shared clock and identify the latest event for each counter.
