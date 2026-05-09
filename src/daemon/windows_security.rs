//! Windows process privilege diagnostics.

#![cfg(target_os = "windows")]

use std::mem::size_of;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenElevation,
    TokenIntegrityLevel, TOKEN_ELEVATION, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const SECURITY_MANDATORY_MEDIUM_RID: u32 = 0x0000_2000;
const SECURITY_MANDATORY_HIGH_RID: u32 = 0x0000_3000;
const SECURITY_MANDATORY_SYSTEM_RID: u32 = 0x0000_4000;

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

        let mut buf = vec![0_u8; needed as usize];
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

        let label = unsafe { &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
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
}

impl Drop for TokenHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
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
