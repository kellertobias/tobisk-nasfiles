#![no_main]

use libfuzzer_sys::fuzz_target;
use std::fs;

fuzz_target!(|data: &str| {
    // Create a temporary root directory
    let dir = tempfile::tempdir().unwrap();

    // Create some content so the resolver has something to work with
    fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
    fs::write(dir.path().join("a/file.txt"), "x").unwrap();
    fs::write(dir.path().join("a/b/file.txt"), "x").unwrap();

    // Fuzz the resolve function with arbitrary input
    // The critical property: if resolve() returns Ok(path),
    // the path MUST be inside the root directory.
    if let Ok(resolved) = nasfiles_core::safe_path::resolve(dir.path(), data) {
        let canonical_root = dir.path().canonicalize().unwrap();
        assert!(
            resolved.starts_with(&canonical_root),
            "ESCAPE: resolve({:?}) returned {:?} which is outside root {:?}",
            data,
            resolved,
            canonical_root
        );
    }
    // Errors are fine — we're looking for escapes, not crashes
});
