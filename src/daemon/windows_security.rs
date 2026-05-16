//! Windows process privilege diagnostics.

#![cfg(target_os = "windows")]

use std::{
    ffi::c_void,
    mem::{size_of, MaybeUninit},
    ptr::null_mut,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenElevation,
    TokenIntegrityLevel, TokenUser, PSID, TOKEN_ELEVATION, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    TOKEN_USER,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const SECURITY_MANDATORY_MEDIUM_RID: u32 = 0x0000_2000;
const SECURITY_MANDATORY_HIGH_RID: u32 = 0x0000_3000;
const SECURITY_MANDATORY_SYSTEM_RID: u32 = 0x0000_4000;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn ConvertSidToStringSidW(sid: PSID, string_sid: *mut *mut u16) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

/// Best-effort process privilege report for doctor output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WindowsSecurityReport {
    pub(crate) elevated: Option<bool>,
    pub(crate) integrity: Option<String>,
}

/// Read current-process elevation and integrity information.
///
/// # Returns
///
/// A best-effort report. Individual fields are `None` when Windows denies or
/// omits that query.
pub(crate) fn current_process_security_report() -> WindowsSecurityReport {
    match TokenHandle::open_current() {
        Some(token) => WindowsSecurityReport {
            elevated: token.is_elevated(),
            integrity: token.integrity_label(),
        },
        None => WindowsSecurityReport {
            elevated: None,
            integrity: None,
        },
    }
}

/// Return the current process user's SID string.
///
/// # Returns
///
/// `Some("S-1-...")` when Windows token inspection succeeds.
pub(crate) fn current_user_sid_string() -> Option<String> {
    TokenHandle::open_current()?.user_sid_string()
}

struct TokenHandle(HANDLE);

impl TokenHandle {
    fn open_current() -> Option<Self> {
        let mut handle = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) }
            .ok()
            .map(|_| Self(handle))
    }

    fn is_elevated(&self) -> Option<bool> {
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0_u32;
        unsafe {
            GetTokenInformation(
                self.0,
                TokenElevation,
                Some(&mut elevation as *mut TOKEN_ELEVATION as *mut _),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        }
        .ok()?;
        Some(elevation.TokenIsElevated != 0)
    }

    fn integrity_label(&self) -> Option<String> {
        let mut needed = 0_u32;
        let _ = unsafe { GetTokenInformation(self.0, TokenIntegrityLevel, None, 0, &mut needed) };
        if needed == 0 {
            return None;
        }

        let mut buf = token_info_storage::<TOKEN_MANDATORY_LABEL>(needed)?;
        unsafe {
            GetTokenInformation(
                self.0,
                TokenIntegrityLevel,
                Some(buf.as_mut_ptr() as *mut _),
                needed,
                &mut needed,
            )
        }
        .ok()?;

        let label = token_info_head::<TOKEN_MANDATORY_LABEL>(&buf, needed)?;
        let count = unsafe { GetSidSubAuthorityCount(label.Label.Sid) };
        if count.is_null() || unsafe { *count } == 0 {
            return None;
        }
        let rid_index = u32::from(unsafe { *count }) - 1;
        let rid = unsafe { GetSidSubAuthority(label.Label.Sid, rid_index) };
        if rid.is_null() {
            return None;
        }
        Some(integrity_label_from_rid(unsafe { *rid }))
    }

    fn user_sid_string(&self) -> Option<String> {
        let mut needed = 0_u32;
        let _ = unsafe { GetTokenInformation(self.0, TokenUser, None, 0, &mut needed) };
        if needed == 0 {
            return None;
        }

        let mut buf = token_info_storage::<TOKEN_USER>(needed)?;
        unsafe {
            GetTokenInformation(
                self.0,
                TokenUser,
                Some(buf.as_mut_ptr() as *mut _),
                needed,
                &mut needed,
            )
        }
        .ok()?;

        let user = token_info_head::<TOKEN_USER>(&buf, needed)?;
        let mut sid = null_mut::<u16>();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) } == 0 || sid.is_null() {
            return None;
        }
        let result = unsafe { wide_ptr_to_string(sid) };
        unsafe {
            let _ = LocalFree(sid as *mut c_void);
        }
        result
    }
}

impl Drop for TokenHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn token_info_storage<T>(needed: u32) -> Option<Vec<MaybeUninit<T>>> {
    let needed = usize::try_from(needed).ok()?;
    let item_size = size_of::<T>();
    if item_size == 0 || needed < item_size {
        return None;
    }

    let slots = needed.div_ceil(item_size);
    let mut storage = Vec::with_capacity(slots);
    storage.resize_with(slots, MaybeUninit::uninit);
    Some(storage)
}

fn token_info_head<T>(storage: &[MaybeUninit<T>], returned: u32) -> Option<&T> {
    let returned = usize::try_from(returned).ok()?;
    let item_size = size_of::<T>();
    let storage_bytes = storage.len().checked_mul(item_size)?;
    if item_size == 0 || returned < item_size || returned > storage_bytes {
        return None;
    }

    // GetTokenInformation wrote the header into storage aligned for T.
    Some(unsafe { &*(storage.as_ptr() as *const T) })
}

fn integrity_label_from_rid(rid: u32) -> String {
    if rid < SECURITY_MANDATORY_MEDIUM_RID {
        return "low".to_string();
    }
    if rid < SECURITY_MANDATORY_HIGH_RID {
        return "medium".to_string();
    }
    if rid < SECURITY_MANDATORY_SYSTEM_RID {
        return "high".to_string();
    }
    "system".to_string()
}

unsafe fn wide_ptr_to_string(ptr: *const u16) -> Option<String> {
    let mut len = 0_usize;
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(ptr, len) }).ok()
}
