//! Integration coverage for Unix OpenBLAS layout detection.

mod common;
#[path = "../build/unix_openblas.rs"]
mod unix_openblas;

use unix_openblas::find_unix_openblas;

struct Case {
    name: &'static str,
    files: &'static [&'static str],
    expected: Option<(&'static str, &'static str)>,
}

const CASES: &[Case] = &[
    Case {
        name: "source-install",
        files: &["include/cblas.h", "lib/libopenblas.so"],
        expected: Some(("include", "lib/libopenblas.so")),
    },
    Case {
        name: "conda-static",
        files: &["include/openblas/cblas.h", "lib/libopenblas.a"],
        expected: Some(("include/openblas", "lib/libopenblas.a")),
    },
    Case {
        name: "debian-multiarch",
        files: &[
            "include/x86_64-linux-gnu/openblas-pthread/cblas.h",
            "lib/x86_64-linux-gnu/libopenblas.so.0",
        ],
        expected: Some((
            "include/x86_64-linux-gnu/openblas-pthread",
            "lib/x86_64-linux-gnu/libopenblas.so.0",
        )),
    },
    Case {
        name: "missing-header",
        files: &["lib/libopenblas.so"],
        expected: None,
    },
    Case {
        name: "missing-library",
        files: &["include/cblas.h"],
        expected: None,
    },
];

#[test]
fn detects_common_unix_openblas_layouts() {
    for case in CASES {
        let root =
            common::fixture_root_with_files("unix-openblas-layout-tests", case.name, case.files);
        let found = find_unix_openblas(&root);

        match case.expected {
            Some((include_dir, library)) => {
                let found = found.unwrap_or_else(|| panic!("{} should be detected", case.name));
                assert_eq!(found.include_dir, root.join(include_dir), "{}", case.name);
                assert_eq!(found.library, root.join(library), "{}", case.name);
            }
            None => assert!(found.is_none(), "{} should be rejected", case.name),
        }
    }
}
