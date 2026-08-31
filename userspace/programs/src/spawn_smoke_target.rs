//! Spawn smoke target -- the image init's run_spawn_smoke() (#713) spawns
//! on x86 to prove spawn() works end to end (create + exec + run + exit +
//! reap), independent of the boot-script chain (see #722). Exits 0
//! unconditionally, no output, mirroring simple_exit.rs's own minimal
//! shape but with the exit code run_spawn_smoke's gate pin
//! (INIT_SPAWN_SMOKE_REAP_LITERAL = "exited (code 0)") actually needs.

fn main() {
    std::process::exit(0);
}
