
    #[derive(Debug, PartialEq)] pub enum SyscallResult { Ok(u64), Err(u64) }
    pub enum ErrorCode { InvalidArgument = 22, Fault = 14 }
    pub mod userptr {
        pub fn copy_to_user<T: Copy>(ptr: *mut T, value: &T) -> Result<(), ()> {
            if ptr.is_null() { return Err(()); }
            unsafe { ptr.write(*value); }
            Ok(())
        }
    }
    #[path = "../../kernel/src/syscall/pointer.rs"]
    pub mod pointer;

