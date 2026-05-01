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
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param, View};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
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
    let mut lcs: Vec<LibraryClass> = views
        .iter()
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
    let method_name = Symbol::from(stem);

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

    // Build the method signature: typed param list so the body-typer
    // (and downstream emitters' type-aware dispatch) can resolve
    // `articles.empty?` to Array dispatch, `article.title` to a
    // model-attribute access, etc. Without this, params come through
    // as `Ty::Untyped` and emit-side dispatch falls through.
    //
    // Type rules:
    //   - Index views (`articles/index`): main arg is `Array[Model]`
    //     for the singularize-camelize-matches-known-model case.
    //   - Show / edit / new / partial: main arg is the model itself.
    //   - Layout: main arg is String (the rendered body HTML).
    //   - Extra params (notice, alert, …): String? (nullable).
    let signature = build_view_signature(stem, dir, base.starts_with('_'), &arg_name, &extra_params, &known_models);

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
        stylesheets: app.stylesheets.clone(),
    };

    let mut body_stmts: Vec<Expr> = Vec::new();
    body_stmts.push(assign_accumulator_string_new(&ctx.accumulator));
    body_stmts.extend(walk_body(&rewritten, &ctx));
    body_stmts.push(var_ref(Symbol::from(ctx.accumulator.as_str())));

    let body = seq(body_stmts);

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

    let form_builder_ty = Ty::Class {
        id: ClassId(Symbol::from("FormBuilder")),
        args: vec![],
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
    // form_with yields a FormBuilder to its block — register with
    // `block: Some(FormBuilder)` so the typer's block_params_for
    // binds `|form|` to FormBuilder when encountered.
    vh.class_methods.insert(
        Symbol::from("form_with"),
        fn_sig_with_block(
            vec![(Symbol::from("opts"), untyped.clone())],
            Some(form_builder_ty.clone()),
            Ty::Str,
        ),
    );
    // Layout slot helpers — `content_for_get(:title)` / `get_slot(:title)`
    // return the previously-stored String; counterpart to content_for_set.
    for name in ["content_for_get", "get_slot"] {
        vh.class_methods.insert(
            Symbol::from(name),
            fn_sig(vec![(Symbol::from("name"), Ty::Sym)], Ty::Str),
        );
    }
    let nil_helpers = ["content_for_set", "content_for", "set_flash", "flash"];
    for name in nil_helpers {
        vh.class_methods.insert(
            Symbol::from(name),
            fn_sig(vec![(Symbol::from("args"), untyped.clone())], Ty::Nil),
        );
    }
    tag_all_method(&mut vh);
    classes.insert(ClassId(Symbol::from("ViewHelpers")), vh);

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
    let kwargs = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let bc_sig = fn_sig(vec![(Symbol::from("opts"), kwargs)], Ty::Nil);
    for name in ["prepend", "replace", "remove", "append"] {
        bc.class_methods.insert(Symbol::from(name), bc_sig.clone());
    }
    tag_all_method(&mut bc);
    classes.insert(ClassId(Symbol::from("Broadcasts")), bc);

    // Importmap — `Importmap.pins -> Array<Hash<Str,Str>>`,
    // `Importmap.entry -> Str`. The view lowerer's
    // `JavascriptImportmapTags` rewrite emits Send calls on
    // `Importmap` (used to be `Importmap::PINS` const access);
    // the typer needs to resolve them or the body has untyped
    // sub-expressions and the residual ratchet trips.
    let mut im = crate::analyze::ClassInfo::default();
    let pin_hash_ty = Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) };
    im.class_methods.insert(
        Symbol::from("pins"),
        fn_sig(vec![], Ty::Array { elem: Box::new(pin_hash_ty) }),
    );
    im.class_methods.insert(
        Symbol::from("entry"),
        fn_sig(vec![], Ty::Str),
    );
    tag_all_method(&mut im);
    classes.insert(ClassId(Symbol::from("Importmap")), im);

    // FormBuilder — instance methods called on the block param `form`
    // inside `form_with do |form| ... end`. Each helper renders one
    // input/label/button and returns a string.
    let mut fb = crate::analyze::ClassInfo::default();
    let fb_inputs = [
        "label",
        "text_field",
        "text_area",
        "select",
        "submit",
        "hidden_field",
        "checkbox",
        "check_box",
        "radio_button",
        "number_field",
        "email_field",
        "password_field",
        "date_field",
        "datetime_field",
        "file_field",
        "url_field",
        "color_field",
        "range_field",
        "phone_field",
        "search_field",
        "fields_for",
        "object_name",
    ];
    for name in fb_inputs {
        fb.instance_methods.insert(
            Symbol::from(name),
            fn_sig(vec![(Symbol::from("args"), untyped.clone())], Ty::Str),
        );
    }
    tag_all_method(&mut fb);
    classes.insert(ClassId(Symbol::from("FormBuilder")), fb);

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
/// Run the body-typer over a method's body so Send dispatch sees
/// typed receivers. Per-method (no cross-method registry) since
/// view methods are independent and view-body Send patterns
/// resolve through primitive method tables (Array / String / Hash)
/// that don't need a class registry.
fn type_method_body(method: &mut MethodDef) {
    let empty_classes: std::collections::HashMap<
        crate::ident::ClassId,
        crate::analyze::ClassInfo,
    > = std::collections::HashMap::new();
    let typer = crate::analyze::BodyTyper::new(&empty_classes);
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
fn build_view_signature(
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
    /// Stylesheet logical names ingested from `app/assets/stylesheets/`
    /// + `app/assets/builds/`. Used by the `stylesheet_link_tag(:app,
    /// ...)` expansion: a `:app` symbol arg fans out to one call per
    /// stylesheet, mirroring how Rails' Propshaft resolves `:app`.
    pub(super) stylesheets: Vec<String>,
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
