fn main() {
    match libbreenix::etc_reconcile::reconcile_selected(std::path::Path::new("/")) {
        Ok(conflicts) => for name in conflicts {
            println!("[etc-reconcile] kept local /etc/{}; wrote .factory sibling", name);
        },
        Err(why) => {
            eprintln!("etc-reconcile: {}", why);
            std::process::exit(1);
        },
    }
}
