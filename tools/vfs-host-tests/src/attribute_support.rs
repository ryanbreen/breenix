//! Scheduler and process lookup shims for the production credential snapshot.
//! Attribute policy tests pass explicit credentials instead of using current().
pub mod process {
    pub use crate::credentials;
    pub struct Cwd;
    impl Cwd { pub fn umask(&self) -> u32 { 0 } }
    pub struct Process {
        pub cred: credentials::ProcessCredentials,
        pub cwd: Cwd,
    }
    pub struct Manager;
    impl Manager {
        pub fn find_process_by_thread(&self, _tid: u64) -> Option<(u64, &Process)> { None }
    }
    pub fn manager() -> Option<Manager> { None }
}
pub mod task {
    pub mod scheduler {
        pub fn current_thread_id() -> Option<u64> { None }
        pub fn thread_is_user(_tid: u64) -> bool { false }
    }
}
pub mod test_control {
    pub const CRED_ORPHAN: u32 = 32;
    pub fn take(_point: u32, _tid: u64) -> Option<u64> { None }
}
