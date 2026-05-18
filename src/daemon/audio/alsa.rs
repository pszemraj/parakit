//! Linux ALSA process-level setup.

use std::sync::Once;

static INSTALL: Once = Once::new();

/// Stop libasound from printing probe failures directly to stderr.
///
/// CPAL and rodio go through ALSA on Linux. During normal device discovery ALSA
/// may try stale `dmix`, `dsnoop`, or card aliases and print C-level diagnostics
/// before the Rust caller can decide whether the device matters. parakit keeps
/// its own microphone/sound warnings; this only suppresses libasound's default
/// stderr handler.
pub(crate) fn install_error_silencer() {
    INSTALL.call_once(|| {
        // SAFETY: The C shim installs a process-wide no-op ALSA error handler.
        // It does not retain Rust pointers and is guarded by `Once`.
        unsafe { parakit_install_alsa_error_silencer() };
    });
}

unsafe extern "C" {
    fn parakit_install_alsa_error_silencer();
}
