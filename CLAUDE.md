# Breenix OS

## Project Overview

Breenix is an x86_64 operating system kernel written in Rust. **This is NOT a toy or learning project - we are building a production-quality operating system for the long haul.**

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

## MCP Server Usage (REQUIRED - ALWAYS USE MCP)

**CRITICAL: Always use MCP for ALL Breenix development and testing. NEVER run QEMU or kernel tests directly without MCP.**

**ABSOLUTE REQUIREMENT: YOU MUST NEVER RUN QEMU DIRECTLY**
- NEVER use `cargo run --bin qemu-uefi` or `cargo run --bin qemu-bios` directly
- NEVER manually start QEMU processes
- ALWAYS use the MCP tools (`mcp__breenix__start`, etc.) for ALL kernel execution
- This is a HARD REQUIREMENT - no exceptions!

**CRITICAL: ALWAYS STOP BEFORE START**
- **YOU MUST ALWAYS call `mcp__breenix__stop` before `mcp__breenix__start`**
- This prevents "Breenix is already running" errors
- Even if you think Breenix isn't running, ALWAYS stop first
- The correct sequence is ALWAYS:
  1. `mcp__breenix__stop`
  2. `mcp__breenix__start`
- No exceptions to this rule!

Breenix includes a comprehensive MCP (Model Context Protocol) server that enables programmatic interaction with the kernel for development and testing. This is REQUIRED for Claude Code integration and provides essential visibility for debugging.

**Why MCP is mandatory:**
- Provides real-time visibility into kernel behavior through tmux panes
- Enables proper debugging with Ryan's assistance
- Maintains consistent testing environment
- Prevents stuck QEMU processes
- Tracks all kernel interactions and logs

### Quick Start with tmuxinator

The recommended way to work with Breenix is using tmuxinator, which provides a complete development environment:

```bash
# Start the MCP development environment
tmuxinator start breenix-mcp
```

This creates a horizontal split terminal with:
- **Top pane**: MCP HTTP server running on port 8080
- **Bottom pane**: Live kernel logs streaming from `/tmp/breenix-mcp/kernel.log`

### MCP Tools Available

The server provides these tools for interacting with Breenix:

- **Process Management**: `mcp__breenix__start`, `mcp__breenix__stop`, `mcp__breenix__running`, `mcp__breenix__kill`
- **Communication**: `mcp__breenix__send`, `mcp__breenix__wait_prompt`, `mcp__breenix__run_command`
- **Logging**: `mcp__breenix__logs`

### Manual Usage

If you prefer manual control:

```bash
# Start MCP server manually
cd mcp && BREENIX_MCP_PORT=8080 cargo run --bin breenix-http-server

# Test the HTTP API
curl http://localhost:8080/health
curl -X POST http://localhost:8080/start -d '{"display": false}' -H "Content-Type: application/json"
```

### Development Workflow with MCP

1. **Start Environment**: `tmuxinator start breenix-mcp`
2. **Use Claude Code**: Claude automatically discovers and uses MCP tools
3. **Monitor Logs**: Watch the bottom pane for real-time kernel output
4. **Restart if needed**: `./scripts/restart_mcp.sh` or `tmuxinator restart breenix-mcp`

### Key Benefits

- **Automated Testing**: Run kernel tests programmatically via Claude Code
- **Real-time Monitoring**: Live log streaming in dedicated terminal pane
- **Command Injection**: Send commands to running kernel via serial interface
- **Session Management**: Controlled QEMU process lifecycle
- **HTTP API**: RESTful endpoints for external tool integration

See `docs/MCP_INTEGRATION.md` for complete documentation.

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
1. Kernel code changes are made in `kernel/src/`
2. Build system automatically creates disk images
3. Tests can be run using QEMU for both UEFI and BIOS modes
4. Legacy code serves as reference for features being reimplemented
5. **CRITICAL: Always ensure clean builds before declaring victory** - this is AS IMPORTANT as implementing tests:
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

# NEVER RUN QEMU DIRECTLY - USE MCP INSTEAD:
# DO NOT USE: cargo run --bin qemu-uefi
# DO NOT USE: cargo run --bin qemu-bios
# INSTEAD USE MCP TOOLS:
# - mcp__breenix__start
# - mcp__breenix__stop
# - mcp__breenix__send
# etc.

# Run tests (these are OK as they use controlled QEMU instances)
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