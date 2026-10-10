use std::{fs, path::PathBuf, process::Command};

const HTTP_RUNTIME: &str = include_str!("../runtime/rust/http.rs");

#[test]
#[ignore = "compiles the emitted Rust HTTP runtime in an isolated Cargo crate"]
fn request_cookie_transport_is_request_scoped_and_appends_headers() {
    let root: PathBuf = std::env::temp_dir().join("roundhouse-rust-http-cookie-transport");
    let src = root.join("src");
    if root.exists() {
        fs::remove_dir_all(&root).expect("remove prior scratch crate");
    }
    fs::create_dir_all(&src).expect("create scratch source directory");
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "roundhouse-rust-http-cookie-transport"
version = "0.1.0"
edition = "2024"

[dependencies]
axum = "0.8"
serde_json = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }

[dev-dependencies]
axum-test = "18"
time = ">=0.3, <0.3.48"
"#,
    )
    .expect("write scratch manifest");
    fs::write(
        src.join("lib.rs"),
        r#"pub mod flash {
    use std::collections::HashMap;

    pub struct Flash;

    impl Flash {
        pub fn from_persisted(_: Option<&HashMap<String, String>>) -> Self {
            Self
        }
    }
}

pub mod http;
"#,
    )
    .expect("write minimal crate root");
    fs::write(src.join("http.rs"), HTTP_RUNTIME).expect("copy emitted HTTP runtime");

    let target = std::env::temp_dir().join("roundhouse-rust-http-cookie-target");
    let output = Command::new("cargo")
        .args(["test", "--lib", "cookie", "--", "--nocapture"])
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", target)
        .env("CARGO_PROFILE_DEV_DEBUG", "0")
        .env("CARGO_PROFILE_TEST_DEBUG", "0")
        .output()
        .expect("run isolated emitted-runtime tests");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "isolated emitted HTTP runtime test failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout
            .contains("cookie_transport_is_request_scoped_and_appends_without_overwriting ... ok")
            && stdout.contains("cookie_transport_rejects_invalid_response_header_values ... ok")
            && stdout
                .contains("request_context_middleware_flushes_cookies_after_the_handler ... ok"),
        "cookie transport regression tests did not all execute:\n{stdout}"
    );
}
