#[test]
fn real_prelude_storage_api_conformance() {
    let output = std::process::Command::new("node")
        .arg("--test")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/storage_api.test.js"
        ))
        .output()
        .expect("Node is required for the prelude conformance lane");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
