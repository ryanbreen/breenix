#!/usr/bin/env python3
"""Run the R240 workload series or its native macOS references."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import time

RUNS = 5
MIB = 1024 * 1024
ROOT = Path(__file__).resolve().parents[2]


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def inputs(args):
    if args.host:
        if not args.source or not args.data:
            raise ValueError("remote runs require --source and --data")
        source = Path(args.source)
        data = Path(args.data)
        remote = ["/usr/bin/ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10",
                  "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=8", args.host]
        return source, data, remote
    if not args.fixture or not args.linker or not args.driver:
        raise ValueError("native runs require --fixture, --linker and --driver")
    return ROOT, args.out / "data", []


def main(args):
    if args.host and args.mac_ssh:
        raise ValueError("choose Breenix SSH or the macOS loopback reference")
    args.out = args.out.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    source, data, remote = inputs(args)
    reports = Path("/var/lib/bbench") if remote else args.out
    data = data / args.run

    def run(words, *, log=None, check=True, capture=True, **kwargs):
        command = remote + [shlex.join(map(str, words))] if remote else list(map(str, words))
        return subprocess.run(
            command, check=check, stdout=subprocess.PIPE if capture else None,
            stderr=log, **kwargs,
        )

    if remote:
        run(["/bin/mkdir", "-p", data])
        current = run(["/bin/cat", "/opt/breenix/rust/CURRENT"]).stdout.decode().strip()
        fixture = Path("/opt/breenix/rust") / current
        linker = fixture / "bin/bld"
        if args.driver:
            driver = Path(args.driver)
        else:
            names = run(["/bin/ls", fixture / "lib"]).stdout.decode().splitlines()
            [driver_name] = [name for name in names
                             if name.startswith("librustc_driver-") and name.endswith(".so")]
            driver = fixture / "lib" / driver_name
    else:
        data.mkdir(parents=True, exist_ok=True)
        fixture = args.fixture.resolve()
        linker = args.linker.resolve()
        driver = Path(args.driver).resolve()
    (args.out / "toolchain.txt").write_bytes(
        run([fixture / "bin/rustc", "-vV"], check=False).stdout
        if not remote else (current + "\n").encode()
    )

    rustc = str(fixture / "bin/rustc")
    compiler = [args.bcargo, rustc] if args.wrap_rustc else [rustc]
    target = str(fixture / "targets/x86_64-breenix.json")
    sysroot = str(fixture / "sysroot")
    env = os.environ.copy()
    if not remote:
        cargo_home = data / "cargo-home"
        cargo_home.mkdir(exist_ok=True)
        lib = fixture / "lib"
        env.update(
            CARGO_HOME=str(cargo_home), RUSTC=rustc,
            RUST_TARGET_PATH=str(fixture / "targets"), CARGO_BUILD_JOBS="4",
            CARGO_TERM_COLOR="never",
            CARGO_ENCODED_RUSTFLAGS="\x1f".join([
                f"--sysroot={sysroot}", f"-Clinker={linker}",
                f"-Clink-arg=-T{lib / 'linker.ld'}", f"-Lnative={lib}",
                "-Clink-arg=--allow-multiple-definition", "-Cdefault-linker-libraries=no",
                f"--remap-path-prefix={source}=/breenix-src", "-Dwarnings",
            ]),
        )
        for key in ("RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            env.pop(key, None)

    def cargo(workspace, package, profile, output):
        recipe = [args.bcargo] if remote else [fixture / "bin/cargo"]
        command = [*recipe, "build", "--offline", "--frozen", "--profile", profile,
                   "-p", package, "--target-dir", output]
        if not remote:
            command += ["--target", target]
        return ["command", source / workspace, *command]

    noop = cargo("userland", "yes", "dev-box", data / "noop-artifacts")
    cases = {
        "rustc-version": lambda _: ["command", data, *compiler, "-vV"],
        "hello-object": lambda sample: [
            "command", source / "toolchain/rust-host/k5/corpus", *compiler,
            "--edition=2021", "--sysroot", sysroot, "-Dwarnings", "--emit=obj",
            "--target", target, "--crate-type=lib", "hello.rs", "-o", data / f"{sample}.o",
        ],
        "cargo-noop": lambda _: noop,
        "yes-dev-box": lambda sample: cargo("userland", "yes", "dev-box", data / sample),
        "yes-release": lambda sample: cargo("userland", "yes", "release", data / sample),
        **{name: lambda sample, package=name: cargo(
            "toolchain", package, "selfhost", data / sample
        ) for name in ("bobj", "bld", "bmake", "bcc")},
        "fork-exec": lambda _: ["fork-exec", Path(args.probe).with_name("bbench-exit"), 100],
        "anonymous": lambda _: ["anonymous"],
        "file-map": lambda _: ["file-map", driver],
        "copy": lambda _: ["copy"],
        "sequential": lambda _: ["sequential", 1024 * MIB],
        "small-files": lambda _: ["small-files", 10_000],
        "walk": lambda _: ["walk", 10_000],
        "fsync": lambda _: ["fsync", 32],
    }
    selected = args.cases.split(",") if args.cases else list(cases)
    unknown = set(selected) - cases.keys() - {"network"}
    if unknown:
        raise ValueError(f"unknown cases: {', '.join(sorted(unknown))}")
    failures = 0

    def probe(sample, words):
        nonlocal failures
        receipt = args.out / f"{sample}.json"
        if receipt.exists():
            failures += int(bool(json.loads(receipt.read_bytes())["error"]))
            return
        print(sample, flush=True)
        with (args.out / f"{sample}.log").open("ab") as log:
            probe_env = ["/bin/env", f"RUST_TARGET_PATH={fixture / 'targets'}"] if remote else []
            result = run([*probe_env, args.probe, data, reports, sample, *words],
                         log=log, check=False, env=env)
        if not remote:
            # The native probe writes directly to the local receipt directory.
            if not receipt.exists() or receipt.read_bytes() != result.stdout:
                raise RuntimeError(f"native receipt missing or different for {sample}")
        else:
            value = json.loads(result.stdout)
            if value["sample"] != sample or bool(value["error"]) != bool(result.returncode):
                raise RuntimeError(f"invalid remote receipt for {sample}")
            receipt.write_bytes(result.stdout)
        if result.returncode:
            failures += 1
            print(f"{sample} failed, see its JSON and log", flush=True)

    for case in selected:
        if case == "network":
            if not remote and not args.mac_ssh:
                raise ValueError("native network reference requires --mac-ssh localhost")
            network_series(args, data, run)
            continue
        if case == "cargo-noop":
            seed = f"{args.run}-noop-seed"
            probe(seed, noop)
            if json.loads((args.out / f"{seed}.json").read_bytes())["error"]:
                continue
        for number in range(1, (1 if case == "bcc" else RUNS) + 1):
            sample = f"{args.run}-{case}-{number}"
            words = cases[case](sample)
            # Cargo's target directory must differ from the probe's scratch directory.
            if case in ("yes-dev-box", "yes-release", "bobj", "bld", "bmake", "bcc"):
                words = cases[case](f"{sample}-artifacts")
            probe(sample, words)
    return int(failures != 0)


def network_series(args, data, run):
    local = args.out / "network"
    local.mkdir(exist_ok=True)
    payload = local / "payload"
    if not payload.exists():
        with payload.open("xb") as file:
            for _ in range(100):
                file.write(os.urandom(1_000_000))
    digest = sha256(payload)
    run(["/bin/mkdir", "-p", data / "network"])
    host = args.host or args.mac_ssh
    ssh_options = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10",
                   "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=8"]

    def counters():
        if not args.host:
            return {}
        values = {}
        for path in ("/proc/stat", "/proc/block/cache", "/proc/uptime"):
            result = run(["/bin/cat", path], check=False, log=subprocess.DEVNULL)
            if result.returncode == 0:
                values[path] = result.stdout.decode()
        trace = run(["/usr/bin/sudo", "-n", "/bin/cat", "/proc/trace/counters"],
                    check=False, log=subprocess.DEVNULL)
        if trace.returncode == 0:
            values["/proc/trace/counters"] = trace.stdout.decode()
        return values

    for number in range(1, RUNS + 1):
        destination = data / "network" / str(number)
        run(["/bin/mkdir", "-p", destination])
        remote_file = f"{host}:{destination}/payload"
        for case in ("upload", "download", "ssh"):
            sample = f"{args.run}-{case}-{number}"
            receipt = args.out / f"{sample}.json"
            if receipt.exists():
                if json.loads(receipt.read_bytes())["error"]:
                    raise RuntimeError(f"{sample} failed in this series. Use a new run ID.")
                continue
            if case == "upload":
                existing = run(["/bin/test", "-e", destination / "payload"], check=False)
                if existing.returncode != 1:
                    raise RuntimeError(f"{sample} needs a fresh destination. Use a new run ID.")
            print(sample, flush=True)
            before = counters()
            returned = args.out / f"{sample}.download"
            upload = ([args.bsync, str(local), f"{host}:{destination}"] if args.host
                      else ["/usr/bin/scp", *ssh_options, str(payload), remote_file])
            commands = {
                "upload": upload,
                "download": ["/usr/bin/scp", *ssh_options, remote_file, str(returned)],
                "ssh": ["/usr/bin/ssh", *ssh_options, host, "/usr/bin/true" if not args.host else "/bin/true"],
            }
            start = time.monotonic_ns()
            with (args.out / f"{sample}.log").open("wb") as log:
                process = subprocess.run(commands[case], stdout=log, stderr=log)
            elapsed = time.monotonic_ns() - start
            after = counters()
            error = f"exit status: {process.returncode}" if process.returncode else None
            metrics = {}
            if case != "ssh" and not error:
                if case == "download":
                    actual = sha256(returned)
                elif args.host:
                    actual = run(["/bin/sha256sum", destination / "payload"]).stdout.decode().split()[0]
                else:
                    actual = sha256(destination / "payload")
                if actual != digest:
                    error = "payload SHA-256 mismatch"
                metrics = {"bytes": payload.stat().st_size, "sha256": actual}
            returned.unlink(missing_ok=True)
            record = {"sample": sample, "argv": commands[case], "wall_ns": elapsed, "before": before,
                      "after": after, "metrics": metrics, "error": error}
            encoded = (json.dumps(record) + "\n").encode()
            if args.host:
                run(["/bin/bput", f"/var/lib/bbench/{sample}.json", len(encoded),
                     hashlib.sha256(encoded).hexdigest(), "644"], input=encoded)
            receipt.write_bytes(encoded)
            if error:
                raise RuntimeError(f"{sample}: {error}")
        run(["/bin/rm", "-f", destination / "payload"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", required=True, help="unique run prefix, letters/digits/hyphens")
    parser.add_argument("--out", required=True, type=Path, help="local receipt and log directory")
    parser.add_argument("--probe", required=True, help="absolute path to bbench")
    parser.add_argument("--cases", help="comma-separated case names")
    parser.add_argument("--host", help="Breenix SSH login or alias")
    parser.add_argument("--mac-ssh", choices=["localhost"], help="macOS loopback SSH reference")
    parser.add_argument("--source", help="absolute path to the synced source tree on Breenix")
    parser.add_argument("--data", help="absolute path to the scratch directory on Breenix")
    parser.add_argument("--bcargo", help="absolute path to bcargo on Breenix")
    parser.add_argument("--wrap-rustc", action="store_true",
                        help="use the PR 3 bcargo loader workaround for direct rustc probes")
    parser.add_argument("--fixture", type=Path, help="Mac E25 twin fixture")
    parser.add_argument("--linker", type=Path, help="Mac E25 bld")
    parser.add_argument("--driver", help="absolute path to the matching Breenix driver ELF")
    parser.add_argument("--bsync", default=shutil.which("bsync"), help="Mac bsync executable")
    args = parser.parse_args()
    if not args.run or not all(c.isascii() and (c.isalnum() or c == "-") for c in args.run):
        parser.error("invalid --run")
    if args.host and not args.bcargo:
        parser.error("remote runs require --bcargo")
    if args.wrap_rustc and not args.host:
        parser.error("--wrap-rustc requires a Breenix endpoint")
    raise SystemExit(main(args))
