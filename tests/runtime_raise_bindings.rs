//! Native raises bind the error kinds used by shared Ruby methods.

#[allow(dead_code)]
#[path = "../runtime/rust/errors_ext.rs"]
mod rust_errors;

fn sanitize_sql_like_only(
    out_path: &str,
    classes: Vec<roundhouse::dialect::LibraryClass>,
) -> Vec<roundhouse::dialect::LibraryClass> {
    if out_path != "src/main/kotlin/ActiveRecordBase.kt" {
        return Vec::new();
    }
    classes
        .into_iter()
        .map(|mut class| {
            class
                .methods
                .retain(|method| method.name.as_str() == "sanitize_sql_like");
            class
        })
        .collect()
}

#[test]
fn sanitize_sql_like_error_references_have_rust_runtime_bindings() {
    let files = roundhouse::emit::rust::emit(&roundhouse::App::default());
    let base = &files
        .into_iter()
        .find(|file| file.path == std::path::Path::new("src/active_record_base.rs"))
        .expect("ActiveRecord::Base runtime")
        .content;
    for (name, kind, expected) in [
        (
            "RuntimeError",
            rust_errors::RuntimeError,
            "FrameworkError::Runtime",
        ),
        (
            "IndexError",
            rust_errors::IndexError,
            "FrameworkError::Index",
        ),
    ] {
        assert!(base.contains(&format!("raise({name},")), "{name}: {base}");
        assert!(
            base.contains(&format!("use crate::errors_ext::{name};")),
            "{name}: {base}"
        );
        let failure = std::panic::catch_unwind(|| rust_errors::raise(kind, "replacement escape"))
            .expect_err("raise must diverge");
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .expect("panic message");
        assert_eq!(message, expected);
    }
}

#[test]
fn sanitize_sql_like_errors_use_kotlin_standard_exceptions() {
    let units = roundhouse::runtime_loader::kotlin_units(sanitize_sql_like_only)
        .expect("emit shared Kotlin runtime");
    let base = &units
        .into_iter()
        .find(|unit| unit.out_path == std::path::Path::new("src/main/kotlin/ActiveRecordBase.kt"))
        .expect("ActiveRecord::Base runtime")
        .content;
    for (ruby, kotlin) in [
        ("RuntimeError", "RuntimeException"),
        ("IndexError", "IndexOutOfBoundsException"),
    ] {
        assert!(
            base.contains(&format!("throw {kotlin}(")),
            "{kotlin}: {base}"
        );
        assert!(
            !base.contains(&format!("throw {ruby}(")),
            "undefined {ruby}: {base}"
        );
    }
}

#[test]
fn emitted_rust_like_escaping_compiles_and_runs_with_a_deadline() {
    use std::fs;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let files = roundhouse::emit::rust::emit(&roundhouse::App::default());
    let base = &files
        .iter()
        .find(|file| file.path == std::path::Path::new("src/active_record_base.rs"))
        .expect("ActiveRecord::Base runtime")
        .content;
    let start = base
        .find("    pub fn sanitize_sql_like(")
        .expect("sanitizer");
    let end = start + base[start..].find("\n    }\n").expect("method end") + 6;
    let method = &base[start..end];
    let cases = [
        ("", "\\", ""),
        ("plain text", "\\", "plain text"),
        ("100%_done", "\\", "100\\%\\_done"),
        ("back\\slash%_", "\\", "back\\\\slash\\%\\_"),
        ("100%_!", "!", "100!%!_!!"),
        ("100%_", "%", "100%%%_"),
        ("100%_", "_", "100_%__"),
        ("ab%_ab", "ab", "ababab%ab_abab"),
        ("100%_", "", "100%_"),
        ("café_50%", "!", "café!_50!%"),
        ("é%_é", "é", "ééé%é_éé"),
        ("東🎉%_東🎉", "東🎉", "東🎉東🎉東🎉%東🎉_東🎉東🎉"),
        ("x%_", "\\\\", "x\\%\\_"),
        ("x%_", "\\0", "x%_"),
        ("x%_", "\\&", "x%_"),
        ("x%_", "\\1", "x%_"),
        ("x%_", "\\9", "x%_"),
        ("x%_", "\\+", "x%_"),
        ("x%_", "\\10", "x0%0_"),
        ("x%_", "\\`", "xx%x%_"),
        ("x%_", "\\'", "x%_%__"),
        ("x%_", "\\q", "x\\q%\\q_"),
    ];
    let errors =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/rust/errors_ext.rs");
    let source = format!(
        r#"#[allow(dead_code)]
#[path = {errors:?}]
mod errors_ext;
use errors_ext::{{raise, RuntimeError, IndexError}};
struct LikeProbe;
impl LikeProbe {{
{method}
}}
fn main() {{
    for (value, marker, expected) in {cases:?} {{
        assert_eq!(LikeProbe::sanitize_sql_like(value, marker), expected, "{{value:?}}/{{marker:?}}");
    }}
    std::panic::set_hook(Box::new(|_| {{}}));
    for (marker, expected) in [
        ("\\k<name>", "FrameworkError::Index"),
        ("\\k<>", "FrameworkError::Index"),
        ("\\k<name", "FrameworkError::Runtime"),
    ] {{
        assert_eq!(LikeProbe::sanitize_sql_like("plain", marker), "plain");
        let error = std::panic::catch_unwind(|| LikeProbe::sanitize_sql_like("x%_", marker))
            .expect_err("named replacement must raise");
        let message = error.downcast_ref::<String>().map(String::as_str)
            .or_else(|| error.downcast_ref::<&str>().copied()).expect("panic message");
        assert_eq!(message, expected);
    }}
}}
"#
    );
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let scratch =
        Scratch(std::env::temp_dir().join(format!("rh-like-rust-{}", std::process::id())));
    fs::create_dir(&scratch.0).expect("create native probe directory");
    let input = scratch.0.join("probe.rs");
    let binary = scratch.0.join("probe");
    fs::write(&input, source).expect("write emitted sanitizer");
    let compiled = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("--edition=2021")
        .arg(&input)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("compile emitted sanitizer");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let log_path = scratch.0.join("stderr");
    let mut child = Command::new(binary)
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log_path).expect("native stderr"))
        .spawn()
        .expect("run emitted sanitizer");
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll native sanitizer") {
            assert!(
                status.success(),
                "{}",
                fs::read_to_string(&log_path).unwrap()
            );
            break;
        }
        if started.elapsed() > Duration::from_secs(3) {
            child.kill().expect("stop hung sanitizer");
            child.wait().expect("reap sanitizer");
            panic!("emitted sanitizer exceeded its 3-second deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
