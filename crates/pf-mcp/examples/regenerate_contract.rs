fn main() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/agent-contract.md");
    let document = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, pf_mcp::contract::regenerate(&document)).unwrap();
    let snapshot =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tools.snapshot.json");
    std::fs::write(
        snapshot,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&pf_mcp::tools()).unwrap()
        ),
    )
    .unwrap();
}
