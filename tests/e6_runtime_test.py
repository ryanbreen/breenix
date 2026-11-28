"""Verify a staged E6 runtime with host instruction emulation, without a boot.

Requires Unicorn and pyelftools. Pass the staging parent (x86_64 and aarch64
subdirectories), or use --headers-only to check the staged header contract
with the source headers and the stage's generation selector.
"""
import argparse
from io import BytesIO
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "tests/e6_runtime"
sys.path.insert(0, str(ROOT / "scripts"))
from userspace_generation import verify_inputs

PAGE = 4096
STACK = 0x80000000
LOAD_BIAS = 0x40000000
LOADER_BIAS = 0x60000000


def command(*args, **kwargs):
    return subprocess.check_output(args, cwd=ROOT, text=True, **kwargs)


def header_flags(clang, include):
    resource = Path(command(clang, "-print-resource-dir").strip()) / "include"
    return ["-nostdinc", "-isystem", include, "-isystem", resource]


def verify_headers(clang, include, *flags):
    for headers in (("limits.h", "pthread.h"), ("pthread.h", "limits.h"), ("limits.h",)):
        source = "#define _GNU_SOURCE\n" + "".join(f"#include <{name}>\n" for name in headers)
        expansion = command(clang, "-E", "-P", "-Werror", *header_flags(clang, include), *flags,
                            "-x", "c", "-", input=source + "\nPTHREAD_STACK_MIN\n")
        assert "sysconf" in expansion.strip().splitlines()[-1], "PTHREAD_STACK_MIN is still fixed"


def image(data):
    from elftools.elf.elffile import ELFFile
    return ELFFile(BytesIO(data))


def symbol(elf, name):
    table = elf.get_section_by_name(".symtab")
    found = table.get_symbol_by_name(name) if table else None
    assert found, f"missing {name} in link output"
    return found[0]["st_value"]


class Process:
    """The narrow syscall model used by the CRT, loader, and static linker probes."""
    def __init__(self, arch, files):
        from unicorn import Uc, UC_ARCH_ARM64, UC_ARCH_X86, UC_HOOK_INSN, UC_HOOK_INTR
        from unicorn import UC_MODE_ARM, UC_MODE_64
        from unicorn import arm64_const as arm, x86_const as x86

        self.arm = arch == "aarch64"
        self.files = {key: bytearray(value) for key, value in files.items()}
        self.fds = {}
        self.pages = set()
        self.next_map = 0x100000000
        self.status = None
        self.diagnostics = bytearray()
        self.cpu = Uc(UC_ARCH_ARM64, UC_MODE_ARM) if self.arm else Uc(UC_ARCH_X86, UC_MODE_64)
        if self.arm:
            self.cpu.hook_add(UC_HOOK_INTR, lambda cpu, trap, _: self.syscall())
            self.nr, self.sp, self.ret = arm.UC_ARM64_REG_X8, arm.UC_ARM64_REG_SP, arm.UC_ARM64_REG_X0
            self.args = [getattr(arm, f"UC_ARM64_REG_X{i}") for i in range(6)]
            self.fs = None
        else:
            self.cpu.hook_add(UC_HOOK_INSN, lambda cpu, _: self.syscall(), None, 1, 0, x86.UC_X86_INS_SYSCALL)
            self.nr, self.sp, self.ret = x86.UC_X86_REG_RAX, x86.UC_X86_REG_RSP, x86.UC_X86_REG_RAX
            self.args = [getattr(x86, f"UC_X86_REG_{reg}") for reg in ("RDI", "RSI", "RDX", "R10", "R8", "R9")]
            self.fs = x86.UC_X86_REG_FS_BASE

    def map(self, address, length):
        for page in range(address & -PAGE, (address + length + PAGE - 1) & -PAGE, PAGE):
            if page not in self.pages:
                self.pages.add(page)
                self.cpu.mem_map(page, PAGE)

    def load(self, elf, bias):
        for segment in elf.iter_segments(type="PT_LOAD"):
            address = bias + segment["p_vaddr"]
            self.map(address, segment["p_memsz"])
            if segment["p_filesz"]:
                self.cpu.mem_write(address, segment.data())
        return bias + elf.header["e_entry"]

    def string(self, address):
        result = bytearray()
        while byte := self.cpu.mem_read(address + len(result), 1)[0]:
            result.append(byte)
        return result.decode()

    def syscall(self):
        numbers = ({
            17: "getcwd", 25: "fcntl", 29: "ioctl", 56: "openat", 57: "close",
            62: "seek", 63: "read", 64: "write", 66: "writev", 67: "pread",
            78: "readlinkat", 79: "statat", 80: "fstat", 93: "exit", 94: "exit",
            96: "tid", 135: "mask", 172: "tid", 178: "tid", 215: "unmap",
            222: "map", 226: "protect", 261: "limit", 278: "random",
        } if self.arm else {
            0: "read", 1: "write", 2: "open", 3: "close", 5: "fstat",
            8: "seek", 9: "map", 10: "protect", 11: "unmap", 14: "mask",
            16: "ioctl", 17: "pread", 20: "writev", 39: "tid", 60: "exit",
            72: "fcntl", 79: "getcwd", 89: "readlink", 158: "arch_prctl",
            186: "tid", 218: "tid", 231: "exit", 257: "openat", 262: "statat",
            267: "readlinkat", 302: "limit", 318: "random",
        })
        nr = self.cpu.reg_read(self.nr)
        assert nr in numbers, f"unexpected {'ARM64' if self.arm else 'x86-64'} syscall {nr}"
        op = numbers[nr]
        args = [self.cpu.reg_read(reg) for reg in self.args]
        self.cpu.reg_write(self.ret, self.handle(op, args) % (1 << 64))

    def handle(self, op, args):
        a, b, c, d, e, f = args
        write = self.cpu.mem_write
        if op == "exit":
            self.status = a
            self.cpu.emu_stop()
        elif op == "tid":
            return 1
        elif op == "arch_prctl":
            assert a == 0x1002
            self.cpu.reg_write(self.fs, b)
        elif op == "limit":
            return -38
        elif op == "mask" and c:
            write(c, bytes(8))
        elif op == "random":
            write(a, b"R" * b)
            return b
        elif op == "getcwd":
            write(a, b"/\0")
            return 2
        elif op in ("open", "openat"):
            path, flags = (self.string(a), b) if op == "open" else (self.string(b), c)
            if flags & 0x40:
                self.files[path] = bytearray()
            if path not in self.files:
                return -2
            fd = next(fd for fd in range(3, 1024) if fd not in self.fds)
            self.fds[fd] = (path, 0)
            return fd
        elif op == "close":
            self.fds.pop(a, None)
        elif op in ("readlink", "readlinkat"):
            name, out, size = (a, b, c) if op == "readlink" else (b, c, d)
            if self.string(name) != "/proc/self/exe":
                return -2
            value = b"/probe"[:size]
            write(out, value)
            return len(value)
        elif op in ("fstat", "statat"):
            path, out = (self.fds[a][0], b) if op == "fstat" else (self.string(b), c)
            if path not in self.files:
                return -2
            inode = list(self.files).index(path) + 1
            stat = bytearray(128 if self.arm else 144)
            struct.pack_into("<QQ", stat, 0, 1, inode)
            struct.pack_into("<I", stat, 16 if self.arm else 24, 0o100755)
            struct.pack_into("<q", stat, 48, len(self.files[path]))
            write(out, bytes(stat))
        elif op in ("write", "writev"):
            chunks = [(b, c)] if op == "write" else [
                struct.unpack("<QQ", self.cpu.mem_read(b + 16*i, 16)) for i in range(c)]
            value = b"".join(self.cpu.mem_read(ptr, size) for ptr, size in chunks if size)
            if a in (1, 2):
                self.diagnostics.extend(value)
            else:
                path, pos = self.fds[a]
                self.files[path][pos:pos + len(value)] = value
                self.fds[a] = (path, pos + len(value))
            return len(value)
        elif op in ("read", "pread", "seek"):
            path, pos = self.fds[a]
            if op == "seek":
                pos = (0, pos, len(self.files[path]))[c] + struct.unpack("<q", struct.pack("<Q", b))[0]
                self.fds[a] = (path, pos)
                return pos
            if op == "pread":
                pos = d
            value = self.files[path][pos:pos + c]
            if value:
                write(b, bytes(value))
            if op == "read":
                self.fds[a] = (path, pos + len(value))
            return len(value)
        elif op == "map":
            base = a if d & 0x10 else self.next_map
            length = (b + PAGE - 1) & -PAGE
            if not d & 0x10:
                self.next_map += length
            self.map(base, length)
            value = b"" if d & 0x20 else bytes(self.files[self.fds[e][0]][f:f + length])
            write(base, value.ljust(length, b"\0"))
            return base
        elif op == "protect":
            self.cpu.mem_protect(a, (b + PAGE - 1) & -PAGE, c)
        elif op == "unmap":
            length = (b + PAGE - 1) & -PAGE
            self.cpu.mem_unmap(a, length)
            self.pages.difference_update(range(a, a + length, PAGE))
        elif op in ("ioctl", "fcntl"):
            return -25
        return 0

    def run(self, argv, capabilities=(), interpreter=False):
        elf = image(self.files["/probe"])
        bias = LOAD_BIAS if interpreter else 0
        entry = self.load(elf, bias)
        phoff = elf.header["e_phoff"]
        phdr = next(bias + seg["p_vaddr"] + phoff for seg in elf.iter_segments(type="PT_LOAD")
                    if seg["p_offset"] == 0)
        auxv = [(3, phdr), (4, 56), (5, elf.header["e_phnum"]), (6, PAGE), (9, entry)]
        if interpreter:
            entry = self.load(image(self.files["/lib/ld-breenix.so"]), LOADER_BIAS)
            auxv.extend(((7, LOADER_BIAS), (0x60000000, bias)))
        self.map(STACK - 0x200000, 0x200000)
        pointers = []
        address = STACK - PAGE
        for arg in argv:
            pointers.append(address)
            value = arg.encode() + b"\0"
            self.cpu.mem_write(address, value)
            address += len(value)
        words = [len(argv), *pointers, 0, 0]
        for kind, value in [*auxv, *capabilities, (0, 0)]:
            words.extend((kind, value))
        sp = (STACK - PAGE - 8 * len(words)) & -16
        self.cpu.mem_write(sp, struct.pack(f"<{len(words)}Q", *words))
        self.cpu.reg_write(self.sp, sp)
        self.cpu.emu_start(entry, 0, count=80_000_000)
        assert self.status == 0, f"{argv}: exit {self.status}: {self.diagnostics.decode(errors='replace')}"


def compile_case(clang, arch, include, out, name, source, *flags):
    command(clang, f"--target={arch}-unknown-none-elf", *header_flags(clang, include),
            "-ffreestanding", "-fno-stack-protector", "-O2", "-Wall", "-Wextra", "-Werror",
            *flags, "-c", SOURCE / source, "-o", out / name)
    return out / name


def copy_trace(process, libc, base):
    from unicorn import UC_HOOK_CODE
    observed = []
    names = ("memcpy",) if process.arm else ("breenix_copy_words", "breenix_copy_rep")
    for name in names:
        address = base + symbol(libc, name)
        def record(cpu, pc, size, name):
            if cpu.reg_read(process.args[2]) == 23:
                observed.append(name)
        process.cpu.hook_add(UC_HOOK_CODE, record, name, address, address)
    return observed


def check_startup(proc, interpreter=False):
    fixture = image(proc.files["/lib/libc.so" if interpreter else "/probe"])
    for mode, caps in (("absent", []), ("zero", [(16, 0), (26, 0)]),
                       ("enabled", [(16, 2 if proc.arm else 1), (26, 0 if proc.arm else 1),
                                    (51, 20000)])):
        case = Process("aarch64" if proc.arm else "x86_64", proc.files)
        if interpreter:
            # The shared object reserves its PT_LOAD span with anonymous
            # PROT_NONE, then maps its file at that reservation using MAP_FIXED.
            traces = []
            handle = case.handle
            def mapped(op, args):
                address = handle(op, args)
                _, _, _, flags, fd, offset = args
                if op == "map" and flags & 0x10 and not flags & 0x20 and offset == 0:
                    if case.fds[fd][0] == "/lib/libc.so":
                        traces.append(copy_trace(case, fixture, args[0]))
                return address
            case.handle = mapped
        else:
            trace = copy_trace(case, fixture, 0)
        case.run(["/probe", mode], caps, interpreter)
        expected = "memcpy" if case.arm else ("breenix_copy_rep" if mode == "enabled" else "breenix_copy_words")
        if interpreter:
            assert traces == [[expected, expected]], (mode, traces)
        else:
            assert trace == [expected], (mode, trace)


def verify_artifacts(root, work, clang, lld, readelf, arch):
    ordinary = f"{arch}-breenix"
    hosted = f"{ordinary}-hosted"
    native = root / "sysroot/usr"
    lib = native / "lib"
    include = native / "include"
    static = [lib / name for name in ("libc.a", "linker-hosted.ld")]
    shared = [lib / name for name in ("libc.so", "ld-breenix.so", "breenix-crt.o")]
    bld = native / "bin/bld"
    verify_inputs(root, ordinary, [*static, bld])
    if arch == "x86_64":
        verify_inputs(root, f"{ordinary}-c", static)
    verify_inputs(root, hosted, shared)
    verify_headers(clang, include)

    obj = compile_case(clang, arch, include, work, "startup.o", "startup.c")
    ref = work / "reference"
    link = ["-T", static[1], "--gc-sections", "-static", obj, static[0]]
    command(lld, "--fatal-warnings", *link, "-o", ref)

    # Execute the actual rebuilt guest bld, with the same ordered input
    # files as the pinned reference linker. Execute both products below.
    files = {"/probe": bld.read_bytes(), "/linker.ld": static[1].read_bytes(),
             "/startup.o": obj.read_bytes(), "/libc.a": static[0].read_bytes()}
    guest = Process(arch, files)
    guest.run(["/probe", "-T", "/linker.ld", "--gc-sections", "-static",
               "/startup.o", "/libc.a", "-o", "/out"])
    for data in (ref.read_bytes(), guest.files["/out"]):
        assert image(data).header["e_type"] == "ET_EXEC"
        assert not any(True for _ in image(data).iter_segments(type="PT_INTERP"))
        check_startup(Process(arch, {"/probe": data}))

    dynamic = compile_case(clang, arch, include, work, "dynamic.o", "startup.c", "-fPIC", "-DDYNAMIC")
    dependency = compile_case(clang, arch, include, work, "dep.o", "consumer.c", "-fPIC")
    late = compile_case(clang, arch, include, work, "late.o", "consumer.c", "-fPIC", "-DLATE")
    for name, source in (("dep.so", dependency), ("late.so", late)):
        command(lld, "--fatal-warnings", "--hash-style=sysv", "-shared", f"-soname={name}",
                source, "-o", work / name)
    exe = work / "dynamic"
    command(lld, "--fatal-warnings", "--hash-style=sysv", "-pie", "-E",
            "--dynamic-linker=/lib/ld-breenix.so", "-rpath", "/lib",
            "-o", exe, shared[2], dynamic, shared[0], work / "dep.so")
    for artifact in [exe, *shared[:2], ref]:
        info = command(readelf, "-r", artifact)
        assert "IRELATIVE" not in info, artifact
        assert not re.search(r"\bIFUNC\b", command(readelf, "-sW", artifact)), artifact
    files = {"/probe": exe.read_bytes()}
    files.update({f"/lib/{path.name}": path.read_bytes() for path in (*shared[:2], work / "dep.so", work / "late.so")})
    check_startup(Process(arch, files), interpreter=True)
    print(f"{arch}: both static links and interpreted startup passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path, nargs="?")
    parser.add_argument("--headers-only", action="store_true")
    args = parser.parse_args()
    clang = command(ROOT / "scripts/breenix-cc", "--arch", "aarch64", "--tool-dir").strip() + "/clang"
    if args.headers_only:
        verify_headers(clang, ROOT / "libs/libbreenix-libc/include", "-D__BREENIX_E6=1")
        return
    if args.root is None:
        parser.error("the staged runtime parent is required")
    host = re.search(r"^host: (.+)$", command("rustc", "-vV"), re.M)[1]
    tools = Path(command("rustc", "--print", "sysroot").strip()) / "lib/rustlib" / host / "bin"
    for arch in ("x86_64", "aarch64"):
        root = args.root.resolve() / arch
        with tempfile.TemporaryDirectory(prefix="e6-runtime-", dir=root.parent) as directory:
            verify_artifacts(root, Path(directory), clang, tools / "gcc-ld/ld.lld",
                             tools / "llvm-readobj", arch)


if __name__ == "__main__":
    main()
