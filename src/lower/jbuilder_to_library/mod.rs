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
//!   4. `json.partial! P, V: <expr>`    → inlines a single method call to P,
//!                                       passing each local by the name
//!                                       P takes it under (`PartialParams`)
//!   5. `json.partial! partial: P, collection: C, as: V`
//!                                       → Jbuilder's own alias for (3)
//!   6. `json.(obj, :a, :b)`            → Ruby's `.()` spelling of (1)
//!   7. `json.<key> do … end`           → one pair whose value is a
//!                                       nested object
//!   8. `json.<key> obj, partial: P, as: V`
//!                                       → one pair whose value is a
//!                                       partial render of ONE object
//!   9. `json.cache! key do … end`      → transparent: the block's
//!                                       statements ARE the object's
//!  10. `if c … else … end` and the `if`/`unless` modifiers around any
//!      of the above                    → the same `if`, each branch's
//!                                       pairs appended inside it
//!  11. `begin … rescue … end`          → the same `begin`; a `rescue`
//!                                       first drops a pair the body
//!                                       left half-written
//!  12. `x = <expr>`                    → kept as written, in place; a
//!                                       local the template reads
//!                                       later
//!  13. `json.<key> col do |x| … end`   → one pair whose value is an
//!                                       array, one object per element
//!                                       built by the block
//!  14. `json.array! col do |x| … end`  → the same array, as the whole
//!                                       template
//!  15. `json.partial! @record`         → (4) with the path Rails takes
//!                                       from the record's model
//!                                       (`partial:` and `as:` after
//!                                       the record do not change it)
//!
//! (6)-(9) arrived together with campfire's bot API, which is six
//! jbuilder templates written in exactly that dialect.
//!
//! `cache!` is the one with a SEMANTIC gap behind it, so say it plainly:
//! Jbuilder's is a fragment cache keyed on the record, and the emit has
//! no fragment cache to put the rendered JSON in. Rendering the block
//! every time is correct output at the wrong cost, which is the same
//! trade `<% cache %>` already takes on the ERB side. Reading it as an
//! ordinary `json.<key>` pair — which is what the classifier did before
//! this — was neither: it emitted a literal `"cache!"` key holding the
//! whole record.
//!
//! Still deferred: `json.merge!`, `json.key_format!`, `json.ignore_nil!`,
//! and `json.child!`.

use crate::App;
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param, View};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, InterpPart, IrHint, LValue, Literal, RescueClause};
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
    let partials = std::rc::Rc::new(partial_params(app));
    let mut lcs: Vec<LibraryClass> = views
        .iter()
        .filter(|v| v.jbuilder && !v.analysis_only)
        .map(|v| build_library_class(v, app, &partials, /*type_body=*/ false))
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
    // The app's OWN route helpers, including the per-format variants
    // (`article_json_path`). `insert_framework_stubs` carries a
    // hardcoded stem list that cannot know them, and an unknown helper
    // types as `Ty::Var` — which the rust emitter reads as "already a
    // Value" and passes straight into `JsonBuilder::encode_value`,
    // where it is a String. `json.url article_url(a, format: :json)` is
    // exactly that call.
    crate::lower::view_to_library::insert_route_helper_stubs(&mut classes, app);
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
            // A record standing where a route helper wants an id.
            // `rewrite_path_arg_local` above handles the bare local
            // (`message` in `room_message_url(message)`) by SHAPE, which
            // is all the shape there is before typing; an attribute
            // chain needs the type. campfire's boost partial writes
            // `room_message_url(boost.message.room, boost.message)` and
            // both arguments reached the helper whole, so every boost's
            // `url` read `/rooms/#<Room:0x…>/messages/#<Message:0x…>`.
            // Same pass the controller and test bodies run, for the
            // same reason and with the same idempotence. In-place + skip
            // the follow-up type when the projection is a no-op — the
            // cloning entry re-walked every jbuilder body twice.
            if crate::lower::controller_to_library::rewrites::
                project_route_helper_ids_in_place(&mut method.body)
            {
                crate::lower::typing::type_method_body(method, &classes, &empty_ivars);
            }
        }
    }
    lcs
}

/// Untyped jbuilder LibraryClasses — method signatures only. The
/// controller lowerer registers these so `Views::X.<action>_json`
/// resolves; body typing is the jbuilder lowerer's job.
pub fn jbuilder_signature_classes(views: &[View], app: &App) -> Vec<LibraryClass> {
    let partials = partial_params(app);
    views
        .iter()
        .filter(|v| v.jbuilder && !v.analysis_only)
        .map(|v| {
            let (module_id, method) = jbuilder_signature_method(v, app, &partials);
            LibraryClass {
                name: module_id,
                is_module: true,
                parent: None,
                parent_span: Default::default(),
                includes: Vec::new(),
                methods: vec![method],
                nullable_columns: Vec::new(),
                origin: None,
                constants: Vec::new(),
                unknown_calls: Vec::new(),
                class_ivar_initializers: Vec::new(),
            }
        })
        .collect()
}

/// Single-template entry. Used by tests and the dump_ir binary; the
/// production bulk path is `lower_jbuilder_to_library_classes`.
pub fn lower_jbuilder_to_library_class(view: &View, app: &App) -> LibraryClass {
    let partials = std::rc::Rc::new(partial_params(app));
    build_library_class(view, app, &partials, /*type_body=*/ true)
}

fn build_library_class(
    view: &View,
    app: &App,
    partials: &std::rc::Rc<PartialParams>,
    type_body: bool,
) -> LibraryClass {
    let (dir, base) = split_view_name(view.name.as_str());
    let stem = base.trim_start_matches('_');
    let is_partial = base.starts_with('_');
    let (module_id, mut method, arg_name, _extra_params, known_models) =
        jbuilder_method_parts(view, app, dir, stem, is_partial, partials);

    // Rewrite `@ivar` → bare `ivar` so the inferred arg / extras
    // read as plain locals. Mirrors the ERB lowerer. A partial's ivar
    // whose name one of its locals takes reads its own parameter
    // (`PartialParams`), so the two stay apart.
    let partial_key = is_partial.then(|| {
        let (mod_path, method) = partial_target(&format!("{dir}/{stem}"), dir);
        partial_key(&mod_path, &method)
    });
    let mut rewritten = rewrite_ivars_to_locals(&view.body);
    if let Some(ivars) = partial_key.as_ref().and_then(|k| partials.ivars.get(k)) {
        let renames: std::collections::HashMap<Symbol, Symbol> = ivars
            .iter()
            .filter(|(iv, p)| crate::naming::safe_local(iv.as_str()) != p.as_str())
            .cloned()
            .collect();
        if !renames.is_empty() {
            let mut body = view.body.clone();
            rename_ivars(&mut body, &renames);
            rewritten = rewrite_ivars_to_locals(&body);
        }
    }
    // A partial's `local_assigns[:x]` is its parameter `x`, nil when
    // the caller did not pass it — the ERB lowering's reading.
    if is_partial {
        crate::lower::view_to_library::rewrite_local_assigns_to_locals(&mut rewritten, &[]);
    }

    // The record the template's temporal columns are read from: the
    // inferred arg, for a partial only when it is one of its params
    // (a partial whose callers name its local `widget` reads that,
    // and the model is the one the name gives, as the param type).
    let params: Vec<&str> = method.params.iter().map(|p| p.name.as_str()).collect();
    let (arg_name, arg_columns) = if arg_name.is_empty() {
        (arg_name, std::collections::HashMap::new())
    } else if !is_partial || params.contains(&arg_name.as_str()) {
        let columns = columns_for_arg(&arg_name, dir, is_partial, stem, app);
        (arg_name, columns)
    } else {
        params
            .iter()
            .find_map(|p| match crate::lower::view_to_library::ivar_ty(p, &known_models) {
                crate::ty::Ty::Class { id, .. } => {
                    Some((p.to_string(), columns_for_model(id.0.as_str(), app)))
                }
                _ => None,
            })
            .unwrap_or_default()
    };
    let ctx = Ctx {
        resource_dir: dir.to_string(),
        accumulator: "io".to_string(),
        arg_name: arg_name.clone(),
        arg_columns,
        direct_helpers: app
            .routes
            .direct_helpers
            .iter()
            .map(|h| h.name.as_str().to_string())
            .collect(),
        models: known_models.iter().cloned().collect(),
        temps: Default::default(),
        partials: partials.clone(),
        partial_key,
    };

    let mut body_stmts: Vec<Expr> = Vec::new();
    body_stmts.push(assign_accumulator_string_new(&ctx.accumulator));
    body_stmts.extend(walk_template(&rewritten, &ctx));
    let mut result = var_ref(Symbol::from(ctx.accumulator.as_str()));
    result.hint = Some(IrHint::StringBuilderResult);
    body_stmts.push(result);

    let mut body = seq(body_stmts);
    // File-grain catch-all: whatever the walk-level stamps didn't reach
    // (`io = String.new`, the `{`/`}` wrappers, the trailing `io`)
    // attributes to the template as a whole — same convention as the
    // ERB lowerer.
    body.inherit_span(view.body.span);
    method.body = body;

    if type_body {
        type_method_body_solo(&mut method);
    }

    LibraryClass {
        name: module_id,
        is_module: true,
        parent: None,
        parent_span: Default::default(),
        includes: Vec::new(),
        methods: vec![method],
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        unknown_calls: Vec::new(),
        class_ivar_initializers: Vec::new(),
    }
}

fn jbuilder_signature_method(
    view: &View,
    app: &App,
    partials: &PartialParams,
) -> (ClassId, MethodDef) {
    let (dir, base) = split_view_name(view.name.as_str());
    let stem = base.trim_start_matches('_');
    let is_partial = base.starts_with('_');
    let (module_id, method, _, _, _) =
        jbuilder_method_parts(view, app, dir, stem, is_partial, partials);
    (module_id, method)
}

/// Params + signature for a jbuilder template. The controller lowerer
/// only needs this shape; `build_library_class` then walks the template
/// into `method.body`.
fn jbuilder_method_parts(
    view: &View,
    app: &App,
    dir: &str,
    stem: &str,
    is_partial: bool,
    partials: &PartialParams,
) -> (ClassId, MethodDef, String, Vec<String>, Vec<String>) {
    let module_id = view_module_id(dir);
    let method_name = Symbol::from(format!("{stem}_json"));

    let known_models: Vec<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .collect();
    let arg_name = infer_view_arg(stem, dir, is_partial, &known_models);

    // The IVARS THIS TEMPLATE READS decide an action view's parameters,
    // and the NAME CONVENTION is only the fallback.
    //
    // `infer_view_arg` guesses from the stem and directory
    // (`articles/index` -> `articles`), which is right whenever the
    // controller named its ivar after the resource — every real-blog
    // json template — and wrong the moment it did not. campfire's
    // `autocompletable/users/index.json.jbuilder` renders
    // `@page.records`: the guess said `users`, the body read `page`, and
    // the method referenced a local that was never a parameter.
    //
    // Same call the render call site's contract uses
    // (`action_view_ivar_map`, which now carries json), so def site and
    // call site cannot disagree about arity or order. The ivars are
    // the template's CLOSURE (`PartialParams::closures`): its own reads
    // and those of every partial it renders, which it passes on.
    let ivar_params: Vec<Symbol> = if is_partial {
        Vec::new()
    } else {
        partials.closures.get(&view.name).cloned().unwrap_or_else(|| {
            crate::lower::view_to_library::view_read_ivars(&view.body)
        })
    };

    // A PARTIAL's parameters are the locals its callers render it
    // with, then the ivars of its closure (`PartialParams`): Rails
    // binds a partial's locals by the names the call passes, never by
    // the partial's own file name, and a partial reads its renderer's
    // ivars.
    let local_params: Option<&Vec<Symbol>> = if is_partial {
        let (mod_path, method) = partial_target(&format!("{dir}/{stem}"), dir);
        partials.params.get(&partial_key(&mod_path, &method))
    } else {
        None
    };

    let nil_default = nil_lit();
    let mut params: Vec<Param> = Vec::new();
    if let Some(locals) = local_params {
        params.extend(locals.iter().cloned().map(Param::positional));
    } else if !ivar_params.is_empty() {
        params.extend(ivar_params.iter().cloned().map(Param::positional));
    } else if !arg_name.is_empty() {
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

    // THE SIGNATURE FOLLOWS THE PARAMS, not the stem/dir guess a second
    // time. `build_view_signature` re-derives the arg from the template
    // path — `autocompletable/users/index.json.jbuilder` gives
    // `users: Array[untyped]` — while the params above come from the
    // ivars the template actually reads (`@page.records` -> `page`). The
    // emitted `def self.index_json(page)` was declared
    // `(Array[untyped] users)`, and `Array[untyped]` on a parameter is a
    // claim about REPRESENTATION: spinel took the controller's
    // `sp_Page *` and passed it where an `sp_PolyArray *` was declared.
    // Mirrors what the ERB lowerer already does — one `typed` list feeds
    // both the params and `build_view_signature_from`.
    let signature = if let Some(locals) = local_params.filter(|l| **l != [Symbol::from(arg_name.as_str())]) {
        // A local named as the convention would have named it keeps
        // the convention's type (the model of the partial's
        // directory); any other takes the model its own name gives.
        let key = {
            let (mod_path, method) = partial_target(&format!("{dir}/{stem}"), dir);
            partial_key(&mod_path, &method)
        };
        let ivar_of = |p: &Symbol| {
            partials.ivars.get(&key).and_then(|ivs| ivs.iter().find(|(_, q)| q == p)).map(|(iv, _)| iv.clone())
        };
        let typed: Vec<(String, crate::ty::Ty)> = locals
            .iter()
            .map(|l| {
                let n = l.as_str().to_string();
                let t = if let Some(iv) = ivar_of(l).filter(|iv| iv.as_str() != n) {
                    // A renamed ivar parameter is typed as its ivar.
                    crate::lower::view_to_library::ivar_ty(iv.as_str(), &known_models)
                } else if n == arg_name {
                    crate::lower::view_to_library::record_arg_ty(dir, false, &known_models)
                } else {
                    crate::lower::view_to_library::ivar_ty(&n, &known_models)
                };
                (n, t)
            })
            .collect();
        crate::lower::view_to_library::build_view_signature_from(&typed, &extra_params)
    } else if ivar_params.is_empty() {
        build_view_signature(stem, dir, is_partial, &arg_name, &extra_params, &known_models)
    } else {
        let typed: Vec<(String, crate::ty::Ty)> = ivar_params
            .iter()
            .map(|iv| {
                let n = iv.as_str().to_string();
                let t = crate::lower::view_to_library::ivar_ty(&n, &known_models);
                (n, t)
            })
            .collect();
        crate::lower::view_to_library::build_view_signature_from(&typed, &extra_params)
    };

    let method = MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: method_name,
        receiver: MethodReceiver::Class,
        params,
        body: seq(Vec::new()),
        signature,
        effects: EffectSet::default(),
        enclosing_class: Some(module_id.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    };
    (module_id, method, arg_name, extra_params, known_models)
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

#[derive(Clone)]
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
    /// temporal columns through their storage text (see
    /// `temporal_column_json`) rather than the generic `encode_value`.
    arg_columns: std::collections::HashMap<Symbol, crate::schema::ColumnType>,
    /// Names declared by `direct :name do |…| … end`, without the
    /// `_path`/`_url` suffix. A direct helper's block parameter is
    /// whatever the CALLER hands it — campfire's `direct
    /// :fresh_user_avatar do |user, options|` reads `user.avatar_token`
    /// and `user.updated_at`, so it wants the RECORD — where a resource
    /// member helper wants the `:id` segment. `rewrite_path_arg_local`
    /// asks this before appending `.id`.
    direct_helpers: std::collections::HashSet<String>,
    /// Every model class the app declares. A route-helper argument
    /// whose last reader NAMES one is a record standing where an id
    /// belongs, and the app's own model list is the evidence — the
    /// alternative is the type, and the ruby emit lowers each jbuilder
    /// template SOLO, with only framework stubs registered, so
    /// `boost.message.room` has no type to ask.
    models: std::collections::HashSet<String>,
    /// Counter for the method's synthesized collection locals
    /// (`__col0`, `__col1`, …), shared by every clone of the template's
    /// `Ctx` so two collection blocks never bind the same name.
    temps: std::rc::Rc<std::cell::Cell<usize>>,
    /// Every jbuilder partial's parameters, for the calls this template
    /// makes to them.
    partials: std::rc::Rc<PartialParams>,
    /// This template's own `partial_key` when it is a partial: the
    /// key its ivar parameters are under (`PartialParams::ivar_param`).
    partial_key: Option<(String, String)>,
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
    /// `json.partial! P, V: <expr>, …` — full-template partial call.
    /// `locals` are the names the partial is rendered with, each with
    /// the value the call binds to it (see `partial_locals`).
    Partial {
        partial_path: String,
        locals: Vec<(Symbol, &'a Expr)>,
    },
    /// `json.partial! @record` — the same call, with the path Rails
    /// takes from the record (`to_partial_path`). Resolved at emit time,
    /// where the app's models are known.
    PartialRecord { arg: &'a Expr, as_name: Option<Symbol> },
    /// `json.<key> obj, partial: P, as: V` — one pair whose value is a
    /// partial render of a SINGLE object. The `array!`/`partial!`
    /// siblings above render a collection and own the whole template;
    /// this one is a pair like any other and composes with them.
    PairPartial {
        key: Symbol,
        partial_path: String,
        arg: &'a Expr,
        as_name: Symbol,
    },
    /// `json.<key> do … end` — one pair whose value is a nested object,
    /// built by re-entering the object walker on the block's body.
    Nested { key: Symbol, body: &'a Expr },
    /// `if c … else … end`, `unless`, or a statement under an `if` /
    /// `unless` modifier — the same branch, its pairs appended inside
    /// it. (`unless` ingests as an `If` with the branches swapped.)
    Cond {
        cond: &'a Expr,
        then_branch: &'a Expr,
        else_branch: &'a Expr,
    },
    /// `begin … rescue … end` (no `else`, no `ensure`) — the same
    /// `begin`, the body's pairs and each `rescue`'s appended inside it.
    Guarded {
        body: &'a Expr,
        rescues: &'a [RescueClause],
    },
    /// `x = <expr>` — a template local. Emitted as written; it adds
    /// no pair.
    Local,
    /// `json.<key> col do |x| … end` — one pair whose value is an array
    /// with one element per member of `col`, each built by the block.
    PairBlock {
        key: Symbol,
        collection: &'a Expr,
        item_var: Symbol,
        body: &'a Expr,
    },
    /// `json.array! col do |x| … end` — the same array as the whole
    /// template.
    ArrayBlock {
        collection: &'a Expr,
        item_var: Symbol,
        body: &'a Expr,
    },
    /// Unrecognized DSL or non-Send statement. Surfaces as an empty io
    /// append so the lowered body stays well-formed.
    Unknown,
}

fn walk_template(body: &Expr, ctx: &Ctx) -> Vec<Expr> {
    emit_object(&flatten_cache_blocks(stmts_of(body)), ctx)
}

/// A template body's top-level statements. A one-statement template is
/// a bare Send rather than a `Seq`.
fn stmts_of(body: &Expr) -> Vec<&Expr> {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    }
}

/// Splice `json.cache! key do … end` blocks open, recursively. The
/// cache is not modeled (see the module header); its block's statements
/// belong to the enclosing object, and flattening them here — before
/// classification — is what keeps `emit_object`'s comma bookkeeping
/// counting real pairs.
fn flatten_cache_blocks(stmts: Vec<&Expr>) -> Vec<&Expr> {
    let mut out: Vec<&Expr> = Vec::new();
    for stmt in stmts {
        match cache_block_body(stmt) {
            Some(body) => out.extend(flatten_cache_blocks(stmts_of(body))),
            None => out.push(stmt),
        }
    }
    out
}

/// The block body of a `json.cache! … do … end`, or None for anything
/// else. `cache!` with no block is not this shape and stays Unknown.
fn cache_block_body(stmt: &Expr) -> Option<&Expr> {
    let ExprNode::Send { recv: Some(recv), method, block: Some(block), .. } = &*stmt.node else {
        return None;
    };
    if method.as_str() != "cache!" || !is_json_receiver(recv) {
        return None;
    }
    let ExprNode::Lambda { body, .. } = &*block.node else {
        return None;
    };
    Some(body)
}

fn emit_object(raw_stmts: &[&Expr], ctx: &Ctx) -> Vec<Expr> {
    let classified: Vec<JbStmt<'_>> = raw_stmts.iter().map(|s| classify(s)).collect();

    // Whole-template DSL forms (single stmt covers the entire JSON
    // body) — array! and partial! produce a top-level array or method
    // call respectively, no `{}` wrap. Template locals around that one
    // statement stay where they are and do not count.
    let mut dsl = classified
        .iter()
        .enumerate()
        .filter(|(_, c)| !matches!(c, JbStmt::Local));
    if let (Some((index, only)), None) = (dsl.next(), dsl.next()) {
        // Synthesis choke point (whole-template forms): everything
        // emitted for the single DSL statement attributes back to it.
        let src_span = raw_stmts[index].span;
        let whole = match only {
            JbStmt::ArrayPartial { collection, partial_path, item_var } => {
                Some(emit_array_partial(collection, partial_path, item_var, ctx))
            }
            JbStmt::Partial { partial_path, locals } => {
                let locals = locals.iter().map(|(k, v)| (k.clone(), (*v).clone())).collect();
                Some(emit_partial_call(partial_path, locals, ctx))
            }
            JbStmt::ArrayBlock { collection, item_var, body } => {
                Some(emit_array_block(collection, item_var, body, ctx))
            }
            JbStmt::PartialRecord { arg, as_name } => {
                record_partial_path(arg, as_name.as_ref(), &ctx.resource_dir, &ctx.models)
                    .map(|(partial_path, local)| {
                        emit_partial_call(&partial_path, vec![(local, (*arg).clone())], ctx)
                    })
            }
            _ => None,
        };
        if let Some(mut whole) = whole {
            for e in &mut whole {
                e.inherit_span(src_span);
            }
            let mut out: Vec<Expr> = Vec::new();
            for (i, src) in raw_stmts.iter().enumerate() {
                if i == index {
                    out.append(&mut whole);
                } else {
                    out.push(emit_local(src, ctx));
                }
            }
            return out;
        }
    }

    // Object form — wrap accumulated pair-emitting statements in
    // `{` … `}`. Comma is inserted between pairs; the lowerer knows
    // statically whether a pair is the first, except after a branch
    // that may or may not have emitted one (`Sep::Unknown`).
    let mut out: Vec<Expr> = Vec::new();
    out.push(io_append_lit(&ctx.accumulator, "{"));
    emit_pairs(&classified, raw_stmts, ctx, &mut out, Sep::First);
    out.push(io_append_lit(&ctx.accumulator, "}"));
    out
}

/// Whether the next pair of an object takes a `,` before it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sep {
    /// No pair yet: no comma.
    First,
    /// At least one pair on every path here: a comma.
    After,
    /// Some paths emitted a pair and some did not (a conditional with
    /// pairs in one branch only): decided when the template runs.
    Unknown,
}

/// The comma before a pair, per `sep`. `Unknown` asks the accumulator
/// (`io << "," if !io.end_with?("{")`):
/// the object's pairs are the only thing appended after its `{`, and
/// no complete JSON value ends in `{`, so the last character is `{`
/// exactly when no pair has been emitted yet.
fn push_separator(out: &mut Vec<Expr>, ctx: &Ctx, sep: Sep) {
    match sep {
        Sep::First => {}
        Sep::After => out.push(io_append_lit(&ctx.accumulator, ",")),
        Sep::Unknown => {
            let opened = send(
                Some(var_ref(Symbol::from(ctx.accumulator.as_str()))),
                "end_with?",
                vec![lit_str("{".to_string())],
                None,
                true,
            );
            out.push(Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: send(Some(opened), "!", Vec::new(), None, false),
                    then_branch: io_append_lit(&ctx.accumulator, ","),
                    else_branch: seq(Vec::new()),
                },
            ));
        }
    }
}

/// The statements of an `if` branch. A missing `else` (and the empty
/// side of a modifier) ingests as `nil`, which has no pairs.
fn branch_stmts(branch: &Expr) -> Vec<&Expr> {
    match &*branch.node {
        ExprNode::Lit { value: Literal::Nil } => Vec::new(),
        _ => flatten_cache_blocks(stmts_of(branch)),
    }
}

/// Append the pairs of an object's statements to `out`, starting from
/// `sep`, and answer the separator state after them.
fn emit_pairs(
    classified: &[JbStmt<'_>],
    raw_stmts: &[&Expr],
    ctx: &Ctx,
    out: &mut Vec<Expr>,
    mut sep: Sep,
) -> Sep {
    for (stmt, src) in classified.iter().zip(raw_stmts.iter()) {
        // Synthesis choke point: everything pushed for this DSL
        // statement (key/comma appends, encode_value calls) attributes
        // back to it. The `{` / `}` wrappers belong to the template as
        // a whole and ride the method-level catch-all instead.
        let start = out.len();
        match stmt {
            JbStmt::Extract { obj, attrs } => {
                let obj_is_arg = obj_is_named_local(obj, &ctx.arg_name);
                for attr in attrs {
                    push_separator(out, ctx, sep);
                    out.push(io_append_lit(
                        &ctx.accumulator,
                        &format!("\"{}\":", attr.as_str()),
                    ));
                    // A temporal column serializes from its `<col>_raw`
                    // storage reader (the stored ISO-8601 text), NOT the
                    // parsing `<col>` reader. Other columns (Integer,
                    // String, …) ride encode_value's type dispatch.
                    let encoded = ctx
                        .arg_columns
                        .get(attr)
                        .filter(|_| obj_is_arg)
                        .and_then(|t| temporal_column_json(obj, attr, t))
                        .unwrap_or_else(|| {
                        json_builder_encode(send(
                            Some((*obj).clone()),
                            attr.as_str(),
                            Vec::new(),
                            None,
                            false,
                        ))
                    });
                    out.push(io_append_call(&ctx.accumulator, encoded));
                    sep = Sep::After;
                }
            }
            JbStmt::Pair { key, value } => {
                push_separator(out, ctx, sep);
                out.push(io_append_lit(
                    &ctx.accumulator,
                    &format!("\"{}\":", key.as_str()),
                ));
                // `json.created_at message.created_at.utc` — a temporal
                // column of the template's argument, bare or through
                // `utc`/`getutc`/`gmtime`, takes the same
                // `encode_datetime(<col>_raw)` route the Extract arm
                // gives `json.(message, :created_at)`: the stored TEXT
                // is UTC, so `utc` is the identity on it, and
                // `encode_value` would otherwise render `Time#to_s`
                // ("2026-09-12 13:26:25 UTC") where Rails renders
                // `xmlschema(3)` ("2026-09-12T13:26:25.149Z").
                let encoded = temporal_column_read(value, ctx)
                    .and_then(|(obj, col)| temporal_column_json(&obj, &col, ctx.arg_columns.get(&col)?))
                    .unwrap_or_else(|| {
                        json_builder_encode(rewrite_h_escape(&rewrite_route_helpers(value, ctx)))
                    });
                out.push(io_append_call(&ctx.accumulator, encoded));
                sep = Sep::After;
            }
            JbStmt::PairPartial { key, partial_path, arg, as_name } => {
                push_separator(out, ctx, sep);
                out.push(io_append_lit(
                    &ctx.accumulator,
                    &format!("\"{}\":", key.as_str()),
                ));
                let arg = rewrite_h_escape(&rewrite_route_helpers(arg, ctx));
                out.extend(emit_partial_call(partial_path, vec![(as_name.clone(), arg)], ctx));
                sep = Sep::After;
            }
            JbStmt::Nested { key, body } => {
                push_separator(out, ctx, sep);
                out.push(io_append_lit(
                    &ctx.accumulator,
                    &format!("\"{}\":", key.as_str()),
                ));
                out.extend(emit_object(
                    &flatten_cache_blocks(stmts_of(body)),
                    ctx,
                ));
                sep = Sep::After;
            }
            JbStmt::PairBlock { key, collection, item_var, body } => {
                push_separator(out, ctx, sep);
                out.push(io_append_lit(
                    &ctx.accumulator,
                    &format!("\"{}\":", key.as_str()),
                ));
                out.extend(emit_array_block(collection, item_var, body, ctx));
                sep = Sep::After;
            }
            JbStmt::ArrayPartial { .. }
            | JbStmt::Partial { .. }
            | JbStmt::ArrayBlock { .. }
            | JbStmt::PartialRecord { .. } => {
                // These shouldn't appear in an object template, but if
                // they do (mixed with pair-emitting stmts), drop a
                // TODO marker rather than emit malformed JSON.
                out.push(io_append_lit(&ctx.accumulator, ""));
            }
            JbStmt::Cond { cond, then_branch, else_branch } => {
                let branch = |body: &Expr| {
                    let stmts = branch_stmts(body);
                    let classified: Vec<JbStmt<'_>> = stmts.iter().map(|s| classify(s)).collect();
                    let mut appends = Vec::new();
                    let after = emit_pairs(&classified, &stmts, ctx, &mut appends, sep);
                    (seq(appends), after)
                };
                let (then_appends, then_sep) = branch(then_branch);
                let (else_appends, else_sep) = branch(else_branch);
                let is_empty = |e: &Expr| matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty());
                // `x if c` keeps its `if`; `x unless c` ingests as an
                // `if c` with an empty then-branch, which reads better
                // as `if !c`.
                let (cond, then_appends, else_appends) =
                    if is_empty(&then_appends) && !is_empty(&else_appends) {
                        let negated = send(Some((*cond).clone()), "!", Vec::new(), None, false);
                        (negated, else_appends, then_appends)
                    } else {
                        ((*cond).clone(), then_appends, else_appends)
                    };
                out.push(Expr::new(
                    Span::synthetic(),
                    ExprNode::If { cond, then_branch: then_appends, else_branch: else_appends },
                ));
                sep = if then_sep == else_sep { then_sep } else { Sep::Unknown };
            }
            JbStmt::Guarded { body, rescues } => {
                sep = emit_guarded(body, rescues, ctx, out, sep);
            }
            JbStmt::Local => {
                out.push(emit_local(src, ctx));
            }
            JbStmt::Unknown => {
                out.push(io_append_lit(&ctx.accumulator, ""));
            }
        }
        for e in &mut out[start..] {
            e.inherit_span(src.span);
        }
    }
    sep
}

/// `begin … rescue … end` around pairs, with Jbuilder's outcome when
/// the body raises: the pairs it finished stay, and the rescue's pairs
/// follow them.
///
///   io_mark = io.length
///   begin
///     io << "\"id\":"
///     io << JsonBuilder.encode_value(widget.id)
///     io_mark = io.length
///     …
///   rescue StandardError
///     io.slice!(io_mark, io.length)
///     io << "," if !(io.end_with?("{"))
///     io << "\"error\":"
///     …
///   end
///
/// Jbuilder sets a pair only once its value is computed, so a raise
/// inside one leaves nothing of it; here the key (and its comma) is
/// already appended by then, which is what the mark after each finished
/// statement is for. A rescue starts from the comma state of any mark,
/// and the state after the whole statement is that of the body's end or
/// any rescue's end.
fn emit_guarded(
    body: &Expr,
    rescues: &[RescueClause],
    ctx: &Ctx,
    out: &mut Vec<Expr>,
    sep: Sep,
) -> Sep {
    let mark = Symbol::from(format!("{}_mark", ctx.accumulator));
    let length = || {
        send(Some(var_ref(Symbol::from(ctx.accumulator.as_str()))), "length", Vec::new(), None, false)
    };
    let set_mark = || {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign { target: LValue::Var { id: VarId(0), name: mark.clone() }, value: length() },
        )
    };
    let merge = |a: Sep, b: Sep| if a == b { a } else { Sep::Unknown };

    out.push(set_mark());
    let mut body_out: Vec<Expr> = Vec::new();
    let mut at_mark = sep;
    let mut state = sep;
    for stmt in branch_stmts(body) {
        state = emit_pairs(&[classify(stmt)], &[stmt], ctx, &mut body_out, state);
        body_out.push(set_mark());
        at_mark = merge(at_mark, state);
    }

    let mut after = state;
    let mut clauses: Vec<RescueClause> = Vec::new();
    for clause in rescues {
        let mut appends = vec![send(
            Some(var_ref(Symbol::from(ctx.accumulator.as_str()))),
            "slice!",
            vec![var_ref(mark.clone()), length()],
            None,
            true,
        )];
        let stmts = branch_stmts(&clause.body);
        let classified: Vec<JbStmt<'_>> = stmts.iter().map(|s| classify(s)).collect();
        let end = emit_pairs(&classified, &stmts, ctx, &mut appends, at_mark);
        after = merge(after, end);
        clauses.push(RescueClause {
            classes: clause.classes.clone(),
            binding: clause.binding.clone(),
            body: seq(appends),
        });
    }
    out.push(Expr::new(
        Span::synthetic(),
        ExprNode::BeginRescue {
            body: seq(body_out),
            rescues: clauses,
            else_branch: None,
            ensure: None,
            implicit: false,
        },
    ));
    after
}

/// A template local as written, its value given the rewrites a pair's
/// value gets (`<x>_url` to its absolute URL over `RouteHelpers.<x>_path`,
/// `h`): the value is read by pairs later, and the emitted view has no
/// `_url` helpers.
fn emit_local(stmt: &Expr, ctx: &Ctx) -> Expr {
    let ExprNode::Assign { target, value } = &*stmt.node else {
        return stmt.clone();
    };
    let mut out = Expr::new(
        stmt.span,
        ExprNode::Assign {
            target: target.clone(),
            value: rewrite_h_escape(&rewrite_route_helpers(value, ctx)),
        },
    );
    out.ty = stmt.ty.clone();
    out
}

fn classify<'a>(stmt: &'a Expr) -> JbStmt<'a> {
    if let ExprNode::BeginRescue { body, rescues, else_branch: None, ensure: None, .. } = &*stmt.node {
        if !rescues.is_empty() {
            return JbStmt::Guarded { body, rescues };
        }
    }
    if let ExprNode::If { cond, then_branch, else_branch } = &*stmt.node {
        return JbStmt::Cond { cond, then_branch, else_branch };
    }
    if let ExprNode::Assign { target: LValue::Var { .. }, .. } = &*stmt.node {
        return JbStmt::Local;
    }
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        block,
        ..
    } = &*stmt.node
    else {
        return JbStmt::Unknown;
    };
    if !is_json_receiver(recv) {
        return JbStmt::Unknown;
    }
    match method.as_str() {
        // `json.extract! obj, :a, :b` and its `.()` spelling
        // `json.(obj, :a, :b)`, which prism parses as a `call` Send.
        // Jbuilder documents the two as the same thing and campfire's
        // bot API writes the short one; matching only `extract!` left
        // every template that used it emitting a `"call"` key.
        "extract!" | "call" => {
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
            // `json.array! col do |x| … end` — the block builds each
            // element. Only with no options: Jbuilder renders a
            // `partial:` before it looks at a block.
            if args.len() == 1 {
                if let Some((item_var, body)) = item_block(block) {
                    if !element_body_supported(body) {
                        return JbStmt::Unknown;
                    }
                    return JbStmt::ArrayBlock { collection, item_var, body };
                }
            }
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
            // `json.partial! partial: P, collection: C, as: V` — the
            // COLLECTION form, which Jbuilder documents as the same
            // thing `array!` does one line up: render each element
            // through the partial, answer a top-level ARRAY. campfire's
            // autocomplete index is spelled this way, and matching only
            // the positional form below left the whole template
            // Unknown — a lowered method whose body emitted `{}` and
            // whose only caller then disagreed with it about arity.
            //
            // Checked BEFORE the positional shape because this call has
            // no positional at all: `args.first()` is the options Hash,
            // and asking it for a string path is what failed.
            if let Some(opts) = args.iter().find_map(extract_hash) {
                if let (Some(partial_path), Some(item_var)) = (
                    hash_get_string(opts, "partial"),
                    hash_get_symbol(opts, "as"),
                ) {
                    if let Some(collection) = hash_get_value(opts, "collection") {
                        return JbStmt::ArrayPartial {
                            collection,
                            partial_path,
                            item_var,
                        };
                    }
                }
            }
            // `json.partial! P, V: <expr>`. First positional is the
            // partial path; trailing Hash has one entry whose value
            // is the arg to pass.
            let Some(path_arg) = args.first() else {
                return JbStmt::Unknown;
            };
            // `json.partial! @record` — a positional that is a record,
            // not a path. Jbuilder renders the record's own partial
            // (`record.to_partial_path`) with the record as its local.
            // `partial:` and `as:` after it do not change that:
            // jbuilder's `partial!` sets `options[:partial]` to the
            // positional, over the option, and `as:` only names the
            // local (`record_partial_path` checks it is the name the
            // lowered partial takes the record under). Any other
            // option is a local for the partial, which the call has no
            // way to pass, so that form stays Unknown.
            if block.is_none() && local_name(path_arg).is_some() {
                let options_ok = match args.len() {
                    1 => true,
                    2 => extract_hash(&args[1]).is_some_and(|opts| {
                        opts.iter().all(|(k, _)| {
                            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                                if matches!(value.as_str(), "partial" | "as"))
                        })
                    }),
                    _ => false,
                };
                if !options_ok {
                    return JbStmt::Unknown;
                }
                // `as:` is a Symbol or a String: Action View `to_sym`s
                // it (`as: "entry"` binds `entry`). Any other value is
                // a name only known at run time.
                let as_value =
                    args.get(1).and_then(extract_hash).and_then(|opts| hash_get_value(opts, "as"));
                let as_name = match as_value {
                    None => None,
                    Some(v) => match &*v.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                        _ => match string_literal(v) {
                            Some(s) => Some(Symbol::from(s.as_str())),
                            None => return JbStmt::Unknown,
                        },
                    },
                };
                return JbStmt::PartialRecord { arg: path_arg, as_name };
            }
            if block.is_some() {
                return JbStmt::Unknown;
            }
            // `json.partial! partial: P, k: v` — the options Hash alone.
            // Jbuilder renders it with every key but its own options as
            // a local.
            if let (Some(opts), 1) = (extract_hash(path_arg), args.len()) {
                let Some(partial_path) = hash_get_string(opts, "partial") else {
                    return JbStmt::Unknown;
                };
                return match partial_locals(opts, &["partial", "as", "collection", "cached"]) {
                    Some(locals) => JbStmt::Partial { partial_path, locals },
                    None => JbStmt::Unknown,
                };
            }
            // `json.partial! P, k: v, …`. The Hash after the path is
            // the partial's locals, keyed by the names the partial
            // reads them under; `as:` with `collection:` is the
            // collection form again, spelled with a positional path.
            let partial_path = match string_literal(path_arg) {
                Some(s) => s,
                None => return JbStmt::Unknown,
            };
            let locals = match args.get(1).map(|a| (extract_hash(a), args.len())) {
                None => Vec::new(),
                Some((Some(opts), 2)) => {
                    if let (Some(item_var), Some(collection)) =
                        (hash_get_symbol(opts, "as"), hash_get_value(opts, "collection"))
                    {
                        return JbStmt::ArrayPartial { collection, partial_path, item_var };
                    }
                    match partial_locals(opts, &["as"]) {
                        Some(locals) => locals,
                        None => return JbStmt::Unknown,
                    }
                }
                Some(_) => return JbStmt::Unknown,
            };
            JbStmt::Partial { partial_path, locals }
        }
        // `json.<key> do … end` — the value is a nested object. No
        // positional args; the block body is another object template.
        key if args.is_empty() && block.is_some() => {
            let Some(block) = block else {
                return JbStmt::Unknown;
            };
            let ExprNode::Lambda { body, .. } = &*block.node else {
                return JbStmt::Unknown;
            };
            JbStmt::Nested { key: Symbol::from(key), body }
        }
        // `json.<key> col do |x| … end` — Jbuilder's `set!` with a value
        // AND a block is `array!` on the value under that key: an array
        // of objects, one per element, each built by the block. Checked
        // before the single-pair shape below, which would otherwise take
        // the collection as the value and drop the block.
        key if args.len() == 1 && block.is_some() => {
            let Some((item_var, body)) = item_block(block) else {
                return JbStmt::Unknown;
            };
            if !element_body_supported(body) {
                return JbStmt::Unknown;
            }
            JbStmt::PairBlock {
                key: Symbol::from(key),
                collection: &args[0],
                item_var,
                body,
            }
        }
        // `json.<key> <expr>` — single-pair shape. The method name IS
        // the JSON key; the single positional arg is the value.
        key if args.len() == 1 => JbStmt::Pair {
            key: Symbol::from(key),
            value: &args[0],
        },
        // `json.<key> obj, partial: P, as: V` — one pair rendered
        // through a partial. Shares its options with `array!` and takes
        // the same two out; what differs is that the positional is ONE
        // record, not a collection, so it emits an object where
        // `array!` emits an array.
        key if args.len() == 2 => {
            let arg = &args[0];
            let Some(opts) = extract_hash(&args[1]) else {
                return JbStmt::Unknown;
            };
            let (Some(partial_path), Some(as_name)) = (
                hash_get_string(opts, "partial"),
                hash_get_symbol(opts, "as"),
            ) else {
                return JbStmt::Unknown;
            };
            JbStmt::PairPartial { key: Symbol::from(key), partial_path, arg, as_name }
        }
        _ => JbStmt::Unknown,
    }
}

/// The element variable and body of a one-parameter block,
/// `do |x| … end` / `{ |x| … }`. Anything else is not the shape.
fn item_block(block: &Option<Expr>) -> Option<(Symbol, &Expr)> {
    let block = block.as_ref()?;
    let ExprNode::Lambda { extra_params, params, rest_param: None, body, .. } = &*block.node else {
        return None;
    };
    if !extra_params.is_empty() {
        return None;
    }
    let [item_var] = params.as_slice() else {
        return None;
    };
    Some((item_var.clone(), body))
}

/// Whether a collection block's body lowers to the element Jbuilder
/// builds. A lone `json.partial!` is the element; a partial next to
/// other statements (or under a branch) renders into the same element
/// in Jbuilder, which the object walker cannot do yet: it writes a
/// partial there as an empty append and the element would lose the
/// partial's fields. Such a body is reported as unsupported and the
/// statement stays Unknown, rather than lowered without them. The same
/// holds for a partial inside a nested `json.<key> do … end` object of
/// the element, which the object walker writes the same way.
fn element_body_supported(body: &Expr) -> bool {
    fn has_partial(stmts: &[&Expr]) -> bool {
        stmts.iter().any(|s| match classify(s) {
            JbStmt::Partial { .. } => true,
            JbStmt::Cond { then_branch, else_branch, .. } => {
                has_partial(&branch_stmts(then_branch)) || has_partial(&branch_stmts(else_branch))
            }
            JbStmt::Nested { body, .. } => has_partial(&flatten_cache_blocks(stmts_of(body))),
            JbStmt::Guarded { body, rescues } => {
                has_partial(&branch_stmts(body))
                    || rescues.iter().any(|r| has_partial(&branch_stmts(&r.body)))
            }
            _ => false,
        })
    }
    let stmts = flatten_cache_blocks(stmts_of(body));
    if stmts.len() == 1 && matches!(classify(stmts[0]), JbStmt::Partial { .. }) {
        return true;
    }
    if !has_partial(&stmts) {
        return true;
    }
    crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
        file: String::new(),
        message: "jbuilder: a collection block that mixes `json.partial!` with other statements is not compiled"
            .to_string(),
    });
    false
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

/// The VALUE under a Symbol key, whatever its shape — the collection
/// expression of a `partial!`/`array!` options hash. Its siblings take
/// a String or a Symbol out; this one takes the expression itself.
fn hash_get_value<'a>(entries: &'a [(Expr, Expr)], key: &str) -> Option<&'a Expr> {
    for (k, v) in entries {
        if let ExprNode::Lit {
            value: Literal::Sym { value },
        } = &*k.node
        {
            if value.as_str() == key {
                return Some(v);
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

/// The locals a `json.partial!` options Hash renders its partial
/// with: each Symbol key but the options in `skip`, with its value, or
/// the entries of an explicit `locals:` Hash, which Jbuilder then takes
/// instead. None when a key is not a Symbol literal, or `locals:` is
/// not a literal Hash or comes with other locals: names the call does
/// not spell out.
fn partial_locals<'a>(entries: &'a [(Expr, Expr)], skip: &[&str]) -> Option<Vec<(Symbol, &'a Expr)>> {
    let mut out: Vec<(Symbol, &'a Expr)> = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
            return None;
        };
        if !skip.contains(&value.as_str()) {
            out.push((value.clone(), v));
        }
    }
    match out.iter().position(|(k, _)| k.as_str() == "locals") {
        None => Some(out),
        Some(_) if out.len() == 1 => partial_locals(extract_hash(out[0].1)?, &[]),
        Some(_) => None,
    }
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

    let partial_call =
        partial_call(partial_path, vec![(item_var.clone(), var_ref(item_var.clone()))], ctx);

    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { extra_params: Vec::new(), rest_param: None,
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

/// An array with one element per member of `collection`, each element
/// the JSON the block's body builds for it:
///
///   __col0 = col
///   io << "["
///   if !(__col0.nil?)
///     io << __col0.map do |x|
///       io_x = String.new
///       io_x << "{" … io_x << "}"
///       io_x
///     end.join(",")
///   end
///   io << "]"
///
/// Same map+join as `emit_array_partial`; the element is the block's
/// body walked as a template of its own, into its own accumulator. A
/// body that is one `json.partial!` call is that call, without the
/// accumulator around it. The collection is bound once to a `__col<n>`
/// local, so an expression (`@widget.parts`) is not evaluated twice by
/// the nil check and the `map`.
fn emit_array_block(collection: &Expr, item_var: &Symbol, body: &Expr, ctx: &Ctx) -> Vec<Expr> {
    let stmts = flatten_cache_blocks(stmts_of(body));
    let single_partial = match stmts.as_slice() {
        [only] => match classify(only) {
            JbStmt::Partial { partial_path, locals } => Some((partial_path, locals)),
            _ => None,
        },
        _ => None,
    };
    // A one-call element reads as a one-line `{ |x| … }`; a built one
    // is several statements and takes `do |x| … end`.
    let block_style = if single_partial.is_some() {
        crate::expr::BlockStyle::Brace
    } else {
        crate::expr::BlockStyle::Do
    };
    let element = match single_partial {
        Some((partial_path, locals)) => {
            // The arguments get the rewrites a `PairPartial` argument
            // gets (`<x>_url` to its absolute URL, `h`).
            let locals = locals
                .iter()
                .map(|(k, v)| (k.clone(), rewrite_h_escape(&rewrite_route_helpers(v, ctx))))
                .collect();
            partial_call(&partial_path, locals, ctx)
        }
        None => {
            let mut inner = ctx.clone();
            inner.accumulator = format!("{}_{}", ctx.accumulator, item_var.as_str());
            let mut exprs = vec![assign_accumulator_string_new(&inner.accumulator)];
            exprs.extend(emit_object(&stmts, &inner));
            let mut result = var_ref(Symbol::from(inner.accumulator.as_str()));
            result.hint = Some(IrHint::StringBuilderResult);
            exprs.push(result);
            seq(exprs)
        }
    };

    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda { extra_params: Vec::new(),
            rest_param: None,
            params: vec![item_var.clone()],
            block_param: None,
            body: element,
            block_style,
        },
    );
    let n = ctx.temps.get();
    ctx.temps.set(n + 1);
    let col = Symbol::from(format!("__col{n}"));
    let bind = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: col.clone() },
            value: collection.clone(),
        },
    );
    let mapped = send(Some(var_ref(col.clone())), "map", Vec::new(), Some(block), false);
    let joined = send(Some(mapped), "join", vec![lit_str(",".to_string())], None, true);
    // Jbuilder's `array!` answers `[]` for a nil collection, and
    // `json.<key>(nil) { … }` goes through it.
    let present = send(
        Some(send(Some(var_ref(col)), "nil?", Vec::new(), None, false)),
        "!",
        Vec::new(),
        None,
        false,
    );
    vec![
        bind,
        io_append_lit(&ctx.accumulator, "["),
        Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: present,
                then_branch: io_append_call(&ctx.accumulator, joined),
                else_branch: seq(Vec::new()),
            },
        ),
        io_append_lit(&ctx.accumulator, "]"),
    ]
}

/// The partial path `json.partial! <local>` renders: Active Model's
/// `to_partial_path`, `"<plural>/<singular>"` of the record's model
/// (`widgets/widget` for a `Widget`), under the namespace of the
/// template's own directory (`admin/widgets/widget` from
/// `admin/widgets/`), which is Action View's
/// `prefix_partial_path_with_controller_namespace` default. The model
/// is the one the local is named after, the same name-to-model reading
/// the template's parameters get (`ivar_ty`). Unresolved when the
/// local names no model of the app, or when `as:` names the partial's
/// local something other than the model's singular: the lowered partial
/// takes its record under that singular, and a partial reading the
/// `as:` name would not find it.
fn record_partial_path(
    arg: &Expr,
    as_name: Option<&Symbol>,
    resource_dir: &str,
    models: &std::collections::HashSet<String>,
) -> Option<(String, Symbol)> {
    let name = local_name(arg)?;
    let model = crate::naming::camelize(name.as_str());
    if !models.contains(&model) {
        return None;
    }
    let singular = crate::naming::snake_case(&model);
    if as_name.is_some_and(|a| a.as_str() != singular) {
        return None;
    }
    let path = format!("{}/{}", crate::naming::pluralize_snake(&model), singular);
    let path = match resource_dir.rsplit_once('/') {
        Some((namespace, _)) => format!("{namespace}/{path}"),
        None => path,
    };
    Some((path, Symbol::from(singular)))
}

/// The name of a bare local: a `Var`, or the receiverless, argless,
/// blockless `Send` prism gives a partial's locals.
fn local_name(e: &Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Var { name, .. } => Some(name.clone()),
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            Some(method.clone())
        }
        _ => None,
    }
}

fn emit_partial_call(partial_path: &str, locals: Vec<(Symbol, Expr)>, ctx: &Ctx) -> Vec<Expr> {
    vec![io_append_call(&ctx.accumulator, partial_call(partial_path, locals, ctx))]
}

/// `Views::<Module>.<p>_json(…)` for a render of `partial_path` with
/// `locals`. The lowered partial takes one positional parameter per
/// local its callers bind (`PartialParams`), so the call passes each
/// parameter the value this call binds under that name, and `nil` for
/// a local only other callers pass: Rails would leave it unset, which
/// the partial reads through `local_assigns[:x]` as nil. An ivar the
/// partial reads is passed on from the caller, whose own closure made
/// it a parameter there too. A partial this app has no jbuilder
/// template for takes the values in call order.
fn partial_call(partial_path: &str, locals: Vec<(Symbol, Expr)>, ctx: &Ctx) -> Expr {
    let (mod_path, method) = partial_target(partial_path, &ctx.resource_dir);
    let key = partial_key(&mod_path, &method);
    let args: Vec<Expr> = match ctx.partials.params.get(&key) {
        Some(params) => {
            let ivars = ctx.partials.ivars.get(&key);
            let mut locals = locals;
            let binds = |k: &Symbol, p: &Symbol| crate::naming::safe_local(k.as_str()) == p.as_str();
            params
                .iter()
                .map(|p| {
                    // An ivar the partial reads: the caller's own
                    // parameter for it, which its closure gave it.
                    if let Some((iv, _)) = ivars.and_then(|ivs| ivs.iter().find(|(_, q)| q == p)) {
                        return var_ref(ctx.partials.ivar_param(ctx.partial_key.as_ref(), iv));
                    }
                    match locals.iter().position(|(k, _)| binds(k, p)) {
                        Some(i) => locals.remove(i).1,
                        None => nil_lit(),
                    }
                })
                .collect()
        }
        None => locals.into_iter().map(|(_, v)| v).collect(),
    };
    send(Some(const_path(&mod_path)), &format!("{method}_json"), args, None, true)
}

/// The contract between the app's jbuilder templates and the partials
/// they render, computed once from every template.
///
/// A partial's parameters (`params`, keyed by `partial_key`) are the
/// locals the app's jbuilder templates render it with, in first-seen
/// order — a `json.partial!` locals key, the `as:` of a collection or
/// one-record render, the record's own name for `json.partial!
/// @record` — then the `local_assigns[:x]` keys the partial reads that
/// no call passes, then the ivars of its closure. Rails binds a partial's locals by those names, never by the
/// partial's file name, so two callers that pass different keys give
/// the partial one parameter per key, each caller passing nil for the
/// keys it does not. A partial no template renders by a recognized call
/// keeps the name convention (`infer_view_arg`) for its locals.
///
/// A partial reads its renderer's ivars, as every Rails view in the
/// request does. `closures` (keyed by view name) is each template's
/// own ivar reads and, transitively, those of every partial it
/// renders: an action template takes them all as parameters, which
/// the controller passes (`action_view_ivar_map` reads the same map),
/// and passes each partial the ones it reads (`ivars`).
///
/// A local and an ivar of one name are two values in Rails — `note`
/// is what the caller passed, `@note` the controller's — so they are
/// two parameters here. The ivar's is named after it (`note`) unless a
/// local of the partial already takes that name; then it is the
/// generated `__rh_ivar_note` (`_2`, `_3`, … past any name the partial
/// uses), its `@note` reads are rewritten to it, and every render
/// passes it on under the caller's own parameter for `@note`.
#[derive(Default)]
pub(crate) struct PartialParams {
    params: std::collections::HashMap<(String, String), Vec<Symbol>>,
    /// Each partial's ivar parameters: the ivar, then its parameter.
    ivars: std::collections::HashMap<(String, String), Vec<(Symbol, Symbol)>>,
    closures: std::collections::HashMap<Symbol, Vec<Symbol>>,
}

impl PartialParams {
    /// The parameter a template reads `@ivar` through: a partial's
    /// own, else the ivar's name, as an action template takes it.
    fn ivar_param(&self, key: Option<&(String, String)>, ivar: &Symbol) -> Symbol {
        key.and_then(|k| self.ivars.get(k))
            .and_then(|ivs| ivs.iter().find(|(iv, _)| iv == ivar))
            .map(|(_, p)| p.clone())
            .unwrap_or_else(|| Symbol::from(crate::naming::safe_local(ivar.as_str())))
    }
}

fn partial_params(app: &App) -> PartialParams {
    let known_models: Vec<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    let views: Vec<&View> = app.views.iter().filter(|v| v.jbuilder && !v.analysis_only).collect();
    let edges = render_edges(&views, &known_models);
    let closures = ivar_closures(&views, &edges);
    let mut bound: std::collections::HashMap<(String, String), Vec<Symbol>> =
        std::collections::HashMap::new();
    for (_, sites) in &edges {
        for (key, _, names) in sites {
            let entry = bound.entry(key.clone()).or_default();
            for n in names {
                let n = Symbol::from(crate::naming::safe_local(n.as_str()));
                if !entry.contains(&n) {
                    entry.push(n);
                }
            }
        }
    }
    let mut out = PartialParams { closures, ..Default::default() };
    for v in &views {
        let (dir, base) = split_view_name(v.name.as_str());
        let Some(stem) = base.strip_prefix('_') else { continue };
        let (mod_path, method) = partial_target(&format!("{dir}/{stem}"), dir);
        let key = partial_key(&mod_path, &method);
        let mut names = bound.get(&key).cloned().unwrap_or_default();
        let mut optional = Vec::new();
        collect_local_assigns_keys(&v.body, &mut optional);
        for n in optional {
            let n = Symbol::from(crate::naming::safe_local(&n));
            if !names.contains(&n) {
                names.push(n);
            }
        }
        if names.is_empty() {
            let arg_name = infer_view_arg(stem, dir, true, &known_models);
            if !arg_name.is_empty() {
                names.push(Symbol::from(arg_name));
            }
        }
        let locals = names.len();
        let mut used = std::collections::HashSet::new();
        collect_local_names(&v.body, &mut used);
        used.extend(names.iter().cloned());
        let mut ivars = Vec::new();
        for iv in out.closures.get(&v.name).into_iter().flatten() {
            let own = Symbol::from(crate::naming::safe_local(iv.as_str()));
            let param = if names[..locals].contains(&own) {
                let base = format!("__rh_ivar_{own}");
                let fresh = std::iter::once(base.clone())
                    .chain((2u32..).map(|i| format!("{base}_{i}")))
                    .map(Symbol::from)
                    .find(|n| !used.contains(n))
                    .expect("an unused name");
                used.insert(fresh.clone());
                fresh
            } else {
                own
            };
            if !names.contains(&param) {
                names.push(param.clone());
            }
            ivars.push((iv.clone(), param));
        }
        out.params.insert(key.clone(), names);
        out.ivars.insert(key, ivars);
    }
    out
}

/// Each jbuilder template's partial renders: the partial's key, its
/// view name when the app has a jbuilder template for it, and the
/// local names the render binds.
type RenderSites = Vec<((String, String), Option<Symbol>, Vec<Symbol>)>;

fn render_edges(views: &[&View], known_models: &[String]) -> Vec<(Symbol, RenderSites)> {
    let models: std::collections::HashSet<String> = known_models.iter().cloned().collect();
    let mut partial_views: std::collections::HashMap<(String, String), Symbol> =
        std::collections::HashMap::new();
    for v in views {
        let (dir, base) = split_view_name(v.name.as_str());
        if let Some(stem) = base.strip_prefix('_') {
            let (mod_path, method) = partial_target(&format!("{dir}/{stem}"), dir);
            partial_views.insert(partial_key(&mod_path, &method), v.name.clone());
        }
    }
    views
        .iter()
        .map(|v| {
            let (dir, _) = split_view_name(v.name.as_str());
            let mut sites = Vec::new();
            collect_partial_sites(&v.body, dir, &models, &mut sites);
            let sites = sites
                .into_iter()
                .map(|(key, names)| {
                    let target = partial_views.get(&key).cloned();
                    (key, target, names)
                })
                .collect();
            (v.name.clone(), sites)
        })
        .collect()
}

/// The ivar closure of every template: its own reads, in first-seen
/// order, then each rendered partial's closure, depth first. A render
/// cycle contributes what was found before it closed.
fn ivar_closures(
    views: &[&View],
    edges: &[(Symbol, RenderSites)],
) -> std::collections::HashMap<Symbol, Vec<Symbol>> {
    use std::collections::{HashMap, HashSet};
    fn visit(
        name: &Symbol,
        bodies: &HashMap<Symbol, &Expr>,
        children: &HashMap<Symbol, Vec<Symbol>>,
        done: &mut HashMap<Symbol, Vec<Symbol>>,
        open: &mut HashSet<Symbol>,
    ) -> Vec<Symbol> {
        if let Some(c) = done.get(name) {
            return c.clone();
        }
        if !open.insert(name.clone()) {
            return Vec::new();
        }
        let mut out = bodies
            .get(name)
            .map(|b| crate::lower::view_to_library::view_read_ivars(b))
            .unwrap_or_default();
        for child in children.get(name).into_iter().flatten() {
            for iv in visit(child, bodies, children, done, open) {
                if !out.contains(&iv) {
                    out.push(iv);
                }
            }
        }
        open.remove(name);
        done.insert(name.clone(), out.clone());
        out
    }
    let bodies: HashMap<Symbol, &Expr> = views.iter().map(|v| (v.name.clone(), &v.body)).collect();
    let children: HashMap<Symbol, Vec<Symbol>> = edges
        .iter()
        .map(|(name, sites)| (name.clone(), sites.iter().filter_map(|(_, t, _)| t.clone()).collect()))
        .collect();
    let mut done = HashMap::new();
    for v in views {
        visit(&v.name, &bodies, &children, &mut done, &mut HashSet::new());
    }
    done
}

/// The jbuilder side of the view render graph, for the analyzer:
/// each jbuilder template's name with the names of the jbuilder
/// partials it renders, so a partial is typed with its renderers'
/// ivars as an ERB partial is.
pub(crate) fn jbuilder_render_edges(app: &App) -> Vec<(Symbol, Vec<Symbol>)> {
    let known_models: Vec<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    let views: Vec<&View> = app.views.iter().filter(|v| v.jbuilder && !v.analysis_only).collect();
    render_edges(&views, &known_models)
        .into_iter()
        .map(|(name, sites)| (name, sites.into_iter().filter_map(|(_, t, _)| t).collect()))
        .collect()
}

/// Each jbuilder template's ivar closure, keyed by view name — the
/// parameters an action template takes and the controller passes.
pub(crate) fn jbuilder_ivar_closures(
    views: &[View],
    models: &[crate::dialect::Model],
) -> std::collections::HashMap<Symbol, Vec<Symbol>> {
    let known_models: Vec<String> = models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    let views: Vec<&View> = views.iter().filter(|v| v.jbuilder && !v.analysis_only).collect();
    let edges = render_edges(&views, &known_models);
    ivar_closures(&views, &edges)
}

/// Every name a template binds or reads as a local — variables, block
/// parameters, and the receiverless reads a partial's locals are — so a
/// generated parameter name takes none of them.
fn collect_local_names(e: &Expr, out: &mut std::collections::HashSet<Symbol>) {
    match &*e.node {
        ExprNode::Var { name, .. }
        | ExprNode::Assign { target: LValue::Var { name, .. }, .. } => {
            out.insert(name.clone());
        }
        ExprNode::Send { recv: None, method, args, .. } if args.is_empty() => {
            out.insert(method.clone());
        }
        ExprNode::Lambda { params, rest_param, block_param, .. } => {
            out.extend(params.iter().cloned());
            out.extend(rest_param.iter().cloned());
            out.extend(block_param.iter().cloned());
        }
        _ => {}
    }
    e.node.for_each_child(&mut |c| collect_local_names(c, out));
}

/// Every partial render in a jbuilder template, with the local names
/// it binds. Calls with a block are skipped (no partial call has one)
/// and walked into, so the collection blocks' own partial calls count.
fn collect_partial_sites(
    e: &Expr,
    dir: &str,
    models: &std::collections::HashSet<String>,
    out: &mut Vec<((String, String), Vec<Symbol>)>,
) {
    if let ExprNode::Send { recv: Some(recv), block: None, .. } = &*e.node {
        if is_json_receiver(recv) {
            let site = match classify(e) {
                JbStmt::Partial { partial_path, locals } => {
                    Some((partial_path, locals.into_iter().map(|(k, _)| k).collect()))
                }
                JbStmt::ArrayPartial { partial_path, item_var, .. } => Some((partial_path, vec![item_var])),
                JbStmt::PairPartial { partial_path, as_name, .. } => Some((partial_path, vec![as_name])),
                JbStmt::PartialRecord { arg, as_name } => {
                    record_partial_path(arg, as_name.as_ref(), dir, models).map(|(p, l)| (p, vec![l]))
                }
                _ => None,
            };
            if let Some((path, names)) = site {
                let (mod_path, method) = partial_target(&path, dir);
                out.push((partial_key(&mod_path, &method), names));
            }
        }
    }
    e.node.for_each_child(&mut |c| collect_partial_sites(c, dir, models, out));
}

/// The `local_assigns[:x]` keys a template reads, in first-seen order.
fn collect_local_assigns_keys(e: &Expr, out: &mut Vec<String>) {
    if let Some(k) = crate::lower::view_to_library::local_assigns_key(e) {
        if !out.contains(&k) {
            out.push(k);
        }
    }
    e.node.for_each_child(&mut |c| collect_local_assigns_keys(c, out));
}

/// The `PartialParams` key of a partial: its module path and method
/// base, as `partial_target` resolves them.
fn partial_key(mod_path: &[Symbol], method: &str) -> (String, String) {
    let module = mod_path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    (module, method.to_string())
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
    // `camelize_path`, not `camelize`: a NESTED partial dir carries a
    // slash (`autocompletable/users`) and the module it names is
    // `Views::Autocompletable::Users`. The plain camelizer left the
    // slash in, so the emitted call read `Views::Autocompletable/users
    // .user_json(user)` — which parses as DIVISION and dies on an
    // undefined `users`. Same helper the ERB view lowering's
    // `view_module_id` uses, so the two agree on where a nested view
    // module lives.
    let module_camel =
        crate::naming::camelize_path(&crate::naming::snake_case(&dir));
    let module_path: Vec<Symbol> = std::iter::once(Symbol::from("Views"))
        .chain(module_camel.split("::").map(Symbol::from))
        .collect();
    (module_path, base)
}

/// `h(x)` in a JSON template is a REAL escape, unlike its ERB twin.
///
/// The ERB walker UNWRAPS `h(...)` — the auto-escape wrapper it adds is
/// the same operation, and leaving both would double-escape. A JSON
/// value has no such wrapper: `JsonBuilder.encode_value` escapes for
/// JSON (quotes, backslashes, control characters) and not for HTML, so
/// dropping the `h` would ship `<script>` verbatim where Rails ships
/// `&lt;script&gt;`. campfire's autocomplete asserts exactly that on a
/// user whose name is `David <script>alert(123)</script>`.
///
/// Nested `h(h(x))` double-escapes, as it does in Rails.
fn rewrite_h_escape(e: &Expr) -> Expr {
    let new_node = match &*e.node {
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "h" && args.len() == 1 =>
        {
            ExprNode::Send {
                recv: Some(Expr::new(
                    e.span,
                    ExprNode::Const {
                        path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")],
                    },
                )),
                method: Symbol::from("html_escape"),
                args: vec![rewrite_h_escape(&args[0])],
                block: None,
                parenthesized: true,
            }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(rewrite_h_escape),
            method: method.clone(),
            args: args.iter().map(rewrite_h_escape).collect(),
            block: block.as_ref().map(rewrite_h_escape),
            parenthesized: *parenthesized,
        },
        _ => return e.clone(),
    };
    let mut out = Expr::new(e.span, new_node);
    out.ty = e.ty.clone();
    out
}

// ── route-helper rewrite for pair values ────────────────────────────

/// Rewrite bare `<x>_url(record, format: :fmt, ...)` calls into
/// `ActionView::ViewHelpers.url_for_path(RouteHelpers.<x>_path(record.id))`.
/// The runtime's `app/route_helpers.rb` (generated by `routes_to_library`)
/// exposes `_path` helpers only; `url_for_path` adds the host half
/// where the runtime knows it. The ruby family answers Rails' absolute
/// `http://host/articles/1.json` (`view_helpers_ext.rb`, over
/// `Rails.application.protocol` / `.domain`). The strict targets carry
/// no request into a view and answer the path, which is what this
/// rewrite rendered everywhere before.
///
/// A `format: :<sym>` kwarg appends `.<sym>` to the path, so JSON
/// templates emit the `…/articles/1.json` self-link shape Rails
/// produces. Other kwargs still drop on the floor.
fn rewrite_route_helpers(e: &Expr, ctx: &Ctx) -> Expr {
    let new_node = match &*e.node {
        ExprNode::Send {
            recv: None,
            method,
            args,
            block,
            parenthesized,
        } if method.as_str().ends_with("_url") => {
            let stem = &method.as_str()[..method.as_str().len() - 4];
            let path_name = format!("{stem}_path");
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
                .map(|a| rewrite_path_arg_local(a, stem, ctx))
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
            let path = match format_sym {
                Some(fmt) => send(
                    Some(path_call),
                    "+",
                    vec![lit_str(format!(".{}", fmt.as_str()))],
                    None,
                    false,
                ),
                None => path_call,
            };
            return send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")],
                    },
                )),
                "url_for_path",
                vec![path],
                None,
                true,
            );
        }
        ExprNode::Send {
            recv,
            method,
            args,
            block,
            parenthesized,
        } => ExprNode::Send {
            recv: recv.as_ref().map(|r| rewrite_route_helpers(r, ctx)),
            method: method.clone(),
            args: args.iter().map(|a| rewrite_route_helpers(a, ctx)).collect(),
            block: block.as_ref().map(|b| rewrite_route_helpers(b, ctx)),
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
/// The positional argument of a rewritten `<x>_url(…)` call.
///
/// A resource member helper is typed for the `:id` SEGMENT
/// (`article_path(Integer id)`), so a bare record local becomes
/// `record.id`. A `direct` helper is not: its body is the block the app
/// wrote, and campfire's `direct :fresh_user_avatar do |user, options|`
/// reads `user.avatar_token` and `user.updated_at` off it. Handing that
/// one an Integer is `undefined method 'updated_at' for an instance of
/// Integer` — at RENDER time, in a template that compiled fine.
fn rewrite_path_arg_local(arg: &Expr, stem: &str, ctx: &Ctx) -> Expr {
    if ctx.direct_helpers.contains(stem) {
        return arg.clone();
    }
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
    if let Some(n) = name {
        return send(Some(var_ref(n)), "id", Vec::new(), None, false);
    }
    // An ATTRIBUTE CHAIN whose last reader names a model —
    // `boost.message`, `boost.message.room`. Same record-standing-where-
    // an-id-belongs case the bare local above covers, one link further
    // out: campfire's boost partial writes
    // `room_message_url(boost.message.room, boost.message)`, and both
    // arguments reached the helper whole, so every boost's `url` read
    // `/rooms/#<Room:0x…>/messages/#<Message:0x…>`.
    //
    // The model list is what keeps this from being a blind projection:
    // `users(:david).bot_key` and `message.room_id` both end in a
    // reader too, and neither names a model.
    if let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &*arg.node {
        if args.is_empty()
            && method.as_str() != "id"
            && ctx
                .models
                .contains(&crate::naming::singularize_camelize(method.as_str()))
        {
            return send(Some(arg.clone()), "id", Vec::new(), None, false);
        }
    }
    arg.clone()
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

/// The JSON value of `<obj>.<col>` for a temporal column, from its
/// `<col>_raw` storage reader.
///
/// * datetime / time: `JsonBuilder.encode_datetime(<obj>.<col>_raw)`,
///   the exact string→string reformat to `xmlschema(3)` (no float
///   sub-second hazards, no native parse→format round-trip per row).
/// * date: `JsonBuilder.encode_value(ActiveSupport.format_db_date(
///   ActiveSupport.parse_db_date(<obj>.<col>_raw)))` — the column's
///   own seam, as the model's `as_json` writer uses. A date has no
///   clock, so `encode_datetime` is the wrong primitive for it, and it
///   quoted the "" an unset nonnullable slot (or an adapter's NULL)
///   holds where Rails renders `null`.
///
/// `None` for any other column type.
fn temporal_column_json(obj: &Expr, col: &Symbol, ty: &crate::schema::ColumnType) -> Option<Expr> {
    use crate::schema::ColumnType;
    let raw = send(Some(obj.clone()), &format!("{}_raw", col.as_str()), Vec::new(), None, false);
    match ty {
        ColumnType::DateTime | ColumnType::Time => Some(json_builder_call("encode_datetime", raw)),
        ColumnType::Date => {
            let active_support = || {
                Expr::new(Span::synthetic(), ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] })
            };
            let date = send(Some(active_support()), "parse_db_date", vec![raw], None, true);
            let text = send(Some(active_support()), "format_db_date", vec![date], None, true);
            Some(json_builder_encode(text))
        }
        _ => None,
    }
}

/// True when `obj` reads as the named local — either a bare `Var`
/// or a `Send` with no receiver, no args, no block (the bareword
/// shape Prism produces for partial-scope locals).
/// `<arg>.<temporal col>`, optionally under `.utc` / `.getutc` /
/// `.gmtime` — the (receiver, column) pair, when the column is one of
/// the template argument's datetime/date/time columns.
fn temporal_column_read(value: &Expr, ctx: &Ctx) -> Option<(Expr, Symbol)> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*value.node else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let (obj, col) = if matches!(method.as_str(), "utc" | "getutc" | "gmtime") {
        let ExprNode::Send { recv: Some(obj), method: col, args, block: None, .. } = &*recv.node else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        (obj, col)
    } else {
        (recv, method)
    };
    if !obj_is_named_local(obj, &ctx.arg_name) {
        return None;
    }
    let temporal = matches!(
        ctx.arg_columns.get(col),
        Some(crate::schema::ColumnType::DateTime)
            | Some(crate::schema::ColumnType::Date)
            | Some(crate::schema::ColumnType::Time)
    );
    temporal.then(|| (obj.clone(), col.clone()))
}

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
    out = columns_for_model(&model_class, app);
    let _ = arg_name; // arg_name is informational; the dir → model
                     // resolution above is the load-bearing path.
    out
}

/// The `{column_name → ColumnType}` map of `model_class`'s table; empty
/// when the app has no such model or table.
fn columns_for_model(
    model_class: &str,
    app: &App,
) -> std::collections::HashMap<Symbol, crate::schema::ColumnType> {
    let mut out: std::collections::HashMap<Symbol, crate::schema::ColumnType> =
        std::collections::HashMap::new();
    let Some(model) = app.models.iter().find(|m| m.name.0.as_str() == model_class) else {
        return out;
    };
    let Some(table) = app.schema.tables.get(&model.table.0) else {
        return out;
    };
    for col in &table.columns {
        out.insert(col.name.clone(), col.col_type.clone());
    }
    out
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

/// `@x` → `@<renames[x]>`, reads and writes, before the rewrite to
/// locals: a partial's ivar parameter whose name a local takes.
fn rename_ivars(e: &mut Expr, renames: &std::collections::HashMap<Symbol, Symbol>) {
    match &mut *e.node {
        ExprNode::Ivar { name } | ExprNode::Assign { target: LValue::Ivar { name }, .. } => {
            if let Some(to) = renames.get(name) {
                *name = to.clone();
            }
        }
        _ => {}
    }
    e.node.for_each_child_mut(&mut |c| rename_ivars(c, renames));
}

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
        ExprNode::Lambda { rest_param, extra_params,
            params,
            block_param,
            body,
            block_style,
        } => ExprNode::Lambda { rest_param: rest_param.clone(), extra_params: extra_params.clone(),
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
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, implicit } => {
            ExprNode::BeginRescue {
                body: rewrite_ivars_to_locals(body),
                rescues: rescues
                    .iter()
                    .map(|r| RescueClause {
                        classes: r.classes.clone(),
                        binding: r.binding.clone(),
                        body: rewrite_ivars_to_locals(&r.body),
                    })
                    .collect(),
                else_branch: else_branch.as_ref().map(rewrite_ivars_to_locals),
                ensure: ensure.as_ref().map(rewrite_ivars_to_locals),
                implicit: *implicit,
            }
        }
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
        LValue::Const { path } => LValue::Const { path: path.clone() },
    }
}

// Silence unused-import warnings until we wire up a future stretch
// primitive that needs `singularize`.
#[allow(dead_code)]
fn _unused() {
    let _ = singularize;
}
