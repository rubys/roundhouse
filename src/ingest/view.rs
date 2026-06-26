//! Text-template view ingestion (ERB today; HAML / herb as they land).
//!
//! Every engine in this family compiles its template text to the same
//! `_buf`-append Ruby shape (`_buf = ""` … `_buf = _buf + EXPR` … `_buf`)
//! plus a segment map (compiled-Ruby ↔ template byte ranges), then flows
//! through one shared pipeline: register the source, ingest the compiled
//! Ruby via Prism, translate spans back to template coordinates, and wrap
//! as a `View`. The ONLY engine-specific piece is the compile function —
//! [`ingest_template`] is the shared body and [`ViewEngine`] is the
//! extension dispatch. Adding an engine = one [`ViewEngine`] arm + its
//! `compile_*_mapped` fn; nothing in the pipeline below changes.
//!
//! (Jbuilder is *not* in this family: its source is already plain Ruby
//! with native spans, so it has its own `ingest_jbuilder` with no
//! compile/segment step.)

use std::path::Path;

use crate::Symbol;
use crate::dialect::View;
use crate::erb;
use crate::ty::Row;

use super::IngestResult;
use super::expr::ingest_ruby_program;

/// A compiled-Ruby ↔ template byte-range map — the span-translation
/// contract every text-template engine produces. (Defined in `erb` for
/// historical reasons; re-exported here under the neutral seam name so
/// HAML/herb compilers name the shared type, not the ERB one.)
pub use crate::erb::ErbSegment as TemplateSegment;

/// Lowers a template's source text to the `_buf`-append Ruby shape plus
/// its segment map. Implemented once per engine
/// (`erb::compile_erb_mapped`, and — to come — `haml::compile_haml_mapped`,
/// `herb::compile_herb_mapped`).
pub type CompileFn = fn(&str) -> (String, Vec<TemplateSegment>);

/// The text-template engines that flow through [`ingest_template`].
/// Adding HAML/herb is a new variant plus its `compile_fn` /
/// `from_extension` arms; the rest of the view pipeline is
/// engine-agnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewEngine {
    Erb,
}

impl ViewEngine {
    /// Resolve an engine from a view file's final (engine) extension —
    /// the `erb` in `index.html.erb`. `None` if no text-template engine
    /// claims it (jbuilder, not-yet-supported engines, plain files).
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "erb" => Some(ViewEngine::Erb),
            _ => None,
        }
    }

    /// This engine's template → `_buf`-Ruby compiler.
    pub fn compile_fn(self) -> CompileFn {
        match self {
            ViewEngine::Erb => erb::compile_erb_mapped,
        }
    }
}

/// Ingest a single `.html.erb` template through the ERB engine. Thin
/// wrapper over [`ingest_template`] — the named entry `mod.rs` re-exports.
pub fn ingest_view(source: &str, rel_path: &Path, file: &str) -> IngestResult<View> {
    ingest_template(source, rel_path, file, erb::compile_erb_mapped)
}

/// Shared text-template ingest. Parse name/format from the path
/// (`posts/index.html.erb` → name=`posts/index`, format=`html`),
/// register the on-disk source, compile to `_buf` Ruby via `compile`,
/// ingest that through the generic Ruby pipeline, and translate spans
/// from compiled-Ruby offsets back to template offsets so every
/// downstream span (diagnostics, source maps) indexes the real template.
///
/// `compile` is the only engine-specific input; pass
/// [`ViewEngine::compile_fn`] from the dispatch, or any [`CompileFn`].
pub fn ingest_template(
    source: &str,
    rel_path: &Path,
    file: &str,
    compile: CompileFn,
) -> IngestResult<View> {
    let path_str = rel_path.to_string_lossy();
    // Strip the engine's own (final) extension, then split `stem.format`.
    // `posts/index.html.erb` → `posts/index.html` → (`posts/index`, `html`).
    let no_engine = path_str
        .rsplit_once('.')
        .map(|(stem, _ext)| stem)
        .unwrap_or(&path_str);
    let (name, format) = match no_engine.rsplit_once('.') {
        Some((stem, fmt)) => (stem.to_string(), fmt.to_string()),
        None => (no_engine.to_string(), "html".to_string()),
    };

    // Span coordinates: register the on-disk template text under this
    // path FIRST — registration is first-text-wins, so the compiled Ruby
    // that `ingest_ruby_program` registers as it parses is a no-op.
    // Ingest builds spans as compiled-Ruby offsets; `translate_spans`
    // rewrites them to template offsets via the engine's segment table,
    // so every span downstream indexes the text actually registered.
    super::sources::register(file, source);
    let (compiled, map) = compile(source);
    let mut body = ingest_ruby_program(&compiled, file)?;
    erb::translate_spans(&mut body, &map);

    Ok(View {
        name: Symbol::from(name),
        format: Symbol::from(format),
        locals: Row::closed(),
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The minimal possible engine: the whole template as one static
    /// text chunk. It has nothing to do with ERB, which is the point —
    /// it proves [`ingest_template`] is engine-agnostic: any `CompileFn`
    /// producing the `_buf` shape + segment map flows through to a
    /// well-formed `View` with template-coordinate spans.
    fn whole_text_engine(src: &str) -> (String, Vec<TemplateSegment>) {
        let mut out = String::from("_buf = \"\"\n_buf = _buf + ");
        let c_start = out.len() as u32;
        // Rust's debug formatting is a valid Ruby double-quoted literal
        // for the interpolation-free ASCII input used by the test.
        out.push_str(&format!("{src:?}"));
        let c_end = out.len() as u32;
        out.push_str("\n_buf\n");
        (
            out,
            vec![TemplateSegment {
                c_start,
                c_end,
                e_start: 0,
                e_end: src.len() as u32,
            }],
        )
    }

    fn for_each(e: &mut crate::expr::Expr, f: &mut impl FnMut(&crate::expr::Expr)) {
        f(e);
        e.node.for_each_child_mut(&mut |c| for_each(c, f));
    }

    #[test]
    fn ingest_template_is_engine_agnostic() {
        let src = "hello world";
        let view = ingest_template(
            src,
            Path::new("greetings/hi.html.custom"),
            "greetings/hi.html.custom",
            whole_text_engine,
        )
        .expect("custom engine ingests");

        // name/format parsing is engine-independent (strips the final
        // extension, whatever it is).
        assert_eq!(view.name.as_str(), "greetings/hi");
        assert_eq!(view.format.as_str(), "html");

        // Spans were translated into template coordinates: every real
        // (non-synthetic) span lands inside the template's byte range.
        let mut body = view.body.clone();
        let mut saw_real = false;
        for_each(&mut body, &mut |e| {
            if !e.span.is_synthetic() {
                saw_real = true;
                assert!(
                    e.span.start <= e.span.end && e.span.end <= src.len() as u32,
                    "span {}..{} outside template bounds 0..{}",
                    e.span.start,
                    e.span.end,
                    src.len(),
                );
            }
        });
        assert!(saw_real, "expected at least one template-coordinate span");
    }

    #[test]
    fn view_engine_dispatch_resolves_erb_only_for_now() {
        assert_eq!(ViewEngine::from_extension("erb"), Some(ViewEngine::Erb));
        // Not-yet-supported engines resolve to None (recorded as survey
        // gaps by the walker) — flipping one to `Some(..)` + a
        // `compile_fn` arm is the whole drop-in for a new engine.
        assert_eq!(ViewEngine::from_extension("haml"), None);
        assert_eq!(ViewEngine::from_extension("herb"), None);
        assert_eq!(ViewEngine::from_extension("jbuilder"), None);
    }
}
