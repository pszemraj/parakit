//! Build-time BLAS path override helpers.

/// Return whether CMake BLAS path variables form a complete manual override.
///
/// One variable alone is treated as a partial CMake hint. Both variables
/// together are authoritative and must not be overwritten by autodetection.
pub fn complete_manual_path_override(include_dirs: Option<&str>, libraries: Option<&str>) -> bool {
    include_dirs.is_some() && libraries.is_some()
}
