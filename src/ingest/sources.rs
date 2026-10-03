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
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::span::{FileId, SourceFile};

thread_local! {
    static SOURCES: RefCell<Registry> = RefCell::new(Registry::default());
}

#[derive(Default)]
struct Registry {
    files: Vec<SourceFile>,
    by_path: HashMap<String, FileId>,
    root: Option<PathBuf>,
    parsed: HashMap<usize, ParsedSource>,
}

struct ParsedSource {
    lines: Vec<usize>,
    reserved_locals: HashSet<String>,
}

/// Clear the registry for a fresh whole-app ingest.
pub fn reset() {
    SOURCES.with(|s| *s.borrow_mut() = Registry::default());
}

/// Whole-app ingest supplies the real root once; source identities and
/// diagnostic spans stay unchanged, while `__FILE__` can be relocatable.
/// Hold the guard through ingest so early errors cannot leak its root.
#[must_use]
pub(super) fn set_root(root: &Path) -> impl Drop {
    struct ClearRoot;
    impl Drop for ClearRoot {
        fn drop(&mut self) {
            SOURCES.with(|s| s.borrow_mut().root = None);
        }
    }
    SOURCES.with(|s| s.borrow_mut().root = Some(root.to_path_buf()));
    ClearRoot
}

pub(super) fn relative_path(path: &str) -> String {
    SOURCES.with(|s| {
        let reg = s.borrow();
        let path = Path::new(path);
        reg.root.as_ref().and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path).to_string_lossy().into_owned()
    })
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
/// source, including ERB's translated template offsets. `None` when `path`
/// was never registered (a bare `roundhouse-ast -e` snippet that
/// bypassed `ingest_ruby_program`/`register`), so the caller can fall
/// back rather than mis-report line 1.
pub fn line_at(path: &str, offset: usize) -> Option<u32> {
    SOURCES.with(|s| {
        let reg = s.borrow();
        let id = *reg.by_path.get(path)?;
        let file = reg.files.get((id.0 as usize).checked_sub(1)?)?;
        Some(file.text.as_bytes()[..offset.min(file.text.len())].iter()
            .filter(|&&b| b == b'\n').count() as u32 + 1)
    })
}

/// Track the actual parsed bytes separately from first-text-wins span sources.
/// Prism locations borrow this input: its address identifies a live version,
/// even when several versions of the same filename are being ingested.
pub(super) fn register_parse(source: &[u8]) {
    let lines = source.iter().enumerate().filter_map(|(offset, &byte)|
        (byte == b'\n').then_some(offset)).collect();
    // Reserve compiler-style identifiers throughout the input, including
    // parameters and later assignments outside the expression being lowered.
    // Strings/comments may conservatively reserve a name too; no user binding
    // may be captured just because it is outside this node's source slice.
    let reserved_locals = source.split(|b| !b.is_ascii_alphanumeric() && *b != b'_')
        .filter(|word| word.starts_with(b"__"))
        .map(|word| String::from_utf8_lossy(word).into_owned()).collect();
    SOURCES.with(|s| {
        s.borrow_mut().parsed.insert(source.as_ptr() as usize, ParsedSource { lines, reserved_locals });
    });
}

pub(super) fn line_at_parse(location: &ruby_prism::Location<'_>) -> Option<u32> {
    let offset = location.start_offset();
    // Recover only the input's identity, never dereference an adjusted pointer.
    let source = location.as_slice().as_ptr() as usize - offset;
    SOURCES.with(|s| s.borrow().parsed.get(&source)
        .map(|parsed| parsed.lines.partition_point(|&newline| newline < offset) as u32 + 1))
}

/// Check a generated local against the actual parse, not first-text-wins spans.
/// Direct expression callers without the parse wrapper still reserve names
/// in their supplied node; whole-file entry points reserve the entire input.
pub(super) fn generated_local_is_reserved(location: &ruby_prism::Location<'_>, name: &str) -> bool {
    let source = location.as_slice().as_ptr() as usize - location.start_offset();
    SOURCES.with(|s| s.borrow().parsed.get(&source)
        .is_some_and(|parsed| parsed.reserved_locals.contains(name)))
        || location.as_slice().split(|b| !b.is_ascii_alphanumeric() && *b != b'_')
            .any(|word| word == name.as_bytes())
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
        reg.root = None;
        reg.parsed.clear();
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
    fn parsed_line_indices_follow_live_inputs_and_clear_on_drain() {
        reset();
        let first = super::super::prism::parse(b"1\n__LINE__", "probe.rb");
        let second = super::super::prism::parse(b"\n\n__LINE__", "probe.rb");
        let a = first.node().as_program_node().unwrap().statements().body().iter().last().unwrap().location();
        let b = second.node().as_program_node().unwrap().statements().body().iter().last().unwrap().location();
        assert_eq!(line_at_parse(&a), Some(2));
        assert_eq!(line_at_parse(&b), Some(3));
        assert!(snapshot().is_empty(), "parse metadata must not allocate file identities");
        drain();
        assert_eq!(line_at_parse(&a), None);
        assert_eq!(line_at_parse(&b), None);
    }

    #[test]
    fn generated_names_follow_the_live_parse_and_clear_on_drain() {
        reset();
        let first = super::super::prism::parse(b"__mw_0=11; __mw_0_1=22; a, *b, c=[11,22,33]", "probe.rb");
        let second = super::super::prism::parse(b"a, *b, c=[11,22,33]", "probe.rb");
        let a = first.node().as_program_node().unwrap().statements().body().iter().last().unwrap().location();
        let b = second.node().as_program_node().unwrap().statements().body().iter().last().unwrap().location();
        assert!(generated_local_is_reserved(&a, "__mw_0"));
        assert!(generated_local_is_reserved(&a, "__mw_0_1"));
        assert!(!generated_local_is_reserved(&a, "__mw_0_2"));
        assert!(!generated_local_is_reserved(&b, "__mw_0"));
        drain();
        assert!(!generated_local_is_reserved(&a, "__mw_0"));
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
