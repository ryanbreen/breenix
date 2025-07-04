# Fundamental Process Management Redesign

Date: 2025-01-03
Context: After implementing a spawn mechanism and encountering persistent deadlocks, we've identified fundamental architectural issues that require a redesign.

## Current Problems

### 1. Doing Too Much in Interrupt Context
- Timer interrupt handler is trying to perform complex operations:
  - Acquiring multiple locks (scheduler, process manager)
  - Making scheduling decisions
  - Setting up thread contexts
  - Modifying interrupt frames
- This violates the principle that interrupt handlers should be minimal

### 2. Conflating Fork/Exec Model
- Created a "spawn" mechanism that tries to create AND exec in one operation
- Using kernel threads that "become" user processes (backwards approach)
- Not following the traditional Unix fork/exec separation of concerns

### 3. Improper Context Switching
- Trying to perform context switches INSIDE the timer interrupt handler
- Should instead set a flag and switch on interrupt return
- Mixing scheduling decisions with context switch implementation

### 4. Lock Ordering and Deadlocks
- Timer interrupt holds scheduler lock, then tries to acquire process manager lock
- Functions calling each other while holding locks
- Modifying critical data structures from within lock-holding closures

## Correct Architecture

### 1. Minimal Timer Interrupt
```rust
// Timer interrupt should ONLY:
- Increment tick count
- Update current thread's time slice
- Set need_resched flag if quantum expired
- Send EOI and return
```

### 2. Context Switch on Interrupt Return
```rust
// Assembly interrupt return path should:
- Check need_resched flag
- If set, call schedule() to pick next thread
- Perform actual context switch
- Return to selected thread (kernel or user)
```

### 3. Proper Fork Implementation
```rust
// Fork should:
- Create new process with copy of parent's address space
- Create new thread for the child process
- Add child thread to scheduler
- Return child PID to parent
- Return 0 to child (when it runs)
```

### 4. Proper Exec Implementation
```rust
// Exec should:
- Replace current process's address space
- Load new program image
- Set up new stack
- Jump to entry point
- Never return (on success)
```

### 5. User Threads from the Start
- Processes should have user-mode threads from creation
- These threads enter kernel mode via syscalls/interrupts
- No "kernel threads becoming user threads"

## Implementation Plan

### Phase 1: Simplify Timer Interrupt
1. Remove all scheduling logic from timer interrupt handler
2. Just update tick count and set need_resched flag
3. Add resched check to interrupt return path

### Phase 2: Fix Context Switching
1. Move context switch logic out of interrupt handler
2. Implement proper switch_to() function
3. Handle kernel->kernel, kernel->user, user->kernel, user->user

### Phase 3: Implement Proper Fork
1. Create process_fork() that duplicates address space
2. Set up child process with parent's context
3. Return different values to parent/child

### Phase 4: Implement Proper Exec
1. Create process_exec() that replaces address space
2. Parse ELF and set up new memory layout
3. Transition to new program

### Phase 5: Remove Spawn Mechanism
1. Delete the spawn thread approach
2. Use fork() + exec() for process creation
3. Update init process creation to use new model

## Key Principles

1. **Interrupt handlers do minimal work**
2. **Scheduling != Context switching**
3. **Fork creates, exec replaces**
4. **User processes have user threads**
5. **Lock ordering must be consistent**
6. **Never hold locks across complex operations**

## Benefits of Redesign

1. **No more deadlocks** from complex interrupt handlers
2. **Cleaner separation** of concerns
3. **Standard Unix model** that developers expect
4. **Easier to debug** with simpler code paths
5. **Better performance** with minimal interrupt latency

## Next Session Plan

1. Start by implementing minimal timer interrupt
2. Add need_resched flag and interrupt return check
3. Build proper fork() from scratch
4. Test with simple fork test program
5. Then implement exec()
6. Finally remove the broken spawn mechanism

This redesign aligns with established OS design principles and will resolve our fundamental architectural issues.