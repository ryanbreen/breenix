# Userspace Regression Root Cause and Fix

## Summary

The userspace regression was caused by changing the logger level from `Trace` to `Debug`. This simple change broke userspace execution entirely.

## Root Cause

In commit(s) after the `happy-ring-3` branch, the logger initialization was changed:

```rust
// BROKEN - causes userspace to fail
log::set_max_level(LevelFilter::Debug);

// WORKING - allows userspace to execute
log::set_max_level(LevelFilter::Trace);
```

Location: `/Users/wrb/fun/code/breenix/kernel/src/logger.rs:202`

## Investigation Process

1. **Initial Investigation**: Tested commit 1448ac7 and found userspace wasn't working
2. **Broader Search**: Tested multiple commits going back in history - none had working userspace
3. **Branch Testing**: Checked out `happy-ring-3` branch and confirmed userspace DOES work there
4. **Diff Analysis**: Compared working branch with broken main branch
5. **Root Cause Found**: Logger level was changed from Trace to Debug

## Fix Applied

Changed logger level back to Trace in `kernel/src/logger.rs`:

```rust
pub fn init_early() {
    // Set up the logger immediately so all log calls work
    log::set_logger(&COMBINED_LOGGER)
        .expect("Logger already set");
    log::set_max_level(LevelFilter::Trace);  // Changed from Debug
}
```

## Verification

After applying the fix:
- `cargo run -p xtask -- ring3-smoke` passes successfully
- Exit code: 0
- Test output confirms: "✅ Ring‑3 smoke test passed - userspace execution detected"

## Why This Matters

The logger level affects what log statements are compiled into the kernel. When set to Debug, all TRACE-level log statements are completely removed at compile time. This suggests that somewhere in the kernel, there's a TRACE log statement that has a critical side effect needed for userspace execution.

## Lessons Learned

1. **Never change logger levels without testing**: Logger level changes can have unexpected side effects
2. **Side effects in log statements are dangerous**: The kernel apparently relies on some side effect in a TRACE log
3. **Test userspace after ANY change**: Even seemingly innocuous changes can break critical functionality

## Next Steps

1. **Find the problematic TRACE log**: Search for TRACE logs that might have side effects
2. **Remove the side effect**: Move any critical logic out of log statements
3. **Add regression test**: Ensure CI catches this specific regression in the future