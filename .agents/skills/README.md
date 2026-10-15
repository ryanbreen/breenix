# Breenix agent skills

This directory holds the skills that coding agents use when working on Breenix. Each
subdirectory is one skill: a `SKILL.md` with its instructions, plus any helper scripts it needs.
The repository's working rules live in [`AGENTS.md`](../../AGENTS.md) and
[`CLAUDE.md`](../../CLAUDE.md); where a skill and those files disagree, the root files win.

## Installing them in Claude Code

The repository is a local plugin marketplace (`.claude-plugin/marketplace.json`). From a
checkout of the repository:

```bash
/plugin marketplace add .
/plugin install breenix-development@breenix-skills
/plugin install breenix-testing@breenix-skills
```

## Skills

### breenix-development

| Skill | Use it for |
|---|---|
| `breenix-boot-analysis` | Boot sequence order, stage timing and boot failures. |
| `breenix-code-quality-check` | Zero-warning builds, clippy and log side-effect checks before a commit. |
| `breenix-gdb-attach` | One-shot GDB work: exceptions, faults, register state, page tables. |
| `breenix-gdb-chat` | Conversational GDB sessions; the implementation lives in [`tools/gdb-chat/`](../../tools/gdb-chat/). |
| `breenix-interrupt-syscall-development` | The rules for interrupt, syscall, context-switch and timer code. |
| `breenix-interrupt-trace` | QEMU interrupt tracing and privilege-transition analysis. |
| `breenix-kernel-debug-loop` | Time-bounded kernel runs that stop on a marker (`scripts/quick_debug.py`). |
| `breenix-log-analysis` | Searching serial output and logs for markers, faults and panics. |
| `breenix-memory-debugging` | Page faults, allocator problems, page tables and stack overflows. |
| `breenix-qemu-debug-session` | Setting up a full QEMU debugging session. |
| `breenix-register-watch` | Register corruption across context switches and syscalls. |
| `breenix-systematic-debugging` | Problem, root cause, solution and evidence write-ups for hard bugs. |

### breenix-testing

| Skill | Use it for |
|---|---|
| `breenix-integration-test-authoring` | Writing kernel integration tests with checkpoint markers. |

## Notes

- Breenix runs no builds or tests in hosted CI. Every check runs locally, so no skill here
  creates or edits workflow files.
- Kernel tracing and GDB come before logging: see the tracing and GDB sections of `CLAUDE.md`.
- Each skill's helper scripts locate the repository root from their own path, so run them from
  anywhere inside the checkout.
