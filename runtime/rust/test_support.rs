//! Roundhouse Rust test-support runtime.
//!
//! Hand-written, shipped alongside generated code (copied in by the
//! Rust emitter as `src/test_support.rs`). Emitted controller tests
//! use this surface through the `TestResponseExt` trait, so the
//! assertion call sites stay stable while the implementation can
//! evolve.
//!
//! Phase 4d ships substring-match implementations of `assert_select`
//! — matches railcar's choice, zero extra deps, good-enough for the
//! scaffold blog's HTML assertions. A later upgrade to a real CSS
//! selector engine (the `scraper` crate, `html5ever`, or similar)
//! only needs to touch this file — emitted tests call the trait
//! methods and are insulated from the rendering strategy.

use axum_test::TestResponse;

pub trait TestResponseExt {
    /// `assert_response :success` — status 200 OK.
    fn assert_ok(&self);

    /// `assert_response :unprocessable_entity` — status 422.
    fn assert_unprocessable(&self);

    /// `assert_response <status>` for any concrete status.
    fn assert_status(&self, code: u16);

    /// `assert_redirected_to <path>`. Checks the response is a 3xx
    /// redirect and the `Location` header matches the expected path.
    /// Phase 4d substring-matches the path in Location to forgive
    /// absolute-vs-relative URL differences; a stricter check can
    /// swap in later without touching emitted tests.
    fn assert_redirected_to(&self, path: &str);

    /// `assert_select <selector>` — response body contains an
    /// element matching the (very) loose selector form. Substring
    /// match on the opening tag or `id=` / `class=` attribute
    /// fragment. Covers the scaffold blog's shapes:
    ///   "h1"             → contains "<h1"
    ///   "#articles"      → contains `id="articles"`
    ///   ".p-4"           → contains `class="... p-4 ..."` (as substring)
    ///   "form"           → contains "<form"
    fn assert_select(&self, selector: &str);

    /// `assert_select <selector>, <text>` — the `selector` check
    /// above *and* the response body contains `text`. Phase 4d
    /// doesn't verify the text lives inside the selector match
    /// (would require structural parsing); a later scraper-backed
    /// impl can tighten this.
    fn assert_select_text(&self, selector: &str, text: &str);

    /// `assert_select <selector>, minimum: N` — response body
    /// contains at least `n` occurrences of the selector fragment.
    /// Again substring-counted in Phase 4d.
    fn assert_select_min(&self, selector: &str, n: usize);
}

impl TestResponseExt for TestResponse {
    fn assert_ok(&self) {
        assert_eq!(self.status_code(), 200, "expected 200 OK");
    }

    fn assert_unprocessable(&self) {
        assert_eq!(self.status_code(), 422, "expected 422 Unprocessable Entity");
    }

    fn assert_status(&self, code: u16) {
        assert_eq!(self.status_code().as_u16(), code, "expected status {code}");
    }

    fn assert_redirected_to(&self, path: &str) {
        assert!(
            self.status_code().is_redirection(),
            "expected redirection, got {}",
            self.status_code(),
        );
        let location = self
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            location.contains(path),
            "expected Location to contain {path:?}, got {location:?}",
        );
    }

    fn assert_select(&self, selector: &str) {
        let body = self.text();
        let fragment = selector_fragment(selector);
        assert!(
            body.contains(&fragment),
            "expected body to match selector {selector:?} (looked for substring {fragment:?})",
        );
    }

    fn assert_select_text(&self, selector: &str, text: &str) {
        self.assert_select(selector);
        let body = self.text();
        assert!(
            body.contains(text),
            "expected body to contain text {text:?} under selector {selector:?}",
        );
    }

    fn assert_select_min(&self, selector: &str, n: usize) {
        let body = self.text();
        let fragment = selector_fragment(selector);
        let count = body.matches(&fragment).count();
        assert!(
            count >= n,
            "expected at least {n} matches for selector {selector:?} (fragment {fragment:?}), got {count}",
        );
    }
}

/// Map a loose selector to a substring fragment that probably appears
/// in matching HTML. Phase 4d: handles `#id`, `.class`, `tag`, and
/// the first element of compound selectors like `"#comments .p-4"`
/// (splits on whitespace, picks the first chunk). Every match is a
/// substring search — false positives are possible but the blog's
/// HTML is narrow enough that the tests are a reliable signal.
fn selector_fragment(selector: &str) -> String {
    let first = selector.split_whitespace().next().unwrap_or("");
    if let Some(id) = first.strip_prefix('#') {
        format!("id=\"{id}\"")
    } else if let Some(class) = first.strip_prefix('.') {
        format!("{class}\"")
    } else {
        format!("<{first}")
    }
}
