// Stub symbols for lwext4 C library when compiled with glibc toolchain instead of musl.
// These would normally come from ulibc.c, but glibc headers cause conflicts.
use core::ffi::c_int;

#[unsafe(no_mangle)]
pub static mut stdout: *mut core::ffi::c_void = core::ptr::null_mut();

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fflush(_f: *mut core::ffi::c_void) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __printf_chk(
    _flag: c_int,
    _fmt: *const u8,
    ...
) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __strcpy_chk(
    _dst: *mut u8,
    _src: *const u8,
    _destlen: usize,
) -> *mut u8 {
    core::ptr::null_mut()
}
