//! Render-partial dispatch + yield handling — both are output-position
//! dispatches from `emit_io_append`.

use crate::expr::{
    Arm, BlockStyle, Expr, ExprNode, Literal, LValue, Pattern,
};
use crate::ident::{Symbol, VarId};
use crate::naming::{camelize_path, last_segment, singularize, snake_case};
use crate::span::Span;

use crate::lower::view::RenderPartial;

use super::extra_params::{flash_params, local_key_of};
use super::{
    accumulator_append_call, assign_accumulator_string_new, lit_sym, nil_lit, send, seq,
    var_ref, view_helpers_call, ViewCtx, ViewKind,
};

/// A collection render whose `each` sets `flag` before each record
/// renders, so `render(x) || fallback` can tell whether anything
/// rendered without evaluating the collection a second time. `None`
/// for the renders of one partial and for the cached collections,
/// whose records render through several loops, hits from the store.
pub(super) fn emit_flagged_collection_render(
    rp: &RenderPartial<'_>,
    flag: &Symbol,
    ctx: &ViewCtx,
) -> Option<Expr> {
    match rp {
        RenderPartial::Collection { .. }
        | RenderPartial::Association { .. }
        | RenderPartial::TypedCollection { .. }
        | RenderPartial::CollectionNamed { cached: false, cached_key: None, .. } => {}
        _ => return None,
    }
    let render = emit_render_partial(rp, ctx)?;
    // After the locals `bind_locals_once` binds, the `each` is last.
    let mut stmts = match &*render.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![render],
    };
    let mut each = stmts.pop()?;
    let ExprNode::Send { method, block: Some(block), .. } = &mut *each.node else {
        return None;
    };
    if method.as_str() != "each" {
        return None;
    }
    let ExprNode::Lambda { body, .. } = &mut *block.node else {
        return None;
    };
    let set = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: flag.clone() },
            value: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value: true } }),
        },
    );
    *body = seq(vec![set, body.clone()]);
    if stmts.is_empty() {
        return Some(each);
    }
    stmts.push(each);
    Some(seq(stmts))
}

pub(super) fn emit_render_partial(rp: &RenderPartial<'_>, ctx: &ViewCtx) -> Option<Expr> {
    match rp {
        // `render articles` (collection) — iterate, rendering one
        // partial per element. The partial-fn name comes from the
        // singular form of the local: `articles` → `Views::Articles
        // .article(a)`. The inner `io << ...` uses the active
        // accumulator so a `render @articles` inside a form_with
        // capture appends to `body` rather than the outer `io`.
        RenderPartial::Collection { collection, name, locals } => {
            let plural = *name;
            let collection_recv = var_ref(Symbol::from(plural));
            bind_locals_once(collection_recv, *locals, collection.span, ctx, |recv, locals| {
                Some(emit_partial_each(recv, plural, locals, ctx))
            })
        }
        // `render @message` (one record) — a single append, not an
        // `each`. Rails resolves the partial through the record's
        // `to_partial_path`: its own name in its PLURAL directory,
        // `messages/_message`. Spelled here exactly as
        // `render_partial_key` spells the graph edge, so the partial's
        // closure args line up with the call. Its locals are bound
        // before the call when it would run them out of source order
        // (`evaluated_out_of_order`).
        RenderPartial::Record { record, name, locals } => {
            let singular = (*name).to_string();
            let plural_camel = camelize_path(&crate::naming::pluralize_snake(&singular));
            let stem = ctx.partial_stems.stem(&plural_camel, &singular);
            let mut record_ref = var_ref(Symbol::from(crate::naming::safe_local(&singular)));
            let mut binds = Vec::new();
            let bound;
            let mut locals = *locals;
            let entries = locals.unwrap_or_default();
            if evaluated_out_of_order(entries, call_position(&plural_camel, &stem, None, ctx), ctx) {
                record_ref = bind_if_first(record_ref, "record", record.span, entries, ctx, &mut binds);
                bound = bind_effectful_locals(entries, record.span, ctx, &mut binds);
                locals = Some(&bound);
            }
            let mut call_args = vec![record_ref];
            call_args.extend(partial_extra_args(ctx, &plural_camel, &stem));
            call_args.extend(locals_extra_args(locals, &plural_camel, &stem, ctx));
            let method = ctx.render_method(ViewKind::Partial, &plural_camel, &stem);
            let call = send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("Views"), Symbol::from(plural_camel)],
                    },
                )),
                method.as_str(),
                call_args,
                None,
                true,
            );
            let append = accumulator_append_call(call, ctx);
            if binds.is_empty() {
                return Some(append);
            }
            binds.push(append);
            Some(seq(binds))
        }
        // `render "form", article: @article` — explicit-name partial.
        // Rewrites to `<accumulator> << Views::<Plural>.<method>(arg)`
        // — a single Send append, NOT an each block, since named
        // partials render once. The partial path can be a slash-form
        // (`"comments/comment"`) routing to a different module; bare
        // names (`"form"`) resolve to the current resource_dir's
        // module. The arg is the first hash entry's value (Rails
        // convention; additional hash entries get dropped today —
        // matches existing classifier policy). Its locals are bound as a
        // record render's are.
        RenderPartial::Named { partial, arg, locals } => {
            let entries = locals.unwrap_or_default();
            let (module, stem) = super::partial_name_to_key(partial, &ctx.resource_dir, &ctx.partial_stems);
            let record = match ctx.strict_locals.get(&(module.clone(), stem.clone())) {
                Some(decl) => decl[0].name.as_str().to_string(),
                None => singularize(&snake_case(last_segment(&module))),
            };
            if !evaluated_out_of_order(entries, call_position(&module, &stem, Some(&record), ctx), ctx) {
                let call = named_partial_call(partial, *arg, *locals, ctx)?;
                return Some(accumulator_append_call(call, ctx));
            }
            // The temps are named after the first local's value, as a
            // collection's are after the collection.
            let mut binds = Vec::new();
            let bound = bind_effectful_locals(entries, entries[0].1.span, ctx, &mut binds);
            binds.push(accumulator_append_call(named_partial_call(partial, *arg, Some(&bound), ctx)?, ctx));
            Some(seq(binds))
        }
        // `render layout: "x" do … end` needs MULTIPLE statements (the
        // capture local, then the call), so the walker routes it to
        // `emit_layout_block` before reaching here. Anything that arrives
        // at this arm has no single-expression form.
        RenderPartial::LayoutBlock { .. } => None,        // `render @article.comments` — has_many association iteration.
        // The `receiver` is the post-ivar-rewrite `Var(article)` (or a
        // bareword `Send`); `method` is the assoc name, plural. We
        // build `receiver.method.each { |c| io << Views::<Plural>
        // .<singular>(c) }`. No has_many table lookup needed: the
        // method dispatch on `receiver` resolves to whatever the
        // model's lowered association method returns at runtime.
        RenderPartial::Association { receiver, method, locals } => {
            let assoc_recv = send(
                Some((*receiver).clone()),
                method,
                Vec::new(),
                None,
                false,
            );
            bind_locals_once(assoc_recv, *locals, receiver.span, ctx, |recv, locals| {
                Some(emit_partial_each(recv, method, locals, ctx))
            })
        }
        // `render @author.posts.order(:title)` — the chain itself is the
        // each receiver, and the element class's partial renders each
        // record, exactly as `render partial: "posts/post", collection:
        // <chain>` does.
        RenderPartial::TypedCollection { collection, partial, locals } => {
            bind_locals_once((*collection).clone(), *locals, collection.span, ctx, |recv, locals| {
                emit_named_collection_each(recv, partial, None, locals, ctx)
            })
        }
        // `render partial: "stories/listdetail", collection: stories, as:
        // :story` — iterate `collection`, calling the explicitly-named
        // partial once per element with the element bound to the `as:`
        // local. Like emit_partial_each but the partial module/method come
        // from the explicit name, and the block var from `as:` (default:
        // the partial's base name).
        RenderPartial::CollectionNamed {
            collection,
            partial,
            as_name,
            locals,
            cached,
            cached_key,
        } => bind_locals_once((*collection).clone(), *locals, collection.span, ctx, |recv, locals| {
            if let Some(key) = cached_key {
                wrap_keyed_cached_collection(recv, partial, *as_name, locals, key, KeyedCache::Callable, ctx)
            } else if *cached {
                wrap_cached_true_collection(recv, partial, *as_name, locals, ctx)
            } else {
                emit_named_collection_each(recv, partial, *as_name, locals, ctx)
            }
        }),
        // `render partial: @above` — the name is a runtime value. Emit a
        // `case @above` whose arms are the pooled candidate partials (the
        // string literals controllers assign to `@above`), each resolving
        // to its `Views::<Module>.<method>` with a nil record arg and the
        // threaded closure ivars. A name outside the pool matches no arm →
        // renders nothing (Rails would raise; the pool covers every assigned
        // value). Empty pool → None (leaves the original render unresolved).
        RenderPartial::Template { name } => {
            let (module_camel, method_sym) =
                super::template_name_to_key(name, &ctx.resource_dir);
            if module_camel.is_empty() {
                return None;
            }
            let method_name = ctx
                .render_method(ViewKind::Template, &module_camel, &method_sym)
                .as_str()
                .to_string();
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
                    super::partial_name_to_key(pname, &ctx.resource_dir, &ctx.partial_stems);
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
                            ExprNode::Ivar { name } => {
                                var_ref(Symbol::from(ctx.ivar_local(name.as_str())))
                            }
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
                    if let Some(hash) = strict_kwargs(&decl[1..], &lookup, context_kwargs(ctx, &module_camel, &method_sym)) {
                        call_args.push(hash);
                    }
                }
                let method = ctx.render_method(ViewKind::Partial, &module_camel, &method_sym);
                let render_call = send(
                    Some(Expr::new(
                        Span::synthetic(),
                        ExprNode::Const {
                            path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
                        },
                    )),
                    method.as_str(),
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

/// Rails evaluates a render's hash once, before the first record renders
/// and when there is none, but a collection render passes its locals to
/// each record's partial from inside the `each`. So `render @posts, flag:
/// advance!` binds `__local_<site>_flag = advance!` before the loop and
/// passes the temp, in source order; a local that only reads
/// (`reads_without_effect`) stays inline, and without any other local
/// `emit` gets the render as written. The collection, which Ruby
/// evaluates before the hash, is bound first then, even when it only
/// reads, since a local may reassign what it reads (`render posts, flag:
/// (posts = others)`); not when an explicit `locals:` is written before
/// its `collection:`, which the `each` then evaluates after them, as Ruby
/// does. `site` is the collection's source span, which names the temps
/// apart from those of another render in the view.
fn bind_locals_once(
    collection: Expr,
    locals: Option<&[(Expr, Expr)]>,
    site: Span,
    ctx: &ViewCtx,
    emit: impl FnOnce(&Expr, Option<&[(Expr, Expr)]>) -> Option<Expr>,
) -> Option<Expr> {
    let entries = locals.unwrap_or_default();
    if entries.iter().all(|(_, v)| reads_without_effect(v, ctx)) {
        return emit(&collection, locals);
    }
    let mut binds = Vec::new();
    let collection = bind_if_first(collection, "collection", site, entries, ctx, &mut binds);
    let bound = bind_effectful_locals(entries, site, ctx, &mut binds);
    binds.push(emit(&collection, Some(&bound))?);
    Some(seq(binds))
}

/// The render's first argument, bound to `__<kind>_<site>` when it is
/// written before the first local `bind_effectful_locals` binds, so that
/// local cannot change what it reads; otherwise as it is.
fn bind_if_first(
    subject: Expr,
    kind: &str,
    site: Span,
    entries: &[(Expr, Expr)],
    ctx: &ViewCtx,
    binds: &mut Vec<Expr>,
) -> Expr {
    let first = entries.iter().find(|(_, v)| !reads_without_effect(v, ctx)).map(|(_, v)| v.span);
    let written_first = first.is_none_or(|first| site.is_synthetic() || first.is_synthetic() || site.start < first.start);
    if !written_first {
        return subject;
    }
    let name = Symbol::from(format!("__{kind}_{}", site.start));
    let span = subject.span;
    binds.push(bind_temp(&name, subject));
    Expr { span, ..var_ref(name) }
}

/// Binds each local that is more than a read (`reads_without_effect`) to
/// `__local_<site>_<key>`, in source order, and answers the locals with
/// the temps in their place.
fn bind_effectful_locals(
    entries: &[(Expr, Expr)],
    site: Span,
    ctx: &ViewCtx,
    binds: &mut Vec<Expr>,
) -> Vec<(Expr, Expr)> {
    entries
        .iter()
        .enumerate()
        .map(|(i, (k, v))| {
            if reads_without_effect(v, ctx) {
                return (k.clone(), v.clone());
            }
            let key = local_key_name(k)
                .filter(|n| n.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()))
                .unwrap_or_else(|| i.to_string());
            let name = Symbol::from(format!("__local_{}_{key}", site.start));
            binds.push(bind_temp(&name, v.clone()));
            (k.clone(), Expr { span: v.span, ..var_ref(name) })
        })
        .collect()
}

fn bind_temp(name: &Symbol, value: Expr) -> Expr {
    Expr::new(
        value.span,
        ExprNode::Assign { target: LValue::Var { id: VarId(0), name: name.clone() }, value },
    )
}

/// Whether the call a single render emits passes a local that is more
/// than a read after a local written after it, which then runs, or
/// reads, before it. The call orders its locals by the partial's
/// params (`call_position`), and `render @post, z: (n += 1), a: (n +=
/// 1)` would run `a` first where Ruby runs `z` first. A literal is the
/// same wherever it is evaluated.
fn evaluated_out_of_order(
    entries: &[(Expr, Expr)],
    position: impl Fn(&str) -> Option<usize>,
    ctx: &ViewCtx,
) -> bool {
    let at = |k: &Expr| match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => position(value.as_str()),
        _ => None,
    };
    entries.iter().enumerate().any(|(i, (k, v))| {
        !reads_without_effect(v, ctx)
            && at(k).is_some_and(|p| {
                entries[i + 1..]
                    .iter()
                    .any(|(k, v)| !matches!(&*v.node, ExprNode::Lit { .. }) && at(k).is_some_and(|q| q < p))
            })
    })
}

/// Where the call a single render emits passes each local: the local a
/// named render takes its record from (`record`) first, then the
/// partial's extras (`locals_extra_args`), or a strict-locals partial's
/// keywords in its header's order (`strict_kwargs`). `None` for a local
/// the call leaves out.
fn call_position<'a>(
    module: &str,
    stem: &str,
    record: Option<&'a str>,
    ctx: &'a ViewCtx,
) -> impl Fn(&str) -> Option<usize> + 'a {
    let key = (module.to_string(), stem.to_string());
    let params: Vec<String> = match ctx.strict_locals.get(&key) {
        Some(decl) => decl[1..].iter().map(|p| p.name.as_str().to_string()).collect(),
        None => ctx.partial_extras.get(&key).cloned().unwrap_or_default(),
    };
    move |name| {
        if record == Some(name) {
            Some(0)
        } else {
            params.iter().position(|p| p == name).map(|i| i + 1)
        }
    }
}

/// A literal, or a read of a local, an ivar or a constant: evaluating it
/// once per record is the same as once. Narrower than the walker's
/// `is_pure_read`, which takes every zero-arg bare send for a template
/// local: here one is a read only when the view knows the name as a
/// local, so `advance!` is not.
fn reads_without_effect(v: &Expr, ctx: &ViewCtx) -> bool {
    match &*v.node {
        ExprNode::Lit { .. } | ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Const { .. } => true,
        ExprNode::Send { recv: None, method, args, block: None, .. } => {
            args.is_empty() && ctx.is_local(method.as_str())
        }
        _ => false,
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
    context: Vec<(Expr, Expr)>,
) -> Option<Expr> {
    let entries: Vec<(Expr, Expr)> = kw_locals
        .iter()
        .filter_map(|p| lookup(p.name.as_str()).map(|v| (lit_sym(p.name.clone()), v)))
        .chain(context)
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
    // A strict-locals partial's record is its FIRST DECLARED local (which
    // is in `declared`), NOT the dir-convention singular. Its def-side
    // closure excludes exactly the declared set, so this side must too —
    // and must NOT additionally drop the dir-singular, or a strict partial
    // that reads a dir-conventional `@ivar` (present in the def closure)
    // gets one fewer arg here → arity mismatch. The dir-singular exclusion
    // applies only to convention-inferred (non-strict) partials.
    let strict = ctx.strict_locals.get(&key);
    let declared: crate::det::HashSet<&str> = strict
        .map(|ps| ps.iter().map(|p| p.name.as_str()).collect())
        .unwrap_or_default();
    let is_strict = strict.is_some();
    ctx.partial_ivars
        .get(&key)
        .map(|ivars| {
            ivars
                .iter()
                .filter(|n| {
                    (is_strict || n.as_str() != record_name)
                        && !declared.contains(n.as_str())
                })
                // Caller bodies are post-ivar-rewrite: an ivar lives
                // there as the CALLER's local for it (`@for` → `for_`,
                // see `ivar_local_names`). The partial binds it by
                // position, under its own name.
                .map(|n| var_ref(Symbol::from(ctx.ivar_local(n.as_str()))))
                .collect()
        })
        .unwrap_or_default()
}

/// `render partial: "…", collection: xs, as: :x` — iterate `xs`, calling
/// the named partial per element.
fn emit_named_collection_each(
    collection: &Expr,
    partial: &str,
    as_name: Option<&str>,
    locals: Option<&[(Expr, Expr)]>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    let (var_name, render_call) = named_collection_item_render(partial, as_name, locals, ctx)?;
    let inner = accumulator_append_call(render_call, ctx);
    let block_lambda = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { extra_params: Vec::new(),
            rest_param: None,
            params: vec![var_name],
            block_param: None,
            body: inner,
            block_style: BlockStyle::Brace,
        },
    );
    Some(send(
        Some(collection.clone()),
        "each",
        Vec::new(),
        Some(block_lambda),
        false,
    ))
}

/// The name for a collection render's block param: `name`, or `name`
/// with `_` appended until no other argument of the per-record call
/// reads it. Those arguments are the caller's locals, which the block
/// param would otherwise shadow: `<% p = 5 %><%= render @posts, depth:
/// p %>` must hand the partial the view's `p`, not the record. A
/// strict-locals partial's keywords are one kwargs Hash.
fn unshadowed(name: String, args: &[Expr]) -> Symbol {
    fn reads(a: &Expr, n: &str) -> bool {
        match &*a.node {
            ExprNode::Var { name, .. } => name.as_str() == n,
            ExprNode::Send { recv: None, method, args, block: None, .. } => {
                args.is_empty() && method.as_str() == n
            }
            ExprNode::Hash { entries, kwargs: true } => entries.iter().any(|(_, v)| reads(v, n)),
            _ => false,
        }
    }
    let mut name = name;
    while args.iter().any(|a| reads(a, &name)) {
        name.push('_');
    }
    Symbol::from(name)
}

/// The element local and the `Views::<Module>.<partial>(element, …)` call
/// one collection element renders through.
fn named_collection_item_render(
    partial: &str,
    as_name: Option<&str>,
    locals: Option<&[(Expr, Expr)]>,
    ctx: &ViewCtx,
) -> Option<(Symbol, Expr)> {
    let (module_dir, base_name) = match partial.rsplit_once('/') {
        Some((dir, name)) => (dir.to_string(), name.to_string()),
        None => (ctx.resource_dir.clone(), partial.to_string()),
    };
    if module_dir.is_empty() {
        return None;
    }
    let module_camel = camelize_path(&snake_case(&module_dir));
    let method_sym = ctx.partial_stems.stem(&module_camel, &base_name);
    let mut extra = partial_extra_args(ctx, &module_camel, &method_sym);
    extra.extend(locals_extra_args(locals, &module_camel, &method_sym, ctx));
    let var_name = unshadowed(
        crate::naming::safe_local(
            &as_name
                .map(|s| s.to_string())
                .unwrap_or_else(|| base_name.trim_start_matches('_').to_string()),
        ),
        &extra,
    );

    let mut call_args = vec![var_ref(var_name.clone())];
    call_args.extend(extra);
    let method = ctx.render_method(ViewKind::Partial, &module_camel, &method_sym);
    let render_call = send(
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
            },
        )),
        method.as_str(),
        call_args,
        None,
        true,
    );
    Some((var_name, render_call))
}

/// Rails' collection cache with a callable key (`cached: ->(item) { key }`),
/// as `ActionView::CollectionCaching` runs it:
///
/// ```ruby
/// c = <collection>
/// if ActionView::ViewHelpers.collection_caching?
///   c.preload_associations(c.to_a) if c.respond_to?(:preload_associations)
///   keys = []
///   c.each { |item| keys << ActionView::ViewHelpers.collection_fragment_key("x/_y", <key>) }
///   hits = ActionView::ViewHelpers.collection_fragment_read_multi(keys)
///   i = 0
///   c.each do |y|
///     if hits[i].nil? then io << collection_fragment_write(keys[i], <render y>) else io << hits[i] end
///     i += 1
///   end
///   ActionView::ViewHelpers.collection_rendered("app/views/x/_y.html.erb", hits)
/// else
///   c.preload_associations(c.to_a) if c.respond_to?(:preload_associations)
///   c.each { |y| io << <render y> }
/// end
/// ```
///
/// The collection is preloaded before the keys are built, because the
/// key may read associations (Rails' `collection.preload!`); every key
/// is built and read before any element renders (`read_multi`). With
/// caching off, a collection that can preload still does, as Rails'
/// `PreloadCollectionIterator` does at iteration: campfire's message
/// page is loaded with `skip_preloading!` and relies on it, and without
/// it each of 40 messages loaded five associations on its own.
///
/// `KeyedCache::Item` is `cached: true` (`wrap_cached_true_collection`):
/// the key is the element itself, which reads no association, so as in
/// Rails nothing is preloaded before the lookup and only the misses are
/// preloaded, just before they render; collections at or below
/// `MAX_UNCACHED_COLLECTION_LENGTH` take the uncached branch.
#[derive(Clone, Copy, PartialEq)]
enum KeyedCache {
    Callable,
    Item,
}

fn wrap_keyed_cached_collection(
    collection: &Expr,
    partial: &str,
    as_name: Option<&str>,
    locals: Option<&[(Expr, Expr)]>,
    key_lambda: &Expr,
    mode: KeyedCache,
    ctx: &ViewCtx,
) -> Option<Expr> {
    let ExprNode::Lambda { params, body, .. } = &*key_lambda.node else {
        return None;
    };
    let [key_param] = params.as_slice() else {
        return None;
    };
    let (var_name, render_call) = named_collection_item_render(partial, as_name, locals, ctx)?;
    let span = collection.span;
    let uniq = span.start;
    let name = |stem: &str| Symbol::from(format!("__ck_{stem}_{uniq}"));
    let (coll, keys, hits, idx, hit) =
        (name("c"), name("keys"), name("hits"), name("i"), name("hit"));
    let cap = format!("__ck_io_{uniq}");
    let assign = |target: &Symbol, value: Expr| {
        Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: target.clone() },
                value,
            },
        )
    };
    let block = |param: Symbol, body: Expr| {
        Expr::new(
            span,
            ExprNode::Lambda { extra_params: Vec::new(),
                rest_param: None,
                params: vec![param],
                block_param: None,
                body,
                block_style: BlockStyle::Brace,
            },
        )
    };
    let int_lit = |value: i64| Expr::new(span, ExprNode::Lit { value: Literal::Int { value } });
    let str_lit = |value: &str| {
        Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.to_string() } })
    };
    let at = |array: &Symbol| send(Some(var_ref(array.clone())), "[]", vec![var_ref(idx.clone())], None, false);
    let preload = || Expr::new(
        span,
        ExprNode::If {
            cond: send(
                Some(var_ref(coll.clone())),
                "respond_to?",
                vec![lit_sym(Symbol::from("preload_associations"))],
                None,
                false,
            ),
            // Not the collection itself: `preload_associations` takes its records, and a Spinel build reads an argument typed Array as one
            then_branch: send(
                Some(var_ref(coll.clone())),
                "preload_associations",
                vec![send(Some(var_ref(coll.clone())), "to_a", Vec::new(), None, false)],
                None,
                false,
            ),
            else_branch: nil_lit(),
        },
    );

    // Rails keys each element under the partial's TEMPLATE
    // (`messages/_message`, its digest path), the same name the
    // partial's own `<% cache message do %>` uses (`ctx.view_name`,
    // `walker::record_cache_key`). Keyed by the render's spelling
    // (`messages/message`), a miss rendered the partial and its block
    // filed a second copy under its own name that nothing read. The
    // variant the render resolves to names it, as the block's view
    // does (`fragment_template_path`): html and turbo_stream renders of
    // one collection would otherwise serve each other's markup.
    let (module, stem) =
        super::partial_name_to_key(partial, &ctx.resource_dir, &ctx.partial_stems);
    let format = ctx.render_format(ViewKind::Partial, &module, &stem);
    let template = crate::lower::view::fragment_template_path(
        &match partial.rsplit_once('/') {
            Some((dir, base)) => format!("{dir}/_{}", base.trim_start_matches('_')),
            None => format!("{}/_{}", ctx.resource_dir, partial.trim_start_matches('_')),
        },
        format,
    );
    let build_keys = send(
        Some(var_ref(coll.clone())),
        "each",
        Vec::new(),
        Some(block(
            key_param.clone(),
            seq(vec![
                send(
                    Some(var_ref(keys.clone())),
                    "<<",
                    vec![view_helpers_call(
                        "collection_fragment_key",
                        vec![str_lit(&template), body.clone()],
                    )],
                    None,
                    false,
                ),
                nil_lit(),
            ]),
        )),
        false,
    );
    let read_all = assign(
        &hits,
        view_helpers_call("collection_fragment_read_multi", vec![var_ref(keys.clone())]),
    );
    // Rails' `render_collection.action_view` names the partial's template
    // file: an ERB one, in the format the render resolves it to.
    let identifier = match partial.rsplit_once('/') {
        Some((dir, base)) => format!("app/views/{dir}/_{base}.{format}.erb"),
        None => format!("app/views/_{partial}.{format}.erb"),
    };
    let rendered = view_helpers_call(
        "collection_rendered",
        vec![str_lit(&identifier), var_ref(hits.clone())],
    );
    let miss_ctx = ViewCtx { accumulator: cap.clone(), ..ctx.clone() };
    let miss = seq(vec![
        assign_accumulator_string_new(&cap),
        accumulator_append_call(render_call.clone(), &miss_ctx),
        accumulator_append_call(
            view_helpers_call(
                "collection_fragment_write",
                vec![at(&keys), super::accumulator_result_ref(&cap)],
            ),
            ctx,
        ),
    ]);
    let render_each = send(
        Some(var_ref(coll.clone())),
        "each",
        Vec::new(),
        Some(block(
            var_name.clone(),
            seq(vec![
                assign(&hit, at(&hits)),
                Expr::new(
                    span,
                    ExprNode::If {
                        cond: send(Some(var_ref(hit.clone())), "nil?", Vec::new(), None, false),
                        then_branch: miss,
                        else_branch: accumulator_append_call(var_ref(hit.clone()), ctx),
                    },
                ),
                assign(&idx, send(Some(var_ref(idx.clone())), "+", vec![int_lit(1)], None, false)),
                nil_lit(),
            ]),
        )),
        false,
    );
    let empty_array = || Expr::new(span, ExprNode::Array { elements: Vec::new(), style: Default::default() });
    let cached = match mode {
        KeyedCache::Callable => seq(vec![
            preload(),
            assign(&keys, empty_array()),
            build_keys,
            read_all,
            assign(&idx, int_lit(0)),
            render_each,
            rendered,
        ]),
        KeyedCache::Item => {
            // Only the misses are preloaded, once every key has been read.
            let misses = name("misses");
            let elem = name("e");
            let collect_misses = send(
                Some(var_ref(coll.clone())),
                "each",
                Vec::new(),
                Some(block(
                    elem.clone(),
                    seq(vec![
                        Expr::new(
                            span,
                            ExprNode::If {
                                cond: send(Some(at(&hits)), "nil?", Vec::new(), None, false),
                                then_branch: send(Some(var_ref(misses.clone())), "<<", vec![var_ref(elem.clone())], None, false),
                                else_branch: nil_lit(),
                            },
                        ),
                        assign(&idx, send(Some(var_ref(idx.clone())), "+", vec![int_lit(1)], None, false)),
                        nil_lit(),
                    ]),
                )),
                false,
            );
            let preload_misses = Expr::new(
                span,
                ExprNode::If {
                    cond: send(
                        Some(var_ref(coll.clone())),
                        "respond_to?",
                        vec![lit_sym(Symbol::from("preload_associations"))],
                        None,
                        false,
                    ),
                    then_branch: Expr::new(
                        span,
                        ExprNode::If {
                            cond: send(Some(var_ref(misses.clone())), "empty?", Vec::new(), None, false),
                            then_branch: nil_lit(),
                            else_branch: send(
                                Some(var_ref(coll.clone())),
                                "preload_associations",
                                vec![var_ref(misses.clone())],
                                None,
                                false,
                            ),
                        },
                    ),
                    else_branch: nil_lit(),
                },
            );
            seq(vec![
                assign(&keys, empty_array()),
                build_keys,
                read_all,
                assign(&misses, empty_array()),
                assign(&idx, int_lit(0)),
                collect_misses,
                preload_misses,
                assign(&idx, int_lit(0)),
                render_each,
                rendered,
            ])
        }
    };
    let uncached = || -> Option<Expr> {
        Some(seq(vec![
            preload(),
            emit_named_collection_each(&var_ref(coll.clone()), partial, as_name, locals, ctx)?,
        ]))
    };
    let when_caching = match mode {
        KeyedCache::Callable => cached,
        // Small collections render without the store (the cost gate).
        KeyedCache::Item => Expr::new(
            span,
            ExprNode::If {
                cond: send(
                    Some(send(Some(var_ref(coll.clone())), "length", Vec::new(), None, false)),
                    ">",
                    vec![int_lit(MAX_UNCACHED_COLLECTION_LENGTH)],
                    None,
                    false,
                ),
                then_branch: cached,
                else_branch: uncached()?,
            },
        ),
    };
    Some(seq(vec![
        assign(&coll, collection.clone()),
        Expr::new(
            span,
            ExprNode::If {
                cond: view_helpers_call("collection_caching?", Vec::new()),
                then_branch: when_caching,
                else_branch: uncached()?,
            },
        ),
    ]))
}

/// `render collection:, cached: true`: Rails' collection cache keyed by
/// each element itself (its `cache_key_with_version`, under the app's
/// `combined_fragment_cache_key`), read in one `read_multi`, with only
/// the misses preloaded and rendered. `wrap_keyed_cached_collection`
/// does the work with the element as the key.
///
/// The element stays an OBJECT in the key, so an app's
/// `combined_fragment_cache_key` can ask what it is (campfire's shares
/// fragments of messages whose version the database maintains across
/// viewers and commits). Explicit `locals:` and the partial's threaded
/// closure ivars follow it as `name=value` segments, where Rails would
/// leave them out: two renders of the same records with different
/// context cannot share markup. A local whose name is not a literal
/// Symbol/String cannot be keyed safely — the collection renders
/// uncached.
fn wrap_cached_true_collection(
    collection: &Expr,
    partial: &str,
    as_name: Option<&str>,
    locals: Option<&[(Expr, Expr)]>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    let span = collection.span;
    let (module_dir, base_name) = match partial.rsplit_once('/') {
        Some((dir, name)) => (dir.to_string(), name.to_string()),
        None => (ctx.resource_dir.clone(), partial.to_string()),
    };
    if module_dir.is_empty() {
        return emit_named_collection_each(collection, partial, as_name, locals, ctx);
    }
    let module_camel = camelize_path(&snake_case(&module_dir));
    let method_sym = ctx.partial_stems.stem(&module_camel, &base_name);
    let mut named_inputs: Vec<(String, Expr)> = Vec::new();
    if let Some(entries) = locals {
        for (k, v) in entries {
            let Some(name) = local_key_name(k) else {
                return emit_named_collection_each(collection, partial, as_name, locals, ctx);
            };
            named_inputs.push((name, v.clone()));
        }
    }
    named_inputs.extend(partial_extra_named_args(ctx, &module_camel, &method_sym));

    let item = Symbol::from(format!("__ck_item_{}", span.start));
    let key = if named_inputs.is_empty() {
        var_ref(item.clone())
    } else {
        let mut elements = vec![var_ref(item.clone())];
        for (name, v) in named_inputs {
            elements.push(Expr::new(
                span,
                ExprNode::StringInterp {
                    parts: vec![
                        crate::expr::InterpPart::Text { value: format!("{name}=") },
                        crate::expr::InterpPart::Expr { expr: stable_cache_fragment(v) },
                    ],
                },
            ));
        }
        Expr::new(span, ExprNode::Array { elements, style: Default::default() })
    };
    let key_lambda = Expr::new(
        span,
        ExprNode::Lambda {
            extra_params: Vec::new(),
            rest_param: None,
            params: vec![item],
            block_param: None,
            body: key,
            block_style: BlockStyle::Brace,
        },
    );
    wrap_keyed_cached_collection(collection, partial, as_name, locals, &key_lambda, KeyedCache::Item, ctx)
}

/// Literal Symbol/String name of a `locals:` key, or None when dynamic.
fn local_key_name(key: &Expr) -> Option<String> {
    match &*key.node {
        ExprNode::Lit {
            value: Literal::Sym { value },
        } => Some(value.as_str().to_string()),
        ExprNode::Lit {
            value: Literal::Str { value },
        } => Some(value.clone()),
        _ => None,
    }
}

/// `v.respond_to?(:cache_key_with_version) ? v.cache_key_with_version : v.inspect`
/// — records get a stable versioned key; scalars get a quoted `inspect`
/// so slash-bearing values cannot fuse adjacent key segments.
fn stable_cache_fragment(v: Expr) -> Expr {
    let span = v.span;
    let cond = send(
        Some(v.clone()),
        "respond_to?",
        vec![lit_sym(Symbol::from("cache_key_with_version"))],
        None,
        false,
    );
    let then_branch = send(
        Some(v.clone()),
        "cache_key_with_version",
        Vec::new(),
        None,
        false,
    );
    let else_branch = send(Some(v), "inspect", Vec::new(), None, false);
    Expr::new(
        span,
        ExprNode::If {
            cond,
            then_branch,
            else_branch,
        },
    )
}

/// Threaded closure ivars the partial receives, with their names — the
/// request/view context Rails would otherwise leave out of a bare
/// collection key. Included so two renders of the same records with
/// different closure values cannot share markup.
fn partial_extra_named_args(
    ctx: &ViewCtx,
    module: &str,
    method: &str,
) -> Vec<(String, Expr)> {
    let record_name = singularize(&snake_case(last_segment(module)));
    let key = (module.to_string(), method.to_string());
    let strict = ctx.strict_locals.get(&key);
    let declared: crate::det::HashSet<&str> = strict
        .map(|ps| ps.iter().map(|p| p.name.as_str()).collect())
        .unwrap_or_default();
    let is_strict = strict.is_some();
    ctx.partial_ivars
        .get(&key)
        .map(|ivars| {
            ivars
                .iter()
                .filter(|n| {
                    (is_strict || n.as_str() != record_name)
                        && !declared.contains(n.as_str())
                })
                .map(|n| {
                    let safe = ctx.ivar_local(n.as_str());
                    (safe.clone(), var_ref(Symbol::from(safe)))
                })
                .collect()
        })
        .unwrap_or_default()
}


/// At or below this length, `cached: true` skips the collection store:
/// key build + `read_str` costs more than rendering a small/empty page
/// (Campfire sidebar after #488). Room/messages keep `PAGE_SIZE` 40
/// above the line. Prefer `length` over `size` so an unloaded Relation
/// loads once rather than `COUNT` then load. The emit is `length > N`.
const MAX_UNCACHED_COLLECTION_LENGTH: i64 = super::MAX_UNCACHED_COLLECTION_LENGTH;

/// Common shape for collection / association partial renders:
/// `<recv>.each { |x| <accumulator> << Views::<Plural>.<singular>(x) }`.
/// `plural_name` is the resource name in plural form
/// (`articles` / `comments`); the partial-fn name is its singular
/// (`article` / `comment`); the variable name is the singular's first
/// letter (`a` / `c`). `locals` binds the partial's extra params as an
/// explicit `locals:` does (`locals_extra_args`).
fn emit_partial_each(
    recv: &Expr,
    plural_name: &str,
    locals: Option<&[(Expr, Expr)]>,
    ctx: &ViewCtx,
) -> Expr {
    let singular = singularize(plural_name);
    let plural_camel = camelize_path(&snake_case(plural_name));
    let stem = ctx.partial_stems.stem(&plural_camel, &singular);
    let mut extra = partial_extra_args(ctx, &plural_camel, &stem);
    extra.extend(locals_extra_args(locals, &plural_camel, &stem, ctx));
    let var_name = unshadowed(singular.chars().next().unwrap_or('x').to_string(), &extra);

    let mut call_args = vec![var_ref(var_name.clone())];
    call_args.extend(extra);
    let method = ctx.render_method(ViewKind::Partial, &plural_camel, &stem);
    let render_call = send(
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from("Views"), Symbol::from(plural_camel.clone())],
            },
        )),
        method.as_str(),
        call_args,
        None,
        true,
    );
    let inner = accumulator_append_call(render_call, ctx);
    let block_lambda = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { extra_params: Vec::new(), rest_param: None,
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

/// Build the `Views::<Module>.<partial>(record, …)` call for a named
/// partial, WITHOUT appending it to the accumulator.
///
/// Split out of `emit_render_partial`'s `Named` arm so a caller that
/// needs the rendered fragment as a VALUE can reuse the same module /
/// method / extra-arg resolution. `turbo_stream.append target, record`
/// is one: the record's partial is the fragment's html, not a statement
/// appended to `io`.
pub(super) fn named_partial_call(
    partial: &str,
    arg: Option<&Expr>,
    locals: Option<&[(Expr, Expr)]>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    named_partial_call_with_record(partial, arg, locals, None, ctx)
}

/// `named_partial_call` with the record argument supplied directly
/// rather than resolved from `arg`/`locals`.
///
/// The one caller is the layout-block render, whose record is the
/// captured block output — a local this lowering just synthesized, which
/// no `locals:` hash names and no dir convention could find. `locals`
/// still governs the trailing extras.
pub(super) fn named_partial_call_with_record(
    partial: &str,
    arg: Option<&Expr>,
    locals: Option<&[(Expr, Expr)]>,
    record_override: Option<Expr>,
    ctx: &ViewCtx,
) -> Option<Expr> {

            let (module_dir, base_name) = match partial.rsplit_once('/') {
                Some((dir, name)) => (dir.to_string(), name.to_string()),
                None => (ctx.resource_dir.clone(), (*partial).to_string()),
            };
            if module_dir.is_empty() {
                return None;
            }
            let module_camel = camelize_path(&snake_case(&module_dir));
            let method_sym = ctx.partial_stems.stem(&module_camel, &base_name);

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
            let arg_expr = match (record_override, strict_decl, locals.is_some()) {
                (Some(record), _, _) => record,
                (None, strict_decl, has_locals) => match (strict_decl, has_locals) {
                (Some(decl), true) => lookup_local(decl[0].name.as_str())
                    .or_else(|| decl[0].default.clone())
                    .unwrap_or_else(nil_lit),
                (Some(decl), false) => arg
                    .cloned()
                    .or_else(|| decl[0].default.clone())
                    .unwrap_or_else(nil_lit),
                (None, true) => lookup_local(&record_name).unwrap_or_else(nil_lit),
                (None, false) => arg.cloned().unwrap_or_else(nil_lit),
                },
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
                if let Some(hash) = strict_kwargs(&decl[1..], &lookup_local, context_kwargs(ctx, &module_camel, &method_sym)) {
                    call_args.push(hash);
                }
            } else {
                call_args.extend(locals_extra_args(locals, &module_camel, &method_sym, ctx));
            }
            let method = ctx.render_method(ViewKind::Partial, &module_camel, &method_sym);
            let render_call = send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
                    },
                )),
                // `view_method_name`, matching the DEF side and the
                // `Template` arm above: a partial named `_new.html.erb`
                // defines `self._new`, because a module method called
                // `new` would collide with `Object.new`. Three of the four
                // call-site builders here had drifted from that rule —
                // latent until campfire's `rooms/layouts/_new`. The
                // extras/strict-locals keys stay on the RAW stem, which
                // is what the def side keys on too.
                method.as_str(),
                call_args,
                None,
                true,
            );
            Some(render_call)
}

/// `render layout: "rooms/layouts/new"[, locals: { … }] do … end`
///
/// Rails renders the block, then renders the layout partial with that
/// markup as its `yield`. The lowered shape is the same two steps, flat:
///
/// ```text
/// _layout_body = String.new
/// <block body, appending to _layout_body>
/// io << Views::Rooms::Layouts._new(_layout_body, room)
/// ```
///
/// The layout partial already takes its body as the dir-convention
/// record arg and reads it for `<%= yield %>` (see `partial::emit_yield`
/// and the arg-contract derivation in `mod.rs`) — this is the call half
/// that was missing, which is why the form previously survived into the
/// emit as a bare `render` inside a module that has no such method.
///
/// The capture local is `_layout_body`, deliberately NOT the `_cap` the
/// generic block-form helper arm uses: a `render layout:` block whose
/// own body contains a block-form helper would otherwise have the inner
/// capture clobber the outer one (Ruby block bodies share the enclosing
/// scope for names already bound). Nesting two LAYOUT renders would
/// still collide; no corpus view does, and the fix then is a counter,
/// not a rename.
pub(super) fn emit_layout_block(
    layout: &str,
    locals: Option<&[(Expr, Expr)]>,
    block: &Expr,
    ctx: &ViewCtx,
) -> Option<Vec<Expr>> {
    let ExprNode::Lambda { extra_params, params, body, .. } = &*block.node else {
        return None;
    };
    if !extra_params.is_empty() {
        return None;
    }
    let cap = "_layout_body";
    let cap_ctx = ViewCtx {
        accumulator: cap.to_string(),
        ..ctx.with_locals(params.iter().map(|p| p.as_str().to_string()))
    };
    let mut out = vec![super::assign_accumulator_string_new(cap)];
    out.extend(super::walker::walk_body(body, &cap_ctx));
    let call = named_partial_call_with_record(
        layout,
        None,
        locals,
        Some(var_ref(Symbol::from(cap))),
        ctx,
    )?;
    out.push(accumulator_append_call(call, ctx));
    Some(out)
}

/// The trailing extra-param arguments an explicit `locals:` hash binds,
/// and the caller's flash for the flash slots it leaves out, in the
/// partial's declared extra order.
///
/// A partial's non-record params are positional with nil defaults, so a
/// provided local has to land at its declared index; only the tail up to
/// the LAST provided one is emitted, keeping the short call when a
/// caller supplies nothing. Shared by the named-render and
/// collection-render call sites — Rails passes `locals:` to every
/// element render of a collection too, and having only one of the two
/// bind them is what left `rooms/opens/_user`'s `room` nil.
fn locals_extra_args(
    locals: Option<&[(Expr, Expr)]>,
    module_camel: &str,
    method_sym: &str,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let entries = locals.unwrap_or_default();
    let Some(extras) = ctx
        .partial_extras
        .get(&(module_camel.to_string(), method_sym.to_string()))
    else {
        return Vec::new();
    };
    // A strict-locals partial takes every local after its record as a
    // keyword, as `named_partial_call_with_record` binds them, and no
    // flash. At their extra positions the locals were an arity error.
    if let Some(decl) = ctx.strict_locals.get(&(module_camel.to_string(), method_sym.to_string())) {
        let lookup = |name: &str| {
            entries.iter().find_map(|(k, v)| match &*k.node {
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name => Some(v.clone()),
                _ => None,
            })
        };
        return strict_kwargs(&decl[1..], &lookup, context_kwargs(ctx, module_camel, method_sym))
            .into_iter()
            .collect();
    }
    // Rails renders a partial in its caller's view context, so a bare
    // `notice` in `shared/_flash` reads the same flash the layout does.
    // Here the flash is a param of every template, so the caller passes
    // its own (`ViewCtx::own_flash`): the layout's `notice`, or its
    // `notice_` when an `@notice` ivar took the plain name
    // (`flash_locals`). Without it every partial got nil. A render's
    // `notice:` local still wins, under whichever name the partial's
    // flash param goes by (`local_key_of`). A partial caller whose
    // `notice` may be a local (a render hands it `notice:`, or its
    // strict locals declare it) passes nil: Rails does not hand a
    // partial's locals on, and no other flash is in reach here. So does
    // a render under a block param or after an assignment that rebinds
    // `notice` (`ViewCtx::shadow_flash`).
    let flash = flash_params(extras);
    let bound: Vec<Option<Expr>> = extras
        .iter()
        .map(|name| {
            let key = local_key_of(extras, name);
            entries
                .iter()
                .find_map(|(k, v)| match &*k.node {
                    ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == key => {
                        Some(v.clone())
                    }
                    _ => None,
                })
                // The request context a run-time url Hash reads passes on from the rendering view (`request_context_views`).
                .or_else(|| {
                    (matches!(name.as_str(), "action_name" | "controller_name" | "path_parameters")
                        && ctx.locals.iter().any(|l| l == name))
                    .then(|| var_ref(Symbol::from(name.as_str())))
                })
                .or_else(|| {
                    let slot = flash.iter().position(|f| *f == Some(name.as_str()))?;
                    ctx.own_flash[slot].as_deref().map(|n| var_ref(Symbol::from(n)))
                })
        })
        .collect();
    match bound.iter().rposition(|b| b.is_some()) {
        Some(last) => bound.into_iter().take(last + 1).map(|b| b.unwrap_or_else(nil_lit)).collect(),
        None => Vec::new(),
    }
}

/// The request context a strict-locals partial takes as keywords
/// (`request_context_views`), bound from the rendering view's own.
fn context_kwargs(ctx: &ViewCtx, module: &str, method: &str) -> Vec<(Expr, Expr)> {
    let key = (module.to_string(), method.to_string());
    let Some(declared) = ctx.strict_locals.get(&key) else { return Vec::new() };
    let Some(extras) = ctx.partial_extras.get(&key) else { return Vec::new() };
    ["action_name", "controller_name", "path_parameters"]
        .into_iter()
        .filter(|n| {
            extras.iter().any(|e| e == n)
                && !declared.iter().any(|p| p.name.as_str() == *n)
                && ctx.locals.iter().any(|l| l == n)
        })
        .map(|n| (lit_sym(Symbol::from(n)), var_ref(Symbol::from(n))))
        .collect()
}
