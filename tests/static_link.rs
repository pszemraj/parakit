//! Native consumer metadata used without repository-local Cargo configuration.

#[path = "../build/static_link.rs"]
mod static_link;

#[test]
fn selects_release_consumer_and_requests_codemodel() {
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
    static_link::emit(&root, false).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
