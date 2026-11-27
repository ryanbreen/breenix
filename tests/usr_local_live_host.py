"""Run one focused DEV-9 host check by name."""
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PROGRAMS = ROOT / "userspace/programs/src"
PREFIX = "/usr/local/sbin:/usr/local/bin"


def function(source, name):
    return re.search(rf"^(?:pub )?fn {name}(?:<[^>]+>)?\(.*?^}}", source, re.M | re.S)[0]


def run_rust(directory, source, *args):
    path = directory / "check.rs"
    binary = directory / "check"
    path.write_text(source)
    subprocess.run(["taskpolicy", "-b", "rustc", "--edition=2021", path, "-o", binary],
                   cwd=ROOT, check=True, timeout=20)
    subprocess.run([binary, directory, *args], check=True, timeout=5)


def check_cutover(mode):
    kernel = ROOT / "kernel/src"
    shell = (ROOT / "tests/usr_local_mount_host.rs").read_text()
    boot = (kernel / "fs/vfs/boot.rs").read_text()
    cutover = re.search(r"^fn mount_cutover\(.*?(?=^impl ExplicitMount)", boot, re.M | re.S)[0]
    modules = {
        "attrs": "fs/attrs.rs",
        "errno": "fs/errno.rs",
        "syscall_errno": "syscall/errno.rs",
        "mount_table": "fs/vfs/mount_table.rs",
    }
    paths = "\n".join(f'#[path = "{kernel / path}"] pub mod {module};'
                      for module, path in modules.items())
    with tempfile.TemporaryDirectory() as scratch:
        run_rust(Path(scratch), "#![allow(dead_code)]\n" + paths + "\n" + shell + cutover, mode)


class LiveLayer(unittest.TestCase):
    def test_usr_local_is_a_writable_system_bind(self):
        check_cutover("mount")

    def test_usr_local_creates_root_wheel_directories(self):
        check_cutover("modes")

    def test_default_path_order(self):
        # Also check the session consumers so an independent default cannot
        # replace the shared login environment or sudo's reset/search path.
        consumers = {
            "login.rs": ("session::login_environment(",),
            "bsshd.rs": ("session::login_environment(",),
            "su.rs": ("session::login_environment(", "session::SECURE_PATH"),
            "sudo.rs": ("session::SECURE_PATH", 'format!("PATH={}", session::SECURE_PATH)'),
        }
        for file, calls in consumers.items():
            for call in calls:
                self.assertIn(call, (PROGRAMS / file).read_text())

        defaults = {
            "scripts/create_ext2_disk.sh": (r'env\("PATH", "([^"]+)"\)', 2),
            "kernel/src/process/manager.rs": (r'b"PATH=([^"]+)\\0"', 2),
            "libs/libbreenix/src/accounts/session.rs": (
                r'SECURE_PATH: &str = "([^"]+)"|^        "(/[^"]+:[^"]+)"', 3),
            "userspace/programs/src/su.rs": (r'unwrap_or_else\(\|_\| "([^"]+)"\.into\(\)\)', 1),
            "userspace/programs/src/bsh.rs": (
                r'String::from\("(/[^"]+:[^"]+)"\)', 2),
            "userspace/programs/src/init_shell.rs": (r'DEFAULT_PATH: &str = "([^"]+)"', 1),
            "userspace/programs/src/init_services.rs": (r'"PATH=([^"]+)"', 1),
            "libs/libbreenix-libc/src/unistd.rs": (r'DEFAULT_PATH: &\[u8\] = b"([^"]+)"', 1),
            "libs/libbreenix-libc/src/spawn.rs": (r'&b"(/[^"]+:[^"]+)"', 1),
            "libs/libbreenix-libc/include/paths.h": (r'_PATH_(?:STD|DEF)PATH "([^"]+)"', 2),
        }
        for file, (pattern, count) in defaults.items():
            with self.subTest(file=file):
                matches = [next(group for group in m.groups() if group is not None)
                           for m in re.finditer(pattern, (ROOT / file).read_text(), re.M)]
                self.assertEqual(len(matches), count)
                for path in matches:
                    components = path.split(":")
                    self.assertEqual(":".join(components[:2]), PREFIX)
                    self.assertEqual(len(components), len(set(components)))

    def test_init_reaches_bshrc_reconciliation_on_read_only_root(self):
        source = (ROOT / "libs/libbreenix/src/etc_reconcile.rs").read_text()
        # The image already owns /bin with the declared attributes. Model
        # the read-only generation rejecting a redundant metadata write.
        with tempfile.TemporaryDirectory() as scratch:
            run_rust(Path(scratch), r'''
use std::{fs::OpenOptions, os::unix::fs::{OpenOptionsExt, PermissionsExt}, path::Path};
type Result<T> = std::result::Result<T, String>;

mod layout {
    pub const TABLE: &str = "dir bin 0755 0 0\n";
}

mod fs {
    use std::{io, path::Path};
    pub use std::fs::{create_dir, Permissions};

    pub struct Metadata;
    #[allow(dead_code)]
    impl Metadata {
        pub fn is_dir(&self) -> bool { true }
        pub fn is_file(&self) -> bool { false }
        pub fn mode(&self) -> u32 { 0o40755 }
        pub fn uid(&self) -> u32 { 0 }
        pub fn gid(&self) -> u32 { 0 }
    }

    pub fn symlink_metadata(_: &Path) -> io::Result<Metadata> { Ok(Metadata) }
    pub fn set_permissions(_: &Path, _: Permissions) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(30))
    }
}

fn exists(_: &Path) -> Result<bool> { Ok(true) }
fn clear_volatile(_: &Path) -> Result<()> { Ok(()) }
fn reconcile_selected(_: &Path) -> Result<Vec<String>> { Ok(vec!["bshrc".into()]) }
''' + function(source, "io") + "\n" + function(source, "prepare") + r'''
fn main() {
    let root = std::env::args_os().nth(1).unwrap();
    assert_eq!(prepare(Path::new(&root)).unwrap(), ["bshrc"]);
}
''')

    def test_bsh_reads_only_a_safe_user_rc(self):
        source = (PROGRAMS / "bsh.rs").read_text()
        startup = function(source, "prepare_shell_context")
        self.assertRegex(startup, r'(?s)load_rc_file\(&mut ctx, "/etc/bshrc"\).*read_user_rc\(')
        with tempfile.TemporaryDirectory() as scratch:
            run_rust(Path(scratch), r'''
use std::{fs, io::Read, os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt}, path::Path};
// Darwin open flags for the same nonblocking, no-follow open.
const USER_RC_OPEN_FLAGS: i32 = 0x104;
''' + function(source, "read_user_rc") + r'''
fn main() {
    let root = std::env::args_os().nth(1).unwrap();
    let path = Path::new(&root).join(".bshrc");
    assert_eq!(read_user_rc(&path, 0), None);
    fs::write(&path, "user script").unwrap();
    let uid = fs::metadata(&path).unwrap().uid();
    for (mode, allowed) in [(0o600, true), (0o644, true), (0o620, false), (0o602, false)] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(read_user_rc(&path, uid).as_deref(), allowed.then_some("user script"));
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read_user_rc(&path, uid + 1), None);
    let target = path.with_file_name("target");
    fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert_eq!(read_user_rc(&path, uid), None);
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(read_user_rc(&path, uid), None);
}
''')

    def test_flash_keeps_live_usr_local_outside_the_generation(self):
        installer = PROGRAMS / "persistent_install.rs"
        staging = function((PROGRAMS / "persistent_install/breenfs.rs").read_text(), "copy_generation")
        # Run the guest's actual staging function on an ordinary host
        # directory. The guest-only snapshot ioctls are outside this seam.
        source = re.sub(r'#\[path = "([^"]+)"\]',
                        lambda m: f'#[path = "{(installer.parent / m[1]).resolve()}"]',
                        installer.read_text())
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch)
            module = directory / "installer.rs"
            module.write_text("#![allow(dead_code)]\n" + source + "\nmod stage {\nuse super::*;\n" + staging + r'''
pub fn verify(root: &Path) {
    let source = root.join("image");
    let system = root.join("system");
    let live = system.join(".breenix-shared/usr-local/bin");
    let staged = system.join("draft");
    for path in [source.join("bin"), source.join("sbin"), source.join("usr/share/factory/etc"),
                 source.join("usr/local/bin"), live.clone(), staged.clone()] {
        fs::create_dir_all(path).unwrap();
    }
    fs::write(live.join("tool"), b"live delivery").unwrap();
    fs::write(source.join("usr/local/bin/tool"), b"image copy").unwrap();
    fs::write(source.join("bin/tool"), b"image tool").unwrap();
    copy_generation(&source, &staged, "next", &local::LocalFiles::default(), "old", None).unwrap();
    assert_eq!(fs::read(live.join("tool")).unwrap(), b"live delivery");
    assert_eq!(fs::read(staged.join("bin/tool")).unwrap(), b"image tool");
    assert_eq!(fs::read_dir(staged.join("usr/local")).unwrap().count(), 0);
}
}
pub use stage::verify;
''')
            run_rust(directory, '''
mod installer;
fn main() {
    installer::verify(std::path::Path::new(&std::env::args_os().nth(1).unwrap()));
}
''')


if __name__ == "__main__":
    unittest.main()
