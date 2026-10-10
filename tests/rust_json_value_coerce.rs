//! Pin the rust emit of RBS unions that collapse to `serde_json::Value`.
//!
//! `scripts/compare rust` compiles the transpiled `view_helpers.rb`, not
//! the handwritten `runtime/rust/view_helpers.rs`. Heterogeneous RBS
//! unions (`String | Symbol`, `String | Array[untyped]`, `String |
//! Integer | Float | bool | nil`) used to emit `&str` / `Value` /
//! `.is_none()` combinations that rustc rejected (E0308 / E0599).

use roundhouse::analyze::Analyzer;
use roundhouse::emit::{rust, EmittedFile};
use roundhouse::ingest::ingest_app;

fn file(files: &[EmittedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .unwrap_or_else(|| panic!("missing {suffix}"))
        .content
        .clone()
}

fn method_body<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("pub fn {name}("))
        .unwrap_or_else(|| panic!("missing fn {name} in:\n{src}"));
    let rest = &src[start..];
    let end = rest
        .find("\n    pub fn ")
        .unwrap_or(rest.len());
    &rest[..end]
}

fn emit_real_blog_rust() -> Vec<EmittedFile> {
    let mut app = ingest_app(roundhouse::fixtures::real_blog()).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    rust::emit(&app)
}

#[test]
fn view_helpers_nil_pred_and_value_string_coercions_typecheck() {
    let files = emit_real_blog_rust();
    let vh = file(&files, "view_helpers.rs");
    let http = file(&files, "http.rs");

    assert!(
        http.contains("impl<T: RubyToS> RubyToS for Option<T>"),
        "Ruby nil/String union coercions need an Option-aware RubyToS bridge:\n{http}"
    );

    let optional = method_body(&vh, "optional_value_attr");
    assert!(
        optional.contains("is_null()") && !optional.contains("is_none()"),
        "optional_value_attr must use Value::is_null, not Option::is_none:\n{optional}"
    );
    assert!(
        optional.contains("ruby_to_s"),
        "optional_value_attr must to_s Value via ruby_to_s (no JSON quotes):\n{optional}"
    );

    let escape = method_body(&vh, "escape_or_empty");
    assert!(
        escape.contains("is_null()") && !escape.contains("is_none()"),
        "escape_or_empty must use Value::is_null, not Option::is_none:\n{escape}"
    );

    let form_with = method_body(&vh, "form_with");
    assert!(
        form_with.contains("method_override_input") && form_with.contains("ruby_to_s"),
        "form_with must stringify the Hash fetch before method_override_input:\n{form_with}"
    );
    assert!(
        !form_with.contains("method_override_input(opts"),
        "gradual opts must not cross method_override_input:\n{form_with}"
    );

    let articles = file(&files, "views/articles.rs");
    assert!(
        articles.contains("turbo_stream_from(serde_json::Value::from(\"articles\")"),
        "string turbo_stream_from sites must box into Value:\n{articles}"
    );
    assert!(
        articles.contains("turbo_stream_from(serde_json::Value::from(format!("),
        "interpolated turbo_stream_from sites must box into Value:\n{articles}"
    );
}
