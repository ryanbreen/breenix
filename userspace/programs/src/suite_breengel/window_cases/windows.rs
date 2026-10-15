//! windows: window lifecycle on every target (contract section 17).

use crate::*;

pub(crate) const WINDOWS: &[Case] = &[
    case("create", "create_window returns a window id and buffers of the requested size", windows_create),
    case("create-zeroed", "A new window's buffers are all zero", windows_create_zeroed),
    case("register-listed", "A registered window is listed with its title, size and owner", windows_register_listed),
    case("title-limit", "A 64-byte title is kept whole, and a longer one is refused with EINVAL", windows_title_limit),
    case("title-utf8", "A UTF-8 title is stored and listed byte for byte", windows_title_utf8),
    case("title-change", "Setting a new title updates the listing", windows_title_change),
    case("size-zero", "A zero width or height fails with EINVAL", windows_size_zero),
    case("size-too-big", "A dimension over 4,096 fails with EINVAL", windows_size_too_big),
    case("limit", "The seventeenth window fails with the contract's errno, and the first sixteen are unaffected", windows_limit),
    case("multi-window", "One process creates and draws four windows independently", windows_multi_window),
    case("destroy", "The destroy op removes the window from the listing and unmaps its buffers", windows_destroy),
    case("drop-destroys", "Dropping a Breengel Window destroys it", windows_drop_destroys),
    case("stale-id", "Every op on a destroyed window's id fails with ENOENT, and ids are never reused in a boot", windows_stale_id),
    case("register-unwinds", "When registration fails, Window::new frees the buffers it created", windows_register_unwinds),
    case("exit-cleanup", "A process's windows disappear when it exits", windows_exit_cleanup),
    case("exec-cleanup", "A process's windows disappear when it execs", windows_exec_cleanup),
    case("fork-no-mapping", "A forked child inherits no mapping of its parent's buffers", windows_fork_no_mapping),
    case("fork-no-authority", "A forked child cannot commit, resize or destroy its parent's window", windows_fork_no_authority),
    case("commit-swaps", "After a commit, acquire returns a different buffer holding the committed pixels", windows_commit_swaps),
    case("acquire-replays-damage", "With three buffers and alternating disjoint updates (A commits `[1,0]`, B commits `[1,2]` with only the second cell damaged), the next acquired buffer holds `[1,2]`, and every acquire over 300 such commits equals the newest front", windows_acquire_replays_damage),
    case("acquire-full-copy", "A buffer with no base, or whose missing commits have left the 8-entry damage log, is replayed in full by the kernel and equals the newest front, for a Breengel client and a raw client alike", windows_acquire_full_copy),
    case("stale-base-commit", "Of two concurrently acquired buffers, committing the one with the older base gives exactly that buffer's pixels, with damage widened to cover every pixel that changed. With `acquire-stall` holding thread 1's replay copy from front A halfway, thread 2 commits C and then acquires: it does not get A until the copy ends, and thread 1's buffer equals exactly the commit its returned base names. A stale buffer committed after more than eight intervening commits gets whole-window damage. Every step compares whole pixels to the formula and checks that the damage covers every changed pixel", windows_stale_base_commit),
    case("commit-immutable", "Writing a committed buffer faults", windows_commit_immutable),
    case("commit-busy", "Committing a buffer twice without acquiring it again fails with EBUSY", windows_commit_busy),
    case("thread-commit", "Two threads of the owner committing concurrently both succeed with distinct, ordered commit ids", windows_thread_commit),
    case("release-order", "A retired buffer returns to free only after its composite and any capture copy in progress are done", windows_release_order),
    case("acquire-timeout", "acquire with no free buffer returns ETIMEDOUT no earlier than its timeout and within 100 ms after it", windows_acquire_timeout),
    case("resize", "The owner's resize gives buffers of the new size holding the old contents in the intersection", windows_resize),
    case("resize-error", "A resize the kernel refuses is reported as an error, and no Resized event claims the new size", windows_resize_error),
    case("resize-rlimit", "A resize over RLIMIT_AS fails with ENOMEM and leaves the old buffers intact", windows_resize_rlimit),
    case("resize-rollback", "A failure at each allocation step of create and resize leaves registry, mappings and accounting unchanged", windows_resize_rollback),
    case("rlimit-accounting", "Window buffers count against RLIMIT_AS, and destroying the window returns them", windows_rlimit_accounting),
    case("capture-across-destroy", "A capture in progress when its window is destroyed returns the whole commit, with the tree page of that same commit when a tree destination is given, and the buffer is freed when the copy ends", windows_capture_across_destroy),
    case("two-buffer-capture-load", "A two-buffer client committing 300 frames while a reader captures it continuously and an evidence holder is registered never waits in acquire beyond one composite and one capture copy", windows_two_buffer_capture_load),
    case("remnant-on-death", "When the owner of a window with an evidence holder is killed, the holder reads its last front and tree, and releasing the remnant returns memory to the baseline", windows_remnant_on_death),
    case("churn", "100 create and destroy cycles return the registry, allocation counter and memory to the baseline", windows_churn),
    case("distinct-content", "Sixteen live windows each composite their own pixels, never another window's", windows_distinct_content),
    case("raw-client", "A client using the raw libbreenix calls, without Breengel, creates, commits and destroys a window", windows_raw_client),
    case("every-target", "Window creation works on this target's backend and reports the backend by name", windows_every_target),
];

// ---------------------------------------------------------------------------------------
// windows: window lifecycle on every target (contract section 17)
// ---------------------------------------------------------------------------------------

fn windows_create() -> CaseResult { needs_window("17.1, 17.2", "CREATE's id and buffers of the requested size") }
fn windows_create_zeroed() -> CaseResult { needs_window("17.2", "a new window's all-zero buffers") }
fn windows_register_listed() -> CaseResult { needs_window("17.1, 17.2", "REGISTER and LIST's title, size and owner") }
fn windows_title_limit() -> CaseResult { needs_window("17.2", "the 64-byte title limit (EINVAL past it)") }
fn windows_title_utf8() -> CaseResult { needs_window("17.2", "a UTF-8 title stored byte for byte") }
fn windows_title_change() -> CaseResult { needs_window("17.1", "SET_TITLE updating the listing") }
fn windows_size_zero() -> CaseResult { needs_window("17.2", "EINVAL for a zero width or height") }
fn windows_size_too_big() -> CaseResult { needs_window("17.2", "EINVAL for a dimension over 4,096") }
fn windows_limit() -> CaseResult { needs_window("17.2", "EAGAIN for a seventeenth window") }
fn windows_multi_window() -> CaseResult { needs_window("17.2", "four windows of one process drawn independently") }
fn windows_destroy() -> CaseResult { needs_window("17.2", "DESTROY removing the window and its mappings") }
fn windows_drop_destroys() -> CaseResult { needs_window("17.2", "Breengel's Window destroying its window on Drop") }
fn windows_stale_id() -> CaseResult { needs_window("17.2", "ENOENT on a destroyed id and ids never reused") }
fn windows_register_unwinds() -> CaseResult { needs_window("17.2", "Window::new freeing its window when registration fails") }
fn windows_exit_cleanup() -> CaseResult { needs_window("17.2", "a process's windows destroyed at exit") }
fn windows_exec_cleanup() -> CaseResult { needs_window("17.2", "a process's windows destroyed at exec") }
fn windows_fork_no_mapping() -> CaseResult { needs_window("17.2", "window mappings not inherited across fork") }
fn windows_fork_no_authority() -> CaseResult { needs_window("17.2, 17.4", "a forked child without authority over its parent's window") }
fn windows_commit_swaps() -> CaseResult { needs_window("17.3", "COMMIT and ACQUIRE returning another buffer holding the committed pixels") }
fn windows_acquire_replays_damage() -> CaseResult { needs_window("17.3", "the kernel's replay of missed damage inside ACQUIRE") }
fn windows_acquire_full_copy() -> CaseResult { needs_window("17.3", "the whole-window replay for a buffer with no base or a lost damage log") }
fn windows_stale_base_commit() -> CaseResult { needs_window("17.3, 21", "stale-base commits, widened damage and the acquire-stall control") }
fn windows_commit_immutable() -> CaseResult { needs_window("17.3", "SIGSEGV on a write to a committed buffer") }
fn windows_commit_busy() -> CaseResult { needs_window("17.3", "EBUSY for a second commit without an acquire") }
fn windows_thread_commit() -> CaseResult { needs_window("17.3", "concurrent commits by two threads, with ordered ids") }
fn windows_release_order() -> CaseResult { needs_window("17.3", "retired buffers freed only after their composite and capture") }
fn windows_acquire_timeout() -> CaseResult { needs_window("17.3, 17.5", "ACQUIRE's ETIMEDOUT within its timeout plus 100 ms") }
fn windows_resize() -> CaseResult { needs_window("17.1", "RESIZE keeping the old contents in the intersection") }
fn windows_resize_error() -> CaseResult { needs_window("17.2, 19.2", "a refused resize reported, with no Resized event") }
fn windows_resize_rlimit() -> CaseResult { needs_window("17.2", "ENOMEM against RLIMIT_AS with the old buffers intact") }
fn windows_resize_rollback() -> CaseResult { needs_window("17.2", "rollback at every allocation step of CREATE and RESIZE") }
fn windows_rlimit_accounting() -> CaseResult { needs_window("17.2", "window buffers counted against RLIMIT_AS") }
fn windows_capture_across_destroy() -> CaseResult { needs_window("17.3", "a capture across destroy returning one whole commit and its tree") }
fn windows_two_buffer_capture_load() -> CaseResult { needs_window("17.3", "no pins between system calls under continuous capture") }
fn windows_remnant_on_death() -> CaseResult { needs_window("17.3", "remnants for a registered evidence holder") }
fn windows_churn() -> CaseResult { needs_window("17.2, 18", "100 create and destroy cycles returning to the baseline") }
fn windows_distinct_content() -> CaseResult { needs_window("18", "sixteen windows each composited from their own pixels") }
fn windows_raw_client() -> CaseResult { needs_window("17.1, 24", "raw-window_test's create, commit and destroy") }
fn windows_every_target() -> CaseResult { needs_window("17.1, 20.2", "WinInfo naming this target's backend") }
