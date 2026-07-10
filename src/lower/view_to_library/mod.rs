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
mod attr_parts;

use crate::App;
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param, View};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, InterpPart, IrHint, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::{camelize, singularize, snake_case};
use crate::span::Span;

use self::extra_params::collect_extra_params;
use self::walker::walk_body;

/// Bulk entry: lower every view, then type their bodies against a
/// shared registry so dispatch on framework helpers (ViewHelpers,
/// RouteHelpers, Inflector), sibling view modules, and model classes
/// resolves end-to-end. `extras` typically carries the model + view
/// ClassInfo entries the model lowerer built; this entry adds the
/// framework runtime stubs (ViewHelpers/RouteHelpers/Inflector/String)
/// before typing.
///
/// For per-view typing-isolated calls (tests/probes that don't have
/// a registry to share), the single-view entry below still works
/// and will run its own internal typing pass.
pub fn lower_views_to_library_classes(
    views: &[View],
    app: &App,
    extras: Vec<(ClassId, crate::analyze::ClassInfo)>,
) -> Vec<LibraryClass> {
    // Build LibraryClasses (with method signatures populated) but
    // *skip* the per-view internal body-typing pass — we'll do it
    // below with the merged registry.
    //
    // Only ERB (html-format) views go through this path. Jbuilder
    // (json-format) views are lowered by `jbuilder_to_library`,
    // which produces `<name>_json` methods on the same view module.
    let mut lcs: Vec<LibraryClass> = views
        .iter()
        .filter(|v| v.format.as_str() == "html")
        .map(|v| build_library_class(v, app, /*type_body=*/ false))
        .collect();

    // Merge: caller extras + framework runtime stubs + view modules
    // themselves (so cross-view dispatch like Views::Articles.article
    // resolves from one view to another).
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
        // Also register a last-segment alias for the typer's
        // Const-path resolver.
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

/// Migration entry point: lower views to `LibraryFunction`s, the
/// canonical post-lowering shape for module-callable artifacts.
/// Each template becomes one function whose `module_path` matches
/// the view directory (`["Views", "Articles"]`) and whose `name` is
/// the template's method name (`"article"`, `"index"`, etc.).
///
/// Implemented as a flattener over the existing class-shaped
/// lowerer — typing, framework stubs, registry merging are all
/// shared. The shape change happens at the boundary, not in the
/// body-typing core.
pub fn lower_views_to_library_functions(
    views: &[View],
    app: &App,
    extras: Vec<(ClassId, crate::analyze::ClassInfo)>,
) -> Vec<crate::dialect::LibraryFunction> {
    let lcs = lower_views_to_library_classes(views, app, extras);
    flatten_lcs_to_functions(&lcs)
}

/// Pivot LibraryClass methods into LibraryFunctions. Every method
/// (always class-method on a view module) becomes a standalone
/// function whose module_path is the LC name split on `::`.
///
/// Public so the TS emit can stage migration without changing the
/// body-typer registry shape — `extras_from_lcs` keeps consuming the
/// class form, while emit walks the function form.
pub fn flatten_lcs_to_functions(
    lcs: &[LibraryClass],
) -> Vec<crate::dialect::LibraryFunction> {
    let mut out = Vec::with_capacity(lcs.len());
    for lc in lcs {
        let module_path: Vec<Symbol> = lc
            .name
            .0
            .as_str()
            .split("::")
            .map(Symbol::from)
            .collect();
        for m in &lc.methods {
            out.push(crate::dialect::LibraryFunction {
                module_path: module_path.clone(),
                name: m.name.clone(),
                params: m.params.clone(),
                body: m.body.clone(),
                signature: m.signature.clone(),
                effects: m.effects.clone(),
                is_async: m.is_async,
            });
        }
    }
    out
}

/// Single-view entry point — kept for tests/probes. Runs an internal
/// body-typing pass with an empty registry; for whole-app emit where
/// cross-class dispatch matters, use `lower_views_to_library_classes`.
///
/// `app` is consulted only for known model names (so view args can be
/// typed implicitly downstream) and for FK resolution; the lowering is
/// otherwise pure.
pub fn lower_view_to_library_class(view: &View, app: &App) -> LibraryClass {
    build_library_class(view, app, /*type_body=*/ true)
}

fn build_library_class(view: &View, app: &App, type_body: bool) -> LibraryClass {
    let (dir, base) = split_view_name(view.name.as_str());
    let stem = base.trim_start_matches('_');

    let module_id = view_module_id(dir);
    let method_name = crate::lower::view::view_method_name(stem);

    let known_models: Vec<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    let arg_name = infer_view_arg(stem, dir, base.starts_with('_'), &known_models);

    // Rewrite `@ivar` → bare `ivar` everywhere so the inferred arg name
    // (and any extra params we surface) read as plain locals in the
    // emitted body. Mirrors the controller-side ivar-to-local pass.
    let rewritten = rewrite_ivars_to_locals(&view.body);

    // Apply erubi's `<% %>`-on-its-own-line trim before walking, so the
    // text-chunk literals that survive into the spinel-shape body
    // already match Rails' rendered whitespace. Other targets call
    // `trim_view` from their per-target view emitter; the spinel emit
    // path goes through this lowerer directly, so the trim has to
    // happen here too. (Compare-spinel's `<main>` whitespace diff
    // surfaced this gap: untrimmed bodies left an extra `\n` after
    // every non-output ERB tag.)
    let rewritten = crate::lower::erb_trim::trim_view(&rewritten);

    // Collect free names other than the inferred arg → those become
    // additional positional params. Today this picks up `notice`,
    // `alert`, etc. (Rails flash helpers parsed as bare Sends/Vars),
    // plus names referenced inside `defined?(name)` Sends (partial-
    // local optionality markers in ERB).
    let extra_params = collect_extra_params(&rewritten, &arg_name);

    // Rewrite `defined?(name)` marker Sends to `!name.nil?` checks.
    // Runs AFTER collect_extra_params so the inner Var name has been
    // captured as a nullable partial parameter. Once the partial's
    // signature includes `name: nil`, the nil-check captures the
    // same semantics the author intended ("is this optional local
    // present?") and downstream emitters don't need target-specific
    // `defined?` knowledge.
    let mut rewritten = rewritten;
    rewrite_defined_to_nil_check(&mut rewritten);

    // The inferred record arg (e.g. `articles`, `article`) is the
    // required positional. Free locals discovered downstream
    // (`notice`, `alert`, …) get a `nil` default so controllers that
    // don't have a flash to pass can still call `Views::X.action(rec)`
    // without arity errors. Spinel-blog's hand-written views use
    // keyword-with-default for these (`notice: nil`); the lowerer
    // models the same callability with positional-with-nil-default
    // until kw-args are first-class in `Param`.
    let nil_default = Expr::new(
        view.body.span,
        ExprNode::Lit { value: Literal::Nil },
    );
    // View↔controller data contract. Partials and layouts take a single
    // record/body arg supplied by the render/yield call site (a local, not
    // an ivar), so they keep the convention-derived `arg_name`. ACTION
    // views (index/show/…) instead take exactly the @ivars their template
    // reads, in first-seen order — the controller passes `@<name>` for each
    // (see controller_to_library's render rewrite). A multi-ivar view like
    // home/index then receives all of @stories/@page/@show_more/…; a view
    // that reads exactly its one resource ivar (the blog) gets the same
    // signature the convention would have produced.
    let is_partial = base.starts_with('_');
    let is_layout = dir == "layouts";
    let is_action_view = !is_partial && !is_layout && !dir.is_empty();

    // Render-tree ivar closure: the ivars this view needs (its own reads ∪
    // the ivars every partial it renders needs, transitively). Action views
    // take exactly these as positional params; partials take their record
    // arg PLUS these (threaded from the rendering view); the controller /
    // render call sites pass the matching values. Layouts are body-only
    // for now (their call site is main.rb, not yet threaded).
    let closures = std::rc::Rc::new(view_ivar_closures(&app.views, &app.controllers));
    let dyn_pools = std::rc::Rc::new(dynamic_partial_pools(&app.controllers));
    let closure_ivars: Vec<String> = view_key_of(view)
        .and_then(|k| closures.get(&k).cloned())
        .unwrap_or_default()
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();

    // Typed primary params: (name, type, required). Extras (notice/alert/…)
    // are appended afterward as nullable optionals.
    let mut typed: Vec<(String, crate::ty::Ty)> = Vec::new();
    if is_action_view {
        for iv in &closure_ivars {
            typed.push((iv.clone(), ivar_ty(iv, &known_models)));
        }
    } else {
        // Partial/layout: record/body arg from the render/yield call site,
        // then the threaded closure ivars. Layouts get the same closure
        // threading as partials — their call site is the Ruby emit path's
        // layout wrap (`apply_layout_lowering` rewrites each action's
        // `render(Views::X.y(...))` to pass the controller's @ivars), so a
        // layout reading @user/@title receives them like any partial. A
        // layout with an empty closure (the blog) keeps its body-only
        // signature, so the other targets' `Layouts.application(body)`
        // dispatch call sites are arity-stable.
        if !arg_name.is_empty() {
            typed.push((arg_name.clone(), record_arg_ty(dir, is_layout, &known_models)));
        }
        for iv in &closure_ivars {
            // The record arg already covers a same-named ivar (a `_form`
            // whose record is `category` and which reads `@category`) —
            // don't emit a duplicate param. The call site excludes it too.
            if iv == &arg_name {
                continue;
            }
            typed.push((iv.clone(), ivar_ty(iv, &known_models)));
        }
        // Layouts render in the controller's view context, where `flash`
        // is live — thread it as a param when the template reads it bare
        // (`flash[f]`). The layout wrap passes `@flash`.
        if is_layout && view_uses_bare_name(&rewritten, "flash") {
            typed.push(("flash".to_string(), crate::ty::Ty::Untyped));
        }
    }

    let mut params: Vec<Param> = Vec::new();
    for (n, _) in &typed {
        params.push(Param::positional(Symbol::from(n.as_str())));
    }
    for n in &extra_params {
        params.push(Param::with_default(
            Symbol::from(n.clone()),
            nil_default.clone(),
        ));
    }

    // Method signature: typed param list so the body-typer (and per-target
    // type-aware dispatch) resolves `articles.empty?` to Array dispatch,
    // `article.title` to a model attribute, etc. Primaries typed above;
    // extras are nullable strings.
    let signature = build_view_signature_from(&typed, &extra_params);

    let mut locals: Vec<String> = typed.iter().map(|(n, _)| n.clone()).collect();
    locals.extend(extra_params.iter().cloned());

    // Nullable-for-predicates: the `nil`-default extras (notice/alert) PLUS
    // any Untyped param. `present?`/`blank?` are defined to handle nil
    // (Rails: `nil.present?` == false), so an Untyped ivar the controller
    // may leave nil (`@referer ||= request.referer`) must get the nil-safe
    // `!x.nil? && !x.empty?` form, not a bare `!x.empty?` that crashes on
    // nil. Blog-neutral: its present?/any? receivers are all Array-typed
    // collections or already-nullable — never bare Untyped params.
    let mut nullable: std::collections::HashSet<String> =
        extra_params.iter().cloned().collect();
    for (n, ty) in &typed {
        if matches!(ty, crate::ty::Ty::Untyped) {
            nullable.insert(n.clone());
        }
    }

    let ctx = ViewCtx {
        locals,
        // Only layouts consult arg_name (emit_yield → the `body` local);
        // action views don't yield, so an empty name is fine for them.
        arg_name: if is_action_view { String::new() } else { arg_name.clone() },
        resource_dir: dir.to_string(),
        accumulator: "io".to_string(),
        form_records: Vec::new(),
        nullable_locals: nullable,
        reference_reads: std::rc::Rc::new(reference_reader_names(app)),
        nilable_scalar_reads: std::rc::Rc::new(nilable_scalar_reader_names(app)),
        stylesheets: app.stylesheets.clone(),
        partial_ivars: closures.clone(),
        dyn_pools: dyn_pools.clone(),
        partial_extras: std::rc::Rc::new(partial_extras_map(app)),
    };

    let mut body_stmts: Vec<Expr> = Vec::new();
    body_stmts.push(assign_accumulator_string_new(&ctx.accumulator));
    body_stmts.extend(walk_body(&rewritten, &ctx));
    body_stmts.push(accumulator_result_ref(&ctx.accumulator));

    let mut body = seq(body_stmts);
    // File-grain catch-all: whatever synthesis the walk-level stamps
    // didn't reach (`io = String.new`, the trailing `io`, TODO
    // markers from unrecognized shapes) attributes to the template as
    // a whole, so an emit-time diagnostic always names the right file.
    body.inherit_span(view.body.span);

    // View methods render HTML — they're functions in the spinel
    // sense (return String), so Method is the right kind.
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
            block_param: None,
    };

    // Run the body-typer over the lowered body so per-target emitters
    // get typed Sends (e.g. `articles.empty?` with recv typed as
    // `Ty::Array<Article>` so the Array dispatch resolves correctly).
    // Single-view path uses an empty class registry (sufficient for
    // primitive Array/String/Hash dispatch); the bulk entry above
    // re-types with the merged registry for cross-class resolution.
    if type_body {
        type_method_body(&mut method);
    }

    LibraryClass {
        name: module_id,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods: vec![method],
        origin: None,
        constants: Vec::new(),
    }
}

/// Framework runtime stubs the view bodies dispatch on. Each helper
/// returns `Ty::Str` (HTML output) or `Ty::Nil` (side-effecting
/// helpers like content_for_set). Args are mostly Untyped — refining
/// per-helper is future work; the Str-typed return is what unblocks
/// downstream typing.
pub(crate) fn insert_framework_stubs(
    classes: &mut std::collections::HashMap<ClassId, crate::analyze::ClassInfo>,
) {
    use crate::dialect::AccessorKind;
    use crate::lower::typing::{fn_sig, fn_sig_with_block};
    use crate::ty::Ty;

    // Helper: tag every method on a ClassInfo as `Method` (the
    // default for framework calls — every helper, route generator,
    // and runtime function takes parens). Called once per stub.
    let tag_all_method = |info: &mut crate::analyze::ClassInfo| {
        for name in info.instance_methods.keys().cloned().collect::<Vec<_>>() {
            info.instance_method_kinds.entry(name).or_insert(AccessorKind::Method);
        }
        for name in info.class_methods.keys().cloned().collect::<Vec<_>>() {
            info.class_method_kinds.entry(name).or_insert(AccessorKind::Method);
        }
    };

    // ViewHelpers — every output helper returns String; setters return Nil.
    let mut vh = crate::analyze::ClassInfo::default();
    let untyped = Ty::Untyped;
    let any_hash = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let html_helpers = [
        "turbo_stream_from",
        "link_to",
        "button_to",
        "html_escape",
        "truncate",
        "dom_id",
        "dom_class",
        "image_tag",
        "stylesheet_link_tag",
        "javascript_include_tag",
        "javascript_importmap_tags",
        "csrf_meta_tags",
        "csp_meta_tag",
        "yield_content",
        "fields_for",
        "label",
        "text_field",
        "text_area",
        "select",
        "submit",
        "hidden_field",
        "render",
        "time_ago_in_words",
        "number_to_human",
        "number_with_delimiter",
        "pluralize",
        "raw",
        "safe_join",
        "tag",
        "content_tag",
        "concat",
    ];
    for name in html_helpers {
        // Loose signature: variadic kwargs (Untyped) → String. The
        // precise per-helper arity isn't load-bearing for typing the
        // call SITE; the body-typer just needs the return type.
        vh.class_methods.insert(
            Symbol::from(name),
            fn_sig(vec![(Symbol::from("args"), untyped.clone())], Ty::Str),
        );
    }
    // Override the loose shim for helpers whose last param is an
    // explicit `opts = {}` positional Hash. The body-typer's
    // normalize_trailing_kwargs uses the last param's TYPE to decide
    // whether a trailing kwargs Hash should flip to an explicit Hash
    // literal at the call site (Crystal doesn't auto-collect kwargs
    // into a Hash positional). With `Ty::Untyped`, normalize can't
    // tell — declare `Ty::Hash` so it can. Helpers with named-
    // keyword params (truncate, dom_id, ...) keep the loose shim:
    // their kwargs DON'T flip and bind to the right named slot under
    // Crystal's named-arg dispatch.
    let opts_hash_helpers: &[(&str, &[(&str, &Ty)])] = &[
        ("link_to", &[("text", &untyped), ("href", &Ty::Str), ("opts", &any_hash)]),
        ("button_to", &[("text", &untyped), ("href", &Ty::Str), ("opts", &any_hash)]),
        ("stylesheet_link_tag", &[("name", &Ty::Str), ("opts", &any_hash)]),
    ];
    for (name, params) in opts_hash_helpers {
        let param_pairs: Vec<(Symbol, Ty)> = params
            .iter()
            .map(|(n, t)| (Symbol::from(*n), (*t).clone()))
            .collect();
        vh.class_methods.insert(
            Symbol::from(*name),
            fn_sig(param_pairs, Ty::Str),
        );
    }
    // form_with and FormBuilder stubs retired alongside the runtime
    // classes themselves: the lowerer macro-inlines form_with +
    // form.label/text_field/text_area/submit at lower time, so no
    // call site ever names `ViewHelpers.form_with` or
    // `FormBuilder.<method>` in the lowered output. The body-typer
    // doesn't need to resolve symbols that can't appear.
    //
    // Layout slot helpers — `content_for_get(:title)` / `get_slot(:title)`
    // return the previously-stored String, or nil when the slot was
    // never set. Matches the framework Ruby RBS (`String?`) and the
    // runtime semantics (`@slots.fetch(slot, nil)`). The Option<String>
    // shape lets the rust2 coerce path (Family 7) thread through to
    // `html_escape(content_for_get(:title))` without manual coercions.
    let option_string = Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
    for name in ["content_for_get", "get_slot"] {
        vh.class_methods.insert(
            Symbol::from(name),
            fn_sig(vec![(Symbol::from("name"), Ty::Sym)], option_string.clone()),
        );
    }
    // form_with macro-inline primitives (Wedge 1b-i). The inlined
    // form_with expansion calls these as small typed-scalar runtime
    // helpers rather than baking CSRF/_method bytes into every form
    // call site; future signed-token / CSP nonce work hooks here.
    vh.class_methods.insert(
        Symbol::from("csrf_token_hidden_input"),
        fn_sig(vec![], Ty::Str),
    );
    vh.class_methods.insert(
        Symbol::from("method_override_input"),
        fn_sig(vec![(Symbol::from("method"), Ty::Sym)], Ty::Str),
    );
    // `optional_value_attr(value: untyped) -> String` — used by the
    // inlined form.text_field expansion to emit ` value="..."` only
    // when the record's attribute is non-nil-non-empty. `untyped`
    // (rather than `String?`) so the call site can pass the
    // abstract Base#[] return type (`Int64 | String | … | Nil`)
    // directly without per-column casts.
    vh.class_methods.insert(
        Symbol::from("optional_value_attr"),
        fn_sig(
            vec![(Symbol::from("value"), Ty::Untyped)],
            Ty::Str,
        ),
    );
    // `escape_or_empty(value: untyped) -> String` — used by the
    // inlined form.text_area expansion: returns html_escape(value)
    // when non-nil, "" when nil. Same untyped rationale as
    // `optional_value_attr` above.
    vh.class_methods.insert(
        Symbol::from("escape_or_empty"),
        fn_sig(
            vec![(Symbol::from("value"), Ty::Untyped)],
            Ty::Str,
        ),
    );
    let nil_helpers = ["content_for_set", "content_for", "set_flash", "flash"];
    for name in nil_helpers {
        vh.class_methods.insert(
            Symbol::from(name),
            fn_sig(vec![(Symbol::from("args"), untyped.clone())], Ty::Nil),
        );
    }
    tag_all_method(&mut vh);
    // Canonical Rails-style nested path; body-typer's Const-arm
    // bare-name expansion swaps `Const { path: ["ViewHelpers"] }`
    // (from app/view source code) to the full path via this registry
    // entry. Lowerer-synthesized refs (`view_helpers_call`) also
    // resolve through the same key.
    classes.insert(ClassId(Symbol::from("ActionView::ViewHelpers")), vh);

    // RouteHelpers — every `_path` / `_url` helper returns String.
    // Catch-all: the typer's `Class { id }` lookup returns Untyped if
    // the method isn't in the table; we can't enumerate every
    // `<resource>_path` here, so we register a single permissive entry
    // that covers the most common ones used in real-blog. A
    // catch-all "any method on RouteHelpers returns String" would
    // need typer support that doesn't exist yet.
    let mut rh = crate::analyze::ClassInfo::default();
    let route_stems = [
        "article", "articles", "comment", "comments", "root",
        "new_article", "edit_article", "new_comment", "edit_comment",
        "article_comment", "article_comments", "new_article_comment",
        "edit_article_comment",
    ];
    for stem in route_stems {
        for suffix in ["path", "url"] {
            let name = format!("{stem}_{suffix}");
            rh.class_methods.insert(
                Symbol::from(name),
                fn_sig(vec![(Symbol::from("args"), untyped.clone())], Ty::Str),
            );
        }
    }
    tag_all_method(&mut rh);
    classes.insert(ClassId(Symbol::from("RouteHelpers")), rh);

    // Inflector — pluralize/singularize.
    let mut inf = crate::analyze::ClassInfo::default();
    inf.class_methods.insert(
        Symbol::from("pluralize"),
        fn_sig(
            vec![(Symbol::from("count"), Ty::Int), (Symbol::from("word"), Ty::Str)],
            Ty::Str,
        ),
    );
    inf.class_methods.insert(
        Symbol::from("singularize"),
        fn_sig(vec![(Symbol::from("word"), Ty::Str)], Ty::Str),
    );
    tag_all_method(&mut inf);
    classes.insert(ClassId(Symbol::from("Inflector")), inf);

    // JsonBuilder — encode_value / encode_string. Used by lowered
    // `*.json.jbuilder` templates (see `jbuilder_to_library`). Both
    // return String; encode_value takes Untyped because it dispatches
    // on the dynamic value type at runtime.
    let mut jb = crate::analyze::ClassInfo::default();
    jb.class_methods.insert(
        Symbol::from("encode_value"),
        fn_sig(vec![(Symbol::from("v"), Ty::Untyped)], Ty::Str),
    );
    jb.class_methods.insert(
        Symbol::from("encode_string"),
        fn_sig(
            vec![(Symbol::from("s"), Ty::Union { variants: vec![Ty::Str, Ty::Nil] })],
            Ty::Str,
        ),
    );
    tag_all_method(&mut jb);
    classes.insert(ClassId(Symbol::from("JsonBuilder")), jb);

    // Db — primitive surface the per-model `_adapter_*` Level-3
    // emit calls into. Backend-agnostic (sqlite via cruby gem here,
    // spinel-FFI sqlite planned, postgres/etc. siblings later); every
    // shim satisfies this contract. Stmt handle is opaque (Integer
    // here); per-target narrowing happens at emit time. See
    // project_level_3_adapter_emit.md and runtime/ruby/db.rbs.
    let mut db_info = crate::analyze::ClassInfo::default();
    db_info.class_methods.insert(
        Symbol::from("configure"),
        fn_sig(vec![(Symbol::from("path"), Ty::Str)], Ty::Nil),
    );
    db_info.class_methods.insert(
        Symbol::from("close"),
        fn_sig(vec![], Ty::Nil),
    );
    db_info.class_methods.insert(
        Symbol::from("exec"),
        fn_sig(vec![(Symbol::from("sql"), Ty::Str)], Ty::Nil),
    );
    db_info.class_methods.insert(
        Symbol::from("prepare"),
        fn_sig(vec![(Symbol::from("sql"), Ty::Str)], Ty::Int),
    );
    db_info.class_methods.insert(
        Symbol::from("step?"),
        fn_sig(vec![(Symbol::from("stmt"), Ty::Int)], Ty::Bool),
    );
    db_info.class_methods.insert(
        Symbol::from("column_int"),
        fn_sig(
            vec![(Symbol::from("stmt"), Ty::Int), (Symbol::from("i"), Ty::Int)],
            Ty::Int,
        ),
    );
    db_info.class_methods.insert(
        Symbol::from("column_text"),
        fn_sig(
            vec![(Symbol::from("stmt"), Ty::Int), (Symbol::from("i"), Ty::Int)],
            Ty::Str,
        ),
    );
    db_info.class_methods.insert(
        Symbol::from("finalize"),
        fn_sig(vec![(Symbol::from("stmt"), Ty::Int)], Ty::Nil),
    );
    db_info.class_methods.insert(
        Symbol::from("last_insert_rowid"),
        fn_sig(vec![], Ty::Int),
    );
    db_info.class_methods.insert(
        Symbol::from("changes"),
        fn_sig(vec![], Ty::Int),
    );
    db_info.class_methods.insert(
        Symbol::from("escape_string"),
        fn_sig(vec![(Symbol::from("s"), Ty::Str)], Ty::Str),
    );
    db_info.class_methods.insert(
        Symbol::from("escape_int"),
        fn_sig(vec![(Symbol::from("n"), Ty::Int)], Ty::Str),
    );
    tag_all_method(&mut db_info);
    classes.insert(ClassId(Symbol::from("Db")), db_info);

    // String — register `new` returning Ty::Str so the lowered
    // `io = String.new` produces a Str-typed local; downstream
    // `io << X` then dispatches through the primitive str_method
    // table (which already covers `<<`). Without this, `io` would
    // type as Class(String) and `<<` falls through to unregistered-
    // class behavior.
    let mut str_class = crate::analyze::ClassInfo::default();
    str_class.class_methods.insert(
        Symbol::from("new"),
        fn_sig(vec![], Ty::Str),
    );
    tag_all_method(&mut str_class);
    classes.insert(ClassId(Symbol::from("String")), str_class);

    // Broadcasts — re-stub here so view-only callers don't need to
    // remember to add it themselves.
    let _ = any_hash; // captured by html_helpers above
    let mut bc = crate::analyze::ClassInfo::default();
    // Broadcasts.* takes a kwargs bag (`**opts`); see
    // `model_to_library::broadcasts_class_info` for the rationale on
    // marking the param `KeywordRest` so the body-typer's
    // normalize_trailing_kwargs leaves the call's `kwargs: true` flag
    // alone (preserves the bare named-args call shape across targets).
    let opts_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let bc_sig = Ty::Fn {
        params: vec![crate::ty::Param {
            name: Symbol::from("opts"),
            ty: opts_ty,
            kind: crate::ty::ParamKind::KeywordRest,
        }],
        block: None,
        ret: Box::new(Ty::Nil),
        effects: crate::effect::EffectSet::pure(),
    };
    for name in ["prepend", "replace", "remove", "append"] {
        bc.class_methods.insert(Symbol::from(name), bc_sig.clone());
    }
    tag_all_method(&mut bc);
    classes.insert(ClassId(Symbol::from("Broadcasts")), bc);

    // Importmap — `Importmap.pins -> Array<Record{name: Str, path: Str}>`,
    // `Importmap.entry -> Str`. The view lowerer's
    // `JavascriptImportmapTags` rewrite emits Send calls on
    // `Importmap` (used to be `Importmap::PINS` const access);
    // the typer needs to resolve them or the body has untyped
    // sub-expressions and the residual ratchet trips. Each pin is a
    // record with two fixed fields, mirroring the importmap lowerer
    // (`importmap_to_library`). Record (rather than `Hash<Sym, Str>`)
    // keeps `p[:name]` access typed across strict targets — Crystal
    // parses `{name: ..., path: ...}` as `NamedTuple`, which matches
    // `Ty::Record` but conflicts with `Hash`.
    let mut im = crate::analyze::ClassInfo::default();
    let pin_record_ty = {
        let mut fields = indexmap::IndexMap::new();
        fields.insert(Symbol::from("name"), Ty::Str);
        fields.insert(Symbol::from("path"), Ty::Str);
        Ty::Record { row: crate::ty::Row { fields, rest: None } }
    };
    im.class_methods.insert(
        Symbol::from("pins"),
        fn_sig(vec![], Ty::Array { elem: Box::new(pin_record_ty) }),
    );
    im.class_methods.insert(
        Symbol::from("entry"),
        fn_sig(vec![], Ty::Str),
    );
    tag_all_method(&mut im);
    classes.insert(ClassId(Symbol::from("Importmap")), im);

    // FormBuilder stub retired — see the `form_with` comment above
    // (form.label/text_field/text_area/submit dispatch inline-
    // expands at lower time, so the body-typer never resolves
    // FormBuilder instance methods in a lowered view).

    // ErrorCollection — what `record.errors` returns. `each` yields
    // a String message (Spinel-shape: errors are stored as flat
    // String messages, not ActiveModel::Error objects). `empty?`
    // and `count` cover the common predicates view bodies use.
    let mut ec = crate::analyze::ClassInfo::default();
    ec.instance_methods.insert(
        Symbol::from("each"),
        fn_sig_with_block(vec![], Some(Ty::Str), Ty::Nil),
    );
    ec.instance_methods.insert(Symbol::from("empty?"), fn_sig(vec![], Ty::Bool));
    ec.instance_methods.insert(Symbol::from("any?"), fn_sig(vec![], Ty::Bool));
    ec.instance_methods.insert(Symbol::from("count"), fn_sig(vec![], Ty::Int));
    ec.instance_methods.insert(Symbol::from("size"), fn_sig(vec![], Ty::Int));
    ec.instance_methods.insert(Symbol::from("length"), fn_sig(vec![], Ty::Int));
    ec.instance_methods.insert(
        Symbol::from("full_messages"),
        fn_sig(vec![], Ty::Array { elem: Box::new(Ty::Str) }),
    );
    ec.instance_methods.insert(
        Symbol::from("[]"),
        fn_sig(
            vec![(Symbol::from("attr"), Ty::Sym)],
            Ty::Array { elem: Box::new(Ty::Str) },
        ),
    );
    tag_all_method(&mut ec);
    classes.insert(ClassId(Symbol::from("ErrorCollection")), ec);

    // ActionController::Parameters used to be seeded here so the
    // body-typer could resolve `params.require(...).to_h` chains. As
    // of the Parameters retirement, `@params` is a plain
    // `Hash[String, untyped]` and Hash's primitive method surface is
    // a builtin — no per-class seed needed.

    // ActionDispatch::Flash — what `controller.flash` is post-Phase-
    // 2.5(b) (was HashWithIndifferentAccess). Typed `notice`/`alert`
    // fields + HWIA-shape shim methods. Controllers read
    // `@flash[:notice]` / write `@flash[:notice] = "..."`; the lowerer
    // emits these as Send-`[]` / Send-`[]=` calls and the typer
    // resolves them through this stub.
    let mut flash_cls = crate::analyze::ClassInfo::default();
    let nullable_str = Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
    flash_cls.instance_methods.insert(
        Symbol::from("[]"),
        fn_sig(vec![(Symbol::from("key"), Ty::Sym)], nullable_str.clone()),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("[]="),
        fn_sig(
            vec![(Symbol::from("key"), Ty::Sym), (Symbol::from("value"), nullable_str.clone())],
            nullable_str.clone(),
        ),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("fetch"),
        fn_sig(
            vec![(Symbol::from("key"), Ty::Sym), (Symbol::from("default"), nullable_str.clone())],
            nullable_str.clone(),
        ),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("key?"),
        fn_sig(vec![(Symbol::from("key"), Ty::Sym)], Ty::Bool),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("has_key?"),
        fn_sig(vec![(Symbol::from("key"), Ty::Sym)], Ty::Bool),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("delete"),
        fn_sig(vec![(Symbol::from("key"), Ty::Sym)], nullable_str.clone()),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("length"),
        fn_sig(vec![], Ty::Int),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("size"),
        fn_sig(vec![], Ty::Int),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("empty?"),
        fn_sig(vec![], Ty::Bool),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("to_h"),
        fn_sig(vec![], Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) }),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("notice"),
        fn_sig(vec![], nullable_str.clone()),
    );
    flash_cls.instance_methods.insert(
        Symbol::from("alert"),
        fn_sig(vec![], nullable_str.clone()),
    );
    tag_all_method(&mut flash_cls);
    classes.insert(ClassId(Symbol::from("ActionDispatch::Flash")), flash_cls);

    // ActionDispatch::Session — empty for real-blog (no session keys
    // exercised). Shim methods registered so `@session.length()` and
    // friends resolve. Values typed Untyped (Hash-backed storage).
    let mut session_cls = crate::analyze::ClassInfo::default();
    session_cls.instance_methods.insert(
        Symbol::from("[]"),
        fn_sig(vec![(Symbol::from("key"), Ty::Sym)], Ty::Untyped),
    );
    session_cls.instance_methods.insert(
        Symbol::from("[]="),
        fn_sig(
            vec![(Symbol::from("key"), Ty::Sym), (Symbol::from("value"), Ty::Untyped)],
            Ty::Untyped,
        ),
    );
    session_cls.instance_methods.insert(
        Symbol::from("fetch"),
        fn_sig(
            vec![(Symbol::from("key"), Ty::Sym), (Symbol::from("default"), Ty::Untyped)],
            Ty::Untyped,
        ),
    );
    session_cls.instance_methods.insert(
        Symbol::from("key?"),
        fn_sig(vec![(Symbol::from("key"), Ty::Sym)], Ty::Bool),
    );
    session_cls.instance_methods.insert(
        Symbol::from("has_key?"),
        fn_sig(vec![(Symbol::from("key"), Ty::Sym)], Ty::Bool),
    );
    session_cls.instance_methods.insert(
        Symbol::from("delete"),
        fn_sig(vec![(Symbol::from("key"), Ty::Sym)], Ty::Untyped),
    );
    session_cls.instance_methods.insert(
        Symbol::from("length"),
        fn_sig(vec![], Ty::Int),
    );
    session_cls.instance_methods.insert(
        Symbol::from("size"),
        fn_sig(vec![], Ty::Int),
    );
    session_cls.instance_methods.insert(
        Symbol::from("empty?"),
        fn_sig(vec![], Ty::Bool),
    );
    session_cls.instance_methods.insert(
        Symbol::from("to_h"),
        fn_sig(
            vec![],
            Ty::Hash { key: Box::new(Ty::Untyped), value: Box::new(Ty::Untyped) },
        ),
    );
    tag_all_method(&mut session_cls);
    classes.insert(ClassId(Symbol::from("ActionDispatch::Session")), session_cls);
}

// ── view-name → module / arg / method helpers ────────────────────

pub(crate) fn split_view_name(name: &str) -> (&str, &str) {
    name.rsplit_once('/').unwrap_or(("", name))
}

/// Module the view's method lives under: `Views::Articles` for an
/// `articles/...` view. Empty `dir` (uncommon — top-level view) maps
/// to the bare `Views` module.
pub(crate) fn view_module_id(dir: &str) -> ClassId {
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
/// Run the body-typer over a method's body so Send dispatch sees
/// typed receivers. Per-method (no cross-method registry) since
/// view methods are independent and view-body Send patterns
/// resolve through primitive method tables (Array / String / Hash)
/// that don't need a class registry.
fn type_method_body(method: &mut MethodDef) {
    // Seed the registry with framework stubs (ViewHelpers,
    // RouteHelpers, FormBuilder, ...) so the body-typer's bare-Const
    // expansion can resolve `ViewHelpers.dom_id(...)` to the full
    // path `ActionView::ViewHelpers.dom_id(...)`. The single-view
    // lowerer is invoked from the Spinel/Ruby per-view emit path
    // where no shared cross-class registry exists; without these
    // stubs, the rewrite fails silently and Ruby gets bare refs
    // that can't resolve under nested-module lexical scope.
    let mut classes: std::collections::HashMap<
        crate::ident::ClassId,
        crate::analyze::ClassInfo,
    > = std::collections::HashMap::new();
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
            id: crate::ident::ClassId(enclosing.clone()),
            args: vec![],
        });
    }
    typer.analyze_expr(&mut method.body, &ctx);
}

/// Build a `Ty::Fn` signature for the synthesized view method.
/// Lets the body-typer propagate types through the body (so e.g.
/// `articles.empty?` resolves to Array's `.empty?` dispatch and
/// renders correctly per-target). Without this, params come through
/// as `Ty::Untyped` and emit-side type-aware dispatch falls through.
pub(crate) fn build_view_signature(
    stem: &str,
    dir: &str,
    is_partial: bool,
    arg_name: &str,
    extra_params: &[String],
    known_models: &[String],
) -> Option<crate::ty::Ty> {
    use crate::ty::{Param as TyParam, ParamKind, Ty};

    if arg_name.is_empty() && extra_params.is_empty() {
        return None;
    }

    let model_class = crate::naming::singularize_camelize(dir);
    let model_known = known_models.iter().any(|m| m == &model_class);

    // Type for the main arg (when present).
    let arg_ty = if arg_name.is_empty() {
        None
    } else if dir == "layouts" {
        // `body` arg of a layout — the rendered inner HTML.
        Some(Ty::Str)
    } else if !is_partial && stem == "index" {
        // `articles` — Array<Article> when the model is known,
        // otherwise Array<Untyped>.
        if model_known {
            Some(Ty::Array {
                elem: Box::new(Ty::Class {
                    id: crate::ident::ClassId(crate::ident::Symbol::from(model_class.as_str())),
                    args: vec![],
                }),
            })
        } else {
            Some(Ty::Array { elem: Box::new(Ty::Untyped) })
        }
    } else {
        // Show / edit / new / partial: arg is the model itself.
        if model_known {
            Some(Ty::Class {
                id: crate::ident::ClassId(crate::ident::Symbol::from(model_class.as_str())),
                args: vec![],
            })
        } else {
            Some(Ty::Untyped)
        }
    };

    let mut sig_params: Vec<TyParam> = Vec::new();
    if let Some(t) = arg_ty {
        sig_params.push(TyParam {
            name: crate::ident::Symbol::from(arg_name),
            ty: t,
            kind: ParamKind::Required,
        });
    }
    // Extra params (`notice`, `alert`, …) — nullable strings.
    for n in extra_params {
        sig_params.push(TyParam {
            name: crate::ident::Symbol::from(n.as_str()),
            ty: Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
            kind: ParamKind::Optional,
        });
    }

    Some(Ty::Fn {
        params: sig_params,
        block: None,
        ret: Box::new(Ty::Str),
        effects: crate::effect::EffectSet::default(),
    })
}

/// The argument contract an action view expects from its controller: the
/// read-ivars it takes positionally, plus whether it references the
/// `action_name`/`controller_name` controller-context helpers (which the
/// controller then passes as literals — but ONLY to views that use them,
/// so views/targets that don't gain no extra params).
#[derive(Default)]
pub(crate) struct ViewArgs {
    pub ivars: Vec<Symbol>,
    pub uses_action_name: bool,
    pub uses_controller_name: bool,
}

/// Map `(view-module, action-stem) -> ViewArgs` for the controller's
/// render rewrite. Keyed to match `views_module_name(controller)` (which
/// equals `camelize(snake_case(dir))`) plus the rendered action. Only
/// HTML, non-partial, non-layout views participate.
pub(crate) fn action_view_ivar_map(
    views: &[crate::dialect::View],
    controllers: &[crate::dialect::Controller],
) -> std::collections::HashMap<(String, String), ViewArgs> {
    // The controller passes an action view its full render-tree ivar
    // closure (its own reads ∪ its partials' needs, including dynamic-
    // partial pools), matching the view's generated params — so an ivar a
    // deep partial reads (e.g. @user) is threaded even when the action
    // view itself doesn't read it.
    let closures = view_ivar_closures(views, controllers);
    let mut out = std::collections::HashMap::new();
    for v in views {
        let (dir, base) = split_view_name(v.name.as_str());
        if dir.is_empty() || dir == "layouts" || base.starts_with('_') {
            continue;
        }
        if v.format.as_str() != "html" {
            continue;
        }
        let module = camelize(&snake_case(dir));
        let key = (module, base.to_string());
        let ivars = closures
            .get(&key)
            .cloned()
            .unwrap_or_else(|| view_read_ivars(&v.body));
        out.insert(
            key,
            ViewArgs {
                ivars,
                uses_action_name: view_uses_bare_name(&v.body, "action_name"),
                uses_controller_name: view_uses_bare_name(&v.body, "controller_name"),
            },
        );
    }
    out
}

/// True when the view body references `name` as a bare identifier — a
/// no-recv/no-arg Send (`action_name`) or a Var (`action_name` already
/// lowered to a local). Used to surface controller-context helpers
/// (action_name/controller_name) as view params only when actually used.
pub(crate) fn view_uses_bare_name(body: &Expr, name: &str) -> bool {
    fn walk(e: &Expr, name: &str) -> bool {
        let hit = match &*e.node {
            ExprNode::Var { name: n, .. } => n.as_str() == name,
            ExprNode::Send { recv: None, method, args, block, .. } => {
                method.as_str() == name && args.is_empty() && block.is_none()
            }
            _ => false,
        };
        if hit {
            return true;
        }
        let mut found = false;
        e.node.for_each_child(&mut |c| {
            if !found {
                found = walk(c, name);
            }
        });
        found
    }
    walk(body, name)
}

/// A view's identity key for the render graph / closure map:
/// `(module, stem)` matching `views_module_name(controller)` ==
/// `camelize(snake_case(dir))` plus the rendered action/partial name.
pub(crate) type ViewKey = (String, String);

/// Per-PARTIAL extras list ((module, method) → collect_extra_params
/// output), so a render call site carrying an explicit `locals:` hash
/// can bind values to the partial's trailing extra params positionally.
/// Mirrors the def-site computation in build_library_class exactly —
/// same ivar rewrite, same trim, same collector — so the orders can't
/// drift.
pub(super) fn partial_extras_map(
    app: &App,
) -> std::collections::HashMap<(String, String), Vec<String>> {
    let known_models: Vec<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    let mut out: std::collections::HashMap<(String, String), Vec<String>> =
        std::collections::HashMap::new();
    for view in &app.views {
        let (dir, base) = split_view_name(view.name.as_str());
        if dir.is_empty() || !base.starts_with('_') {
            continue;
        }
        let stem = base.trim_start_matches('_');
        let arg_name = infer_view_arg(stem, dir, true, &known_models);
        let rewritten = rewrite_ivars_to_locals(&view.body);
        let rewritten = crate::lower::erb_trim::trim_view(&rewritten);
        let extras = collect_extra_params(&rewritten, &arg_name);
        out.insert((camelize(&snake_case(dir)), stem.to_string()), extras);
    }
    out
}

fn view_key_of(v: &View) -> Option<ViewKey> {
    let (dir, base) = split_view_name(v.name.as_str());
    if dir.is_empty() {
        return None;
    }
    Some((camelize(&snake_case(dir)), base.trim_start_matches('_').to_string()))
}

/// Ruby-emit-path layout wrap factory: the Expr for
///
/// ```ruby
/// Views::Layouts.application(<inner>, @<ivar>…, @flash?, @flash[:notice], @flash[:alert])
/// ```
///
/// mirroring the layout signature `build_library_class` constructs
/// (body, closure ivars, flash-if-used, then the uniform notice/alert
/// extras). None when the app has no `layouts/application` html view.
/// Consumed by `emit::ruby::library::apply_layout_lowering`, which
/// rewrites each action's `render(Views::X.y(...))` — the controller
/// seam where the @ivars a layout reads are statically in scope. (The
/// generic dispatch previously wrapped layouts body-only; a layout
/// reading @user had no way to receive it there.)
pub fn layout_wrap_expr(app: &crate::App, inner: Expr) -> Option<Expr> {
    let key: ViewKey = ("Layouts".to_string(), "application".to_string());
    let layout = app
        .views
        .iter()
        .find(|v| v.format.as_str() == "html" && view_key_of(v).as_ref() == Some(&key))?;
    let closures = view_ivar_closures(&app.views, &app.controllers);
    let ivars = closures.get(&key).cloned().unwrap_or_default();
    let span = inner.span;
    let ivar = |name: &Symbol| Expr::new(span, ExprNode::Ivar { name: name.clone() });
    let flash_slot = |slot: &str| {
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(
                    span,
                    ExprNode::Ivar { name: Symbol::from("flash") },
                )),
                method: Symbol::from("[]"),
                args: vec![Expr::new(
                    span,
                    ExprNode::Lit { value: Literal::Sym { value: Symbol::from(slot) } },
                )],
                block: None,
                parenthesized: true,
            },
        )
    };
    let mut args: Vec<Expr> = vec![inner];
    for iv in &ivars {
        args.push(ivar(iv));
    }
    if view_uses_bare_name(&layout.body, "flash") {
        args.push(ivar(&Symbol::from("flash")));
    }
    args.push(flash_slot("notice"));
    args.push(flash_slot("alert"));
    Some(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const {
                    path: vec![Symbol::from("Views"), Symbol::from("Layouts")],
                },
            )),
            method: Symbol::from("application"),
            args,
            block: None,
            parenthesized: true,
        },
    ))
}

/// Transitive instance-variable closure per view: the ivars a view NEEDS
/// = the ivars its own template reads ∪ the ivars every partial it renders
/// needs (recursively). This is the typed alternative to a dynamic
/// assigns-bag — each needed ivar threads through as a typed positional
/// param (view params + partial call-site args both read this map, so they
/// agree). Dynamic partial names (`render partial: @x`) can't be resolved
/// statically, so that subtree's needs aren't folded in (those renders are
/// nil-guarded by the caller). Keyed for every html view (action +
/// partial).
pub(crate) fn view_ivar_closures(
    views: &[View],
    controllers: &[crate::dialect::Controller],
) -> std::collections::HashMap<ViewKey, Vec<Symbol>> {
    use std::collections::{BTreeSet, HashMap};
    let pools = dynamic_partial_pools(controllers);
    let mut closure: HashMap<ViewKey, BTreeSet<Symbol>> = HashMap::new();
    let mut edges: HashMap<ViewKey, Vec<ViewKey>> = HashMap::new();
    for v in views {
        if v.format.as_str() != "html" {
            continue;
        }
        let Some(key) = view_key_of(v) else { continue };
        let (dir, _) = split_view_name(v.name.as_str());
        let reads: BTreeSet<Symbol> = view_read_ivars(&v.body).into_iter().collect();
        closure.entry(key.clone()).or_default().extend(reads);
        let mut child_keys = render_partial_keys(&v.body, dir);
        // A `render partial: @above` folds every pooled candidate partial's
        // ivar needs into this view, so the dispatch's arms have their
        // closure args threaded as params here.
        child_keys.extend(dynamic_render_edges(&v.body, dir, &pools));
        edges.entry(key).or_default().extend(child_keys);
    }
    // Fixpoint: propagate each partial's needs up to every view that
    // renders it, until no set grows.
    loop {
        let mut changed = false;
        let keys: Vec<ViewKey> = edges.keys().cloned().collect();
        for key in keys {
            let children = edges.get(&key).cloned().unwrap_or_default();
            let mut add: BTreeSet<Symbol> = BTreeSet::new();
            for child in &children {
                if let Some(c) = closure.get(child) {
                    add.extend(c.iter().cloned());
                }
            }
            let entry = closure.entry(key).or_default();
            for s in add {
                if entry.insert(s) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    closure
        .into_iter()
        .map(|(k, set)| (k, set.into_iter().collect()))
        .collect()
}

/// The partial views a body renders, as `ViewKey`s — for the render graph.
/// Resolves the same render shapes `classify_render_partial` recognizes;
/// unresolvable (dynamic) partial names are skipped.
fn render_partial_keys(body: &Expr, dir: &str) -> Vec<ViewKey> {
    let mut out = Vec::new();
    collect_render_keys(body, dir, &mut out);
    out
}

fn collect_render_keys(e: &Expr, dir: &str, out: &mut Vec<ViewKey>) {
    if let ExprNode::Send { recv, method, args, block, .. } = &*e.node {
        if let Some(rp) = crate::lower::view::classify_render_partial(
            recv.as_ref(),
            method.as_str(),
            args,
            block.as_ref(),
            &|_| true,
        ) {
            if let Some(k) = render_partial_key(&rp, dir) {
                out.push(k);
            }
        }
    }
    e.node.for_each_child(&mut |c| collect_render_keys(c, dir, out));
}

/// Resolve a partial-name string to its `(module, method)` ViewKey.
/// A slash form (`"stories/subnav"`) names an explicit module; a bare
/// name (`"active"`) resolves relative to `dir` (the rendering view's
/// directory) — matching Rails' relative-partial-path lookup.
fn partial_name_to_key(name: &str, dir: &str) -> ViewKey {
    match name.rsplit_once('/') {
        Some((d, n)) => (camelize(&snake_case(d)), n.trim_start_matches('_').to_string()),
        None => (
            camelize(&snake_case(dir)),
            name.trim_start_matches('_').to_string(),
        ),
    }
}

fn render_partial_key(rp: &crate::lower::view::RenderPartial<'_>, dir: &str) -> Option<ViewKey> {
    use crate::lower::view::RenderPartial;
    Some(match rp {
        RenderPartial::Collection { name, .. } => (camelize(&snake_case(name)), singularize(name)),
        RenderPartial::Association { method, .. } => {
            (camelize(&snake_case(method)), singularize(method))
        }
        RenderPartial::Named { partial, .. } | RenderPartial::CollectionNamed { partial, .. } => {
            partial_name_to_key(partial, dir)
        }
        // A dynamic name resolves to a POOL of keys, not one — folded into
        // the render graph separately (see `dynamic_render_edges`).
        RenderPartial::DynamicNamed { .. } => return None,
    })
}

/// The controller's convention view directory: the class name minus its
/// `Controller` suffix, snake-cased (`HomeController` → `home`). Matches
/// the `dir` component of that controller's view names and each view's
/// `resource_dir`, so a dynamic-partial pool keyed by dir lines up with
/// the rendering view.
fn controller_view_dir(name: &ClassId) -> String {
    let s = name.0.as_str();
    snake_case(s.strip_suffix("Controller").unwrap_or(s))
}

/// For each `(view-dir, ivar)`, the partial-name string literals a
/// controller assigns to `@<ivar>` — the pool a `render partial: @<ivar>`
/// can resolve to at runtime. Collected from every action body's
/// `@x = "literal"` writes. Only consulted when a view actually renders
/// `@<ivar>` dynamically, so over-collection (every string ivar, not just
/// the rendered ones) is inert. Empty for the blog (no such assignments →
/// no dynamic-partial dispatch anywhere).
pub(crate) fn dynamic_partial_pools(
    controllers: &[crate::dialect::Controller],
) -> std::collections::HashMap<(String, Symbol), Vec<String>> {
    use std::collections::{BTreeSet, HashMap};
    let mut acc: HashMap<(String, Symbol), BTreeSet<String>> = HashMap::new();
    for c in controllers {
        let dir = controller_view_dir(&c.name);
        for action in c.actions() {
            collect_ivar_str_assigns(&action.body, &dir, &mut acc);
        }
    }
    acc.into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect()
}

fn collect_ivar_str_assigns(
    e: &Expr,
    dir: &str,
    acc: &mut std::collections::HashMap<(String, Symbol), std::collections::BTreeSet<String>>,
) {
    if let ExprNode::Assign { target: LValue::Ivar { name }, value } = &*e.node {
        if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
            acc.entry((dir.to_string(), name.clone()))
                .or_default()
                .insert(s.as_str().to_string());
        }
    }
    e.node.for_each_child(&mut |c| collect_ivar_str_assigns(c, dir, acc));
}

/// The render-graph edges a view's DYNAMIC partials contribute: for each
/// `render partial: @<ivar>` in the body, every pooled name for
/// `(dir, ivar)` resolves to a partial ViewKey. Lets the closure fixpoint
/// fold each candidate partial's ivar needs into the rendering view.
fn dynamic_render_edges(
    body: &Expr,
    dir: &str,
    pools: &std::collections::HashMap<(String, Symbol), Vec<String>>,
) -> Vec<ViewKey> {
    let mut out = Vec::new();
    collect_dynamic_edges(body, dir, pools, &mut out);
    out
}

fn collect_dynamic_edges(
    e: &Expr,
    dir: &str,
    pools: &std::collections::HashMap<(String, Symbol), Vec<String>>,
    out: &mut Vec<ViewKey>,
) {
    if let ExprNode::Send { recv, method, args, block, .. } = &*e.node {
        if let Some(crate::lower::view::RenderPartial::DynamicNamed { ivar, .. }) =
            crate::lower::view::classify_render_partial(
                recv.as_ref(),
                method.as_str(),
                args,
                block.as_ref(),
                &|_| true,
            )
        {
            if let Some(names) = pools.get(&(dir.to_string(), Symbol::from(ivar))) {
                for name in names {
                    out.push(partial_name_to_key(name, dir));
                }
            }
        }
    }
    e.node.for_each_child(&mut |c| collect_dynamic_edges(c, dir, pools, out));
}

/// The instance variables an action view READS, in first-seen order.
/// This is the view↔controller contract: an action view's parameters are
/// exactly these ivars and the controller passes `@<name>` for each, so a
/// multi-ivar template (home/index reads @stories, @page, …) gets them
/// all — not just one convention-named record. Computed on the ORIGINAL
/// view body (before `rewrite_ivars_to_locals`) and by the controller
/// render rewrite on the same body, so both sides agree on the list/order.
pub(crate) fn view_read_ivars(body: &Expr) -> Vec<Symbol> {
    let mut seen: std::collections::BTreeSet<Symbol> = Default::default();
    let mut out: Vec<Symbol> = Vec::new();
    collect_read_ivars(body, &mut seen, &mut out);
    out
}

fn collect_read_ivars(
    e: &Expr,
    seen: &mut std::collections::BTreeSet<Symbol>,
    out: &mut Vec<Symbol>,
) {
    if let ExprNode::Ivar { name } = &*e.node {
        if seen.insert(name.clone()) {
            out.push(name.clone());
        }
    }
    e.node.for_each_child(&mut |c| collect_read_ivars(c, seen, out));
}

/// Type for a single read-ivar param by name: `@articles`/`@stories`
/// (plural, singularize-camelizes to a known model) → `Array[Model]`;
/// `@article`/`@story` (singular known model) → `Model`; anything else
/// (`@page`, `@root_path`, …) → `Untyped`. Mirrors the convention typing
/// `build_view_signature` applies to the single arg, but per-ivar.
fn ivar_ty(name: &str, known_models: &[String]) -> crate::ty::Ty {
    use crate::ty::Ty;
    let cam = crate::naming::singularize_camelize(name);
    if known_models.iter().any(|m| m == &cam) {
        let model = Ty::Class {
            id: crate::ident::ClassId(crate::ident::Symbol::from(cam.as_str())),
            args: vec![],
        };
        if crate::naming::singularize(name) != name {
            Ty::Array { elem: Box::new(model) }
        } else {
            model
        }
    } else {
        Ty::Untyped
    }
}

/// Type of a partial/layout's record arg: a layout's `body` is the
/// rendered-HTML String; a partial's record is the singular model for its
/// directory (`stories/_listdetail` → `Story`), else Untyped.
fn record_arg_ty(dir: &str, is_layout: bool, known_models: &[String]) -> crate::ty::Ty {
    use crate::ty::Ty;
    if is_layout {
        return Ty::Str;
    }
    let model_class = crate::naming::singularize_camelize(dir);
    if known_models.iter().any(|m| m == &model_class) {
        Ty::Class {
            id: crate::ident::ClassId(crate::ident::Symbol::from(model_class.as_str())),
            args: vec![],
        }
    } else {
        Ty::Untyped
    }
}

/// Build a view method's `Ty::Fn` from its typed primary params (record
/// arg and/or threaded ivars) followed by the nullable extra params
/// (notice/alert/action_name/…).
fn build_view_signature_from(
    typed: &[(String, crate::ty::Ty)],
    extra_params: &[String],
) -> Option<crate::ty::Ty> {
    use crate::ty::{Param as TyParam, ParamKind, Ty};
    if typed.is_empty() && extra_params.is_empty() {
        return None;
    }
    let mut sig_params: Vec<TyParam> = Vec::new();
    for (n, t) in typed {
        sig_params.push(TyParam {
            name: crate::ident::Symbol::from(n.as_str()),
            ty: t.clone(),
            kind: ParamKind::Required,
        });
    }
    for n in extra_params {
        sig_params.push(TyParam {
            name: crate::ident::Symbol::from(n.as_str()),
            ty: Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
            kind: ParamKind::Optional,
        });
    }
    Some(Ty::Fn {
        params: sig_params,
        block: None,
        ret: Box::new(Ty::Str),
        effects: crate::effect::EffectSet::default(),
    })
}

pub(crate) fn infer_view_arg(stem: &str, dir: &str, is_partial: bool, _known_models: &[String]) -> String {
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
        ExprNode::Hash { entries, kwargs } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (rewrite_ivars_to_locals(k), rewrite_ivars_to_locals(v)))
                .collect(),
            kwargs: *kwargs,
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
        LValue::Const { path } => LValue::Const { path: path.clone() },
    }
}

/// Rewrite every `Send(None, :defined?, [Var(name)])` under `expr` to
/// `Send(Send(Var(name), :nil?, []), :!, [])` — i.e., `!name.nil?`.
/// Post-order walk: rewrite children first, then test the current
/// node so nested `defined?` (rare in ERB) lower bottom-up.
///
/// The author's intent in writing `defined?(name)` in a partial is
/// "is the (optional) local `name` present"; once `name` is collected
/// as a nullable parameter (default `nil`) by `collect_extra_params`,
/// the nil-check captures the same semantics. Downstream emitters
/// then handle a plain Send chain instead of needing target-specific
/// `defined?` keyword knowledge.
fn rewrite_defined_to_nil_check(expr: &mut Expr) {
    // Recurse into children first.
    match &mut *expr.node {
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::SelfRef => {}
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                rewrite_defined_to_nil_check(k);
                rewrite_defined_to_nil_check(v);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                rewrite_defined_to_nil_check(el);
            }
        }
        ExprNode::StringInterp { parts } => {
            for part in parts {
                if let InterpPart::Expr { expr } = part {
                    rewrite_defined_to_nil_check(expr);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            rewrite_defined_to_nil_check(left);
            rewrite_defined_to_nil_check(right);
        }
        ExprNode::Let { value, body, .. } => {
            rewrite_defined_to_nil_check(value);
            rewrite_defined_to_nil_check(body);
        }
        ExprNode::Lambda { body, .. } => rewrite_defined_to_nil_check(body),
        ExprNode::Apply { fun, args, block } => {
            rewrite_defined_to_nil_check(fun);
            for a in args {
                rewrite_defined_to_nil_check(a);
            }
            if let Some(b) = block {
                rewrite_defined_to_nil_check(b);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                rewrite_defined_to_nil_check(r);
            }
            for a in args {
                rewrite_defined_to_nil_check(a);
            }
            if let Some(b) = block {
                rewrite_defined_to_nil_check(b);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            rewrite_defined_to_nil_check(cond);
            rewrite_defined_to_nil_check(then_branch);
            rewrite_defined_to_nil_check(else_branch);
        }
        ExprNode::Case { scrutinee, arms } => {
            rewrite_defined_to_nil_check(scrutinee);
            for arm in arms {
                if let Some(g) = arm.guard.as_mut() {
                    rewrite_defined_to_nil_check(g);
                }
                rewrite_defined_to_nil_check(&mut arm.body);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                rewrite_defined_to_nil_check(e);
            }
        }
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
            rewrite_defined_to_nil_check(value);
            if let LValue::Attr { recv, .. } = target {
                rewrite_defined_to_nil_check(recv);
            }
            if let LValue::Index { recv, index } = target {
                rewrite_defined_to_nil_check(recv);
                rewrite_defined_to_nil_check(index);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                rewrite_defined_to_nil_check(a);
            }
        }
        ExprNode::Raise { value } => rewrite_defined_to_nil_check(value),
        ExprNode::RescueModifier { expr, fallback } => {
            rewrite_defined_to_nil_check(expr);
            rewrite_defined_to_nil_check(fallback);
        }
        ExprNode::Return { value } => rewrite_defined_to_nil_check(value),
        ExprNode::Super { args } => {
            if let Some(arglist) = args {
                for a in arglist {
                    rewrite_defined_to_nil_check(a);
                }
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                rewrite_defined_to_nil_check(v);
            }
        }
        ExprNode::Splat { value } => rewrite_defined_to_nil_check(value),
        ExprNode::MultiAssign { value, .. } => rewrite_defined_to_nil_check(value),
        ExprNode::While { cond, body, .. } => {
            rewrite_defined_to_nil_check(cond);
            rewrite_defined_to_nil_check(body);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                rewrite_defined_to_nil_check(b);
            }
            if let Some(e) = end {
                rewrite_defined_to_nil_check(e);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            rewrite_defined_to_nil_check(body);
            for r in rescues {
                rewrite_defined_to_nil_check(&mut r.body);
            }
            if let Some(eb) = else_branch {
                rewrite_defined_to_nil_check(eb);
            }
            if let Some(en) = ensure {
                rewrite_defined_to_nil_check(en);
            }
        }
        ExprNode::Cast { value, .. } => rewrite_defined_to_nil_check(value),
    }

    // Now test the current node. Match `Send(None, :defined?,
    // [Var(name)])` exactly.
    let is_defined_send = matches!(
        &*expr.node,
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "defined?"
                && args.len() == 1
                && matches!(&*args[0].node, ExprNode::Var { .. })
    );
    if !is_defined_send {
        return;
    }
    // Extract the inner Var and synthesize `!var.nil?`.
    let var_expr = if let ExprNode::Send { args, .. } = &*expr.node {
        args[0].clone()
    } else {
        return;
    };
    let span = expr.span;
    let nil_check = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(var_expr),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    );
    expr.node = Box::new(ExprNode::Send {
        recv: Some(nil_check),
        method: Symbol::from("!"),
        args: vec![],
        block: None,
        parenthesized: false,
    });
}

// ── FormBuilder binding ──────────────────────────────────────────

/// Per-form_with state threaded through the inner block walk so
/// `form.label`/`form.text_field` macro-expansion can synthesize
/// the right attribute names and record-attribute reads at lower
/// time. Populated by `form_with::emit_form_with_inline` when
/// entering the block; consumed by
/// `form_builder::emit_form_builder_inline` when a `form.X` Send is
/// encountered during the walk.
#[derive(Clone)]
pub(super) struct FormBuilderBinding {
    /// The block param name (e.g. "form" from `do |form|`). The
    /// walker matches a `Send { recv: Some(Var(form_param)), … }`
    /// against this to detect macro-call sites.
    pub(super) form_param: String,
    /// Form-prefix string used in `<input name="<model_name>[…]">`
    /// and `<label for="<model_name>_<field>">`. Derived from the
    /// resource dir's singular (or the child class's name for the
    /// polymorphic-array nested-resource form).
    pub(super) model_name: String,
    /// Local Var to dispatch attribute readers on (e.g. `article` →
    /// `article.title` for the value attr). For simple `model: <var>`
    /// shapes this reuses the source local; for complex shapes
    /// (`model: Comment.new`, `model: [parent, Class.new]`) the
    /// inline expansion synthesizes a fresh `<form_param>_record`
    /// local at form_with entry and stores its name here.
    pub(super) record_var: Symbol,
    /// Local Var holding the form method Symbol (`:patch` or `:post`).
    /// Synthesized as `<form_param>_method` at form_with entry.
    /// `form.submit`'s default-text expansion reads this to choose
    /// "Update X" (patch) vs "Create X" (post).
    pub(super) form_method_var: Symbol,
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
    /// FormBuilder bindings active at this scope. Populated when
    /// entering a `form_with` block. The macro-inline form.X
    /// dispatch (form_builder.rs) reads these to expand
    /// `form.text_field :title` into direct HTML accumulation
    /// (`<input name="<model_name>[<field>]" ... value=...>`).
    pub(super) form_records: Vec<FormBuilderBinding>,
    /// Locals known to be nullable — the view's extra_params with a
    /// `nil` default (`notice`, `alert`, …). When a predicate
    /// (`recv.present?`, `recv.empty?`, …) targets one of these,
    /// rewrite to the nil-safe form `!recv.nil? && !recv.empty?` so
    /// the body doesn't NoMethodError when callers omit the kwarg.
    pub(super) nullable_locals: std::collections::HashSet<String>,
    /// Record-reference reader names — every `belongs_to`/`has_one`
    /// association name across the app's models. `rewrite_predicates`
    /// consults this (plus the `_id` suffix) to lower `present?`/`blank?`
    /// on a reference read to the nil test instead of the `empty?` form
    /// (`story.domain.present?` → `!story.domain.nil?`).
    pub(super) reference_reads: std::rc::Rc<std::collections::HashSet<String>>,
    /// Nilable-scalar reader names through a record: typed_store
    /// attributes with no default (nil when unset). Emptiness
    /// predicates on these get the nil-safe forms (see
    /// `rewrite_predicates`). Empty for apps without the DSL.
    pub(super) nilable_scalar_reads: std::rc::Rc<std::collections::HashSet<String>>,
    /// Stylesheet logical names ingested from `app/assets/stylesheets/`
    /// + `app/assets/builds/`. Used by the `stylesheet_link_tag(:app,
    /// ...)` expansion: a `:app` symbol arg fans out to one call per
    /// stylesheet, mirroring how Rails' Propshaft resolves `:app`.
    pub(super) stylesheets: Vec<String>,
    /// Render-tree ivar closure (`view_ivar_closures`), shared across this
    /// view's scopes. `emit_render_partial` looks up a rendered partial's
    /// needed ivars here and passes them as call-site args (the caller's
    /// own locals — its closure ⊇ the partial's, so it always has them).
    pub(super) partial_ivars: std::rc::Rc<std::collections::HashMap<ViewKey, Vec<Symbol>>>,
    /// Dynamic-partial name pools, `(view-dir, ivar) -> [partial-name
    /// literals]` (`dynamic_partial_pools`). `emit_render_partial` reads
    /// this for a `render partial: @<ivar>` DynamicNamed dispatch: each
    /// pooled name resolves to a `Views::X.method` arm. Empty for apps
    /// without dynamic partials (the blog), so the dispatch never fires.
    pub(super) dyn_pools:
        std::rc::Rc<std::collections::HashMap<(String, Symbol), Vec<String>>>,
    /// Per-partial extras list (`partial_extras_map`): the trailing
    /// nil-default params (notice/alert/defined?-marked locals) in def
    /// order. `emit_render_partial` binds an explicit `locals:` hash's
    /// values to these positions.
    pub(super) partial_extras:
        std::rc::Rc<std::collections::HashMap<(String, String), Vec<String>>>,
}

/// Every `belongs_to`/`has_one` association name across the app's models
/// — the single-record readers whose result is a record or nil (see
/// `ViewCtx::reference_reads`). has_many names stay out: collections keep
/// the `empty?`-based predicate forms.
/// Non-bool `typed_store` attribute names with no default across the
/// app's models — readers that yield nil when the attribute is unset.
/// Bool attributes stay out (their read sites are truthiness tests,
/// and the synthesized `<name>?` predicate handles the Rails form).
fn nilable_scalar_reader_names(app: &App) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for m in &app.models {
        for (_col, attrs) in crate::lower::typed_store::typed_store_decls(&m.body) {
            for a in attrs {
                if !a.is_bool && a.nilable() {
                    out.insert(a.name.as_str().to_string());
                }
            }
        }
    }
    out
}

fn reference_reader_names(app: &App) -> std::collections::HashSet<String> {
    use crate::dialect::Association;
    let mut out = std::collections::HashSet::new();
    for m in &app.models {
        for a in m.associations() {
            match a {
                Association::BelongsTo { name, .. } | Association::HasOne { name, .. } => {
                    out.insert(name.as_str().to_string());
                }
                _ => {}
            }
        }
    }
    out
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
///
/// Tagged with `IrHint::StringBuilderInit` so non-Ruby emitters that
/// have a more idiomatic accumulator form (Crystal `String::Builder`,
/// Go `strings.Builder`, TS array+join) can pick it up. Ruby/Spinel/
/// Rust ignore the hint (their canonical form already matches).
pub(super) fn assign_accumulator_string_new(name: &str) -> Expr {
    let string_const = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("String")] },
    );
    let new_call = send(Some(string_const), "new", Vec::new(), None, false);
    let mut e = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from(name) },
            value: new_call,
        },
    );
    e.hint = Some(IrHint::StringBuilderInit);
    e
}

/// `<accumulator> << <arg>` — the per-step append. Always emits with
/// `<<` (a binary operator the Ruby emit_send_base rewrites to infix
/// form), so the source comes out as `io << arg`, not `io.<<(arg)`.
///
/// Tagged with `IrHint::StringBuilderAppend` so emitters can pick the
/// target-idiomatic append form (Go `WriteString`, TS array `push`).
pub(super) fn accumulator_append_call(arg: Expr, ctx: &ViewCtx) -> Expr {
    let mut e = send(
        Some(var_ref(Symbol::from(ctx.accumulator.as_str()))),
        "<<",
        vec![arg],
        None,
        false,
    );
    e.hint = Some(IrHint::StringBuilderAppend);
    e
}

/// Terminal `<accumulator>` reference at the tail of a view function
/// body — returns the accumulated string. Distinct from `var_ref` so
/// only this site picks up `IrHint::StringBuilderResult`; generic Var
/// references to `io` elsewhere stay untagged.
pub(super) fn accumulator_result_ref(name: &str) -> Expr {
    let mut e = var_ref(Symbol::from(name));
    e.hint = Some(IrHint::StringBuilderResult);
    e
}

pub(super) fn view_helpers_call(method: &str, args: Vec<Expr>) -> Expr {
    // Constant-fold `html_escape("literal")`. The escape is deterministic
    // and the literal never changes, so escaping static class strings and
    // button labels on every request is pure waste — the spinel profile
    // showed the regex escaper (`re_exec`) running per request, mostly on
    // compile-time constants. Emit the pre-escaped literal instead. This is
    // byte-identical to the runtime call it replaces: the same 5-char set
    // as `ViewHelpers::HTML_ESCAPES` (`& < > " '`), which is also Rails',
    // so `compare` is unaffected. Only bare String literals fold; dynamic
    // args (article.title, …) keep the runtime call.
    if method == "html_escape" && args.len() == 1 {
        if let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node {
            return lit_str(html_escape_fold(value));
        }
    }
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("ViewHelpers")] },
    );
    // Trailing-kwargs vs explicit-Hash decision happens in the body
    // typer's `normalize_trailing_kwargs` — it consults the receiver
    // class's resolved method signature and flips `kwargs: true →
    // false` only for callees declared with positional Hash params
    // (link_to / button_to / stylesheet_link_tag etc. take `opts =
    // {}`). Keyword-param helpers (truncate, etc.) keep `kwargs:
    // true` so they bind to the right named slot.
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

/// Apply `ViewHelpers::HTML_ESCAPES` at compile time. Single pass over the
/// input so an introduced `&` is never re-escaped — matching the runtime
/// `s.gsub(/[&<>"']/, HTML_ESCAPES)` byte-for-byte (and Rails').
fn html_escape_fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
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
