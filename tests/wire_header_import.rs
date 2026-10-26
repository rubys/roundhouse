use std::process::Command;

#[test]
fn parsed_wire_headers_cannot_populate_server_env_keys() {
    let output = Command::new("ruby")
        .arg("tests/wire_header_import.rb")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Ruby is on PATH");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "80 parsed wire headers and scheme isolation pass\n"
    );
}
