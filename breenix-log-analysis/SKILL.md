---
name: log-analysis
description: This skill should be used when analyzing Breenix kernel logs for debugging, testing verification, or understanding kernel behavior. Use for searching timestamped logs, finding checkpoint signals, tracing execution flow, identifying errors or panics, and extracting diagnostic information.
---

# Kernel Log Analysis for Breenix

Search, analyze, and extract information from Breenix kernel logs for debugging and testing.

## Purpose

Breenix logs all kernel runs to `logs/breenix_YYYYMMDD_HHMMSS.log`. This skill provides patterns for searching these logs efficiently, finding checkpoint signals, tracing execution, and diagnosing issues.

## When to Use

- **Finding test signals**: Locate checkpoint markers like `🎯 KERNEL_POST_TESTS_COMPLETE 🎯`
- **Tracing execution**: Follow kernel boot sequence or specific subsystem initialization
- **Debugging failures**: Find panics, faults, or error messages
- **Verifying behavior**: Confirm expected operations occurred
- **Performance analysis**: Check timing of operations via log timestamps

## Log Location and Format

```bash
# Logs stored in
logs/breenix_YYYYMMDD_HHMMSS.log

# View latest log
ls -t logs/*.log | head -1 | xargs less

# View specific log
less logs/breenix_20250120_143022.log
```

### Log Format
```
[ INFO] kernel::memory: Physical memory: 94 MiB usable
[DEBUG] kernel::memory: Frame allocator initialized
[ WARN] kernel::process: No processes ready
```

Levels: `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`

## Search Using find-in-logs Script

The `scripts/find-in-logs` tool searches recent logs:

```bash
# Create search query (avoids approval prompts)
echo '-A50 "Creating user process"' > /tmp/log-query.txt
./scripts/find-in-logs

# The script reads from /tmp/log-query.txt and searches logs
```

### Common Search Patterns

```bash
# Find panics
echo '-i "panic"' > /tmp/log-query.txt
./scripts/find-in-logs

# Find page faults
echo '-i "page fault"' > /tmp/log-query.txt
./scripts/find-in-logs

# Find context around checkpoint
echo '-A20 -B10 "KERNEL_POST_TESTS_COMPLETE"' > /tmp/log-query.txt
./scripts/find-in-logs

# Find process creation
echo '"Creating user process"' > /tmp/log-query.txt
./scripts/find-in-logs
```

## Direct grep Usage

```bash
# Find specific error
grep -n "ERROR" logs/breenix_20250120_*.log

# Find with context
grep -A10 -B5 "Double Fault" logs/breenix_20250120_*.log

# Case-insensitive search
grep -i "memory" logs/breenix_20250120_*.log

# Multiple patterns
grep -E "panic|fault|error" logs/breenix_20250120_*.log

# Count occurrences
grep -c "Timer interrupt" logs/breenix_20250120_*.log
```

## Common Checkpoint Signals

```bash
# Test completion
grep "🎯 KERNEL_POST_TESTS_COMPLETE 🎯" logs/*.log

# Userspace execution
grep "USERSPACE OUTPUT:" logs/*.log
grep "Hello from userspace" logs/*.log

# System calls
grep "🎉 USERSPACE SYSCALL" logs/*.log

# Initialization checkpoints
grep "initialized\|INITIALIZED" logs/*.log

# Process creation
grep "Process created: PID" logs/*.log
```

## Execution Flow Tracing

### Boot Sequence
```bash
# Full boot trace
grep -E "Boot|GDT|IDT|PIC|Memory|Heap|Timer|Keyboard" logs/latest.log

# Memory subsystem only
grep "memory\|page table\|frame allocator" logs/latest.log

# Process subsystem
grep "process\|fork\|exec\|PID" logs/latest.log
```

### Subsystem Analysis
```bash
# Timer subsystem
grep -n "timer\|RTC\|tick" logs/latest.log

# Interrupt handling
grep -n "interrupt\|IRQ\|IDT" logs/latest.log

# System calls
grep -n "syscall\|sys_\|INT 0x80" logs/latest.log
```

## Error and Fault Analysis

### Finding Faults
```bash
# Double faults
grep -A20 "DOUBLE FAULT" logs/*.log

# Page faults
grep -A10 "PAGE FAULT" logs/*.log

# General panics
grep -B10 -A20 "PANIC" logs/*.log
```

### Error Context
```bash
# Find errors with context
grep -A15 -B5 "ERROR" logs/latest.log

# Find warnings that might indicate problems
grep -A5 "WARN" logs/latest.log
```

## Log Analysis Patterns

### Timeline Analysis
```bash
# Extract just log levels and messages for overview
grep -E "\[(INFO|WARN|ERROR|DEBUG)\]" logs/latest.log | less

# Filter to specific subsystem
grep "\[.*\] kernel::process:" logs/latest.log
```

### Success/Failure Detection
```bash
# Check if test completed
if grep -q "KERNEL_POST_TESTS_COMPLETE" logs/latest.log; then
  echo "Test completed"
else
  echo "Test did not complete"
  # Find last successful checkpoint
  grep "SUCCESS\|initialized\|completed" logs/latest.log | tail -10
fi
```

### Performance Markers
```bash
# Find timing information
grep -E "took|elapsed|ms|seconds" logs/latest.log

# Specific operations
grep "Context switch\|schedule\|preempt" logs/latest.log
```

## Integration with Other Skills

### With kernel-debug-loop
```bash
# Run quick test
kernel-debug-loop/scripts/quick_debug.py --signal "TARGET_CHECKPOINT"

# Then analyze its output
grep "TARGET_CHECKPOINT" logs/latest.log
```

### With ci-failure-analysis
```bash
# Analyze CI logs
ci-failure-analysis/scripts/analyze_ci_failure.py target/xtask_*_output.txt

# Then search for specific patterns found
grep "PATTERN" target/xtask_*_output.txt
```

## Best Practices

1. **Use specific patterns**: Narrow searches to relevant subsystems
2. **Add context**: Use `-A` (after) and `-B` (before) flags
3. **Check latest first**: `ls -t logs/*.log | head -1`
4. **Save search queries**: Use `/tmp/log-query.txt` for complex patterns
5. **Look for first error**: Often followed by cascading failures
6. **Check initialization**: Ensure subsystems initialized before use
7. **Verify checkpoints**: Confirm expected signals appear

## Summary

Effective log analysis requires:
- Knowing checkpoint signals
- Using grep with context flags
- Understanding log levels and formats
- Tracing execution flow
- Finding first failures
- Verifying expected behavior

Logs are the primary window into kernel behavior - use them liberally during development and debugging.
