//! View-helper classifier.
//!
//! Shared recognition of the Rails view helpers that appear in
//! ERB templates: `csrf_meta_tags` / `content_for` / `link_to` /
//! `dom_id` / `stylesheet_link_tag` / etc. Every target emitter
//! consumes this — the method name + arity match happens once, in
//! one place, and each target writes a render-dispatch over
//! `ViewHelperKind` with its own string-formatting conventions.
//!
//! Scope: only the bare `Send { recv: None, block: None }` shapes
//! that appear inside `<%= ... %>` tags. Yield (`ExprNode::Yield`)
//! and `content_for || "default"` (`ExprNode::BoolOp`) are
//! different ExprNode variants and handled directly by each
//! emitter — classifying them through here would widen the API for
//! no shared logic.
//!
//! FormBuilder method calls (`form.label :title`) get a sibling
//! classifier (`classify_form_builder_method`) since they have a
//! different structural shape (recv is the form local).
//!
//! URL arguments that appear in the second position of `link_to`,
//! `button_to`, and `form_with`'s `model:` get
//! `classify_view_url_arg`. The classifier is identical across
//! targets; emission differs.

use crate::expr::{Expr, ExprNode, Literal};

/// A recognized Rails view helper, keyed by Ruby method name. The
/// variant names mirror the surface method (snake_case),
/// regardless of target naming conventions. Each variant carries
/// the positional arg Exprs + optional opts hash as the raw IR —
/// emitters pull out the bits they need.
#[derive(Debug)]
pub enum ViewHelperKind<'a> {
    /// `<%= csrf_meta_tags %>` — no args.
    CsrfMetaTags,
    /// `<%= csp_meta_tag %>` — no args.
    CspMetaTag,
    /// `<%= javascript_importmap_tags %>` — no args.
    JavascriptImportmapTags,
    /// `<%= turbo_stream_from "channel" %>`.
    TurboStreamFrom { channel: &'a Expr },
    /// `<%= dom_id(record [, prefix]) %>`.
    DomId { record: &'a Expr, prefix: Option<&'a Expr> },
    /// `<%= pluralize(count, "word") %>`.
    Pluralize { count: &'a Expr, word: &'a Expr },
    /// `<%= truncate(text [, opts]) %>`.
    Truncate { text: &'a Expr, opts: Option<&'a Expr> },
    /// `<%= stylesheet_link_tag :name [, opts] %>`.
    StylesheetLinkTag { name: &'a Expr, opts: Option<&'a Expr> },
    /// `<%= content_for(:slot) %>` (getter, no body).
    ContentForGetter { slot: &'a str },
    /// `<% content_for :slot, "body" %>` (statement-form setter).
    ContentForSetter { slot: &'a str, body: &'a Expr },
    /// `<%= link_to text, url [, opts] %>`.
    LinkTo { text: &'a Expr, url: &'a Expr, opts: Option<&'a Expr> },
    /// `<%= button_to text, target [, opts] %>`.
    ButtonTo { text: &'a Expr, target: &'a Expr, opts: Option<&'a Expr> },
}

/// Recognize a bare `Send { recv: None, args, block: None }` as
/// a Rails view helper. Returns `None` for unrecognized method
/// names or arities that don't match any variant.
pub fn classify_view_helper<'a>(
    method: &str,
    args: &'a [Expr],
) -> Option<ViewHelperKind<'a>> {
    match (method, args.len()) {
        ("csrf_meta_tags", 0) => Some(ViewHelperKind::CsrfMetaTags),
        ("csp_meta_tag", 0) => Some(ViewHelperKind::CspMetaTag),
        ("javascript_importmap_tags", 0) => Some(ViewHelperKind::JavascriptImportmapTags),
        ("turbo_stream_from", 1) => {
            Some(ViewHelperKind::TurboStreamFrom { channel: &args[0] })
        }
        ("dom_id", 1) => Some(ViewHelperKind::DomId {
            record: &args[0],
            prefix: None,
        }),
        ("dom_id", 2) => Some(ViewHelperKind::DomId {
            record: &args[0],
            prefix: Some(&args[1]),
        }),
        ("pluralize", 2) => Some(ViewHelperKind::Pluralize {
            count: &args[0],
            word: &args[1],
        }),
        ("truncate", 1) => Some(ViewHelperKind::Truncate {
            text: &args[0],
            opts: None,
        }),
        ("truncate", 2) => Some(ViewHelperKind::Truncate {
            text: &args[0],
            opts: Some(&args[1]),
        }),
        ("stylesheet_link_tag", 1) => Some(ViewHelperKind::StylesheetLinkTag {
            name: &args[0],
            opts: None,
        }),
        ("stylesheet_link_tag", 2) => Some(ViewHelperKind::StylesheetLinkTag {
            name: &args[0],
            opts: Some(&args[1]),
        }),
        ("content_for", 1) => {
            let slot = extract_sym_or_str(&args[0])?;
            Some(ViewHelperKind::ContentForGetter { slot })
        }
        ("content_for", 2) => {
            let slot = extract_sym_or_str(&args[0])?;
            Some(ViewHelperKind::ContentForSetter {
                slot,
                body: &args[1],
            })
        }
        ("link_to", 2) => Some(ViewHelperKind::LinkTo {
            text: &args[0],
            url: &args[1],
            opts: None,
        }),
        ("link_to", 3) => Some(ViewHelperKind::LinkTo {
            text: &args[0],
            url: &args[1],
            opts: Some(&args[2]),
        }),
        ("button_to", 2) => Some(ViewHelperKind::ButtonTo {
            text: &args[0],
            target: &args[1],
            opts: None,
        }),
        ("button_to", 3) => Some(ViewHelperKind::ButtonTo {
            text: &args[0],
            target: &args[1],
            opts: Some(&args[2]),
        }),
        _ => None,
    }
}

/// Pull out a `:sym` or `"str"` literal's value as a `&str`. Used
/// by the `content_for` slot-name extraction.
fn extract_sym_or_str(e: &Expr) -> Option<&str> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.as_str()),
        _ => None,
    }
}

// ── FormBuilder method classifier ──────────────────────────────

/// A FormBuilder method call (`form.label`, `form.text_field`,
/// `form.text_area`, `form.submit`). The recognized method set is
/// the scaffold-relevant subset; emitters lower these to their
/// target's FormBuilder API (camelCased in TS, snake_case in rust
/// + python, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormBuilderMethod {
    Label,
    TextField,
    TextArea,
    Submit,
}

/// Map a Ruby form method name to the lowered method kind. Rails
/// accepts both `text_area` and `textarea` as aliases; fold them.
pub fn classify_form_builder_method(method: &str) -> Option<FormBuilderMethod> {
    match method {
        "label" => Some(FormBuilderMethod::Label),
        "text_field" => Some(FormBuilderMethod::TextField),
        "text_area" | "textarea" => Some(FormBuilderMethod::TextArea),
        "submit" => Some(FormBuilderMethod::Submit),
        _ => None,
    }
}

// ── URL-arg classifier ─────────────────────────────────────────

/// The URL position of `link_to` / `button_to` / `form_with(model:
/// …)` can take several shapes. This enum names them so each
/// target's URL-rendering logic can dispatch cleanly instead of
/// pattern-matching the same Expr shapes six times.
#[derive(Debug)]
pub enum ViewUrlArg<'a> {
    /// `"literal/path"` — a plain string.
    Literal { value: &'a str },
    /// `articles_path` / `new_article_path` / `edit_article_path
    /// (article)` — a path-helper method call. `name` is the Ruby
    /// method (still snake_case with the `_path` suffix).
    PathHelper { name: &'a str, args: &'a [Expr] },
    /// `@article` / bare `article` — a reference to a record. The
    /// `name` is the local's identifier; emitters pair it with a
    /// singularize-and-camelize step for the path helper.
    RecordRef { name: &'a str },
    /// `[@article, Comment.new]` — array form for nested resources.
    /// `elements` is the raw element list; emitters classify each
    /// element (recursively via RecordRef / association read).
    NestedArray { elements: &'a [Expr] },
}

/// Classify the URL-position arg of a nav helper. `is_local`
/// checks whether a bare name is in the current view scope — used
/// to distinguish a local-bound record from an unrelated Const or
/// global call. Passed as a closure so callers don't have to
/// export their `TsViewCtx` / `ViewEmitCtx` here.
pub fn classify_view_url_arg<'a, F>(arg: &'a Expr, is_local: &F) -> Option<ViewUrlArg<'a>>
where
    F: Fn(&str) -> bool,
{
    match &*arg.node {
        ExprNode::Lit { value: Literal::Str { value } } => {
            Some(ViewUrlArg::Literal { value: value.as_str() })
        }
        // Path helper: `articles_path()` / `article_path(x)` — any
        // method ending in `_path`.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str().ends_with("_path") =>
        {
            Some(ViewUrlArg::PathHelper { name: method.as_str(), args })
        }
        // Record ref — either Var/Ivar (regular local) or a bare
        // no-arg Send (partial-scope local that Prism parsed as
        // implicit-self method call before scope analysis).
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } if is_local(name.as_str()) => {
            Some(ViewUrlArg::RecordRef { name: name.as_str() })
        }
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && is_local(method.as_str()) => {
            Some(ViewUrlArg::RecordRef { name: method.as_str() })
        }
        // Nested-resource array: `[parent, child]` or deeper.
        ExprNode::Array { elements, .. } if elements.len() >= 2 => {
            Some(ViewUrlArg::NestedArray { elements })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::Symbol;
    use crate::span::Span;

    fn sym(s: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Sym {
                    value: Symbol::from(s),
                },
            },
        )
    }

    fn str_lit(s: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Str { value: s.to_string() },
            },
        )
    }

    #[test]
    fn classifies_csrf_meta_tags() {
        let kind = classify_view_helper("csrf_meta_tags", &[]).unwrap();
        assert!(matches!(kind, ViewHelperKind::CsrfMetaTags));
    }

    #[test]
    fn classifies_content_for_getter() {
        let args = vec![sym("title")];
        let kind = classify_view_helper("content_for", &args).unwrap();
        match kind {
            ViewHelperKind::ContentForGetter { slot } => assert_eq!(slot, "title"),
            _ => panic!("expected ContentForGetter"),
        }
    }

    #[test]
    fn classifies_content_for_setter() {
        let args = vec![sym("title"), str_lit("Articles")];
        let kind = classify_view_helper("content_for", &args).unwrap();
        assert!(matches!(
            kind,
            ViewHelperKind::ContentForSetter { slot: "title", .. }
        ));
    }

    #[test]
    fn dom_id_arity_distinguishes_prefix() {
        let one_args = vec![str_lit("x")];
        let one = classify_view_helper("dom_id", &one_args).unwrap();
        assert!(matches!(
            one,
            ViewHelperKind::DomId { prefix: None, .. }
        ));
        let two_args = vec![str_lit("x"), sym("n")];
        let two = classify_view_helper("dom_id", &two_args).unwrap();
        assert!(matches!(
            two,
            ViewHelperKind::DomId { prefix: Some(_), .. }
        ));
    }

    #[test]
    fn link_to_three_arg_captures_opts() {
        let args = vec![str_lit("Show"), str_lit("/articles/1"), sym("cls")];
        assert!(matches!(
            classify_view_helper("link_to", &args),
            Some(ViewHelperKind::LinkTo { opts: Some(_), .. })
        ));
    }

    #[test]
    fn unknown_method_returns_none() {
        assert!(classify_view_helper("not_a_helper", &[]).is_none());
    }

    #[test]
    fn form_builder_method_aliases() {
        assert_eq!(
            classify_form_builder_method("text_area"),
            Some(FormBuilderMethod::TextArea)
        );
        assert_eq!(
            classify_form_builder_method("textarea"),
            Some(FormBuilderMethod::TextArea)
        );
        assert_eq!(classify_form_builder_method("unknown"), None);
    }

    #[test]
    fn url_arg_literal() {
        let arg = str_lit("/articles");
        let kind = classify_view_url_arg(&arg, &|_: &str| false).unwrap();
        match kind {
            ViewUrlArg::Literal { value } => assert_eq!(value, "/articles"),
            _ => panic!("expected Literal"),
        }
    }

    #[test]
    fn url_arg_path_helper() {
        let arg = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("articles_path"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        match classify_view_url_arg(&arg, &|_: &str| false).unwrap() {
            ViewUrlArg::PathHelper { name, args } => {
                assert_eq!(name, "articles_path");
                assert!(args.is_empty());
            }
            _ => panic!("expected PathHelper"),
        }
    }

    #[test]
    fn url_arg_record_ref_respects_is_local() {
        let arg = Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from("article"),
            },
        );
        assert!(classify_view_url_arg(&arg, &|n: &str| n == "article").is_some());
        assert!(classify_view_url_arg(&arg, &|_: &str| false).is_none());
    }
}
