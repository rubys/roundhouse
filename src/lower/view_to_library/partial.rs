//! Render-partial dispatch + yield handling — both are output-position
//! dispatches from `emit_io_append`.

use crate::expr::{Arm, BlockStyle, Expr, ExprNode, Literal, Pattern};
use crate::ident::Symbol;
use crate::naming::{camelize_path, last_segment, singularize, snake_case};
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
        RenderPartial::Named { partial, arg, locals } => {
            let (module_dir, base_name) = match partial.rsplit_once('/') {
                Some((dir, name)) => (dir.to_string(), name.to_string()),
                None => (ctx.resource_dir.clone(), (*partial).to_string()),
            };
            if module_dir.is_empty() {
                return None;
            }
            let module_camel = camelize_path(&snake_case(&module_dir));
            let method_sym = base_name.trim_start_matches('_').to_string();

            // An explicit `locals:` hash binds by NAME: the entry matching
            // the partial's record-arg convention (singular of its dir)
            // becomes the record; remaining entries land at their matching
            // trailing extra-param positions (nil-filled gaps). Without
            // `locals:`, the single bare `name: rec` value stays the record
            // (historical behavior).
            let lookup_local = |name: &str| -> Option<Expr> {
                locals.and_then(|entries| {
                    entries.iter().find_map(|(k, v)| match &*k.node {
                        ExprNode::Lit { value: Literal::Sym { value } }
                            if value.as_str() == name =>
                        {
                            Some(v.clone())
                        }
                        _ => None,
                    })
                })
            };
            let record_name = singularize(&snake_case(last_segment(&module_camel)));
            let strict_key = (module_camel.clone(), method_sym.clone());
            let strict_decl = ctx.strict_locals.get(&strict_key);
            // A strict-locals partial's positional record is its FIRST
            // declared local (the def side builds the signature from the
            // header, not the dir convention — `messages/_form` declares
            // `new_message:`, not `message:`), so bind it by the DECLARED
            // name; an omitted record falls to the header default. The
            // dir-convention singular governs only convention-inferred
            // partials.
            let arg_expr = match (strict_decl, locals.is_some()) {
                (Some(decl), true) => lookup_local(decl[0].name.as_str())
                    .or_else(|| decl[0].default.clone())
                    .unwrap_or_else(nil_lit),
                (Some(decl), false) => arg
                    .cloned()
                    .or_else(|| decl[0].default.clone())
                    .unwrap_or_else(nil_lit),
                (None, true) => lookup_local(&record_name).unwrap_or_else(nil_lit),
                (None, false) => arg.cloned().unwrap_or_else(nil_lit),
            };
            let mut call_args = vec![arg_expr];
            call_args.extend(partial_extra_args(ctx, &module_camel, &method_sym));
            if let Some(decl) = strict_decl {
                // Strict-locals partial: bind each PROVIDED keyword local by
                // name (`render "comments/comment", comment: c, show_story:
                // true` → `Views::Comments.comment(c, show_story: true)`);
                // the record (index 0) rode positionally as `arg_expr` and
                // the closure ivars followed. Omitted optionals fall to the
                // header defaults.
                if let Some(hash) = strict_kwargs(&decl[1..], &lookup_local) {
                    call_args.push(hash);
                }
            } else if locals.is_some() {
                if let Some(extras) = ctx
                    .partial_extras
                    .get(&(module_camel.clone(), method_sym.clone()))
                {
                    // Only emit up to the LAST extra actually provided —
                    // wholly-absent tails keep the short call.
                    let bound: Vec<Option<Expr>> =
                        extras.iter().map(|e| lookup_local(e)).collect();
                    if let Some(last) = bound.iter().rposition(|b| b.is_some()) {
                        for b in bound.into_iter().take(last + 1) {
                            call_args.push(b.unwrap_or_else(nil_lit));
                        }
                    }
                }
            }
            let render_call = send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
                    },
                )),
                &method_sym,
                call_args,
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
        // `render partial: "stories/listdetail", collection: stories, as:
        // :story` — iterate `collection`, calling the explicitly-named
        // partial once per element with the element bound to the `as:`
        // local. Like emit_partial_each but the partial module/method come
        // from the explicit name, and the block var from `as:` (default:
        // the partial's base name).
        RenderPartial::CollectionNamed { collection, partial, as_name } => {
            let (module_dir, base_name) = match partial.rsplit_once('/') {
                Some((dir, name)) => (dir.to_string(), name.to_string()),
                None => (ctx.resource_dir.clone(), (*partial).to_string()),
            };
            if module_dir.is_empty() {
                return None;
            }
            let module_camel = camelize_path(&snake_case(&module_dir));
            let method_sym = base_name.trim_start_matches('_').to_string();
            let var_name = Symbol::from(
                as_name
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| base_name.trim_start_matches('_').to_string()),
            );

            let mut call_args = vec![var_ref(var_name.clone())];
            call_args.extend(partial_extra_args(ctx, &module_camel, &method_sym));
            let render_call = send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
                    },
                )),
                &method_sym,
                call_args,
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
            Some(send(
                Some((*collection).clone()),
                "each",
                Vec::new(),
                Some(block_lambda),
                false,
            ))
        }
        // `render partial: @above` — the name is a runtime value. Emit a
        // `case @above` whose arms are the pooled candidate partials (the
        // string literals controllers assign to `@above`), each resolving
        // to its `Views::<Module>.<method>` with a nil record arg and the
        // threaded closure ivars. A name outside the pool matches no arm →
        // renders nothing (Rails would raise; the pool covers every assigned
        // value). Empty pool → None (leaves the original render unresolved).
        RenderPartial::Template { name } => {
            let (module_camel, method_sym) =
                super::partial_name_to_key(name, &ctx.resource_dir);
            if module_camel.is_empty() {
                return None;
            }
            let method_name =
                crate::lower::view::view_method_name(&method_sym).as_str().to_string();
            // An action view has no record arg — its params are its
            // FULL closure (threaded into the caller by the
            // render-graph fold) plus nil-default extras. No
            // record-name dedup here: `story` is a plain closure param
            // on Views::Stories.show, not a separately-passed record.
            let call_args: Vec<Expr> = ctx
                .partial_ivars
                .get(&(module_camel.clone(), method_sym.clone()))
                .map(|ivars| ivars.iter().map(|n| var_ref(n.clone())).collect())
                .unwrap_or_default();
            let render_call = send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
                    },
                )),
                &method_name,
                call_args,
                None,
                true,
            );
            Some(accumulator_append_call(render_call, ctx))
        }
        RenderPartial::DynamicNamed { name, ivar } => {
            let entries = ctx
                .dyn_pools
                .get(&(ctx.resource_dir.clone(), Symbol::from(*ivar)))
                .cloned()
                .unwrap_or_default();
            let mut arms: Vec<Arm> = Vec::new();
            for entry in &entries {
                let pname = &entry.name;
                let (module_camel, method_sym) =
                    super::partial_name_to_key(pname, &ctx.resource_dir);
                if module_camel.is_empty() {
                    continue;
                }
                // The entry's options-form `locals:` resolve here: an
                // ivar value was folded into this view's closure (see
                // view_ivar_closures' dynamic edges), so it reads as the
                // threaded local; a literal passes through verbatim.
                let lookup = |n: &str| -> Option<Expr> {
                    entry.locals.iter().find(|(k, _)| k.as_str() == n).map(|(_, v)| {
                        match &*v.node {
                            ExprNode::Ivar { name } => var_ref(Symbol::from(
                                crate::naming::safe_local(name.as_str()),
                            )),
                            _ => v.clone(),
                        }
                    })
                };
                let strict_key = (module_camel.clone(), method_sym.clone());
                let strict_decl = ctx.strict_locals.get(&strict_key);
                // A strict-locals target's record slot is its first
                // declared local — bind it from the entry's locals by
                // the DECLARED name (header default when absent). A
                // convention target gets no record object: nil, then
                // its closure ivars (threaded as this view's params).
                let record = match strict_decl {
                    Some(decl) => lookup(decl[0].name.as_str())
                        .or_else(|| decl[0].default.clone())
                        .unwrap_or_else(nil_lit),
                    None => nil_lit(),
                };
                let mut call_args = vec![record];
                call_args.extend(partial_extra_args(ctx, &module_camel, &method_sym));
                if let Some(decl) = strict_decl {
                    if let Some(hash) = strict_kwargs(&decl[1..], &lookup) {
                        call_args.push(hash);
                    }
                }
                let render_call = send(
                    Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Const {
                            path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
                        },
                    )),
                    &method_sym,
                    call_args,
                    None,
                    true,
                );
                arms.push(Arm {
                    pattern: Pattern::Lit {
                        value: Literal::Str { value: pname.clone() },
                    },
                    guard: None,
                    body: accumulator_append_call(render_call, ctx),
                });
            }
            if arms.is_empty() {
                return None;
            }
            Some(Expr::new(
                Span::synthetic(),
                ExprNode::Case { scrutinee: (*name).clone(), arms },
            ))
        }
    }
}

/// Build the trailing keyword-args hash for a strict-locals partial call
/// from the locals a render site provides. `kw_locals` is the partial's
/// declared KEYWORD tail (record excluded); `lookup` resolves a local
/// name to its provided value. Only declared locals the caller actually
/// supplies are emitted — the rest fall to the header's defaults. Returns
/// `None` when nothing beyond the record is passed (a bare positional call).
fn strict_kwargs(
    kw_locals: &[crate::dialect::Param],
    lookup: &impl Fn(&str) -> Option<Expr>,
) -> Option<Expr> {
    let entries: Vec<(Expr, Expr)> = kw_locals
        .iter()
        .filter_map(|p| lookup(p.name.as_str()).map(|v| (lit_sym(p.name.clone()), v)))
        .collect();
    if entries.is_empty() {
        return None;
    }
    Some(Expr::new(
        Span::synthetic(),
        ExprNode::Hash { entries, kwargs: true },
    ))
}

/// The threaded ivar args a rendered partial needs (its render-tree
/// closure), looked up by `(module, method)`. These are the calling
/// view's own locals (its closure ⊇ the partial's), passed positionally
/// after the record arg to match the partial's generated signature.
fn partial_extra_args(ctx: &ViewCtx, module: &str, method: &str) -> Vec<Expr> {
    // The partial's record arg (singular of its dir) is passed separately
    // and covers any same-named ivar, so exclude it from the threaded set —
    // matching the dedup on the partial's def side (build_library_class).
    let record_name = singularize(&snake_case(last_segment(module)));
    let key = (module.to_string(), method.to_string());
    // For a strict-locals partial, its def-side closure excludes EVERY
    // declared local (a name that is both a local and an `@ivar` collapses
    // after the ivar→local rewrite). Exclude the same set here so the
    // threaded positional closure args line up with the signature.
    let declared: std::collections::HashSet<&str> = ctx
        .strict_locals
        .get(&key)
        .map(|ps| ps.iter().map(|p| p.name.as_str()).collect())
        .unwrap_or_default();
    ctx.partial_ivars
        .get(&key)
        .map(|ivars| {
            ivars
                .iter()
                .filter(|n| n.as_str() != record_name && !declared.contains(n.as_str()))
                // Caller bodies are post-ivar-rewrite: a reserved-word
                // ivar (`@for`) lives there as its `safe_local` form.
                .map(|n| var_ref(Symbol::from(crate::naming::safe_local(n.as_str()))))
                .collect()
        })
        .unwrap_or_default()
}

/// Common shape for collection / association partial renders:
/// `<recv>.each { |x| <accumulator> << Views::<Plural>.<singular>(x) }`.
/// `plural_name` is the resource name in plural form
/// (`articles` / `comments`); the partial-fn name is its singular
/// (`article` / `comment`); the variable name is the singular's first
/// letter (`a` / `c`).
fn emit_partial_each(recv: &Expr, plural_name: &str, ctx: &ViewCtx) -> Expr {
    let singular = singularize(plural_name);
    let plural_camel = camelize_path(&snake_case(plural_name));
    let var_name = Symbol::from(singular.chars().next().unwrap_or('x').to_string());

    let mut call_args = vec![var_ref(var_name.clone())];
    call_args.extend(partial_extra_args(ctx, &plural_camel, &singular));
    let render_call = send(
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from("Views"), Symbol::from(plural_camel.clone())],
            },
        )),
        &singular,
        call_args,
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
