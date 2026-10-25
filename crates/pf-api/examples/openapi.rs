fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&pf_api::openapi::document()).unwrap()
    );
}
