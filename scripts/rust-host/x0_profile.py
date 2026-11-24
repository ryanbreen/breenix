"""X0.9 capture driver and Cargo's per-rustc measurement wrapper."""

import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGES = ("bobj", "bas", "bld", "bar", "bnm", "bcc", "bmake")
TIME = "/usr/bin/time"
CHANNEL = "nightly-2025-06-24"
TARGET = "x86_64-breenix"
STACK_MIN, STACK_MAX = 64, 64 * 1024
WRITE_CALLS = ("write", "writev", "pwrite64", "pwritev", "pwritev2")
DETAIL_CALLS = ("mmap", "clone", "clone3", *WRITE_CALLS)
FAMILIES = {
    "memory": "mmap mmap2 mremap munmap mprotect madvise brk",
    "limits": "getrlimit setrlimit prlimit64",
    "process_and_threads": "clone clone3 fork vfork execve execveat",
    "pipes": "pipe pipe2",
    "wait": "wait4 waitid waitpid",
    "locks": "flock fcntl fcntl64",
    "file_metadata": "access faccessat faccessat2 fstat fstat64 newfstatat lstat stat statx "
                     "readlink readlinkat getdents64",
}


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def call(args, *, stack=STACK_MAX, **kwargs):
    jobserver = re.search(r"--jobserver-(?:auth|fds)=(\d+),(\d+)",
                          os.environ.get("CARGO_MAKEFLAGS", ""))
    # Cargo passes the jobserver pipe through its wrapper. subprocess otherwise
    # closes it before bash, time, strace and rustc can inherit it.
    pass_fds = tuple(map(int, jobserver.groups())) if jobserver else ()
    return subprocess.run(
        ["bash", "-c", 'ulimit -S -s "$1" && shift && exec "$@"', "x0-stack", str(stack), *args],
        pass_fds=pass_fds,
        **kwargs,
    )


def trace_details(path, scratch):
    anonymous = file_backed = written = 0
    root = None
    threads = set()
    exited = set()
    peak_threads = 1
    unfinished = {}
    with path.open() as trace:
        for line in trace:
            pid, _, event = line.split(maxsplit=2)
            if root is None:
                root = pid
                threads.add(pid)
            if event.startswith("+++"):
                exited.add(pid)
                threads.discard(pid)
                continue
            if event.rstrip().endswith("<unfinished ...>"):
                unfinished[pid] = event.split("<unfinished ...>")[0]
                continue
            if event.startswith("<... "):
                event = unfinished.pop(pid) + event.split(" resumed>", 1)[1]
            match = re.match(r"(\w+)\((.*)\)\s+= (0x[0-9a-f]+|[0-9]+)(?:\s|$)", event)
            if not match:
                continue
            name, args, result = match.groups()
            if name in ("clone", "clone3") and "CLONE_THREAD" in args and pid in threads:
                # A very short-lived child can exit before clone returns in the trace.
                peak_threads = max(peak_threads, len(threads) + 1)
                if result not in exited:
                    threads.add(result)
            if name == "mmap":
                _, length, protection, flags, descriptor, _ = args.split(", ", 5)
                size = int(length)
                if "MAP_ANONYMOUS" in flags:
                    anonymous = max(anonymous, size)
                else:
                    file_backed = max(file_backed, size)
                    if (f"<{scratch}/" in descriptor and "MAP_SHARED" in flags
                            and "PROT_WRITE" in protection):
                        written += size
            if name in WRITE_CALLS and f"<{scratch}/" in args.split(",", 1)[0]:
                written += int(result)
    return {
        "peak_threads": peak_threads,
        "largest_anonymous_mapping_bytes": anonymous,
        "largest_file_mapping_bytes": file_backed,
        "scratch_bytes": written,
    }


def compile_unit(argv, probe):
    name = argv[argv.index("--crate-name") + 1]
    kind = argv[argv.index("--crate-type") + 1]
    work = Path(tempfile.mkdtemp(prefix=f"{name}-{kind}-", dir=probe))
    summary = work / "strace-summary"
    timing = work / "time"
    trace = work / "strace-detail"
    env = dict(os.environ, LC_ALL="C", TMPDIR=str(work))
    measured = call(
        [TIME, "-v", "-o", timing, "strace", "-f", "-c",
         "--summary-columns=name,calls,errors", "-o", summary, *argv], env=env,
    )
    if measured.returncode:
        return measured.returncode

    replay = argv.copy()
    # Cargo's dependencies stay in place. Only this invocation's outputs move.
    out = replay.index("--out-dir") + 1
    with tempfile.TemporaryDirectory(prefix="detail-", dir=work) as scratch:
        replay[out] = scratch
        # Only the primary invocation may send Cargo compiler artifact events.
        with (work / "detail.log").open("wb") as log:
            detail = call(
                ["strace", "-f", "-ttt", "-yy", "-s", "0", "-e", "trace=" + ",".join(DETAIL_CALLS),
                 "-o", trace, *replay], env=dict(env, TMPDIR=scratch), stdout=log, stderr=log,
            )
        if detail.returncode:
            print(f"{name} detail replay failed: {work / 'detail.log'}", file=sys.stderr)
            return detail.returncode
        resources = trace_details(trace, scratch)

    stats = timing.read_text()
    rss = re.search(r"Maximum resident set size \(kbytes\): (\d+)", stats)
    wall = re.search(r"Elapsed \(wall clock\) time \(h:mm:ss or m:ss\): ([\d:.]+)", stats)
    seconds = 0.0
    for field in wall[1].split(":"):
        seconds = seconds * 60 + float(field)
    calls = {
        row.split()[0]: int(row.split()[1])
        for row in summary.read_text().splitlines()
        if re.fullmatch(r"\w+\s+\d+(?:\s+\d+)?\s*", row) and not row.startswith("total")
    }
    result = {
        "kind": kind,
        "peak_rss_kib": int(rss[1]),
        "wall_seconds": seconds,
        **resources,
        "syscall_families": {
            family: {call_name: calls[call_name] for call_name in members.split() if call_name in calls}
            for family, members in FAMILIES.items()
        },
        "syscalls": calls,
        "time_sha256": sha256(timing),
        "syscall_summary_sha256": sha256(summary),
        "detail_trace_sha256": sha256(trace),
    }

    if name in PACKAGES:
        trials = {STACK_MAX: 0}

        def compiles(limit):
            with tempfile.TemporaryDirectory(prefix="stack-", dir=work) as scratch:
                replay[out] = scratch
                log = work / f"stack-{limit}.log"
                with log.open("wb") as stream:
                    trial = call(replay, stack=limit, env=dict(env, TMPDIR=scratch),
                                 stdout=stream, stderr=stream)
                trials[limit] = trial.returncode
                if trial.returncode and abs(trial.returncode) not in (6, 11):
                    raise RuntimeError(f"{name} at {limit} KiB failed without a stack fault: {log}")
                return trial.returncode == 0

        low, high = STACK_MIN, STACK_MAX
        if compiles(low):
            high = low
        else:
            while high - low > 1:
                mid = (low + high) // 2
                if compiles(mid):
                    high = mid
                else:
                    low = mid
        result.update(min_stack_kib=high, stack_exit_status=trials)

    (work / "unit.json").write_text(json.dumps({"crate": name, **result}) + "\n")
    return 0


def capture(scratch, workspace_commit, prediction, started):
    if sys.platform != "linux" or os.uname().machine != "x86_64":
        raise SystemExit("X0.9 requires the x86-64 Linux build container")
    scratch = scratch.resolve()
    scratch.mkdir(exist_ok=True)
    if (scratch / "target").exists():
        raise SystemExit("X0.9 requires a fresh Cargo target directory")
    units = scratch / "rustc"
    units.mkdir()
    cargo = ["cargo", f"+{CHANNEL}"]
    build = [*cargo, "build", "--offline", "--locked"]
    rustc = Path(subprocess.check_output(
        ["rustup", "which", "--toolchain", CHANNEL, "rustc"], text=True,
    ).strip())
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    library = ROOT / "rust-fork/library"
    fork = tomllib.loads((ROOT / "rust-fork.toml").read_text())["commit"]
    libc_target = scratch / "libc"
    target_json = ROOT / f"{TARGET}.json"
    libc = libc_target / TARGET / "release"
    wrapper = ROOT / "scripts/rust-host/x0-rustc-probe.sh"
    remap = [f"--remap-path-prefix={ROOT}=/breenix", f"--remap-path-prefix={cargo_home}=/cargo"]
    env = dict(os.environ, RUSTC=str(rustc), CARGO_BUILD_JOBS="4",
               CARGO_ENCODED_RUSTFLAGS="\x1f".join(remap))
    for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
        env.pop(name, None)
    subprocess.run([ROOT / "scripts/rust-fork.sh", "verify"], check=True, env=env)
    subprocess.run([*build, "--release", "--target", target_json, "--target-dir", libc_target],
                   check=True, cwd=ROOT / "libs/libbreenix-libc", env=env)
    rustc_version = subprocess.check_output([rustc, "-vV"], text=True).splitlines()
    host = next(line.removeprefix("host: ") for line in rustc_version if line.startswith("host: "))
    llvm_tools = rustc.parent.parent / "lib/rustlib" / host / "bin"
    subprocess.run([sys.executable, ROOT / "scripts/libc-clean-archive.py",
                    "--archive", libc / "libc.a", "--tool-dir", llvm_tools], check=True, env=env)

    # Same soft-float libc.a, fork, build-std features and linker flags as build.sh.
    flags = [*remap, "-L", f"native={libc}", "-C", f"link-arg=-T{ROOT}/userspace/programs/linker.ld",
             "-C", "link-arg=--allow-multiple-definition", "-C", "default-linker-libraries=no",
             "-C", f"metadata=breenix_std_{fork}"]
    command = [*build, "--profile", "selfhost", "--target", str(target_json),
               "--target-dir", str(scratch / "target"), "-Z", "build-std=std,panic_abort",
               "-Z", "build-std-features=compiler-builtins-mem",
               *(arg for name in PACKAGES for arg in ("-p", name))]
    subprocess.run(command, cwd=ROOT / "toolchain", check=True, env=dict(
        env, RUSTC_WRAPPER=str(wrapper), X0_RUSTC_PROBE_DIR=str(units),
        __CARGO_TESTS_ONLY_SRC_ROOT=str(library), CARGO_ENCODED_RUSTFLAGS="\x1f".join(flags),
    ))

    crates, standard = {}, {}
    for path in sorted(units.glob("*/unit.json")):
        unit = json.loads(path.read_text())
        name = unit.pop("crate")
        group = crates if name in PACKAGES else standard
        group.setdefault(name, []).append(unit)
    if set(crates) != set(PACKAGES) or "std" not in standard:
        raise RuntimeError("Cargo did not produce a complete X0.9 capture")

    def public_path(arg):
        return arg.replace(str(ROOT), "/breenix").replace(str(scratch), "/probe").replace(
            str(cargo_home), "/cargo")

    receipt = {
        **prediction,
        "time": started.isoformat(),
        "executor": "linux-build-container",
        "workspace_commit": workspace_commit,
        "manifest_sha256": sha256(ROOT / "toolchain/Cargo.toml"),
        "target": TARGET,
        "profile": "selfhost",
        "toolchain": CHANNEL,
        "rust_library_commit": fork,
        "rustc_version": rustc_version,
        "rustc_sha256": sha256(rustc),
        "cargo_version": subprocess.check_output([*cargo, "--version"], text=True).strip(),
        "command": [public_path(arg) for arg in command],
        "rustflags": [public_path(arg) for arg in flags],
        "rustc_wrapper": public_path(str(wrapper)),
        "measurement": {
            "time": "GNU time -v around strace -f -c and rustc, with tracing overhead",
            "threads": "Peak thread count in rustc from clone/clone3 and exit events in the detail replay",
            "mappings": "Largest successful mmap requests in rustc and descendants in the detail replay",
            "scratch": "Successful write-family bytes plus writable shared mmap lengths for the replay's private output and temporary files",
            "stack": "Lowest passing soft RLIMIT_STACK within 64–65536 KiB at 1 KiB resolution; 64 is the search floor",
        },
        "crates": crates,
        "std_information_only": standard,
    }
    (scratch / "capture.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    args = sys.argv[1:]
    if "--crate-name" in args and "--out-dir" in args:
        sys.exit(compile_unit(args, os.environ["X0_RUSTC_PROBE_DIR"]))
    # Cargo also uses the wrapper for rustc version and target queries.
    os.execv(args[0], args)
