# ChatDBG Analysis and Breenix GDB Skill Design

Date: 2025-12-04

## Part 1: ChatDBG Evaluation

### Overview

ChatDBG is an LLM-powered debugger assistant that achieved significant adoption (75,000+ downloads) and recognition (FSE Distinguished Artifact Award). It integrates with pdb, GDB, and LLDB to provide conversational debugging through OpenAI's GPT-4.

### Why ChatDBG Was Likely Skipped

Based on the analysis, ChatDBG was probably not recommended for Breenix for these reasons:

#### 1. **OpenAI Dependency (Critical Blocker)**
- **Hard requirement**: ChatDBG requires an OpenAI API key with active balance
- **Cost barrier**: Minimum $0.50-$1.00 credit purchase, with per-query costs
- **Vendor lock-in**: No support for alternative LLM providers (Claude, local models)
- **Breenix context**: The project uses Claude Code/Anthropic, making OpenAI dependency a non-starter

#### 2. **Architecture Mismatch**
- **Interactive focus**: ChatDBG is designed for post-mortem debugging sessions where developers manually type "why" commands
- **Synchronous flow**: User types → LLM analyzes → LLM calls debugger functions → Response
- **Human-in-the-loop**: Assumes a developer at the terminal making decisions
- **Breenix needs**: Autonomous agent-driven debugging where Claude Code orchestrates investigations programmatically

#### 3. **Integration Pattern Incompatibility**
ChatDBG's approach:
```python
# ChatDBG: Extends the debugger class itself
class ChatDBG(pdb.Pdb):
    def do_why(self, arg):
        # Sends entire context to LLM
        # Waits for LLM to call debugger functions
        # Returns formatted response
```

This won't work for Breenix because:
- **Stateful GDB process**: ChatDBG assumes it IS the debugger process
- **Python API dependency**: Uses GDB's Python API (`import gdb`), which requires running inside GDB
- **No external orchestration**: Can't be called from outside the debugger session

#### 4. **Limited Kernel Debugging Features**
- **No QEMU integration**: Doesn't handle QEMU-specific GDB remote protocol nuances
- **User-space focus**: Examples target Python, C++, Rust user programs, not kernel debugging
- **Missing kernel primitives**: No specialized support for page tables, interrupt handlers, control registers

### What Makes ChatDBG Successful (Lessons to Learn)

Despite the incompatibility, ChatDBG demonstrates valuable patterns:

#### 1. **Automatic Information Gathering**
ChatDBG gives the LLM functions to call:
```python
tools = [
    {"name": "info", "description": "Run 'info locals', 'info args', etc."},
    {"name": "print", "description": "Evaluate expressions"},
    {"name": "backtrace", "description": "Show call stack"},
    {"name": "list", "description": "Show source code"},
]
```

The LLM automatically decides what information to gather before diagnosing the problem.

**Lesson for Breenix**: The GDB skill should provide atomic operations that Claude can compose, not pre-packaged "diagnose this" functions.

#### 2. **Output Truncation and Token Management**
ChatDBG implements proportional truncation:
```python
def truncate_proportionally(text, max_tokens):
    # If output is too large, truncate middle sections
    # Preserve beginning (context) and end (crash point)
    # Return "... [1234 lines omitted] ..." markers
```

**Lesson for Breenix**: GDB output (especially backtraces, memory dumps) can be huge. The skill should include intelligent truncation.

#### 3. **Command Isolation with Output Capture**
```python
def _capture_onecmd(self, command):
    old_stdout = sys.stdout
    sys.stdout = StringIO()
    result = self.onecmd(command)
    output = sys.stdout.getvalue()
    sys.stdout = old_stdout
    return output
```

**Lesson for Breenix**: Each GDB command needs clean input/output isolation for parsing.

#### 4. **Conversation State Management**
ChatDBG maintains:
- Command history with outputs
- Error context (exception type, message, traceback)
- Stack frame state
- Conversation context for multi-turn queries

**Lesson for Breenix**: The skill needs session state management, not just one-shot commands.

#### 5. **Enriched Context Building**
Before sending to the LLM, ChatDBG enriches the prompt with:
- Exception details
- Current stack frame
- Local variables
- Recent command history
- Source code context

**Lesson for Breenix**: When Claude asks "why did this crash?", the skill should proactively gather registers, backtrace, and instruction context.

### Why Not Adapt ChatDBG Directly?

**Could we fork it and swap OpenAI for Claude?** Technically yes, but:

1. **Python API dependency**: ChatDBG relies on `import gdb` (Python inside GDB)
   - Breenix needs **external orchestration** (Python script controlling GDB)
   - Different architecture entirely

2. **Session lifecycle mismatch**: ChatDBG assumes:
   ```
   [User starts debugger] → [Load ChatDBG] → [Interactive session] → [User exits]
   ```

   Breenix needs:
   ```
   [Agent plans investigation] → [Launch GDB + QEMU] → [Run commands] → [Parse results] → [Report findings] → [Terminate]
   ```

3. **Conversation model incompatibility**: ChatDBG maintains conversation state inside the debugger. Breenix needs Claude Code to maintain state, with the GDB skill as a stateless command executor.

### Conclusion: ChatDBG Analysis

ChatDBG is an excellent tool for its use case (interactive post-mortem debugging with human developers), but:
- **OpenAI dependency** makes it incompatible with Breenix's Claude-based workflow
- **Architecture** assumes the LLM is embedded in the debugger, not orchestrating from outside
- **Integration pattern** doesn't support agent-driven programmatic control

However, we should **borrow these patterns**:
- Atomic operations for LLM to compose
- Output truncation and token management
- Clean command isolation
- Enriched context gathering
- Session state management

---

## Part 2: Breenix GDB Skill Design

### Design Philosophy

The Breenix GDB skill should be a **conversational interface** where Claude can:
1. Say "Let me check the registers" → Get current register state
2. Say "Setting breakpoint at sys_clock_gettime" → Breakpoint confirmed
3. Say "What instruction caused the page fault?" → Get faulting RIP + disassembly

Key principles:
- **Fast turnaround**: Each operation completes in <1 second
- **Stateful session**: GDB process persists across multiple Claude queries
- **Kernel-specific**: Tailored primitives for Breenix debugging
- **Agent-friendly**: Designed for programmatic use, not human interaction

### Architecture: Two-Process Model

```
┌─────────────────────────────────────────────────────────────┐
│                    Claude Code Agent                         │
│  "Let me check if the page table is valid..."               │
└──────────────────┬──────────────────────────────────────────┘
                   │
                   ↓ (calls skill script)
┌─────────────────────────────────────────────────────────────┐
│              breenix-gdb-chat/scripts/gdb_cmd.py            │
│  - Maintains GDB session in background                       │
│  - Sends command via GDB/MI or console                       │
│  - Parses output, returns structured data                    │
└──────────────────┬──────────────────────────────────────────┘
                   │
                   ↓ (communicates via stdin/stdout)
┌─────────────────────────────────────────────────────────────┐
│                   GDB Process                                │
│  - Connected to QEMU on localhost:1234                       │
│  - Kernel loaded with symbols                                │
│  - Breakpoints, watchpoints, etc.                           │
└──────────────────┬──────────────────────────────────────────┘
                   │
                   ↓ (GDB remote protocol)
┌─────────────────────────────────────────────────────────────┐
│            QEMU (running Breenix kernel)                     │
│  - Paused at breakpoint or running                           │
│  - Exposes GDB server on port 1234                          │
└─────────────────────────────────────────────────────────────┘
```

### Session Lifecycle

#### 1. **Session Creation**
```bash
# Claude invokes skill to start debug session
breenix-gdb-chat/scripts/start_session.py --mode uefi --timeout 300
```

Output:
```json
{
  "session_id": "gdb_20251204_143022",
  "pid": 12345,
  "status": "connected",
  "kernel_binary": "/Users/wrb/fun/code/breenix/target/x86_64-breenix/release/kernel",
  "qemu_pid": 12346,
  "socket": "/tmp/breenix_gdb_12345.sock"
}
```

What happens:
1. Launches QEMU with `-s -S` (GDB server on localhost:1234, paused)
2. Starts GDB in background with kernel symbols loaded
3. Connects GDB to QEMU
4. Loads .gdbinit custom commands
5. Sets up communication socket for commands
6. Returns session ID

#### 2. **Command Execution**
```bash
# Claude sends command to active session
breenix-gdb-chat/scripts/gdb_cmd.py --session gdb_20251204_143022 --command "info registers"
```

Output:
```json
{
  "command": "info registers",
  "success": true,
  "output": {
    "rax": "0x0000000000000000",
    "rbx": "0xffff800000010000",
    "rcx": "0x0000000000000001",
    "rip": "0xffffffff80001234",
    "cs": "0x0008",
    "rflags": "0x0000000000000202"
  },
  "raw": "rax            0x0    0\nrbx            0xffff800000010000...",
  "execution_time_ms": 45
}
```

#### 3. **Session Termination**
```bash
# Claude cleans up when done
breenix-gdb-chat/scripts/stop_session.py --session gdb_20251204_143022
```

Output:
```json
{
  "session_id": "gdb_20251204_143022",
  "status": "terminated",
  "total_commands": 27,
  "session_duration_s": 183
}
```

### File Structure

```
breenix-gdb-chat/
├── SKILL.md                      # Skill documentation (usage, patterns)
├── scripts/
│   ├── start_session.py          # Launch GDB + QEMU session
│   ├── gdb_cmd.py                # Execute command in active session
│   ├── stop_session.py           # Terminate session cleanly
│   ├── list_sessions.py          # Show active debug sessions
│   └── lib/
│       ├── gdb_controller.py     # GDB process management
│       ├── gdb_parser.py         # Parse GDB output to structured data
│       ├── qemu_manager.py       # QEMU lifecycle management
│       └── kernel_context.py     # Breenix-specific context helpers
├── tests/
│   ├── test_session_lifecycle.py
│   ├── test_command_parsing.py
│   └── test_kernel_context.py
└── examples/
    ├── page_fault_investigation.md    # Example debugging session
    └── syscall_trace.md               # Example syscall debugging
```

### Skill Interface: SKILL.md Excerpt

```markdown
---
name: gdb-chat
description: Conversational GDB debugging for Breenix kernel. Use for interactive debugging sessions where you need to inspect registers, set breakpoints, examine memory, trace execution, or investigate crashes. Maintains stateful GDB session across multiple queries.
---

# Breenix GDB Chat

## When to Use This Skill

Use this skill when you need **interactive, stateful GDB debugging** with back-and-forth investigation:

- **Multi-step investigations**: "Check register, then if X is null, examine memory at Y"
- **Conditional analysis**: "Set breakpoint, continue, then analyze state"
- **Iterative hypothesis testing**: "Try setting watchpoint, see what writes to this address"
- **Crash analysis**: "Start at crash point, walk back through frames, check each"

**Don't use this skill for:**
- One-shot GDB attach (use `breenix-gdb-attach` skill instead)
- Log analysis without GDB (use `breenix-log-analysis` skill)
- Running tests (use appropriate test skills)

## Quick Start

### Starting a Debug Session

```bash
# Start GDB session with UEFI kernel
breenix-gdb-chat/scripts/start_session.py

# With custom timeout (default: 300s)
breenix-gdb-chat/scripts/start_session.py --timeout 600

# BIOS mode
breenix-gdb-chat/scripts/start_session.py --mode bios
```

The script will output a session ID like `gdb_20251204_143022`. Save this for subsequent commands.

### Sending Commands

```bash
# Basic command
breenix-gdb-chat/scripts/gdb_cmd.py --session SESSION_ID --command "info registers"

# Multiple commands (executed in sequence)
breenix-gdb-chat/scripts/gdb_cmd.py --session SESSION_ID \
  --command "break sys_clock_gettime" \
  --command "continue"

# Get structured output (JSON)
breenix-gdb-chat/scripts/gdb_cmd.py --session SESSION_ID \
  --command "info registers" \
  --format json

# Human-readable output
breenix-gdb-chat/scripts/gdb_cmd.py --session SESSION_ID \
  --command "backtrace" \
  --format text
```

### Stopping a Session

```bash
breenix-gdb-chat/scripts/stop_session.py --session SESSION_ID
```

## Available Commands

### Execution Control

| Command | Description | Example |
|---------|-------------|---------|
| `continue` / `c` | Continue execution | `--command "continue"` |
| `step` / `s` | Step one source line | `--command "step"` |
| `stepi` / `si` | Step one instruction | `--command "stepi"` |
| `next` / `n` | Step over function calls | `--command "next"` |
| `nexti` / `ni` | Step over one instruction | `--command "nexti"` |
| `finish` | Run until function returns | `--command "finish"` |

### Breakpoints

| Command | Description | Example |
|---------|-------------|---------|
| `break LOCATION` | Set breakpoint | `--command "break kernel_main"` |
| `hbreak LOCATION` | Hardware breakpoint | `--command "hbreak _start"` |
| `tbreak LOCATION` | Temporary breakpoint | `--command "tbreak *0xffff800000001000"` |
| `watch EXPR` | Watch for writes | `--command "watch *0xffff800000010000"` |
| `rwatch EXPR` | Watch for reads | `--command "rwatch global_var"` |
| `info breakpoints` | List breakpoints | `--command "info breakpoints"` |
| `delete N` | Delete breakpoint | `--command "delete 1"` |

### Register Inspection

| Command | Description | Example |
|---------|-------------|---------|
| `info registers` | Show all registers | `--command "info registers"` |
| `print $REG` | Show specific register | `--command "print/x $rip"` |
| `show-segments` | Show segment registers | `--command "show-segments"` |
| `show-control` | Show control registers | `--command "show-control"` |
| `show-all-registers` | Show everything | `--command "show-all-registers"` |

### Memory Inspection

| Command | Description | Example |
|---------|-------------|---------|
| `x/FMT ADDR` | Examine memory | `--command "x/16xg $rsp"` |
| `disassemble` | Disassemble current | `--command "disassemble"` |
| `disassemble FUNC` | Disassemble function | `--command "disassemble kernel_main"` |

### Stack & Frames

| Command | Description | Example |
|---------|-------------|---------|
| `backtrace` / `bt` | Show call stack | `--command "backtrace"` |
| `backtrace full` | Stack + locals | `--command "backtrace full"` |
| `frame N` | Select frame | `--command "frame 0"` |
| `info locals` | Show local variables | `--command "info locals"` |
| `show-stack` | Pretty stack dump | `--command "show-stack 32"` |

## Kernel-Specific Helpers

The skill includes Breenix-specific helper commands from `.gdbinit`:

```bash
# Connect to QEMU (called automatically during start_session)
--command "breenix-connect"

# Set common kernel breakpoints
--command "breenix-breaks"

# Show execution context (registers + instruction + stack)
--command "ctx"
```

## Usage Patterns for Claude

### Pattern 1: Crash Investigation

```python
# Claude's debugging flow:
# 1. Start session
result = start_session()
session_id = result['session_id']

# 2. Check where we are
gdb_cmd(session_id, "info registers")
# → See that RIP is at page_fault_handler

# 3. Get faulting address
gdb_cmd(session_id, "print/x $cr2")
# → See CR2 = 0x0000000000000008

# 4. Check instruction that faulted
gdb_cmd(session_id, "x/i $rip")
# → See: mov rax, qword ptr [rax + 8]

# 5. Check RAX value
gdb_cmd(session_id, "print/x $rax")
# → RAX was null, null pointer dereference confirmed

# 6. Get call stack
gdb_cmd(session_id, "backtrace")
# → See path: kernel_main → init_subsystem → setup_device

# 7. Investigate caller
gdb_cmd(session_id, "frame 1")
gdb_cmd(session_id, "info locals")
# → See which variable was null

# 8. Report findings
stop_session(session_id)
```

### Pattern 2: Conditional Breakpoint Investigation

```python
# Investigate: "Why does syscall 228 fail?"

session_id = start_session()['session_id']

# Set conditional breakpoint
gdb_cmd(session_id, "break rust_syscall_handler if $rax == 228")

# Continue until syscall 228
gdb_cmd(session_id, "continue")

# Check registers
regs = gdb_cmd(session_id, "info registers")
# → RDI has the argument

# Step through handler
gdb_cmd(session_id, "step")
gdb_cmd(session_id, "step")
gdb_cmd(session_id, "step")

# Check return value
gdb_cmd(session_id, "print/x $rax")
# → -ENOSYS

# Diagnose: syscall not implemented
stop_session(session_id)
```

### Pattern 3: Memory Corruption Hunt

```python
# Find: "What writes to this memory location?"

session_id = start_session()['session_id']

# Set watchpoint on critical memory
gdb_cmd(session_id, "watch *(uint64_t*)0xffff800000010000")

# Continue until write occurs
gdb_cmd(session_id, "continue")
# → Watchpoint hit!

# See what instruction wrote
gdb_cmd(session_id, "x/i $rip")

# Check what value was written
gdb_cmd(session_id, "x/xg 0xffff800000010000")

# Get backtrace to find caller
gdb_cmd(session_id, "backtrace")

stop_session(session_id)
```

## Integration with Other Skills

### With `breenix-kernel-debug-loop`

Use debug loop to reproduce the issue, then attach GDB for detailed inspection:

```bash
# 1. Reproduce with debug loop
breenix-kernel-debug-loop/scripts/quick_debug.py --signal "CRASH" --timeout 10

# 2. See crash in logs, now debug with GDB
breenix-gdb-chat/scripts/start_session.py
# ... investigate with GDB commands ...
```

### With `breenix-log-analysis`

Use logs to identify suspicious behavior, then use GDB to investigate:

```bash
# 1. Find anomaly in logs
echo '"Page fault at"' > /tmp/log-query.txt
./scripts/find-in-logs

# 2. Set breakpoint at page fault handler
# ... use GDB chat to investigate ...
```

### With `breenix-systematic-debugging`

Use GDB chat to gather evidence for systematic debugging documents:

1. **Problem**: Document symptoms from logs
2. **Root Cause**: Use GDB chat to investigate state at crash
3. **Solution**: Verify fix with GDB chat session
4. **Evidence**: Include GDB output showing before/after

## Technical Details

### Session State Management

Sessions are stored in `/tmp/breenix_gdb_sessions/`:
```
/tmp/breenix_gdb_sessions/
├── gdb_20251204_143022.pid       # GDB process PID
├── gdb_20251204_143022.sock      # Communication socket
├── gdb_20251204_143022.log       # Command history
└── gdb_20251204_143022.qemu      # QEMU PID
```

### Command Timeout

Each command has a default timeout of 30 seconds. Override with:
```bash
breenix-gdb-chat/scripts/gdb_cmd.py --session SESSION_ID \
  --command "continue" \
  --timeout 60
```

### Output Formats

#### JSON Format (default for programmatic use)
```json
{
  "command": "info registers",
  "success": true,
  "output": {
    "rax": "0x0000000000000000",
    "rip": "0xffffffff80001234"
  },
  "raw": "rax            0x0    0\n...",
  "execution_time_ms": 45
}
```

#### Text Format (human-readable)
```
rax            0x0    0
rbx            0xffff800000010000    281474976776192
rcx            0x1    1
...
```

### Error Handling

If a command fails, the response includes error details:
```json
{
  "command": "break invalid_function",
  "success": false,
  "error": "Function 'invalid_function' not defined.",
  "error_type": "gdb_error",
  "execution_time_ms": 12
}
```

Claude can handle this and adjust strategy.

## Best Practices

1. **Always start with execution context**: Use `ctx` to understand where you are
2. **Set hardware breakpoints early**: Before paging is set up, use `hbreak`
3. **Use conditional breakpoints**: Narrow down to specific conditions
4. **Check control registers for faults**: CR2 for page faults, CR3 for page table
5. **Step carefully in interrupt handlers**: Timer interrupts can fire frequently
6. **Always stop sessions**: Clean up with `stop_session.py` when done
7. **Check session list**: Use `list_sessions.py` to find orphaned sessions

## Troubleshooting

### "Session not found"
The GDB process may have died. Check:
```bash
breenix-gdb-chat/scripts/list_sessions.py
```
Start a new session if needed.

### "Command timeout"
QEMU or GDB is hung. Stop the session and investigate:
```bash
breenix-gdb-chat/scripts/stop_session.py --session SESSION_ID --force
```

### "Cannot connect to QEMU"
QEMU may not be ready. Check QEMU logs:
```bash
# Session start script logs QEMU output to:
/tmp/breenix_gdb_sessions/SESSION_ID.qemu.log
```

## Examples

See `examples/` directory for complete debugging session walkthroughs:
- `page_fault_investigation.md`: Diagnosing null pointer dereferences
- `syscall_trace.md`: Tracing syscall execution paths
```

### Implementation: Key Components

#### 1. **gdb_controller.py** (GDB Process Management)

```python
#!/usr/bin/env python3
"""
GDB process controller for Breenix debugging.
Manages GDB lifecycle, command execution, and output parsing.
"""

import os
import subprocess
import time
import signal
import json
from pathlib import Path
from typing import Optional, Dict, Any
import pexpect

class GDBSession:
    """Manages a GDB debugging session connected to QEMU."""

    def __init__(self, session_id: str, kernel_binary: Path, mode: str = "uefi"):
        self.session_id = session_id
        self.kernel_binary = kernel_binary
        self.mode = mode
        self.gdb_process: Optional[pexpect.spawn] = None
        self.qemu_process: Optional[subprocess.Popen] = None
        self.session_dir = Path(f"/tmp/breenix_gdb_sessions")
        self.session_dir.mkdir(exist_ok=True)
        self.log_file = self.session_dir / f"{session_id}.log"
        self.command_count = 0
        self.start_time = time.time()

    def start(self, timeout: int = 300) -> Dict[str, Any]:
        """Start GDB session and connect to QEMU."""

        # 1. Start QEMU in GDB mode
        self.qemu_process = self._start_qemu()

        # Wait for QEMU to be ready (GDB server on port 1234)
        time.sleep(2)

        # 2. Start GDB with kernel symbols
        gdb_cmd = [
            "gdb",
            str(self.kernel_binary),
            "-ex", "set pagination off",
            "-ex", "set confirm off",
            "-ex", "set print pretty on",
            "-ex", f"source {Path.home() / 'fun/code/breenix/.gdbinit'}",
            "-ex", "target remote localhost:1234",
        ]

        self.gdb_process = pexpect.spawn(
            " ".join(gdb_cmd),
            encoding='utf-8',
            timeout=30,
            logfile=open(self.log_file, 'w')
        )

        # Wait for GDB prompt
        self.gdb_process.expect(r'\(gdb\)')

        # Save session metadata
        self._save_metadata()

        return {
            "session_id": self.session_id,
            "pid": self.gdb_process.pid,
            "qemu_pid": self.qemu_process.pid,
            "status": "connected",
            "kernel_binary": str(self.kernel_binary),
            "mode": self.mode,
        }

    def execute_command(self, command: str, timeout: int = 30) -> Dict[str, Any]:
        """Execute a GDB command and return parsed output."""

        if not self.gdb_process or not self.gdb_process.isalive():
            return {
                "success": False,
                "error": "GDB process not running",
                "error_type": "session_dead"
            }

        start = time.time()

        try:
            # Send command
            self.gdb_process.sendline(command)

            # Wait for prompt (command finished)
            self.gdb_process.expect(r'\(gdb\)', timeout=timeout)

            # Get output (everything between our command and the prompt)
            output = self.gdb_process.before

            # Remove echo of our command (first line)
            lines = output.split('\n')
            if lines and lines[0].strip() == command.strip():
                lines = lines[1:]
            output = '\n'.join(lines).strip()

            execution_time = int((time.time() - start) * 1000)
            self.command_count += 1

            # Parse output based on command type
            parsed = self._parse_output(command, output)

            return {
                "command": command,
                "success": True,
                "output": parsed,
                "raw": output,
                "execution_time_ms": execution_time
            }

        except pexpect.TIMEOUT:
            return {
                "command": command,
                "success": False,
                "error": f"Command timed out after {timeout}s",
                "error_type": "timeout"
            }
        except pexpect.EOF:
            return {
                "command": command,
                "success": False,
                "error": "GDB process terminated unexpectedly",
                "error_type": "process_died"
            }
        except Exception as e:
            return {
                "command": command,
                "success": False,
                "error": str(e),
                "error_type": "unknown"
            }

    def stop(self) -> Dict[str, Any]:
        """Stop GDB and QEMU processes cleanly."""

        duration = time.time() - self.start_time

        # Stop GDB
        if self.gdb_process and self.gdb_process.isalive():
            self.gdb_process.sendline("quit")
            try:
                self.gdb_process.expect(pexpect.EOF, timeout=5)
            except:
                self.gdb_process.kill(signal.SIGKILL)

        # Stop QEMU
        if self.qemu_process:
            self.qemu_process.terminate()
            try:
                self.qemu_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.qemu_process.kill()

        # Clean up session files
        self._cleanup_session()

        return {
            "session_id": self.session_id,
            "status": "terminated",
            "total_commands": self.command_count,
            "session_duration_s": int(duration)
        }

    def _start_qemu(self) -> subprocess.Popen:
        """Start QEMU with GDB server enabled."""

        env = os.environ.copy()
        env["BREENIX_GDB"] = "1"

        if self.mode == "bios":
            cmd = ["cargo", "run", "--release", "--bin", "qemu-bios"]
        else:
            cmd = ["cargo", "run", "--release", "--bin", "qemu-uefi"]

        cmd.extend(["--", "-serial", "stdio", "-display", "none"])

        qemu_log = self.session_dir / f"{self.session_id}.qemu.log"

        return subprocess.Popen(
            cmd,
            stdout=open(qemu_log, 'w'),
            stderr=subprocess.STDOUT,
            env=env,
            cwd=Path.home() / "fun/code/breenix"
        )

    def _parse_output(self, command: str, output: str) -> Any:
        """Parse GDB output into structured data."""

        # Delegate to specialized parsers
        from .gdb_parser import parse_registers, parse_backtrace, parse_memory

        if command.startswith("info registers"):
            return parse_registers(output)
        elif command.startswith("backtrace") or command.startswith("bt"):
            return parse_backtrace(output)
        elif command.startswith("x/"):
            return parse_memory(output)
        else:
            # Return raw for unsupported commands
            return output

    def _save_metadata(self):
        """Save session metadata to disk."""
        metadata = {
            "session_id": self.session_id,
            "gdb_pid": self.gdb_process.pid,
            "qemu_pid": self.qemu_process.pid,
            "kernel_binary": str(self.kernel_binary),
            "mode": self.mode,
            "start_time": self.start_time,
        }

        metadata_file = self.session_dir / f"{self.session_id}.json"
        with open(metadata_file, 'w') as f:
            json.dump(metadata, f, indent=2)

    def _cleanup_session(self):
        """Remove session files."""
        for file in self.session_dir.glob(f"{self.session_id}.*"):
            try:
                file.unlink()
            except:
                pass
```

#### 2. **gdb_parser.py** (Output Parsing)

```python
#!/usr/bin/env python3
"""
GDB output parser for structured data extraction.
Converts GDB console output into JSON-friendly dictionaries.
"""

import re
from typing import Dict, List, Any

def parse_registers(output: str) -> Dict[str, str]:
    """Parse 'info registers' output into structured dict."""

    registers = {}

    # Match lines like: "rax            0x0    0"
    pattern = r'(\w+)\s+0x([0-9a-fA-F]+)\s+(\d+)'

    for line in output.split('\n'):
        match = re.search(pattern, line)
        if match:
            reg_name = match.group(1)
            hex_value = match.group(2)
            registers[reg_name] = f"0x{hex_value}"

    return registers

def parse_backtrace(output: str) -> List[Dict[str, Any]]:
    """Parse backtrace output into list of frames."""

    frames = []

    # Match lines like: "#0  0xffffffff80001234 in kernel_main () at kernel/src/main.rs:42"
    pattern = r'#(\d+)\s+(0x[0-9a-fA-F]+)\s+in\s+(\S+)\s+\(([^)]*)\)\s+at\s+([^:]+):(\d+)'

    for line in output.split('\n'):
        match = re.search(pattern, line)
        if match:
            frame = {
                "number": int(match.group(1)),
                "address": match.group(2),
                "function": match.group(3),
                "args": match.group(4),
                "file": match.group(5),
                "line": int(match.group(6))
            }
            frames.append(frame)

    return frames

def parse_memory(output: str) -> List[str]:
    """Parse memory examination (x/ command) output."""

    values = []

    # Match lines like: "0xffff800000010000:    0x0000000000000000    0x0000000000000001"
    pattern = r'0x[0-9a-fA-F]+:\s+((?:0x[0-9a-fA-F]+\s*)+)'

    for line in output.split('\n'):
        match = re.search(pattern, line)
        if match:
            # Extract all hex values from this line
            hex_values = re.findall(r'0x[0-9a-fA-F]+', match.group(1))
            values.extend(hex_values)

    return values

def truncate_output(text: str, max_lines: int = 100) -> str:
    """
    Truncate large output to prevent token overflow.
    Inspired by ChatDBG's proportional truncation.
    """

    lines = text.split('\n')

    if len(lines) <= max_lines:
        return text

    # Keep first 40%, skip middle, keep last 40%
    keep_top = int(max_lines * 0.4)
    keep_bottom = int(max_lines * 0.4)
    omitted = len(lines) - keep_top - keep_bottom

    result = '\n'.join(lines[:keep_top])
    result += f"\n... [{omitted} lines omitted] ...\n"
    result += '\n'.join(lines[-keep_bottom:])

    return result
```

#### 3. **start_session.py** (Session Launcher)

```python
#!/usr/bin/env python3
"""
Start a GDB debugging session for Breenix kernel.
"""

import argparse
import sys
import json
from datetime import datetime
from pathlib import Path

# Add lib directory to path
sys.path.insert(0, str(Path(__file__).parent / "lib"))

from gdb_controller import GDBSession

def main():
    parser = argparse.ArgumentParser(
        description='Start GDB debugging session for Breenix'
    )
    parser.add_argument(
        '--mode',
        choices=['uefi', 'bios'],
        default='uefi',
        help='Boot mode (default: uefi)'
    )
    parser.add_argument(
        '--timeout',
        type=int,
        default=300,
        help='Session timeout in seconds (default: 300)'
    )
    parser.add_argument(
        '--kernel',
        type=Path,
        default=Path.home() / "fun/code/breenix/target/x86_64-breenix/release/kernel",
        help='Path to kernel binary'
    )

    args = parser.parse_args()

    # Generate session ID
    session_id = f"gdb_{datetime.now().strftime('%Y%m%d_%H%M%S')}"

    # Create and start session
    session = GDBSession(session_id, args.kernel, args.mode)

    try:
        result = session.start(timeout=args.timeout)
        print(json.dumps(result, indent=2))
        sys.exit(0)
    except Exception as e:
        print(json.dumps({
            "success": False,
            "error": str(e),
            "error_type": "start_failed"
        }), file=sys.stderr)
        sys.exit(1)

if __name__ == '__main__':
    main()
```

#### 4. **gdb_cmd.py** (Command Executor)

```python
#!/usr/bin/env python3
"""
Execute command in active GDB session.
"""

import argparse
import sys
import json
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent / "lib"))

from gdb_controller import GDBSession

def load_session(session_id: str) -> GDBSession:
    """Load existing session from metadata."""

    session_dir = Path("/tmp/breenix_gdb_sessions")
    metadata_file = session_dir / f"{session_id}.json"

    if not metadata_file.exists():
        raise FileNotFoundError(f"Session {session_id} not found")

    with open(metadata_file) as f:
        metadata = json.load(f)

    # Reconstruct session object (simplified for example)
    session = GDBSession(
        session_id=metadata["session_id"],
        kernel_binary=Path(metadata["kernel_binary"]),
        mode=metadata["mode"]
    )

    # Reattach to existing processes (implementation detail)
    session._reattach(metadata["gdb_pid"], metadata["qemu_pid"])

    return session

def main():
    parser = argparse.ArgumentParser(
        description='Execute command in GDB session'
    )
    parser.add_argument(
        '--session',
        required=True,
        help='Session ID'
    )
    parser.add_argument(
        '--command',
        required=True,
        action='append',
        help='GDB command to execute (can specify multiple)'
    )
    parser.add_argument(
        '--timeout',
        type=int,
        default=30,
        help='Command timeout in seconds'
    )
    parser.add_argument(
        '--format',
        choices=['json', 'text'],
        default='json',
        help='Output format'
    )

    args = parser.parse_args()

    try:
        session = load_session(args.session)

        results = []
        for cmd in args.command:
            result = session.execute_command(cmd, timeout=args.timeout)
            results.append(result)

        if args.format == 'json':
            if len(results) == 1:
                print(json.dumps(results[0], indent=2))
            else:
                print(json.dumps(results, indent=2))
        else:
            for result in results:
                if result['success']:
                    print(result['raw'])
                else:
                    print(f"Error: {result['error']}", file=sys.stderr)

        # Exit with error if any command failed
        if any(not r['success'] for r in results):
            sys.exit(1)

    except Exception as e:
        print(json.dumps({
            "success": False,
            "error": str(e),
            "error_type": "execution_failed"
        }), file=sys.stderr)
        sys.exit(1)

if __name__ == '__main__':
    main()
```

### Sample Interaction: How Claude Uses the Skill

#### Example: Investigating a Page Fault

```
[Claude's internal reasoning]
User reported: "Kernel crashes with page fault during sys_clock_gettime"
Plan: Use GDB chat to investigate the crash state

[Claude executes]
```

**Step 1: Start debug session**
```bash
$ breenix-gdb-chat/scripts/start_session.py
```

Output:
```json
{
  "session_id": "gdb_20251204_151030",
  "status": "connected",
  "kernel_binary": "/Users/wrb/fun/code/breenix/target/x86_64-breenix/release/kernel"
}
```

**Step 2: Set breakpoint at page fault handler**
```bash
$ breenix-gdb-chat/scripts/gdb_cmd.py \
  --session gdb_20251204_151030 \
  --command "break page_fault_handler"
```

Output:
```json
{
  "command": "break page_fault_handler",
  "success": true,
  "output": "Breakpoint 1 at 0xffffffff80003450: file kernel/src/interrupts.rs, line 87.",
  "execution_time_ms": 23
}
```

**Step 3: Continue until page fault**
```bash
$ breenix-gdb-chat/scripts/gdb_cmd.py \
  --session gdb_20251204_151030 \
  --command "continue"
```

Output:
```json
{
  "command": "continue",
  "success": true,
  "output": "Continuing.\n\nBreakpoint 1, page_fault_handler () at kernel/src/interrupts.rs:87",
  "execution_time_ms": 1243
}
```

**Step 4: Check execution context**
```bash
$ breenix-gdb-chat/scripts/gdb_cmd.py \
  --session gdb_20251204_151030 \
  --command "ctx"
```

Output:
```json
{
  "success": true,
  "raw": "=== Instruction Context ===\nRIP: 0xffffffff80003450\n0xffffffff80003450 <page_fault_handler>:    push   rbp\n...\n=== Registers ===\nRAX: 0x0000000000000000  RBX: 0xffff800000010000 ...\n=== Stack Top ===\n0xffff800000008ff0:    0xffffffff80002134 ..."
}
```

**Step 5: Get faulting address**
```bash
$ breenix-gdb-chat/scripts/gdb_cmd.py \
  --session gdb_20251204_151030 \
  --command "print/x \$cr2"
```

Output:
```json
{
  "success": true,
  "output": "0x0000000000000008"
}
```

**Step 6: Get backtrace**
```bash
$ breenix-gdb-chat/scripts/gdb_cmd.py \
  --session gdb_20251204_151030 \
  --command "backtrace"
```

Output:
```json
{
  "success": true,
  "output": [
    {
      "number": 0,
      "function": "page_fault_handler",
      "file": "kernel/src/interrupts.rs",
      "line": 87
    },
    {
      "number": 1,
      "function": "sys_clock_gettime",
      "file": "kernel/src/syscall/handler.rs",
      "line": 234
    },
    {
      "number": 2,
      "function": "rust_syscall_handler",
      "file": "kernel/src/syscall/handler.rs",
      "line": 45
    }
  ]
}
```

**Step 7: Investigate frame 1 (sys_clock_gettime)**
```bash
$ breenix-gdb-chat/scripts/gdb_cmd.py \
  --session gdb_20251204_151030 \
  --command "frame 1" \
  --command "info locals"
```

Output:
```json
[
  {
    "command": "frame 1",
    "success": true,
    "output": "#1  0xffffffff80002134 in sys_clock_gettime () at kernel/src/syscall/handler.rs:234"
  },
  {
    "command": "info locals",
    "success": true,
    "output": "clock_id = 1\ntp = 0x0\ntsc = <optimized out>"
  }
]
```

**Step 8: Stop session**
```bash
$ breenix-gdb-chat/scripts/stop_session.py --session gdb_20251204_151030
```

**Claude reports to user:**
```
I found the root cause of the page fault in sys_clock_gettime:

**Problem**: Null pointer dereference at offset 0x8
**Location**: kernel/src/syscall/handler.rs:234 in sys_clock_gettime()
**Root Cause**: The `tp` parameter (timespec pointer) is null (0x0)

**Analysis**:
- CR2 (faulting address) = 0x0000000000000008
- This is null + 8 bytes, indicating access to a struct field
- Local variable `tp = 0x0` confirms null pointer
- The syscall handler didn't validate the timespec pointer before dereferencing

**Fix**: Add null pointer check before accessing tp:
```rust
if tp.is_null() {
    return -EFAULT;
}
```

Would you like me to implement this fix?
```

---

## Summary

### Task 1: ChatDBG Evaluation

**Why it was skipped**:
1. Hard OpenAI API dependency (incompatible with Claude-based workflow)
2. Architecture mismatch (embedded in debugger vs external orchestration)
3. Integration pattern incompatibility (Python API inside GDB vs external control)

**What to learn from it**:
1. Atomic operations for LLM composition
2. Output truncation and token management
3. Clean command isolation
4. Enriched context gathering
5. Session state management

### Task 2: GDB Skill Design

**Key features**:
- **Stateful sessions**: GDB + QEMU persist across multiple Claude queries
- **Fast turnaround**: <1s per command
- **Conversational interface**: Claude says "check registers" → get registers
- **Kernel-specific**: Tailored for Breenix debugging (page tables, interrupts, etc.)
- **Agent-friendly**: Designed for programmatic use by Claude Code

**Architecture**:
- Two-process model: Python controller + persistent GDB session
- Session lifecycle: start → execute commands → stop
- Structured output: JSON-friendly parsing of GDB console output
- Clean integration: Works with existing Breenix skills (debug-loop, log-analysis)

**Next steps**:
1. Implement core scripts (start_session.py, gdb_cmd.py, stop_session.py)
2. Build gdb_controller.py and gdb_parser.py libraries
3. Add kernel-specific helpers (page table walkers, TSC readers)
4. Write tests and example sessions
5. Integrate with existing Breenix debugging workflows
