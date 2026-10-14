"""Execute production TTL parsing and copy-length helpers on the host."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / "kernel/src/socket/opts.rs").read_text()
def function(name):
    start = source.index("fn " + name + "(")
    opening = source.index("{", start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end]
code = "const IPPROTO_IP:i32=0; const IP_TTL:i32=2; const SOL_SOCKET:i32=1; const IPPROTO_TCP:i32=6; const SO_RCVTIMEO:i32=20; const SO_SNDTIMEO:i32=21; const SO_LINGER:i32=13; const EINVAL:i32=22; const DEFAULT_TTL:u8=64;\n"
code += "\n".join(function(name) for name in ("value_len", "result_len", "ttl_value", "int_value"))
code += r'''
fn main() {
    for room in 0..16 {
        let expected = if room < 4 { room.min(1) } else { 4 };
        assert_eq!(value_len(IPPROTO_IP, IP_TTL, room), expected);
        assert_eq!(result_len(IPPROTO_IP, IP_TTL, room, 4), expected);
        assert_eq!(result_len(SOL_SOCKET, 2, room, 4), room.min(4));
    }
    assert_eq!(ttl_value(&[42]), Ok(42));
    assert_eq!(ttl_value(&(-1i32).to_ne_bytes()), Ok(64));
    assert_eq!(ttl_value(&[0]), Err(22));
    assert_eq!(ttl_value(&256i32.to_ne_bytes()), Err(22));
}
'''
with tempfile.TemporaryDirectory() as tmp:
    path = Path(tmp)
    (path / "ttl.rs").write_text(code)
    subprocess.run(["rustc", "--edition=2021", "-Dwarnings", str(path / "ttl.rs"), "-o", str(path / "ttl")], check=True)
    subprocess.run([str(path / "ttl")], check=True)
print("TTL parsing and copy lengths passed")
