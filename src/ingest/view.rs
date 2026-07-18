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
use crate::expr::Expr;
use crate::{erb, haml};
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
    Haml,
}

impl ViewEngine {
    /// Resolve an engine from a view file's final (engine) extension —
    /// the `erb` in `index.html.erb`. `None` if no text-template engine
    /// claims it (jbuilder, not-yet-supported engines, plain files).
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "erb" => Some(ViewEngine::Erb),
            "haml" => Some(ViewEngine::Haml),
            _ => None,
        }
    }

    /// This engine's template → `_buf`-Ruby compiler.
    pub fn compile_fn(self) -> CompileFn {
        match self {
            ViewEngine::Erb => erb::compile_erb_mapped,
            ViewEngine::Haml => haml::compile_haml_mapped,
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
        strict_locals: parse_strict_locals(source),
    })
}

/// Parse a Rails strict-locals magic comment — a leading `<%# locals:
/// (comment:, was_merged: false, …) -%>` — into ordered KEYWORD
/// `Param`s. Required locals (`comment:`) get no default; defaulted
/// ones (`was_merged: false`) carry the parsed literal. Returns `None`
/// when the template has no such header (the common case). Only the
/// literal defaults lobsters uses (true/false/nil/int/str/sym) are
/// modeled; an unrecognized default degrades to `nil` (the param stays
/// optional, just mis-defaulted — no caller in the corpus hits it).
fn parse_strict_locals(source: &str) -> Option<Vec<crate::dialect::Param>> {
    use crate::dialect::Param;
    // The magic comment must be a `<%# … locals: ( … ) … %>` tag. Anchor
    // on `locals:` and require an enclosing `<%#` comment opener with no
    // intervening tag close (so a stray `locals:` in body text is ignored).
    let kw = source.find("locals:")?;
    let open = source[..kw].rfind("<%#")?;
    if source[open..kw].contains("%>") {
        return None;
    }
    // Bound the header to THIS comment's close: `%>` after `locals:` ends
    // the tag (we already know there's none before it). Without this bound
    // the `(`/`)` scan runs past the comment into unrelated template code,
    // where a stray paren would hijack the signature (finding: phantom
    // header). No `%>` after `locals:` at all → not a real header.
    let close = kw + source[kw..].find("%>")?;
    let after = &source[kw + "locals:".len()..close];
    let lp = after.find('(')?;
    let rest = &after[lp + 1..];
    let mut depth = 1usize;
    let mut end = None;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let inner = &rest[..end?];
    let mut params = Vec::new();
    for entry in split_top_level_commas(inner) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // `name:` (required) or `name: <default>` (optional). A colon-less
        // entry (`**attrs`, or a splat) isn't a plain local — SKIP it, don't
        // abort the whole header (a `?` here dropped every declared local).
        let Some(colon) = entry.find(':') else { continue };
        let name = entry[..colon].trim();
        if name.is_empty() {
            continue;
        }
        let default_src = entry[colon + 1..].trim();
        let sym = Symbol::from(name);
        if default_src.is_empty() {
            params.push(Param::keyword(sym, None));
        } else {
            params.push(Param::keyword(sym, Some(parse_default_literal(default_src))));
        }
    }
    (!params.is_empty()).then_some(params)
}

/// Split on commas that aren't nested inside `()`/`[]`/`{}` or a string
/// literal — strict-locals defaults can be `{a: 1}` or `[1, 2]`.
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) => {
                buf.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    buf.push(c);
                }
                '(' | '[' | '{' => {
                    depth += 1;
                    buf.push(c);
                }
                ')' | ']' | '}' => {
                    depth -= 1;
                    buf.push(c);
                }
                ',' if depth == 0 => {
                    out.push(std::mem::take(&mut buf));
                }
                _ => buf.push(c),
            },
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf);
    }
    out
}

/// Parse a strict-locals default's source into a literal `Expr`. Covers
/// the literals a header default realistically uses; anything else
/// degrades to `nil`.
fn parse_default_literal(s: &str) -> Expr {
    use crate::expr::{ArrayStyle, ExprNode, Literal};
    use crate::span::Span;
    // Empty-array default (`read_by_notifications: []`) → a real empty
    // Array, so a body `.include?`/`.each`/`.length` on it doesn't
    // NoMethodError against a degraded `nil`.
    if s == "[]" {
        return Expr::new(
            Span::synthetic(),
            ExprNode::Array { elements: Vec::new(), style: ArrayStyle::default() },
        );
    }
    let lit = match s {
        "true" => Literal::Bool { value: true },
        "false" => Literal::Bool { value: false },
        "nil" => Literal::Nil,
        _ if s.starts_with(':') => Literal::Sym { value: Symbol::from(&s[1..]) },
        _ if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
            || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2) =>
        {
            Literal::Str { value: s[1..s.len() - 1].to_string() }
        }
        _ if s.parse::<i64>().is_ok() => Literal::Int { value: s.parse().unwrap() },
        // Float before the nil fallback (`1.5` fails i64 but parses f64).
        _ if s.parse::<f64>().is_ok() => Literal::Float { value: s.parse().unwrap() },
        _ => Literal::Nil,
    };
    Expr::new(Span::synthetic(), ExprNode::Lit { value: lit })
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
    fn view_engine_dispatch() {
        assert_eq!(ViewEngine::from_extension("erb"), Some(ViewEngine::Erb));
        assert_eq!(ViewEngine::from_extension("haml"), Some(ViewEngine::Haml));
        // Not-yet-supported engines resolve to None (recorded as survey
        // gaps by the walker) — flipping one to `Some(..)` + a
        // `compile_fn` arm is the whole drop-in for a new engine.
        assert_eq!(ViewEngine::from_extension("herb"), None);
        assert_eq!(ViewEngine::from_extension("jbuilder"), None);
    }

    fn locals_names(src: &str) -> Vec<String> {
        parse_strict_locals(src)
            .unwrap_or_default()
            .iter()
            .map(|p| p.name.as_str().to_string())
            .collect()
    }

    #[test]
    fn strict_locals_parses_required_and_defaulted() {
        let src = "<%# locals: (comment:, was_merged: false, story: nil) -%>\n<div>";
        let ps = parse_strict_locals(src).unwrap();
        assert_eq!(ps.len(), 3);
        assert_eq!(ps[0].name.as_str(), "comment");
        assert!(ps[0].default.is_none()); // required
        assert!(ps[1].default.is_some()); // was_merged: false
    }

    #[test]
    fn strict_locals_default_literals_int_float_array_sym() {
        use crate::expr::{ExprNode, Literal};
        // `[]` → a real empty Array; `1.5` → Float; neither degrades to nil.
        let src = "<%# locals: (a: [], b: 1.5, c: :x, d: 3) -%>";
        let ps = parse_strict_locals(src).unwrap();
        assert!(matches!(&*ps[0].default.as_ref().unwrap().node, ExprNode::Array { .. }));
        assert!(matches!(
            &*ps[1].default.as_ref().unwrap().node,
            ExprNode::Lit { value: Literal::Float { .. } }
        ));
        assert!(matches!(
            &*ps[2].default.as_ref().unwrap().node,
            ExprNode::Lit { value: Literal::Sym { .. } }
        ));
    }

    #[test]
    fn strict_locals_colonless_entry_is_skipped_not_fatal() {
        // A splat/colon-less entry must not abort the whole header.
        let src = "<%# locals: (comment:, **attrs) -%>";
        assert_eq!(locals_names(src), vec!["comment"]);
    }

    #[test]
    fn strict_locals_paren_scan_bounded_to_comment() {
        // A bare `locals:` in a NON-header comment must not scavenge a `(`
        // from later template code and hijack the signature.
        let src = "<%# locals: no parens here %>\n<%= foo(bar) %>";
        assert_eq!(parse_strict_locals(src), None);
    }

    #[test]
    fn strict_locals_absent_returns_none() {
        assert_eq!(parse_strict_locals("<div>plain view</div>"), None);
    }
}
