//! Render-partial dispatch + yield handling — both are output-position
//! dispatches from `emit_io_append`.

use crate::expr::{BlockStyle, Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::naming::{camelize, singularize, snake_case};
use crate::span::Span;

use crate::lower::view::RenderPartial;

use super::{
    accumulator_append_call, lit_sym, nil_lit, send, var_ref, view_helpers_call, ViewCtx,
};

pub(super) fn emit_render_partial(rp: &RenderPartial<'_>, ctx: &ViewCtx) -> Option<Expr> {
    match rp {
        // `render articles` (collection) — iterate, rendering one
        // partial per element. The partial-fn name comes from the
        // singular form of the local: `articles` → `Views::Articles
        // .article(a)`. The inner `io << ...` uses the active
        // accumulator so a `render @articles` inside a form_with
        // capture appends to `body` rather than the outer `io`.
        RenderPartial::Collection { name, .. } => {
            let plural = *name;
            let collection_recv = var_ref(Symbol::from(plural));
            Some(emit_partial_each(&collection_recv, plural, ctx))
        }
        // `render "form", article: @article` — explicit-name partial.
        // Rewrites to `<accumulator> << Views::<Plural>.<method>(arg)`
        // — a single Send append, NOT an each block, since named
        // partials render once. The partial path can be a slash-form
        // (`"comments/comment"`) routing to a different module; bare
        // names (`"form"`) resolve to the current resource_dir's
        // module. The arg is the first hash entry's value (Rails
        // convention; additional hash entries get dropped today —
        // matches existing classifier policy).
        RenderPartial::Named { partial, arg } => {
            let (module_dir, base_name) = match partial.rsplit_once('/') {
                Some((dir, name)) => (dir.to_string(), name.to_string()),
                None => (ctx.resource_dir.clone(), (*partial).to_string()),
            };
            if module_dir.is_empty() {
                return None;
            }
            let module_camel = camelize(&snake_case(&module_dir));
            let method_sym = base_name.trim_start_matches('_').to_string();

            let arg_expr = arg.cloned().unwrap_or_else(nil_lit);
            let render_call = send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
                    },
                )),
                &method_sym,
                vec![arg_expr],
                None,
                true,
            );
            Some(accumulator_append_call(render_call, ctx))
        }
        // `render @article.comments` — has_many association iteration.
        // The `receiver` is the post-ivar-rewrite `Var(article)` (or a
        // bareword `Send`); `method` is the assoc name, plural. We
        // build `receiver.method.each { |c| io << Views::<Plural>
        // .<singular>(c) }`. No has_many table lookup needed: the
        // method dispatch on `receiver` resolves to whatever the
        // model's lowered association method returns at runtime.
        RenderPartial::Association { receiver, method } => {
            let assoc_recv = send(
                Some((*receiver).clone()),
                method,
                Vec::new(),
                None,
                false,
            );
            Some(emit_partial_each(&assoc_recv, method, ctx))
        }
    }
}

/// Common shape for collection / association partial renders:
/// `<recv>.each { |x| <accumulator> << Views::<Plural>.<singular>(x) }`.
/// `plural_name` is the resource name in plural form
/// (`articles` / `comments`); the partial-fn name is its singular
/// (`article` / `comment`); the variable name is the singular's first
/// letter (`a` / `c`).
fn emit_partial_each(recv: &Expr, plural_name: &str, ctx: &ViewCtx) -> Expr {
    let singular = singularize(plural_name);
    let plural_camel = camelize(&snake_case(plural_name));
    let var_name = Symbol::from(singular.chars().next().unwrap_or('x').to_string());

    let render_call = send(
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from("Views"), Symbol::from(plural_camel)],
            },
        )),
        &singular,
        vec![var_ref(var_name.clone())],
        None,
        true,
    );
    let inner = accumulator_append_call(render_call, ctx);
    let block_lambda = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![var_name],
            block_param: None,
            body: inner,
            block_style: BlockStyle::Brace,
        },
    );
    send(
        Some(recv.clone()),
        "each",
        Vec::new(),
        Some(block_lambda),
        false,
    )
}

// ── yield ────────────────────────────────────────────────────────

/// Lower a Yield expr the layout body can produce:
///   `<%= yield %>`        →  `body` (the layout's body parameter)
///   `<%= yield :slot %>`  →  `ViewHelpers.get_slot(:slot)`
/// Bare yield uses the inferred view arg (`body` for layouts).
/// Outside of layouts, bare yield is malformed Rails ERB anyway —
/// we still emit a Var read against whatever the inferred arg was.
pub(super) fn emit_yield(args: &[Expr], ctx: &ViewCtx) -> Expr {
    if let Some(first) = args.first() {
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*first.node {
            return view_helpers_call("get_slot", vec![lit_sym(value.clone())]);
        }
    }
    let local_name = if ctx.arg_name.is_empty() {
        "body".to_string()
    } else {
        ctx.arg_name.clone()
    };
    var_ref(Symbol::from(local_name))
}
