---
name: systematic-debugging
description: This skill should be used when debugging complex kernel issues requiring systematic investigation and documentation. Use for documenting problem analysis, root cause investigation, solution implementation, and evidence collection following Breenix's Problem→Root Cause→Solution→Evidence pattern.
---

# Systematic Debugging for Breenix

Document-driven debugging workflow for kernel issues.

## Purpose

Complex kernel bugs require systematic investigation and documentation. This skill provides the pattern used in Breenix debugging docs like TIMER_INTERRUPT_INVESTIGATION.md, DIRECT_EXECUTION_FIX.md, and PAGE_TABLE_FIX.md.

## The Four-Phase Pattern

All debugging documents follow this structure:

1. **Problem**: What's broken? Observable symptoms
2. **Root Cause**: Why is it broken? Deep analysis
3. **Solution**: What fixes it? Implementation details
4. **Evidence**: How do you know it's fixed? Before/after proof

## When to Use

- **Complex kernel issues**: Not simple typos or obvious bugs
- **Architectural problems**: Issues requiring design changes
- **Recurring failures**: Problems that reappear or are hard to reproduce
- **Learning opportunities**: Bugs that teach important lessons
- **CI investigations**: Failed tests requiring deep analysis

## Debugging Workflow

### Phase 1: Problem Definition

**Document observable symptoms:**

```markdown
# Problem Summary

[Brief description of what's failing]

## Symptoms

- What fails? (test, boot, specific operation)
- When does it fail? (always, intermittently, specific conditions)
- Error messages or behavior observed
- What works vs what doesn't
```

**Example from DIRECT_EXECUTION_FIX.md:**
```markdown
# Problem Summary
Direct userspace execution was failing with a double fault at `int 0x80`
instruction (`0x10000019`).

## Symptoms
- Userspace processes boot successfully
- Calling int 0x80 triggers double fault
- Error occurs during Ring 3 → Ring 0 transition
```

### Phase 2: Root Cause Analysis

**Investigate systematically:**

1. **Reproduce consistently**
   ```bash
   # Use kernel-debug-loop for fast iteration
   kernel-debug-loop/scripts/quick_debug.py --signal "FAILURE_POINT" --timeout 10
   ```

2. **Add diagnostic logging**
   ```rust
   log::debug!("About to perform operation X");
   log::debug!("Variable state: {:?}", state);
   log::debug!("After operation X");
   ```

3. **Narrow down location**
   - Binary search: Add checkpoint in middle of suspect code
   - If reached: problem is after
   - If not reached: problem is before
   - Repeat until isolated

4. **Analyze state**
   - What values are variables?
   - What should they be?
   - What assumptions are violated?

**Document findings:**

```markdown
## Root Cause Analysis

1. **Sequence of Events**:
   - Step 1 happens
   - Step 2 happens
   - Step 3 fails because X

2. **Technical Details**:
   - Specific memory addresses, registers, flags
   - Code paths taken
   - Assumptions violated

3. **Why It Happens**:
   - Fundamental reason for the failure
   - What design assumption was wrong
```

### Phase 3: Solution Implementation

**Document the fix:**

```markdown
## Solution

### 1. [Component] Fix
**File**: `path/to/file.rs`
**Lines**: X-Y

[Explanation of what changed and why]

```rust
// Code snippet showing the fix
```

### 2. [Another Component] Fix
**File**: `path/to/another/file.rs`
**Lines**: X-Y

[Explanation]
```

**Example structure:**
- Identify all files that need changes
- For each change:
  - File path
  - Line numbers
  - Explanation of change
  - Code snippet
  - Rationale

### Phase 4: Evidence Collection

**Prove it works:**

```markdown
## Evidence

### Before Fix:
```
[Log output or error messages showing failure]
```

### After Fix:
```
[Log output showing success]
```

### Test Results:
- Test X: PASS
- Test Y: PASS
- Feature Z: Working as expected
```

## Integration with Tools

### With kernel-debug-loop

Fast iteration during investigation:

```bash
# Test hypothesis quickly
kernel-debug-loop/scripts/quick_debug.py \
  --signal "CHECKPOINT_AFTER_FIX" \
  --timeout 15
```

### With log-analysis

Extract evidence from logs:

```bash
# Find before/after comparison
echo '"Error pattern"' > /tmp/log-query.txt
./scripts/find-in-logs

echo '"Success pattern"' > /tmp/log-query.txt
./scripts/find-in-logs
```

### With ci-failure-analysis

Analyze CI test failures:

```bash
ci-failure-analysis/scripts/analyze_ci_failure.py \
  --context target/xtask_*_output.txt
```

## Debug Document Template

```markdown
# [Issue Name] Fix

Date: [YYYY-MM-DD]

## Problem Summary

[What's broken - one paragraph]

## Symptoms

- Symptom 1
- Symptom 2
- Error messages or behavior

## Root Cause Analysis

### Sequence of Events
1. [Step by step what happens]
2. [Leading to failure]

### Technical Details
- Memory addresses, registers, etc.
- Code paths taken
- State at time of failure

### Why It Happens
[Fundamental explanation]

## Solution

### 1. [First Change]
**File**: `path/to/file.rs`
**Lines**: X-Y

[Explanation]

```rust
// Code change
```

### 2. [Second Change]
**File**: `path/to/file2.rs`
**Lines**: X-Y

[Explanation]

## Evidence

### Before Fix:
```
[Error output]
```

### After Fix:
```
[Success output]
```

## Lessons Learned

1. [Key insight 1]
2. [Key insight 2]
3. [Patterns to apply in future]

## Related Issues

- [Link to similar past bugs]
- [Related design decisions]
```

## Example: Real Debugging Session

Based on TIMER_INTERRUPT_INVESTIGATION.md:

**Problem**: Kernel hanging after enabling interrupts

**Investigation**:
1. Compare with other OS implementations (blog_os, xv6, Linux)
2. Identify what they do (minimal timer handlers)
3. Identify what Breenix does (complex timer handler with locks)
4. Hypothesis: Timer handler too complex

**Solution**: Create simple_timer.rs with minimal handler

**Evidence**:
- Before: Kernel hangs immediately
- After: Kernel boots and reaches testing menu

**Lesson**: Interrupt handlers must be TRULY minimal

## Best Practices

1. **Document as you debug**: Don't wait until after
2. **Include evidence**: Logs, test results, screenshots
3. **Explain reasoning**: Why you investigated X, not Y
4. **Note dead ends**: What you tried that didn't work
5. **Extract lessons**: What to remember for next time
6. **Update related docs**: If this reveals design issues
7. **Create regression tests**: Prevent this bug from returning

## When to Create a Debug Document

Create a document when:
- Bug took >2 hours to solve
- Solution required design changes
- Bug could reoccur without understanding
- Lessons applicable to future development
- Multiple components involved
- Fix not immediately obvious from code change

## Summary

Systematic debugging follows:
1. Problem - Clear symptom description
2. Root Cause - Deep technical analysis
3. Solution - Implementation with rationale
4. Evidence - Before/after proof

This pattern ensures:
- Thorough understanding
- Proper fixes (not workarounds)
- Knowledge preservation
- Prevention of similar bugs
