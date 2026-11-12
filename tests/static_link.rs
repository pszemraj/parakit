//! Native consumer metadata used without repository-local Cargo configuration.

#[path = "../build/static_link.rs"]
mod static_link;

#[test]
fn selects_release_consumer_and_requests_codemodel() {
    const MODE: &str = "PARAKIT_STATIC_LINK_TEST_MODE";
    const ROOT: &str = "PARAKIT_STATIC_LINK_TEST_ROOT";
    if let Ok(mode) = std::env::var(MODE) {
        static_link::emit(
            std::path::Path::new(&std::env::var_os(ROOT).unwrap()),
            mode == "windows",
        )
        .unwrap();
        return;
    }
    let root =
        std::path::Path::new("target/tmp/static-link-tests").join(std::process::id().to_string());
    static_link::request(&root).unwrap();
    assert!(root
        .join("build/.cmake/api/v1/query/codemodel-v2")
        .is_file());
    let reply = root.join("build/.cmake/api/v1/reply");
    std::fs::create_dir_all(&reply).unwrap();
    for (name, contents) in [
        (
            "index-1.json",
            r#"{"reply":{"codemodel-v2":{"jsonFile":"model.json"}}}"#,
        ),
        (
            "model.json",
            r#"{"configurations":[{"name":"Debug","targets":[]},{"name":"Release","targets":[{"name":"parakit-native-link","jsonFile":"consumer.json"}]}]}"#,
        ),
        (
            "consumer.json",
            r#"{"link":{"commandFragments":[{"role":"libraries","fragment":"libcrispasr.a -lm"}]}}"#,
        ),
    ] {
        std::fs::write(reply.join(name), contents).unwrap();
    }
    std::fs::write(root.join("build/parakit-cxx-runtime.txt"), "stdc++\n").unwrap();
    let bin_search = format!(
        "cargo:rustc-link-search=native={}",
        root.join("bin").display()
    );
    for mode in ["unix", "windows"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "selects_release_consumer_and_requests_codemodel",
                "--nocapture",
            ])
            .env(MODE, mode)
            .env(ROOT, &root)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            stdout.lines().any(|line| line == bin_search),
            mode == "windows",
            "{mode}: {stdout}"
        );
        assert!(stdout.contains("cargo:rustc-link-lib=static=crispasr"));
        assert!(stdout.contains("cargo:rustc-link-lib=stdc++"));
    }
    std::fs::remove_dir_all(root).unwrap();
}
