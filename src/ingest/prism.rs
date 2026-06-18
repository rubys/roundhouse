//! Prism parse wrapper that routes syntax errors into a diagnostic sink.
//!
//! `ruby_prism::parse` is error-recovering: on a syntax error it still
//! returns a (partial) AST via `node()`, and every per-file ingester
//! walks that tree regardless. Historically the `errors()` list was
//! dropped on the floor, so a genuine syntax error in app source
//! vanished silently and resurfaced — if at all — as a confusing
//! downstream `Unsupported` gap on a node Prism never actually built.
//!
//! [`parse`] is a drop-in replacement for `ruby_prism::parse` that, in
//! addition to returning the same [`ParseResult`], converts each Prism
//! error into a [`Diagnostic`] (kind [`crate::diagnostic::DiagnosticKind::Parse`])
//! carrying a real [`Span`] and pushes it into a thread-local sink. The
//! `ParseResult` is returned unchanged, so callers keep walking the
//! recovered AST exactly as before — reporting is purely additive.
//!
//! The lifecycle mirrors the emit diagnostic sink ([`crate::emit::diagnostics`])
//! and the [`sources`] registry: [`scope`] installs a fresh buffer for
//! the duration of one whole-app ingest (saving/restoring any outer
//! buffer so scopes nest), runs the closure, and returns the collected
//! diagnostics alongside its result. Outside any scope [`push`] is a
//! no-op — standalone unit-test ingests (`ingest_ruby_program` called
//! directly) parse without collecting, same as before.
//!
//! The span's [`crate::span::FileId`] comes from the [`sources`] registry,
//! which every per-file ingester populates (via `sources::register`)
//! immediately before calling `parse`, so the file is already registered
//! by the time the wrapper looks it up. Call sites with no registered
//! path resolve to the synthetic `FileId(0)` and render message-only —
//! the same graceful fallback the rest of the diagnostics use.

use std::cell::RefCell;

use ruby_prism::ParseResult;

use crate::diagnostic::Diagnostic;
use crate::span::Span;

use super::sources;

thread_local! {
    /// Active parse-diagnostic buffer. `None` when no [`scope`] is
    /// installed (pushes drop); `Some` for the duration of a scope.
    static PARSE_DIAGS: RefCell<Option<Vec<Diagnostic>>> = const { RefCell::new(None) };
}

/// Run `f` with a fresh parse-diagnostic buffer installed, returning its
/// result paired with every parse diagnostic recorded during the call.
/// Any outer buffer is saved and restored, so scopes nest and run in
/// sequence without leaking diagnostics into one another.
pub fn scope<T>(f: impl FnOnce() -> T) -> (T, Vec<Diagnostic>) {
    let prev = PARSE_DIAGS.with(|c| c.borrow_mut().replace(Vec::new()));
    let result = f();
    let collected = PARSE_DIAGS.with(|c| {
        std::mem::replace(&mut *c.borrow_mut(), prev).unwrap_or_default()
    });
    (result, collected)
}

/// Push a parse diagnostic into the active buffer. A no-op outside a
/// [`scope`].
fn push(d: Diagnostic) {
    PARSE_DIAGS.with(|c| {
        if let Some(buf) = c.borrow_mut().as_mut() {
            buf.push(d);
        }
    });
}

/// Drop-in replacement for [`ruby_prism::parse`]: parse `source` and, for
/// every syntax error Prism reports, record a [`Diagnostic`] against
/// `file` before returning the (error-recovered) [`ParseResult`]
/// unchanged. `file` is the source path used to resolve each
/// diagnostic's [`Span`] against the [`sources`] registry; pass `""`
/// (or any unregistered path) to get a message-only diagnostic.
pub fn parse<'pr>(source: &'pr [u8], file: &str) -> ParseResult<'pr> {
    let result = ruby_prism::parse(source);
    let errors = result.errors();
    // Peek before resolving the file id so the registry lookup is
    // skipped on the common (no-error) path.
    let mut errors = errors.peekable();
    if errors.peek().is_some() {
        let file_id = sources::file_id(file);
        for err in errors {
            let loc = err.location();
            let span = Span {
                file: file_id,
                start: loc.start_offset() as u32,
                end: loc.end_offset() as u32,
            };
            push(Diagnostic::parse(span, err.message()));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticKind;

    #[test]
    fn clean_source_records_nothing() {
        let (result, diags) = scope(|| parse(b"x = 1\n", "a.rb"));
        assert_eq!(result.errors().count(), 0);
        assert!(diags.is_empty());
    }

    #[test]
    fn syntax_error_is_recorded_as_a_parse_diagnostic() {
        // Register the file so the span resolves to a real id.
        sources::reset();
        sources::register("broken.rb", "def\n");
        let (result, diags) = scope(|| parse(b"def\n", "broken.rb"));
        // Prism still hands back a (partial) AST — callers keep walking.
        assert!(result.node().as_program_node().is_some());
        // And the dropped error is now a collected Parse diagnostic.
        assert!(!diags.is_empty());
        assert!(matches!(diags[0].kind, DiagnosticKind::Parse { .. }));
        assert_eq!(diags[0].code(), "parse");
        assert!(!diags[0].span.is_synthetic());
        sources::reset();
    }

    #[test]
    fn push_outside_scope_is_a_noop() {
        // Must not panic and must not retain anything for the next scope.
        let _ = parse(b"def\n", "");
        let (_, diags) = scope(|| ());
        assert!(diags.is_empty());
    }
}
