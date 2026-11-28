# Main host checks, 2026-11-28

Baseline `04f82af2fb973ecdb49b896281fb30909410662c` on `origin/main`.

Commands ran on an Apple Silicon macOS host with the repository’s pinned Rust nightly.
The counts use one invocation per file, crate or build profile. A command that exits
nonzero counts once even if it reports multiple failed cases or compiler errors.
Output excerpts are capped at 280 characters. Complete logs and exit codes are in
`target/host-checks-inventory` in the worktree.

## Counts

| Group | Baseline run | Baseline failed | Final run | Final failed |
| --- | ---: | ---: | ---: | ---: |
| Python scripts | 773 | 226 | 0 | 0 |
| Python host harnesses | 45 | 15 | 0 | 0 |
| Rust integration targets | 431 | 82 | 0 | 0 |
| Host-test crates | 28 | 10 | 0 | 0 |
| x86-64 kernel builds | 15 | 5 | 0 | 0 |
| ARM64 kernel builds with ELF inputs | 10 | 1 | 0 | 0 |
| ARM64 kernel build with BAS | 1 | 1 | 0 | 0 |
| **Total** | **1303** | **340** | **0** | **0** |

## Scope and prerequisites

- Python checks use their executable entry point, unittest discovery, or pytest for test functions without an executable entry point.
- Rust files use `cargo test --test <stem>`. Some of those files are fragments for generated harnesses. Their direct Cargo diagnostics are retained below and the Python wrappers have their own results.
- Host crates use `cargo test --manifest-path tools/<name>-host-tests/Cargo.toml`. The test crate controls any opt-in long cases.
- Kernel profile logs show the feature arguments. The x86 UEFI launcher’s nested Cargo install stalled with an isolated target directory. Direct kernel builds target `x86_64-unknown-none`, the launcher artifact target.
- ARM64 boot-test builds require `simple_exit.elf` and `hello_time.elf`. The initial clean checkout lacked them. They were built from this checkout with the pinned Breenix Rust library and the in-tree libc, then staged before the ARM64 build checks were repeated.
- The ARM64 compiler reports its existing future-incompatibility advisory for the pinned Rust `core` package.
- Two BusyBox source harnesses were excluded under the clean-room rule. `gt710_synthetic.py` and `vigil_record_fixture.py` provide fixtures for other checks.
- Guest boots and live metal access were outside this host inventory.

## Python scripts failures on main

| Command | First diagnostic | Exit | Final |
| --- | --- | ---: | --- |
| `python3 tests/a3_qemu_blob_cache_test.py` | /var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/a3-host-cache-dta5or1j/cache.c:6:10: fatal error: 'epoxy/egl.h' file not found | 1 | Pending |
| `python3 tests/a3_qemu_egl_init_test.py` | FileNotFoundError: [Errno 2] No such file or directory: '/tmp/breenix-qemu-gl-build/src/qemu-10.0.12-utm/build-breenix/qapi/qapi-types-ui.h' | 1 | Pending |
| `python3 tests/a3_qemu_renderer_test.py` | FileNotFoundError: [Errno 2] No such file or directory: '/tmp/breenix-qemu-gl-build/src/qemu-10.0.12-utm/hw/display/virtio-gpu-virgl.c' | 1 | Pending |
| `python3 tests/ahci_completion_timeout_test.py` | error[E0609]: no field 'wait_by' on type '&CmdToken' | 1 | Pending |
| `python3 tests/arm64_image_layout_test.py` | ValueError: cannot find NC_DMA_SIZE | 1 | Pending |
| `python3 tests/arm_boot_hygiene_test.py` | error[E0425]: cannot find function 'reconcile_machine_state' in this scope | 101 | Pending |
| `python3 tests/bld_allocation_budget.py` | usage: bld_allocation_budget.py [-h] objects archive | 2 | Pending |
| `python3 tests/boot_artifacts_test.py` | AssertionError: Items in the first set but not the second: | 1 | Pending |
| `python3 tests/boot_interactive_qmp_test.py` | AssertionError: True is not false : a one-case run must not register with Vigil | 1 | Pending |
| `python3 tests/boot_progress_test.py` | AssertionError: True is not false | 1 | Pending |
| `python3 tests/breenfs_commit_preemption_test.py` | AssertionError: 'preemption_point();' not found in '{\n        self.publishable()?;\n        let result = self.settle_changes();\n        self.result(result)?;\n        #[cfg(test)]\n        self.check_space_state();\n        Ok(self.space_root)\n    }' | 1 | Pending |
| `python3 tests/breenfs_corrupt_fixture_test.py` | AssertionError: 117.01382033294067 not less than 60 | 1 | Pending |
| `python3 tests/breenfs_orphan_observation_test.py` | AssertionError: '16' not found in 'filesystem needs exactly one valid ROOT record' | 1 | Pending |
| `python3 tests/breenfs_scrub_abi_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/breenfs_wait_retry_test.py` | error[E0432]: unresolved import 'super::cache::PressureWait' | 1 | Pending |
| `python3 tests/breengel_console_observation_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/breengel_inflight_screen_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/breengel_runtime_display_test.py` | TypeError: cannot unpack non-iterable int object | 1 | Pending |
| `python3 tests/breengel_scale2_ownership_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/breengel_x86_image_test.py` | EE. | 1 | Pending |
| `python3 tests/bterm_acceptance_paths_test.py` | error[E0433]: failed to resolve: could not find 'input' in the crate root | 1 | Pending |
| `python3 tests/bwm_check_storage_test.py` | AssertionError: 1 != 0 : error[E0609]: no field 'costs' on type 'CheckWorkspace' | 1 | Pending |
| `python3 tests/bwm_completed_frame_timing_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'libuitest' | 1 | Pending |
| `python3 tests/bwm_hold_deadline_test.py` | AssertionError: 'wake::wait_ns(timeout, pacing_ns(), hold)' not found in '//! Breenix Window Manager (bwm) — Pure Compositor + Input Router\n//!\n//! Discovers Breengel client windows registered on the window interface (version 2,\n//! 'docs/gui/breengel.md' section 17), composes… | 1 | Pending |
| `python3 tests/bwm_input_buffer_test.py` | error[E0610]: 'u64' is a primitive type and therefore doesn't have fields | 1 | Pending |
| `python3 tests/bwm_memory_bound_test.py` | error[E0433]: cannot find module or crate 'libuitest' in this scope | 1 | Pending |
| `python3 tests/bwm_prompt_report_test.py` | AssertionError: 101 != 0 : error[E0425]: cannot find function 'poll' in module 'io' | 1 | Pending |
| `python3 tests/bwm_startup_failure_test.py` | AssertionError: True is not false | 1 | Pending |
| `python3 tests/canvas_wait_test.py` | error[E0433]: failed to resolve: could not find 'park' in 'task' | 1 | Pending |
| `python3 tests/cargo_image_stage_test.py` | FileNotFoundError: [Errno 2] No such file or directory: '/Users/wrb/fun/code/breenix.worktrees/fx/host-green/userspace/programs/target/kd/aarch64/ld-breenix.so' | 1 | Pending |
| `python3 tests/cargo_port_test.py` | FileNotFoundError: [Errno 2] No such file or directory: 'llvm-readelf' | 1 | Pending |
| `python3 tests/cargo_suite_test.py` | AssertionError: '4af0de1d89b99abdf23e4164ee821af89e2b12ebd13b630eb27e05d37c14b6c9' != 'd2e8fbde0080ee2ced108abb5175f2d1dd8afc732ed2a1fbafba477835da0b86' | 1 | Pending |
| `python3 tests/console_gpu_admission_test.py` | AssertionError: 'screen::await_binding()' not found in 'fn read_back(tgid: u64, a: &mut abi::Readback, observer: Observer, arg: Option<u64>) -> Result<u64, Refused> {\n    read_back_impl(tgid, a, observer, arg, None)\n}' | 1 | Pending |
| `python3 tests/console_gpu_owner_path_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/cxx_runtime_probe_test.py` | AssertionError: False is not true : run scripts/create_ext2_disk.sh --arch aarch64 first | 1 | Pending |
| `python3 tests/cxx_runtime_test.py` | fatal: cannot change to '/Users/wrb/fun/code/breenix.worktrees/fx/host-green/rust-fork': No such file or directory | 1 | Pending |
| `python3 tests/demo_attach_probe_test.py` | ValueError: max() iterable argument is empty | 1 | Pending |
| `python3 tests/demo_review_test.py` | ValueError: max() iterable argument is empty | 1 | Pending |
| `python3 tests/demos_resize_geometry_test.py` | AssertionError: 1 != 0 : error[E0428]: the name 'Frame' is defined multiple times | 1 | Pending |
| `python3 tests/desktop_gpu_admission_test.py` | error[E0433]: failed to resolve: could not find 'window' in 'libbreenix' | 1 | Pending |
| `python3 tests/display_consumer_admission_test.py` | AssertionError: 1 != 0 : error[E0599]: no method named 'current_mode' found for struct 'Modes' in the current scope | 1 | Pending |
| `python3 tests/e6_abi_test.py` | usage: e6_abi_test.py [-h] [--legacy] [root] | 2 | Pending |
| `python3 tests/e6_runtime_test.py` | usage: e6_runtime_test.py [-h] [--headers-only \| --thread-stack-only] | 2 | Pending |
| `python3 tests/effort_target_guard_test.py` | AssertionError: False is not true : Missing breenix-runs; run scripts/build-host-test-tools.sh from the repository root | 1 | Pending |
| `python3 tests/ext2_create_io_test.py` | AssertionError: error[E0432]: unresolved import 'crate::block::BlockDevice' | 1 | Pending |
| `python3 tests/ext2_mount_data_io_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/ext2_orphan_bitmap_test.py` | error[E0599]: no method named 'remount_read_only' found for struct 'ext2::mount_device::MountDevice' in the current scope | 1 | Pending |
| `python3 tests/ext2_removal_pressure_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/ext2_sparse_extension_test.py` | AssertionError: error[E0425]: cannot find function 'zero_file_tail' in this scope | 1 | Pending |
| `python3 tests/gen_stage_boot_markers_test.py` | AssertionError: 1 != 0 : userspace/programs/src/suite_breengel/stage_boot_markers.in is missing or stale: run scripts/gen-stage-boot-markers.py | 1 | Pending |
| `python3 tests/gpu_batch_wait_test.py` | error[E0609]: no field 'warm_required' on type '&Ctx' | 1 | Pending |
| `python3 tests/gpu_expect_test.py` | AssertionError: Regex didn't match: '[A-Za-z][A-Za-z -]* \\(#[0-9A-Fa-f]{6}\\)' not found in 'Fixed stage renderer: breengel, target "fixed inputs", one passed composition/takeover-giveback tile, key 20 ms, elapsed 250 ms, fps 60, no thumbnails or spotlight; boundary from the sca… | 1 | Pending |
| `python3 tests/gpu_large_target_view_test.py` | error[E0609]: no field 'extra_modes' on type 'Modes' | 1 | Pending |
| `python3 tests/gpu_test_control_reset_test.py` | error[E0425]: cannot find function 'arch_without_interrupts' in the crate root | 1 | Pending |
| `python3 tests/gt710_alpha_report_test.py` | ValueError: missing checkpoint isolation/client-cannot-scanout end | 1 | Pending |
| `python3 tests/gt710_artifact_memory_test.py` | error[E0425]: cannot find value 'resource_row' in this scope | 1 | Pending |
| `python3 tests/gt710_bar_flush_test.py` | error[E0599]: no method named 'live' found for mutable reference '&mut impl Io' in the current scope | 1 | Pending |
| `python3 tests/gt710_batch_progress_test.py` | AssertionError: 1 != 0 : error[E0609]: no field 'warm_required' on type '&&Context' | 1 | Pending |
| `python3 tests/gt710_bench_test.py` | error: function 'run_c5e' is never used | 1 | Pending |
| `python3 tests/gt710_breengel_evidence_test.py` | AssertionError: 'FAIL' != 'PASS' | 1 | Pending |
| `python3 tests/gt710_breengel_full_test.py` | ValueError: missing checkpoint composition/backend-error end | 1 | Pending |
| `python3 tests/gt710_breengel_selection_test.py` | ValueError: missing checkpoint composition/backend-error end | 1 | Pending |
| `python3 tests/gt710_c1_test.py` | error[E0425]: cannot find function 'run_c6' in module 'probe_gpu_bench' | 1 | Pending |
| `python3 tests/gt710_ce_pause_test.py` | error[E0433]: failed to resolve: could not find 'park' in 'task' | 1 | Pending |
| `python3 tests/gt710_ce_transfers_test.py` | IndexError: list index out of range | 1 | Pending |
| `python3 tests/gt710_clock_report_test.py` | ValueError: missing checkpoint isolation/client-cannot-scanout end | 1 | Pending |
| `python3 tests/gt710_clock_test.py` | error: function 'probe_reader' is never used | 1 | Pending |
| `python3 tests/gt710_cold_selection_test.py` | AssertionError: 1 != 0 | 1 | Pending |
| `python3 tests/gt710_completion_milestone_test.py` | ValueError: console and GPU suites require production build.profile | 1 | Pending |
| `python3 tests/gt710_console_backing_test.py` | error[E0599]: no method named 'identify' found for struct 'Arc<Pages>' in the current scope | 1 | Pending |
| `python3 tests/gt710_console_input_diagnostics_test.py` | AssertionError: 101 != 0 :  | 1 | Pending |
| `python3 tests/gt710_console_trace_lock_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: could not find 'window' in the crate root | 1 | Pending |
| `python3 tests/gt710_content_receipt_test.py` | AssertionError: 1 != 0 : error[E0061]: this function takes 2 arguments but 3 arguments were supplied | 1 | Pending |
| `python3 tests/gt710_contexts_orchestration_test.py` | error[E0425]: cannot find function 'checked_traps' in this scope | 1 | Pending |
| `python3 tests/gt710_core_adapter_test.py` | error[E0599]: no method named 'present_complete' found for struct 'NativeChannel' in the current scope | 1 | Pending |
| `python3 tests/gt710_core_ownership_test.py` | error[E0433]: failed to resolve: could not find 'console' in the crate root | 1 | Pending |
| `python3 tests/gt710_coverage_cost_test.py` | KeyError: 'values' | 1 | Pending |
| `python3 tests/gt710_crc_ordering_test.py` | AssertionError: 1 != 0 : error[E0425]: cannot find function 'restore_accepted' in module 'native' | 1 | Pending |
| `python3 tests/gt710_display_evidence_test.py` | error[E0425]: cannot find function 'restore_accepted' in module 'native' | 1 | Pending |
| `python3 tests/gt710_display_restore_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/gt710_draw_submission_test.py` | error[E0308]: mismatched types | 1 | Pending |
| `python3 tests/gt710_fifo_present_test.py` | AssertionError: 'Transition::for_mutation(tgid)?' not found in 'pub(super) fn set_mode(arg: u64, len: u64) -> Result<u64, i64> {\n    let tgid = caller()?;\n    if !owner(tgid) { return Err(EPERM); }\n    let detailed = len == core::mem::size_of::<abi::ModeReport>() as u64;\n    … | 1 | Pending |
| `python3 tests/gt710_frame_retirement_test.py` | error[E0063]: missing field 'draw_groups' in initializer of 'NativeChannel' | 1 | Pending |
| `python3 tests/gt710_gr_two_channel_test.py` | error[E0433]: failed to resolve: could not find 'gk208' in the crate root | 1 | Pending |
| `python3 tests/gt710_icons_admission_test.py` | error[E0433]: failed to resolve: unresolved import | 1 | Pending |
| `python3 tests/gt710_isolation_report_test.py` | ValueError: missing checkpoint isolation/client-cannot-scanout end | 1 | Pending |
| `python3 tests/gt710_latched_observation_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/gt710_logical_mode_path_test.py` | error[E0433]: failed to resolve: could not find 'serial_println' in the crate root | 1 | Pending |
| `python3 tests/gt710_native_console_record_test.py` | thread 'main' panicked at /var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/tmp980lj39c/console.rs:48:2: assertion 'left == right' failed | 1 | Pending |
| `python3 tests/gt710_native_install_test.py` | error[E0432]: unresolved import 'crate::gpu::dev::Device' | 1 | Pending |
| `python3 tests/gt710_native_update_test.py` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'gpu_abi' | 1 | Pending |
| `python3 tests/gt710_observation_path_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/gt710_output_perturb_test.py` | error[E0425]: cannot find value 'RESIDENT_CENSUS' in module 'tc_op' | 1 | Pending |
| `python3 tests/gt710_owner_mode_restore_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/gt710_panic_capture_identity_test.py` | fatal: Not a valid commit name bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb | 1 | Pending |
| `python3 tests/gt710_panic_route_test.py` | .fatal: Not a valid commit name bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb | 1 | Pending |
| `python3 tests/gt710_panic_userd_test.py` | error: couldn't read '/var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/tmpgizh52sx/@ROOT@/kernel/src/gpu/gk208/edid.rs': No such file or directory (os error 2) | 1 | Pending |
| `python3 tests/gt710_performance_report_test.py` | ValueError: missing checkpoint isolation/client-cannot-scanout end | 1 | Pending |
| `python3 tests/gt710_preemption_test.py` | error[E0433]: failed to resolve: could not find 'gpu' in the crate root | 1 | Pending |
| `python3 tests/gt710_present_acceptance_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/gt710_present_miss_trace_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: could not find 'window' in the crate root | 1 | Pending |
| `python3 tests/gt710_present_retirement_test.py` | error[E0609]: no field 'modes' on type 'Display' | 1 | Pending |
| `python3 tests/gt710_present_test.py` | AssertionError: '(0..1 + READBACK_BYTES / 4096).map(\|i\| dma.address(i))' not found in '/* fuc microcode for gf100 PGRAPH/HUB\n *\n * Copyright 2011 Red Hat Inc.\n *\n * Permission is hereby granted, free of charge, to any person obtaining a\n * copy of this software and associate… | 1 | Pending |
| `python3 tests/gt710_presentation_selection_test.py` | ValueError: missing checkpoint isolation/client-cannot-scanout end | 1 | Pending |
| `python3 tests/gt710_probe_console_test.py` | error[E0425]: cannot find value 'display_modes' in this scope | 1 | Pending |
| `python3 tests/gt710_probe_routes_test.py` | AssertionError: Items in the first set but not the second: | 1 | Pending |
| `python3 tests/gt710_probe_startup_test.py` | IndexError: list index out of range | 1 | Pending |
| `python3 tests/gt710_probe_window_test.py` | IndexError: list index out of range | 1 | Pending |
| `python3 tests/gt710_profile_test.py` | AttributeError: 'Driver' object has no attribute 'migration_phase' | 1 | Pending |
| `python3 tests/gt710_quad_coordinates_test.py` | thread 'main' panicked at /var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/tmpwae1gmnz/encode.rs:1865:3: assertion 'left == right' failed: convert each distinct rectangle coordinate once | 1 | Pending |
| `python3 tests/gt710_raster_phase_test.py` | AssertionError: 1 != 0 : error[E0061]: this function takes 2 arguments but 3 arguments were supplied | 1 | Pending |
| `python3 tests/gt710_readback_aperture_test.py` | AssertionError: 'console.width' not found in 'impl Reader {\n    pub(in crate::gk208) fn new(\n        regs: &mut Registers,\n        device: &pci::Device,\n        layout: &mut memory::Layout,\n        console: &super::super::ConsoleSurface,\n        backing: &[(u64, Allocation)… | 1 | Pending |
| `python3 tests/gt710_readback_fence_wait_test.py` | error[E0433]: failed to resolve: unresolved import | 1 | Pending |
| `python3 tests/gt710_request_batch_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/gt710_reset_mode_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/gt710_reset_retirement_test.py` | error[E0609]: no field 'cpu_reading' on type '&Resource' | 1 | Pending |
| `python3 tests/gt710_resources_report_test.py` | ValueError: missing checkpoint isolation/client-cannot-scanout end | 1 | Pending |
| `python3 tests/gt710_restart_state_test.py` | error[E0560]: struct 'Monitor<'_>' has no field named 'resident_polls' | 1 | Pending |
| `python3 tests/gt710_retained_acceptance_test.py` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'gpu_abi' | 1 | Pending |
| `python3 tests/gt710_retained_verdict_test.py` | AssertionError: 1 != 0 | 1 | Pending |
| `python3 tests/gt710_runner_restore_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/gt710_runtime_upload_test.py` | error[E0425]: cannot find function 'upload_slot_with' in module 'gt710_artifacts' | 1 | Pending |
| `python3 tests/gt710_scanout_extent_test.py` | error[E0061]: this function takes 4 arguments but 3 arguments were supplied | 1 | Pending |
| `python3 tests/gt710_session_present_failure_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'gpu_abi' | 1 | Pending |
| `python3 tests/gt710_shapes_report_test.py` | ValueError: missing checkpoint isolation/client-cannot-scanout end | 1 | Pending |
| `python3 tests/gt710_source_alias_test.py` | error[E0425]: cannot find value 'phase' in this scope | 1 | Pending |
| `python3 tests/gt710_stage_proc_test.py` | error[E0425]: cannot find function 'get_monotonic_time_ns' in module 'crate::time' | 1 | Pending |
| `python3 tests/gt710_stall_snapshot_test.py` | error[E0308]: mismatched types | 1 | Pending |
| `python3 tests/gt710_stream_overflow_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/gt710_submitted_receipt_test.py` | AssertionError: 1 != 0 : error[E0061]: this function takes 2 arguments but 3 arguments were supplied | 1 | Pending |
| `python3 tests/gt710_trap_ack_capture_test.py` | error[E0308]: mismatched types | 1 | Pending |
| `python3 tests/gt710_two_boot_acceptance_test.py` | ValueError: core producer capture incomplete, dirty, gapped or wrong commit | 1 | Pending |
| `python3 tests/gt710_ui_step_test.py` | TypeError: UiStep.test_launch_shape_completion_marker.<locals>.<lambda>() got an unexpected keyword argument 'only' | 1 | Pending |
| `python3 tests/gt710_upload_cost_test.py` | error[E0609]: no field 'cpu_reading' on type '&mut Resource' | 1 | Pending |
| `python3 tests/gt710_upload_lifetime_test.py` | error[E0609]: no field 'categories' on type 'Suite' | 1 | Pending |
| `python3 tests/gt710_vbios_perf_test.py` | error: aborting due to 1 previous error | 1 | Pending |
| `python3 tests/gt710_window_front_test.py` | error[E0433]: failed to resolve: could not find 'pipeline' in 'super' | 1 | Pending |
| `python3 tests/gt710_window_lifetime_test.py` | error[E0433]: failed to resolve: could not find 'pipeline' in 'super' | 1 | Pending |
| `python3 tests/gt710_wm_fence_wait_test.py` | AssertionError: 'let id = wait_for_completion(None, true, false,' not found in 'pub(super) fn composite_present_time(fence: u64, content_ready_ns: u64) -> Result<u64, i64> {\n    #[cfg(target_arch = "x86_64")]\n    if dev::backend() == abi::backend::GK208 {\n        let id = wait… | 1 | Pending |
| `python3 tests/gt710_wm_step_test.py` | AssertionError: 0 != 1 | 1 | Pending |
| `python3 tests/gt710_worker_fence_wait_test.py` | error[E0433]: failed to resolve: could not find 'park' in 'task' | 1 | Pending |
| `python3 tests/guskit_picker_render_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'session_dialog' | 1 | Pending |
| `python3 tests/guskit_session_title_test.py` | error[E0425]: cannot find value 'resize' in this scope | 1 | Pending |
| `python3 tests/guskit_sharegone_order_test.py` | AssertionError: 1 != 0 : error[E0425]: cannot find function 'phase' in this scope | 1 | Pending |
| `python3 tests/i6300esb_mmio_test.py` | /opt/homebrew/opt/llvm/bin/llvm-nm: error: /Users/wrb/fun/code/breenix.worktrees/fx/host-green/target/aarch64-breenix-kernel/release/kernel-aarch64: No such file or directory | 1 | Pending |
| `python3 tests/init_bsshd_receipt_test.py` | ValueError: cutover boot did not supply its verification receipts | 1 | Pending |
| `python3 tests/input_pressure_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: could not find 'pipeline' in 'window' | 1 | Pending |
| `python3 tests/input_watermark_order_test.py` | error[E0433]: failed to resolve: could not find 'clock' in the crate root | 1 | Pending |
| `python3 tests/k6_link_record_test.py` | ValueError: cannot resolve -lc in the linker's -L directories | 1 | Pending |
| `python3 tests/libc_busybox_install_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/libc_host_test.py` | usage: libc_host_test.py [-h] [--llvm-nm LLVM_NM] archive | 2 | Pending |
| `python3 tests/llvm_port_test.py` | FileNotFoundError: [Errno 2] No such file or directory: '/Users/wrb/fun/code/breenix.worktrees/fx/host-green/build/llvm-port/x86_64/record.json' | 1 | Pending |
| `python3 tests/loader_stack_reservation_test.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/metal_breenfs_install_recovery_test.py` | TypeError: StoppedUpgradeTest.test_upgrade_replaces_only_the_admitted_next_and_keeps_current_and_rescue.<locals>.<lambda>() got an unexpected keyword argument 'writable_root' | 1 | Pending |
| `python3 tests/metal_console_test.py` | AttributeError: 'Driver' object has no attribute 'migration_phase' | 1 | Pending |
| `python3 tests/metal_persistent_test.py` | FileNotFoundError: [Errno 2] No such file or directory: '/Users/wrb/.config/breenix-metal/host.env' | 1 | Pending |
| `python3 tests/metal_prebuilt_test.py` | AttributeError: 'types.SimpleNamespace' object has no attribute 'stub' | 1 | Pending |
| `python3 tests/metal_smb_test.py` | AssertionError: 'users-hostssh smb-restart' not found in '#!/usr/bin/env bash\n# Installs the x86-metal tools from this checkout onto the boot host (docs/metal/README.md, "Deploying").\n#\n#   scripts/metal/metal-deploy HOST [--create] [--restart]\n#\n# HOST is the boot host\'s s… | 1 | Pending |
| `python3 tests/metal_tools_test.py` | AssertionError: '7d13eeb7f255ef0f301d293a824d4cbcdb6025671a3334db524bd38ae434dd2d' != 'ce78728d038e3e6db31c79155e19454ff403b8e6ff4e1d4976f0c57ffced15e2' | 1 | Pending |
| `python3 tests/migrate_manifest_test.py` | FileNotFoundError: [Errno 2] No such file or directory: PosixPath('/Users/wrb/fun/code/breenix.worktrees/fx/host-green/tools/breenfs/target/debug/breenfs-mkfs') | 1 | Pending |
| `python3 tests/networking_resolve_image_test.py` | AssertionError: 2 != 0 : Creating ext2 disk image... | 1 | Pending |
| `python3 tests/ns7_acceptance_test.py` | error[E0425]: cannot find function 'is_member' in module 'sequence' | 1 | Pending |
| `python3 tests/persistent_migrate_test.py` | AssertionError: 1052672 not less than 1048578 | 1 | Pending |
| `python3 tests/qemu_desktop_stage_test.py` | ModuleNotFoundError: No module named 'vm_site' | 1 | Pending |
| `python3 tests/qemu_gl_test.py` | AssertionError: 1 != 0 : ==> Commit: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa deployed=1 | 1 | Pending |
| `python3 tests/qemu_pci_ecam_test.py` | error[E0425]: cannot find function 'active_boot_pa_limit' in this scope | 1 | Pending |
| `python3 tests/remote_upgrade_test.py` | AssertionError: False is not true : {'boot': '20261023T000000Z-metal', 'powerOn': 999, 'passed': False, 'sshAttempts': 4, 'error': 'ValueError: OpenSSH failed (exit 127)', 'completedAt': 1000} | 1 | Pending |
| `python3 tests/rust_host_elf_parity_test.py` | FileNotFoundError: [Errno 2] No such file or directory: 'llvm-readelf' | 1 | Pending |
| `python3 tests/rust_host_libc_abi_test.py` | usage: rust_host_libc_abi_test.py [-h] [--rustc RUSTC] [--clang CLANG] | 2 | Pending |
| `python3 tests/rust_host_link_test.py` | usage: rust_host_link_test.py [-h] --arch {x86_64,aarch64} [--readelf READELF] | 2 | Pending |
| `python3 tests/rust_host_probe_test.py` | FileNotFoundError: [Errno 2] No such file or directory: 'llvm-readelf' | 1 | Pending |
| `python3 tests/rust_link_image_stage_test.py` | FileNotFoundError: [Errno 2] No such file or directory: '/var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/tmpqp_lcruv/toolchain/rust-host/hosted-libc-x86_64.json' | 1 | Pending |
| `python3 -m unittest discover -s tests -p rust_link_suite_test.py` | FileNotFoundError: [Errno 2] No such file or directory: '/Users/wrb/fun/code/breenix.worktrees/fx/host-green/toolchain/target/release/bld' | 1 | Pending |
| `python3 tests/rustc_port_test.py` | AssertionError: 1 != 0 : Traceback (most recent call last): | 1 | Pending |
| `python3 tests/screen_acceptance_test.py` | ValueError: GT 710 PCI function: expected one record, got 0 | 1 | Pending |
| `python3 tests/screen_binding_test.py` | error[E0425]: cannot find value 'CRC' in this scope | 1 | Pending |
| `python3 tests/screen_evidence_test.py` | error[E0433]: failed to resolve: could not find 'CrcRequest' in 'screen' | 1 | Pending |
| `python3 tests/screen_lifecycle_test.py` | error: field 'perturb_attempted' is never read | 1 | Pending |
| `python3 tests/screen_perturb_acceptance_test.py` | ValueError: missing checkpoint device/no-fallback intent | 1 | Pending |
| `python3 tests/screen_sor_crc_test.py` | ValueError: missing checkpoint isolation/client-cannot-scanout end | 1 | Pending |
| `python3 tests/selfhost_mac_receipt.py` | ValueError: not enough values to unpack (expected 2, got 0) | 1 | Pending |
| `python3 tests/serial_arm_order_test.py` | error[E0433]: failed to resolve: could not find 'tracing' in the crate root | 1 | Pending |
| `python3 tests/serial_tx_drain_test.py` | error[E0599]: no function or associated item named 'acquire_ordered' found for struct 'Ownership' in the current scope | 1 | Pending |
| `python3 tests/server_target_test.py` | ValueError: invalid VM boot identity | 1 | Pending |
| `python3 tests/smbfsd_idle_memory_test.py` | AssertionError: 1 != 0 : error: function 'getpid' is never used | 1 | Pending |
| `python3 tests/stall_launchers_test.py` | AssertionError: 1 != 0 : UTC clock PASS source=UEFI guest=1795867259 host=1795867260.047 offset=-1.047s bracket=[1795867258.952,1795867260.047] | 1 | Pending |
| `python3 tests/suite_outcome_memory_test.py` | error[E0609]: no field 'gui' on type 'Screen' | 1 | Pending |
| `python3 tests/suite_resume_test.py` | PermissionError: [Errno 13] Permission denied: '/var/lib/breenix-metal' | 1 | Pending |
| `python3 tests/suite_runner_roles_test.py` | AssertionError: 'pub [5310 chars]  // The desktop gui-apps subreaper owns the b[13809 chars]   }' != 'pub [5310 chars]  // A full run as PID 1 keeps the memory reco[11370 chars]   }' | 1 | Pending |
| `python3 tests/suite_verdict_test.py` | AssertionError: 1 != 0 : FAIL: SUITE toolchain START cases=70 but the manifest lists 73 cases | 1 | Pending |
| `python3 tests/toolchain_bundle_test.py` | usage: toolchain_bundle_test.py [-h] [--probe PROBE] [--baseline BASELINE] | 2 | Pending |
| `python3 tests/toolchain_cc_corpus_test.py` | AssertionError: 1 != 0 : /Users/wrb/fun/code/breenix.worktrees/fx/host-green/scripts/build-toolchain.sh: line 39: BREENIX_TOOLCHAIN_CLANG: set the pinned absolute Clang path | 1 | Pending |
| `python3 tests/toolchain_inventory_test.py` | FileNotFoundError: [Errno 2] No such file or directory: '/Users/wrb/fun/code/breenix.worktrees/fx/host-green/toolchain/target/debug/bcc' | 1 | Pending |
| `python3 tests/toolchain_suite_linker_test.py` | KeyError: 'TOOLCHAIN_ARM_BUNDLE' | 1 | Pending |
| `python3 tests/toolchain_sysroot_test.py` | usage: toolchain_sysroot_test.py [-h] --clang CLANG --lld LLD --ar AR | 2 | Pending |
| `python3 tests/ui_app_control_test.py` | ValueError: max() iterable argument is empty | 1 | Pending |
| `python3 tests/ui_bundle_disk_test.py` | AssertionError: 1 != 0 | 1 | Pending |
| `python3 tests/ui_case_phases_test.py` | AssertionError: error: field 'mutations' is never read | 1 | Pending |
| `python3 tests/ui_config_isolation_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'control' | 1 | Pending |
| `python3 tests/ui_control_watchdogs_test.py` | AssertionError: Items in the first set but not the second: | 1 | Pending |
| `python3 tests/ui_stored_frames_test.py` | error[E0433]: cannot find 'control' in 'suite' | 101 | Pending |
| `python3 tests/ui_teardown_trace_test.py` | AssertionError: error[E0433]: failed to resolve: unresolved import | 1 | Pending |
| `python3 tests/uitest_arena_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'breenix_suite' | 1 | Pending |
| `python3 tests/uitest_capture_busy_test.py` | AssertionError: 1 != 0 : error[E0425]: cannot find function 'during' in this scope | 1 | Pending |
| `python3 tests/uitest_collect_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: could not find 'control' in 'suite' | 1 | Pending |
| `python3 tests/uitest_devices_runtime_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: could not find 'messages' in the crate root | 1 | Pending |
| `python3 tests/uitest_fixture_creation_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'libbreenix' | 1 | Pending |
| `python3 tests/uitest_local_collect_test.py` | AssertionError: 1 != 0 : error[E0425]: cannot find value 'startup_record' in this scope | 1 | Pending |
| `python3 tests/uitest_ownership_runtime_test.py` | AssertionError: 1 != 0 : error[E0425]: cannot find function 'outputs' in module 'self' | 1 | Pending |
| `python3 tests/uitest_pin_cache_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'libbreenix' | 1 | Pending |
| `python3 tests/uitest_startup_regression_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: could not find 'startup' in the crate root | 1 | Pending |
| `python3 tests/uitest_startup_success_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: could not find 'process' in 'libbreenix' | 1 | Pending |
| `python3 tests/uitest_startup_trace_test.py` | AssertionError: 1 != 0 : error[E0433]: failed to resolve: could not find 'process' in 'libbreenix' | 1 | Pending |
| `python3 tests/userspace_incremental_test.py` | AssertionError: 1795867280638016128 != 1795867280522932397 | 1 | Pending |
| `python3 tests/vfs_statfs_fixture_test.py` | E | 1 | Pending |
| `python3 tests/virtio_blk_pci_driver_test.py` | error[E0432]: unresolved imports 'crate::task::park', 'crate::task::park' | 1 | Pending |
| `python3 tests/virtio_block_timeout_test.py` | error[E0412]: cannot find type 'IoContext' in this scope | 1 | Pending |
| `python3 tests/vm_candidate_test.py` | AssertionError: 1 != 0 : Traceback (most recent call last): | 1 | Pending |
| `python3 tests/vm_clock_verdict_test.py` | AssertionError: 'PASS: SUITE time DONE passed=92 failed=0' not found in 'UTC clock FAIL: missing guest UTC epoch record\nFAIL: SUITE time DONE passed=92 failed=0 skipped=0 total=92: memory allowance 64 differs from MEMORY_RECLAIM_ALLOWANCE_PAGES=48\n' | 1 | Pending |
| `python3 tests/vm_suite_disk_test.py` | AssertionError: 1 != 0 : FAIL: SUITE disk DONE passed=1 failed=0 skipped=0 total=1: memory allowance 64 differs from MEMORY_RECLAIM_ALLOWANCE_PAGES=48 | 1 | Pending |
| `python3 tests/window_claim_end_test.py` | error[E0433]: failed to resolve: could not find 'events' in the crate root | 1 | Pending |
| `python3 tests/window_pipeline_tail_test.py` | error[E0433]: failed to resolve: unresolved import | 1 | Pending |
| `python3 tests/wm_elapsed_test.py` | AssertionError: 1 != 0 : error[E0425]: cannot find function 'hostpointer_acked' in module 'breenix_suite::suite' | 1 | Pending |
| `python3 tests/wm_pointer_removal_test.py` | FileNotFoundError: [Errno 2] No such file or directory: '/var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/tmpniqj_8ba/suite-verdict.txt' | 1 | Pending |
| `python3 tests/x86_console_status_serial_test.py` | thread 'main' panicked at /var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/x86-console-status-1jxh9vg1/check.rs:259:2: assertion failed: serial.contains("SUITE console DONE") | 1 | Pending |
| `python3 tests/x86_qemu_policy_test.py` | AssertionError: False is not true : Missing breenix-runs; run scripts/build-host-test-tools.sh from the repository root | 1 | Pending |

## Python host harnesses failures on main

| Command | First diagnostic | Exit | Final |
| --- | --- | ---: | --- |
| `python3 tests/arm64_boot_disk_selection_test.py` | error[E0433]: failed to resolve: could not find 'boot' in the crate root | 1 | Pending |
| `python3 -m pytest -q tests/breenfs_root_image_test.py` | E                   FileNotFoundError: [Errno 2] No such file or directory: 'mke2fs' | 1 | Pending |
| `python3 tests/breenfs_unmount_host.py` | AssertionError: existing UFS path changed | 1 | Pending |
| `python3 tests/console_fs_logging_test.py` | AssertionError: path printed as a byte array: "sys_link: failed: {:?}", e | 1 | Pending |
| `python3 tests/gt710_console_ribbon_test.py` | thread 'main' panicked at /var/folders/yv/_v01qqx127j449b8bd85bblm0000gn/T/tmpfnmvbl0t/test.rs:37:3: assertion 'left == right' failed | 1 | Pending |
| `python3 tests/libc_stdlib_host.py` | error[E0432]: unresolved import 'crate::lifecycle' | 1 | Pending |
| `python3 tests/namei_path_host.py` | error[E0061]: this function takes 2 arguments but 1 argument was supplied | 1 | Pending |
| `python3 tests/persistent_namespace_host.py` | ValueError: substring not found | 1 | Pending |
| `python3 tests/screen_supervision_test.py` | error[E0412]: cannot find type 'AtomicBool' in this scope | 1 | Pending |
| `python3 tests/stage_boundary_test.py` | error[E0425]: cannot find function 'readback' in module 'win' | 1 | Pending |
| `python3 tests/ufs_reclaim_host.py` | error[E0433]: failed to resolve: could not find 'flock' in 'fs' | 1 | Pending |
| `python3 tests/ufs_unmount_host.py` | error[E0425]: cannot find value 'target' in this scope | 1 | Pending |
| `python3 tests/ui_canonical_test.py` | error: direct cast of function item into an integer | 101 | Pending |
| `python3 tests/ui_capture_test.py` | error[E0432]: unresolved import 'breengel::tree' | 101 | Pending |
| `python3 tests/wm_focus_release_test.py` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'breenix_suite' | 1 | Pending |

## Rust integration targets failures on main

| Command | First diagnostic | Exit | Final |
| --- | --- | ---: | --- |
| `cargo test --test atomic_fixed_picture_test` | error[E0425]: cannot find value 'production_pattern' in this scope | 101 | Pending |
| `cargo test --test block_request_lifetime_structure` | thread 'driver_guard_wedge_call_population_is_pinned' panicked at tests/block_request_lifetime_structure.rs:812:5: assertion 'left == right' failed: guard wedge-call census changed across 60 discovered driver Rust files | 101 | Pending |
| `cargo test --test boot_ribbon_capture_test` | error: couldn't read '/Users/wrb/fun/code/breenix.worktrees/fx/host-green/target/debug/build/breenix-a63b2c5dc61c3f4f/out/boot_ribbon.rs': No such file or directory (os error 2) | 101 | Pending |
| `cargo test --test boot_target` | error[E0004]: non-exhaustive patterns: 'Ok(Target::PersistentMigrate)' not covered | 101 | Pending |
| `cargo test --test breengel_gpu_only` | thread 'png_encoder_is_unreachable_from_upload_capable_crates' panicked at tests/breengel_gpu_only.rs:344:9: userspace/programs: PNG encoder reachable through /Users/wrb/fun/code/breenix.worktrees/fx/host-green/userspace/programs/../../libs/libuitest/Cargo.toml | 101 | Pending |
| `cargo test --test bwm_core_input` | error[E0433]: failed to resolve: could not find 'render_requirements' in the crate root | 101 | Pending |
| `cargo test --test bwm_input_backlog_structure` | thread 'a_slow_client_receives_discrete_events_in_order_and_motion_keeps_barriers' panicked at tests/bwm_input_backlog_structure.rs:25:9: assertion failed: ops.contains("take_while(\|e\| w.ring.try_push(*e))") | 101 | Pending |
| `cargo test --test bwm_launch_points` | thread 'launch_point_extends_the_disjoint_point_allocations' panicked at tests/bwm_launch_points.rs:12:5: assertion 'left == right' failed | 101 | Pending |
| `cargo test --test bwm_shutdown` | thread 'hotkey_spawn_delivers_term_with_default_action_on_both_spawn_paths' panicked at tests/bwm_shutdown.rs:87:55: called 'Option::unwrap()' on a 'None' value | 101 | Pending |
| `cargo test --test bwm_structure` | thread 'restricted_grabs_follow_compositor_claims' panicked at tests/bwm_structure.rs:104:42: called 'Option::unwrap()' on a 'None' value | 101 | Pending |
| `cargo test --test completion_killable` | thread 'killable_wait_checks_fatal_signal_at_each_syscall_wait_boundary' panicked at tests/completion_killable.rs:12:53: called 'Option::unwrap()' on a 'None' value | 101 | Pending |
| `cargo test --test completion_killable_runtime` | thread 'production_completion_observes_pending_sigkill' panicked at tests/completion_killable_runtime.rs:36:5: error[E0432]: unresolved import 'crate::task::park' | 101 | Pending |
| `cargo test --test console_fixed_picture_test` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'oracle' | 101 | Pending |
| `cargo test --test console_panel_vt_test` | error: expected one of '!' or '::', found 'UPDATE_METHOD' | 101 | Pending |
| `cargo test --test console_read_structure` | thread 'device_modes_poll_and_non_sleeping_device_api' panicked at tests/console_read_structure.rs:71:5: assertion 'left == right' failed | 101 | Pending |
| `cargo test --test console_restore_test` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'raster' | 101 | Pending |
| `cargo test --test context_restore_structure` | thread 'inline_schedule_trampoline_retains_the_redundant_elr_guard' panicked at tests/context_restore_structure.rs:4643:5: assertion 'left == right' failed | 101 | Pending |
| `cargo test --test coreproof_component_h_structure` | thread 'inline_schedule_state_is_only_ever_read_via_a_consumed_swap' panicked at tests/coreproof_component_h_structure.rs:33:9: no '.scheduler_ptr' access exists; the census parser has drifted off the source | 101 | Pending |
| `cargo test --test dhcp_client` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'log' | 101 | Pending |
| `cargo test --test dispatch_strand_census_behavior` | error[E0433]: failed to resolve: unresolved import | 101 | Pending |
| `cargo test --test dispatch_strand_census_structure` | thread 'replacement_census_is_wired_to_save_restore_exit_heartbeat_and_completion' panicked at tests/dispatch_strand_census_structure.rs:275:5: kstrandd is no longer spawned beside kloopbackd | 101 | Pending |
| `cargo test --test exec_lock_order_structure` | thread 'exec_smoke_is_wired_into_both_boot_paths' panicked at tests/exec_lock_order_structure.rs:1848:6: T7 validation: "init main must call run_exec_smoke exactly once per architecture, found 0" | 101 | Pending |
| `cargo test --test exec_segments` | thread 'staged_programs_load_the_same_bytes_as_the_whole_file_copy' panicked at tests/exec_segments.rs:606:33: /Users/wrb/fun/code/breenix.worktrees/fx/host-green/userspace/programs/target/x86_64-breenix/release is missing: build userspace for x86_64 first | 101 | Pending |
| `cargo test --test fs_backend_boundary_structure` | thread 'ext2_references_outside_ext2_stay_under_ceiling' panicked at tests/fs_backend_boundary_structure.rs:208:5: ext2 references outside fs/ext2: 1 lines (ceiling 0) | 101 | Pending |
| `cargo test --test green_program_envelope_structure` | thread 'aarch64_tty_oracle_runs_with_exactly_one_persistent_background_process' panicked at tests/green_program_envelope_structure.rs:215:5: never reached a aarch64-gated call to run_tty_oracle(); main()'s call sequence changed shape enough that this census can no longer find its… | 101 | Pending |
| `cargo test --test gt710_bench_test` | thread 'c4_runs_only_three_core_and_three_gpc_after_handshake' panicked at tests/gt710_bench_test.rs:213:5: assertion 'left == right' failed | 101 | Pending |
| `cargo test --test gt710_c2_test` | thread 'c3_pll_coefficients_and_lock_before_source_at_the_three_authorized_targets' panicked at tests/gt710_c2_test.rs:335:81: called 'Result::unwrap()' on an 'Err' value: NotPresent | 101 | Pending |
| `cargo test --test gt710_coverage_cost_test` | error[E0425]: cannot find function, tuple struct or tuple variant 'RUNTIME_TRANSLATE' in module 'encode' | 101 | Pending |
| `cargo test --test gt710_modeset_test` | thread 'captured_sink_uses_preferred_raster_and_exact_fractional_pll' panicked at tests/gt710_modeset_test.rs:463:5: assertion 'left == right' failed: the established 75 Hz timing must be decoded exactly once | 101 | Pending |
| `cargo test --test gt710_probe_encode_test` | thread 'all_probe_routines_bind_their_own_code_and_literal_constant_words' panicked at tests/gt710_probe_encode_test.rs:164:93: called 'Result::unwrap()' on an 'Err' value: Os { code: 2, kind: NotFound, message: "No such file or directory" } | 101 | Pending |
| `cargo test --test gt710_sampled_encode_test` | thread 'warm_draws_cover_the_manifest_rendering_catalogue' panicked at tests/gt710_sampled_encode_test.rs:366:124: called 'Result::unwrap()' on an 'Err' value: Os { code: 2, kind: NotFound, message: "No such file or directory" } | 101 | Pending |
| `cargo test --test gui_apps_shell_output` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'breenish_js' | 101 | Pending |
| `cargo test --test guskit_argv` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'libcollab' | 101 | Pending |
| `cargo test --test guskit_collab` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'libcollab' | 101 | Pending |
| `cargo test --test guskit_session_dialog` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'libcollab' | 101 | Pending |
| `cargo test --test image_panel_test` | error: environment variable 'BREENIX_PANEL_ROOT' not defined at compile time | 101 | Pending |
| `cargo test --test init_desktop_structure` | thread 'supervised_bwm_forwards_its_startup_banner' panicked at tests/init_desktop_structure.rs:32:5: assertion failed: forwarding.contains("line.starts_with(b\"[bwm] Breenix Window Manager starting\")") | 101 | Pending |
| `cargo test --test input_driver_coverage` | thread 'input_driver_callers_are_mapped_and_registration_is_shared' panicked at tests/input_driver_coverage.rs:29:9: missing coverage row: absolute device pending Q1 | 101 | Pending |
| `cargo test --test input_replay_runtime` | error[E0425]: cannot find function 'arch_without_interrupts' in the crate root | 101 | Pending |
| `cargo test --test input_replay_worker` | thread 'mailbox_keeps_scheduler_queries_lock_free_and_serializes_transactions' panicked at tests/input_replay_worker.rs:159:48: called 'Result::unwrap_err()' on an 'Ok' value: Receipt { calls: 2, posted: 4, ring: 4 } | 101 | Pending |
| `cargo test --test input_stream_structure` | thread 'input_core_has_no_architecture_conditionals' panicked at tests/input_stream_structure.rs:191:5: architecture conditionals in input core: ["/Users/wrb/fun/code/breenix.worktrees/fx/host-green/kernel/src/input/stream.rs: #[cfg(target_arch=\"x86_64\")]", "/Users/wrb/fun/code… | 101 | Pending |
| `cargo test --test kernel_build_test` | error[E0432]: unresolved import 'super::clock::perf' | 101 | Pending |
| `cargo test --test libc-malloc-interim-cases` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'stdlib' | 101 | Pending |
| `cargo test --test libc-string-words-cases` | error[E0583]: file not found for module 'mem_asm' | 101 | Pending |
| `cargo test --test libc_alloc_cache` | error: macro-expanded 'extern crate' items cannot shadow names passed with '--extern' | 101 | Pending |
| `cargo test --test libc_shell_foundation_host` | error[E0433]: failed to resolve: could not find 'crt' in the crate root | 101 | Pending |
| `cargo test --test libc_termios` | error[E0425]: cannot find function 'syscall0' in module 'raw' | 101 | Pending |
| `cargo test --test loopback_pump_structure` | thread 'pinned_hold_credits_deliver_once_on_the_home_cpu' panicked at tests/loopback_pump_structure.rs:4507:5: host probe compile: error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'rq' | 101 | Pending |
| `cargo test --test metal_breenfs_upgrade` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'metal_efi' | 101 | Pending |
| `cargo test --test metal_replace_usr_host` | error[E0432]: unresolved import 'crate::live_flash' | 101 | Pending |
| `cargo test --test metal_replace_usr_reconcile` | error[E0432]: unresolved import 'crate::live_flash' | 101 | Pending |
| `cargo test --test metal_slot_transaction` | error[E0432]: unresolved import 'crate::live_flash' | 101 | Pending |
| `cargo test --test metal_userland_flash` | error[E0432]: unresolved import 'crate::live_flash' | 101 | Pending |
| `cargo test --test native_boot_grid_resize` | error[E0433]: failed to resolve: unresolved import | 101 | Pending |
| `cargo test --test quiet_console` | error[E0433]: failed to resolve: unresolved import | 101 | Pending |
| `cargo test --test screen_capture_test` | error[E0432]: unresolved import 'screen_capture' | 101 | Pending |
| `cargo test --test sdk_isolation` | ModuleNotFoundError: No module named 'userspace_generation' | 101 | Pending |
| `cargo test --test self_host_tools` | thread 'aarch64_text_bytes_match_arm_encoding_fixture' panicked at tests/self_host_tools.rs:93:10: called 'Result::unwrap()' on an 'Err' value: Os { code: 2, kind: NotFound, message: "No such file or directory" } | 101 | Pending |
| `cargo test --test service_logs_recovery` | thread 'ring_overrun_is_reported' panicked at tests/service_logs_recovery.rs:11:105: called 'Result::unwrap()' on an 'Err' value: Os { code: 1, kind: PermissionDenied, message: "Operation not permitted" } | 101 | Pending |
| `cargo test --test softirq_deferral_structure` | thread 'softirq_reexport_is_rustfmt_clean' panicked at tests/softirq_deferral_structure.rs:113:5: assertion failed: std::process::Command::new("rustfmt").current_dir(root).args(["--check", | 101 | Pending |
| `cargo test --test strand_handoff_structure` | thread 'injection_stimulus_default_is_idle_gated_and_live_widening_is_cfg_only' panicked at tests/strand_handoff_structure.rs:1927:5: injection gate reads this CPU's idle_thread | 101 | Pending |
| `cargo test --test subreaper_reparent` | thread 'production_prctl_requests' panicked at tests/subreaper_reparent.rs:42:5: error[E0433]: failed to resolve: could not find 'memory' in the crate root | 101 | Pending |
| `cargo test --test suite_intent_test` | error[E0433]: failed to resolve: there are too many leading 'super' keywords | 101 | Pending |
| `cargo test --test suite_manifests` | thread 'metal_stream_is_independent_and_resolves_its_measures' panicked at tests/suite_manifests.rs:1864:5: assertion 'left == right' failed | 101 | Pending |
| `cargo test --test suite_screen_test` | error[E0433]: failed to resolve: there are too many leading 'super' keywords | 101 | Pending |
| `cargo test --test teardown_rustfmt_structure` | thread 'teardown_provider_is_rustfmt_clean' panicked at tests/teardown_rustfmt_structure.rs:12:5: full-provider rustfmt failed: Diff in /Users/wrb/fun/code/breenix.worktrees/fx/host-green/kernel/src/tracing/providers/teardown.rs:1406: | 101 | Pending |
| `cargo test --test teardown_structure` | thread 'a_fatal_el1_redirect_cannot_abandon_its_scheduler_guard' panicked at tests/teardown_structure.rs:1216:49: an EL1 fault on the scheduler's owner CPU cannot return to idle with the guard stranded | 101 | Pending |
| `cargo test --test ttbr0_shadow_reconciliation_structure` | thread 'the_asid_tag_is_constructed_in_one_place' panicked at tests/ttbr0_shadow_reconciliation_structure.rs:2596:5: these sites build an ASID field themselves instead of calling 'bind_process_root', so the ASID they install is not one the allocator gave that address space: [ | 101 | Pending |
| `cargo test --test tty_irq_pm_structure` | thread 'every_tty_input_irq_handler_is_free_of_a_blocking_acquisition' panicked at tests/tty_irq_pm_structure.rs:691:50: #821 rule 6: "found 2 input entries; this rule was written against at least 4, and a census that fell below that is a rule that stopped looking" | 101 | Pending |
| `cargo test --test ui_canonical` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'libgfx_ref' | 101 | Pending |
| `cargo test --test ui_capture_app` | error[E0432]: unresolved import 'breengel::tree' | 101 | Pending |
| `cargo test --test ui_captures` | error[E0432]: unresolved imports 'crate::errno', 'crate::io', 'crate::time', 'crate::types' | 101 | Pending |
| `cargo test --test ui_click_target` | thread 'reader_probe_reaches_content_instead_of_resize_chrome' panicked at tests/ui_click_target.rs:124:5: error[E0599]: no method named 'hit_content' found for reference '&Window' in the current scope | 101 | Pending |
| `cargo test --test ui_frame_schema` | error[E0433]: failed to resolve: could not find 'control' in 'suite' | 101 | Pending |
| `cargo test --test ui_pointer_script` | thread 'attaching_a_desktop_reader_wakes_an_idle_compositor' panicked at tests/ui_pointer_script.rs:224:5: error[E0412]: cannot find type 'SessionHandle' in this scope | 101 | Pending |
| `cargo test --test ui_stored_frames` | error[E0433]: failed to resolve: could not find 'control' in 'suite' | 101 | Pending |
| `cargo test --test uitest_arena` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'breenix_suite' | 101 | Pending |
| `cargo test --test userspace_markers` | thread 'userspace_stage_markers_do_not_occur_in_kernel_source' panicked at tests/userspace_markers.rs:138:48: called 'Result::unwrap()' on an 'Err' value: Error { kind: InvalidData, message: "stream did not contain valid UTF-8" } | 101 | Pending |
| `cargo test --test window_hit_test` | thread 'hit_test_abi_layout' panicked at tests/window_hit_test.rs:36:5: assertion 'left == right' failed | 101 | Pending |
| `cargo test --test window_input_backpressure_structure` | thread 'a_full_v2_ring_refuses_the_next_key_without_changing_queued_keys' panicked at tests/window_input_backpressure_structure.rs:20:5: assertion failed: ops.contains("take_while(\|e\| w.ring.try_push(*e))") | 101 | Pending |
| `cargo test --test x86_boot_tests_profile_structure` | thread 'context_fork_is_profile_independent' panicked at tests/x86_boot_tests_profile_structure.rs:112:5: assertion 'left == right' failed | 101 | Pending |
| `cargo test --test x86_madt_reader_bounds` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'alloc' | 101 | Pending |

## Host-test crates failures on main

| Command | First diagnostic | Exit | Final |
| --- | --- | ---: | --- |
| `cargo test --manifest-path tools/console-host-tests/Cargo.toml` | error[E0433]: failed to resolve: could not find 'status' in the crate root | 101 | Pending |
| `cargo test --manifest-path tools/ext2-host-tests/Cargo.toml` | thread 'errno::every_ext2_error_literal_is_classified' panicked at tests/../src/errno.rs:117:5: unclassified ext2 errors: [ | 101 | Pending |
| `cargo test --manifest-path tools/ipc-host-tests/Cargo.toml` | error[E0432]: unresolved import 'crate::fs::errno' | 101 | Pending |
| `cargo test --manifest-path tools/kmem-host-tests/Cargo.toml` | thread 'every_runtime_allocation_site_has_an_owner' panicked at tests/oom_inventory.rs:422:5: 13 of 1463 runtime allocation sites have no inventory row, for example: | 101 | Pending |
| `cargo test --manifest-path tools/shmem-api-host-tests/Cargo.toml` | error[E0433]: failed to resolve: could not find 'window' in the crate root | 101 | Pending |
| `cargo test --manifest-path tools/shmsem-host-tests/Cargo.toml` | error[E0433]: failed to resolve: could not find 'waitqueue' in 'task' | 101 | Pending |
| `cargo test --manifest-path tools/udp-host-tests/Cargo.toml` | error[E0433]: failed to resolve: unresolved import | 101 | Pending |
| `cargo test --manifest-path tools/ui-host-tests/Cargo.toml` | error[E0433]: failed to resolve: use of unresolved module or unlinked crate 'breenix_suite' | 101 | Pending |
| `cargo test --manifest-path tools/unix-host-tests/Cargo.toml` | error[E0432]: unresolved imports 'crate::fs::attrs', 'crate::fs::vfs' | 101 | Pending |
| `cargo test --manifest-path tools/vfs-host-tests/Cargo.toml` | thread 'read_conformance::readonly_mapping_reads_holes_and_zero_fills_the_partial_tail' panicked at src/../tests/read_conformance.rs:608:5: assertion 'left == right' failed | 101 | Pending |

## x86-64 kernel builds failures on main

| Command | First diagnostic | Exit | Final |
| --- | --- | ---: | --- |
| `cargo build --release --features boot_tests --target x86_64-unknown-none -p kernel --bin kernel` | error[E0609]: no field 'owner_pid' on type 'Result<thread::Thread, &str>' | 101 | Pending |
| `cargo build --release --features boot_tests,ext2_lock_race --target x86_64-unknown-none -p kernel --bin kernel` | error[E0609]: no field 'owner_pid' on type 'Result<thread::Thread, &str>' | 101 | Pending |
| `cargo build --release --features boot_tests,fs_fault_inject --target x86_64-unknown-none -p kernel --bin kernel` | error[E0609]: no field 'owner_pid' on type 'Result<thread::Thread, &str>' | 101 | Pending |
| `cargo build --release --features boot_tests,testing,external_test_bins --target x86_64-unknown-none -p kernel --bin kernel` | error[E0609]: no field 'owner_pid' on type 'Result<thread::Thread, &str>' | 101 | Pending |
| `cargo build --release --features testing,boot_tests,x86_staged_registry,btrt --target x86_64-unknown-none -p kernel --bin kernel` | error[E0609]: no field 'owner_pid' on type 'Result<thread::Thread, &str>' | 101 | Pending |

## ARM64 kernel builds with ELF inputs failures on main

| Command | First diagnostic | Exit | Final |
| --- | --- | ---: | --- |
| `cargo build --release --features boot_tests,coreproof --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | error[E0432]: unresolved imports 'super::sites::SiteId', 'super::sites::ALL' | 101 | Pending |

## ARM64 kernel build with BAS failures on main

| Command | First diagnostic | Exit | Final |
| --- | --- | ---: | --- |
| `BREENIX_KERNEL_ASSEMBLER=bas cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | error: failed to run custom build command for 'kernel v0.1.0 (/Users/wrb/fun/code/breenix.worktrees/fx/host-green/kernel)' | 101 | Pending |

## Kernel compiler warnings

- `BREENIX_KERNEL_ASSEMBLER=bas cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` reported warning: build failed, waiting for other jobs to finish.... Final warning: pending.
- `cargo build --release --features dns_test_only --target x86_64-unknown-none -p kernel --bin kernel` reported warning: unused import: `contract_runner`. Final warning: pending.
- `cargo build --release --features blocking_recv_test --target x86_64-unknown-none -p kernel --bin kernel` reported warning: unused import: `contract_runner`. Final warning: pending.
- `cargo build --release --features nonblock_eagain_test --target x86_64-unknown-none -p kernel --bin kernel` reported warning: unused import: `contract_runner`. Final warning: pending.
- `cargo build --release --features kthread_test_only --target x86_64-unknown-none -p kernel --bin kernel` reported warning: unused import: `contract_runner`. Final warning: pending.
- `cargo build --release --features workqueue_test_only --target x86_64-unknown-none -p kernel --bin kernel` reported warning: unused import: `contract_runner`. Final warning: pending.

## Decisions and remaining work

### Behavior decisions

- **ARM64 reclamation at a scheduler handoff.** Decide which handoffs must run reclamation before switching. `waitqueue::switch_while_blocked` deliberately skips reclamation after publishing its blocked continuation. The completion wait still runs it, and the regular scheduler wrapper also places a proof seam between reclamation and dispatch. The check needs the intended contract for each role.
- **Static locks in drop paths.** Decide whether the `WritableOwner` and `FileLocks` registry operations can run on any IRQ-sensitive drop path. The census finds their current bare mutexes. Changing lock type and IRQ masking requires a custody and lock-order decision.
- **Reparenting across retained tombstones.** Decide whether the ancestor walk may use a retained tombstone’s parent link while skipping it as a reaper. The raw lookup in `reparent_children_to_init` currently sees that link, while using the usual live-row predicate would end that ancestry walk.
- **Lifetime of the GT710 clock failure string.** Decide how the error text should be retained across clock initialization attempts. The allocation identified by the `Box::leak` check is real. The census still rejects it.
- **Suite setup failures before the case table starts.** Decide where setup panics, explicit exit and diagnostics belong in the BreenGL, console and GPU suites. The manifest check requires observable suite outcomes. Its setup guard still fires. The case-observer DSL diagnostics also need independent parser work.
- **Suite and tool artifact inputs.** The reference-link allocation probe and E6 runtime and ABI probes require staged input directories or libraries. The no-argument results remain visible so these are tracked as prerequisite gaps.

Other red targets in the tables still need investigation. Exit codes alone cannot distinguish regressions from outdated harnesses or missing local inputs.
