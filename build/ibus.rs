//! Compile the optional Linux input-method bridge against the system IBus API.

/// Build the engine and daemon-side bridge only for Linux IBus builds.
///
/// # Panics
/// Reports a build error when IBus development files or a C compiler are missing.
pub(crate) fn build() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux")
        || std::env::var_os("CARGO_FEATURE_IBUS").is_none()
    {
        return;
    }
    let library = pkg_config::Config::new().probe("ibus-1.0").expect(
        "Linux IBus support requires libibus development files (libibus-1.0-dev on Debian/Ubuntu); use --no-default-features --features bundled,daemon for an X11-only build",
    );
    let mut build = cc::Build::new();
    for path in [
        "src/daemon/desktop/ibus/client.c",
        "src/daemon/desktop/ibus/helper.c",
    ] {
        println!("cargo:rerun-if-changed={path}");
        build.file(path);
    }
    println!("cargo:rerun-if-changed=src/daemon/desktop/ibus/shared.h");
    build
        .includes(library.include_paths)
        .warnings(true)
        .compile("parakit_ibus");
}
