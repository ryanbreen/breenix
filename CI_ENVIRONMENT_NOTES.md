# CI Environment Notes

## Key Differences Between Local and GitHub Actions

### 1. Operating System
- **Local**: macOS (Darwin 24.5.0)
- **CI**: Ubuntu Latest (Linux)

### 2. QEMU Installation
- **Local**: Installed via Homebrew (`brew install qemu`)
- **CI**: Installed via apt (`sudo apt-get install qemu-system-x86`)

### 3. Display Configuration
- **Local**: Can run with or without display
- **CI**: Must use `-display none` (no X11/display server)

### 4. Timeout Handling
- Both use 30-second timeout to prevent hangs
- Exit code 124 indicates timeout (normal for our test)

### 5. Log Locations
- **Local**: `logs/breenix_YYYYMMDD_HHMMSS.log`
- **CI**: Same, plus `test_output.log` in workflow root

### 6. Success Criteria
- Both check for: `"USERSPACE OUTPUT: Hello from userspace"`
- This string indicates successful userspace execution

## GitHub Actions Workflows Created

1. **test-sanity-check.yml**
   - Triggers on push to `sanity-check-happy-ring-3`
   - Runs single test and checks for success
   - Uploads logs on failure

2. **manual-test.yml**
   - Manually triggered via workflow_dispatch
   - Runs 3 tests to check consistency
   - Shows system info for debugging
   - Can test any branch

3. **ci-test.sh**
   - Helper script that runs test exactly as locally
   - Returns exit 0 on success, 1 on failure

## Important Notes

- The `--features testing` flag is included in `run_breenix.sh`
- Serial output goes to stdio (captured by tee)
- QEMU runs headless with `-display none`
- Rust toolchain must be nightly with rust-src and llvm-tools-preview

## Next Steps

1. Push to GitHub and verify workflows run
2. Monitor for any CI-specific failures
3. If failures occur, check:
   - QEMU version differences
   - CPU virtualization features
   - Memory allocation differences
   - Timing differences in CI environment