# Kthread Requirements for Deferred Framebuffer Rendering

## Problem Statement

The interactive shell crashes with a stack overflow when rendering text to the framebuffer. The rendering call stack is extremely deep:

```
keyboard IRQ handler
  → TTY driver echo
    → terminal_manager::write_char_to_shell
      → terminal_pane::write_char
        → draw_char
          → draw_glyph
            → font bitmap rendering
              → pixel-by-pixel framebuffer writes
```

This call stack consumes approximately **500KB+ of stack space**. The main kernel stack is 512KB, so any additional context (IRQ handler frames, etc.) causes overflow into the guard page.

## Solution: Deferred Rendering via Kernel Thread

The solution is to decouple the "queue text for rendering" operation (cheap, ~10 instructions) from the actual rendering (expensive, deep stack). A dedicated kernel thread with its own large stack handles the rendering.

```
Producer (any context)              Consumer (render kthread)
──────────────────────              ─────────────────────────
       │                                     │
  queue_byte()  ────► Ring Buffer ────►  drain_and_render()
  (O(1), no stack)    (16KB static)      (500KB stack OK)
```

## Implementation Status

The deferred rendering infrastructure is **complete** in `breenix-graphics`:

| File | Status | Purpose |
|------|--------|---------|
| `kernel/src/graphics/render_queue.rs` | ✅ Done | Lock-free 16KB ring buffer |
| `kernel/src/graphics/render_task.rs` | ✅ Done | Kthread spawn + render loop |
| `kernel/src/tty/driver.rs` | ✅ Done | Calls `queue_byte()` instead of direct render |
| `kernel/src/logger.rs` | ✅ Done | Queues log output when ready |
| `kernel/src/main.rs` | ✅ Done | Calls `spawn_render_thread()` on boot |

**What's missing:** The kthread doesn't actually run because kernel thread scheduling isn't working yet.

## Required Kthread API

The render task needs these capabilities from the kthread subsystem:

### 1. Thread Creation with Custom Stack Size

```rust
// Current API in task/spawn.rs - DEFAULT_STACK_SIZE is 64KB
pub fn spawn_thread(name: &str, entry_point: fn()) -> Result<u64, &'static str>

// What render_task.rs does:
const RENDER_STACK_SIZE: usize = 1024 * 1024; // 1MB

let stack = crate::memory::stack::allocate_stack_with_privilege(
    RENDER_STACK_SIZE,
    ThreadPrivilege::Kernel,
)?;

let thread = Box::new(Thread::new(
    String::from("render"),
    render_thread_main,      // fn() entry point
    stack.top(),
    stack.bottom(),
    tls_block,
    ThreadPrivilege::Kernel,
));

crate::task::scheduler::spawn(thread);
```

**Requirement:** The thread MUST actually run on the allocated 1MB stack, not the main kernel stack.

### 2. Thread Scheduling

The render thread uses a simple polling loop:

```rust
fn render_thread_main() {
    loop {
        if render_queue::has_pending_data() {
            let rendered = render_queue::drain_and_render();
            if rendered > 0 {
                flush_framebuffer();
            }
        }

        // Yield to let other threads run
        crate::task::scheduler::yield_current();
    }
}
```

**Requirements:**
- `scheduler::spawn(thread)` must add the thread to the run queue
- `scheduler::yield_current()` must context-switch away and eventually return
- The scheduler must periodically schedule the render thread (even low priority is fine)

### 3. Stack Isolation (CRITICAL)

This is the whole point. When `render_thread_main()` calls `drain_and_render()`, which calls `write_char_to_shell()`, which triggers the 500KB deep rendering stack... **that stack usage must happen on the render thread's 1MB stack, NOT on the main kernel stack or any other stack.**

**Verification:** If the render thread is working correctly, typing in the interactive shell should NOT cause a page fault at `0xffffc90000102ff8` (the main kernel stack guard page).

## Testing the Integration

Once kthreads are working:

1. Build with interactive feature:
   ```bash
   cargo build --release --features interactive --bin qemu-uefi
   ```

2. Run interactive mode:
   ```bash
   cargo run -p xtask -- interactive
   ```

3. Type `help` in the shell

**Expected:** No crash, text appears on screen
**Current:** Page fault at kernel stack guard page

## Debug Markers

The render thread logs these markers when it starts:

```
Render thread spawned with ID X (1024KB stack)
Render thread started on dedicated 1MB stack
```

If you don't see these in the serial output, the thread isn't being scheduled.

## Questions for Kthread Implementation

1. **Does `Thread::new()` correctly set up the initial stack frame?** The `entry_point: fn()` needs to start executing with RSP pointing to the allocated stack.

2. **Does context switch restore RSP from the thread's saved context?** When switching TO the render thread, RSP must be set to that thread's stack pointer, not left pointing to the previous thread's stack.

3. **Is the thread actually being added to the scheduler's run queue?** Check if `scheduler::spawn()` is being called and the thread is in a `Ready` state.

4. **Is the scheduler ever picking the render thread?** Even if it's in the queue, it needs to actually get scheduled.

## Files to Reference

In `breenix-graphics` (this branch):
- `kernel/src/graphics/render_queue.rs` - The ring buffer implementation
- `kernel/src/graphics/render_task.rs` - The thread spawn code
- `kernel/src/task/spawn.rs` - Current thread spawning API
- `kernel/src/task/thread.rs` - Thread structure and creation
- `kernel/src/task/scheduler.rs` - Scheduler implementation

## Summary

**TL;DR:** We need a kernel thread that:
1. Gets created with a 1MB stack
2. Actually gets scheduled to run
3. Runs on its own stack (not the main kernel stack)
4. Can yield and get rescheduled

The render_task code is ready and waiting. Once kthreads work, the interactive shell will stop crashing.
