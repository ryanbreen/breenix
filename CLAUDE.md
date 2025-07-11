# Breenix OS

## Project Overview

Breenix is an x86_64 operating system kernel written in Rust. **This is NOT a toy or learning project - we are building a production-quality operating system for the long haul.**

## 🚨 CRITICAL COMMAND LINE POLICY 🚨

**NEVER generate unique bash commands that require user approval**

Claude Code MUST use reusable utilities and scripts instead of creating new command lines each time:
- Use `./scripts/find-in-logs` for ALL log searching (configure via `/tmp/log-query.txt`)
- Create reusable scripts for common operations rather than inline commands
- Avoid commands with varying parameters that trigger approval prompts
- If a new utility is needed, create it as a script FIRST, then use it consistently

This policy ensures smooth workflow without constant approval interruptions.

## 🚨 CRITICAL DESIGN PRINCIPLE 🚨

**ALWAYS FOLLOW OS-STANDARD PRACTICES - NO SHORTCUTS**

Under **NO CIRCUMSTANCES** should you choose "easy" workarounds that deviate from standard OS development practices. When implementing any feature:

- **Follow Linux/FreeBSD patterns**: If real operating systems do it a certain way, that's our standard
- **No quick hacks**: Don't implement temporary solutions that avoid complexity  
- **Build for production**: Every design decision must scale to a real OS
- **Quality over speed**: Take the time to implement features correctly the first time

**Examples of REQUIRED standard practices:**
- Page table switching during exec() ELF loading (not double-mapping)
- Proper copy-on-write fork() implementation
- Standard syscall interfaces and semantics
- Real virtual memory management with proper isolation
- Proper interrupt and exception handling

**If it's good enough for Linux, it's the standard we follow.**

### Current Status
- Basic bootloader integration using the `bootloader` crate
- Framebuffer graphics support with pixel rendering capabilities
- Custom target specification for bare metal x86_64
- Legacy codebase being migrated/rewritten (see `src.legacy/` directory)

## Architecture

### Directory Structure
```
breenix/
├── kernel/          # Core kernel implementation
│   └── src/
│       ├── main.rs       # Kernel entry point
│       └── framebuffer.rs # Graphics/display handling
├── src/             # Build system entry point
├── src.legacy/      # Previous implementation (being phased out)
├── libs/            # Supporting libraries
│   ├── libbreenix/  # System call interface library
│   └── tiered_allocator/ # Memory allocation library
├── tests/           # Integration tests
└── build.rs         # Build script for disk image creation
```

### Key Components

1. **Kernel** (`kernel/`): The main kernel binary that runs after boot
   - No standard library (`#![no_std]`)
   - Custom panic handler
   - Basic framebuffer graphics using `embedded-graphics`

2. **Build System**:
   - Uses cargo workspaces
   - Custom build script creates UEFI and BIOS bootable disk images
   - Artifact dependencies for kernel binary

3. **Legacy Code** (`src.legacy/`):
   - Contains previous implementation including:
     - Interrupt handling (GDT, IDT)
     - Memory management
     - Task scheduling
     - Device drivers (VGA, serial, keyboard, network)
     - PCI support

4. **MCP Integration** (`mcp/`): Model Context Protocol server for programmatic kernel interaction
   - HTTP server providing tools for Claude Code integration  
   - Real-time kernel log streaming and command injection
   - Process lifecycle management for QEMU/Breenix sessions
   - RESTful API and JSON-RPC endpoints for automation
   - **Note**: MCP is now optional - see direct cargo commands below

## Running Breenix (Direct Cargo Commands)

**IMPORTANT: All kernel runs produce timestamped log files in the `logs/` directory**

### Quick Start

```bash
# Run Breenix with automatic logging
./scripts/run_breenix.sh

# Run with specific options
./scripts/run_breenix.sh uefi -display none
./scripts/run_breenix.sh bios

# Run tests
./scripts/run_test.sh
```

### Direct Cargo Commands

You can also run directly with cargo, but logs will only go to console:

```bash
# Run UEFI mode
cargo run --release --bin qemu-uefi -- -serial stdio -display none

# Run BIOS mode  
cargo run --release --bin qemu-bios -- -serial stdio -display none

# Run with testing features
cargo run --release --features testing --bin qemu-uefi -- -serial stdio
```

### Log Files

All log files are automatically saved to `logs/` with timestamps:
- Format: `breenix_YYYYMMDD_HHMMSS.log`
- Example: `logs/breenix_20250105_143022.log`

To analyze logs after a run:

```bash
# View latest log
ls -t logs/*.log | head -1 | xargs less

# Search in latest log
ls -t logs/*.log | head -1 | xargs grep "DOUBLE FAULT"

# Tail latest log
ls -t logs/*.log | head -1 | xargs tail -f
```

### Development Workflow

1. **Make code changes**
2. **Run with automated testing**: `./scripts/breenix_runner.py` (runs in background)
3. **Monitor execution**: Check timestamped log files in `logs/` directory
4. **Analyze results**: Use grep to search logs for specific events
5. **Debug issues**: Compare log patterns between working and broken runs

### Testing and Log Analysis Best Practices

**Running Breenix for Testing:**
```bash
# Automated testing (preferred method)
./scripts/breenix_runner.py > /dev/null 2>&1 &
sleep 15  # Wait for kernel to boot and run tests

# Check latest log
ls -t logs/*.log | head -1

# Analyze specific functionality
grep -E "Fork succeeded|exec succeeded|DOUBLE FAULT" logs/breenix_YYYYMMDD_HHMMSS.log
```

**Log Analysis Patterns:**
- Look for timestamped kernel messages: `NNNNNNNNNN - [LEVEL] module::function: message`
- Successful operations: Look for `✓` or "succeeded" messages
- Failures: Look for `✗`, "failed", "ERROR", or "DOUBLE FAULT"
- Process execution: Check for userspace context switches and syscalls
- Memory issues: Check for page fault details and memory mapping logs
- **CRITICAL BASELINE**: Look for "Hello from userspace!" output from direct test
- **Page table issues**: Look for "get_next_page_table" and "page table switch" messages

**Efficient Log Searching:**
To avoid approval prompts when searching logs with different parameters, use the `find-in-logs` script:

```bash
# 1. Write search parameters to /tmp/log-query.txt
echo '-A50 "Creating user process"' > /tmp/log-query.txt

# 2. Run the search script
./scripts/find-in-logs

# Examples:
echo '-A10 "scheduler::spawn"' > /tmp/log-query.txt
./scripts/find-in-logs

echo '-B5 -A20 "DOUBLE FAULT"' > /tmp/log-query.txt
./scripts/find-in-logs

echo '-E "Fork succeeded|exec succeeded"' > /tmp/log-query.txt
./scripts/find-in-logs
```

This approach allows Claude Code to search logs without triggering approval prompts for different search strings.

**Modified breenix_runner.py:**
- Changed from `-serial pty` to `-serial stdio` for proper log capture
- Captures all kernel output to timestamped log files
- Runs automatic tests during kernel initialization
- No longer requires interactive PTY communication

### Testing Commands

**Automatic Tests (run during kernel boot):**
- **CRITICAL**: Direct hello world test runs first to validate baseline syscall functionality
- Fork/exec pattern test runs after direct test
- Check logs for "BASELINE TEST: Direct userspace execution" and "REGRESSION TEST: Fork/exec pattern"
- **IMPORTANT**: Direct test MUST work before attempting fork/exec debugging

**Interactive Commands (if using interactive mode):**
- `exectest` - Test exec() system call
- `Ctrl+U` - Run single userspace test
- `Ctrl+P` - Test multiple concurrent processes
- `Ctrl+F` - Test fork() system call
- `Ctrl+E` - Test exec() system call
- `Ctrl+X` - Test fork+exec pattern
- `Ctrl+H` - Test shell-style fork+exec
- `Ctrl+T` - Show time debug info
- `Ctrl+M` - Show memory debug info

### Cleanup

```bash
# Kill any stuck QEMU processes
pkill -f qemu-system-x86_64

# Clean old logs (keeps last 10)
ls -t logs/*.log | tail -n +11 | xargs rm -f
```

## Coding Practices

### Rust-Specific Conventions
- **No Standard Library**: The kernel uses `#![no_std]` and `#![no_main]`
- **Nightly Rust**: Requires nightly toolchain with specific components
- **Custom Target**: Uses `x86_64-breenix.json` for bare metal compilation
- **Panic Handling**: Custom panic handler that enters infinite loop

### Code Style
- Clear module organization with descriptive names
- Use of `bootloader_api` for boot information access
- Embedded graphics abstractions for display handling
- Const-correctness for hardware constants
- Explicit error handling where applicable

### Build Configuration
- **Toolchain**: Nightly Rust with `rust-src` and `llvm-tools-preview`
- **Target**: Custom x86_64 target without OS
- **Features**: Disabled hardware features (`-mmx,-sse,+soft-float`)
- **Panic Strategy**: Abort on panic
- **Red Zone**: Disabled for interrupt safety

### Testing
- Integration tests for basic functionality:
  - Boot testing
  - Heap allocation
  - Stack overflow handling
  - Panic testing

#### Test Development Best Practices
When implementing new features, the build/test loop is KEY to our development process:

1. **Create Test Cases Early**:
   - Write integration tests in `tests/` directory
   - Add runtime tests with the `testing` feature flag
   - Create shell scripts for comprehensive testing when appropriate
   - Test both positive cases AND error conditions

2. **Test Infrastructure Overview**:
   - **Shared QEMU Tests**: Most tests use `tests/shared_qemu.rs` for efficient testing
   - **Special Tests**: Some tests require specific configurations and are marked `#[ignore]`
   - **POST Completion**: Tests wait for kernel completion marker `🎯 KERNEL_POST_TESTS_COMPLETE 🎯`

3. **Test Categories**:

   **Standard Tests (use shared QEMU):**
   ```bash
   cargo test  # Runs all standard tests efficiently (~45 seconds)
   ```
   - `boot_post_test.rs` - Comprehensive POST validation (14 subsystems)
   - `interrupt_tests.rs` - Interrupt system validation (4 tests)
   - `memory_tests.rs` - Memory management tests (3 tests)
   - `logging_tests.rs` - Logging system tests (3 tests)
   - `timer_tests.rs` - Timer and RTC tests (4 tests)
   - `simple_kernel_test.rs` - Basic execution test
   - `kernel_build_test.rs` - Build validation (3 tests)
   - `system_tests.rs` - Boot sequence and stability (2 tests)

   **Special Tests (require specific handling):**
   ```bash
   # BIOS boot test (requires BIOS mode)
   cargo test test_bios_boot -- --ignored

   # Runtime testing feature (requires --features testing)
   cargo test test_runtime_testing_feature -- --ignored
   cargo run --features testing --bin qemu-uefi -- -serial stdio
   ```

4. **Build/Test Loop**:
   ```bash
   # Standard development workflow (FAST)
   cargo test  # Runs 21 tests with single QEMU boot

   # Manual kernel testing
   cargo run --bin qemu-uefi -- -serial stdio -display none

   # Test with runtime features
   cargo run --features testing --bin qemu-uefi -- -serial stdio

   # Visual testing (shows QEMU display window)
   BREENIX_VISUAL_TEST=1 cargo test         # Run all tests with visual output
   BREENIX_VISUAL_TEST=1 cargo test memory  # Run specific test with visual output

   # Manual testing
   ./scripts/test_kernel.sh       # Interactive manual test
   ```

5. **Performance**: Standard tests run ~3x faster due to shared QEMU instance

6. **Visual Testing**: Set `BREENIX_VISUAL_TEST=1` environment variable to see QEMU display

7. **Legacy Scripts**: Removed old redundant test scripts, kept only:
   - `scripts/test_kernel.sh` - Interactive manual testing

### Development Workflow

## 🚨 MANDATORY PRE-COMMIT TESTING & CLEAN BUILD 🚨

**NEVER commit without BOTH clean builds AND passing tests!**

### Before EVERY Commit:

1. **Ensure ZERO compiler warnings**:
   ```bash
   cargo build 2>&1 | grep -E "(warning|error)" || echo "BUILD CLEAN!"
   ```
   - If ANY warnings appear: DO NOT COMMIT - fix them first
   - We maintain a zero-warning policy for code quality

2. **Run the complete test suite**:
   ```bash
   cargo test
   ```
   
3. **Verify ALL tests pass**:
   - `test_divide_by_zero` - Exception handling works
   - `test_invalid_opcode` - Exception handling works
   - `test_page_fault` - Exception handling works
   - `test_multiple_processes` - 5 processes run concurrently
   
4. **Check test output** - Don't just look for green checkmarks:
   - For `test_multiple_processes`: Verify you see 5 "Hello from userspace!" messages
   - For exception tests: Verify you see the TEST_MARKER output
   
5. **If ANY warnings OR test failures**: DO NOT COMMIT - fix the issues first

### Standard Development Workflow:
1. Kernel code changes are made in `kernel/src/`
2. **Run `cargo test` after EVERY change**
3. Build system automatically creates disk images
4. Tests can be run using QEMU for both UEFI and BIOS modes
5. Legacy code serves as reference for features being reimplemented
6. **When adding new features**: ADD A TEST to the test harness
7. **CRITICAL: Always ensure clean builds before declaring victory** - this is AS IMPORTANT as implementing tests:
   - Fix ALL compiler warnings (unused imports, dead code, unsafe blocks, etc.)
   - Fix ALL clippy warnings when available
   - The code MUST compile with `cargo build` without ANY warnings
   - Never commit code with warnings - treat warnings as errors
   - Add `#[allow(dead_code)]` only for legitimate API functions that will be used later
   - Use proper patterns (e.g., `Once` for static initialization) to avoid unsafe code warnings

### Pull Request Workflow

**CRITICAL: NEVER push directly to main branch!**

**IMPORTANT: Before starting any work, ALWAYS ensure you're on a clean branch off main:**
```bash
git checkout main
git pull origin main
git checkout -b feature-name
```
This prevents confusion about why you can't push upstream when you've accidentally added commits to main.

Once Ryan is happy with an implementation:

1. **Always work on a feature branch**:
   ```bash
   git checkout -b feature-name
   ```

2. **Push to the feature branch**:
   ```bash
   git push -u origin feature-name
   ```

3. **Create PR using GitHub CLI**:
   ```bash
   gh pr create --title "Brief description" --body "Detailed description with testing results"
   ```

4. **After creating the PR**:
   - The command will output a URL like `https://github.com/ryanbreen/breenix/pull/XX`
   - **ALWAYS open this URL** to verify the PR was created correctly
   - Share the URL with Ryan for review

5. **PR Description Should Include**:
   - Summary of changes
   - Implementation details
   - Testing performed and results
   - Any improvements over legacy implementation
   - Co-authorship credit

## Building and Running

### Prerequisites
- QEMU installed (`brew install qemu` on macOS)
- Rust nightly toolchain with required components (see rust-toolchain.toml)
- x86_64 target support

### Build Commands
On all systems:
```bash
# Build kernel with custom target (kernel uses x86_64-breenix.json)
cargo build

# Run Breenix with logging
./scripts/run_breenix.sh

# Or use direct cargo commands (logs to console only)
cargo run --release --bin qemu-uefi -- -serial stdio -display none
cargo run --release --bin qemu-bios -- -serial stdio -display none

# Run tests (these use controlled QEMU instances)
cargo test --test simple_kernel_test
```

## Important Notes
- The project is transitioning from a legacy codebase to a new implementation
- Current focus appears to be on establishing basic graphics and boot capabilities
- Network and advanced I/O drivers exist in legacy code but aren't yet ported
- The kernel currently implements a simple blue square rendering demo
- The kernel is built with the custom x86_64-breenix.json target
- QEMU runners and build system run on the host platform
- Tests properly separate host and target concerns

## 🚨 CRITICAL DEBUGGING REQUIREMENT 🚨

**NEVER declare success without definitive proof from kernel logs**

When implementing or debugging features:
1. **Require explicit log evidence**: Must show exact log lines proving functionality works
2. **No assumptions**: "Should work" or "likely works" is NOT acceptable  
3. **Trace execution**: For userspace execution, need logs showing:
   - Instructions actually executing in userspace (not just preparing to)
   - Successful transitions between kernel/user mode
   - System calls completing successfully
4. **Double fault ≠ success**: A double fault at userspace address is NOT proof of execution
5. **Be skeptical**: If you don't see explicit logs of success, it didn't happen

**Example of what constitutes proof:**
```
[INFO] Userspace instruction executed at 0x10000000
[INFO] Syscall 0x80 received from userspace  
[INFO] Returning to userspace at 0x10000005
```

**Example of what is NOT proof:**
```
[INFO] Scheduled page table switch for process 1
[DEBUG] TSS RSP0 updated
DOUBLE FAULT at 0x10000005  <-- This is a CRASH, not execution!
```

**Current state**: Exec() appears to complete but double faults immediately. No evidence of actual userspace execution in logs.

## Development Notes
All commits should be signed as co-developed by Ryan Breen and Claude Code because we're best buds!

## Project Roadmap and Documentation

### Master Roadmap
**IMPORTANT**: The project roadmap is maintained at `docs/planning/PROJECT_ROADMAP.md` and tracks:
- Current Development Status (what's done, in progress, and next)
- All completed phases (✅)
- Current development focus (🚧)
- Future planned work (📋)
- Technical decisions and architecture
- Success metrics for each phase

### Maintaining Current Development Status
The top section of PROJECT_ROADMAP.md must be updated:
- **After each PR merge**: Update "Recently Completed" with what was done
- **When starting new work**: Update "Currently Working On"
- **Weekly**: Review and update "Immediate Next Steps"
- **Format**: Use checkmarks (✅) for completed, construction (🚧) for in-progress

Example update after completing fork():
```markdown
### Recently Completed (Last Sprint)
- ✅ Implemented fork() system call with copy-on-write
- ✅ Fixed keyboard responsiveness after userspace process exit
```

### Documentation Structure
All documentation lives in `docs/planning/`:
- **Numbered directories (00-15)**: One per roadmap phase
- **Cross-cutting directories**: `posix-compliance/`, `legacy-migration/`
- **Key documents**:
  - `docs/planning/PROJECT_ROADMAP.md` - Master roadmap with current status
  - `docs/planning/legacy-migration/FEATURE_COMPARISON.md` - Legacy vs new
  - `docs/planning/06-userspace-execution/USERSPACE_SUMMARY.md` - Userspace status
  - `docs/planning/posix-compliance/POSIX_COMPLIANCE.md` - POSIX strategy

### Adding New Documentation
When creating new docs:
1. Place in the appropriate phase directory (00-15)
2. Use clear, descriptive filenames
3. Include context and date in the document
4. Update PROJECT_ROADMAP.md if it affects current work

### Legacy Code Removal Policy
As we complete feature migrations from `src.legacy/` to the new kernel:

1. **When to Remove Legacy Code**:
   - Once a feature reaches full parity or better in the new kernel
   - After verifying all functionality works correctly
   - When the feature comparison shows ✅ for both legacy and new

2. **Process**:
   - Identify the specific legacy modules/files that are now redundant
   - Remove the code from `src.legacy/`
   - Update FEATURE_COMPARISON.md to reflect the removal
   - Include legacy code removal in the same commit as the feature completion

3. **Benefits**:
   - Reduces codebase size and complexity
   - Prevents confusion about which implementation to reference
   - Makes it clear what still needs to be migrated
   - Keeps the project focused on the new implementation

Example: When timestamp logging reaches parity, remove the legacy print macros and timer code that are no longer needed as reference.