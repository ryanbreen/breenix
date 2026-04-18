# Decisions - F32f Immediate Wake Under Waitqueue Lock

## 2026-04-18T22:02:39Z - Initial Factory Scope
**Choice:** Keep this run scoped to auditing and implementing Linux-parity immediate task-context waitqueue wake only.
**Alternatives considered:** Broader AHCI, CPU routing, timer fallback, or Parallels-specific mechanisms.
**Evidence:** The task contract explicitly identifies deferred waitqueue wake latency as the remaining Linux-parity gap and prohibits those alternative mechanisms.
