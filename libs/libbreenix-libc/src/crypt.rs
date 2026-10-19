//! POSIX crypt(), SHA-512-crypt only. The traditional static result is overwritten
//! by the next call; callers must serialize concurrent uses of crypt().
use core::ffi::c_char;
use libbreenix::crypto::sha512_crypt::{sha512_crypt, MAX_OUTPUT};

static mut RESULT: [u8; MAX_OUTPUT + 1] = [0; MAX_OUTPUT + 1];

#[no_mangle]
pub unsafe extern "C" fn crypt(key: *const c_char, setting: *const c_char) -> *mut c_char {
    let result = (|| {
        if key.is_null() || setting.is_null() { return None; }
        let key = core::ffi::CStr::from_ptr(key).to_bytes();
        let setting = core::ffi::CStr::from_ptr(setting).to_str().ok()?;
        let mut output = [0; MAX_OUTPUT];
        let length = sha512_crypt(key, setting, &mut output).ok()?;
        let ptr = core::ptr::addr_of_mut!(RESULT).cast::<u8>();
        core::ptr::copy_nonoverlapping(output.as_ptr(), ptr, length);
        ptr.add(length).write(0);
        Some(ptr.cast())
    })();
    result.unwrap_or_else(|| { *crate::__errno_location() = 22; core::ptr::null_mut() })
}
