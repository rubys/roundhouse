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
//! Registration records *the text prism parsed*, which for `.html.erb`
//! views is the compiled Ruby out of `compile_erb`, not the raw
//! template (the compiler merges text chunks, so byte offsets only
//! make sense against its output). View diagnostics therefore name
//! the right file, but line numbers index the compiled template body.
//!
//! Standalone ingest entry points (unit tests calling
//! `ingest_ruby_program` directly) also register; without a
//! surrounding `reset`/`drain` the table just accumulates on the test
//! thread, which is harmless. Re-registering a path keeps the first
//! text — ids stay stable for spans already handed out.

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
pub fn register(path: &str, text: &str) -> FileId {
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

/// Move the registered files out (ids stay valid as indices + 1) and
/// clear the registry.
pub fn drain() -> Vec<SourceFile> {
    SOURCES.with(|s| {
        let mut reg = s.borrow_mut();
        reg.by_path.clear();
        std::mem::take(&mut reg.files)
    })
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
}
