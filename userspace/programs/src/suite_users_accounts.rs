// Account cases run on the disposable suite root rather than the persistent store.
use libbreenix::accounts as ac;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

struct AccountFixture {
    name: String,
    home: String,
}
impl AccountFixture {
    fn new(tag: &str) -> Self {
        let name = format!("ac_{}_{}", tag, pid());
        let home = format!("/home/{name}");
        Self { name, home }
    }
    fn add(&self, extra: &[&str]) -> CaseResult {
        let mut args = extra.to_vec();
        args.push(&self.name);
        account_tool("useradd", &args, true)
    }
    fn account(&self) -> Result<ac::Account, String> {
        ac::Database::read(Path::new("/"))?
            .account(&self.name)
            .ok_or_else(|| "new account missing".into())
    }
}
impl Drop for AccountFixture {
    fn drop(&mut self) {
        let _ = ac::userdel(Path::new("/"), &self.name, false);
        let _ = ac::groupdel(Path::new("/"), &self.name);
        let _ = fs::remove_dir_all(&self.home);
    }
}
fn account_tool(name: &str, args: &[&str], success: bool) -> CaseResult {
    let out = Command::new(format!("/usr/sbin/{name}"))
        .args(args)
        .output()
        .map_err(|e| format!("{name}: {e}"))?;
    check(
        out.status.success() == success,
        &format!(
            "{name}: {} {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}
fn account_snapshot() -> Result<[String; 3], String> {
    let db = ac::Database::read(Path::new("/"))?;
    Ok([db.passwd, db.group, db.shadow])
}
fn accounts_entries() -> CaseResult {
    let f = AccountFixture::new("entries");
    f.add(&[])?;
    let db = ac::Database::read(Path::new("/"))?;
    let a = f.account()?;
    check(
        a.uid >= 1000 && a.gid == a.uid && a.home == f.home && a.shell == "/bin/bsh",
        "new account fields",
    )?;
    check(
        db.group
            .lines()
            .any(|l| l == format!("{}:x:{}:", f.name, a.gid)),
        "private group missing",
    )?;
    check(
        db.shadow(&f.name).is_some_and(|s| s.fields[1] == b"!"),
        "new password is not locked",
    )?;
    for (name, mode) in [
        ("passwd", 0o644),
        ("group", 0o644),
        ("shadow", 0o600),
        (".pwd.lock", 0o600),
    ] {
        attrs(&Path::new("/etc").join(name), mode, 0, 0, false)?;
    }
    Ok(())
}
fn accounts_home() -> CaseResult {
    let f = AccountFixture::new("home");
    let skel = Path::new("/etc/skel/accounts-fixture");
    fs::write(skel, b"skeleton content\n").map_err(|e| e.to_string())?;
    fs::set_permissions(skel, fs::Permissions::from_mode(0o640)).map_err(|e| e.to_string())?;
    let result = f.add(&["-m"]);
    let _ = fs::remove_file(skel);
    result?;
    let a = f.account()?;
    attrs(Path::new(&f.home), 0o700, a.uid, a.gid, true)?;
    attrs(
        &Path::new(&f.home).join("accounts-fixture"),
        0o640,
        a.uid,
        a.gid,
        false,
    )?;
    check(
        fs::read(Path::new(&f.home).join("accounts-fixture")).map_err(|e| e.to_string())?
            == b"skeleton content\n",
        "skeleton bytes changed",
    )
}
fn accounts_duplicate() -> CaseResult {
    let a = AccountFixture::new("dup");
    let b = AccountFixture::new("dup_id");
    a.add(&[])?;
    let before = account_snapshot()?;
    account_tool("useradd", &[&a.name], false)?;
    let uid = a.account()?.uid.to_string();
    account_tool("useradd", &["-u", &uid, &b.name], false)?;
    check(
        before == account_snapshot()?,
        "duplicate refusal changed account files",
    )
}
fn accounts_options() -> CaseResult {
    let f = AccountFixture::new("opts");
    account_tool(
        "useradd",
        &[
            "-u",
            "32001",
            "-g",
            "users",
            "-G",
            "wheel,adm",
            "-s",
            "/bin/sh",
            "-d",
            "/srv/unused",
            &f.name,
        ],
        true,
    )?;
    let a = f.account()?;
    check(
        a.uid == 32001
            && a.gid == 100
            && a.groups.contains(&10)
            && a.groups.contains(&4)
            && a.shell == "/bin/sh"
            && a.home == "/srv/unused",
        "explicit useradd fields",
    )?;
    as_user(A, || {
        let out = Command::new("/usr/sbin/useradd")
            .arg("ac_refused")
            .output()
            .map_err(|e| e.to_string())?;
        want(!out.status.success(), "non-root useradd succeeded")
    })?;
    check(
        ac::Database::read(Path::new("/"))?
            .account("ac_refused")
            .is_none(),
        "non-root created an account",
    )
}
fn accounts_delete() -> CaseResult {
    let f = AccountFixture::new("delete");
    f.add(&["-m", "-G", "wheel"])?;
    account_tool("userdel", &["-r", &f.name], true)?;
    let db = ac::Database::read(Path::new("/"))?;
    check(
        db.account(&f.name).is_none()
            && db.shadow(&f.name).is_none()
            && !Path::new(&f.home).exists(),
        "userdel retained account or home",
    )?;
    check(
        !db.group
            .lines()
            .filter_map(|l| ac::parse::Group::parse(l.as_bytes()))
            .any(|g| {
                g.fields[3]
                    .split(|b| *b == b',')
                    .any(|m| m == f.name.as_bytes())
            }),
        "userdel retained membership",
    )
}
fn accounts_busy() -> CaseResult {
    let f = AccountFixture::new("busy");
    f.add(&[])?;
    let a = f.account()?;
    let holder = Holder::start(Account {
        uid: a.uid,
        gid: a.gid,
        groups: &[],
    })?;
    let before = account_snapshot()?;
    account_tool("userdel", &[&f.name], false)?;
    check(
        before == account_snapshot()?,
        "busy refusal changed databases",
    )?;
    drop(holder);
    account_tool("userdel", &[&f.name], true)
}
fn accounts_home_owner() -> CaseResult {
    let f = AccountFixture::new("owner");
    f.add(&["-m"])?;
    std::os::unix::fs::chown(&f.home, Some(UB), Some(UB)).map_err(|e| e.to_string())?;
    let before = account_snapshot()?;
    account_tool("userdel", &["-r", &f.name], false)?;
    check(
        before == account_snapshot()? && Path::new(&f.home).is_dir(),
        "unowned home was removed or account changed",
    )
}
fn accounts_groups() -> CaseResult {
    let f = AccountFixture::new("groups");
    f.add(&["-G", "wheel"])?;
    let original = ac::Database::read(Path::new("/"))?.group;
    account_tool("usermod", &["-a", "-G", "adm", &f.name], true)?;
    let a = f.account()?;
    check(
        a.groups.contains(&4) && a.groups.contains(&10),
        "append lost a membership",
    )?;
    account_tool("usermod", &["-G", "users", &f.name], true)?;
    let a = f.account()?;
    check(
        a.groups.contains(&100) && !a.groups.contains(&4) && !a.groups.contains(&10),
        "replace retained old memberships",
    )?;
    let now = ac::Database::read(Path::new("/"))?.group;
    for g in original
        .lines()
        .filter_map(|l| ac::parse::Group::parse(l.as_bytes()))
    {
        for m in g.fields[3]
            .split(|b| *b == b',')
            .filter(|m| !m.is_empty() && *m != f.name.as_bytes())
        {
            check(
                now.lines()
                    .filter_map(|l| ac::parse::Group::parse(l.as_bytes()))
                    .any(|n| n.gid == g.gid && n.fields[3].split(|b| *b == b',').any(|v| v == m)),
                "another account's membership changed",
            )?;
        }
    }
    Ok(())
}
fn accounts_modify() -> CaseResult {
    let f = AccountFixture::new("modify");
    f.add(&[])?;
    ac::change_password(Path::new("/"), &f.name, b"", b"account fixture password")?;
    let original = ac::Database::read(Path::new("/"))?
        .shadow(&f.name)
        .unwrap()
        .fields[1]
        .to_vec();
    account_tool(
        "usermod",
        &["-L", "-s", "/bin/sh", "-e", "2000-01-01", &f.name],
        true,
    )?;
    let db = ac::Database::read(Path::new("/"))?;
    let s = db.shadow(&f.name).ok_or("shadow missing")?;
    check(
        s.fields[1].strip_prefix(b"!") == Some(original.as_slice())
            && s.expired(ac::today())
            && f.account()?.shell == "/bin/sh",
        "lock, expiration or shell change failed",
    )?;
    account_tool("usermod", &["-U", "-e", "-1", &f.name], true)?;
    let db = ac::Database::read(Path::new("/"))?;
    let s = db.shadow(&f.name).ok_or("shadow missing")?;
    check(
        s.fields[1] == original && !s.expired(ac::today()),
        "unlock lost hash or expiration not cleared",
    )
}
fn accounts_group_tools() -> CaseResult {
    let group = AccountFixture::new("newgroup");
    account_tool("groupadd", &["-g", "32002", &group.name], true)?;
    account_tool("groupadd", &[&group.name], false)?;
    let f = AccountFixture::new("primary");
    f.add(&["-g", &group.name])?;
    check(f.account()?.gid == 32002, "primary group did not resolve")?;
    account_tool("groupdel", &[&group.name], false)?;
    account_tool("userdel", &[&f.name], true)?;
    account_tool("groupdel", &[&group.name], true)
}
fn accounts_serializes() -> CaseResult {
    let a = AccountFixture::new("lock_a");
    let b = AccountFixture::new("lock_b");
    let lock = ac::Lock::acquire(Path::new("/"))?;
    let mut first = Command::new("/usr/sbin/useradd")
        .arg(&a.name)
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut second = Command::new("/usr/sbin/useradd")
        .arg(&b.name)
        .spawn()
        .map_err(|e| e.to_string())?;
    // Observe both tools asleep before releasing the lock. They have no input
    // to wait for: the only blocking operation before publication is F_SETLKW.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let blocked = [first.id(), second.id()].iter().all(|pid| {
            fs::read_to_string(format!("/proc/{pid}/status"))
                .is_ok_and(|s| s.lines().any(|l| l == "State:\tBlocked"))
        });
        if blocked {
            break;
        }
        check(
            first.try_wait().map_err(|e| e.to_string())?.is_none()
                && second.try_wait().map_err(|e| e.to_string())?.is_none(),
            "writer escaped a held lock",
        )?;
        check(
            std::time::Instant::now() < deadline,
            "writers did not block on the held password lock",
        )?;
        std::thread::yield_now();
    }
    drop(lock);
    check(
        first.wait().map_err(|e| e.to_string())?.success()
            && second.wait().map_err(|e| e.to_string())?.success(),
        "concurrent useradd failed",
    )?;
    a.account()?;
    b.account()?;
    Ok(())
}
fn accounts_libc_lock() -> CaseResult {
    extern "C" {
        fn lckpwdf() -> i32;
        fn ulckpwdf() -> i32;
        fn fcntl(fd: i32, command: i32, ...) -> i32;
    }
    use std::os::fd::AsRawFd;
    let (ready_r, ready_w) = io::pipe()?;
    let (hold_r, hold_w) = io::pipe()?;
    match process::fork()? {
        ForkResult::Child => {
            let _ = io::close(ready_r);
            let _ = io::close(hold_w);
            if unsafe { lckpwdf() } != 0 {
                process::exit(1);
            }
            let _ = io::write(ready_w, b"locked");
            let _ = io::close(ready_w);
            let mut byte = [0];
            let _ = io::read(hold_r, &mut byte);
            process::exit(if unsafe { ulckpwdf() } == 0 { 0 } else { 2 });
        }
        ForkResult::Parent(child) => {
            let _ = io::close(ready_w);
            let _ = io::close(hold_r);
            let ready = read_all(ready_r);
            let _ = io::close(ready_r);
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/etc/.pwd.lock")
                .map_err(|e| e.to_string())?;
            let mut record = ac::PasswordLock::write();
            let query = unsafe { fcntl(file.as_raw_fd(), 5, &mut record) };
            let _ = io::close(hold_w);
            let status = wait(child.raw() as i32)?;
            check(
                ready == b"locked"
                    && query == 0
                    && record.kind == 1
                    && record.pid == child.raw() as i32
                    && status == 0,
                "libc and Rust did not share the lock",
            )?;
            drop(file);
            let _lock = ac::Lock::acquire(Path::new("/"))?;
            Ok(())
        }
    }
}
fn protocol_tree(path: &Path) -> Result<ac::Database, String> {
    fs::create_dir_all(path.join("etc")).map_err(|e| e.to_string())?;
    fs::set_permissions(path.join("etc"), fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    for (name, bytes, mode) in [
        ("passwd", "root:x:0:0::/root:/bin/bsh\n", 0o644),
        ("group", "root:x:0:\n", 0o644),
        ("shadow", "root:!:::::::\n", 0o600),
    ] {
        let p = path.join("etc").join(name);
        fs::write(&p, bytes).map_err(|e| e.to_string())?;
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())?;
    }
    ac::Database::read(path)
}
fn protocol_new(old: &ac::Database) -> ac::Database {
    ac::Database {
        passwd: format!("{}next:x:32003:0::/home/next:/bin/bsh\n", old.passwd),
        group: old.group.clone(),
        shadow: format!("{}next:!:::::::\n", old.shadow),
    }
}
fn accounts_interrupted() -> CaseResult {
    let tree = format!("/tmp/accounts-protocol-{}", pid());
    let root = Path::new(&tree);
    let old = protocol_tree(root)?;
    let new = protocol_new(&old);
    let mut boundaries = 0;
    {
        let _lock = ac::Lock::acquire(root)?;
        new.publish(root, &old, &mut || {
            boundaries += 1;
            Ok(())
        })?;
    }
    for stop in 1..=boundaries {
        let iteration = root.join(format!("stop-{stop}"));
        let root = iteration.as_path();
        let old = protocol_tree(root)?;
        let new = protocol_new(&old);
        match process::fork()? {
            ForkResult::Child => {
                let result = (|| -> Result<(), String> {
                    let _lock = ac::Lock::acquire(root)?;
                    let mut at = 0;
                    new.publish(root, &old, &mut || {
                        at += 1;
                        if at == stop {
                            process::exit(99);
                        }
                        Ok(())
                    })
                })();
                process::exit(if result.is_ok() { 0 } else { 1 });
            }
            ForkResult::Parent(child) => {
                check(
                    wait(child.raw() as i32)? == 99 << 8,
                    "interruption point was not reached",
                )?;
                let current = ac::Database::read(root)?;
                check(
                    (current.passwd == old.passwd || current.passwd == new.passwd)
                        && (current.shadow == old.shadow || current.shadow == new.shadow)
                        && current.group == old.group,
                    "interruption left a partial file",
                )?;
                check(
                    current.passwd != new.passwd || current.shadow == new.shadow,
                    "passwd published before shadow",
                )?;
                let _lock = ac::Lock::acquire(root)?;
                new.publish(root, &current, &mut || Ok(()))?;
                check(
                    ac::Database::read(root)?.passwd == new.passwd
                        && ac::Database::read(root)?.shadow == new.shadow,
                    "retry failed",
                )?;
            }
        }
    }
    fs::remove_dir_all(root).map_err(|e| e.to_string())?;
    Ok(())
}
fn password_fixture(tag: &str) -> Result<AccountFixture, String> {
    let f = AccountFixture::new(tag);
    f.add(&[]).map_err(|e| format!("{e:?}"))?;
    ac::change_password(Path::new("/"), &f.name, b"", b"initial fixture password")?;
    Ok(f)
}
fn with_passwd_identity(uid: u32, operation: impl FnOnce() -> Checked) -> CaseResult {
    as_account(None, || {
        // This is passwd's set-ID state: real user, effective/saved root.
        ok(
            "passwd real identity",
            sc(nr::SETRESUID, &[uid as u64, 0, 0]),
        )?;
        operation()
    })
}
fn accounts_password() -> CaseResult {
    let f = password_fixture("password")?;
    let uid = f.account()?.uid;
    with_passwd_identity(uid, || {
        ac::change_password(
            Path::new("/"),
            &f.name,
            b"initial fixture password",
            b"replacement fixture password",
        )
    })?;
    let db = ac::Database::read(Path::new("/"))?;
    let s = db.shadow(&f.name).ok_or("shadow missing")?;
    let hash = std::str::from_utf8(s.fields[1]).map_err(|e| e.to_string())?;
    check(
        hash.starts_with("$6$rounds=100000$")
            && libbreenix::crypto::sha512_crypt::verify(b"replacement fixture password", hash)
            && !libbreenix::crypto::sha512_crypt::verify(b"initial fixture password", hash),
        "replacement password did not authenticate exclusively",
    )
}
fn accounts_password_refused() -> CaseResult {
    let a = password_fixture("pw_a")?;
    let b = password_fixture("pw_b")?;
    let uid = a.account()?.uid;
    let before = account_snapshot()?;
    with_passwd_identity(uid, || {
        want(
            ac::change_password(
                Path::new("/"),
                &b.name,
                b"initial fixture password",
                b"replacement fixture password",
            )
            .is_err(),
            "changed another account",
        )?;
        want(
            ac::change_password(
                Path::new("/"),
                &a.name,
                b"wrong fixture password",
                b"replacement fixture password",
            )
            .is_err(),
            "wrong old password accepted",
        )?;
        want(
            ac::change_password(
                Path::new("/"),
                &a.name,
                b"initial fixture password",
                b"short",
            )
            .is_err(),
            "short password accepted",
        )
    })?;
    check(
        before == account_snapshot()?,
        "password refusal changed a database",
    )
}
fn accounts_id() -> CaseResult {
    let f = AccountFixture::new("id");
    f.add(&["-G", "wheel"])?;
    let a = f.account()?;
    let out = Command::new("/bin/id")
        .arg(&f.name)
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    check(
        out.status.success()
            && text.contains(&format!("uid={}({})", a.uid, f.name))
            && text.contains(&format!("gid={}({})", a.gid, f.name))
            && text.contains("10(wheel)"),
        "id did not resolve account fields",
    )?;
    let out = Command::new("/bin/groups")
        .arg(&f.name)
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    check(
        out.status.success() && text.contains(&f.name) && text.contains("wheel"),
        "groups did not resolve account memberships",
    )
}
fn accounts_malformed() -> CaseResult {
    let db = ac::Database {
        passwd: "bad:x:no:1000::/home/bad:/bin/sh\ngood:x:32000:32000::/home/good:/bin/bsh\n"
            .into(),
        group: "broken:x:no:good\ngood:x:32000:\nextra:x:32001:good\n".into(),
        shadow: "bad:!:no::::::\ngood:!:::::::\n".into(),
    };
    check(
        db.account("bad").is_none() && db.shadow("bad").is_none(),
        "malformed account remained usable",
    )?;
    check(
        db.account("good")
            .is_some_and(|a| a.groups == [32000, 32001])
            && db.shadow("good").is_some(),
        "bad account disabled its neighbour",
    )
}
fn accounts_audit() -> CaseResult {
    let f = AccountFixture::new("audit");
    f.add(&[])?;
    attrs(Path::new("/var/log/auth.log"), 0o640, 0, 4, false)?;
    let log = fs::read_to_string("/var/log/auth.log").map_err(|e| e.to_string())?;
    check(
        log.lines().any(|l| {
            l.contains(" useradd[")
                && l.contains(&format!("target={}", f.name))
                && l.contains("result=ok")
        }),
        "successful account change was not audited",
    )?;
    use std::os::unix::fs::FileTypeExt;
    let m = fs::metadata("/dev/kmsg").map_err(|e| e.to_string())?;
    check(
        m.file_type().is_char_device()
            && m.uid() == 0
            && m.gid() == 0
            && m.mode() & 0o7777 == 0o200,
        "unsafe audit sink",
    )?;
    as_user(A, || {
        refused("user audit injection", try_open("/dev/kmsg", O_WRONLY), 13)
    })
}
