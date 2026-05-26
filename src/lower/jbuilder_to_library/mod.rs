//! Lower a `*.json.jbuilder` `View` (parsed-Ruby IR) into a
//! `LibraryClass` whose body is one `module_function`-style class method
//! per template, in the same string-accumulator shape ERB views use:
//!
//!   io = String.new
//!   io << "{"
//!   io << "\"id\":" << JsonBuilder.encode_value(article.id) << ","
//!   io << "\"title\":" << JsonBuilder.encode_value(article.title)
//!   io << "}"
//!   io
//!
//! Module/method naming:
//!   articles/_article.json.jbuilder → Views::Articles.article_json(article)
//!   articles/index.json.jbuilder    → Views::Articles.index_json(articles)
//!   articles/show.json.jbuilder     → Views::Articles.show_json(article)
//!
//! The `_json` suffix disambiguates from the ERB sibling
//! `Views::Articles.article(article)` — the same module hosts both
//! renderers.
//!
//! The four DSL primitives recognized today (real-blog coverage):
//!
//!   1. `json.extract! obj, :a, :b`     → emits one pair per attribute
//!   2. `json.<key> <expr>`             → emits one pair "<key>": <enc>
//!   3. `json.array! @col, partial: P, as: V`
//!                                       → emits a JSON array via P-per-item
//!   4. `json.partial! P, V: <expr>`    → inlines a single method call to P
//!
//! Lowerer scope is the real-blog `articles/*.json.jbuilder` set.
//! Stretch DSL forms (block-form `json.<key> do … end`, `json.merge!`,
//! `json.cache!` …) are deferred until a benchmark fixture forces them.

use crate::App;
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param, View};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, InterpPart, IrHint, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::singularize;
use crate::span::Span;

use super::view_to_library::{
    build_view_signature, infer_view_arg, insert_framework_stubs, split_view_name, view_module_id,
};

/// Bulk entry: lower every json-format view to a `LibraryClass`. Mirrors
/// `lower_views_to_library_classes` for ERB — same registry-merge +
/// body-typing structure; just routes a different DSL.
pub fn lower_jbuilder_to_library_classes(
    views: &[View],
    app: &App,
    extras: Vec<(ClassId, crate::analyze::ClassInfo)>,
) -> Vec<LibraryClass> {
    let mut lcs: Vec<LibraryClass> = views
        .iter()
        .filter(|v| v.format.as_str() == "json")
        .map(|v| build_library_class(v, app, /*type_body=*/ false))
        .collect();

    // Merge: caller extras + framework runtime stubs + the jbuilder LCs
    // themselves (so `Views::Articles.article_json` resolves when
    // referenced from `Views::Articles.index_json`).
    let mut classes: std::collections::HashMap<ClassId, crate::analyze::ClassInfo> =
        std::collections::HashMap::new();
    for (id, info) in extras {
        classes.insert(id, info);
    }
    insert_framework_stubs(&mut classes);
    for lc in &lcs {
        let info = classes.entry(lc.name.clone()).or_default();
        for m in &lc.methods {
            if let Some(sig) = &m.signature {
                if matches!(m.receiver, MethodReceiver::Class) {
                    info.class_methods.insert(m.name.clone(), sig.clone());
                    info.class_method_kinds.insert(m.name.clone(), m.kind);
                } else {
                    info.instance_methods.insert(m.name.clone(), sig.clone());
                    info.instance_method_kinds.insert(m.name.clone(), m.kind);
                }
            }
        }
        // Last-segment alias (e.g. `Articles` → `Views::Articles`) so
        // the typer's bare-Const-path resolver finds the method.
        let raw = lc.name.0.as_str();
        let last = raw.rsplit("::").next().unwrap_or(raw).to_string();
        if last != raw {
            let alias_id = ClassId(Symbol::from(last));
            let entry = classes.entry(alias_id).or_default();
            for m in &lc.methods {
                if let Some(sig) = &m.signature {
                    if matches!(m.receiver, MethodReceiver::Class) {
                        entry.class_methods.insert(m.name.clone(), sig.clone());
                        entry.class_method_kinds.insert(m.name.clone(), m.kind);
                    } else {
                        entry.instance_methods.insert(m.name.clone(), sig.clone());
                        entry.instance_method_kinds.insert(m.name.clone(), m.kind);
                    }
                }
            }
        }
    }

    let empty_ivars: std::collections::HashMap<Symbol, crate::ty::Ty> =
        std::collections::HashMap::new();
    for lc in &mut lcs {
        for method in &mut lc.methods {
            crate::lower::typing::type_method_body(method, &classes, &empty_ivars);
        }
    }
    lcs
}

/// Single-template entry. Used by tests and the dump_ir binary; the
/// production bulk path is `lower_jbuilder_to_library_classes`.
pub fn lower_jbuilder_to_library_class(view: &View, app: &App) -> LibraryClass {
    build_library_class(view, app, /*type_body=*/ true)
}

fn build_library_class(view: &View, app: &App, type_body: bool) -> LibraryClass {
    let (dir, base) = split_view_name(view.name.as_str());
    let stem = base.trim_start_matches('_');
    let is_partial = base.starts_with('_');

    let module_id = view_module_id(dir);
    let method_name = Symbol::from(format!("{stem}_json"));

    let known_models: Vec<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .collect();
    let arg_name = infer_view_arg(stem, dir, is_partial, &known_models);

    // Rewrite `@ivar` → bare `ivar` so the inferred arg / extras
    // read as plain locals. Mirrors the ERB lowerer.
    let rewritten = rewrite_ivars_to_locals(&view.body);

    let nil_default = nil_lit();
    let mut params: Vec<Param> = Vec::new();
    if !arg_name.is_empty() {
        params.push(Param::positional(Symbol::from(arg_name.clone())));
    }
    // jbuilder templates today don't reference flash locals; if a
    // future template surfaces `notice` / `alert` we'd plumb extras
    // here the same way view_to_library does.
    let extra_params: Vec<String> = Vec::new();
    for n in &extra_params {
        params.push(Param::with_default(
            Symbol::from(n.clone()),
            nil_default.clone(),
        ));
    }

    let signature = build_view_signature(
        stem,
        dir,
        is_partial,
        &arg_name,
        &extra_params,
        &known_models,
    );

    let arg_columns = if arg_name.is_empty() {
        std::collections::HashMap::new()
    } else {
        columns_for_arg(&arg_name, dir, is_partial, stem, app)
    };

    let ctx = Ctx {
        resource_dir: dir.to_string(),
        accumulator: "io".to_string(),
        arg_name: arg_name.clone(),
        arg_columns,
    };

    let mut body_stmts: Vec<Expr> = Vec::new();
    body_stmts.push(assign_accumulator_string_new(&ctx.accumulator));
    body_stmts.extend(walk_template(&rewritten, &ctx));
    let mut result = var_ref(Symbol::from(ctx.accumulator.as_str()));
    result.hint = Some(IrHint::StringBuilderResult);
    body_stmts.push(result);

    let body = seq(body_stmts);

    let mut method = MethodDef {
        name: method_name,
        receiver: MethodReceiver::Class,
        params,
        body,
        signature,
        effects: EffectSet::default(),
        enclosing_class: Some(module_id.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
    };

    if type_body {
        type_method_body_solo(&mut method);
    }

    LibraryClass {
        name: module_id,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods: vec![method],
        origin: None,
    }
}

fn type_method_body_solo(method: &mut MethodDef) {
    let mut classes: std::collections::HashMap<ClassId, crate::analyze::ClassInfo> =
        std::collections::HashMap::new();
    insert_framework_stubs(&mut classes);
    let typer = crate::analyze::BodyTyper::new(&classes);
    let mut ctx = crate::analyze::Ctx::default();
    if let Some(crate::ty::Ty::Fn { params, .. }) = &method.signature {
        for (param, sig) in method.params.iter().zip(params.iter()) {
            ctx.local_bindings.insert(param.name.clone(), sig.ty.clone());
        }
    }
    if let Some(enclosing) = &method.enclosing_class {
        ctx.self_ty = Some(crate::ty::Ty::Class {
            id: ClassId(enclosing.clone()),
            args: vec![],
        });
    }
    typer.analyze_expr(&mut method.body, &ctx);
}

// ── walker ───────────────────────────────────────────────────────────

struct Ctx {
    /// Source directory of the template — `articles` for
    /// `articles/_article.json.jbuilder`. Used by partial resolution
    /// when `json.partial! "post"` (no slash) needs the current dir.
    resource_dir: String,
    /// Name of the accumulator local (`io`). Synthesized at body head;
    /// every appended fragment goes through `accumulator_append`.
    accumulator: String,
    /// Name of the template's main positional arg (`article` for the
    /// `_article` partial; `articles` for `index`). Used to recognize
    /// `json.extract! article, :a, :b` calls so the lowerer can look
    /// up column types on the implied model.
    arg_name: String,
    /// Column-type table for the model that backs `arg_name`. Keyed
    /// by column-name symbol; empty when the arg has no resolvable
    /// model (e.g. layouts, untyped fixtures). Used to route
    /// datetime columns through `JsonBuilder.encode_datetime` rather
    /// than the generic `encode_value`.
    arg_columns: std::collections::HashMap<Symbol, crate::schema::ColumnType>,
}

/// Classification of a single top-level statement in a jbuilder
/// template. The walker normalizes recognized DSL Sends into this
/// closed enum; unknown shapes are tagged `Unknown` and emit a TODO
/// marker so the file still parses.
enum JbStmt<'a> {
    /// `json.extract! obj, :a, :b` — N pairs, one per attribute.
    Extract { obj: &'a Expr, attrs: Vec<Symbol> },
    /// `json.<key>(<expr>)` — one pair `"<key>": <enc>`. `parens` is
    /// false for the `json.url article_url(...)` style (no paren on
    /// the json.X call, single positional arg).
    Pair { key: Symbol, value: &'a Expr },
    /// `json.array! @col, partial: P, as: V` — full-template array.
    ArrayPartial {
        collection: &'a Expr,
        partial_path: String,
        item_var: Symbol,
    },
    /// `json.partial! P, V: <expr>` — full-template partial call.
    Partial {
        partial_path: String,
        arg: &'a Expr,
    },
    /// Unrecognized DSL or non-Send statement. Surfaces as an empty io
    /// append so the lowered body stays well-formed.
    Unknown,
}

fn walk_template(body: &Expr, ctx: &Ctx) -> Vec<Expr> {
    let raw_stmts: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };

    let classified: Vec<JbStmt<'_>> = raw_stmts.iter().map(|s| classify(s)).collect();

    // Whole-template DSL forms (single stmt covers the entire JSON
    // body) — array! and partial! produce a top-level array or method
    // call respectively, no `{}` wrap.
    if classified.len() == 1 {
        match &classified[0] {
            JbStmt::ArrayPartial { collection, partial_path, item_var } => {
                return emit_array_partial(collection, partial_path, item_var, ctx);
            }
            JbStmt::Partial { partial_path, arg } => {
                return emit_partial_call(partial_path, arg, ctx);
            }
            _ => {}
        }
    }

    // Object form — wrap accumulated pair-emitting statements in
    // `{` … `}`. Comma is inserted between pairs; the lowerer knows
    // statically how many pairs to emit, so no runtime "first" flag.
    let mut out: Vec<Expr> = Vec::new();
    out.push(io_append_lit(&ctx.accumulator, "{"));
    let mut emitted = 0usize;
    for stmt in &classified {
        match stmt {
            JbStmt::Extract { obj, attrs } => {
                let obj_is_arg = obj_is_named_local(obj, &ctx.arg_name);
                for attr in attrs {
                    if emitted > 0 {
                        out.push(io_append_lit(&ctx.accumulator, ","));
                    }
                    out.push(io_append_lit(
                        &ctx.accumulator,
                        &format!("\"{}\":", attr.as_str()),
                    ));
                    let value = send(
                        Some((*obj).clone()),
                        attr.as_str(),
                        Vec::new(),
                        None,
                        false,
                    );
                    // Route datetime / date columns through
                    // `JsonBuilder.encode_datetime` for Rails-canonical
                    // ISO 8601 output. Other columns (Integer, String,
                    // …) ride encode_value's type dispatch.
                    let use_datetime = obj_is_arg
                        && matches!(
                            ctx.arg_columns.get(attr),
                            Some(crate::schema::ColumnType::DateTime)
                                | Some(crate::schema::ColumnType::Date)
                                | Some(crate::schema::ColumnType::Time)
                        );
                    let encoded = if use_datetime {
                        json_builder_call("encode_datetime", value)
                    } else {
                        json_builder_encode(value)
                    };
                    out.push(io_append_call(&ctx.accumulator, encoded));
                    emitted += 1;
                }
            }
            JbStmt::Pair { key, value } => {
                if emitted > 0 {
                    out.push(io_append_lit(&ctx.accumulator, ","));
                }
                out.push(io_append_lit(
                    &ctx.accumulator,
                    &format!("\"{}\":", key.as_str()),
                ));
                let rewritten_value = rewrite_route_helpers(value);
                out.push(io_append_call(
                    &ctx.accumulator,
                    json_builder_encode(rewritten_value),
                ));
                emitted += 1;
            }
            JbStmt::ArrayPartial { .. } | JbStmt::Partial { .. } => {
                // These shouldn't appear in an object template, but if
                // they do (mixed with pair-emitting stmts), drop a
                // TODO marker rather than emit malformed JSON.
                out.push(io_append_lit(&ctx.accumulator, ""));
            }
            JbStmt::Unknown => {
                out.push(io_append_lit(&ctx.accumulator, ""));
            }
        }
    }
    out.push(io_append_lit(&ctx.accumulator, "}"));
    out
}

fn classify<'a>(stmt: &'a Expr) -> JbStmt<'a> {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        ..
    } = &*stmt.node
    else {
        return JbStmt::Unknown;
    };
    if !is_json_receiver(recv) {
        return JbStmt::Unknown;
    }
    match method.as_str() {
        "extract!" => {
            // First arg = object, rest = attribute symbols.
            let Some((obj, rest)) = args.split_first() else {
                return JbStmt::Unknown;
            };
            let mut attrs: Vec<Symbol> = Vec::new();
            for a in rest {
                if let ExprNode::Lit {
                    value: Literal::Sym { value },
                } = &*a.node
                {
                    attrs.push(value.clone());
                } else {
                    return JbStmt::Unknown;
                }
            }
            JbStmt::Extract { obj, attrs }
        }
        "array!" => {
            // `json.array! @col, partial: P, as: V`. First positional
            // is the collection; trailing Hash carries partial/as.
            let Some(collection) = args.first() else {
                return JbStmt::Unknown;
            };
            let Some(opts) = args.iter().skip(1).find_map(extract_hash) else {
                return JbStmt::Unknown;
            };
            let partial_path = match hash_get_string(&opts, "partial") {
                Some(s) => s,
                None => return JbStmt::Unknown,
            };
            let item_var = match hash_get_symbol(&opts, "as") {
                Some(s) => s,
                None => return JbStmt::Unknown,
            };
            JbStmt::ArrayPartial {
                collection,
                partial_path,
                item_var,
            }
        }
        "partial!" => {
            // `json.partial! P, V: <expr>`. First positional is the
            // partial path; trailing Hash has one entry whose value
            // is the arg to pass.
            let Some(path_arg) = args.first() else {
                return JbStmt::Unknown;
            };
            let partial_path = match string_literal(path_arg) {
                Some(s) => s,
                None => return JbStmt::Unknown,
            };
            let Some(opts) = args.iter().skip(1).find_map(extract_hash) else {
                return JbStmt::Unknown;
            };
            // First (and conventionally only) Hash entry's value is
            // the arg. The key names the local the partial expects,
            // but the lowerer dispatches by position — drop the key.
            let Some((_k, arg)) = hash_first_entry(&opts) else {
                return JbStmt::Unknown;
            };
            JbStmt::Partial {
                partial_path,
                arg,
            }
        }
        // `json.<key> <expr>` — single-pair shape. The method name IS
        // the JSON key; the single positional arg is the value.
        key if args.len() == 1 => JbStmt::Pair {
            key: Symbol::from(key),
            value: &args[0],
        },
        _ => JbStmt::Unknown,
    }
}

/// `json` parsed as a bare method call: `Send { recv: None, method:
/// "json", args: [] }`. Anything else fails the discriminator.
fn is_json_receiver(recv: &Expr) -> bool {
    matches!(
        &*recv.node,
        ExprNode::Send {
            recv: None,
            method,
            args,
            ..
        } if method.as_str() == "json" && args.is_empty()
    )
}

fn extract_hash<'a>(e: &'a Expr) -> Option<&'a [(Expr, Expr)]> {
    if let ExprNode::Hash { entries, .. } = &*e.node {
        Some(entries.as_slice())
    } else {
        None
    }
}

fn hash_get_string(entries: &[(Expr, Expr)], key: &str) -> Option<String> {
    for (k, v) in entries {
        if let ExprNode::Lit {
            value: Literal::Sym { value },
        } = &*k.node
        {
            if value.as_str() == key {
                return string_literal(v);
            }
        }
    }
    None
}

fn hash_get_symbol(entries: &[(Expr, Expr)], key: &str) -> Option<Symbol> {
    for (k, v) in entries {
        if let ExprNode::Lit {
            value: Literal::Sym { value },
        } = &*k.node
        {
            if value.as_str() == key {
                if let ExprNode::Lit {
                    value: Literal::Sym { value: vsym },
                } = &*v.node
                {
                    return Some(vsym.clone());
                }
            }
        }
    }
    None
}

fn hash_first_entry<'a>(entries: &'a [(Expr, Expr)]) -> Option<(Symbol, &'a Expr)> {
    let (k, v) = entries.first()?;
    let ExprNode::Lit {
        value: Literal::Sym { value: ksym },
    } = &*k.node
    else {
        return None;
    };
    Some((ksym.clone(), v))
}

fn string_literal(e: &Expr) -> Option<String> {
    if let ExprNode::Lit {
        value: Literal::Str { value },
    } = &*e.node
    {
        Some(value.clone())
    } else {
        None
    }
}

// ── emitters ────────────────────────────────────────────────────────

fn emit_array_partial(
    collection: &Expr,
    partial_path: &str,
    item_var: &Symbol,
    ctx: &Ctx,
) -> Vec<Expr> {
    // io << "["
    // io << collection.map { |item_var| Views::<Module>.<p>_json(item_var) }.join(",")
    // io << "]"
    //
    // map+join (rather than each + mutable first-flag or each_with_index)
    // emits idiomatically in both Ruby and TypeScript. The earlier
    // mutable-flag pattern triggered TS's "used before declaration"
    // when an inner `first = false` re-declared the outer binding;
    // each_with_index lacks a direct TS mapping. map+join avoids both.
    let mut out: Vec<Expr> = Vec::new();
    out.push(io_append_lit(&ctx.accumulator, "["));

    let (mod_path, method) = partial_target(partial_path, &ctx.resource_dir);
    let partial_call = send(
        Some(const_path(&mod_path)),
        &format!("{method}_json"),
        vec![var_ref(item_var.clone())],
        None,
        true,
    );

    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![item_var.clone()],
            block_param: None,
            body: partial_call,
            block_style: crate::expr::BlockStyle::Brace,
        },
    );

    let mapped = send(
        Some(collection.clone()),
        "map",
        Vec::new(),
        Some(block),
        false,
    );
    let joined = send(Some(mapped), "join", vec![lit_str(",".to_string())], None, true);
    out.push(io_append_call(&ctx.accumulator, joined));
    out.push(io_append_lit(&ctx.accumulator, "]"));
    out
}

fn emit_partial_call(partial_path: &str, arg: &Expr, ctx: &Ctx) -> Vec<Expr> {
    let (mod_path, method) = partial_target(partial_path, &ctx.resource_dir);
    let call = send(
        Some(const_path(&mod_path)),
        &format!("{method}_json"),
        vec![arg.clone()],
        None,
        true,
    );
    vec![io_append_call(&ctx.accumulator, call)]
}

/// Resolve a Jbuilder partial path to (module-path, base-method-name).
/// `"articles/article"` → (["Views","Articles"], "article")
/// `"article"` (no slash) → (["Views","<current-dir-camel>"], "article")
fn partial_target(path: &str, resource_dir: &str) -> (Vec<Symbol>, String) {
    let (dir, base) = match path.rsplit_once('/') {
        Some((d, b)) => (d.to_string(), b.to_string()),
        None => (resource_dir.to_string(), path.to_string()),
    };
    let base = base.trim_start_matches('_').to_string();
    let module_camel = crate::naming::camelize(&crate::naming::snake_case(&dir));
    let module_path = vec![Symbol::from("Views"), Symbol::from(module_camel)];
    (module_path, base)
}

// ── route-helper rewrite for pair values ────────────────────────────

/// Rewrite bare `<x>_url(record, format: :fmt, ...)` calls into
/// `RouteHelpers.<x>_path(record.id)`. The runtime's
/// `app/route_helpers.rb` (generated by `routes_to_library`) exposes
/// `_path` helpers only — host-aware `_url` helpers aren't emitted —
/// so this rewrite is the bridge between Rails' `article_url(article,
/// format: :json)` convention and the runtime's path-only surface.
///
/// A `format: :<sym>` kwarg appends `.<sym>` to the result so JSON
/// templates emit the same `/articles/1.json` self-link shape Rails
/// produces — without that the comparator flags every `json.url`
/// pair as a value mismatch. Other kwargs still drop on the floor;
/// scheme+host (the rest of the `_url` vs `_path` difference) is
/// per-deployment noise the comparator canonicalizes away.
fn rewrite_route_helpers(e: &Expr) -> Expr {
    let new_node = match &*e.node {
        ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            parenthesized,
        } if method.as_str().ends_with("_url") => {
            let path_name = format!("{}_path", &method.as_str()[..method.as_str().len() - 4]);
            let format_sym: Option<Symbol> = args.iter().find_map(|a| {
                if let ExprNode::Hash { kwargs: true, entries } = &*a.node {
                    hash_get_symbol(entries, "format")
                } else {
                    None
                }
            });
            let path_args: Vec<Expr> = args
                .iter()
                .filter(|a| !matches!(&*a.node, ExprNode::Hash { kwargs: true, .. }))
                .map(rewrite_path_arg_local)
                .collect();
            let path_call = send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("RouteHelpers")],
                    },
                )),
                &path_name,
                path_args,
                block.clone(),
                *parenthesized,
            );
            return match format_sym {
                Some(fmt) => send(
                    Some(path_call),
                    "+",
                    vec![lit_str(format!(".{}", fmt.as_str()))],
                    None,
                    false,
                ),
                None => path_call,
            };
        }
        ExprNode::Send {
            recv,
            method,
            args,
            block,
            parenthesized,
        } => ExprNode::Send {
            recv: recv.as_ref().map(rewrite_route_helpers),
            method: method.clone(),
            args: args.iter().map(rewrite_route_helpers).collect(),
            block: block.as_ref().map(rewrite_route_helpers),
            parenthesized: *parenthesized,
        },
        other => other.clone(),
    };
    Expr::new(e.span, new_node)
}

/// Route-helper positional args want `record.id` instead of bare
/// `record` — Rails accepts either, but `RouteHelpers.article_path`
/// takes an Integer. Bare Var/Send-no-args of a presumed local rewrite
/// to `<local>.id`; anything else passes through unchanged.
fn rewrite_path_arg_local(arg: &Expr) -> Expr {
    let name = match &*arg.node {
        ExprNode::Var { name, .. } => Some(name.clone()),
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() => Some(method.clone()),
        _ => None,
    };
    match name {
        Some(n) => send(Some(var_ref(n)), "id", Vec::new(), None, false),
        None => arg.clone(),
    }
}

// ── IR constructors (private; mirror view_to_library's helpers) ─────

fn assign_accumulator_string_new(name: &str) -> Expr {
    let string_const = Expr::new(
        Span::synthetic(),
        ExprNode::Const {
            path: vec![Symbol::from("String")],
        },
    );
    let new_call = send(Some(string_const), "new", Vec::new(), None, false);
    let mut e = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: Symbol::from(name),
            },
            value: new_call,
        },
    );
    e.hint = Some(IrHint::StringBuilderInit);
    e
}

fn io_append_lit(accumulator: &str, s: &str) -> Expr {
    let recv = var_ref(Symbol::from(accumulator));
    let mut e = send(Some(recv), "<<", vec![lit_str(s.to_string())], None, false);
    e.hint = Some(IrHint::StringBuilderAppend);
    e
}

fn io_append_call(accumulator: &str, call: Expr) -> Expr {
    let recv = var_ref(Symbol::from(accumulator));
    let mut e = send(Some(recv), "<<", vec![call], None, false);
    e.hint = Some(IrHint::StringBuilderAppend);
    e
}

fn json_builder_encode(value: Expr) -> Expr {
    json_builder_call("encode_value", value)
}

fn json_builder_call(method: &str, value: Expr) -> Expr {
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const {
            path: vec![Symbol::from("JsonBuilder")],
        },
    );
    send(Some(recv), method, vec![value], None, true)
}

/// True when `obj` reads as the named local — either a bare `Var`
/// or a `Send` with no receiver, no args, no block (the bareword
/// shape Prism produces for partial-scope locals).
fn obj_is_named_local(obj: &Expr, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    match &*obj.node {
        ExprNode::Var { name: n, .. } => n.as_str() == name,
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } => args.is_empty() && method.as_str() == name,
        _ => false,
    }
}

/// Build the `{column_name → ColumnType}` map for the template's
/// main positional arg, when that arg resolves to a known model
/// backed by a schema table. Returns an empty map for layouts,
/// index views (arg is a collection, not a single record), and any
/// arg we can't tie back to a schema row.
fn columns_for_arg(
    arg_name: &str,
    dir: &str,
    is_partial: bool,
    stem: &str,
    app: &App,
) -> std::collections::HashMap<Symbol, crate::schema::ColumnType> {
    let mut out: std::collections::HashMap<Symbol, crate::schema::ColumnType> =
        std::collections::HashMap::new();
    // Index views' arg is the plural collection (`articles`), not a
    // single record. Per-row column lookups don't apply — the
    // extract! inside the partial handles those instead.
    if !is_partial && stem == "index" {
        return out;
    }
    if dir == "layouts" {
        return out;
    }
    let model_class = crate::naming::singularize_camelize(dir);
    let Some(model) = app.models.iter().find(|m| m.name.0.as_str() == model_class) else {
        return out;
    };
    let Some(table) = app.schema.tables.get(&model.table.0) else {
        return out;
    };
    for col in &table.columns {
        out.insert(col.name.clone(), col.col_type.clone());
    }
    let _ = arg_name; // arg_name is informational; the dir → model
                     // resolution above is the load-bearing path.
    out
}

fn assign_var(name: &str, value: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: Symbol::from(name),
            },
            value,
        },
    )
}

fn const_path(path: &[Symbol]) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Const {
            path: path.to_vec(),
        },
    )
}

fn send(
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

fn lit_str(s: String) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit {
            value: Literal::Str { value: s },
        },
    )
}

fn lit_int(n: i64) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit {
            value: Literal::Int { value: n },
        },
    )
}

fn nil_lit() -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Nil },
    )
}

fn var_ref(name: Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: VarId(0),
            name,
        },
    )
}

fn seq(exprs: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
}

// ── ivar → local rewrite (shared shape with view_to_library) ────────

fn rewrite_ivars_to_locals(expr: &Expr) -> Expr {
    let new_node = match &*expr.node {
        ExprNode::Ivar { name } => ExprNode::Var {
            id: VarId(0),
            name: name.clone(),
        },
        ExprNode::Assign {
            target: LValue::Ivar { name },
            value,
        } => ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: name.clone(),
            },
            value: rewrite_ivars_to_locals(value),
        },
        ExprNode::Assign { target, value } => ExprNode::Assign {
            target: rewrite_lvalue(target),
            value: rewrite_ivars_to_locals(value),
        },
        ExprNode::Send {
            recv,
            method,
            args,
            block,
            parenthesized,
        } => ExprNode::Send {
            recv: recv.as_ref().map(rewrite_ivars_to_locals),
            method: method.clone(),
            args: args.iter().map(rewrite_ivars_to_locals).collect(),
            block: block.as_ref().map(rewrite_ivars_to_locals),
            parenthesized: *parenthesized,
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(rewrite_ivars_to_locals).collect(),
        },
        ExprNode::If {
            cond,
            then_branch,
            else_branch,
        } => ExprNode::If {
            cond: rewrite_ivars_to_locals(cond),
            then_branch: rewrite_ivars_to_locals(then_branch),
            else_branch: rewrite_ivars_to_locals(else_branch),
        },
        ExprNode::BoolOp {
            op,
            surface,
            left,
            right,
        } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_ivars_to_locals(left),
            right: rewrite_ivars_to_locals(right),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(rewrite_ivars_to_locals).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, kwargs } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (rewrite_ivars_to_locals(k), rewrite_ivars_to_locals(v)))
                .collect(),
            kwargs: *kwargs,
        },
        ExprNode::Lambda {
            params,
            block_param,
            body,
            block_style,
        } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_ivars_to_locals(body),
            block_style: *block_style,
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text {
                        value: value.clone(),
                    },
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
        LValue::Var { id, name } => LValue::Var {
            id: *id,
            name: name.clone(),
        },
        LValue::Ivar { name } => LValue::Var {
            id: VarId(0),
            name: name.clone(),
        },
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

// Silence unused-import warnings until we wire up a future stretch
// primitive that needs `singularize`.
#[allow(dead_code)]
fn _unused() {
    let _ = singularize;
}
