#[path="../kernel/src/drivers/virtio/request_state.rs"]
mod request_state;
use request_state::RequestState;
#[test] fn detached_dma_is_not_reused_before_completion(){let s=RequestState::new();assert!(s.try_acquire());s.detach();assert!(!s.try_acquire());assert!(!s.try_reap(false));assert!(s.try_reap(true));s.release();assert!(s.try_acquire());}
#[test] fn completion_before_detach_is_reaped(){let s=RequestState::new();assert!(s.try_acquire());assert!(!s.try_reap(true));s.detach();assert!(s.try_reap(true));assert!(!s.try_reap(true));}
#[test] fn live_owner_cannot_be_reaped(){let s=RequestState::new();assert!(s.try_acquire());assert!(!s.try_reap(true));assert!(!s.try_acquire());}
#[test] fn failed_device_stays_wedged(){let s=RequestState::new();assert!(s.try_acquire());s.wedge();assert!(s.is_wedged());assert!(!s.try_acquire());assert!(!s.try_reap(true));}
#[test] fn only_one_contender_reaps(){let s=std::sync::Arc::new(RequestState::new());assert!(s.try_acquire());s.detach();let threads:Vec<_>=(0..8).map(|_|{let s=s.clone();std::thread::spawn(move||s.try_reap(true))}).collect();assert_eq!(threads.into_iter().map(|t|t.join().unwrap()).filter(|v|*v).count(),1);}

#[test] fn availability_matches_completion_and_ownership(){let s=RequestState::new();assert!(s.available(false));assert!(s.try_acquire());assert!(!s.available(true));s.detach();assert!(!s.available(false));assert!(s.available(true));s.wedge();assert!(!s.available(true));}
