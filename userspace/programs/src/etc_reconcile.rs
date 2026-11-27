use std::path::Path;

fn run(root: &Path, args: &[String]) -> Result<(), String> {
    match args {
        [flag] if flag == "--enable-wheel-nopass" => {
            libbreenix::etc_reconcile::enable_wheel_nopass(root)?;
            println!("etc-reconcile: passwordless wheel policy active");
            return Ok(());
        }
        [] => {}
        _ => return Err("usage: etc-reconcile [--enable-wheel-nopass]".into()),
    }
    for name in libbreenix::etc_reconcile::reconcile_selected(root)? {
        println!("[etc-reconcile] kept local /etc/{}; wrote .factory sibling", name);
    }
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let Err(why) = run(Path::new("/"), &args) {
        eprintln!("etc-reconcile: {}", why);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libbreenix::accounts::policy::{authentication, parse_policy, Authentication};
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn activate_wheel_policy_without_factory_defaults() {
        let dir = std::env::temp_dir().join(format!("wheel-policy-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let tree = Scratch(dir);
        let etc = tree.0.join("etc");
        fs::create_dir(&etc).unwrap();
        fs::set_permissions(&etc, fs::Permissions::from_mode(0o755)).unwrap();
        let policy = etc.join("sudo.policy");
        let old = "permit %wheel as root\n";
        let custom = "permit %wheel as root cmd /bin/id\n";
        let args = ["--enable-wheel-nopass".to_string()];
        let write = |body| {
            fs::write(&policy, body).unwrap();
            fs::set_permissions(&policy, fs::Permissions::from_mode(0o440)).unwrap();
        };
        let decision = |groups: &[String]| {
            let rules = parse_policy(&fs::read_to_string(&policy).unwrap()).unwrap();
            authentication(Some(&rules), "developer", groups, "root", "/bin/bput")
        };

        write(old);
        let wheel = ["wheel".to_string()];
        assert_eq!(decision(&wheel), Authentication::Password);
        run(&tree.0, &args).unwrap();
        assert_eq!(decision(&wheel), Authentication::Passwordless);
        assert_eq!(decision(&[]), Authentication::Denied);
        assert_eq!(fs::metadata(&policy).unwrap().permissions().mode() & 0o7777, 0o440);
        run(&tree.0, &args).unwrap();

        fs::remove_file(&policy).unwrap();
        assert!(run(&tree.0, &args).is_err());
        assert!(!policy.exists());
        write(custom);
        assert!(run(&tree.0, &args).is_err());
        assert_eq!(fs::read_to_string(&policy).unwrap(), custom);
        fs::remove_file(&policy).unwrap();
        let linked = etc.join("linked");
        fs::write(&linked, old).unwrap();
        fs::set_permissions(&linked, fs::Permissions::from_mode(0o440)).unwrap();
        fs::hard_link(&linked, &policy).unwrap();
        assert!(run(&tree.0, &args).is_err());
        assert_eq!(fs::read_to_string(&policy).unwrap(), old);
    }
}
