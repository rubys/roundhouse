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
//!   9. `json.cache! key do … end`      → Jbuilder's fragment cache
//!                                       when it wraps the whole
//!                                       template; otherwise transparent:
//!                                       the block's statements ARE the
//!                                       object's
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
//!                                       the record do not change it,
//!                                       nor does a literal local the
//!                                       partial never reads, such as
//!                                       `cache: true`)
//!
//! (6)-(9) arrived together with campfire's bot API, which is six
//! jbuilder templates written in exactly that dialect.
//!
//! `cache!` is Jbuilder's fragment cache keyed on the record. A template
//! that is one `cache!` around everything it writes is cached as Jbuilder
//! caches it, through the controller's `perform_caching` and
//! `combined_fragment_cache_key` and `JbuilderTemplate`'s read/write pair
//! (`walk_cached_template`); the strict targets' runtime never caches, so
//! there it renders every time. Any other `cache!` (among sibling pairs,
//! nested, with options) is still read transparently: correct output at
//! the wrong cost, the trade the ERB `<% cache %>` takes when it cannot
//! build a key. Reading it as an ordinary `json.<key>` pair, which is
//! what the classifier did before either, was neither: it emitted a
//! literal `"cache!"` key holding the whole record.
//!
//! Still deferred: `json.key_format!`, `json.ignore_nil!` and
//! `json.child!`.

use crate::App;
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param, View};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, InterpPart, IrHint, LValue, Literal, RescueClause};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::singularize;
use crate::span::Span;

mod record_partial;
mod record_value;

use record_partial::Unresolved;
use record_value::{RecordShapes, RecordValue};

use super::view_to_library::{
    build_view_signature, infer_view_arg, insert_framework_stubs, split_view_name, view_module_id,
    PartialStems,
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
    let direct_helpers = crate::lower::routes::direct_helper_names(app);
    let mut lcs: Vec<LibraryClass> = views
        .iter()
        .filter(|v| v.jbuilder && !v.analysis_only)
        .map(|v| build_library_class(v, app, &partials, /*type_body=*/ false))
        .collect();

    // Merge: caller extras + framework runtime stubs + the jbuilder LCs
    // themselves (so `Views::Articles.article_json` resolves when
    // referenced from `Views::Articles.index_json`).
    let mut classes: crate::det::HashMap<ClassId, crate::analyze::ClassInfo> =
        crate::det::HashMap::default();
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

    let empty_ivars: crate::det::HashMap<Symbol, crate::ty::Ty> =
        crate::det::HashMap::default();
    // Collect once for every view — `ground_jbuilder_library` walks
    // value-position blank predicates after typing (shared with the
    // single-template entry so Ruby emit and every other target agree).
    let blank_defs = crate::lower::blank::AppDefinitions::collect(app);
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
                project_route_helper_ids_in_place(&mut method.body, &direct_helpers)
            {
                crate::lower::typing::type_method_body(method, &classes, &empty_ivars);
            }
        }
        ground_jbuilder_library(lc, &blank_defs, |method| {
            crate::lower::typing::type_method_body(method, &classes, &empty_ivars);
        });
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
                include_spans: Vec::new(),
                methods: vec![method],
                nullable_columns: Vec::new(),
                origin: None,
                constants: Vec::new(),
                conditional_definitions: Vec::new(),
                unknown_calls: Vec::new(),
                class_ivar_initializers: Vec::new(),
            }
        })
        .collect()
}

/// Single-template entry. Ruby/Spinel emit (`emit_lowered_jbuilder_views`)
/// and focused tests use this; every other emitter goes through
/// [`lower_jbuilder_to_library_classes`]. Both call
/// [`ground_jbuilder_library`] so value-position blank predicates are
/// grounded once, in a shared home.
pub fn lower_jbuilder_to_library_class(view: &View, app: &App) -> LibraryClass {
    let partials = std::rc::Rc::new(partial_params(app));
    let mut lc = build_library_class(view, app, &partials, /*type_body=*/ true);
    // `presence` / `present?` / `blank?` in a value
    // (`json.note(widget.note.presence || "none")`): the app-wide blank
    // lowering walks no view, and the Spinel runtime has no such method
    // on String or nil. Ground them as every app body's are.
    let defs = crate::lower::blank::AppDefinitions::collect(app);
    ground_jbuilder_library(&mut lc, &defs, type_method_body_solo);
    lc
}

/// Ground `presence` / `present?` / `blank?` in jbuilder value positions
/// on an already-typed library class, then retype any body that changed.
/// Shared by the bulk and single-template entries so a target cannot
/// silently miss the rewrite by picking the other door.
fn ground_jbuilder_library(
    lc: &mut LibraryClass,
    defs: &crate::lower::blank::AppDefinitions,
    mut retype: impl FnMut(&mut MethodDef),
) {
    for method in &mut lc.methods {
        if crate::lower::blank::ground_jbuilder_values(&mut method.body, defs) {
            retype(method);
        }
    }
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
        let (mod_path, method) = partial_target(&format!("{dir}/{stem}"), dir, &partials.stems);
        partial_key(&mod_path, &method)
    });
    let mut rewritten = rewrite_ivars_to_locals(&view.body);
    // The request's `params`: the parameter the template's closure gave
    // it (`reads_request_params`). A partial whose callers pass a local
    // named `params` reads that local instead, as Rails' would — and
    // that explicit local must win even when the request-params ivar
    // was renamed to `__rh_ivar_params` to avoid clashing with it.
    let params_param: Option<Symbol> = match &partial_key {
        Some(k) => {
            let explicit = partials
                .params
                .get(k)
                .and_then(|names| names.iter().find(|n| n.as_str() == "params"))
                .cloned();
            if explicit.is_some() {
                explicit
            } else {
                partials
                    .ivars
                    .get(k)
                    .and_then(|ivs| ivs.iter().find(|(iv, _)| iv.as_str() == "params"))
                    .map(|(_, p)| p.clone())
                    .filter(|p| p.as_str() == "params")
            }
        }
        None => method.params.iter().find(|p| p.name.as_str() == "params").map(|p| p.name.clone()),
    };
    if let Some(ivars) = partial_key.as_ref().and_then(|k| partials.ivars.get(k)) {
        let renames: crate::det::HashMap<Symbol, Symbol> = ivars
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
    if let Some(p) = &params_param {
        request_params_to_local(&mut rewritten, p);
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
    let (arg_name, arg_model) = if arg_name.is_empty() {
        (arg_name, None)
    } else if !is_partial || params.contains(&arg_name.as_str()) {
        let model = model_for_arg(dir, is_partial, stem, app);
        (arg_name, model)
    } else {
        params
            .iter()
            .find_map(|p| match crate::lower::view_to_library::ivar_ty(p, &known_models) {
                crate::ty::Ty::Class { id, .. } => Some((p.to_string(), Some(id.0.as_str().to_string()))),
                _ => None,
            })
            .unwrap_or_default()
    };
    let arg_columns = arg_model.as_deref().map(|m| columns_for_model(m, app)).unwrap_or_default();
    let arg_attributes = arg_model.as_deref().and_then(|m| attribute_names(m, app));
    let ctx = Ctx {
        resource_dir: dir.to_string(),
        accumulator: "io".to_string(),
        arg_name: arg_name.clone(),
        arg_columns,
        arg_attributes,
        direct_helpers: app
            .routes
            .direct_helpers
            .iter()
            .map(|h| h.name.as_str().to_string())
            .collect(),
        models: known_models.iter().cloned().collect(),
        model_columns: std::rc::Rc::new(
            known_models.iter().map(|m| (m.clone(), columns_for_model(m, app))).collect(),
        ),
        temps: Default::default(),
        key_marks: Default::default(),
        sound_merges: Default::default(),
        partials: partials.clone(),
        partial_key,
        template: format!("{dir}/{base}"),
        record_sites: std::rc::Rc::new(record_partial::record_sites(&view.body, &app.models)),
        arg_model: arg_model.clone(),
        span_types: std::rc::Rc::new(record_value::span_types(&view.body)),
        record_shapes: std::rc::Rc::new(RecordShapes::build(app)),
    };

    let mut body_stmts: Vec<Expr> = Vec::new();
    body_stmts.push(assign_accumulator_string_new(&ctx.accumulator));
    body_stmts.extend(walk_template(&rewritten, &ctx));
    let mut result = var_ref(Symbol::from(ctx.accumulator.as_str()));
    result.set_hint(Some(IrHint::StringBuilderResult));
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
        include_spans: Vec::new(),
        methods: vec![method],
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        conditional_definitions: Vec::new(),
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
    let method_stem = match is_partial {
        true => partial_target(&format!("{dir}/{stem}"), dir, &partials.stems).1,
        false => stem.to_string(),
    };
    let method_name = Symbol::from(format!("{method_stem}_json"));

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
            let mut reads = crate::lower::view_to_library::view_read_ivars(&view.body);
            if reads_request_params(&view.body) && !reads.iter().any(|r| r.as_str() == "params") {
                reads.push(Symbol::from("params"));
            }
            reads
        })
    };

    // A PARTIAL's parameters are the locals its callers render it
    // with, then the ivars of its closure (`PartialParams`): Rails
    // binds a partial's locals by the names the call passes, never by
    // the partial's own file name, and a partial reads its renderer's
    // ivars.
    let local_params: Option<&Vec<Symbol>> = if is_partial {
        let (mod_path, method) = partial_target(&format!("{dir}/{stem}"), dir, &partials.stems);
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
    } else if !arg_name.is_empty() && is_partial {
        // The convention arg is a PARTIAL's fallback only. An action
        // view's render call passes exactly its ivar closure
        // (`action_view_ivar_map`), so a template whose action sets no
        // ivar (`def index; end` beside `json.ok true`) takes nothing:
        // a guessed `gizmos` parameter made the call an ArgumentError.
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
            let (mod_path, method) =
                partial_target(&format!("{dir}/{stem}"), dir, &partials.stems);
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
                } else if ivar_of(l).is_some() {
                    template_param_ty(&n, &known_models)
                } else {
                    crate::lower::view_to_library::ivar_ty(&n, &known_models)
                };
                (n, t)
            })
            .collect();
        crate::lower::view_to_library::build_view_signature_from(&typed, &extra_params)
    } else if ivar_params.is_empty() {
        let convention_arg = if is_partial { arg_name.as_str() } else { "" };
        build_view_signature(stem, dir, is_partial, convention_arg, &extra_params, &known_models)
    } else {
        let typed: Vec<(String, crate::ty::Ty)> = ivar_params
            .iter()
            .map(|iv| {
                let n = iv.as_str().to_string();
                let t = template_param_ty(&n, &known_models);
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
    let mut classes: crate::det::HashMap<ClassId, crate::analyze::ClassInfo> =
        crate::det::HashMap::default();
    insert_framework_stubs(&mut classes);
    let typer = crate::analyze::BodyTyper::new(&classes);
    let mut ctx = crate::analyze::Ctx::default();
    if let Some(crate::ty::Ty::Fn { params, .. }) = &method.signature {
        for (param, sig) in method.params.iter().zip(params.iter()) {
            ctx.local_bindings.insert(param.name.clone(), (*sig.ty).clone());
        }
    }
    if let Some(enclosing) = &method.enclosing_class {
        ctx.self_ty = Some(crate::ty::Ty::Class {
            id: ClassId(enclosing.clone()),
            args: vec![].into(),
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
    arg_columns: crate::det::HashMap<Symbol, crate::schema::ColumnType>,
    /// The keys Rails' `attributes` answers for the record `arg_name`
    /// names, in order: its model's table columns. `None` when the arg
    /// has no such model, or the model declares an attribute its table
    /// lacks (the attribute API), which `attributes` would also hold.
    arg_attributes: Option<Vec<String>>,
    /// Names declared by `direct :name do |…| … end`, without the
    /// `_path`/`_url` suffix. A direct helper's block parameter is
    /// whatever the CALLER hands it — campfire's `direct
    /// :fresh_user_avatar do |user, options|` reads `user.avatar_token`
    /// and `user.updated_at`, so it wants the RECORD — where a resource
    /// member helper wants the `:id` segment. `rewrite_path_arg_local`
    /// asks this before appending `.id`.
    direct_helpers: crate::det::HashSet<String>,
    /// Every model class the app declares. A route-helper argument
    /// whose last reader NAMES one is a record standing where an id
    /// belongs, and the app's own model list is the evidence — the
    /// alternative is the type, and the ruby emit lowers each jbuilder
    /// template SOLO, with only framework stubs registered, so
    /// `boost.message.room` has no type to ask.
    models: crate::det::HashSet<String>,
    /// Each of those models' column-type tables, for an element of a
    /// collection the template names by its model (`array!(widgets,
    /// :created_at)` reads `Widget`'s columns as `arg_columns` reads
    /// the template argument's).
    model_columns: std::rc::Rc<
        crate::det::HashMap<String, crate::det::HashMap<Symbol, crate::schema::ColumnType>>,
    >,
    /// Counter for the method's synthesized collection locals
    /// (`__col0`, `__col1`, …), shared by every clone of the template's
    /// `Ctx` so two collection blocks never bind the same name.
    temps: std::rc::Rc<std::cell::Cell<usize>>,
    /// Counter for the marks of keys that may be taken back
    /// (`io_key0`, …), shared the same way. Separate from `temps` so a
    /// template's collection locals keep their names.
    key_marks: std::rc::Rc<std::cell::Cell<usize>>,
    /// The literal `json.merge!` statements (by address) an object
    /// proved sound: top-level in their object, every sibling's keys
    /// known and none of them repeated (`mark_sound_merges`).
    sound_merges: std::rc::Rc<std::cell::RefCell<crate::det::HashSet<usize>>>,
    /// Every jbuilder partial's parameters, for the calls this template
    /// makes to them.
    partials: std::rc::Rc<PartialParams>,
    /// This template's own `partial_key` when it is a partial: the
    /// key its ivar parameters are under (`PartialParams::ivar_param`).
    partial_key: Option<(String, String)>,
    /// The template's path under app/views without extensions
    /// (`messages/_message`): what a `json.cache!` key names it by,
    /// where Rails names it by its digest path.
    template: String,
    /// The model of each `json.partial!(record, …)` record, keyed by
    /// the record's span (`record_partial::record_sites`), resolved on
    /// the analyzed template before its ivars become locals.
    record_sites: std::rc::Rc<record_partial::RecordSites>,
    /// The model `arg_name` is a record of, when known.
    arg_model: Option<String>,
    /// The analyzer's type of each expression of the template as
    /// written, by span (`record_value::span_types`): the walk reads a
    /// copy whose ivars are locals, which carries no types.
    span_types: std::rc::Rc<crate::det::HashMap<Span, crate::ty::Ty>>,
    /// The app's models as JSON values (`record_value`).
    record_shapes: std::rc::Rc<RecordShapes>,
}

/// Classification of a single top-level statement in a jbuilder
/// template. The walker normalizes recognized DSL Sends into this
/// closed enum; unknown shapes are tagged `Unknown`: refused when they
/// are calls on `json`, an empty append otherwise.
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
    /// where the app's models are known. `locals` are the names of the
    /// literal locals the call passes besides the record, which render
    /// the same output only while the partial never reads them
    /// (`record_partial_resolve`).
    PartialRecord { arg: &'a Expr, as_name: Option<Symbol>, locals: Vec<Symbol> },
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
    /// `json.array! col, :a, :b` — the same array, each element the
    /// object `json.extract!(element, :a, :b)` writes.
    ArrayExtract {
        collection: &'a Expr,
        attrs: Vec<Symbol>,
    },
    /// `Recv.call(args) do … end` on anything but `json`, whose block
    /// reaches `json` — a tenant scope, `I18n.with_locale`, a gem
    /// helper. The call stays as written; its block's pairs belong to
    /// the enclosing object, as the block runs with the same builder.
    Around { call: &'a Expr, body: &'a Expr },
    /// The same, but with a block the lowerer cannot rebuild (a block
    /// parameter named `json`, or a block argument that is not a
    /// literal block). Reported, never dropped.
    AroundUnlowered,
    /// `json.merge!(hash)` — the hash's pairs join this object. A
    /// literal Hash with literal keys is its pairs, written at compile
    /// time; any other value is walked when the template runs.
    Merge { value: &'a Expr },
    /// `json.extract!(obj, *attrs)` / `json.(obj, params[:f])`: an
    /// attribute list that is not all literal names, so the keys are
    /// not known when the template is lowered. Jbuilder writes one pair
    /// per name, whatever the list holds. Reported, never dropped.
    ExtractDynamic,
    /// `json.array! col, :a, fields: :b`: a Hash among the arguments that
    /// is not `partial:` + `as:` options. Jbuilder takes it for an
    /// attribute name and raises (`TypeError: {fields: :b} is not a
    /// symbol nor a string`). Reported, never dropped.
    ArrayHashArgument,
    /// Unrecognized DSL or non-Send statement. A call on `json` is
    /// refused where it stands; anything else surfaces as an empty io
    /// append so the lowered body stays well-formed.
    Unknown,
}

fn walk_template(body: &Expr, ctx: &Ctx) -> Vec<Expr> {
    if let Some(cached) = walk_cached_template(body, ctx) {
        return cached;
    }
    emit_object(&flatten_cache_blocks(stmts_of(body)), ctx)
}

/// A template that is ONE `json.cache! key do … end` around everything
/// it writes (campfire's message, boost and user partials): Jbuilder's
/// fragment cache, as `JbuilderTemplate#cache!` runs it.
///
/// ```ruby
/// if ActionView::ViewHelpers.jbuilder_caching?
///   k = ActionView::ViewHelpers.jbuilder_cache_key("messages/_message", message)
///   hit = ActionView::ViewHelpers.jbuilder_fragment_read(k)
///   if hit.nil?
///     cap = String.new
///     … the block's object, into cap …
///     io << ActionView::ViewHelpers.jbuilder_fragment_write(k, cap)
///   else
///     io << hit
///   end
/// else
///   … the block's object, into io …
/// end
/// ```
///
/// The cached value is the object's JSON text where Jbuilder caches the
/// object's Hash: the template writes nothing else, so the two are the
/// same fragment. The key is built only when the controller caches, as
/// Jbuilder builds it only then (an app's `combined_fragment_cache_key`
/// may read the request). A `cache!` among other statements, with
/// options, or nested, stays transparent (`flatten_cache_blocks`): the
/// block renders every time.
fn walk_cached_template(body: &Expr, ctx: &Ctx) -> Option<Vec<Expr>> {
    let stmts = stmts_of(body);
    let [only] = stmts.as_slice() else { return None };
    let ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. } = &*only.node else {
        return None;
    };
    if method.as_str() != "cache!" || !is_json_receiver(recv) {
        return None;
    }
    let [key] = args.as_slice() else { return None };
    if matches!(&*key.node, ExprNode::Hash { .. }) {
        return None;
    }
    let ExprNode::Lambda { body: inner, .. } = &*block.node else { return None };
    let inner_stmts = flatten_cache_blocks(stmts_of(inner));

    let n = ctx.temps.get();
    ctx.temps.set(n + 1);
    let key_var = Symbol::from(format!("__jb_key{n}"));
    let hit_var = Symbol::from(format!("__jb_hit{n}"));
    let cap = format!("__jb_io{n}");
    let cap_ctx = Ctx { accumulator: cap.clone(), ..ctx.clone() };

    let helpers = || const_path(&[Symbol::from("ActionView"), Symbol::from("ViewHelpers")]);
    let assign = |name: &Symbol, value: Expr| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign { target: LValue::Var { id: VarId(0), name: name.clone() }, value },
        )
    };
    let if_expr = |cond: Expr, then_branch: Expr, else_branch: Expr| {
        Expr::new(Span::synthetic(), ExprNode::If { cond, then_branch, else_branch })
    };

    let mut miss = vec![assign_accumulator_string_new(&cap)];
    miss.extend(emit_object(&inner_stmts, &cap_ctx));
    let mut cap_result = var_ref(Symbol::from(cap.as_str()));
    cap_result.set_hint(Some(IrHint::StringBuilderResult));
    miss.push(io_append_call(
        &ctx.accumulator,
        send(Some(helpers()), "jbuilder_fragment_write", vec![var_ref(key_var.clone()), cap_result], None, true),
    ));
    let cached = seq(vec![
        assign(
            &key_var,
            send(
                Some(helpers()),
                "jbuilder_cache_key",
                vec![lit_str(ctx.template.clone()), key.clone()],
                None,
                true,
            ),
        ),
        assign(
            &hit_var,
            send(Some(helpers()), "jbuilder_fragment_read", vec![var_ref(key_var.clone())], None, true),
        ),
        if_expr(
            send(Some(var_ref(hit_var.clone())), "nil?", Vec::new(), None, false),
            seq(miss),
            io_append_call(&ctx.accumulator, var_ref(hit_var.clone())),
        ),
    ]);
    let uncached = seq(emit_object(&inner_stmts, ctx));
    Some(vec![if_expr(
        send(Some(helpers()), "jbuilder_caching?", Vec::new(), None, false),
        cached,
        uncached,
    )])
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
    emit_object_filled(raw_stmts, ctx).0
}

/// `emit_object`, and whether every run of it writes at least one pair.
/// `false` when that is only known at run time: a pair under a
/// condition, a whole-template partial (whose own pairs may be
/// conditional), or no pair at all. A whole-template array is always a
/// value.
fn emit_object_filled(raw_stmts: &[&Expr], ctx: &Ctx) -> (Vec<Expr>, bool) {
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
        // An array is a value even when it is empty (`_array` sets
        // `[]`); a partial's object is empty when the partial may set
        // nothing.
        let filled = match only {
            JbStmt::ArrayPartial { .. } | JbStmt::ArrayBlock { .. } | JbStmt::ArrayExtract { .. } => true,
            JbStmt::Partial { partial_path, .. } => partial_always_filled(partial_path, ctx),
            _ => false,
        };
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
            JbStmt::ArrayExtract { collection, attrs } => Some(emit_array_extract(collection, attrs, ctx)),
            JbStmt::PartialRecord { arg, as_name, locals } => {
                record_partial_resolve(arg, as_name.as_ref(), locals, ctx)
                    .ok()
                    .map(|branches| {
                        let (mut stmts, call) = record_partial_call(arg, &branches, ctx);
                        stmts.push(io_append_call(&ctx.accumulator, call));
                        stmts
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
            return (out, filled);
        }
    }

    // Object form — wrap accumulated pair-emitting statements in
    // `{` … `}`. Comma is inserted between pairs; the lowerer knows
    // statically whether a pair is the first, except after a branch
    // that may or may not have emitted one (`Sep::Unknown`).
    let mut out: Vec<Expr> = Vec::new();
    out.push(io_append_lit(&ctx.accumulator, "{"));
    mark_sound_merges(&classified, raw_stmts, ctx);
    let end = emit_pairs(&classified, raw_stmts, ctx, &mut out, Sep::First);
    out.push(io_append_lit(&ctx.accumulator, "}"));
    // A statement the walker could not lower is refused or written as
    // nothing, so an empty object there says nothing about what
    // Jbuilder sets: such an object is left as it was, not taken for
    // BLANK.
    (out, end == Sep::After || has_unlowered(raw_stmts, ctx))
}

/// Whether an object's statements, or a branch of them, include one
/// the walker refuses or writes as an empty append (`JbStmt::Unknown`).
fn has_unlowered(stmts: &[&Expr], ctx: &Ctx) -> bool {
    stmts.iter().any(|s| match classify(s) {
        JbStmt::Unknown
        | JbStmt::ArrayPartial { .. }
        | JbStmt::ArrayBlock { .. }
        | JbStmt::ArrayExtract { .. } => true,
        JbStmt::PartialRecord { arg, as_name, locals } => {
            record_partial_resolve(arg, as_name.as_ref(), &locals, ctx).is_err()
        }
        JbStmt::Cond { then_branch, else_branch, .. } => {
            has_unlowered(&branch_stmts(then_branch), ctx) || has_unlowered(&branch_stmts(else_branch), ctx)
        }
        JbStmt::Guarded { body, rescues } => {
            has_unlowered(&branch_stmts(body), ctx)
                || rescues.iter().any(|r| has_unlowered(&branch_stmts(&r.body), ctx))
        }
        JbStmt::Around { body, .. } => has_unlowered(&branch_stmts(body), ctx),
        JbStmt::AroundUnlowered | JbStmt::ExtractDynamic | JbStmt::ArrayHashArgument => true,
        JbStmt::Merge { value } => !merge_lowers(s, value, ctx),
        _ => false,
    })
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

/// The pairs of a partial rendered inside an object, merged into it:
///
///   io_part0 = Views::Widgets.gadget_json(gadget)
///   if io_part0.length > 2
///     io << "," if !(io.end_with?("{"))
///     io << io_part0[1, io_part0.length - 2]
///   end
///
/// The partial's object without its braces is its pairs; a partial that
/// sets nothing renders `{}` and adds none. Answers the comma state
/// after it.
fn emit_merged_partial(call: Expr, ctx: &Ctx, out: &mut Vec<Expr>, sep: Sep) -> Sep {
    let n = ctx.key_marks.get();
    ctx.key_marks.set(n + 1);
    let part = Symbol::from(format!("{}_part{n}", ctx.accumulator));
    out.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign { target: LValue::Var { id: VarId(0), name: part.clone() }, value: call },
    ));
    let length = || send(Some(var_ref(part.clone())), "length", Vec::new(), None, false);
    let int = |value: i64| Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value } });
    let mut then = Vec::new();
    push_separator(&mut then, ctx, sep);
    let inner = send(
        Some(var_ref(part.clone())),
        "[]",
        vec![int(1), send(Some(length()), "-", vec![int(2)], None, false)],
        None,
        false,
    );
    then.push(io_append_call(&ctx.accumulator, inner));
    out.push(Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: send(Some(length()), ">", vec![int(2)], None, false),
            then_branch: seq(then),
            else_branch: seq(Vec::new()),
        },
    ));
    if sep == Sep::After { Sep::After } else { Sep::Unknown }
}

/// A statement the lowering cannot write, carrying the report: the
/// survey lists it and the Ruby emit raises it where it stood.
fn unsupported_stmt(span: Span, construct: &str, detail: &str) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Nil });
    e.set_diagnostic(Some(crate::diagnostic::DiagnosticKind::Unsupported {
        target: None,
        construct: Symbol::from(construct),
        detail: detail.to_string(),
    }));
    e
}

/// `io.length`.
fn io_length(ctx: &Ctx) -> Expr {
    send(Some(var_ref(Symbol::from(ctx.accumulator.as_str()))), "length", Vec::new(), None, false)
}

/// `io.slice!(mark, io.length) if io.end_with?("{}")`: drop what was
/// appended since `mark` when it ends in an empty object. Only the
/// object just written can end there: a non-empty object ends in a
/// value and its `}`, never in `{}`.
fn drop_blank_object(mark: Symbol, ctx: &Ctx) -> Expr {
    let io = || var_ref(Symbol::from(ctx.accumulator.as_str()));
    Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: send(Some(io()), "end_with?", vec![lit_str("{}".to_string())], None, true),
            then_branch: send(Some(io()), "slice!", vec![var_ref(mark), io_length(ctx)], None, true),
            else_branch: seq(Vec::new()),
        },
    )
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
        // `json.merge!(record.attributes…)` proved sound: the pairs
        // `extract!` writes for the same attributes.
        if let JbStmt::Merge { value } = stmt {
            if let Some((obj, attrs)) = attributes_merge(value, ctx) {
                if ctx.sound_merges.borrow().contains(&(*src as *const Expr as usize)) {
                    sep = emit_pairs(&[JbStmt::Extract { obj, attrs }], &[*src], ctx, out, sep);
                    continue;
                }
            }
        }
        // Synthesis choke point: everything pushed for this DSL
        // statement (key/comma appends, encode_value calls) attributes
        // back to it. The `{` / `}` wrappers belong to the template as
        // a whole and ride the method-level catch-all instead.
        let start = out.len();
        match stmt {
            JbStmt::Extract { obj, attrs } => {
                let obj_is_arg = obj_is_named_local(obj, &ctx.arg_name);
                let obj_model = record_model_of(obj, ctx);
                for attr in attrs {
                    push_separator(out, ctx, sep);
                    out.push(io_append_lit(
                        &ctx.accumulator,
                        &format!("\"{}\":", attr.as_str()),
                    ));
                    // An association: its record (or records) as
                    // `as_json` writes them.
                    if let Some(record) = obj_model.as_deref().and_then(|m| ctx.record_shapes.reader(m, attr)) {
                        let read = send(Some((*obj).clone()), attr.as_str(), Vec::new(), None, false);
                        out.extend(record_value_or_refusal(read, record.as_ref(), src.span, ctx));
                        sep = Sep::After;
                        continue;
                    }
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
                // A record, or records: as `as_json` writes them.
                if let Some(record) = ctx.span_types.get(&value.span).and_then(|t| ctx.record_shapes.of_ty(t)) {
                    let value = rewrite_h_escape(&rewrite_route_helpers(value, ctx));
                    out.extend(record_value_or_refusal(value, Some(&record), src.span, ctx));
                } else if let Some(record) = association_read(value, ctx) {
                    // An association reader the analyzer left untyped
                    // (a polymorphic one): its records as the reader
                    // on the owner's model answers them.
                    let value = rewrite_h_escape(&rewrite_route_helpers(value, ctx));
                    out.extend(record_value_or_refusal(value, record.as_ref(), src.span, ctx));
                } else {
                    let encoded = temporal_column_read(value, ctx)
                        .and_then(|(obj, col)| temporal_column_json(&obj, &col, ctx.arg_columns.get(&col)?))
                        .unwrap_or_else(|| {
                            json_builder_encode(rewrite_h_escape(&rewrite_route_helpers(value, ctx)))
                        });
                    out.push(io_append_call(&ctx.accumulator, encoded));
                }
                sep = Sep::After;
            }
            JbStmt::PairPartial { key, partial_path, arg, as_name } => {
                push_separator(out, ctx, sep);
                out.push(io_append_lit(
                    &ctx.accumulator,
                    &format!("\"{}\":", key.as_str()),
                ));
                let arg_ty = ctx.span_types.get(&arg.span);
                let arg = rewrite_h_escape(&rewrite_route_helpers(arg, ctx));
                // Jbuilder's `_set_inline_partial` answers `[]` for a
                // nil value without rendering the partial, whatever
                // the value's static type.
                if matches!(&*arg.node, ExprNode::Lit { value: Literal::Nil }) {
                    out.push(io_append_lit(&ctx.accumulator, "[]"));
                } else {
                    // The value is bound once to a `__val<n>` local unless it
                    // is a plain variable, so the nil check does not
                    // evaluate it twice.
                    let value = match &*arg.node {
                        ExprNode::Var { .. } | ExprNode::Ivar { .. } => arg,
                        _ => {
                            let n = ctx.temps.get();
                            ctx.temps.set(n + 1);
                            let val = Symbol::from(format!("__val{n}"));
                            out.push(Expr::new(
                                Span::synthetic(),
                                ExprNode::Assign { target: LValue::Var { id: VarId(0), name: val.clone() }, value: arg },
                            ));
                            var_ref(val)
                        }
                    };
                    let render = if arg_ty.is_some_and(is_collection_ty) {
                        // A collection: Jbuilder renders the partial once
                        // per element, into an array.
                        emit_array_partial(&value, partial_path, as_name, ctx)
                    } else {
                        emit_partial_call(partial_path, vec![(as_name.clone(), value.clone())], ctx)
                    };
                    out.push(Expr::new(
                        Span::synthetic(),
                        ExprNode::If {
                            cond: send(Some(value), "nil?", Vec::new(), None, false),
                            then_branch: io_append_lit(&ctx.accumulator, "[]"),
                            else_branch: seq(render),
                        },
                    ));
                }
                sep = Sep::After;
            }
            JbStmt::Nested { key, body } => {
                let (object, filled) =
                    emit_object_filled(&flatten_cache_blocks(stmts_of(body)), ctx);
                // Jbuilder leaves a key out when its block sets nothing
                // (`_merge_block` answers BLANK, `_set_value` skips it).
                // When the lowerer cannot tell that the block always
                // sets a pair, the key, its comma and its `{}` are
                // taken back at run time.
                let mark = (!filled).then(|| {
                    let n = ctx.key_marks.get();
                    ctx.key_marks.set(n + 1);
                    let mark = Symbol::from(format!("{}_key{n}", ctx.accumulator));
                    out.push(Expr::new(
                        Span::synthetic(),
                        ExprNode::Assign {
                            target: LValue::Var { id: VarId(0), name: mark.clone() },
                            value: io_length(ctx),
                        },
                    ));
                    mark
                });
                push_separator(out, ctx, sep);
                out.push(io_append_lit(
                    &ctx.accumulator,
                    &format!("\"{}\":", key.as_str()),
                ));
                out.extend(object);
                sep = match mark {
                    None => Sep::After,
                    Some(mark) => {
                        out.push(drop_blank_object(mark, ctx));
                        if sep == Sep::After { Sep::After } else { Sep::Unknown }
                    }
                };
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
            // `json.partial!` next to pairs, or under a condition: the
            // partial renders into the same object, so its pairs join
            // this one's.
            JbStmt::Partial { partial_path, locals } => {
                let locals = locals
                    .iter()
                    .map(|(k, v)| (k.clone(), rewrite_h_escape(&rewrite_route_helpers(v, ctx))))
                    .collect();
                sep = emit_merged_partial(partial_call(partial_path, locals, ctx), ctx, out, sep);
            }
            JbStmt::PartialRecord { arg, as_name, locals } => {
                match record_partial_resolve(arg, as_name.as_ref(), locals, ctx) {
                    Ok(branches) => {
                        let (stmts, call) = record_partial_call(arg, &branches, ctx);
                        out.extend(stmts);
                        sep = emit_merged_partial(call, ctx, out, sep);
                    }
                    // Refused visibly: rendering nothing here was a 200
                    // with the record's keys missing.
                    Err(why) => {
                        crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                            file: String::new(),
                            message: format!("jbuilder: `json.partial!` of a record whose partial is not statically known: {}", why.detail()),
                        });
                        out.push(unsupported_stmt(src.span, "jbuilder partial! of a record", &why.detail()));
                    }
                }
            }
            JbStmt::ArrayPartial { .. } | JbStmt::ArrayBlock { .. } | JbStmt::ArrayExtract { .. } => {
                // An array is not merged into an object (jbuilder raises
                // a MergeError once the object has a pair): drop a
                // marker rather than emit malformed JSON.
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
            JbStmt::Around { call, body } => {
                // The block may run any number of times: its first pair
                // takes a comma unless one is already known, and so
                // does the pair after the call.
                let start = if sep == Sep::After { Sep::After } else { Sep::Unknown };
                let stmts = branch_stmts(body);
                let classified: Vec<JbStmt<'_>> = stmts.iter().map(|s| classify(s)).collect();
                let mut appends = Vec::new();
                emit_pairs(&classified, &stmts, ctx, &mut appends, start);
                let mut call = (*call).clone();
                if let ExprNode::Send { block: Some(block), .. } = &mut *call.node {
                    if let ExprNode::Lambda { body, .. } = &mut *block.node {
                        *body = seq(appends);
                    }
                }
                out.push(call);
                sep = start;
            }
            JbStmt::AroundUnlowered => {
                crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                    file: String::new(),
                    message: "jbuilder: a block on a non-json call reaches `json` but cannot be lowered"
                        .to_string(),
                });
                out.push(unsupported_stmt(
                    src.span,
                    "jbuilder block on a non-json call",
                    "the block reaches `json` but its parameters or form cannot be lowered",
                ));
            }
            JbStmt::ExtractDynamic => {
                crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                    file: String::new(),
                    message: "jbuilder: `json.extract!`, `json.call` or `json.array!` with an attribute list that is not all literal names"
                        .to_string(),
                });
                out.push(unsupported_stmt(
                    src.span,
                    "jbuilder extract! with dynamic attributes",
                    "only literal attribute names are lowered; a splat, a param, an ivar or a constant names keys known only at run time",
                ));
            }
            JbStmt::ArrayHashArgument => {
                crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                    file: String::new(),
                    message: "jbuilder: `json.array!` with a Hash argument that is not `partial:` and `as:` options"
                        .to_string(),
                });
                out.push(unsupported_stmt(
                    src.span,
                    "jbuilder array! with a Hash that is not partial options",
                    "Jbuilder takes the Hash for an attribute name and raises TypeError; only `partial:` with `as:` is an option",
                ));
            }
            JbStmt::Merge { value } if merge_lowers(src, value, ctx) => {
                sep = emit_merge(value, ctx, out, sep);
            }
            JbStmt::Merge { .. } => {
                crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                    file: String::new(),
                    message: "jbuilder: `json.merge!` of a value that is not a Hash, or over a key the object may already set"
                        .to_string(),
                });
                out.push(unsupported_stmt(
                    src.span,
                    "jbuilder merge!",
                    "only a Hash literal whose keys the object sets nowhere else, or a value typed Hash, is lowered",
                ));
            }
            // A `json.<key>(…)` the classifier does not recognise. Writing
            // it as nothing dropped the key and never evaluated its
            // arguments, with no report: Jbuilder renders it or raises.
            // It is refused where it stands instead, as `ExtractDynamic`
            // is. A statement on any other receiver keeps the empty
            // append.
            JbStmt::Unknown if is_json_send(src) => {
                crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
                    file: String::new(),
                    message: "jbuilder: a `json.<key>(…)` or `json.set!` call whose arguments or block the lowering does not recognise"
                        .to_string(),
                });
                out.push(unsupported_stmt(
                    src.span,
                    "jbuilder call with unrecognised arguments",
                    "the key, its arguments or its block do not match a lowered shape (options that are not literals or that are only known at run time, a non-literal `set!` key, a block that names its own builder)",
                ));
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

/// The pairs of a literal `{ kind: "x", "n" => 2 }`: each key as the
/// String Jbuilder writes, with its value. `None` when the value is not
/// a Hash literal or a key is not a Symbol or String literal.
fn literal_merge_pairs(value: &Expr) -> Option<Vec<(String, &Expr)>> {
    let ExprNode::Hash { entries, .. } = &*value.node else { return None };
    entries
        .iter()
        .map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some((value.as_str().to_string(), v)),
            _ => string_literal(k).map(|key| (key, v)),
        })
        .collect()
}

/// `json.merge!(hash)`: each pair of `hash` appended to this object.
///
/// A literal Hash writes its pairs as `json.<key> <value>` would. Any
/// other value (an ivar, a method's answer) is iterated when the
/// template runs, every pair under the comma check:
///
///   (@extra).each do |merge_key, merge_value|
///     io << "," if !(io.end_with?("{"))
///     io << JsonBuilder.encode_value(merge_key.to_s)
///     io << ":"
///     io << JsonBuilder.encode_value(merge_value)
///   end
///
/// Only reached for a merge `merge_lowers` accepts. A typed Hash whose
/// key the object also sets is written twice: its keys are only known
/// when the template runs.
fn emit_merge(value: &Expr, ctx: &Ctx, out: &mut Vec<Expr>, mut sep: Sep) -> Sep {
    if let Some(pairs) = literal_merge_pairs(value) {
        for (key, v) in pairs {
            push_separator(out, ctx, sep);
            out.push(io_append_lit(&ctx.accumulator, &format!("{}:", json_string_literal(&key))));
            let encoded = json_builder_encode(rewrite_h_escape(&rewrite_route_helpers(v, ctx)));
            out.push(io_append_call(&ctx.accumulator, encoded));
            sep = Sep::After;
        }
        return sep;
    }
    let n = ctx.key_marks.get();
    ctx.key_marks.set(n + 1);
    let key_var = Symbol::from(format!("merge_key{n}"));
    let value_var = Symbol::from(format!("merge_value{n}"));
    let mut body = Vec::new();
    push_separator(&mut body, ctx, if sep == Sep::After { Sep::After } else { Sep::Unknown });
    let key_string = send(Some(var_ref(key_var.clone())), "to_s", Vec::new(), None, false);
    body.push(io_append_call(&ctx.accumulator, json_builder_encode(key_string)));
    body.push(io_append_lit(&ctx.accumulator, ":"));
    body.push(io_append_call(&ctx.accumulator, json_builder_encode(var_ref(value_var.clone()))));
    let block = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            extra_params: Vec::new(),
            params: vec![key_var, value_var],
            rest_param: None,
            block_param: None,
            body: seq(body),
            block_style: crate::expr::BlockStyle::Do,
        },
    );
    let value = rewrite_h_escape(&rewrite_route_helpers(value, ctx));
    out.push(send(Some(value), "each", Vec::new(), Some(block), false));
    // The hash may be empty: the next pair asks the accumulator, unless
    // one is already known to be there.
    if sep == Sep::After { Sep::After } else { Sep::Unknown }
}

/// `json.merge!(record.attributes)`, narrowed by any chain of
/// `except(...)` / `slice(...)` with literal keys: the record and the
/// attributes the merge writes, in the order Rails' Hash answers them.
/// `attributes` is the record's columns under String keys, in column
/// order (an enum column under its label), so the merge is the object
/// `json.extract!(record, *those)` builds. Only for the template's own
/// record, whose model's columns are known (`Ctx::arg_attributes`).
///
/// The Hash is String-keyed: a Symbol passed to `except` / `slice`
/// names no key, as on Rails' plain Hash. `slice` answers in its
/// arguments' order.
fn attributes_merge<'a>(value: &'a Expr, ctx: &Ctx) -> Option<(&'a Expr, Vec<Symbol>)> {
    let mut ops: Vec<(bool, Vec<Option<String>>)> = Vec::new();
    let mut cur = value;
    let record = loop {
        let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*cur.node else { return None };
        match method.as_str() {
            "except" | "slice" => {
                let keys = args
                    .iter()
                    .map(|a| match &*a.node {
                        ExprNode::Lit { value: Literal::Sym { .. } } => Some(None),
                        _ => string_literal(a).map(Some),
                    })
                    .collect::<Option<Vec<_>>>()?;
                ops.push((method.as_str() == "slice", keys));
                cur = recv;
            }
            "attributes" if args.is_empty() => break recv,
            _ => return None,
        }
    };
    if !obj_is_named_local(record, &ctx.arg_name) {
        return None;
    }
    let mut keys: Vec<String> = ctx.arg_attributes.clone()?;
    for (slice, names) in ops.into_iter().rev() {
        let names: Vec<String> = names.into_iter().flatten().collect();
        keys = if slice {
            let mut picked: Vec<String> = Vec::new();
            for n in names {
                if keys.contains(&n) && !picked.contains(&n) {
                    picked.push(n);
                }
            }
            picked
        } else {
            keys.into_iter().filter(|k| !names.contains(k)).collect()
        };
    }
    Some((record, keys.into_iter().map(Symbol::from).collect()))
}

/// Whether a `json.merge!` statement is lowered: a literal Hash its
/// object proved sound (`mark_sound_merges`), or any other value whose
/// type is a Hash. Anything else (an Array, which Jbuilder renders as
/// the whole value, an untyped value, a literal that repeats a key) is
/// refused, as Jbuilder's answer there is not a pair per entry.
fn merge_lowers(stmt: &Expr, value: &Expr, ctx: &Ctx) -> bool {
    if literal_merge_pairs(value).is_some() {
        return ctx.sound_merges.borrow().contains(&(stmt as *const Expr as usize));
    }
    !matches!(&*value.node, ExprNode::Hash { .. } | ExprNode::Array { .. })
        && matches!(value.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
}

/// The keys every run of `stmt` may set in its object, when they are
/// all known: a pair's key, `extract!`'s attributes, a literal merge's
/// keys, both branches of a condition. `None` for a partial, a block
/// on another call, a merge of a non-literal, or anything unknown.
fn object_keys(stmt: &JbStmt<'_>, ctx: &Ctx) -> Option<Vec<String>> {
    let branch = |body: &Expr| -> Option<Vec<String>> {
        let mut keys = Vec::new();
        for s in branch_stmts(body) {
            keys.extend(object_keys(&classify(s), ctx)?);
        }
        Some(keys)
    };
    match stmt {
        JbStmt::Extract { attrs, .. } => Some(attrs.iter().map(|a| a.as_str().to_string()).collect()),
        JbStmt::Pair { key, .. }
        | JbStmt::PairPartial { key, .. }
        | JbStmt::Nested { key, .. }
        | JbStmt::PairBlock { key, .. } => Some(vec![key.as_str().to_string()]),
        JbStmt::Merge { value } => match literal_merge_pairs(value) {
            Some(pairs) => Some(pairs.into_iter().map(|(k, _)| k).collect()),
            None => attributes_merge(value, ctx).map(|(_, attrs)| attrs.iter().map(|a| a.as_str().to_string()).collect()),
        },
        // Refused: the statement lowers to a raise, so a run that
        // reaches it renders nothing and sets no key in this object.
        JbStmt::Local | JbStmt::ExtractDynamic => Some(Vec::new()),
        JbStmt::Cond { then_branch, else_branch, .. } => {
            let mut keys = branch(then_branch)?;
            keys.extend(branch(else_branch)?);
            Some(keys)
        }
        _ => None,
    }
}

/// Record which of an object's own (top-level) literal `json.merge!`
/// statements are sound: their keys are distinct, every other statement
/// of the object sets only known keys, and none of those is one of
/// them. Jbuilder deep-merges a repeated String key and Rails answers
/// 500 for a repeated Symbol one, so a repeat is never a second pair.
fn mark_sound_merges(classified: &[JbStmt<'_>], raw_stmts: &[&Expr], ctx: &Ctx) {
    for (i, stmt) in classified.iter().enumerate() {
        let JbStmt::Merge { value } = stmt else { continue };
        let own = match literal_merge_pairs(value) {
            Some(pairs) => pairs.into_iter().map(|(k, _)| k).collect::<Vec<_>>(),
            None => match attributes_merge(value, ctx) {
                Some((_, attrs)) => attrs.iter().map(|a| a.as_str().to_string()).collect(),
                None => continue,
            },
        };
        let distinct: crate::det::HashSet<&String> = own.iter().collect();
        if distinct.len() != own.len() {
            continue;
        }
        let mut others: Vec<String> = Vec::new();
        let known = classified.iter().enumerate().filter(|(j, _)| *j != i).all(|(_, c)| match object_keys(c, ctx) {
            Some(keys) => {
                others.extend(keys);
                true
            }
            None => false,
        });
        if known && !others.iter().any(|k| distinct.contains(k)) {
            ctx.sound_merges.borrow_mut().insert(raw_stmts[i] as *const Expr as usize);
        }
    }
}

/// `"key"` as a JSON string literal (quotes and backslashes escaped).
fn json_string_literal(key: &str) -> String {
    serde_json::to_string(key).unwrap_or_else(|_| format!("\"{key}\""))
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
    if let ExprNode::Send { recv, block: Some(block), .. } = &*stmt.node {
        // A block parameter named `json` shadows the builder: the
        // block's `json.` calls go to whatever the call yields, which
        // the lowering cannot follow.
        let binds_json = matches!(&*block.node, ExprNode::Lambda { params, .. }
            if params.iter().any(|p| p.as_str() == "json"));
        if !recv.as_ref().is_some_and(is_json_receiver) && (binds_json || mentions_json(block)) {
            return match &*block.node {
                ExprNode::Lambda { params, rest_param: None, body, .. }
                    if !params.iter().any(|p| p.as_str() == "json") =>
                {
                    JbStmt::Around { call: stmt, body }
                }
                _ => JbStmt::AroundUnlowered,
            };
        }
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
            // Silently writing nothing for this statement is a 200 with
            // keys missing; a list that is not all literal names is
            // refused instead.
            let Some(attrs) = literal_attribute_names(rest) else {
                return JbStmt::ExtractDynamic;
            };
            JbStmt::Extract { obj, attrs }
        }
        // `json.merge!(hash)`: Jbuilder merges the hash's pairs into
        // the object being built (`_merge_values`). Without this arm
        // the name fell through to the single-pair shape below and the
        // template wrote a key named `merge!`.
        "merge!" if args.len() == 1 && block.is_none() => JbStmt::Merge { value: &args[0] },
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
                    return JbStmt::ArrayBlock { collection, item_var, body };
                }
            }
            // `json.array! col, :a, :b` — no options, attribute names:
            // Jbuilder extracts them from each element
            // (`_map_collection` over `extract!`).
            let rest = &args[1..];
            // JbuilderTemplate#array! reads its FIRST extra argument as
            // options only when it holds both `partial:` and `as:`
            // (`_partial_options?`); any other Hash is an attribute name
            // to `_extract`, which raises. Refused rather than dropped.
            let partial_options = rest.first().and_then(|a| extract_hash(a)).is_some_and(|opts| {
                hash_get_value(opts, "partial").is_some() && hash_get_value(opts, "as").is_some()
            });
            if block.is_none() && !partial_options && rest.iter().any(|a| extract_hash(a).is_some()) {
                return JbStmt::ArrayHashArgument;
            }
            if block.is_none() && !rest.is_empty() && !rest.iter().any(|a| extract_hash(a).is_some()) {
                return match literal_attribute_names(rest) {
                    Some(attrs) => JbStmt::ArrayExtract { collection, attrs },
                    None => JbStmt::ExtractDynamic,
                };
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
            // `partial:` and `as:` after it, as literals, do not change
            // that:
            // jbuilder's `partial!` sets `options[:partial]` to the
            // positional, over the option, and `as:` only names the
            // local (`record_partial_path` checks it is the name the
            // lowered partial takes the record under). `cached:` is a
            // rendering option Jbuilder strips from the locals with the
            // other two; for one record it only turns on fragment
            // caching (Action View's collection renderer is the only
            // one that reads it), so it renders the same partial. Ruby
            // evaluates any of the three before the render, so one that
            // is not a literal stays Unknown, as in the keyed form.
            //
            // Every other key is a local for the partial (`cache: true`
            // is one: Jbuilder has no such option), unless Action View
            // reads it as a render option too (`layout:`, `formats:`,
            // `locals:`, …), which changes what is rendered and stays
            // Unknown. A local with a literal value is kept by name: the
            // call renders the same output while the partial never
            // reads it, which `record_partial_resolve` checks. A value
            // that is not a literal is evaluated by Ruby before the
            // render, so that form stays Unknown.
            if block.is_none() && record_partial::is_record_position(path_arg) {
                let mut locals = Vec::new();
                let options_ok = match args.len() {
                    1 => true,
                    2 => extract_hash(&args[1]).is_some_and(|opts| {
                        opts.iter().all(|(k, v)| match &*k.node {
                            ExprNode::Lit { value: Literal::Sym { value } } => match value.as_str() {
                                "partial" | "as" | "cached" => is_literal_value(v),
                                key if ACTION_VIEW_RENDER_OPTIONS.contains(&key) => false,
                                _ => {
                                    let literal = matches!(
                                        &*v.node,
                                        ExprNode::Lit {
                                            value: Literal::Nil
                                                | Literal::Bool { .. }
                                                | Literal::Int { .. }
                                                | Literal::Float { .. }
                                                | Literal::Str { .. }
                                                | Literal::Sym { .. }
                                        }
                                    );
                                    locals.push(value.clone());
                                    literal
                                }
                            },
                            _ => false,
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
                return JbStmt::PartialRecord { arg: path_arg, as_name, locals };
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
                return match partial_locals(opts, &["partial", "as", "collection", "cached"])
                    .and_then(|locals| bind_object_local(locals, opts, &partial_path))
                {
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
                    match partial_locals(opts, &["as"])
                        .and_then(|locals| bind_object_local(locals, opts, &partial_path))
                    {
                        Some(locals) => locals,
                        None => return JbStmt::Unknown,
                    }
                }
                Some(_) => return JbStmt::Unknown,
            };
            JbStmt::Partial { partial_path, locals }
        }
        // `json.set!(K, *rest, &blk)` with a literal key is
        // `json.K(*rest, &blk)`: jbuilder 2.15.1 sends any method the
        // builder does not define to `set!`, key first, so the two
        // spellings take the same path with the same arguments. Only
        // the generic key shapes below apply: `set!(:array!, x)` writes
        // a key named `array!`, it is not `json.array!`.
        //
        // With no value, a block that names a parameter is given the
        // builder itself (`_merge_block` yields `self`), and its calls
        // go through that parameter rather than `json`, which the
        // lowering cannot follow: Unknown, and so refused. A key that
        // is not a literal is only known at run time: Unknown too.
        "set!" => {
            let Some((key, rest)) = args.split_first() else {
                return JbStmt::Unknown;
            };
            let Some(key) = set_bang_key(key) else {
                return JbStmt::Unknown;
            };
            if rest.is_empty() && block_names_a_parameter(block) {
                return JbStmt::Unknown;
            }
            classify_key(&key, rest, block)
        }
        key => classify_key(key, args, block),
    }
}

/// The generic `json.<key>(…)` shapes, where the method name (or the
/// literal key of a `set!`) is the JSON key.
fn classify_key<'a>(key: &str, args: &'a [Expr], block: &'a Option<Expr>) -> JbStmt<'a> {
    match key {
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
        //
        // Arguments after the collection do not change that: `set!`
        // takes them as `*args` and, with a block, never reads them
        // (`json.items(col, cache: true) { … }` is the same array). The
        // one exception is JbuilderTemplate's own `set!`, which renders
        // an inline partial before the block is considered when the
        // first argument after the collection is a `partial:` + `as:`
        // Hash, whatever follows it (jbuilder 2.15.1:
        // `_partial_options?(args.first)`, i.e. `Hash === options &&
        // options.key?(:as) && options.key?(:partial)`, so the keys'
        // presence, whatever their values); that shape stays below.
        //
        // Jbuilder still evaluates those arguments before `set!` drops
        // them, so the arm only takes them when they are literals: a
        // call there (`cache: raise("bad")`, `cache: key_for(x)`) has
        // effects the lowered array would lose.
        key if !args.is_empty()
            && block.is_some()
            && args[1..].iter().all(is_literal_value)
            && !args
                .get(1)
                .and_then(extract_hash)
                .is_some_and(|opts| hash_has_key(opts, "partial") && hash_has_key(opts, "as")) =>
        {
            let Some((item_var, body)) = item_block(block) else {
                return JbStmt::Unknown;
            };
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
        // record, so it emits an object where `array!` emits an array.
        // A positional that is a collection renders one partial per
        // element (jbuilder 2.15.1 `_set_inline_partial`); the walker
        // picks that by the positional's type.
        //
        // The call passes the partial only its record. Any other key is
        // a local Jbuilder hands the partial, which the call has no way
        // to pass, and `cached:` is evaluated before Jbuilder renders:
        // the shape is taken only with `partial:`, `as:` and `cached:`,
        // all literals, and is refused otherwise.
        key if args.len() == 2 => {
            let arg = &args[0];
            let Some(opts) = extract_hash(&args[1]) else {
                return JbStmt::Unknown;
            };
            let options_ok = opts.iter().all(|(k, v)| {
                matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                    if matches!(value.as_str(), "partial" | "as" | "cached"))
                    && is_literal_value(v)
            });
            if !options_ok {
                return JbStmt::Unknown;
            }
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

/// The key of `json.set!(key, …)` as Jbuilder writes it, when it is a
/// Symbol or String literal whose text needs no escaping in JSON (the
/// walker writes a key as it stands). `None` for anything else, which
/// the caller refuses.
fn set_bang_key(e: &Expr) -> Option<String> {
    let key = match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
        _ => string_literal(e)?,
    };
    let plain = !key.is_empty()
        && key.chars().all(|c| !c.is_control() && !matches!(c, '"' | '\\' | '<' | '>' | '&' | '\u{2028}' | '\u{2029}'));
    plain.then_some(key)
}

/// Whether a block names any parameter (`{ |g| … }`, `{ |*a| … }`).
fn block_names_a_parameter(block: &Option<Expr>) -> bool {
    matches!(block.as_ref().map(|b| &*b.node), Some(ExprNode::Lambda { params, rest_param, extra_params, block_param, .. })
        if !params.is_empty() || rest_param.is_some() || !extra_params.is_empty() || block_param.is_some())
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

/// `json` parsed as a bare method call: `Send { recv: None, method:
/// "json", args: [] }`. Anything else fails the discriminator.
/// Whether `e` reads the template's builder anywhere: a `json.<x>`
/// call or a bare `json`.
fn mentions_json(e: &Expr) -> bool {
    if let ExprNode::Send { recv: None, method, args, .. } = &*e.node {
        if method.as_str() == "json" && args.is_empty() {
            return true;
        }
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found = found || mentions_json(c));
    found
}

/// The names of an `extract!`/`array!` attribute list, when every one is
/// a literal: a Symbol, or a String, which Jbuilder writes as given just
/// the same. A String is taken only when it is a method name the
/// lowered `record.<name>` read can spell. `None` for anything else.
fn literal_attribute_names(args: &[Expr]) -> Option<Vec<Symbol>> {
    args.iter()
        .map(|a| match &*a.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
            ExprNode::Lit { value: Literal::Str { value } } if is_plain_method_name(value) => {
                Some(Symbol::from(value.as_str()))
            }
            _ => None,
        })
        .collect()
}

fn is_plain_method_name(name: &str) -> bool {
    let body = name.strip_suffix(['?', '!']).unwrap_or(name);
    let mut chars = body.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_ascii_lowercase())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// `json.array! col, :a, :b`: `emit_array_block` over the element
/// block `{ |__item| json.extract!(__item, :a, :b) }`, which is what
/// Jbuilder runs for each element.
///
/// Only over a collection whose elements are known to be records: one
/// the template names by a model (`widgets`, typed `Array[Widget]` as
/// the template's parameter is). The element then reads its temporal
/// columns from their storage text, as the template argument's do.
/// Jbuilder reads a Hash element with `fetch` instead, and a collection
/// of anything else is not known to answer `element.a`, so any other
/// collection is refused rather than lowered to a read that raises.
fn emit_array_extract(collection: &Expr, attrs: &[Symbol], ctx: &Ctx) -> Vec<Expr> {
    let models: Vec<String> = ctx.models.iter().cloned().collect();
    let model = match &*collection.node {
        ExprNode::Var { name, .. } => Some(name.as_str()),
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            Some(method.as_str())
        }
        _ => None,
    }
    .and_then(|name| match crate::lower::view_to_library::ivar_ty(name, &models) {
        crate::ty::Ty::Array { elem } => match &*elem {
            crate::ty::Ty::Class { id, .. } => Some(id.0.as_str().to_string()),
            _ => None,
        },
        _ => None,
    });
    let Some(model) = model else {
        crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
            file: String::new(),
            message: "jbuilder: `json.array!` with attribute names over a collection whose elements are not known to be records"
                .to_string(),
        });
        return vec![unsupported_stmt(
            collection.span,
            "jbuilder array! with attribute names over an untyped collection",
            "only a collection named by its model (`@widgets`) is lowered; Jbuilder reads a Hash element with fetch and other elements by method",
        )];
    };
    let item = Symbol::from("__item");
    let mut args = vec![var_ref(item.clone())];
    args.extend(attrs.iter().map(|a| {
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: a.clone() } })
    }));
    let json = send(None, "json", Vec::new(), None, false);
    let body = send(Some(json), "extract!", args, None, true);
    let mut inner = ctx.clone();
    inner.arg_name = item.as_str().to_string();
    inner.arg_columns = ctx.model_columns.get(&model).cloned().unwrap_or_default();
    inner.arg_model = Some(model);
    emit_array_block(collection, &item, &body, &inner)
}

/// The records `value` reads when it is an association reader on a
/// record of a known model (`@tag.thing`): `Some(None)` for a reader
/// whose class the row decides and the app gives no closed set of.
fn association_read(value: &Expr, ctx: &Ctx) -> Option<Option<RecordValue>> {
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*value.node else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let owner = record_model_of(recv, ctx)?;
    ctx.record_shapes.reader(&owner, method)
}

/// The model of the record `obj` is: its static type, else the
/// template argument's model when `obj` is that argument.
fn record_model_of(obj: &Expr, ctx: &Ctx) -> Option<String> {
    match ctx.span_types.get(&obj.span).and_then(|t| ctx.record_shapes.of_ty(t)) {
        Some(RecordValue::One(m)) => Some(m),
        Some(RecordValue::Many(_) | RecordValue::OneOf(_)) => None,
        None => ctx.arg_model.clone().filter(|_| obj_is_named_local(obj, &ctx.arg_name)),
    }
}

/// The JSON of `value`, a record or records of one model, as Active
/// Record's `as_json` writes it: the object `json.extract!(record,
/// *attributes)` builds (`null` for a nil record), or an array of
/// them. A record of a closed set of models is written by a switch on
/// its class, each branch its model's object, raising for a class
/// outside the set. Refused, with the reason, when a model's `as_json`
/// is not its columns or the record's class is not known (`record` is
/// None: a polymorphic reader with no recorded targets).
fn record_value_or_refusal(value: Expr, record: Option<&RecordValue>, span: Span, ctx: &Ctx) -> Vec<Expr> {
    let refuse = |why: &str| {
        crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
            file: String::new(),
            message: format!("jbuilder: a record as a value, which Jbuilder encodes with `as_json`: {why}"),
        });
        vec![unsupported_stmt(span, "jbuilder record value", why)]
    };
    let Some(record) = record else {
        return refuse("a polymorphic association's record is of the class its row names, and the app declares no closed set of them");
    };
    // `json.extract!(<var>, *attributes)` for `model`, with the context
    // that writes it.
    let extract = |var: &Symbol, model: &str| -> Result<(Expr, Ctx), &'static str> {
        let attrs = ctx.record_shapes.attributes(model)?;
        let mut args = vec![var_ref(var.clone())];
        args.extend(attrs.iter().map(|a| {
            Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: a.clone() } })
        }));
        let json = send(None, "json", Vec::new(), None, false);
        let mut inner = ctx.clone();
        inner.arg_name = var.as_str().to_string();
        inner.arg_columns = ctx.model_columns.get(model).cloned().unwrap_or_default();
        inner.arg_model = Some(model.to_string());
        Ok((send(Some(json), "extract!", args, None, true), inner))
    };
    let models: Vec<&str> = match record {
        RecordValue::Many(model) => {
            let item = Symbol::from("__item");
            return match extract(&item, model) {
                Ok((stmt, inner)) => emit_array_block(&value, &item, &stmt, &inner),
                Err(why) => refuse(why),
            };
        }
        RecordValue::One(model) => vec![model.as_str()],
        RecordValue::OneOf(models) => models.iter().map(String::as_str).collect(),
    };
    let n = ctx.temps.get();
    ctx.temps.set(n + 1);
    let rec = Symbol::from(format!("__rec{n}"));
    let object = |model: &str| -> Result<Expr, &'static str> {
        let (stmt, inner) = extract(&rec, model)?;
        Ok(seq(emit_object_filled(&[&stmt], &inner).0))
    };
    let mut branch = match record {
        // One model: its object, the class unchecked as before.
        RecordValue::One(model) => match object(model) {
            Ok(o) => o,
            Err(why) => return refuse(why),
        },
        _ => {
            let mut switch = raise_outside_set("jbuilder record value", &models);
            for model in models.iter().rev() {
                match object(model) {
                    Ok(o) => switch = is_a_branch(&rec, model, o, switch),
                    Err(why) => return refuse(why),
                }
            }
            switch
        }
    };
    branch = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: send(Some(var_ref(rec.clone())), "nil?", Vec::new(), None, false),
            then_branch: io_append_lit(&ctx.accumulator, "null"),
            else_branch: branch,
        },
    );
    let bind = Expr::new(
        Span::synthetic(),
        ExprNode::Assign { target: LValue::Var { id: VarId(0), name: rec }, value },
    );
    vec![bind, branch]
}

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

/// Whether a value of type `ty` is always a collection: an Array, a
/// Relation or an association collection, never nil.
fn is_collection_ty(ty: &crate::ty::Ty) -> bool {
    use crate::ty::Ty;
    match ty {
        Ty::Array { .. } | Ty::Relation { .. } => true,
        Ty::Class { id, .. } => id.0.as_str().ends_with("Relation") || id.0.as_str().ends_with("CollectionProxy"),
        _ => false,
    }
}

/// Whether `stmt` is a call on the template's builder, `json.<m>(…)`.
fn is_json_send(stmt: &Expr) -> bool {
    matches!(&*stmt.node, ExprNode::Send { recv: Some(recv), .. } if is_json_receiver(recv))
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

/// A literal, or an Array/Hash of literals: evaluating it has no
/// effect, so dropping it unevaluated changes nothing.
fn is_literal_value(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Lit { .. } => true,
        ExprNode::Array { elements, .. } => elements.iter().all(is_literal_value),
        ExprNode::Hash { entries, .. } => entries
            .iter()
            .all(|(k, v)| is_literal_value(k) && is_literal_value(v)),
        _ => false,
    }
}

/// Whether the Hash has a Symbol key `key`, whatever its value.
fn hash_has_key(entries: &[(Expr, Expr)], key: &str) -> bool {
    hash_get_value(entries, key).is_some()
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

/// The locals of a one-partial render with an `object:` option: Action
/// View also binds the object under `as:`, or under the partial's own
/// name (`gadget` for `gadgets/gadget`), as it binds the record of
/// `json.partial!(record)`. `object` stays a local too
/// (`local_assigns[:object]` is set in Rails). None when `as:` is not
/// a literal name.
fn bind_object_local<'a>(
    mut locals: Vec<(Symbol, &'a Expr)>,
    opts: &'a [(Expr, Expr)],
    partial_path: &str,
) -> Option<Vec<(Symbol, &'a Expr)>> {
    let Some(object) = hash_get_value(opts, "object") else {
        return Some(locals);
    };
    let name = match hash_get_value(opts, "as") {
        None => {
            let base = partial_path.rsplit('/').next().unwrap_or(partial_path);
            Symbol::from(base.trim_start_matches('_'))
        }
        Some(v) => match &*v.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.clone(),
            _ => Symbol::from(string_literal(v)?.as_str()),
        },
    };
    if !locals.iter().any(|(k, _)| *k == name) {
        locals.push((name, object));
    }
    Some(locals)
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
    // Whether every element sets a pair: a partial may set nothing,
    // unless its template always does.
    let filled;
    let element = match single_partial {
        Some((partial_path, locals)) => {
            filled = partial_always_filled(&partial_path, ctx);
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
            let (object, always) = emit_object_filled(&stmts, &inner);
            filled = always;
            exprs.extend(object);
            let mut result = var_ref(Symbol::from(inner.accumulator.as_str()));
            result.set_hint(Some(IrHint::StringBuilderResult));
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
    let mut mapped = send(Some(var_ref(col.clone())), "map", Vec::new(), Some(block), false);
    // Jbuilder drops an element whose block sets nothing
    // (`_map_collection` deletes BLANK): `.reject { |e| e == "{}" }`
    // when an element may be empty. Only an empty object is `{}`.
    if !filled {
        let el = Symbol::from("__el");
        let is_blank = send(Some(var_ref(el.clone())), "==", vec![lit_str("{}".to_string())], None, false);
        let reject = Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                extra_params: Vec::new(),
                rest_param: None,
                params: vec![el],
                block_param: None,
                body: is_blank,
                block_style: crate::expr::BlockStyle::Brace,
            },
        );
        mapped = send(Some(mapped), "reject", Vec::new(), Some(reject), false);
    }
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

/// The partial path `json.partial!(record, …)` renders: Active Model's
/// `to_partial_path`, `"<plural>/<singular>"` of the record's model
/// (`widgets/widget` for a `Widget`), under the namespace of the
/// template's own directory (`admin/widgets/widget` from
/// `admin/widgets/`), which is Action View's
/// `prefix_partial_path_with_controller_namespace` default. The model
/// is the record's static type (`record_partial::record_sites`), not
/// the name of the local that holds it. Unresolved when that type is
/// not one model, or when `as:` names the partial's local something
/// other than the model's singular: the lowered partial takes its
/// record under that singular, and a partial reading the `as:` name
/// would not find it.
///
/// A record of a closed set of models (a union's members, a
/// polymorphic reader's targets) has one `(path, local, model)` per
/// member, which `record_partial_call` switches over.
fn record_partial_path(
    arg: &Expr,
    as_name: Option<&Symbol>,
    resource_dir: &str,
    sites: &record_partial::RecordSites,
) -> Result<RecordBranches, Unresolved> {
    let record = sites.get(&arg.span).cloned().unwrap_or(Err(Unresolved::Unknown))?;
    let branches = record
        .names
        .into_iter()
        .map(|model| {
            let (path, element) = record_partial::model_partial_path(&model, resource_dir);
            if let Some(a) = as_name.filter(|a| **a != element) {
                return Err(Unresolved::As { model, as_name: a.clone() });
            }
            Ok((path, element, model))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RecordBranches { branches, switched: record.switched })
}

/// The keys of a `render` options Hash that Action View reads itself
/// (the renderers' options, and the lookup details `extract_details`
/// takes): passed to `json.partial!` they are locals too, but they
/// also change which template renders and how, so a call with one is
/// not lowered as a plain record partial.
const ACTION_VIEW_RENDER_OPTIONS: &[&str] = &[
    "action", "assigns", "body", "collection", "content_type", "file", "formats", "handlers", "html",
    "inline", "json", "layout", "locale", "locals", "object", "plain", "prefixes", "renderable",
    "spacer_template", "status", "template", "type", "variant", "variants",
];

/// Names that let a template read a local it does not name: a
/// partial mentioning one may read any local a call passes.
const READS_ANY_LOCAL: &[&str] = &["local_assigns", "binding", "eval", "local_variables"];

/// `record_partial_path`, and the check that each partial it renders
/// never reads the literal `locals` the call passes besides the record.
/// Rails binds them, the lowered call does not; a partial that never
/// names them renders the same either way. One that names one (or
/// reads its locals through `local_assigns`) depends on it, and the
/// lowered partial has no parameter for it.
fn record_partial_resolve(
    arg: &Expr,
    as_name: Option<&Symbol>,
    locals: &[Symbol],
    ctx: &Ctx,
) -> Result<RecordBranches, Unresolved> {
    let record = record_partial_path(arg, as_name, &ctx.resource_dir, &ctx.record_sites)?;
    for (path, _, _) in &record.branches {
        let (mod_path, method) = partial_target(path, &ctx.resource_dir, &ctx.partials.stems);
        let names = ctx.partials.names.get(&partial_key(&mod_path, &method));
        for local in locals {
            let read = names.is_none_or(|names| {
                names.contains(local) || READS_ANY_LOCAL.iter().any(|n| names.contains(&Symbol::from(*n)))
            });
            if read {
                return Err(Unresolved::ReadsLocal { partial: path.clone(), local: local.clone() });
            }
        }
    }
    Ok(record)
}

/// The `(path, local, model)` of each model a record may be, and
/// whether its class is switched on.
struct RecordBranches {
    branches: Vec<(String, Symbol, String)>,
    switched: bool,
}

/// The statements and the call that render `arg`'s partial. One model:
/// its partial's call. A closed set: the record bound once and a switch
/// on its class, each branch calling its model's partial, and a raise
/// for a record of any other class, whose partial was not lowered:
///
///   __rec0 = <arg>
///   __part0 = ""
///   if __rec0.is_a?(Widget) then __part0 = Views::Widgets.widget_json(__rec0)
///   elsif __rec0.is_a?(Gadget) then __part0 = Views::Gadgets.gadget_json(__rec0)
///   else raise("…") end
fn record_partial_call(arg: &Expr, record: &RecordBranches, ctx: &Ctx) -> (Vec<Expr>, Expr) {
    let branches = &record.branches;
    if let ([(path, local, _)], false) = (branches.as_slice(), record.switched) {
        return (Vec::new(), partial_call(path, vec![(local.clone(), arg.clone())], ctx));
    }
    let n = ctx.temps.get();
    ctx.temps.set(n + 1);
    let rec = Symbol::from(format!("__rec{n}"));
    let part = Symbol::from(format!("__part{n}"));
    let assign = |name: &Symbol, value: Expr| {
        Expr::new(Span::synthetic(), ExprNode::Assign { target: LValue::Var { id: VarId(0), name: name.clone() }, value })
    };
    let models: Vec<&str> = branches.iter().map(|(_, _, m)| m.as_str()).collect();
    let mut switch = raise_outside_set("jbuilder partial! of a record", &models);
    for (path, local, model) in branches.iter().rev() {
        let call = partial_call(path, vec![(local.clone(), var_ref(rec.clone()))], ctx);
        switch = is_a_branch(&rec, model, assign(&part, call), switch);
    }
    let stmts = vec![assign(&rec, arg.clone()), assign(&part, lit_str(String::new())), switch];
    (stmts, var_ref(part))
}

/// `if <rec>.is_a?(<Model>) then <then> else <otherwise> end`.
fn is_a_branch(rec: &Symbol, model: &str, then: Expr, otherwise: Expr) -> Expr {
    let class = const_path(&model.split("::").map(Symbol::from).collect::<Vec<_>>());
    Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: send(Some(var_ref(rec.clone())), "is_a?", vec![class], None, true),
            then_branch: then,
            else_branch: otherwise,
        },
    )
}

/// The `else` of a switch over a closed set of models: a record of a
/// class outside it raises rather than render a shape nobody lowered.
fn raise_outside_set(construct: &str, models: &[&str]) -> Expr {
    let message = format!("roundhouse: {construct} of a class other than {}", models.join(", "));
    send(None, "raise", vec![lit_str(message)], None, true)
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
    let (mod_path, method) = partial_target(partial_path, &ctx.resource_dir, &ctx.partials.stems);
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
    params: crate::det::HashMap<(String, String), Vec<Symbol>>,
    /// Each partial's ivar parameters: the ivar, then its parameter.
    ivars: crate::det::HashMap<(String, String), Vec<(Symbol, Symbol)>>,
    closures: crate::det::HashMap<Symbol, Vec<Symbol>>,
    /// The stem each partial's key and `<stem>_json` method go by.
    stems: PartialStems,
    /// The partials whose every render is a non-empty value: an object
    /// with a pair on every path, or an array. A render of any other
    /// partial may come out `{}`, which Jbuilder treats as BLANK.
    filled: crate::det::HashSet<(String, String)>,
    /// Every name each partial mentions as a local or a receiverless
    /// call (`collect_mentioned_names`): a local a call passes that is
    /// not among them is never read.
    names: crate::det::HashMap<(String, String), crate::det::HashSet<Symbol>>,
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
    let stems = PartialStems::of(&app.views);
    let views: Vec<&View> = app.views.iter().filter(|v| v.jbuilder && !v.analysis_only).collect();
    let edges = render_edges(&views, &app.models, &stems);
    let closures = ivar_closures(&views, &edges);
    let mut bound: crate::det::HashMap<(String, String), Vec<Symbol>> =
        crate::det::HashMap::default();
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
    let mut out = PartialParams { closures, stems, ..Default::default() };
    for v in &views {
        let (dir, base) = split_view_name(v.name.as_str());
        let Some(stem) = base.strip_prefix('_') else { continue };
        let (mod_path, method) = partial_target(&format!("{dir}/{stem}"), dir, &out.stems);
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
        let mut used = crate::det::HashSet::default();
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
        if template_always_filled(&flatten_cache_blocks(stmts_of(&v.body))) {
            out.filled.insert(key.clone());
        }
        let mut mentioned = crate::det::HashSet::default();
        collect_mentioned_names(&v.body, &mut mentioned);
        out.names.insert(key.clone(), mentioned);
        out.params.insert(key.clone(), names);
        out.ivars.insert(key, ivars);
    }
    out
}

/// Whether a template's every render is a non-empty value: a lone
/// `json.array!` (an array, even an empty one), or an object with a
/// pair on every path. Conservative: a partial or a block on another
/// call counts as possibly empty.
fn template_always_filled(stmts: &[&Expr]) -> bool {
    let mut dsl = stmts.iter().map(|s| classify(s)).filter(|c| !matches!(c, JbStmt::Local));
    if let (
        Some(JbStmt::ArrayPartial { .. } | JbStmt::ArrayBlock { .. } | JbStmt::ArrayExtract { .. }),
        None,
    ) = (dsl.next(), dsl.next())
    {
        return true;
    }
    sets_a_pair(stmts)
}

/// Whether every run of `stmts`, as an object's statements, sets a pair.
fn sets_a_pair(stmts: &[&Expr]) -> bool {
    stmts.iter().any(|s| match classify(s) {
        JbStmt::Extract { attrs, .. } => !attrs.is_empty(),
        JbStmt::Pair { .. } | JbStmt::PairPartial { .. } | JbStmt::PairBlock { .. } => true,
        JbStmt::Nested { body, .. } => sets_a_pair(&flatten_cache_blocks(stmts_of(body))),
        JbStmt::Cond { then_branch, else_branch, .. } => {
            sets_a_pair(&branch_stmts(then_branch)) && sets_a_pair(&branch_stmts(else_branch))
        }
        // The body's pairs, or a rescue's after them (`emit_guarded`).
        JbStmt::Guarded { body, rescues } => {
            sets_a_pair(&branch_stmts(body)) && rescues.iter().all(|r| sets_a_pair(&branch_stmts(&r.body)))
        }
        _ => false,
    })
}

/// Whether a render of `partial_path` from this template is always a
/// non-empty value (`PartialParams::filled`).
fn partial_always_filled(partial_path: &str, ctx: &Ctx) -> bool {
    let (mod_path, method) = partial_target(partial_path, &ctx.resource_dir, &ctx.partials.stems);
    ctx.partials.filled.contains(&partial_key(&mod_path, &method))
}

/// Each jbuilder template's partial renders: the partial's key, its
/// view name when the app has a jbuilder template for it, and the
/// local names the render binds.
type RenderSites = Vec<((String, String), Option<Symbol>, Vec<Symbol>)>;

fn render_edges(
    views: &[&View],
    models: &[crate::dialect::Model],
    stems: &PartialStems,
) -> Vec<(Symbol, RenderSites)> {
    let mut partial_views: crate::det::HashMap<(String, String), Symbol> =
        crate::det::HashMap::default();
    for v in views {
        let (dir, base) = split_view_name(v.name.as_str());
        if let Some(stem) = base.strip_prefix('_') {
            let (mod_path, method) = partial_target(&format!("{dir}/{stem}"), dir, stems);
            partial_views.insert(partial_key(&mod_path, &method), v.name.clone());
        }
    }
    views
        .iter()
        .map(|v| {
            let (dir, _) = split_view_name(v.name.as_str());
            let mut sites = Vec::new();
            let records = record_partial::record_sites(&v.body, models);
            collect_partial_sites(&v.body, dir, &records, stems, &mut sites);
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

/// Whether a template reads the request's `params` (a bare, receiverless
/// `params`). A view reads the params of the controller that renders
/// it, which the lowered controller holds as `@params`, so the template
/// takes it as one more ivar of its closure: the controller passes
/// `@params`, and each partial gets it from its renderer.
fn reads_request_params(e: &Expr) -> bool {
    if matches!(&*e.node, ExprNode::Send { recv: None, method, args, block: None, .. }
        if method.as_str() == "params" && args.is_empty())
    {
        return true;
    }
    let mut found = false;
    e.node.for_each_child(&mut |c| found = found || reads_request_params(c));
    found
}

/// Every bare `params` read in `e`, as the local `name`.
fn request_params_to_local(e: &mut Expr, name: &Symbol) {
    if matches!(&*e.node, ExprNode::Send { recv: None, method, args, block: None, .. }
        if method.as_str() == "params" && args.is_empty())
    {
        *e.node = ExprNode::Var { id: VarId(0), name: name.clone() };
        return;
    }
    e.node.for_each_child_mut(&mut |c| request_params_to_local(c, name));
}

/// A template parameter's type: the request's params are a String-keyed
/// Hash (so `params[:q]` reads `"q"`, as in the controller), anything
/// else as its ivar name gives.
fn template_param_ty(name: &str, known_models: &[String]) -> crate::ty::Ty {
    if name == "params" {
        return crate::ty::Ty::Hash {
            key: std::sync::Arc::new(crate::ty::Ty::Str),
            value: std::sync::Arc::new(crate::ty::Ty::Untyped),
        };
    }
    crate::lower::view_to_library::ivar_ty(name, known_models)
}

/// The ivar closure of every template: its own reads, in first-seen
/// order, then each rendered partial's closure, depth first. A render
/// cycle contributes what was found before it closed.
fn ivar_closures(
    views: &[&View],
    edges: &[(Symbol, RenderSites)],
) -> crate::det::HashMap<Symbol, Vec<Symbol>> {
    use crate::det::{HashMap, HashSet};
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
        let params = Symbol::from("params");
        if bodies.get(name).is_some_and(|b| reads_request_params(b)) && !out.contains(&params) {
            out.push(params);
        }
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
    let mut done = HashMap::default();
    for v in views {
        visit(&v.name, &bodies, &children, &mut done, &mut HashSet::default());
    }
    done
}

/// The jbuilder side of the view render graph, for the analyzer:
/// each jbuilder template's name with the names of the jbuilder
/// partials it renders, so a partial is typed with its renderers'
/// ivars as an ERB partial is.
pub(crate) fn jbuilder_render_edges(app: &App) -> Vec<(Symbol, Vec<Symbol>)> {
    let stems = PartialStems::of(&app.views);
    let views: Vec<&View> = app.views.iter().filter(|v| v.jbuilder && !v.analysis_only).collect();
    render_edges(&views, &app.models, &stems)
        .into_iter()
        .map(|(name, sites)| (name, sites.into_iter().filter_map(|(_, t, _)| t).collect()))
        .collect()
}

/// Each jbuilder template's ivar closure, keyed by view name — the
/// parameters an action template takes and the controller passes.
pub(crate) fn jbuilder_ivar_closures(
    views: &[View],
    models: &[crate::dialect::Model],
) -> crate::det::HashMap<Symbol, Vec<Symbol>> {
    let stems = PartialStems::of(views);
    let views: Vec<&View> = views.iter().filter(|v| v.jbuilder && !v.analysis_only).collect();
    let edges = render_edges(&views, models, &stems);
    ivar_closures(&views, &edges)
}

/// Every name a template binds or reads as a local — variables, block
/// parameters, and the receiverless reads a partial's locals are — so a
/// generated parameter name takes none of them.
fn collect_local_names(e: &Expr, out: &mut crate::det::HashSet<Symbol>) {
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

/// Every name `e` reads as a local or calls without a receiver, with
/// or without arguments: a local of that name would be read, or would
/// shadow the method, at any of them. A `READS_ANY_LOCAL` name called
/// on `self` (`self.binding`) counts too: it reads the same locals.
/// So does one reached by dispatch, with no receiver or on `self`:
/// `send(:binding)`, `public_send`/`__send__` alike, `method(:binding)`
/// (a Send, or a `MethodRef` as a block argument `&method(:binding)`),
/// and a `send` or `method` of a name that is not a literal, which may
/// be any of them.
fn collect_mentioned_names(e: &Expr, out: &mut crate::det::HashSet<Symbol>) {
    match &*e.node {
        ExprNode::Var { name, .. } | ExprNode::Send { recv: None, method: name, .. } => {
            out.insert(name.clone());
        }
        ExprNode::Send { recv: Some(recv), method, .. }
            if matches!(&*recv.node, ExprNode::SelfRef) && READS_ANY_LOCAL.contains(&method.as_str()) =>
        {
            out.insert(method.clone());
        }
        ExprNode::MethodRef { recv, name }
            if recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
                && READS_ANY_LOCAL.contains(&name.as_str()) =>
        {
            out.insert(name.clone());
        }
        _ => {}
    }
    if let ExprNode::Send { recv, method, args, .. } = &*e.node {
        let on_self = recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef));
        let dispatch = matches!(method.as_str(), "send" | "public_send" | "__send__");
        if on_self && (dispatch || method.as_str() == "method") {
            let name = match args.first().map(|a| &*a.node) {
                Some(ExprNode::Lit { value: Literal::Sym { value } }) => Some(value.as_str()),
                Some(ExprNode::Lit { value: Literal::Str { value } }) => Some(value.as_str()),
                Some(ExprNode::Lit { .. }) => None,
                _ => {
                    out.extend(READS_ANY_LOCAL.iter().map(|n| Symbol::from(*n)));
                    None
                }
            };
            if let Some(name) = name.filter(|n| READS_ANY_LOCAL.contains(n)) {
                out.insert(Symbol::from(name));
            }
        }
    }
    e.node.for_each_child(&mut |c| collect_mentioned_names(c, out));
}

/// Every partial render in a jbuilder template, with the local names
/// it binds. Calls with a block are skipped (no partial call has one)
/// and walked into, so the collection blocks' own partial calls count.
fn collect_partial_sites(
    e: &Expr,
    dir: &str,
    records: &record_partial::RecordSites,
    stems: &PartialStems,
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
                // Only the record is bound: the literal locals are passed
                // only to partials that never read them.
                JbStmt::PartialRecord { arg, as_name, .. } => {
                    for (path, local, _) in record_partial_path(arg, as_name.as_ref(), dir, records).map(|r| r.branches).unwrap_or_default() {
                        let (mod_path, method) = partial_target(&path, dir, stems);
                        out.push((partial_key(&mod_path, &method), vec![local]));
                    }
                    None
                }
                _ => None,
            };
            if let Some((path, names)) = site {
                let (mod_path, method) = partial_target(&path, dir, stems);
                out.push((partial_key(&mod_path, &method), names));
            }
        }
    }
    e.node.for_each_child(&mut |c| collect_partial_sites(c, dir, records, stems, out));
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
/// The base is the stem `stems` gives the partial.
fn partial_target(path: &str, resource_dir: &str, stems: &PartialStems) -> (Vec<Symbol>, String) {
    let (dir, base) = match path.rsplit_once('/') {
        Some((d, b)) => (d.to_string(), b.to_string()),
        None => (resource_dir.to_string(), path.to_string()),
    };
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
    let base = stems.stem(&module_camel, &base);
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
///
/// `raw(x)` is `x.to_s`: Rails' `raw` is `x.to_s.html_safe`, and the
/// html_safe mark means nothing to JSON encoding (`raw(nil)` renders
/// `""`, not `null`). Left as a call, it reached a view module with no
/// `raw` (`undefined method 'raw'`). `h` leaves an html_safe String as
/// it is, so `h(raw(x))` is `x.to_s` too, unescaped.
///
/// Every value position is walked, inside Arrays, Hashes, conditionals
/// and blocks as well as call arguments.
fn rewrite_h_escape(e: &Expr) -> Expr {
    let mut out = e.clone();
    rewrite_h_escape_in_place(&mut out);
    unmark_raw_string(&mut out);
    out
}

fn rewrite_h_escape_in_place(e: &mut Expr) {
    e.node.for_each_child_mut(&mut rewrite_h_escape_in_place);
    // A marked child has now been seen by its parent (this node), and
    // only an `h` reads the mark: from here on it is Ruby's `to_s`.
    let h_of_raw = matches!(&*e.node, ExprNode::Send { recv: None, method, args, block: None, .. }
        if method.as_str() == "h" && args.len() == 1 && is_raw_string(&args[0]));
    if !h_of_raw {
        e.node.for_each_child_mut(&mut unmark_raw_string);
    }
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &mut *e.node else {
        return;
    };
    if args.len() != 1 {
        return;
    }
    match method.as_str() {
        // `h(raw(x))`: the argument is already the html_safe String
        // (lowered below to `x.to_s`, marked `RAW_STRING`), which `h`
        // returns unescaped.
        "h" if is_raw_string(&args[0]) => {
            let arg = args.remove(0);
            *e = arg;
        }
        "h" => {
            let arg = args.remove(0);
            let ty = e.ty.clone();
            *e.node = ExprNode::Send {
                recv: Some(Expr::new(
                    e.span,
                    ExprNode::Const {
                        path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")],
                    },
                )),
                method: Symbol::from("html_escape"),
                args: vec![arg],
                block: None,
                parenthesized: true,
            };
            e.ty = ty;
        }
        "raw" => {
            let arg = args.remove(0);
            let span = e.span;
            let mut to_s = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(arg),
                    method: Symbol::from(RAW_STRING),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            );
            to_s.ty = Some(crate::ty::Ty::Str);
            *e = to_s;
        }
        _ => {}
    }
}

fn unmark_raw_string(e: &mut Expr) {
    if let ExprNode::Send { method, args, .. } = &mut *e.node {
        if method.as_str() == RAW_STRING && args.is_empty() {
            *method = Symbol::from("to_s");
        }
    }
}

/// The method name `raw(x)` lowers to while its parent is still being
/// rewritten, so an enclosing `h(...)` can tell it from a `to_s` the
/// template wrote (which `h` does escape). Renamed to `to_s` when its
/// parent has been visited (or at the end, for the root).
const RAW_STRING: &str = "__rh_raw_to_s";

fn is_raw_string(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Send { method, args, .. }
        if method.as_str() == RAW_STRING && args.is_empty())
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
    let mut out = Expr::new(e.span, new_node);
    // The same send, rebuilt: keep a grounded `iso8601`'s flag, as
    // `rewrite_ivars_to_locals` does.
    out.decisions |= e.decisions & crate::expr::GROUNDED_TIME_FORMAT;
    out
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
    e.set_hint(Some(IrHint::StringBuilderInit));
    e
}

fn io_append_lit(accumulator: &str, s: &str) -> Expr {
    let recv = var_ref(Symbol::from(accumulator));
    let mut e = send(Some(recv), "<<", vec![lit_str(s.to_string())], None, false);
    e.set_hint(Some(IrHint::StringBuilderAppend));
    e
}

fn io_append_call(accumulator: &str, call: Expr) -> Expr {
    let recv = var_ref(Symbol::from(accumulator));
    let mut e = send(Some(recv), "<<", vec![call], None, false);
    e.set_hint(Some(IrHint::StringBuilderAppend));
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

/// The model the template's main positional arg is a record of, when
/// it resolves to a known model backed by a schema table. `None` for
/// layouts, index views (the arg is a collection, not a single record),
/// and any arg we can't tie back to a schema row.
fn model_for_arg(dir: &str, is_partial: bool, stem: &str, app: &App) -> Option<String> {
    // Index views' arg is the plural collection (`articles`), not a
    // single record. Per-row column lookups don't apply — the
    // extract! inside the partial handles those instead.
    if (!is_partial && stem == "index") || dir == "layouts" {
        return None;
    }
    let backed = |m: String| (!columns_for_model(&m, app).is_empty()).then_some(m);
    // A namespaced view directory (`api/v1/widgets`) names its record by
    // the last segment, as Rails' partial lookup does: the namespace is
    // the controller's. A namespaced model (`admin/widgets` for
    // `Admin::Widget`) is tried first.
    backed(crate::naming::singularize_camelize(dir)).or_else(|| {
        let (_, last) = dir.rsplit_once('/')?;
        backed(crate::naming::classify_path(dir)).or_else(|| backed(crate::naming::singularize_camelize(last)))
    })
}

/// Rails' `attributes` keys for `model_class`, in order: its table's
/// columns. `None` when the model declares an attribute the table lacks
/// (the attribute API adds it to `attributes`), or changes which columns
/// are attributes (`ignored_columns`, which Rails leaves out of
/// `attributes`, or `inheritance_column`): the same declarations
/// `as_json_record` does not read.
fn attribute_names(model_class: &str, app: &App) -> Option<Vec<String>> {
    use crate::lower::as_json_record::{ATTRIBUTE_MACROS, COLUMN_SETTERS};
    let model = app.models.iter().find(|m| m.name.0.as_str() == model_class)?;
    let changes_columns = model.body.iter().any(|item| match item {
        crate::dialect::ModelBodyItem::Unknown { expr, .. } => match &*expr.node {
            ExprNode::Send { recv, method, .. } => {
                COLUMN_SETTERS.contains(&method.as_str())
                    || (recv.is_none() && ATTRIBUTE_MACROS.contains(&method.as_str()))
            }
            _ => false,
        },
        _ => false,
    });
    if changes_columns {
        return None;
    }
    let table = app.schema.tables.get(&model.table.0)?;
    let columns: Vec<String> = table.columns.iter().map(|c| c.name.as_str().to_string()).collect();
    if model.attributes.fields.keys().any(|k| !columns.iter().any(|c| c == k.as_str())) {
        return None;
    }
    Some(columns)
}

/// The `{column_name → ColumnType}` map of `model_class`'s table; empty
/// when the app has no such model or table.
fn columns_for_model(
    model_class: &str,
    app: &App,
) -> crate::det::HashMap<Symbol, crate::schema::ColumnType> {
    let mut out: crate::det::HashMap<Symbol, crate::schema::ColumnType> =
        crate::det::HashMap::default();
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
fn rename_ivars(e: &mut Expr, renames: &crate::det::HashMap<Symbol, Symbol>) {
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
    let mut out = Expr::new(expr.span, new_node);
    // A rebuilt send keeps the lowering's decisions about it: a grounded
    // `iso8601` (`expr::GROUNDED_TIME_FORMAT`) whose receiver was an
    // ivar is now a parameter the emitter cannot type as a time, and
    // the flag is all that tells it to print `iso8601` again.
    out.decisions |= expr.decisions & crate::expr::GROUNDED_TIME_FORMAT;
    out
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
