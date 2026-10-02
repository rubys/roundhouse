//! Per-thread source-file registry: the `FileId` ↔ (path, text) table
//! behind real `Span`s.
//!
//! Ingest is a deep recursive descent (`ingest_expr` alone recurses
//! through every expression node), so threading a registry handle
//! through every signature would touch hundreds of call sites for a
//! value consulted only at `Span` construction. Instead the table
//! lives in a thread-local — the same idiom survey mode and the emit
//! diagnostic sink already use.
//!
//! Lifecycle: [`crate::ingest::app::ingest_app_with_vfs`] calls
//! [`reset`] on entry, each per-file ingester [`register`]s the text
//! it actually parses on the way in, and the walker [`drain`]s the
//! table into `App::sources` on the way out. `FileId`s are 1-based —
//! `FileId(0)` stays the synthetic sentinel (`Span::synthetic`).
//!
//! Registration records the text spans index. For plain Ruby that's
//! the text prism parsed; for `.html.erb` views it's the raw template
//! — view ingest registers the template first (first-text-wins), then
//! translates the compiled-Ruby spans back to template offsets via
//! `erb::translate_spans`, so view diagnostics report real template
//! lines and columns.
//!
//! Standalone ingest entry points (unit tests calling
//! `ingest_ruby_program` directly) also register; without a
//! surrounding `reset`/`drain` the table just accumulates on the test
//! thread, which is harmless. Re-registering a path keeps the first
//! text — ids stay stable for spans already handed out.
//!
//! A label starting with `<` (`"<delegate>"`, `"<channel>"`, …) never
//! gets a slot: [`register`] refuses it outright, returning the
//! synthetic `FileId(0)`. Those labels mark synthesized Ruby that a
//! lowering pass (`ingest::delegate` and its siblings) re-ingests to
//! expand a macro into real methods — text that was never in the app
//! and has no stable place in `App::sources`. Registering one anyway
//! is how a synthesized parse failure once rendered against an
//! unrelated real file: [`ingest_app_with_vfs`][super::app::ingest_app_with_vfs]
//! takes one [`snapshot`] mid-ingest (for a pass that needs to read the
//! app's real source text before the synthesizing passes run) and one
//! real [`drain`] at the end; a real file registered in between would
//! be safe either way, but a synthesized one that slipped past its own
//! `prism::scope` isolation would have collided with a fresh low
//! `FileId` once the table's next drain emptied out from under it.
//! This refusal is the backstop for that case — it holds regardless of
//! ordering.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::span::{FileId, SourceFile};

thread_local! {
    static SOURCES: RefCell<Registry> = RefCell::new(Registry::default());
}

#[derive(Default)]
struct Registry {
    files: Vec<SourceFile>,
    by_path: HashMap<String, FileId>,
}

/// Clear the registry for a fresh whole-app ingest.
pub fn reset() {
    SOURCES.with(|s| *s.borrow_mut() = Registry::default());
}

/// Record a source file and return its `FileId` (1-based). Idempotent
/// by path: a second registration of the same path returns the
/// existing id and keeps the first text.
///
/// A `path` starting with `<` is refused — it never occupies a slot,
/// and this always returns the synthetic `FileId(0)` instead. Those
/// labels mark synthesized (not-really-a-file) Ruby a lowering pass
/// re-ingests; see the module doc for why a real id is never safe for
/// one.
pub fn register(path: &str, text: &str) -> FileId {
    if path.starts_with('<') {
        return FileId(0);
    }
    SOURCES.with(|s| {
        let mut reg = s.borrow_mut();
        if let Some(id) = reg.by_path.get(path) {
            return *id;
        }
        reg.files.push(SourceFile {
            path: path.to_string(),
            text: text.to_string(),
        });
        let id = FileId(reg.files.len() as u32);
        reg.by_path.insert(path.to_string(), id);
        id
    })
}

/// `FileId` for a previously registered path; `FileId(0)` (the
/// synthetic sentinel) when the path was never registered — spans
/// built against it render message-only, same as before real spans.
pub fn file_id(path: &str) -> FileId {
    SOURCES.with(|s| {
        s.borrow()
            .by_path
            .get(path)
            .copied()
            .unwrap_or(FileId(0))
    })
}

/// 1-based line number for a byte offset into `path`'s registered
/// source — what `__LINE__` needs at ingest time. `None` when `path`
/// was never registered (a bare `roundhouse-ast -e` snippet that
/// bypassed `ingest_ruby_program`/`register`), so the caller can fall
/// back rather than mis-report line 1.
pub fn line_at(path: &str, offset: usize) -> Option<u32> {
    SOURCES.with(|s| {
        let reg = s.borrow();
        let id = *reg.by_path.get(path)?;
        let file = reg.files.get((id.0 as usize).checked_sub(1)?)?;
        Some(file.line_col(offset as u32).0)
    })
}

/// The registered path for a `FileId`; `None` for the synthetic
/// sentinel or an id from another ingest.
pub fn path_of(id: FileId) -> Option<String> {
    SOURCES.with(|s| {
        let reg = s.borrow();
        (id.0 as usize)
            .checked_sub(1)
            .and_then(|i| reg.files.get(i))
            .map(|f| f.path.clone())
    })
}

/// Move the registered files out (ids stay valid as indices + 1) and
/// clear the registry.
pub fn drain() -> Vec<SourceFile> {
    SOURCES.with(|s| {
        let mut reg = s.borrow_mut();
        reg.by_path.clear();
        std::mem::take(&mut reg.files)
    })
}

/// Clone the registered files without clearing the registry — unlike
/// [`drain`], `FileId`s already handed out (and any registered after
/// this call) stay valid, because the table itself is untouched.
///
/// For a mid-ingest read of "the real app source registered so far"
/// (`keep_initializer_defined`'s scan of `app/`/`lib/` text) that has
/// to happen before the synthesizing lowering passes run, but must not
/// reset the `FileId` counter out from under them the way [`drain`]
/// would.
pub fn snapshot() -> Vec<SourceFile> {
    SOURCES.with(|s| s.borrow().files.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_is_idempotent_by_path_and_first_text_wins() {
        reset();
        let a = register("app/models/article.rb", "class Article\nend\n");
        let b = register("app/models/article.rb", "different");
        assert_eq!(a, b);
        assert_eq!(file_id("app/models/article.rb"), a);
        let files = drain();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].text, "class Article\nend\n");
    }

    #[test]
    fn unregistered_path_is_the_synthetic_sentinel() {
        reset();
        assert_eq!(file_id("nope.rb"), FileId(0));
    }

    #[test]
    fn line_at_counts_newlines_up_to_the_offset() {
        reset();
        register("a.rb", "one\ntwo\nthree\n");
        assert_eq!(line_at("a.rb", 0), Some(1));
        assert_eq!(line_at("a.rb", 4), Some(2)); // start of "two"
        assert_eq!(line_at("a.rb", 8), Some(3)); // start of "three"
        assert_eq!(line_at("nope.rb", 0), None);
        drain();
    }

    #[test]
    fn ids_are_one_based_drain_clears() {
        reset();
        let a = register("a.rb", "1");
        let b = register("b.rb", "2");
        assert_eq!(a, FileId(1));
        assert_eq!(b, FileId(2));
        let files = drain();
        assert_eq!(files[0].path, "a.rb");
        assert_eq!(files[1].path, "b.rb");
        assert_eq!(file_id("a.rb"), FileId(0));
    }

    #[test]
    fn a_bracketed_label_is_refused_a_real_id() {
        reset();
        assert_eq!(register("<delegate>", "class X\nend\n"), FileId(0));
        assert_eq!(file_id("<delegate>"), FileId(0));
        // And it never took a slot at all.
        assert!(drain().is_empty());
    }

    #[test]
    fn refusing_a_bracketed_label_does_not_disturb_real_ids() {
        reset();
        let a = register("a.rb", "1");
        assert_eq!(register("<delegate>", "class X\nend\n"), FileId(0));
        let b = register("b.rb", "2");
        // The refused registration didn't consume a slot, so real
        // files keep contiguous ids either side of it.
        assert_eq!(a, FileId(1));
        assert_eq!(b, FileId(2));
        assert_eq!(drain().len(), 2);
    }

    #[test]
    fn snapshot_does_not_clear_or_reset_ids() {
        reset();
        let a = register("a.rb", "1");
        let snap = snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].path, "a.rb");
        // Unlike `drain`, the registry is untouched: `a.rb` is still
        // registered, and the next real registration continues the
        // same id sequence rather than restarting at 1.
        assert_eq!(file_id("a.rb"), a);
        let b = register("b.rb", "2");
        assert_eq!(b, FileId(2));
        let files = drain();
        assert_eq!(files.len(), 2);
    }
}
