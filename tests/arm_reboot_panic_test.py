"""Structural checks for ARM PSCI shutdown and panic recovery."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]
ARM = ROOT / "kernel/src/arch_impl/aarch64"


def code(path):
    return re.sub(r"//[^\n]*", "", path.read_text())


class ArmRebootPanic(unittest.TestCase):
    def test_shutdown_uses_shared_psci_after_serial_drain(self):
        helper = ARM / "psci.rs"
        self.assertTrue(helper.exists(), "ARM needs a shared PSCI power helper")
        psci = code(helper)
        self.assertIn("pub mod psci;", code(ARM / "mod.rs"))
        for operation, function_id in [("system_reset", "0x84000009"),
                                       ("system_off", "0x84000008")]:
            self.assertRegex(psci, rf"pub fn {operation}\(\)\s*\{{\s*power_call\({function_id}\);")
        self.assertIn('"hvc #0"', psci)
        self.assertIn('"hvc #0"', code(ARM / "smp.rs"))
        self.assertRegex(psci, r'inout\("x0"\)\s*function_id\s*=>\s*_')
        self.assertNotRegex(psci, r"lock\(|alloc::|format!|serial|log::")
        reboot = code(ROOT / "kernel/src/syscall/reboot.rs")
        arm = reboot[reboot.rindex('#[cfg(target_arch = "aarch64")]'):]
        arm = arm[:arm.index("\n}")]
        drain = arm.index("crate::serial::drain_output();")
        reset = arm.index("crate::arch_impl::aarch64::psci::system_reset();")
        off = arm.index("crate::arch_impl::aarch64::psci::system_off();")
        self.assertLess(drain, reset)
        self.assertLess(reset, off)
        self.assertRegex(arm, r"if command == 0x01234567\s*\{[^}]*system_reset\(\);\s*\}\s*else\s*\{[^}]*system_off\(\);")
        self.assertGreater(arm.index("crate::hlt_loop();"), off)
        self.assertNotIn("hvc", arm)

    def test_panic_prints_message_and_resets_only_outside_test_profiles(self):
        source = code(ROOT / "kernel/src/main_aarch64.rs")
        panic = source[source.index("fn panic(info: &PanicInfo)"):]
        panic = panic[:panic.index("\n}")]
        banner = panic.index('serial_println!("KERNEL PANIC: {}", info.message());')
        dump = panic.index("kernel::tracing::output::trace_dump_counters();")
        show = panic.index("kernel::graphics::boot_screen::show_panic(info);")
        self.assertLess(banner, dump)
        self.assertLess(dump, show)
        tail = panic[show + len("kernel::graphics::boot_screen::show_panic(info);"):]
        self.assertRegex(tail, r'#\[cfg\(not\(any\(feature = "testing", feature = "boot_tests"\)\)\)\]\s*kernel::arch_impl::aarch64::psci::system_reset\(\);')
        self.assertLess(tail.index("psci::system_reset();"), tail.index('"wfi"'))
        self.assertNotRegex(tail, r"lock\(|alloc::|format!|print|drain_output|log::")

    def test_fault_feature_panics_early_in_kernel_main(self):
        source = code(ROOT / "kernel/src/main_aarch64.rs")
        main = source[source.index('pub extern "C" fn kernel_main('):]
        main = main[:main.index("\n}")]
        trigger = '#[cfg(feature = "server_reboot_fault")]\n    trigger_reboot_panic();'
        self.assertIn(trigger, main)
        self.assertLess(main.index("serial::init_serial();"), main.index(trigger))
        self.assertLess(main.index(trigger), main.index("kernel::memory::frame_allocator::init_aarch64("))
        self.assertRegex(source, r'#\[cfg\(all\(target_arch = "aarch64", feature = "server_reboot_fault"\)\)\]\s*fn trigger_reboot_panic\(\)\s*\{\s*panic!\("reboot fault test"\);\s*\}')


if __name__ == "__main__":
    unittest.main()
