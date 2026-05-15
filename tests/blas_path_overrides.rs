#[path = "../build_support/blas_paths.rs"]
mod blas_paths;

use blas_paths::complete_manual_path_override;

#[test]
fn complete_manual_blas_override_requires_include_and_libraries() {
    assert!(complete_manual_path_override(
        Some("C:/blas/include"),
        Some("C:/blas/lib/openblas.lib")
    ));
    assert!(!complete_manual_path_override(
        Some("C:/blas/include"),
        None
    ));
    assert!(!complete_manual_path_override(
        None,
        Some("C:/blas/lib/openblas.lib")
    ));
    assert!(!complete_manual_path_override(None, None));
}
