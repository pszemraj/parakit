//! Standalone IBus engine process, linked only to the input-method bridge.

#[cfg(target_os = "linux")]
#[link(name = "parakit_ibus", kind = "static")]
unsafe extern "C" {
    fn parakit_ibus_run_engine() -> std::ffi::c_int;
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    if std::env::args_os().nth(1).is_some() {
        eprintln!("Usage: parakit-ibus\nIBus launches this engine without arguments.");
        return std::process::ExitCode::from(2);
    }
    // The native entry point owns its GLib loop until IBus disconnects.
    if unsafe { parakit_ibus_run_engine() } == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!("parakit-ibus: could not start the IBus engine");
        std::process::ExitCode::FAILURE
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("parakit-ibus is supported only on Linux");
    std::process::ExitCode::FAILURE
}
