//! Phase 3 acceptance: lower real-blog's three `*.json.jbuilder`
//! templates and pin their lowered method shape via the Ruby emit.
//! Pre-Phase 4 (live byte-equivalence vs Rails), this is the inner
//! checkpoint — if the IR shape regresses, this fires before the
//! external compare-tool catches it.

use std::path::Path;

use roundhouse::dialect::LibraryClass;
use roundhouse::emit::ruby::emit_method;
use roundhouse::ingest::ingest_app;
use roundhouse::lower::lower_jbuilder_to_library_classes;

fn lowered_articles() -> Vec<LibraryClass> {
    let app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");
    lower_jbuilder_to_library_classes(&app.views, &app, Vec::new())
}

fn method_body(lcs: &[LibraryClass], method: &str) -> String {
    let m = lcs
        .iter()
        .flat_map(|l| l.methods.iter())
        .find(|m| m.name.as_str() == method)
        .unwrap_or_else(|| panic!("no method named {method}"));
    emit_method(m)
}

#[test]
fn three_jbuilder_templates_produce_three_methods() {
    let lcs = lowered_articles();
    let methods: Vec<String> = lcs
        .iter()
        .flat_map(|lc| lc.methods.iter().map(|m| m.name.as_str().to_string()))
        .collect();
    let mut sorted = methods.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "article_json".to_string(),
            "index_json".to_string(),
            "show_json".to_string(),
        ],
    );
}

#[test]
fn article_partial_emits_extract_plus_url_pair() {
    let lcs = lowered_articles();
    let body = method_body(&lcs, "article_json");
    // Expect: io = String.new ; io << "{" ;
    //   io << "\"id\":" ; io << JsonBuilder.encode_value(article.id) ; …
    //   io << "}" ; io
    assert!(body.contains("io = String.new"), "body:\n{body}");
    assert!(body.contains("io << \"{\""), "missing object open: {body}");
    assert!(body.contains("io << \"\\\"id\\\":\""), "missing id key: {body}");
    assert!(
        body.contains("JsonBuilder.encode_value(article.id)"),
        "missing id encode: {body}"
    );
    assert!(
        body.contains("JsonBuilder.encode_value(article.title)"),
        "missing title encode: {body}"
    );
    assert!(
        body.contains("JsonBuilder.encode_value(article.body)"),
        "missing body encode: {body}"
    );
    // datetime columns route through `encode_datetime` for Rails-
    // canonical ISO 8601 output — reading the `<col>_raw` storage
    // String, not the parsing `<col>` reader: the string→string
    // reformat is exact and skips a native Time parse→format
    // round-trip per row.
    assert!(
        body.contains("JsonBuilder.encode_datetime(article.created_at_raw)"),
        "missing created_at datetime encode: {body}"
    );
    assert!(
        body.contains("JsonBuilder.encode_datetime(article.updated_at_raw)"),
        "missing updated_at datetime encode: {body}"
    );
    // The trailing `json.url article_url(...)` becomes the "url" pair
    // and `article_url(article, format: :json)` rewrites to
    // `RouteHelpers.article_path(article.id) + ".json"` — the format
    // kwarg threads through as a literal suffix so the lowered output
    // matches Rails' `/articles/1.json` self-link shape. Scheme+host
    // is still dropped (per-deployment noise the comparator strips).
    assert!(body.contains("io << \"\\\"url\\\":\""), "missing url key: {body}");
    assert!(
        body.contains("RouteHelpers.article_path(article.id) + \".json\""),
        "missing route-helper + format-suffix rewrite: {body}"
    );
    assert!(body.contains("io << \",\""), "missing pair separator: {body}");
    assert!(body.contains("io << \"}\""), "missing object close: {body}");
}

#[test]
fn index_emits_array_with_map_join() {
    let lcs = lowered_articles();
    let body = method_body(&lcs, "index_json");
    assert!(body.contains("io << \"[\""), "missing array open: {body}");
    assert!(
        body.contains("articles.map") && body.contains(".join(\",\")"),
        "missing map+join shape: {body}"
    );
    assert!(
        body.contains("Views::Articles.article_json(article)"),
        "missing partial dispatch: {body}"
    );
    assert!(body.contains("io << \"]\""), "missing array close: {body}");
}

#[test]
fn show_emits_single_partial_call() {
    let lcs = lowered_articles();
    let body = method_body(&lcs, "show_json");
    assert!(body.contains("io = String.new"), "{body}");
    assert!(
        body.contains("Views::Articles.article_json(article)"),
        "missing partial call: {body}"
    );
    // show shouldn't open an object or array — only the bare partial call.
    assert!(!body.contains("io << \"{\""), "show should not emit object open: {body}");
    assert!(!body.contains("io << \"[\""), "show should not emit array open: {body}");
}
