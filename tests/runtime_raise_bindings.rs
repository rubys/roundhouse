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
