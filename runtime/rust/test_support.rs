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
        let doc = dom_parse(&self.text());
        assert!(
            !dom_select(&doc, selector).is_empty(),
            "expected body to match selector {selector:?}",
        );
    }

    fn assert_select_text(&self, selector: &str, text: &str) {
        let doc = dom_parse(&self.text());
        let nodes = dom_select(&doc, selector);
        assert!(
            !nodes.is_empty(),
            "expected body to match selector {selector:?}",
        );
        assert!(
            nodes.iter().any(|n| dom_text(n).contains(text)),
            "expected text {text:?} under selector {selector:?}",
        );
    }

    fn assert_select_min(&self, selector: &str, n: usize) {
        let doc = dom_parse(&self.text());
        let count = dom_select(&doc, selector).len();
        assert!(
            count >= n,
            "expected at least {n} matches for selector {selector:?}, got {count}",
        );
    }
}

// ── Dom primitive surface (the assert_select substrate) ────────────
//
// The HTML-query contract `assert_select` lowers to, shared in shape
// with the Ruby/TS/Python/Elixir twins (cross-target contract in
// runtime/spinel/test/test_helper.rbs). Stub: the substring matcher
// dressed as a Dom — `dom_select` fabricates one synthetic node (the
// whole document) per fragment occurrence and `dom_text` returns it
// verbatim, so presence / `minimum:` / text checks degrade to exactly
// the pre-contract behavior. The upgrade path is to swap these three
// functions for a `scraper`/`html5ever`-backed engine — real nodes,
// real CSS selectors — touching only this file; the `TestResponseExt`
// call sites and every other target stay put.

/// Parsed-document handle. Stub: the document *is* its html string.
type DomDoc = String;
/// Matched-node handle. Stub: the html the node was found in.
type DomNode = String;

/// Parse an HTML document.
fn dom_parse(html: &str) -> DomDoc {
    html.to_string()
}

/// Nodes matching `selector` within `root` (a document or node). Stub:
/// one synthetic node (the root's html) per substring-fragment
/// occurrence, so nested selects re-scan the whole string.
fn dom_select(root: &str, selector: &str) -> Vec<DomNode> {
    let fragment = selector_fragment(selector);
    root.match_indices(&fragment).map(|_| root.to_string()).collect()
}

/// Concatenated descendant text of a node. Stub: the node's html
/// verbatim (so a content check degrades to a body-substring check).
fn dom_text(node: &DomNode) -> &str {
    node
}

/// Map a loose selector to a substring fragment that probably appears
/// in matching HTML. The stub's selector rule (replaced by a real CSS
/// engine on upgrade): handles `#id`, `.class`, `tag`, and the first
/// element of compound selectors like `"#comments .p-4"` (splits on
/// whitespace, picks the first chunk).
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
