//! PR-6 census, same-file ordering mutations, and fake-QMP lifecycle oracle.
//! Text checks cover the shipped shell shapes, not arbitrary bash indirection.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const GATE_DIR: &str = "docker/qemu";
const LIB_MARKER: &str = "lib/gate-qmp-backstop.sh";
const DRAIN_CALL: &str = "gqb_dump_and_report";
const PASS_CALL: &str = "gqb_pass_report";
const KILL_WINDOW: usize = 30;

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    let full = repo_path(rel);
    fs::read_to_string(&full)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", full.display()))
}

/// The `.sh` files under `docker/qemu/`, recursively -- the same walk
/// `capture_path_lock_free_structure.rs::gate_scripts()` uses, duplicated
/// (not shared) because these are separate `rustc --test` compilation units
/// with no shared crate between them (`scripts/run-structure-tests.sh`
/// compiles each file standalone).
fn gate_scripts() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("docker/qemu must exist") {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("sh") {
                out.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    walk(&repo_path(GATE_DIR), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(repo_path(""))
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let body = fs::read_to_string(&path).expect("readable script");
            (name, body)
        })
        .collect()
}

/// Census-anchored, not a literal list: the gate scripts that source
/// `lib/gate-qmp-backstop.sh`, whatever that set currently is.
fn converted_gates() -> Vec<(String, String)> {
    gate_scripts()
        .into_iter()
        .filter(|(_, body)| body.contains(LIB_MARKER))
        .collect()
}

fn active_call(line: &str, name: &str) -> bool {
    !line.trim_start().starts_with('#') && line.contains(&format!("$({name}"))
}

fn is_kill_line(line: &str) -> bool {
    let line = line.trim();
    (line.starts_with("kill $QEMU_PID ") || line.starts_with("kill \"$QEMU_PID\" "))
        && line.contains("2>/dev/null")
}

fn uncovered_kill_sites(lines: &[&str]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            (is_kill_line(line)
                && !lines[i.saturating_sub(KILL_WINDOW)..i]
                    .iter()
                    .any(|l| active_call(l, DRAIN_CALL) || active_call(l, PASS_CALL)))
            .then_some(i)
        })
        .collect()
}

// Top-level shell functions in these files close with an unindented brace;
// indented groups and ${...} expansions do not end the function.
fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
    let opening = source
        .find(&format!("{name}() {{\n"))
        .expect("function exists");
    let start = opening + source[opening..].find('\n').unwrap() + 1;
    let end = start + source[start..].find("\n}").expect("closing brace");
    &source[start..end]
}

fn helper_ordered(body: &str) -> bool {
    let lines: Vec<_> = body.lines().collect();
    let kills: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim().starts_with("kill \"$qemu_pid\" "))
        .map(|(i, _)| i)
        .collect();
    kills.len() == 1
        && [DRAIN_CALL, PASS_CALL].iter().all(|name| {
            let calls: Vec<_> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| active_call(l, name))
                .map(|(i, _)| i)
                .collect();
            !calls.is_empty() && calls.iter().all(|i| *i < kills[0])
        })
        && lines[kills[0] + 1..].iter().all(|l| l.trim().is_empty())
}

#[test]
fn census_wiring_calls_both_outcomes() {
    let gates = converted_gates();
    assert!(gates.len() >= 2, "QMP gate census collapsed");
    for (name, body) in gates {
        for call in [DRAIN_CALL, PASS_CALL] {
            assert!(
                body.lines().any(|l| active_call(l, call)),
                "{name} never calls {call}"
            );
        }
    }
}

#[test]
fn strict_kill_follows_qmp_decision() {
    let body = read("docker/qemu/run-aarch64-boot-test-strict.sh");
    let lines: Vec<_> = body.lines().collect();
    assert!(
        lines.iter().any(|l| is_kill_line(l)),
        "kill census collapsed"
    );
    assert!(
        uncovered_kill_sites(&lines).is_empty(),
        "strict kill precedes QMP decision"
    );
}

#[test]
fn moving_the_qmp_decision_after_its_kill_reddens_the_check() {
    let body = read("docker/qemu/run-aarch64-boot-test-strict.sh");
    let mut lines: Vec<_> = body.lines().collect();
    assert!(
        uncovered_kill_sites(&lines).is_empty(),
        "unmutated file must be green"
    );
    let kill = lines
        .iter()
        .position(|l| is_kill_line(l))
        .expect("real kill");
    let decision = lines
        .iter()
        .position(|l| l.contains("local QMP_PRECHECK_PASS"))
        .unwrap();
    assert!(decision < kill);
    let moved = lines.remove(kill);
    lines.insert(decision, moved);
    assert!(
        !uncovered_kill_sites(&lines).is_empty(),
        "kill-before-QMP mutation stayed green"
    );
}

#[test]
fn service_loop_routes_every_verdict_kill_through_ordered_helper() {
    let source = read("docker/qemu/run-aarch64-service-sequence-gate.sh");
    assert!(
        helper_ordered(function_body(&source, "ssg_kill_with_backstop")),
        "helper must decide QMP first and kill last"
    );
    let profile = function_body(&source, "run_profile");
    let boot_loop = &profile[profile.find("for boot in ").expect("per-boot loop")..];
    assert!(
        !boot_loop.lines().any(is_kill_line),
        "raw per-boot kill bypasses QMP"
    );
    let calls: Vec<_> = boot_loop
        .lines()
        .filter(|l| l.trim_start().starts_with("ssg_kill_with_backstop "))
        .collect();
    assert!(calls.len() >= 8, "helper call census collapsed");
    assert_eq!(calls.iter().filter(|l| l.ends_with(" 1")).count(), 1);
    let green = &boot_loop[boot_loop.find("if green_sequence_complete ").unwrap()..];
    assert!(green
        .lines()
        .find(|l| l.trim_start().starts_with("ssg_kill_with_backstop "))
        .unwrap()
        .ends_with(" 1"));
}

#[test]
fn swapping_helper_kill_and_dump_reddens_the_check() {
    let source = read("docker/qemu/run-aarch64-service-sequence-gate.sh");
    let body = function_body(&source, "ssg_kill_with_backstop");
    assert!(helper_ordered(body), "unmutated helper must be green");
    let mut lines: Vec<_> = body.lines().collect();
    let kill = lines
        .iter()
        .position(|l| l.trim().starts_with("kill "))
        .unwrap();
    let dump = lines
        .iter()
        .position(|l| active_call(l, DRAIN_CALL))
        .unwrap();
    lines.swap(kill, dump);
    assert!(
        !helper_ordered(&lines.join("\n")),
        "kill-before-dump mutation stayed green"
    );
}

const FAKE_QMP: &str = r#"
import json, pathlib, signal, socket, sys, time
mode = sys.argv[1]
log = pathlib.Path('commands.log')
log.write_text('')
def record(text):
    with log.open('a') as f:
        f.write('%s %d\n' % (text, time.time_ns() // 1000000))
def terminate(signum, frame):
    record('SIGTERM')
    sys.exit(0)
signal.signal(signal.SIGTERM, terminate)
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.bind('qmp.sock')
s.listen()
pathlib.Path('ready').touch()
while True:
    conn, _ = s.accept()
    with conn:
        conn.sendall(b'{"QMP":{"version":{"qemu":{"major":9,"minor":0,"micro":0}},"capabilities":[]}}\r\n')
        negotiated = False
        for line in conn.makefile('rb'):
            request = json.loads(line)
            command = request['execute']
            record('RECV ' + command)
            if command == 'qmp_capabilities':
                negotiated = True
            else:
                assert negotiated
            if command == 'dump-guest-memory':
                assert request['arguments']['paging'] is False
                if mode == 'hang':
                    while True:
                        time.sleep(1)
                target = request['arguments']['protocol']
                assert target.startswith('file:')
                pathlib.Path(target[5:]).write_bytes(b'fake core bytes')
            result = {'status':'paused','running':False} if command == 'query-status' else {}
            conn.sendall((json.dumps({'return':result})+'\r\n').encode())
"#;

static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Fixture {
    dir: PathBuf,
    server: Option<std::process::Child>,
}
impl Fixture {
    fn new(mode: Option<&str>) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "gqb-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("server.py"), FAKE_QMP).unwrap();
        let server = mode.map(|mode| {
            Command::new("python3")
                .current_dir(&dir)
                .args(["server.py", mode])
                .spawn()
                .unwrap()
        });
        let mut fixture = Self { dir, server };
        if fixture.server.is_some() {
            for _ in 0..100 {
                if fixture.dir.join("ready").exists() {
                    return fixture;
                }
                assert!(
                    fixture
                        .server
                        .as_mut()
                        .unwrap()
                        .try_wait()
                        .unwrap()
                        .is_none(),
                    "server died"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            panic!("fake QMP did not become ready");
        }
        fixture
    }
    fn run(&self, pass: bool, budget: u32) -> (String, std::time::Duration) {
        let start = std::time::Instant::now();
        // Positional parameters preserve arbitrary paths; relative socket names
        // avoid macOS's sockaddr_un path limit in long worktree paths.
        let body = if pass {
            "source \"$1\"; gqb_pass_report"
        } else {
            "source \"$1\"; gqb_dump_and_report qmp.sock capture \"$2\" \"$3\" missing-kernel \"$4\""
        };
        // Independent harness ceiling makes a broken library fail this test
        // instead of wedging the test runner itself.
        let output = Command::new("timeout")
            .current_dir(&self.dir)
            .args([
                "--kill-after=1",
                "15",
                "bash",
                "-euo",
                "pipefail",
                "-c",
                body,
                "test",
            ])
            .arg(repo_path("docker/qemu/lib/gate-qmp-backstop.sh"))
            .arg(repo_path("scripts/forensic-capture.sh"))
            .arg(repo_path("scripts/trace_memory_dump.py"))
            .arg(budget.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "library failed: {:?}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        (String::from_utf8(output.stdout).unwrap(), start.elapsed())
    }
    fn terminate(&mut self) {
        let server = self.server.as_mut().unwrap();
        assert!(Command::new("kill")
            .args(["-TERM", &server.id().to_string()])
            .status()
            .unwrap()
            .success());
        for _ in 0..100 {
            if let Some(status) = server.try_wait().unwrap() {
                assert!(status.success());
                self.server = None;
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("fake QMP did not acknowledge SIGTERM");
    }
    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("commands.log")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = self.server.as_mut() {
            let _ = server.kill();
            let _ = server.wait();
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn fake_qmp_dump_precedes_sigterm_even_when_decode_fails() {
    let mut fixture = Fixture::new(Some("success"));
    let (out, elapsed) = fixture.run(false, 8);
    fixture.terminate(); // the next action, mirroring the gate's first SIGTERM
    assert!(out.starts_with("[QMP_DUMP:capture=complete:"), "{out}");
    assert!(
        out.contains(":decoded_events=-:"),
        "fake bytes must not pretend to decode"
    );
    assert_eq!(out.lines().count(), 1);
    assert!(elapsed.as_secs_f64() < 12.0);
    let log = fixture.log();
    let stamp = |prefix: &str| {
        log.lines()
            .find(|l| l.starts_with(prefix))
            .unwrap()
            .split_whitespace()
            .last()
            .unwrap()
            .parse::<u128>()
            .unwrap()
    };
    assert!(stamp("RECV stop ") < stamp("RECV dump-guest-memory "));
    assert!(
        stamp("RECV dump-guest-memory ") < stamp("SIGTERM "),
        "{log}"
    );
    assert_eq!(
        log.lines()
            .filter(|l| l.starts_with("RECV qmp_capabilities "))
            .count(),
        3
    );
}

#[test]
fn pass_emits_exact_contract_without_any_qmp_traffic() {
    let fixture = Fixture::new(Some("success"));
    let (out, _) = fixture.run(true, 2);
    assert_eq!(
        out,
        "[QMP_DUMP:capture=n/a:reason=n/a:core=n/a:decoded_events=n/a:dump_ms=0]\n"
    );
    assert_eq!(fixture.log(), "", "PASS sent QMP commands");
}

#[test]
fn missing_socket_and_hung_dump_are_partial_and_bounded() {
    let missing = Fixture::new(None);
    let (out, elapsed) = missing.run(false, 3);
    assert!(
        out.contains("capture=partial:reason=qmp_socket_missing:"),
        "{out}"
    );
    assert!(elapsed.as_secs_f64() < 2.0);
    let (invalid, elapsed) = missing.run(false, 0);
    assert!(invalid.contains("reason=invalid_budget:"), "{invalid}");
    assert!(elapsed.as_secs_f64() < 2.0);
    let hanging = Fixture::new(Some("hang"));
    let (out, elapsed) = hanging.run(false, 3);
    assert!(out.contains("capture=partial:reason=qmp_timeout:"), "{out}");
    assert!(elapsed.as_secs_f64() < 10.0, "timeout took {elapsed:?}");
    assert!(
        hanging.log().contains("RECV dump-guest-memory "),
        "hang must reach dump"
    );
}

#[test]
fn core_extraction_uses_physical_load_ranges_and_rejects_missing_or_truncated_bytes() {
    let fixture = Fixture::new(None);
    let script = r#"
import contextlib, io, pathlib, re, runpy, struct, subprocess, sys
from unittest.mock import patch
m = runpy.run_path(sys.argv[1])
linker = m['BREENIX_ROOT'] / 'kernel/src/arch_impl/aarch64/linker.ld'
base = int(re.search(r'^KERNEL_VIRT_BASE\s*=\s*(0x[0-9a-fA-F]+);', linker.read_text(), re.M)[1], 16)
pa = 0x40100000
ring = bytearray(m['buffer_stride']())
struct.pack_into('<QHBBI', ring, 0, 100, next(k for k,v in m['EVENT_TYPES'].items() if v == 'TIMER_TICK'), 0, 0, 0)
struct.pack_into('<Q', ring, m['TRACE_BUFFER_SIZE'] * m['TRACE_EVENT_SIZE'], 1)
ident = b'\x7fELF\x02\x01\x01' + bytes(9)
header = struct.pack('<16sHHIQQQIHHHHHH', ident, 4, 183, 1, 0, 64, 0, 0, 8, 56, 1, 0, 0, 0)
def core(segment_pa, truncate=False):
    # Deliberately unrelated p_vaddr; only p_paddr can locate this ring.
    ph = struct.pack('<IIQQQQQQ', 1, 6, 120, 0x1234, segment_pa, len(ring)+32, len(ring)+32, 4096)
    return header + ph + bytes(32) + (ring[:-1] if truncate else ring)
with patch('subprocess.check_output', return_value='%016x B TRACE_BUFFERS\n' % (base + pa + 32)):
    extract = m['extract_core_buffers']
    data = extract(io.BytesIO(core(pa)), 'kernel', 1)
    assert data == ring
    buffers = m['parse_trace_buffers'](data, 1)
    assert m['validate_trace_buffers'](buffers)[0]
    for blob, message in [(core(pa+4096), 'no PT_LOAD contains'), (core(pa, True), 'truncated PT_LOAD')]:
        try:
            extract(io.BytesIO(blob), 'kernel', 1)
        except SystemExit as error:
            assert message in str(error) and 'segments' in str(error), str(error)
        else:
            raise AssertionError('broken extractor returned bytes')
pathlib.Path('raw.bin').write_bytes(ring)
raw = subprocess.run([sys.executable, sys.argv[1], '--parse', 'raw.bin', '--max-cpus', '1'], text=True, capture_output=True)
assert raw.returncode == 0 and 'TRACE_DECODED_EVENTS:1' in raw.stdout, raw
pathlib.Path('core.elf').write_bytes(core(pa))
missing_kernel = subprocess.run([sys.executable, sys.argv[1], '--parse', 'core.elf'], text=True, capture_output=True)
assert missing_kernel.returncode != 0 and '--kernel is required' in missing_kernel.stderr
"#;
    let output = Command::new("python3")
        .current_dir(&fixture.dir)
        .args(["-c", script])
        .arg(repo_path("scripts/trace_memory_dump.py"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// Q-1/PR-6 fix pass: docs/planning/green-program/failure-capture/PR-6-2026-09-07.md
// section 5 hit AF_UNIX's sun_path ceiling under this repo's own mandated
// worktree convention (BREENIX_GATE_TMP=<worktree>/.gate-tmp) and worked
// around it with a manual, unshipped /tmp symlink. These checks cover the
// structural fix: gqb_alloc_socket/gqb_free_socket exist and are used by
// both converted gates instead of building QMP_SOCK under their own (long,
// worktree-scoped) output directory, and the resulting socket path really
// does bind where the old pattern really does fail.

/// A directory path at least `min_len` bytes long, built from repeated
/// fixed-width components so its exact length is deterministic and
/// independent of this checkout's own location on disk (so the repro below
/// cannot pass by accident just because the sandbox happens to have a short
/// path).
fn long_dir_path(base: &Path, min_len: usize) -> PathBuf {
    let mut dir = base.to_path_buf();
    while dir.as_os_str().len() < min_len {
        dir.push("worktree-scoped-lane-directory-component");
    }
    dir
}

#[test]
fn neither_converted_gate_builds_qmp_sock_under_its_own_output_dir() {
    for (name, body) in converted_gates() {
        assert!(
            body.contains("gqb_alloc_socket"),
            "{name} must obtain QMP_SOCK from gqb_alloc_socket"
        );
        for needle in ["QMP_SOCK=\"$OUTPUT_DIR", "QMP_SOCK=\"$profile_dir"] {
            assert!(
                !body.contains(needle),
                "{name} still builds QMP_SOCK under its own (long, lane-scoped) output directory: {needle}"
            );
        }
    }
}

#[test]
fn gqb_alloc_socket_pairs_with_a_reachable_free_call_in_every_converted_gate() {
    for (name, body) in converted_gates() {
        assert!(
            body.contains("gqb_free_socket"),
            "{name} calls gqb_alloc_socket but never frees the socket directory"
        );
    }
}

#[test]
fn gqb_alloc_socket_returns_a_path_a_real_af_unix_bind_accepts() {
    let output = Command::new("timeout")
        .args([
            "--kill-after=1",
            "10",
            "bash",
            "-euo",
            "pipefail",
            "-c",
            "source \"$1\"; gqb_alloc_socket",
            "test",
        ])
        .arg(repo_path("docker/qemu/lib/gate-qmp-backstop.sh"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gqb_alloc_socket failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = String::from_utf8(output.stdout).unwrap().trim().to_string();
    assert!(!path.is_empty(), "gqb_alloc_socket printed nothing");
    assert!(
        path.len() <= 100,
        "allocated socket path is {} bytes, over the AF_UNIX sun_path budget: {path}",
        path.len()
    );
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap_or_else(|e| {
        panic!("real AF_UNIX bind failed at {path} ({} bytes): {e}", path.len())
    });
    drop(listener);
    let dir = Path::new(&path).parent().unwrap();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_socket_path_under_a_worktree_scoped_output_dir_is_the_reproduced_defect() {
    // Portable, deterministic repro of the exact failure Q-1 reported: an
    // ordinary $OUTPUT_DIR/qmp.sock-shaped path, once BREENIX_GATE_TMP is a
    // worktree-scoped directory (this repo's own mandated lane convention),
    // routinely exceeds AF_UNIX's sun_path limit and a real bind() at that
    // path fails.
    let base = std::env::temp_dir().join(format!(
        "gqb-longpath-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let long_dir = long_dir_path(&base, 110);
    fs::create_dir_all(&long_dir).unwrap();
    let sock = long_dir.join("qmp.sock");
    assert!(
        sock.as_os_str().len() > 104,
        "constructed repro path is not actually over the limit: {} bytes",
        sock.as_os_str().len()
    );
    let result = std::os::unix::net::UnixListener::bind(&sock);
    let _ = fs::remove_dir_all(&base);
    assert!(
        result.is_err(),
        "expected the reproduced $OUTPUT_DIR/qmp.sock-shaped path to overflow \
         AF_UNIX's sun_path limit and fail to bind, but it succeeded"
    );
}

#[test]
fn gqb_free_socket_removes_only_its_own_namespace_directory() {
    let output = Command::new("timeout")
        .args([
            "--kill-after=1",
            "10",
            "bash",
            "-euo",
            "pipefail",
            "-c",
            "source \"$1\"; sock=\"$(gqb_alloc_socket)\"; dir=\"$(dirname \"$sock\")\"; [ -d \"$dir\" ]; gqb_free_socket \"$sock\"; [ ! -e \"$dir\" ]; echo ok",
            "test",
        ])
        .arg(repo_path("docker/qemu/lib/gate-qmp-backstop.sh"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "alloc/free round-trip failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "ok");
}

#[test]
fn gqb_free_socket_refuses_to_remove_paths_outside_its_namespace() {
    let victim = std::env::temp_dir().join(format!("gqb-victim-{}", std::process::id()));
    fs::create_dir_all(&victim).unwrap();
    let sentinel = victim.join("do-not-delete-me");
    fs::write(&sentinel, b"x").unwrap();
    let fake_sock = victim.join("qmp.sock");
    let output = Command::new("timeout")
        .args([
            "--kill-after=1",
            "10",
            "bash",
            "-euo",
            "pipefail",
            "-c",
            "source \"$1\"; gqb_free_socket \"$2\"",
            "test",
        ])
        .arg(repo_path("docker/qemu/lib/gate-qmp-backstop.sh"))
        .arg(&fake_sock)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gqb_free_socket exited nonzero on an out-of-namespace path: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        sentinel.exists(),
        "gqb_free_socket removed a directory it did not allocate"
    );
    let _ = fs::remove_dir_all(&victim);
}
