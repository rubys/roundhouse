//! Lower a `View` (compiled-ERB IR) into a `LibraryClass` whose body is
//! one `module_function`-style class method per view, with bodies in
//! spinel-blog shape:
//!
//!   io = String.new
//!   io << ViewHelpers.turbo_stream_from("articles")
//!   ViewHelpers.content_for_set(:title, "Articles")
//!   if !articles.empty?
//!     articles.each { |a| io << Views::Articles.article(a) }
//!   end
//!   io
//!
//! Helper-call rewrites (`turbo_stream_from` → `ViewHelpers.turbo_stream_from`,
//! `link_to text, url` → `ViewHelpers.link_to(text, RouteHelpers.<x>_path(...))`,
//! auto-escape on bare interpolation, …) and render-partial dispatch
//! happen here so per-target emitters consume canonical IR — the same
//! rationale as `model_to_library` and `controller_to_library`.
//!
//! Scope of this first slice: the helpers needed by `articles/index.html.erb`
//! (turbo_stream_from, content_for setter, link_to with path-helper URL,
//! render @collection, `.any?`-style predicates, html_escape on bare
//! interpolation). FormBuilder/form_with capture, content_for capture,
//! errors-field predicates, and conditional-class composition land in
//! follow-on slices once their forcing fixtures are exercised.

mod predicates;
mod extra_params;
mod walker;
mod helpers;
mod partial;
mod form_with;
mod form_builder;

use crate::App;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, Param, View};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::{camelize, singularize, snake_case};
use crate::span::Span;

use self::extra_params::collect_extra_params;
use self::walker::walk_body;

/// Entry point. Turn one `View` into a one-method `LibraryClass`.
/// `app` is consulted only for known model names (so view args can be
/// typed implicitly downstream) and for FK resolution; the lowering is
/// otherwise pure.
pub fn lower_view_to_library_class(view: &View, app: &App) -> LibraryClass {
    let (dir, base) = split_view_name(view.name.as_str());
    let stem = base.trim_start_matches('_');

    let module_id = view_module_id(dir);
    let method_name = Symbol::from(stem);

    let known_models: Vec<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    let arg_name = infer_view_arg(stem, dir, base.starts_with('_'), &known_models);

    // Rewrite `@ivar` → bare `ivar` everywhere so the inferred arg name
    // (and any extra params we surface) read as plain locals in the
    // emitted body. Mirrors the controller-side ivar-to-local pass.
    let rewritten = rewrite_ivars_to_locals(&view.body);

    // Collect free names other than the inferred arg → those become
    // additional positional params. Today this picks up `notice`,
    // `alert`, etc. (Rails flash helpers parsed as bare Sends/Vars).
    let extra_params = collect_extra_params(&rewritten, &arg_name);

    // The inferred record arg (e.g. `articles`, `article`) is the
    // required positional. Free locals discovered downstream
    // (`notice`, `alert`, …) get a `nil` default so controllers that
    // don't have a flash to pass can still call `Views::X.action(rec)`
    // without arity errors. Spinel-blog's hand-written views use
    // keyword-with-default for these (`notice: nil`); the lowerer
    // models the same callability with positional-with-nil-default
    // until kw-args are first-class in `Param`.
    let nil_default = Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Nil },
    );
    let mut params: Vec<Param> = Vec::new();
    if !arg_name.is_empty() {
        params.push(Param::positional(Symbol::from(arg_name.clone())));
    }
    for n in &extra_params {
        params.push(Param::with_default(
            Symbol::from(n.clone()),
            nil_default.clone(),
        ));
    }

    let mut locals: Vec<String> = Vec::new();
    if !arg_name.is_empty() {
        locals.push(arg_name.clone());
    }
    locals.extend(extra_params.iter().cloned());

    let ctx = ViewCtx {
        locals,
        arg_name: arg_name.clone(),
        resource_dir: dir.to_string(),
        accumulator: "io".to_string(),
        form_records: Vec::new(),
        nullable_locals: extra_params.iter().cloned().collect(),
    };

    let mut body_stmts: Vec<Expr> = Vec::new();
    body_stmts.push(assign_accumulator_string_new(&ctx.accumulator));
    body_stmts.extend(walk_body(&rewritten, &ctx));
    body_stmts.push(var_ref(Symbol::from(ctx.accumulator.as_str())));

    let body = seq(body_stmts);

    let method = MethodDef {
        name: method_name,
        receiver: MethodReceiver::Class,
        params,
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(module_id.0.clone()),
    };

    LibraryClass {
        name: module_id,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods: vec![method],
    }
}

// ── view-name → module / arg / method helpers ────────────────────

fn split_view_name(name: &str) -> (&str, &str) {
    name.rsplit_once('/').unwrap_or(("", name))
}

/// Module the view's method lives under: `Views::Articles` for an
/// `articles/...` view. Empty `dir` (uncommon — top-level view) maps
/// to the bare `Views` module.
fn view_module_id(dir: &str) -> ClassId {
    if dir.is_empty() {
        return ClassId(Symbol::from("Views"));
    }
    let camelized = camelize(&snake_case(dir));
    ClassId(Symbol::from(format!("Views::{camelized}")))
}

/// Pick the single positional parameter name for a view. Action views
/// (`articles/index`) take the plural collection (`articles`); show /
/// new / edit / create / update / destroy + partials take the singular
/// (`article`). Layouts take `body` (the rendered inner-view string;
/// bare `yield` in the layout source resolves to this local). Top-
/// level views with no resource directory fall back to an empty arg
/// name (no positional param).
fn infer_view_arg(stem: &str, dir: &str, is_partial: bool, _known_models: &[String]) -> String {
    if dir.is_empty() {
        return String::new();
    }
    if dir == "layouts" {
        return "body".to_string();
    }
    if is_partial {
        return singularize(dir);
    }
    match stem {
        "index" => dir.to_string(),
        _ => singularize(dir),
    }
}

// ── ivar → local rewrite ─────────────────────────────────────────

/// Rewrite every `@ivar` read (and Ivar-LValue assign) under `expr`
/// into a bare `Var` of the same name. The inferred view arg + any
/// extra params resolve to those rewritten Vars in the emitted body.
fn rewrite_ivars_to_locals(expr: &Expr) -> Expr {
    let new_node = match &*expr.node {
        ExprNode::Ivar { name } => ExprNode::Var { id: VarId(0), name: name.clone() },
        ExprNode::Assign { target: LValue::Ivar { name }, value } => ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: name.clone() },
            value: rewrite_ivars_to_locals(value),
        },
        ExprNode::Assign { target, value } => ExprNode::Assign {
            target: rewrite_lvalue(target),
            value: rewrite_ivars_to_locals(value),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(rewrite_ivars_to_locals),
            method: method.clone(),
            args: args.iter().map(rewrite_ivars_to_locals).collect(),
            block: block.as_ref().map(rewrite_ivars_to_locals),
            parenthesized: *parenthesized,
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(rewrite_ivars_to_locals).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite_ivars_to_locals(cond),
            then_branch: rewrite_ivars_to_locals(then_branch),
            else_branch: rewrite_ivars_to_locals(else_branch),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_ivars_to_locals(left),
            right: rewrite_ivars_to_locals(right),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(rewrite_ivars_to_locals).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (rewrite_ivars_to_locals(k), rewrite_ivars_to_locals(v)))
                .collect(),
            braced: *braced,
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_ivars_to_locals(body),
            block_style: *block_style,
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    InterpPart::Expr { expr } => InterpPart::Expr {
                        expr: rewrite_ivars_to_locals(expr),
                    },
                })
                .collect(),
        },
        other => other.clone(),
    };
    Expr::new(expr.span, new_node)
}

fn rewrite_lvalue(lv: &LValue) -> LValue {
    match lv {
        LValue::Var { id, name } => LValue::Var { id: *id, name: name.clone() },
        LValue::Ivar { name } => LValue::Var { id: VarId(0), name: name.clone() },
        LValue::Attr { recv, name } => LValue::Attr {
            recv: rewrite_ivars_to_locals(recv),
            name: name.clone(),
        },
        LValue::Index { recv, index } => LValue::Index {
            recv: rewrite_ivars_to_locals(recv),
            index: rewrite_ivars_to_locals(index),
        },
    }
}

// ── ViewCtx ──────────────────────────────────────────────────────

#[derive(Clone)]
#[allow(dead_code)] // arg_name + resource_dir read in follow-on slices.
pub(super) struct ViewCtx {
    pub(super) locals: Vec<String>,
    pub(super) arg_name: String,
    pub(super) resource_dir: String,
    /// Name of the local that accumulates output via `<<`. The
    /// top-level method body uses `io`; inside `form_with do |form|
    /// … end` blocks (and other capture-style helpers) the inner
    /// walk uses a fresh `body` so the captured string can be
    /// returned to the wrapping helper. Threaded through walk_body
    /// → walk_stmt → emit_io_append so every accumulator append
    /// resolves to the right local.
    pub(super) accumulator: String,
    /// FormBuilder bindings active at this scope: `(local_name,
    /// record_name)` pairs. Populated when entering a `form_with`
    /// block; consumed by the FormBuilder method dispatch so
    /// `form.text_field :title` resolves to the bound record's
    /// model. Cleared on block exit.
    pub(super) form_records: Vec<(String, String)>,
    /// Locals known to be nullable — the view's extra_params with a
    /// `nil` default (`notice`, `alert`, …). When a predicate
    /// (`recv.present?`, `recv.empty?`, …) targets one of these,
    /// rewrite to the nil-safe form `!recv.nil? && !recv.empty?` so
    /// the body doesn't NoMethodError when callers omit the kwarg.
    pub(super) nullable_locals: std::collections::HashSet<String>,
}

impl ViewCtx {
    pub(super) fn is_local(&self, n: &str) -> bool {
        self.locals.iter().any(|x| x == n)
    }
    pub(super) fn with_locals(&self, more: impl IntoIterator<Item = String>) -> Self {
        let mut next = self.clone();
        for n in more {
            if !next.locals.iter().any(|x| x == &n) {
                next.locals.push(n);
            }
        }
        next
    }
}

// ── small IR constructors ────────────────────────────────────────

/// `<accumulator> = String.new` — synthesized once per template body.
/// The accumulator name comes from the active ViewCtx (`io` at top
/// level; `body` inside `form_with` blocks).
pub(super) fn assign_accumulator_string_new(name: &str) -> Expr {
    let string_const = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("String")] },
    );
    let new_call = send(Some(string_const), "new", Vec::new(), None, false);
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from(name) },
            value: new_call,
        },
    )
}

/// `<accumulator> << <arg>` — the per-step append. Always emits with
/// `<<` (a binary operator the Ruby emit_send_base rewrites to infix
/// form), so the source comes out as `io << arg`, not `io.<<(arg)`.
pub(super) fn accumulator_append_call(arg: Expr, ctx: &ViewCtx) -> Expr {
    send(
        Some(var_ref(Symbol::from(ctx.accumulator.as_str()))),
        "<<",
        vec![arg],
        None,
        false,
    )
}

pub(super) fn view_helpers_call(method: &str, args: Vec<Expr>) -> Expr {
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("ViewHelpers")] },
    );
    send(Some(recv), method, args, None, true)
}

pub(super) fn route_helpers_call(method: &str, args: Vec<Expr>) -> Expr {
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
    );
    send(Some(recv), method, args, None, true)
}

pub(super) fn inflector_call(method: &str, args: Vec<Expr>) -> Expr {
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("Inflector")] },
    );
    send(Some(recv), method, args, None, true)
}

/// A `Send` constructor that makes the parenthesized flag explicit on
/// the call site. The Ruby emitter ignores the flag for zero-arg calls
/// (always emits `recv.method`), so it's safe to pass `true` for any
/// helper Send regardless of arity.
pub(super) fn send(
    recv: Option<Expr>,
    method: &str,
    args: Vec<Expr>,
    block: Option<Expr>,
    parenthesized: bool,
) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block,
            parenthesized,
        },
    )
}

pub(super) fn lit_str(s: String) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Str { value: s } },
    )
}

pub(super) fn lit_sym(s: Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Sym { value: s } },
    )
}

pub(super) fn nil_lit() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil })
}

pub(super) fn var_ref(name: Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name })
}

pub(super) fn seq(exprs: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
}

/// Placeholder for unrecognized template shapes — keeps the lowered
/// output well-formed Ruby (a no-op string append) so the file parses.
/// The tag is purely advisory; callers can grep for it to find gaps.
/// The accumulator-aware path uses `walk_stmt`'s ctx, but this helper
/// has none in scope, so it falls back to the default `io` accumulator.
/// Acceptable since today's gaps either land at the top level or
/// inside scopes that still have an `io` shadow at runtime.
pub(super) fn todo_io_append(tag: &str) -> Expr {
    let _ = tag;
    send(
        Some(var_ref(Symbol::from("io"))),
        "<<",
        vec![lit_str(String::new())],
        None,
        false,
    )
}

// ── tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_id_for_articles_dir() {
        let id = view_module_id("articles");
        assert_eq!(id.0.as_str(), "Views::Articles");
    }

    #[test]
    fn arg_name_index_is_plural() {
        let n = infer_view_arg("index", "articles", false, &[]);
        assert_eq!(n, "articles");
    }

    #[test]
    fn arg_name_partial_is_singular() {
        let n = infer_view_arg("article", "articles", true, &[]);
        assert_eq!(n, "article");
    }

    #[test]
    fn arg_name_show_is_singular() {
        let n = infer_view_arg("show", "articles", false, &[]);
        assert_eq!(n, "article");
    }
}
