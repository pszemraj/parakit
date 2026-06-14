use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn fixture_root(namespace: &str, name: &str) -> PathBuf {
    let root = Path::new("target")
        .join("tmp")
        .join(namespace)
        .join(format!("{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    root
}

pub(crate) fn touch(path: &Path) {
    fs::create_dir_all(path.parent().expect("fixture file should have parent"))
        .expect("fixture parent should be created");
    fs::write(path, b"").expect("fixture file should be created");
}
