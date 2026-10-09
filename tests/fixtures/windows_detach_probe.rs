//! Test DLL recording process attach/detach without any worker dependencies.

use std::ffi::c_void;

#[link(name = "kernel32")]
extern "system" {
    fn GetEnvironmentVariableW(name: *const u16, buffer: *mut u16, size: u32) -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        creation: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn WriteFile(
        file: *mut c_void,
        buffer: *const u8,
        size: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

static mut MARKER: *mut c_void = std::ptr::null_mut();

/// Record loader callbacks in the test-owned marker file.
///
/// Windows serializes these callbacks under the loader lock. Use kernel file
/// APIs so recording detach does not depend on Rust runtime teardown.
///
/// # Arguments
///
/// * `_module` - DLL module supplied by the loader.
/// * `reason` - Attach/detach event supplied by the loader.
/// * `_reserved` - Loader context, unused by this probe.
///
/// # Returns
///
/// One to allow the loader to attach the probe.
///
/// # Safety
///
/// Called only by the Windows loader. The test sets PARAKIT_DETACH_MARKER to
/// a writable repo-local path before loading this DLL.
#[no_mangle]
pub unsafe extern "system" fn DllMain(
    _module: *mut c_void,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if reason == 1 {
        let name: Vec<u16> = "PARAKIT_DETACH_MARKER\0".encode_utf16().collect();
        let mut path = [0u16; 4096];
        GetEnvironmentVariableW(name.as_ptr(), path.as_mut_ptr(), path.len() as u32);
        MARKER = CreateFileW(
            path.as_ptr(),
            0x40000000,
            3,
            std::ptr::null(),
            2,
            0,
            std::ptr::null_mut(),
        );
        let mut written = 0;
        WriteFile(
            MARKER,
            b"attach\n".as_ptr(),
            7,
            &mut written,
            std::ptr::null_mut(),
        );
    } else if reason == 0 {
        let mut written = 0;
        WriteFile(
            MARKER,
            b"detach\n".as_ptr(),
            7,
            &mut written,
            std::ptr::null_mut(),
        );
        CloseHandle(MARKER);
    }
    1
}
