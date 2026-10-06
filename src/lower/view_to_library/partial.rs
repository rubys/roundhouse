//! Render-partial dispatch + yield handling — both are output-position
//! dispatches from `emit_io_append`.

use crate::expr::{
    Arm, BlockStyle, Expr, ExprNode, InterpPart, Literal, LValue, Pattern,
};
use crate::ident::{Symbol, VarId};
use crate::naming::{camelize_path, last_segment, singularize, snake_case};
use crate::span::Span;

use crate::lower::view::RenderPartial;

use super::{
    accumulator_append_call, assign_accumulator_string_new, lit_sym, nil_lit, send, seq,
    var_ref, view_helpers_call, ViewCtx,
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
        // `render @message` (one record) — a single append, not an
        // `each`. Rails resolves the partial through the record's
        // `to_partial_path`: its own name in its PLURAL directory,
        // `messages/_message`. Spelled here exactly as
        // `render_partial_key` spells the graph edge, so the partial's
        // closure args line up with the call.
        RenderPartial::Record { name, .. } => {
            let singular = (*name).to_string();
            let plural_camel = camelize_path(&crate::naming::pluralize_snake(&singular));
            let mut call_args = vec![var_ref(Symbol::from(crate::naming::safe_local(&singular)))];
            call_args.extend(partial_extra_args(ctx, &plural_camel, &singular));
            let call = send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("Views"), Symbol::from(plural_camel)],
                    },
                )),
                &singular,
                call_args,
                None,
                true,
            );
            Some(accumulator_append_call(call, ctx))
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
            let call = named_partial_call(partial, *arg, *locals, ctx)?;
            Some(accumulator_append_call(call, ctx))
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
        RenderPartial::CollectionNamed {
            collection,
            partial,
            as_name,
            locals,
            cached,
        } => {
            if *cached {
                wrap_cached_collection(collection, partial, *as_name, *locals, ctx)
            } else {
                emit_named_collection_each(collection, partial, *as_name, *locals, ctx)
            }
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
                    crate::lower::view::view_method_name(&method_sym).as_str(),
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
    // A strict-locals partial's record is its FIRST DECLARED local (which
    // is in `declared`), NOT the dir-convention singular. Its def-side
    // closure excludes exactly the declared set, so this side must too —
    // and must NOT additionally drop the dir-singular, or a strict partial
    // that reads a dir-conventional `@ivar` (present in the def closure)
    // gets one fewer arg here → arity mismatch. The dir-singular exclusion
    // applies only to convention-inferred (non-strict) partials.
    let strict = ctx.strict_locals.get(&key);
    let declared: std::collections::HashSet<&str> = strict
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
                // Caller bodies are post-ivar-rewrite: a reserved-word
                // ivar (`@for`) lives there as its `safe_local` form.
                .map(|n| var_ref(Symbol::from(crate::naming::safe_local(n.as_str()))))
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
    let (module_dir, base_name) = match partial.rsplit_once('/') {
        Some((dir, name)) => (dir.to_string(), name.to_string()),
        None => (ctx.resource_dir.clone(), partial.to_string()),
    };
    if module_dir.is_empty() {
        return None;
    }
    let module_camel = camelize_path(&snake_case(&module_dir));
    let method_sym = base_name.trim_start_matches('_').to_string();
    let var_name = Symbol::from(crate::naming::safe_local(
        &as_name
            .map(|s| s.to_string())
            .unwrap_or_else(|| base_name.trim_start_matches('_').to_string()),
    ));

    let mut call_args = vec![var_ref(var_name.clone())];
    call_args.extend(partial_extra_args(ctx, &module_camel, &method_sym));
    call_args.extend(locals_extra_args(locals, &module_camel, &method_sym, ctx));
    let render_call = send(
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
            },
        )),
        crate::lower::view::view_method_name(&method_sym).as_str(),
        call_args,
        None,
        true,
    );
    let inner = accumulator_append_call(render_call, ctx);
    let block_lambda = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
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

/// Rails' collection cache: one `read_str` of the concatenated partials,
/// keyed by each element's `cache_key_with_version`. A miss still walks
/// the collection (inner `<% cache %>` fragments still apply).
///
/// Key shape (aligned with Rails' item key + template digest intent):
/// `views/coll/<cache_scope><view>/<partial>/<name>=<stable>/…/<record versions>`.
/// Explicit `locals:` and the partial's threaded closure ivars are named
/// segments; records use `cache_key_with_version`, scalars use `inspect`
/// (quoted, so `a/b`+`c` cannot collide with `a`+`b/c`). A local whose
/// name is not a literal Symbol/String cannot be keyed safely — fall
/// back to the uncached each path.
fn wrap_cached_collection(
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
    let method_sym = base_name.trim_start_matches('_').to_string();

    let mut named_inputs: Vec<(String, Expr)> = Vec::new();
    if let Some(entries) = locals {
        for (k, v) in entries {
            let Some(name) = local_key_name(k) else {
                // Name is dynamic — cannot build a stable key segment.
                return emit_named_collection_each(collection, partial, as_name, locals, ctx);
            };
            named_inputs.push((name, v.clone()));
        }
    }
    for (name, expr) in partial_extra_named_args(ctx, &module_camel, &method_sym) {
        named_inputs.push((name, expr));
    }

    let uniq = span.start;
    let key_name = Symbol::from(format!("__cc_key_{uniq}"));
    let hit_name = Symbol::from(format!("__cc_hit_{uniq}"));
    let rec_name = Symbol::from(format!("__cc_r_{uniq}"));
    let cap = format!("__cc_io_{uniq}");

    let prefix = Expr::new(
        span,
        ExprNode::StringInterp {
            parts: vec![
                InterpPart::Text {
                    value: "views/coll/".to_string(),
                },
                InterpPart::Expr {
                    expr: view_helpers_call("cache_scope", Vec::new()),
                },
                InterpPart::Text {
                    value: format!("{}/{}/", ctx.view_name, partial),
                },
            ],
        },
    );
    let assign_key = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: key_name.clone(),
            },
            value: prefix,
        },
    );
    let str_lit = |value: &str| {
        Expr::new(
            span,
            ExprNode::Lit {
                value: Literal::Str {
                    value: value.to_string(),
                },
            },
        )
    };
    let mut local_key_parts: Vec<Expr> = Vec::new();
    for (name, v) in &named_inputs {
        let append_name = send(
            Some(var_ref(key_name.clone())),
            "<<",
            vec![str_lit(&format!("/{name}="))],
            None,
            false,
        );
        local_key_parts.push(send(
            Some(append_name),
            "<<",
            vec![stable_cache_fragment(v.clone())],
            None,
            false,
        ));
    }
    let rec_ref = var_ref(rec_name.clone());
    let version = send(
        Some(rec_ref),
        "cache_key_with_version",
        Vec::new(),
        None,
        false,
    );
    let append_slash = send(
        Some(var_ref(key_name.clone())),
        "<<",
        vec![str_lit("/")],
        None,
        false,
    );
    let append_version = send(Some(append_slash), "<<", vec![version], None, false);
    let key_lambda = Expr::new(
        span,
        ExprNode::Lambda {
            rest_param: None,
            params: vec![rec_name],
            block_param: None,
            body: append_version,
            block_style: BlockStyle::Brace,
        },
    );
    let build_key = send(
        Some(collection.clone()),
        "each",
        Vec::new(),
        Some(key_lambda),
        false,
    );

    let store = || {
        send(
            Some(Expr::new(
                span,
                ExprNode::Const {
                    path: vec![Symbol::from("Rails")],
                },
            )),
            "cache",
            Vec::new(),
            None,
            false,
        )
    };
    let key_ref = || var_ref(key_name.clone());
    let hit_ref = || var_ref(hit_name.clone());
    let read = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: hit_name.clone(),
            },
            value: send(
                Some(store()),
                "read_str",
                vec![key_ref()],
                None,
                true,
            ),
        },
    );

    let miss_ctx = ViewCtx {
        accumulator: cap.clone(),
        ..ctx.clone()
    };
    let miss_each = emit_named_collection_each(collection, partial, as_name, locals, &miss_ctx)?;
    let miss = vec![
        assign_accumulator_string_new(&cap),
        miss_each,
        accumulator_append_call(
            send(
                Some(store()),
                "write_str",
                vec![
                    key_ref(),
                    super::accumulator_result_ref(&cap),
                    Expr::new(span, ExprNode::Lit {
                        value: Literal::Int { value: 0 },
                    }),
                ],
                None,
                true,
            ),
            ctx,
        ),
    ];
    let mut prelude = vec![assign_key];
    prelude.extend(local_key_parts);
    prelude.push(build_key);
    prelude.push(read);
    prelude.push(Expr::new(
        span,
        ExprNode::If {
            cond: send(Some(hit_ref()), "nil?", Vec::new(), None, false),
            then_branch: seq(miss),
            else_branch: seq(vec![accumulator_append_call(hit_ref(), ctx)]),
        },
    ));
    Some(seq(prelude))
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
    let declared: std::collections::HashSet<&str> = strict
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
                    let safe = crate::naming::safe_local(n.as_str());
                    (safe.clone(), var_ref(Symbol::from(safe)))
                })
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
        ExprNode::Lambda { rest_param: None,
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
                if let Some(hash) = strict_kwargs(&decl[1..], &lookup_local) {
                    call_args.push(hash);
                }
            } else {
                call_args.extend(locals_extra_args(locals, &module_camel, &method_sym, ctx));
            }
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
                crate::lower::view::view_method_name(&method_sym).as_str(),
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
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return None;
    };
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
/// in the partial's declared extra order.
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
    let Some(entries) = locals else { return Vec::new() };
    let Some(extras) = ctx
        .partial_extras
        .get(&(module_camel.to_string(), method_sym.to_string()))
    else {
        return Vec::new();
    };
    let bound: Vec<Option<Expr>> = extras
        .iter()
        .map(|name| {
            entries.iter().find_map(|(k, v)| match &*k.node {
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name => {
                    Some(v.clone())
                }
                _ => None,
            })
        })
        .collect();
    let Some(last) = bound.iter().rposition(|b| b.is_some()) else {
        return Vec::new();
    };
    bound
        .into_iter()
        .take(last + 1)
        .map(|b| b.unwrap_or_else(nil_lit))
        .collect()
}
