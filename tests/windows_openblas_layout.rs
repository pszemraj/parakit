//! Integration coverage for Windows OpenBLAS layout detection.

mod common;
#[path = "../build/windows_openblas.rs"]
mod windows_openblas;

use windows_openblas::{
    find_windows_openblas, is_known_openblas_runtime_dll, remove_known_openblas_runtime_dlls,
    WindowsOpenBlasImportKind,
};

/// Expected layout fields for a case that should be detected.
struct Expected {
    include_dir: &'static str,
    import_lib: &'static str,
    /// Exact, ordered `runtime_dlls` (only asserted when `Some`).
    runtime_dlls_exact: Option<&'static [&'static str]>,
    /// `runtime_dlls` membership checks (order-independent).
    runtime_dlls_contains: &'static [&'static str],
}

struct Case {
    name: &'static str,
    files: &'static [&'static str],
    kind: WindowsOpenBlasImportKind,
    expect: Option<Expected>,
}

const CASES: &[Case] = &[
    Case {
        name: "current-conda",
        files: &[
            "include/openblas/cblas.h",
            "lib/openblas.lib",
            "bin/openblas.dll",
        ],
        kind: WindowsOpenBlasImportKind::Msvc,
        expect: Some(Expected {
            include_dir: "include/openblas",
            import_lib: "lib/openblas.lib",
            runtime_dlls_exact: None,
            runtime_dlls_contains: &["bin/openblas.dll"],
        }),
    },
    Case {
        name: "flat-libopenblas",
        files: &[
            "include/cblas.h",
            "lib/libopenblas.lib",
            "bin/libopenblas.dll",
            "bin/libomp.dll",
        ],
        kind: WindowsOpenBlasImportKind::Msvc,
        expect: Some(Expected {
            include_dir: "include",
            import_lib: "lib/libopenblas.lib",
            runtime_dlls_exact: Some(&["bin/libomp.dll", "bin/libopenblas.dll"]),
            runtime_dlls_contains: &[],
        }),
    },
    Case {
        name: "gnu-versioned",
        files: &[
            "include/cblas.h",
            "lib/libopenblas.dll.a",
            "bin/libopenblas64_.dll",
            "bin/libgcc_s_seh-1.dll",
            "bin/libwinpthread-1.dll",
        ],
        kind: WindowsOpenBlasImportKind::Gnu,
        expect: Some(Expected {
            include_dir: "include",
            import_lib: "lib/libopenblas.dll.a",
            runtime_dlls_exact: None,
            runtime_dlls_contains: &["bin/libopenblas64_.dll", "bin/libwinpthread-1.dll"],
        }),
    },
    Case {
        name: "missing-primary-runtime",
        files: &["include/cblas.h", "lib/openblas.lib", "bin/libomp.dll"],
        kind: WindowsOpenBlasImportKind::Msvc,
        expect: None,
    },
    Case {
        name: "msvc-rejects-dll-a",
        files: &[
            "include/cblas.h",
            "lib/libopenblas.dll.a",
            "bin/libopenblas.dll",
        ],
        kind: WindowsOpenBlasImportKind::Msvc,
        expect: None,
    },
];

fn check_case(case: &Case) -> Option<String> {
    let root =
        common::fixture_root_with_files("windows-openblas-layout-tests", case.name, case.files);

    let found = find_windows_openblas(&root, case.kind);

    let Some(expected) = &case.expect else {
        return found.is_some().then(|| {
            format!(
                "{}: expected layout to be rejected, got {:?}",
                case.name, found
            )
        });
    };

    let Some(found) = found else {
        return Some(format!("{}: expected layout to be detected", case.name));
    };

    let mut mismatches = Vec::new();

    let want_include_dir = root.join(expected.include_dir);
    if found.include_dir != want_include_dir {
        mismatches.push(format!(
            "include_dir: expected {:?}, got {:?}",
            want_include_dir, found.include_dir
        ));
    }

    let want_import_lib = root.join(expected.import_lib);
    if found.import_lib != want_import_lib {
        mismatches.push(format!(
            "import_lib: expected {:?}, got {:?}",
            want_import_lib, found.import_lib
        ));
    }

    if let Some(exact) = expected.runtime_dlls_exact {
        let want = exact.iter().map(|p| root.join(p)).collect::<Vec<_>>();
        if found.runtime_dlls != want {
            mismatches.push(format!(
                "runtime_dlls: expected exactly {:?}, got {:?}",
                want, found.runtime_dlls
            ));
        }
    }

    for contains in expected.runtime_dlls_contains {
        let want = root.join(contains);
        if !found.runtime_dlls.contains(&want) {
            mismatches.push(format!("runtime_dlls: expected to contain {:?}", want));
        }
    }

    (!mismatches.is_empty()).then(|| format!("{}: {}", case.name, mismatches.join("; ")))
}

#[test]
fn detects_and_rejects_windows_openblas_layouts() {
    let failures: Vec<String> = CASES.iter().filter_map(check_case).collect();
    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn target_kind_selects_compatible_import_library_from_mixed_layout() {
    let root = common::fixture_root_with_files(
        "windows-openblas-layout-tests",
        "mixed-import-libs",
        &[
            "include/cblas.h",
            "lib/openblas.lib",
            "lib/libopenblas.dll.a",
            "bin/libopenblas.dll",
        ],
    );

    let msvc = find_windows_openblas(&root, WindowsOpenBlasImportKind::Msvc)
        .expect("MSVC layout should be detected");
    let gnu = find_windows_openblas(&root, WindowsOpenBlasImportKind::Gnu)
        .expect("GNU layout should be detected");

    assert_eq!(msvc.import_lib, root.join("lib/openblas.lib"));
    assert_eq!(gnu.import_lib, root.join("lib/libopenblas.dll.a"));
}

#[test]
fn runtime_dll_filter_accepts_primary_and_known_dependency_names() {
    assert!(is_known_openblas_runtime_dll("openblas.dll"));
    assert!(is_known_openblas_runtime_dll("libopenblas.dll"));
    assert!(is_known_openblas_runtime_dll("libopenblas64_.dll"));
    assert!(is_known_openblas_runtime_dll("libgfortran-5.dll"));
    assert!(is_known_openblas_runtime_dll("libgcc_s_seh-1.dll"));
    assert!(is_known_openblas_runtime_dll("libquadmath-0.dll"));
    assert!(is_known_openblas_runtime_dll("libwinpthread-1.dll"));
    assert!(!is_known_openblas_runtime_dll("unrelated.dll"));
}

#[test]
fn removes_stale_openblas_runtimes_without_touching_other_dlls() {
    let root = common::fixture_root_with_files(
        "windows-openblas-layout-tests",
        "stale-runtime-dlls",
        &[
            "bin/openblas.dll",
            "bin/libgfortran-5.dll",
            "bin/libwinpthread-1.dll",
            "bin/crispasr.dll",
            "bin/unrelated.dll",
        ],
    );
    let bin = root.join("bin");

    remove_known_openblas_runtime_dlls(&bin)
        .expect("stale OpenBLAS runtime DLLs should be removed");

    assert!(!bin.join("openblas.dll").exists());
    assert!(!bin.join("libgfortran-5.dll").exists());
    assert!(!bin.join("libwinpthread-1.dll").exists());
    assert!(bin.join("crispasr.dll").is_file());
    assert!(bin.join("unrelated.dll").is_file());
}
