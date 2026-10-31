//! A template's flash params, handed on to the partials it renders, must
//! not be moved in the Rust target.
//!
//! A partial reads its caller's `notice`/`alert` (#1503), so real-blog's
//! `articles/index` reads `notice` itself and then passes it to each
//! `_article` it renders inside a `for_each` closure. Two moves broke
//! that in Rust:
//!
//! - `<%= notice %>` emitted `notice.map(..)`, which consumes the
//!   `Option<String>` the render reads after it (E0382);
//! - the read inside the closure was `notice`'s last lexical one, so it
//!   moved, but a closure run per item cannot move a capture (E0507).

use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app;

fn emitted_articles_views() -> String {
    let mut app = ingest_app(roundhouse::fixtures::real_blog()).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    rust::emit(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("views/articles.rs"))
        .expect("emitted articles views")
        .content
}

#[test]
fn a_flash_param_rendered_as_text_is_borrowed() {
    let src = emitted_articles_views();
    assert!(
        src.contains("notice.as_ref().map(|v| v.to_string()).unwrap_or_default()"),
        "`<%= notice %>` must borrow the param:\n{src}"
    );
    assert!(!src.contains("(notice.map("), "`<%= notice %>` must not move the param:\n{src}");
}

#[test]
fn a_flash_param_captured_by_a_collection_render_is_cloned() {
    let src = emitted_articles_views();
    assert!(
        src.contains("for_each(|a| { io.push_str(&Articles::article(a.clone(), notice.clone(), alert.clone())); })"),
        "the partial render inside the closure must clone the captured flash:\n{src}"
    );
}
