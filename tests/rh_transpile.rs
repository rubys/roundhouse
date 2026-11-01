#[cfg(unix)]
#[test]
fn transpile_cli_uses_a_staged_compiler_or_cargo() {
    let result = std::process::Command::new("ruby")
        .arg("tests/rh_transpile_test.rb")
        .output();
    let result = match result {
        Ok(result) => result,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: Ruby not available on PATH (required for bin/rh contract tests)");
            return;
        }
        Err(error) => panic!("invoke Ruby for bin/rh transpile tests: {error}"),
    };
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
