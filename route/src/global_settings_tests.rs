#[test]
fn service_credentials_have_global_routes_and_exact_shared_keys() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("../petal.toml")).unwrap();
    assert!(root.join("files/settings/api-key.rs").is_file());
    assert!(
        !root
            .join("files/settings/[wallet]/[index]/api-key.rs")
            .exists()
    );
    assert!(root.join("files/settings/status.json.rs").is_file());
    assert!(
        !root
            .join("files/settings/[wallet]/[index]/status.json.rs")
            .exists()
    );
    assert!(manifest.contains(r#"shared_keys = ["secrets/credentials/enso-api-key"]"#));
}
