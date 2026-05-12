//! Phase 1 sanity test: confirm `.json.jbuilder` templates are picked up
//! during whole-app ingest with `format == "json"` and the expected
//! names.

use std::path::Path;

use roundhouse::ingest::ingest_app;

#[test]
fn real_blog_jbuilder_views_ingested() {
    let app = ingest_app(Path::new("fixtures/real-blog")).expect("ingest");

    let json_views: Vec<_> = app
        .views
        .iter()
        .filter(|v| v.format.as_str() == "json")
        .map(|v| v.name.as_str().to_string())
        .collect();

    let mut sorted = json_views.clone();
    sorted.sort();

    assert_eq!(
        sorted,
        vec![
            "articles/_article".to_string(),
            "articles/index".to_string(),
            "articles/show".to_string(),
        ],
        "expected real-blog's three articles/*.json.jbuilder templates"
    );
}

