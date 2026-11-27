use crate::sftp;
use crate::tree::{Action, Entry, Tree};
use crate::{normal_relative, parent, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

const GUARD: &str = r#"
export LC_ALL=C
unset CDPATH
fail() { printf 'bsync: %s\n' "$*" >&2; exit 1; }

# Walk without following links. The final physical path check also catches a
# directory swapped for a link between the test and cd. Writers then use
# single-component names relative to this working directory.
enter() {
    cd -P / || fail "cannot open /"
    rest=${1#/}
    while [ -n "$rest" ]; do
        part=${rest%%/*}
        [ ! -L "./$part" ] || fail "symlink in directory: $1"
        cd -P "./$part" || fail "cannot open directory: $1"
        case "$rest" in */*) rest=${rest#*/};; *) rest=;; esac
    done
    [ "$(pwd -P)" = "$1" ] || fail "directory moved or resolved outside path: $1"
}

regular_or_absent() {
    [ ! -L "$1" ] && { [ ! -e "$1" ] || [ -f "$1" ]; } ||
        fail "expected regular file or absent path: $1"
}
"#;

const LIST: &str = r#"
walk() (
    [ -r . ] && [ -x . ] || fail "cannot list directory: $1"
    meta=$(/bin/ls -ldn .) || exit 1
    printf 'd\0%s\0%s\0\0' "$1" "$meta"
    for leaf in ./* ./.[!.]* ./..?*; do
        rel=${1:+$1/}${leaf#./}
        if [ -L "$leaf" ]; then
            printf 'o\0%s\0\0\0' "$rel"
        elif [ -d "$leaf" ]; then
            (cd -P "$leaf" && [ "$(pwd -P)" = "${root%/}/$rel" ] && walk "$rel") ||
                fail "cannot list directory: $rel"
        elif [ -f "$leaf" ]; then
            meta=$(/bin/ls -ldn "$leaf") || exit 1
            digest=$(/bin/sha256sum < "$leaf") || exit 1
            printf 'f\0%s\0%s\0%s\0' "$rel" "$meta" "$digest"
        elif [ -e "$leaf" ]; then
            printf 'o\0%s\0\0\0' "$rel"
        fi
    done
)

case "$root" in
    /*) ;;
    *) here=$(pwd -P) || exit 1; root="${here%/}/$root";;
esac
printf 'bsync1\0%s\0' "$root"
if [ "$root" = / ]; then
    enter "$root"
    walk '' || exit 1
else
    base=${root%/*}
    enter "${base:-/}"
    leaf=./${root##*/}
    [ ! -L "$leaf" ] || fail "symlink destination: $root"
    if [ -e "$leaf" ]; then
        enter "$root"
        walk '' || exit 1
    else
        [ -r . ] && [ -x . ] || fail "cannot verify absent destination: $root"
    fi
fi
printf 'end\0'
"#;

pub fn listing(root: &str) -> String {
    format!("{GUARD}\nroot={}\n{LIST}", quote(root))
}

pub struct Listing {
    pub root: String,
    pub entries: Tree,
}

pub fn parse_listing(bytes: &[u8]) -> Result<Listing> {
    let text = std::str::from_utf8(bytes)?;
    let mut fields = text.split_terminator('\0');
    if !text.ends_with('\0') || fields.next() != Some("bsync1") {
        return Err("invalid remote listing header".into());
    }
    let root = fields.next().ok_or("missing listing root")?;
    if !absolute(root) {
        return Err(format!("invalid remote listing root: {root:?}").into());
    }
    let mut entries = Tree::new();
    while let Some(kind) = fields.next() {
        if kind == "end" {
            if fields.next().is_some() {
                return Err("unexpected data after remote listing".into());
            }
            if !entries.is_empty() && !matches!(entries.get(""), Some(Entry::Dir(_))) {
                return Err("remote listing lacks a destination directory".into());
            }
            for path in entries.keys().filter(|path| !path.is_empty()) {
                if !matches!(entries.get(parent(path)), Some(Entry::Dir(_))) {
                    return Err(format!("remote listing lacks parent of {path:?}").into());
                }
            }
            return Ok(Listing {
                root: root.into(),
                entries,
            });
        }
        let path = fields.next().ok_or("truncated remote listing path")?;
        let meta = fields.next().ok_or("truncated remote listing metadata")?;
        let digest = fields.next().ok_or("truncated remote listing digest")?;
        if !(normal_relative(path) || path.is_empty() && kind == "d") {
            return Err(format!("invalid path in remote listing: {path:?}").into());
        }
        let entry = match kind {
            "o" if meta.is_empty() && digest.is_empty() => Entry::Other,
            "d" if digest.is_empty() => Entry::Dir(parse_mode(meta, b'd')?),
            "f" => {
                let mode = parse_mode(meta, b'-')?;
                let size = meta
                    .split_ascii_whitespace()
                    .nth(4)
                    .ok_or("missing file size")?
                    .parse()?;
                let digest = digest
                    .strip_suffix("  -")
                    .ok_or("invalid sha256sum response")?;
                if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(format!("invalid SHA-256 for {path:?}").into());
                }
                Entry::File {
                    size,
                    mode,
                    digest: digest.to_ascii_lowercase(),
                }
            }
            _ => return Err(format!("invalid record for {path:?}").into()),
        };
        if entries.insert(path.into(), entry).is_some() {
            return Err(format!("duplicate path in remote listing: {path:?}").into());
        }
    }
    Err("missing remote listing terminator".into())
}

fn parse_mode(meta: &str, file_type: u8) -> Result<u32> {
    let token = meta.split_ascii_whitespace().next().unwrap_or("");
    let [actual_type, permissions @ ..] = token.as_bytes() else {
        return Err("missing file mode".into());
    };
    if *actual_type != file_type || permissions.len() != 9 {
        return Err(format!("invalid mode in remote listing: {token:?}").into());
    }
    let mut mode = 0;
    for (i, &bit) in permissions.iter().enumerate() {
        match (i % 3, bit) {
            (_, b'-') => {}
            (0, b'r') | (1, b'w') | (2, b'x') => mode |= 1 << (8 - i),
            (2, b's' | b'S') if i < 8 => {
                mode |= 1 << (11 - i / 3);
                if bit == b's' {
                    mode |= 1 << (8 - i);
                }
            }
            (2, b't' | b'T') if i == 8 => {
                mode |= 0o1000;
                if bit == b't' {
                    mode |= 1;
                }
            }
            _ => return Err(format!("invalid mode in remote listing: {token:?}").into()),
        }
    }
    Ok(mode)
}

fn absolute(path: &str) -> bool {
    path.strip_prefix('/')
        .is_some_and(|relative| relative.is_empty() || normal_relative(relative))
}

fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

pub struct Destination {
    peer: String,
    path: String,
}

impl Destination {
    pub fn parse(operand: &str) -> Result<Self> {
        let (user, rest) = operand.split_once('@').ok_or("expected USER@HOST:DEST")?;
        let (host, path) = if let Some(bracketed) = rest.strip_prefix('[') {
            bracketed.split_once("]:")
        } else {
            rest.split_once(':')
        }
        .ok_or("expected USER@HOST:DEST")?;
        let valid_label = |value: &str, separators: &[u8]| {
            !value.is_empty()
                && !value.starts_with('-')
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || separators.contains(&b))
        };
        if !valid_label(user, b"_.-") || !valid_label(host, b"_.-:%") {
            return Err("invalid SSH user or host".into());
        }
        let path = if path == "/" {
            path
        } else {
            path.trim_end_matches('/')
        };
        if !absolute(path) && !normal_relative(path) {
            return Err(format!("invalid destination path: {path:?}").into());
        }
        Ok(Self {
            peer: format!("{user}@{host}"),
            path: path.into(),
        })
    }

    fn command(&self, script: &str, sudo: bool) -> Command {
        let prefix = if sudo { "/usr/bin/sudo -n " } else { "" };
        let mut command = Command::new("/usr/bin/ssh");
        command.args([
            "-T",
            "--",
            &self.peer,
            &format!("{prefix}/bin/sh -c {}", quote(script)),
        ]);
        command
    }

    pub fn inventory(&self, sudo: bool) -> Result<Listing> {
        let output = self
            .command(&listing(&self.path), sudo)
            .stdin(Stdio::null())
            .output()?;
        if !output.stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
        }
        if !output.status.success() {
            return Err(format!("remote listing failed ({})", output.status).into());
        }
        let listed = parse_listing(&output.stdout)?;
        if if self.path.starts_with('/') {
            self.path != listed.root
        } else {
            !listed.root.ends_with(&format!("/{}", self.path))
        } {
            return Err("remote listing describes a different destination".into());
        }
        Ok(listed)
    }

    pub fn apply(
        &self,
        listed: &Listing,
        source: &Path,
        local: &Tree,
        actions: &[Action],
        sudo: bool,
    ) -> Result<()> {
        for action in actions {
            let relative = action.path();
            let path = if relative.is_empty() {
                listed.root.clone()
            } else {
                format!("{}/{relative}", listed.root.trim_end_matches('/'))
            };
            let (base, basename) = path.rsplit_once('/').ok_or("missing destination parent")?;
            let base = if base.is_empty() { "/" } else { base };
            let name = quote(&format!("./{basename}"));
            let enter_base = format!("{GUARD}\nenter {}\n", quote(base));
            let mut input = Stdio::null();
            let script = match action {
                Action::MakeDir(_) => format!("{enter_base}exec /bin/mkdir -m 700 {name}"),
                Action::RemoveFile(_) => format!(
                    "{enter_base}[ ! -L {name} ] && [ -f {name} ] || fail 'expected a regular file'\nexec /bin/rm {name}"
                ),
                Action::RemoveDir(_) => format!(
                    "{enter_base}[ ! -L {name} ] && [ -d {name} ] || fail 'expected a directory'\nexec /bin/rmdir {name}"
                ),
                Action::DirMode(_) => {
                    let Entry::Dir(mode) = local[relative] else { unreachable!() };
                    format!("{GUARD}\nenter {}\nexec /bin/chmod {mode:o} .", quote(&path))
                }
                Action::Upload(_) | Action::FileMode(_) => {
                    let Entry::File { size, mode, digest } = &local[relative] else { unreachable!() };
                    let bput = format!("/bin/bput {name} {size} {digest} {mode:o}");
                    if matches!(action, Action::FileMode(_)) {
                        format!("{enter_base}[ ! -L {name} ] && [ -f {name} ] || fail 'expected a regular file'\nexec {bput} < {name}")
                    } else {
                        let file = open_source(source, relative)?;
                        if !sudo {
                            self.send_sftp(base, &name, &bput, file)
                                .map_err(|error| format!("upload {relative:?}: {error}"))?;
                            continue;
                        }
                        input = file.into();
                        format!("{enter_base}regular_or_absent {name}\nexec {bput}")
                    }
                }
            };
            let status = self
                .command(&script, sudo)
                .stdin(input)
                .stdout(Stdio::null())
                .status()?;
            if !status.success() {
                return Err(format!("{} {relative:?} failed ({status})", action.label()).into());
            }
        }
        Ok(())
    }

    fn send_sftp(&self, base: &str, name: &str, bput: &str, file: File) -> Result<()> {
        let mut nonce = [0u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut nonce)?;
        let stage = format!(".bsync-{:032x}", u128::from_be_bytes(nonce));
        let script = format!(
            "{GUARD}\nbase={}\nstage=./{stage}\nstage_path={}\n{STAGE}\n\
             enter \"$stage_path\"\n\
             [ -f ./ready ] && [ ! -L ./ready ] && [ -f ./payload ] && [ ! -L ./payload ] || fail 'upload was not completed'\n\
             exec 3< ./payload || exit 1\n\
             enter \"$base\"\n\
             regular_or_absent {name}\n\
             {bput} <&3",
            quote(base),
            quote(&format!("{}/{stage}", base.trim_end_matches('/'))),
        );
        let mut ssh = self
            .command(&script, false)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let sent = sftp::send(
            ssh.stdin.as_mut().unwrap(),
            ssh.stdout.as_mut().unwrap(),
            file,
        );
        // EOF ends the worker. The enclosing shell then publishes the file
        // and returns bput's status after cleaning up the staging directory.
        let output = ssh.wait_with_output()?;
        sent?;
        if !output.status.success() {
            return Err(format!("remote publication failed ({})", output.status).into());
        }
        Ok(())
    }
}

const STAGE: &str = r#"
enter "$base"
umask 077
/bin/mkdir -m 700 "$stage" || exit 1
cleanup() (
    enter "$stage_path"
    /bin/rm -f ./payload ./ready || exit 1
    enter "$base"
    /bin/rmdir "$stage"
)
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
(
    enter "$stage_path"
    exec /usr/sbin/bsshd --sftp-server
) || fail "SFTP worker failed"
"#;

fn open_source(root: &Path, relative: &str) -> Result<File> {
    let mut path = root.to_owned();
    let mut components = relative.split('/').peekable();
    while let Some(name) = components.next() {
        path.push(name);
        let metadata = std::fs::symlink_metadata(&path)?;
        let valid = if components.peek().is_some() {
            metadata.is_dir()
        } else {
            metadata.is_file()
        };
        if !valid {
            return Err(format!("source changed type at {path:?}").into());
        }
    }
    Ok(File::open(path)?)
}
