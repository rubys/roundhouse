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

/// A recognized `render ...` call inside an ERB view body. Each
/// variant captures the IR shape; emitters pick naming, iteration
/// syntax, and decide whether a Var/Ivar name resolves to a known
/// renderable (via `ctx.is_local`, `known_models`, etc.) at
/// emission time.
#[derive(Debug)]
pub enum RenderPartial<'a> {
    /// `render @posts` / `render posts` — iterate a named
    /// collection, calling a partial per element. `name` is the
    /// collection's surface name (used by emitters to derive the
    /// partial-fn name and any foreign-key/class lookups).
    Collection { collection: &'a Expr, name: &'a str },
    /// `render @post.comments` — iterate an association method on
    /// a receiver Var/Ivar, calling a partial per element.
    Association { receiver: &'a Expr, method: &'a str },
    /// `render "posts/post", post: @post` — call a named partial
    /// with the first hash entry's value as its argument. Every
    /// target ignores additional hash entries today; centralizing
    /// that quirk here keeps it documented.
    Named { partial: &'a str, arg: Option<&'a Expr> },
}

/// Recognize a `render ...` call inside an ERB view body. Returns
/// `None` when the shape doesn't match any supported form, when
/// there's a receiver, when a block is attached, or when a
/// one-arg Var/Ivar doesn't name a view-scope local (`is_local`).
pub fn classify_render_partial<'a>(
    recv: Option<&'a Expr>,
    method: &str,
    args: &'a [Expr],
    block: Option<&'a Expr>,
    is_local: &impl Fn(&str) -> bool,
) -> Option<RenderPartial<'a>> {
    if method != "render" || recv.is_some() || block.is_some() {
        return None;
    }
    match args {
        [arg] => match &*arg.node {
            ExprNode::Var { name, .. } | ExprNode::Ivar { name }
                if is_local(name.as_str()) =>
            {
                Some(RenderPartial::Collection {
                    collection: arg,
                    name: name.as_str(),
                })
            }
            ExprNode::Send {
                recv: Some(r),
                method: m,
                args: sub_args,
                ..
            } if sub_args.is_empty()
                && matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) =>
            {
                Some(RenderPartial::Association {
                    receiver: r,
                    method: m.as_str(),
                })
            }
            _ => None,
        },
        [a, b] => {
            let ExprNode::Lit { value: Literal::Str { value: partial } } = &*a.node else {
                return None;
            };
            let ExprNode::Hash { entries, .. } = &*b.node else {
                return None;
            };
            let arg = entries.first().map(|(_k, v)| v);
            Some(RenderPartial::Named {
                partial: partial.as_str(),
                arg,
            })
        }
        _ => None,
    }
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

/// Split a FormBuilder method's positional args into its two
/// Rails-shaped halves: the field-name symbol and the trailing
/// options hash. `form.text_field :title, class: "..."` yields
/// `(Some("title"), Some(&[(class, "...")]))`; `form.submit` with
/// no args yields `(None, None)`. Emitters format the opts pairs
/// themselves — the IR walk is target-neutral.
pub fn classify_form_builder_args(
    args: &[Expr],
) -> (Option<&str>, Option<&[(Expr, Expr)]>) {
    if args.is_empty() {
        return (None, None);
    }
    let (field, rest): (Option<&str>, &[Expr]) = match &*args[0].node {
        ExprNode::Lit { value: Literal::Sym { value } } => (Some(value.as_str()), &args[1..]),
        _ => (None, args),
    };
    let opts = rest.iter().find_map(|a| match &*a.node {
        ExprNode::Hash { entries, .. } => Some(entries.as_slice()),
        _ => None,
    });
    (field, opts)
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

// ── Nested URL/form element classifier ────────────────────────

/// One element of a `[parent, child]` array used in link_to /
/// button_to / form_with's `model:` kwarg. Emitters compose these
/// into a nested path-helper call and its positional id arguments.
///
/// The variants drop the distinction between Var/Ivar/partial-scope
/// bare Send — all three are "local reference" from the emitter's
/// perspective; the binding-kind differences matter to Ruby's
/// parser but not to code generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NestedUrlElement<'a> {
    /// Bare local record — `article`, `comment`, etc. The singular
    /// name is the local's identifier; the id source is `{name}.id`.
    DirectLocal { name: &'a str },
    /// `owner.assoc` belongs-to read — `comment.article`. The
    /// singular name is `assoc`; the id source is `{owner}.{assoc}_id`
    /// (the foreign-key column on the owner).
    Association { owner: &'a str, assoc: &'a str },
}

/// Classify one element of a nested-URL array. Returns None for
/// element shapes we don't recognize (literals, complex chains).
pub fn classify_nested_url_element<'a, F>(
    el: &'a Expr,
    is_local: &F,
) -> Option<NestedUrlElement<'a>>
where
    F: Fn(&str) -> bool,
{
    match &*el.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } if is_local(name.as_str()) => {
            Some(NestedUrlElement::DirectLocal { name: name.as_str() })
        }
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && is_local(method.as_str()) => {
            Some(NestedUrlElement::DirectLocal { name: method.as_str() })
        }
        // `owner.assoc` — belongs_to read. `owner` is a local;
        // `assoc` is the association name (singular).
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if args.is_empty() =>
        {
            let owner = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name }
                    if is_local(name.as_str()) =>
                {
                    Some(name.as_str())
                }
                ExprNode::Send {
                    recv: None,
                    method: m,
                    args: ra,
                    block: None,
                    ..
                } if ra.is_empty() && is_local(m.as_str()) => Some(m.as_str()),
                _ => None,
            }?;
            Some(NestedUrlElement::Association {
                owner,
                assoc: method.as_str(),
            })
        }
        _ => None,
    }
}

// ── Errors-field predicate classifier ──────────────────────────

/// `.errors[:field].none?` / `.errors[:field].any?` (and their
/// aliases `.empty?` / `.present?`). The scaffold's class-array-hash
/// pattern uses this shape to toggle validation-error styling on
/// form inputs. Each emitter lowers to its target's equivalent of
/// `fieldHasError(record.errors, "field")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorsFieldPredicate<'a> {
    /// Local record holding the `errors` collection.
    pub record: &'a str,
    /// Field name the predicate is checking (snake_case).
    pub field: String,
    /// True when the predicate asserts "there IS an error"
    /// (`.any?` / `.present?`); false for absence (`.none?` /
    /// `.empty?`).
    pub expect_present: bool,
}

/// Classify an expression as an errors-field predicate. Returns
/// None for unrecognized shapes.
pub fn classify_errors_field_predicate<'a, F>(
    expr: &'a Expr,
    is_local: &F,
) -> Option<ErrorsFieldPredicate<'a>>
where
    F: Fn(&str) -> bool,
{
    let ExprNode::Send { recv: Some(outer), method: outer_method, args: outer_args, .. } = &*expr.node else {
        return None;
    };
    if !outer_args.is_empty() {
        return None;
    }
    let expect_present = match outer_method.as_str() {
        "none?" | "empty?" => false,
        "any?" | "present?" => true,
        _ => return None,
    };
    // `outer.recv` should be `<record>.errors[:field]`. Match the
    // `[]` send with a single symbol arg and recv of `.errors`.
    let ExprNode::Send { recv: Some(errs_recv), method: brackets, args: idx_args, .. } = &*outer.node else {
        return None;
    };
    if brackets.as_str() != "[]" || idx_args.len() != 1 {
        return None;
    }
    let field = match &*idx_args[0].node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
        ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
        _ => return None,
    };
    let ExprNode::Send { recv: Some(rec), method: errs_method, args: ra, .. } = &*errs_recv.node else {
        return None;
    };
    if errs_method.as_str() != "errors" || !ra.is_empty() {
        return None;
    }
    let record = match &*rec.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } if is_local(name.as_str()) => {
            name.as_str()
        }
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && is_local(method.as_str()) => method.as_str(),
        _ => return None,
    };
    Some(ErrorsFieldPredicate {
        record,
        field,
        expect_present,
    })
}

// ── class: value classifier ────────────────────────────────────

/// The shape of a `class:` option value on link_to / button_to /
/// form-field calls. Rails scaffolds use three common shapes:
///   - A plain string (literal or interp).
///   - `[base_string, {cond_class: cond_expr, …}]` where each
///     `cond_expr` is a recognized errors-field predicate.
///   - Anything else (dynamic), which emitters typically render as
///     an empty class.
///
/// Emitters consume the structured form and produce their target's
/// conditional-concatenation idiom.
#[derive(Debug)]
pub enum ClassValueShape<'a> {
    /// Simple expression (literal string, interp, or other
    /// simple-classifier-accepted shape). Emitters render via the
    /// target's usual expr emitter.
    Simple { expr: &'a Expr },
    /// `[base, {cls: pred, …}]` — conditional-class decorations on
    /// a base string. Order preserved from the source hash.
    Conditional {
        base: &'a Expr,
        clauses: Vec<(String, ErrorsFieldPredicate<'a>)>,
    },
    /// Shape not recognized — emitters fall back to empty class.
    Unknown,
}

/// Classify a `class:` value. `is_local` is threaded to the errors-
/// field predicate classifier for each clause.
pub fn classify_class_value<'a, F>(v: &'a Expr, is_local: &F) -> ClassValueShape<'a>
where
    F: Fn(&str) -> bool,
{
    // Array form: `[base_string, {cond_class: cond_expr, ...}]`.
    if let ExprNode::Array { elements, .. } = &*v.node {
        let Some(base) = elements.first() else {
            return ClassValueShape::Unknown;
        };
        let mut clauses: Vec<(String, ErrorsFieldPredicate<'a>)> = Vec::new();
        for el in elements.iter().skip(1) {
            let ExprNode::Hash { entries, .. } = &*el.node else {
                continue;
            };
            for (hk, hv) in entries {
                let cls_text = match &*hk.node {
                    ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                    _ => continue,
                };
                if let Some(pred) = classify_errors_field_predicate(hv, is_local) {
                    clauses.push((cls_text, pred));
                }
            }
        }
        return ClassValueShape::Conditional { base, clauses };
    }
    // Anything else → delegate the simple-check to the emitter.
    // The classifier can't know what "simple" means for each target
    // (the simple-expr gate differs), so we just hand back the expr
    // and let the emitter decide.
    ClassValueShape::Simple { expr: v }
}

// ── Nested form_with child classifier ──────────────────────────

/// The child element of a nested `form_with model: [parent, child]`.
/// Determines both the record expression and the field-name prefix
/// (`"comment"` → `comment[body]` for field names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NestedFormChild<'a> {
    /// `Class.new` — construct a fresh instance. Prefix is the
    /// snake_case form of the class name.
    ClassNew { class: &'a str },
    /// Bare local or partial-scope Send — an existing record.
    /// Prefix is the local's name (conventionally the singular
    /// snake_case).
    Local { name: &'a str },
}

impl NestedFormChild<'_> {
    /// The field-name prefix Rails uses in the form's `name="…"`
    /// attributes: `"comment"` for both `Comment.new` and a bare
    /// `comment` local.
    pub fn prefix(&self) -> String {
        match self {
            NestedFormChild::ClassNew { class } => crate::naming::snake_case(class),
            NestedFormChild::Local { name } => (*name).to_string(),
        }
    }
}

/// Classify the child element of a nested form_with's `model:`
/// array. Returns None for shapes we don't recognize.
pub fn classify_nested_form_child(el: &Expr) -> Option<NestedFormChild<'_>> {
    match &*el.node {
        // `Class.new`.
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "new" && args.is_empty() =>
        {
            if let ExprNode::Const { path } = &*r.node {
                if let Some(class) = path.last() {
                    return Some(NestedFormChild::ClassNew { class: class.as_str() });
                }
            }
            None
        }
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => {
            Some(NestedFormChild::Local { name: name.as_str() })
        }
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() => Some(NestedFormChild::Local { name: method.as_str() }),
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

    fn var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: crate::ident::VarId(0),
                name: Symbol::from(name),
            },
        )
    }

    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: false,
            },
        )
    }

    #[test]
    fn nested_element_direct_local() {
        let el = var("comment");
        let k = classify_nested_url_element(&el, &|n: &str| n == "comment").unwrap();
        assert_eq!(k, NestedUrlElement::DirectLocal { name: "comment" });
    }

    #[test]
    fn nested_element_association() {
        // `comment.article` — owner is a local, method is assoc.
        let el = send(Some(var("comment")), "article", vec![]);
        let k = classify_nested_url_element(&el, &|n: &str| n == "comment").unwrap();
        assert_eq!(
            k,
            NestedUrlElement::Association {
                owner: "comment",
                assoc: "article",
            }
        );
    }

    #[test]
    fn errors_field_predicate_none() {
        // `article.errors[:title].none?`
        let errors = send(Some(var("article")), "errors", vec![]);
        let indexed = send(Some(errors), "[]", vec![sym("title")]);
        let pred_expr = send(Some(indexed), "none?", vec![]);
        let pred = classify_errors_field_predicate(&pred_expr, &|n: &str| n == "article").unwrap();
        assert_eq!(pred.record, "article");
        assert_eq!(pred.field, "title");
        assert!(!pred.expect_present);
    }

    #[test]
    fn errors_field_predicate_any() {
        let errors = send(Some(var("article")), "errors", vec![]);
        let indexed = send(Some(errors), "[]", vec![sym("body")]);
        let pred_expr = send(Some(indexed), "any?", vec![]);
        let pred = classify_errors_field_predicate(&pred_expr, &|n: &str| n == "article").unwrap();
        assert!(pred.expect_present);
    }

    #[test]
    fn nested_form_child_class_new() {
        let comment_class = Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from("Comment")],
            },
        );
        let el = send(Some(comment_class), "new", vec![]);
        let k = classify_nested_form_child(&el).unwrap();
        assert_eq!(k, NestedFormChild::ClassNew { class: "Comment" });
        assert_eq!(k.prefix(), "comment");
    }

    #[test]
    fn nested_form_child_bare_local() {
        let el = var("comment");
        let k = classify_nested_form_child(&el).unwrap();
        assert_eq!(k, NestedFormChild::Local { name: "comment" });
        assert_eq!(k.prefix(), "comment");
    }
}
