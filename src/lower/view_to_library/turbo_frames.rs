//! turbo-rails' `Turbo::FramesHelper#turbo_frame_tag` → the
//! `<turbo-frame>` element it builds.
//!
//! The helper is one line of Rails:
//!
//! ```ruby
//! def turbo_frame_tag(*ids, src: nil, target: nil, **attributes, &block)
//!   id = ids.first.respond_to?(:to_key) || ids.first.is_a?(Class) ?
//!     ActionView::RecordIdentifier.dom_id(*ids) : ids.join('_')
//!   src = url_for(src) if src.present?
//!   tag.turbo_frame(**attributes.merge(id: id, src: src, target: target).compact, &block)
//! end
//! ```
//!
//! so everything it knows is compile-time knowledge: the element name,
//! how the id positionals become one `id` attribute, and the ORDER the
//! attributes render in (the caller's own first, then id, src, target —
//! `merge` keeps an existing key's position, and only appends the ones
//! that are new). Expanded at lower time for the same reason the Drive
//! family is (`turbo_drive.rs`): there is nothing here for a runtime to
//! compute, so one expansion reaches every target instead of nine
//! hand-written copies.
//!
//! MEASURED against turbo-rails 2.0.23 through `fixtures/real-blog`
//! (`ApplicationController.helpers.turbo_frame_tag …`), not derived:
//!
//! ```text
//! turbo_frame_tag :user_sidebar
//!   <turbo-frame id="user_sidebar"></turbo-frame>
//! turbo_frame_tag :user_sidebar, src: nil, target: "_top", data: {turbo_permanent: true}
//!   <turbo-frame data-turbo-permanent="true" id="user_sidebar" target="_top"></turbo-frame>
//! turbo_frame_tag article, :boosting
//!   <turbo-frame id="boosting_article_1"></turbo-frame>
//! turbo_frame_tag :next_page_container, loading: :lazy, src: "/articles?page=2"
//!   <turbo-frame loading="lazy" id="next_page_container" src="/articles?page=2"></turbo-frame>
//! ```
//!
//! Two of those are the reason the rules are worth pinning: a nil `src:`
//! renders NO attribute (`.compact`), and the caller's own attributes
//! come BEFORE id/src/target however they were written.
//!
//! TWO OWNERS call in — the view walker, for the ERB spelling, and
//! `lower::tag_builder`, for a call inside a HELPER body (campfire
//! writes both: `_bell.html.erb` and `users/sidebar_helper.rb`). This
//! module owns the Rails knowledge and returns the open tag's parts;
//! each owner supplies the content between them the way its own body
//! kind demands (a walked template body vs a captured Ruby block).
//!
//! NOT reached by the Python view emitter, which walks compiled ERB on
//! its own (the same split `turbo_drive::head_directive` is shared
//! across). Nothing in `fixtures/` writes a frame, so `compare python`
//! is unaffected today; a fixture that does would need a Python arm
//! first, and the block form is the work there.

use crate::expr::{Expr, ExprNode, InterpPart, Literal};

use super::attr_parts::{append_attr_parts, lit_str_coerce, take_opt};
use super::{lit_str, view_helpers_call};

// The element `tag.turbo_frame` builds: TagBuilder dasherizes an
// underscored name into its custom-element form.
pub(crate) const FRAME_CLOSE: &str = "</turbo-frame>";
const FRAME_OPEN: &str = "<turbo-frame";

/// Rails' `ids.first.respond_to?(:to_key)` asked at COMPILE time: does
/// this expression name an Active Record object?
///
/// The stand-in is the bare-name convention the rest of the pipeline
/// already commits to — `message` / `@room` name their models — which is
/// what `turbo_frame_tag message, :boosting` needs and what
/// `turbo_frame_tag dom_id(room, :involvement)` (a Send with a receiver
/// and arguments) correctly answers no to. Both owners ask through here
/// so the signal has one definition.
pub(crate) fn names_a_record(e: &Expr, models: &std::collections::HashSet<String>) -> bool {
    super::bare_record_name(e)
        .map(|name| models.contains(&name))
        .unwrap_or(false)
}

/// The open tag `<turbo-frame …>` for a recognized `turbo_frame_tag`
/// call, or `None` when the argument shape is one we cannot spell (see
/// [`frame_id`]). Callers append the content and [`FRAME_CLOSE`].
///
/// `is_record` is [`names_a_record`] closed over the caller's model
/// set — a parameter rather than a direct call so the tests can pin the
/// argument shapes without an App.
pub(crate) fn frame_open_parts(
    args: &[Expr],
    is_record: &dyn Fn(&Expr) -> bool,
) -> Option<Vec<InterpPart>> {
    let (ids, mut opts) = split_args(args);
    let id = frame_id(ids, is_record)?;
    // Peeled off before the caller's attributes render, because Rails
    // gives these three the LAST three positions regardless of where
    // they were written.
    let src = take_opt(&mut opts, "src");
    let target = take_opt(&mut opts, "target");

    let mut parts = vec![InterpPart::Text { value: FRAME_OPEN.to_string() }];
    append_attr_parts(&mut parts, &opts);
    // `id` is the one member of the trio that `.compact` can never
    // drop: both branches of Rails' pick (`dom_id` and `join`) return a
    // String, so it renders unguarded.
    parts.extend(rendered_attr("id", id));
    if let Some(src) = src {
        parts.extend(compacted_attr("src", src));
    }
    if let Some(target) = target {
        parts.extend(compacted_attr("target", target));
    }
    parts.push(InterpPart::Text { value: ">".to_string() });
    Some(parts)
}

/// Split the call Rails' way: a trailing Hash literal is the options,
/// every leading positional is an id piece.
fn split_args(args: &[Expr]) -> (&[Expr], Vec<(Expr, Expr)>) {
    match args.last().map(|last| &*last.node) {
        Some(ExprNode::Hash { entries, .. }) => (&args[..args.len() - 1], entries.clone()),
        _ => (args, Vec::new()),
    }
}

/// The `id` attribute's value.
///
/// Rails picks between `dom_id(*ids)` and `ids.join('_')` by asking the
/// first id whether it is a record, at run time. Statically there are
/// three answers:
///
///   * every piece is a Symbol or String literal — fold the join at
///     compile time (`turbo_frame_tag :user_sidebar` → `"user_sidebar"`);
///   * the first piece names a record — `dom_id(record[, prefix])`,
///     which is the same call the `dom_id` view helper lowers to, so
///     both owners already resolve it;
///   * anything else, alone — Rails' join over one element is the
///     element itself, coerced to a String. This is the `turbo_frame_tag
///     dom_id(room, :involvement)` spelling, where the caller already
///     did the dom_id.
///
/// A mixed list of non-literals declines rather than guessing: a join
/// over runtime values would need a runtime helper, and no corpus app
/// writes one.
fn frame_id(ids: &[Expr], is_record: &dyn Fn(&Expr) -> bool) -> Option<Expr> {
    match ids {
        [] => None,
        // A record plus at most a prefix — `dom_id`'s own arity.
        [first, rest @ ..] if is_record(first) && rest.len() <= 1 => {
            Some(view_helpers_call("dom_id", ids.to_vec()))
        }
        _ => {
            let literals: Option<Vec<String>> = ids.iter().map(literal_id_piece).collect();
            match literals {
                Some(pieces) => Some(lit_str(pieces.join("_"))),
                // `[x].join("_")` is `x` itself; the String coercion the
                // attribute needs is `rendered_attr`'s to apply, not
                // ours to apply twice.
                None if ids.len() == 1 => Some(ids[0].clone()),
                None => None,
            }
        }
    }
}

/// The text a Symbol or String literal id piece contributes, if it is
/// one.
fn literal_id_piece(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }
}

/// One attribute under `.compact` — the merged hash's nil values are
/// dropped, so a nil `src:` renders NO attribute at all.
///
/// `src=""` is not the same thing to Turbo as an absent `src`: an empty
/// one still marks the frame as lazily loaded and sends it after the
/// current URL. campfire's `sidebar_turbo_frame_tag(src: nil, &)` passes
/// exactly that nil whenever the sidebar is rendered with a block
/// instead of being lazy-loaded, so the guard is on the milestone path
/// rather than hypothetical.
///
/// A literal decides at compile time; anything else gets the runtime
/// test. The value is evaluated twice there (once for the test, once for
/// the render) — every corpus site is a parameter read or a path helper,
/// both free of side effects, and hoisting a temp is not available
/// inside a string interpolation.
fn compacted_attr(key: &str, value: Expr) -> Vec<InterpPart> {
    if matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) {
        return Vec::new();
    }
    if matches!(&*value.node, ExprNode::Lit { .. } | ExprNode::StringInterp { .. }) {
        return rendered_attr(key, value);
    }
    vec![InterpPart::Expr {
        expr: Expr::new(
            value.span,
            ExprNode::If {
                cond: super::send(Some(value.clone()), "nil?", Vec::new(), None, false),
                then_branch: lit_str(String::new()),
                else_branch: super::attr_parts::string_interp(rendered_attr(key, value)),
            },
        ),
    }]
}

/// ` key="<escaped value>"`. A String literal folds all the way into
/// the surrounding text — the id is a literal at most call sites
/// (`turbo_frame_tag :user_sidebar`), and `id="#{"user_sidebar"}"` is
/// the same bytes through a needless interpolation.
fn rendered_attr(key: &str, value: Expr) -> Vec<InterpPart> {
    if let ExprNode::Lit { value: Literal::Str { value } } = &*value.node {
        return vec![InterpPart::Text {
            value: format!(" {key}=\"{}\"", super::html_escape_fold(value)),
        }];
    }
    vec![
        InterpPart::Text { value: format!(" {key}=\"") },
        InterpPart::Expr {
            expr: view_helpers_call("html_escape", vec![lit_str_coerce(value)]),
        },
        InterpPart::Text { value: "\"".to_string() },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::Symbol;
    use crate::span::Span;

    fn sym(name: &str) -> Expr {
        crate::lower::view_to_library::lit_sym(Symbol::from(name))
    }

    fn bare(name: &str) -> Expr {
        crate::lower::view_to_library::send(None, name, Vec::new(), None, false)
    }

    fn hash(entries: Vec<(Expr, Expr)>) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Hash { entries, kwargs: true })
    }

    /// The static text of the rendered open tag, with each runtime
    /// interpolation shown as `{}` — enough to pin element name,
    /// attribute order and which attributes render at all.
    fn shape(args: &[Expr], records: &[&str]) -> String {
        let is_record = |e: &Expr| match &*e.node {
            ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
                records.contains(&method.as_str())
            }
            _ => false,
        };
        let parts = frame_open_parts(args, &is_record).expect("recognized");
        parts
            .iter()
            .map(|p| match p {
                InterpPart::Text { value } => value.clone(),
                InterpPart::Expr { .. } => "{}".to_string(),
            })
            .collect()
    }

    #[test]
    fn a_literal_id_folds_into_the_tag() {
        assert_eq!(shape(&[sym("user_sidebar")], &[]), "<turbo-frame id=\"user_sidebar\">");
    }

    #[test]
    fn a_record_plus_prefix_becomes_dom_id() {
        // `turbo_frame_tag message, :boosting` — one interpolation, the
        // `dom_id(message, :boosting)` call.
        assert_eq!(shape(&[bare("message"), sym("boosting")], &["message"]), "<turbo-frame id=\"{}\">");
    }

    /// Rails renders the caller's own attributes FIRST, then id, src,
    /// target — `attributes.merge(id:, src:, target:)` keeps the
    /// caller's keys where they were and appends the rest.
    #[test]
    fn the_callers_attributes_come_before_id_src_and_target() {
        let args = vec![
            sym("next_page_container"),
            hash(vec![
                (sym("loading"), sym("lazy")),
                (sym("src"), bare("next_page_url")),
                (sym("target"), lit_str("_top".to_string())),
            ]),
        ];
        assert_eq!(
            shape(&args, &[]),
            "<turbo-frame loading=\"{}\" id=\"next_page_container\"{} target=\"_top\">"
        );
    }

    /// `.compact` drops a nil `src:`; an empty `src` is a DIFFERENT
    /// element to Turbo, not a cosmetic difference.
    #[test]
    fn a_literal_nil_src_renders_no_attribute() {
        let args = vec![
            sym("user_sidebar"),
            hash(vec![(
                sym("src"),
                Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            )]),
        ];
        assert_eq!(shape(&args, &[]), "<turbo-frame id=\"user_sidebar\">");
    }

    /// The already-spelled id (`turbo_frame_tag dom_id(room, :involvement)`)
    /// is Rails' join-over-one-element: the value itself.
    #[test]
    fn a_single_computed_id_renders_as_a_string() {
        assert_eq!(shape(&[bare("some_id")], &[]), "<turbo-frame id=\"{}\">");
    }

    #[test]
    fn a_join_over_several_runtime_pieces_declines() {
        assert!(frame_open_parts(&[bare("a"), bare("b")], &|_| false).is_none());
        assert!(frame_open_parts(&[], &|_| false).is_none());
    }
}
