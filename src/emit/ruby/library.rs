//! Library-shape Ruby emission — for transpiled-shape input where class
//! bodies already contain explicit methods (no Rails DSL expansion).
//! Mirrors `src/emit/typescript/library.rs` in scope; produces one
//! `app/models/<name>.rb` per `LibraryClass`.
//!
//! Ruby is implicit about ivar declaration and global about constant
//! resolution, so this emitter is shorter than the TS analog: no ivar
//! field block, no import partition.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use super::super::EmittedFile;
use crate::App;
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;

pub(super) fn emit_library_class_decls(app: &App) -> Vec<EmittedFile> {
    let mut lcs: Vec<LibraryClass> = app.library_classes.clone();
    apply_scope_lowering(&mut lcs, app);
    apply_library_partial_render_lowering(&mut lcs, app);
    apply_helper_lowering(&mut lcs, app);
    // A bare `<x>_path` that `apply_helper_lowering` did not claim and
    // the route table DOES answer is a route helper called somewhere
    // nothing qualifies — campfire's `Messages::AttachmentPresentation`
    // `delegate`s `rails_blob_path` to its context, which leaves three
    // calls to a method the emitted tree defines nowhere.
    crate::lower::route_helper_receiver::qualify_lcs(&mut lcs, app);
    apply_route_param_lowering(&mut lcs, app);
    apply_raw_helper_monomorphization(&mut lcs, app);
    // send→case grounding, update-kwargs inlining, mailer class-side
    // wrappers, and duration grounding run in the shared post-analyze
    // hook, which covers these library classes (dispatch's plural
    // duration-unit arms arrive already grounded).
    // Transpiled-shape classes carry hand-written accessors that
    // `synth_attr_reader` never sees, so the datetime reader/writer
    // rewrite still runs here for them (Ruby-only). Model-lowered classes
    // get the reader from `synth_attr_reader` (shared, all targets); this
    // re-applies the same reader idempotently and adds the Ruby writer
    // normalize.
    apply_datetime_lowering(&mut lcs, app);
    apply_boolean_lowering(&mut lcs, app);
    apply_hydration_nil_lowering(&mut lcs, app);
    apply_nilsafe_empty_lowering(&mut lcs);
    apply_time_format_lowering(&mut lcs);
    // Same rooting the view pipeline applies, for the same reason one
    // step over: a class nested in a namespace that SHADOWS what it
    // references. `Message::Broadcasts` is where the broadcast lowering
    // emits `Broadcasts.append(...)`, and inside `module Broadcasts`
    // that constant is the concern itself. Idempotent (an already-rooted
    // head is skipped), so running it in both pipelines is safe.

    apply_constant_rooting(&mut lcs, app, RootingScope::RuntimeOnly);
    lcs.iter()
        .flat_map(|lc| {
            // `underscore`, not `snake_case`: a namespaced reopen
            // (lobsters' `ActiveRecord::Base.q`, `Net::HTTP`,
            // `ShortId::CandidateId`) nests as `active_record/base.rb` —
            // a literal `::` in the filename breaks the emitted
            // Makefile's dependency list.
            let file_stem = crate::naming::underscore(lc.name.0.as_str());
            let out_path = PathBuf::from(format!("app/models/{file_stem}.rb"));
            emit_library_class_pair(lc, app, out_path)
        })
        .collect()
}

use crate::facades::{Facade, EXTRAS_FACADES};

/// Swap façade-fated extras emits (scaffold base: spinel + the trees
/// derived from it). No-op when the app doesn't define the class —
/// the path simply isn't present.
pub(super) fn apply_extras_facades(files: &mut [(String, String)]) {
    for Facade {
        stem, rb, rbs, ..
    } in EXTRAS_FACADES
    {
        for (path, content) in files.iter_mut() {
            if path == &format!("{stem}.rb") {
                *content = (*rb).to_string();
            } else if path == &format!("{stem}.rbs") || path == &format!("sig/{stem}.rbs") {
                // BOTH forms, and the `sig/` one is the one that fires.
                // `emit_library_class_rbs` writes sidecars `sig/`-rooted
                // (`sig_path_for`), and `spin_shape` only flattens them to
                // file-adjacent LATER — so at this point every sidecar is
                // still `sig/app/models/<stem>.rbs`. Matching only the bare
                // form meant this arm never fired for any of the four
                // façades: the `.rb` swapped, the `.rbs` did not, and every
                // consumer read roundhouse's own inference instead of the
                // hand-written contract sitting beside the façade.
                //
                // What that cost, end to end: the contract declares
                // `OpenSSL::RandomSource.random_bytes: (Integer) -> String`,
                // and without it that call typed untyped, which widened
                // `str` in `Utils.random_str`, which made
                // `CandidateId#to_s` untyped, which put a
                // `def to_s: () -> untyped` in the program — and one of
                // those widens every poly `.to_s` (matz/spinel#4090). Ten
                // links on, the cookie jar's `@inbound[k.to_s] = v` had
                // untyped KEYS and the C build stopped on a PolyPolyHash
                // reaching a `Hash[String, String]` slot.
                *content = (*rbs).to_string();
            }
        }
    }
}

/// CRuby: put the verbatim source-shape emit back over the façades —
/// the real net/https / resolv / ipaddr are available there and the
/// vendored bodies run as written. Re-renders the library classes so
/// the restored bytes are exactly what the base would have emitted
/// without the swap.
pub(super) fn restore_extras_facades(files: &mut [(String, String)], app: &App) {
    for ef in emit_library_class_decls(app) {
        let p = ef.path.to_string_lossy().into_owned();
        if EXTRAS_FACADES
            .iter()
            .any(|f| p == format!("{}.rb", f.stem) || p == format!("{}.rbs", f.stem))
        {
            for (path, content) in files.iter_mut() {
                if *path == p {
                    content.clone_from(&ef.content);
                }
            }
        }
    }
}

/// Ruby-family pre-emit pass: a partial render in a LIBRARY-CLASS body
/// (lobsters' ApplicationHelper#link_post renders a partial from a
/// helper). A helper's render RETURNS the string, so the rewrite is the
/// bare `Views::<Mod>.<stem>(record, closure…, extras…)` call — locals
/// bind by name against the partial's contract, everything else nil
/// (a module body has no controller ivars to thread). Slashed partial
/// names only; a bare name has no module context here.
///
/// BOTH spellings Rails accepts: `render partial: "x/y", locals: {…}`
/// and the SHORTHAND `render "x/y"` (locals, if any, ride a trailing
/// hash). campfire's `MessagesHelper#message_tag` uses the shorthand in
/// a `rescue` body — `render "messages/unrenderable"` is what a message
/// row renders when building it raised — and unclaimed it emitted as a
/// bare `render` call, which is a method a helper module does not
/// have.
///
/// A name the locals do not bind falls back to the enclosing method's
/// own PARAMETER of that name before it falls back to nil. Same policy
/// as the controller-side rewrite ("a same-named local wins"), and here
/// it is what keeps the call type-correct: the partial's record
/// parameter is typed as its record (`unrenderable(Message message,
/// …)`), so a nil there is a seed contradiction that stops a strict
/// build. The divergence it buys is bounded and in the safe direction —
/// Rails supplies no local at all for a shorthand render, so a partial
/// that reads one raises there; ours renders with the caller's
/// same-named value.
pub(crate) fn apply_library_partial_render_lowering(lcs: &mut [LibraryClass], app: &App) {
    let contracts = crate::lower::view_to_library::partial_call_contracts(
        &app.views,
        &app.controllers,
        &app.library_classes,
    );
    if contracts.is_empty() {
        return;
    }
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            let in_scope: Vec<Symbol> = m.params.iter().map(|p| p.name.clone()).collect();
            rewrite_library_partial_render(&mut m.body, &contracts, &in_scope);
        }
    }
}

/// Symbol-keyed entries of a hash ARG, as partial locals. Rails lets a
/// shorthand render carry its locals in a trailing hash (`render
/// "messages/message", message: m`), which is the same map `locals:`
/// spells one level down.
fn hash_locals(arg: Option<&Expr>) -> Vec<(Symbol, Expr)> {
    use crate::expr::Literal;
    let Some(arg) = arg else { return Vec::new() };
    let ExprNode::Hash { entries, .. } = &*arg.node else { return Vec::new() };
    entries
        .iter()
        .filter_map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some((value.clone(), v.clone())),
            _ => None,
        })
        .collect()
}

fn rewrite_library_partial_render(
    expr: &mut Expr,
    contracts: &std::collections::HashMap<
        (String, String),
        crate::lower::view_to_library::PartialCallContract,
    >,
    in_scope: &[Symbol],
) {
    use crate::expr::Literal;
    expr.node
        .for_each_child_mut(&mut |c| rewrite_library_partial_render(c, contracts, in_scope));
    let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return };
    if method.as_str() != "render" && method.as_str() != "render_to_string" {
        return;
    }
    let Some(first) = args.first() else { return };
    let mut partial: Option<String> = None;
    let mut locals: Vec<(Symbol, Expr)> = Vec::new();
    match &*first.node {
        // The shorthand — the string IS the partial name.
        ExprNode::Lit { value: Literal::Str { value } } => {
            partial = Some(value.clone());
            locals = hash_locals(args.get(1));
        }
        ExprNode::Hash { entries, kwargs: true } => {
            for (k, v) in entries {
                let key = match &*k.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
                    _ => "",
                };
                match key {
                    "partial" => {
                        if let ExprNode::Lit { value: Literal::Str { value } } = &*v.node {
                            partial = Some(value.clone());
                        }
                    }
                    "locals" => {
                        if let ExprNode::Hash { entries: le, .. } = &*v.node {
                            for (lk, lv) in le {
                                if let ExprNode::Lit { value: Literal::Sym { value } } = &*lk.node {
                                    locals.push((value.clone(), lv.clone()));
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => return,
    }
    let Some(pname) = partial else { return };
    let Some((dir, base)) = pname.rsplit_once('/') else { return };
    let module_camel = crate::naming::camelize(&crate::naming::snake_case(dir));
    let stem = base.trim_start_matches('_').to_string();
    let Some(contract) = contracts.get(&(module_camel.clone(), stem.clone())) else { return };
    let span = expr.span;
    let nil = || sp_expr(ExprNode::Lit { value: Literal::Nil });
    let lookup = |name: &str| -> Option<Expr> {
        locals
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .map(|(_, v)| v.clone())
            .or_else(|| {
                in_scope.iter().find(|p| p.as_str() == name).map(|p| {
                    Expr::new(span, ExprNode::Var { id: VarId(0), name: p.clone() })
                })
            })
    };
    let mut view_args: Vec<Expr> = Vec::new();
    view_args.push(lookup(&contract.record).unwrap_or_else(nil));
    for n in &contract.closure {
        view_args.push(lookup(n).unwrap_or_else(nil));
    }
    let bound: Vec<Option<Expr>> = contract.extras.iter().map(|n| lookup(n)).collect();
    if let Some(last) = bound.iter().rposition(|b| b.is_some()) {
        for b in bound.into_iter().take(last + 1) {
            view_args.push(b.unwrap_or_else(nil));
        }
    }
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(sp_expr(ExprNode::Const {
                path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
            })),
            method: crate::lower::view::view_method_name(&stem),
            args: view_args,
            block: None,
            parenthesized: true,
        },
    );
}

/// Ruby-family pre-emit pass: synthesize each model's scope class methods
/// and normalize scope chains (`Story.base(u).positive_ranked` ->
/// `Story.positive_ranked(Story.base(u))`) across a group of library
/// classes, so they run against `ActiveRecord::Relation`. Lives on the
/// Ruby emit path — these methods reference a runtime only the CRuby/JRuby
/// tree provides, so the shared `lower/` must stay target-agnostic. Run
/// once per LC group (models / controllers / library_classes) before
/// rendering. A strict no-op for scope-free apps (the blog).
/// Generate `app/models/relation_scopes.rb` — an
/// `ActiveRecord::Relation` reopen delegating each declared model
/// scope to the model's fixed-arity `__scope_` relation entry points
/// (`push_scope_variants`), so a scope chained on a relation VALUE
/// (lobsters' StoriesPaginator: `@scope.limit(n).for_presentation`
/// where `@scope` is an untyped ctor param) resolves without static
/// receiver-type knowledge. One def per scope NAME serves every model
/// sharing it, whatever their arities: SCOPE_UNSET sentinels detect
/// how many positionals the caller supplied, and each arm forwards
/// that exact shape. Positional-only arms dispatch through `klass` —
/// every model's `__scope_<name>__<n>` entry has the same full arity
/// by construction, which is what spinel's class-value dispatch
/// requires (it neither defaults omitted optionals through the
/// dispatch nor accepts kwargs). Keyword-carrying arms dispatch
/// through `case klass.name` constant receivers instead. An argument
/// shape no model accepts raises ArgumentError at the call, as Rails
/// would. Statically resolvable — explicit defs, no method_missing,
/// no splats. Emitted under app/models/ so the aggregator loads it.
/// None when the app declares no scopes. A name that can't render
/// (rest param, too many keywords, non-tail optional positionals,
/// `?`/`!` name) is skipped with a `lower_residue` diagnostic and a
/// mid-chain call raises NoMethodError.
pub(crate) fn emit_relation_scope_delegates(app: &App) -> Option<EmittedFile> {
    // Names Relation itself defines never delegate — a scope named
    // like a builtin would already dispatch to the builtin under
    // Rails' merge semantics, and the reopen must not clobber the
    // query-builder surface.
    const RELATION_BUILTINS: &[&str] = &[
        "where", "not", "or", "order", "limit", "offset", "group", "having", "joins",
        "left_outer_joins", "left_joins", "select", "distinct", "includes", "preload",
        "find", "find_by", "first", "last", "all", "each", "map", "to_a", "count",
        "exists?", "empty?", "any?", "none?", "sum", "maximum", "minimum", "pluck",
        "pick", "destroy_all", "delete_all", "update_all", "klass", "where_clauses",
    ];
    let scopes = crate::lower::scope_chain::build_scope_registry(&app.models);
    // name -> [(model, params)] in app-model order, names sorted — the
    // generated file must be byte-stable across runs.
    let mut by_name: std::collections::BTreeMap<
        String,
        Vec<(&crate::ident::ClassId, &[crate::dialect::Param])>,
    > = Default::default();
    for model in &app.models {
        let Some(per) = scopes.get(&model.name) else { continue };
        // The registry carries the SYNTHESIZED preload scopes too (it
        // must — the scope-body rewriter reads it to thread `__rel`
        // through a bare `with_attached_attachment`). They are handled
        // by the identity arm below, which is both faster and clearer
        // than a `__scope_` hop to a body that returns its argument;
        // leaving them here would make `by_name` claim them and the
        // identity arm skip them as "a declared scope of the same name".
        let synthesized: std::collections::HashSet<String> =
            crate::lower::rich_text::preload_scope_names(model)
                .into_iter()
                .chain(crate::lower::attached::preload_scope_names(model))
                .map(|n| n.as_str().to_string())
                .collect();
        let mut names: Vec<&Symbol> = per.keys().collect();
        names.sort_by_key(|n| n.as_str());
        for n in names {
            if RELATION_BUILTINS.contains(&n.as_str()) || synthesized.contains(n.as_str()) {
                continue;
            }
            by_name
                .entry(n.as_str().to_string())
                .or_default()
                .push((&model.name, per[n].as_slice()));
        }
    }
    // The SYNTHESIZED preload scopes — `with_attached_<attr>` and
    // `with_rich_text_<attr>` — which Rails declares beside the
    // attachment macro and this compiler adds at emit time
    // (`attached::push_preload_scope_methods` and its rich-text twin).
    // They never pass through `build_scope_registry`, which reads the
    // app's own `scope` declarations, so a call CHAINED ON A RELATION
    // had no delegate at all: campfire's
    // `find_autocompletable_users.with_attached_avatar.ordered` is a
    // NoMethodError on a class method that plainly exists, because the
    // receiver is a relation value and not the class.
    //
    // The delegate is `self`, with no `__scope_` dispatch behind it,
    // because these scopes ARE identity — Rails' `includes(...)` is a
    // query-plan hint and the per-record readers this compiler
    // synthesizes have nothing for it to attach to (the class-side
    // bodies say the same thing by returning `__rel`). So there is no
    // arity to detect and no model to pick: every model that declares
    // the attachment answers the relation unchanged, and a model that
    // does not never has the name reached on it.
    let mut identity: std::collections::BTreeSet<String> = Default::default();
    for model in &app.models {
        let names = crate::lower::rich_text::preload_scope_names(model)
            .into_iter()
            .chain(crate::lower::attached::preload_scope_names(model));
        for n in names {
            let n = n.as_str().to_string();
            // A declared scope of the same name wins — it has a real
            // body, and shadowing it with identity would drop a filter.
            // A Relation builtin is never delegated, same rule as above.
            if by_name.contains_key(&n) || RELATION_BUILTINS.contains(&n.as_str()) {
                continue;
            }
            identity.insert(n);
        }
    }
    if by_name.is_empty() && identity.is_empty() {
        return None;
    }
    let mut skipped: Vec<String> = Vec::new();
    let mut body = String::new();
    for (name, decls) in &by_name {
        match render_scope_delegate(name, decls) {
            Ok(text) => body.push_str(&text),
            Err(reason) => {
                push_delegate_skip_diagnostic(name, &reason, decls);
                skipped.push(name.clone());
            }
        }
    }
    for name in &identity {
        body.push_str("\n    # Preload scope (Rails' `includes`) — identity here, so\n");
        body.push_str("    # the relation passes through unchanged.\n");
        writeln!(body, "    def {name}").unwrap();
        body.push_str("      self\n    end\n");
    }
    let mut s = String::from(
        "# Generated Relation scope delegation (see\n\
         # emit_relation_scope_delegates): each model scope, callable on a\n\
         # relation value mid-chain, forwarding the caller's exact argument\n\
         # shape to the model's fixed-arity __scope_ entry — SCOPE_UNSET\n\
         # sentinels detect how many positionals were supplied.\n",
    );
    if !skipped.is_empty() {
        writeln!(
            s,
            "# No delegate (see the lower_residue diagnostics) — a mid-chain\n\
             # call raises NoMethodError: {}",
            skipped.join(", ")
        )
        .unwrap();
    }
    s.push_str("module ActiveRecord\n\x20\x20class Relation\n");
    s.push_str(
        "\x20\x20\x20\x20# Argument-omitted sentinel for the delegates below:\n\
         \x20\x20\x20\x20# distinguishes an omitted optional from every real value\n\
         \x20\x20\x20\x20# including nil. Compared with equal?, never ==.\n\
         \x20\x20\x20\x20SCOPE_UNSET = Object.new\n",
    );
    s.push_str(&body);
    s.push_str("  end\nend\n");
    Some(EmittedFile {
        path: PathBuf::from("app/models/relation_scopes.rb"),
        content: s,
    })
}

/// Everything the arm renderers need about one delegate name.
struct DelegateCtx<'a> {
    name: &'a Symbol,
    /// Display names for the delegate's positional params.
    pos_names: &'a [String],
    /// Every model declaring the name, with its admissible shape.
    shapes: &'a [(&'a crate::ident::ClassId, crate::lower::scope_chain::DelegableShape<'a>)],
    /// Max positional count across the shapes.
    n_max: usize,
}

/// One delegate def for `name` across every model declaring it, or the
/// reason it can't render (fed to the skip diagnostic).
fn render_scope_delegate(
    name: &str,
    decls: &[(&crate::ident::ClassId, &[crate::dialect::Param])],
) -> Result<String, String> {
    use crate::lower::scope_chain::{delegable_name, DelegableShape, MAX_DELEGATE_KEYWORDS};
    let name_sym = Symbol::from(name);
    if !delegable_name(&name_sym) {
        return Err("`?`/`!` names take no __scope_ arity mangling".into());
    }
    let mut shapes: Vec<(&crate::ident::ClassId, DelegableShape)> = Vec::new();
    for (model, params) in decls {
        let Some(shape) = DelegableShape::of(params) else {
            return Err(format!(
                "shape on {} has a rest param, more than {MAX_DELEGATE_KEYWORDS} keywords, \
                 or optional positionals not forming a tail",
                model.0.as_str()
            ));
        };
        shapes.push((*model, shape));
    }
    let n_max = shapes
        .iter()
        .map(|(_, s)| s.positionals.len())
        .max()
        .unwrap_or(0);
    // Keyword union across models, sorted by name — subset and variant
    // naming must line up with keyword_subsets/scope_variant_name.
    let mut kw_union: Vec<&crate::dialect::Param> = Vec::new();
    for (_, s) in &shapes {
        for kw in &s.keywords {
            if !kw_union.iter().any(|k| k.name == kw.name) {
                kw_union.push(kw);
            }
        }
    }
    kw_union.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    if kw_union.len() > MAX_DELEGATE_KEYWORDS {
        return Err(format!(
            "keyword union across models exceeds {MAX_DELEGATE_KEYWORDS}"
        ));
    }
    if kw_union.iter().any(|k| k.name.as_str() == "klass") {
        return Err("a keyword named `klass` would shadow Relation#klass".into());
    }
    // Positional display names: the models' own param name where every
    // shape agrees at that position, `a<i>` otherwise (or when the name
    // would shadow Relation#klass in the forward).
    let pos_names: Vec<String> = (0..n_max)
        .map(|i| {
            let mut names = shapes
                .iter()
                .filter_map(|(_, s)| s.positionals.get(i).map(|p| p.name.as_str()));
            let n = match names.next() {
                Some(first) if names.all(|other| other == first) => first.to_string(),
                _ => format!("a{i}"),
            };
            if n == "klass" { format!("a{i}") } else { n }
        })
        .collect();
    let mut s = String::new();
    let mut decl: Vec<String> = pos_names
        .iter()
        .map(|n| format!("{n} = SCOPE_UNSET"))
        .collect();
    for kw in &kw_union {
        decl.push(format!("{}: SCOPE_UNSET", kw.name.as_str()));
    }
    if decl.is_empty() {
        writeln!(s, "\x20\x20\x20\x20def {name}").unwrap();
    } else {
        writeln!(s, "\x20\x20\x20\x20def {name}({})", decl.join(", ")).unwrap();
    }
    let ctx = DelegateCtx { name: &name_sym, pos_names: &pos_names, shapes: &shapes, n_max };
    render_kw_tree(&mut s, 3, &kw_union, &mut Vec::new(), &ctx);
    writeln!(s, "\x20\x20\x20\x20end").unwrap();
    Ok(s)
}

/// Branch on which of the delegate's keywords the caller supplied
/// (each independently sentinel-checked), then render the arity arms
/// for that exact keyword subset at the leaf.
fn render_kw_tree<'a>(
    out: &mut String,
    level: usize,
    remaining: &[&'a crate::dialect::Param],
    selected: &mut Vec<&'a crate::dialect::Param>,
    ctx: &DelegateCtx,
) {
    let pad = "\x20\x20".repeat(level);
    match remaining.split_first() {
        None => {
            let mut subset = selected.clone();
            subset.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
            render_arity_arms(out, level, &subset, ctx);
        }
        Some((kw, rest)) => {
            writeln!(out, "{pad}if SCOPE_UNSET.equal?({})", kw.name.as_str()).unwrap();
            render_kw_tree(out, level + 1, rest, selected, ctx);
            writeln!(out, "{pad}else").unwrap();
            selected.push(kw);
            render_kw_tree(out, level + 1, rest, selected, ctx);
            selected.pop();
            writeln!(out, "{pad}end").unwrap();
        }
    }
}

/// The supplied-arity arms for one keyword subset: arm `n` fires when
/// `a<n>` is the first unset positional (guards run in ascending order,
/// each returning or raising). Positional-only arms go through `klass`;
/// keyword arms need constant receivers (`case klass.name`) because
/// kwargs don't survive spinel's class-value dispatch.
fn render_arity_arms(
    out: &mut String,
    level: usize,
    subset: &[&crate::dialect::Param],
    ctx: &DelegateCtx,
) {
    use crate::lower::scope_chain::scope_variant_name;
    let pad = "\x20\x20".repeat(level);
    let kw_list = subset
        .iter()
        .map(|k| format!("{}:", k.name.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    for n in 0..=ctx.n_max {
        // Models whose shape accepts exactly this call: arity within
        // the positional range, subset within the declared keywords,
        // every required keyword supplied.
        let eligible: Vec<&crate::ident::ClassId> = ctx
            .shapes
            .iter()
            .filter(|(_, s)| {
                n >= s.min_required
                    && n <= s.positionals.len()
                    && subset
                        .iter()
                        .all(|kw| s.keywords.iter().any(|d| d.name == kw.name))
                    && s.required_keywords()
                        .iter()
                        .all(|r| subset.iter().any(|kw| &kw.name == *r))
            })
            .map(|(m, _)| *m)
            .collect();
        let guard = (n < ctx.n_max)
            .then(|| format!(" if SCOPE_UNSET.equal?({})", ctx.pos_names[n]))
            .unwrap_or_default();
        let pos_args: String = ctx.pos_names[..n]
            .iter()
            .map(|p| format!(", {p}"))
            .collect();
        if subset.is_empty() {
            let line = if eligible.is_empty() {
                format!(
                    "raise ArgumentError, \"scope {} does not accept {n} positional argument(s)\"",
                    ctx.name.as_str()
                )
            } else {
                format!(
                    "return klass.{}(self{pos_args})",
                    scope_variant_name(ctx.name, n, &[]).as_str()
                )
            };
            writeln!(out, "{pad}{line}{guard}").unwrap();
        } else if eligible.is_empty() {
            writeln!(
                out,
                "{pad}raise ArgumentError, \"scope {} does not accept ({kw_list}) with {n} \
                 positional argument(s)\"{guard}",
                ctx.name.as_str()
            )
            .unwrap();
        } else {
            let kw_args: String = subset
                .iter()
                .map(|kw| format!(", {0}: {0}", kw.name.as_str()))
                .collect();
            let variant = scope_variant_name(ctx.name, n, subset);
            let inner = if n < ctx.n_max {
                writeln!(out, "{pad}if SCOPE_UNSET.equal?({})", ctx.pos_names[n]).unwrap();
                level + 1
            } else {
                level
            };
            let ipad = "\x20\x20".repeat(inner);
            writeln!(out, "{ipad}case klass.name").unwrap();
            for m in &eligible {
                writeln!(
                    out,
                    "{ipad}when \"{0}\" then return {0}.{1}(self{pos_args}{kw_args})",
                    m.0.as_str(),
                    variant.as_str()
                )
                .unwrap();
            }
            writeln!(
                out,
                "{ipad}else raise ArgumentError, \"scope {} with ({kw_list}) unavailable on \
                 #{{klass.name}}\"",
                ctx.name.as_str()
            )
            .unwrap();
            writeln!(out, "{ipad}end").unwrap();
            if n < ctx.n_max {
                writeln!(out, "{pad}end").unwrap();
            }
        }
    }
}

/// Ledger a name the delegate emitter skipped: modeling debt, not an
/// app error — `lower_residue`, Warning severity, one per name.
fn push_delegate_skip_diagnostic(
    name: &str,
    reason: &str,
    decls: &[(&crate::ident::ClassId, &[crate::dialect::Param])],
) {
    use crate::diagnostic::{Diagnostic, DiagnosticKind};
    let models = decls
        .iter()
        .map(|(m, _)| m.0.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from("relation_scope_delegates"),
        construct: Symbol::from(name),
        reason: Symbol::from(reason),
    };
    let d = Diagnostic {
        span: crate::span::Span::synthetic(),
        severity: Diagnostic::default_severity(&kind),
        kind,
        message: format!(
            "scope `{name}` (on {models}) gets no ActiveRecord::Relation mid-chain \
             delegate: {reason}; a mid-chain call raises NoMethodError"
        ),
    };
    crate::emit::diagnostics::push(d);
}

/// A class method a call site reaches through an association, whose
/// constructor cannot take the association's scope. Rails would have set
/// the foreign key here; we leave the call resolving against the folded
/// Array, which fails loudly, and say so.
fn push_assoc_scope_skip(model: &crate::ident::ClassId, method: &Symbol, reason: &str) {
    use crate::diagnostic::{Diagnostic, DiagnosticKind};
    let model = model.0.as_str();
    let name = method.as_str();
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from("assoc_class_method_scope"),
        construct: Symbol::from(format!("{model}.{name}")),
        reason: Symbol::from(reason),
    };
    let d = Diagnostic {
        span: crate::span::Span::synthetic(),
        severity: Diagnostic::default_severity(&kind),
        kind,
        message: format!(
            "`{model}.{name}` is called through an association but cannot take its scope: \
             {reason}; the foreign key Rails would preset is not set"
        ),
    };
    crate::emit::diagnostics::push(d);
}

/// Give `m` the trailing `__rel = ActiveRecord::Relation.new(self)` that
/// makes it relation-taking — in the params AND in the signature the
/// `.rbs` is written from.
///
/// The signature half is not optional: a body the typer could resolve
/// carries a `Ty::Fn`, and the emitted `.rbs` prefers it over the
/// params, so inserting on one side only publishes `() -> bool` for a
/// method every call site now passes a relation to. Placement is before
/// the first keyword in both, since `def f(__rel = …, k:)` is the only
/// legal ordering.
fn insert_rel_param(m: &mut crate::dialect::MethodDef, rel_param: &Symbol) {
    let insert_at = m.params.iter().position(|p| p.keyword).unwrap_or(m.params.len());
    m.params.insert(
        insert_at,
        crate::dialect::Param::with_default(
            rel_param.clone(),
            crate::lower::model_to_library::relation_new_self(),
        ),
    );
    if let Some(crate::ty::Ty::Fn { params, .. }) = &mut m.signature {
        let at = params
            .iter()
            .position(|p| matches!(p.kind, crate::ty::ParamKind::Keyword { .. }))
            .unwrap_or(params.len());
        params.insert(
            at,
            crate::ty::Param {
                name: rel_param.clone(),
                ty: crate::ty::Ty::Untyped,
                kind: crate::ty::ParamKind::Optional,
            },
        );
    }
}

pub(crate) fn apply_scope_lowering(lcs: &mut [LibraryClass], app: &App) {
    // `has_rich_text`'s two preload scopes, and `has_one_attached`'s
    // one. Ahead of the `any_scopes` early return below, because an app
    // can declare a rich-text attribute or an attachment and no `scope`
    // at all — and these still have to exist or every call site
    // chaining through them is a NoMethodError.
    // `attachable_sgid` for the models that mix in
    // `ActionText::Attachable` (campfire declares it one level down,
    // through `User::Mentionable`). Ruby-family only, like the
    // `MessageVerifier` the mint runs through.
    let attachable = crate::lower::attachable::attachable_models(app);
    for lc in lcs.iter_mut() {
        if let Some(model) = app.models.iter().find(|m| m.name == lc.name) {
            crate::lower::rich_text::push_preload_scope_methods(&mut lc.methods, model);
            crate::lower::attached::push_preload_scope_methods(&mut lc.methods, model);
            crate::lower::attachable::push_attachable_sgid(&mut lc.methods, model, &attachable);
        }
    }
    let scopes = crate::lower::scope_chain::build_scope_registry(&app.models);
    let assocs = crate::lower::scope_chain::build_assoc_registry(&app.models);
    // Class methods reached THROUGH an association (`user.sessions
    // .start!`) — demand-gated on the call sites this app actually
    // writes, so an app without that shape emits exactly what it did
    // before. Surveyed over the whole App, not over `lcs`: this pass
    // runs once for models and again for controllers, and the demand
    // for `Session.start!` lives in a CONTROLLER while the parameter
    // has to be inserted on the MODEL.
    let (assoc_class_methods, declined) =
        crate::lower::scope_chain::survey_assoc_class_methods(app, &assocs, &scopes);
    // Reported by the pass that owns the model's own file — this runs
    // once per emitted family over a different `lcs`, and the ledger
    // line should appear once, beside the class it is about.
    for d in &declined {
        if lcs.iter().any(|lc| lc.name == d.model) {
            push_assoc_scope_skip(&d.model, &d.method, &d.reason);
        }
    }
    let models = crate::lower::scope_chain::model_set(&app.models);
    // …and it can call a terminal that has no home on the model CLASS
    // (`Push::Subscription.destroy_by(…)`), which reaches nothing at all
    // without the seed this pass writes. Unlike the three conditions
    // above it is not a question about a REGISTRY — an app with not one
    // scope in it can still write that call — so it is surveyed over the
    // app's own bodies.
    let mut wants_class_root_terminal = false;
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        wants_class_root_terminal = wants_class_root_terminal
            || crate::lower::scope_chain::mentions_class_root_terminal(body, &models);
    });
    if !crate::lower::scope_chain::any_scopes(&scopes)
        && assoc_class_methods.is_empty()
        // An app with no scopes at all can still declare an association
        // extension, and its call sites need the same rewrite.
        && !crate::lower::scope_chain::any_assoc_extensions(&assocs)
        && !wants_class_root_terminal
    {
        return;
    }
    let names = crate::lower::scope_chain::all_scope_names(&scopes);
    let user_returns = crate::lower::scope_chain::build_user_method_returns(&app.models);
    let unique_keys = crate::lower::scope_chain::build_unique_keys(&app.models, &app.schema);
    let regs = crate::lower::scope_chain::Registries {
        scopes: &scopes,
        models: &models,
        assocs: &assocs,
        assoc_class_methods: &assoc_class_methods,
        user_returns: &user_returns,
        unique_keys: &unique_keys,
    };
    for lc in lcs.iter_mut() {
        // Models gain their scope class methods (already chain-normalized).
        let is_model = app.models.iter().any(|m| m.name == lc.name);
        // A model CONCERN is a model body too — the third family
        // `lower::for_each_model_body` names. `module User::Bannable`'s
        // instance methods run on a `User`, so a bare `bans.create!` in
        // one is the same association read it would be in user.rb, and
        // resolving it needs the same owner. Without this the concern's
        // body reached `rewrite_call_site` with NO self model and the
        // reader's folded Array kept the call: `undefined method
        // 'create!' for an instance of Array`, which is what campfire's
        // `User::Bannable#create_bans_from_sessions` raised.
        //
        // MODULES ONLY, and only when the namespace names a model: a
        // plain nested class under a model namespace is its own
        // receiver, not the model's.
        let concern_owner: Option<crate::ident::ClassId> = (!is_model && lc.is_module)
            .then(|| lc.name.0.as_str().rsplit_once("::").map(|(ns, _)| ns.to_string()))
            .flatten()
            .map(|ns| crate::ident::ClassId(crate::ident::Symbol::from(ns.as_str())))
            .filter(|ns| app.models.iter().any(|m| &m.name == ns))
            // An STI SUBCLASS is the same fact by inheritance rather
            // than by namespace: `Rooms::Open < Room` declares no table,
            // so it is not a model, but its instance methods run on a
            // Room — and `memberships.grant_to(...)` in one is the same
            // association-extension call it would be in room.rb. Without
            // this it reached `rewrite_call_site` with no self model and
            // kept the un-flattened `memberships.grant_to`, which
            // nothing defines.
            .or_else(|| {
                if is_model {
                    return None;
                }
                let mut cursor = lc.parent.clone();
                for _ in 0..8 {
                    let parent = cursor?;
                    if app.models.iter().any(|m| m.name == parent) {
                        return Some(parent);
                    }
                    cursor = app
                        .library_classes
                        .iter()
                        .find(|other| other.name == parent)
                        .and_then(|other| other.parent.clone());
                }
                None
            });
        if let Some(model) = app.models.iter().find(|m| m.name == lc.name) {
            crate::lower::model_to_library::push_scope_methods(
                &mut lc.methods,
                model,
                &scopes,
                &models,
                &assocs,
            );
        }
        // User-written class methods the registry admitted as
        // relation-taking (`def self.arrange_for_user` with bare
        // `order(...)` roots) get the same treatment push_scope_methods
        // gives declared scopes: a trailing `__rel =
        // ActiveRecord::Relation.new(self)` param, and their bare chain
        // roots threaded through it. Skip-if-last-param-is-__rel makes
        // this idempotent AND distinguishes user methods from the
        // already-synthesized scope methods.
        if is_model {
            let rel_param = Symbol::from("__rel");
            let registered: Vec<Symbol> = scopes
                .get(&lc.name)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            for m in &mut lc.methods {
                if m.receiver == MethodReceiver::Class
                    && registered.contains(&m.name)
                    // Any-position check: push_scope_methods inserts
                    // __rel BEFORE keyword params, so a kwarg-taking
                    // scope's __rel is not last.
                    && !m.params.iter().any(|p| p.as_str() == "__rel")
                {
                    insert_rel_param(m, &rel_param);
                    crate::lower::scope_chain::rewrite_scope_body(
                        &mut m.body,
                        &lc.name,
                        &rel_param,
                        &scopes,
                        &models,
                        &assocs,
                    );
                }
            }
            // The association-scope side of the same idea: a class
            // method some call site reaches through a has_many takes
            // the relation the same way, and reads it in one or both of
            // two ways. As a CREATE scope — `create!(attrs)` becomes
            // `create!(__rel.scope_attributes.merge(attrs))`, which is
            // how Rails' `scope_for_create` gets `user_id` onto the row.
            // And as a QUERY scope — a bare `count` / `page_before(m)`
            // roots on `__rel` exactly as it would inside a scope body,
            // which is what makes `@room.messages.paged?` count THIS
            // room. The default `Relation.new(self)` is empty and
            // records no scope attributes, so a direct `Session.start!`
            // / `Message.paged?` is unchanged.
            let assoc_registered: Vec<(Symbol, bool, bool)> = assoc_class_methods
                .get(&lc.name)
                .map(|m| m.iter().map(|(n, e)| (n.clone(), e.creates, e.queries)).collect())
                .unwrap_or_default();
            for m in &mut lc.methods {
                let Some(&(_, creates, queries)) = assoc_registered
                    .iter()
                    .find(|(n, _, _)| *n == m.name)
                else {
                    continue;
                };
                if m.receiver != MethodReceiver::Class
                    || m.params.iter().any(|p| p.as_str() == "__rel")
                {
                    continue;
                }
                insert_rel_param(m, &rel_param);
                if creates {
                    crate::lower::scope_chain::merge_scope_attributes(
                        &mut m.body,
                        &lc.name,
                        &rel_param,
                    );
                }
                if queries {
                    crate::lower::scope_chain::rewrite_assoc_scope_body(
                        &mut m.body,
                        &lc.name,
                        &rel_param,
                        &scopes,
                        &models,
                        &assocs,
                    );
                }
            }
        }
        // Every method body: normalize scope chains (call-site form).
        // Scope-free bodies still need the rewrite when they start a
        // query chain on a model constant — the arel inline pass bails
        // on dynamic-value where-hashes, and those chains only run
        // against a seeded Relation. A model's own CLASS methods
        // additionally seed bare implicit-self roots (`where(key: key)`
        // in `Keystore.value_for`), signalled via `class_self`.
        for m in &mut lc.methods {
            // A `__scope_` entry's forward body already carries `__rel`
            // exactly once — never re-thread it (re-entrancy guard for
            // a second apply_scope_lowering over the same lcs).
            if m.name.as_str().starts_with("__scope_") {
                continue;
            }
            let class_self = (m.receiver == MethodReceiver::Class)
                .then(|| if is_model { Some(lc.name.clone()) } else { concern_owner.clone() })
                .flatten();
            // A model's own INSTANCE methods know their self model too —
            // `self.<has_many>.<scope>` there seeds a Relation from the
            // association's foreign key (recent_threads' comment chain).
            let instance_self = (m.receiver == MethodReceiver::Instance)
                .then(|| if is_model { Some(lc.name.clone()) } else { concern_owner.clone() })
                .flatten();
            if crate::lower::scope_chain::mentions_scope(&m.body, &names)
                || crate::lower::scope_chain::mentions_model_chain_start(&m.body, &models)
                || crate::lower::scope_chain::mentions_assoc_constructor(&m.body, &assocs)
                || crate::lower::scope_chain::mentions_assoc_lookup(&m.body, &assocs)
                || crate::lower::scope_chain::mentions_assoc_alias(&m.body, &assocs)
                || crate::lower::scope_chain::mentions_assoc_extension(&m.body, &assocs)
                || crate::lower::scope_chain::mentions_model_insert_all(&m.body, &models)
                || crate::lower::scope_chain::mentions_assoc_class_method(
                    &m.body,
                    &assocs,
                    &scopes,
                    &assoc_class_methods,
                )
                || (class_self.is_some()
                    && crate::lower::scope_chain::mentions_bare_chain_start(&m.body))
            {
                crate::lower::scope_chain::rewrite_call_site(
                    &mut m.body,
                    &regs,
                    class_self.as_ref(),
                    instance_self.as_ref(),
                );
            }
        }
    }
    // Fixed-arity relation entry points, generated AFTER every body
    // rewrite above: their forward bodies mention scope names on model
    // constants, which `rewrite_call_site` would re-thread if it saw
    // them. Placement here (not inside push_scope_methods) is
    // load-bearing for the same reason.
    for lc in lcs.iter_mut() {
        if let Some(per_model) = scopes.get(&lc.name) {
            if !per_model.is_empty() {
                crate::lower::model_to_library::push_scope_variants(
                    &mut lc.methods,
                    &lc.name,
                    per_model,
                );
            }
        }
    }
}

/// Ruby-family pre-emit pass: a `belongs_to` reader MEMOIZES.
///
/// Rails loads a belongs_to target once and hands back the same object
/// on every later read. Ours re-queried, so two reads of
/// `membership.user` were two different objects — and campfire's
/// `@membership.user.expects :reset_remote_connections` stubbed one
/// while the `after_destroy_commit` callback called the other. The
/// callback fired correctly; the expectation was watching an object
/// nothing would ever call.
///
/// ```ruby
/// def user
///   return @user_cache if @user_loaded
///   @user_cache = (… the query …)
///   @user_loaded = true
///   @user_cache
/// end
///
/// def user_id=(value)
///   @user_id = value
///   @user_loaded = false        # <- the other half
/// end
/// ```
///
/// THE INVALIDATION IS NOT OPTIONAL. Rails resets the loaded target
/// when the foreign key is written directly, and a memoizing reader
/// without it answers `membership.user` from an object the key no
/// longer points at. `apply_belongs_to_autosave` above declined to
/// populate the cache for SAVED targets precisely because that reset
/// did not exist yet; with it here, the reader can cache every read and
/// the hazard that note describes is closed.
///
/// Ruby-family only, and for a reason the shared lowering states in
/// `synth_has_many_reader`: a reader that writes an ivar is no longer
/// read-only, so Rust would emit `&mut self` and every immutable caller
/// — views iterating a collection and reading each record's
/// association — would stop borrowing. The strict targets keep the
/// re-query, which is correct, just not identity-preserving.
pub(crate) fn apply_belongs_to_memoization(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        let names: Vec<(Symbol, Symbol)> = model
            .associations()
            .filter_map(|a| match a {
                Association::BelongsTo { name, foreign_key, polymorphic: false, .. } => {
                    Some((name.clone(), foreign_key.clone()))
                }
                _ => None,
            })
            .collect();
        for (name, fk) in names {
            memoize_reader(lc, &name);
            invalidate_on_fk_write(lc, &name, &fk);
        }
    }
}

/// `Seq[guard, query]` -> `Seq[guard, @cache = query, @loaded = true, @cache]`.
///
/// Splitting on the guard rather than reaching into the query is what
/// keeps this robust: the reader's second half differs per association
/// (a `Db.prepare` for most, a preloaded read for others) and this pass
/// does not need to know which.
fn memoize_reader(lc: &mut LibraryClass, name: &Symbol) {
    let cache = Symbol::from(format!("{}_cache", name.as_str()));
    let loaded = Symbol::from(format!("{}_loaded", name.as_str()));
    let Some(m) = lc
        .methods
        .iter_mut()
        .find(|m| &m.name == name && m.receiver == MethodReceiver::Instance)
    else {
        return;
    };
    let ExprNode::Seq { exprs } = &mut *m.body.node else { return };
    // Exactly the synthesized shape: the loaded-guard, then one
    // expression that produces the record. Anything else is a
    // hand-written reader and is left alone.
    if exprs.len() != 2 {
        return;
    }
    let guard_matches = matches!(&*exprs[0].node,
        ExprNode::If { cond, .. } if matches!(&*cond.node, ExprNode::Ivar { name } if name == &loaded));
    if !guard_matches {
        return;
    }
    let span = exprs[1].span;
    let query = exprs[1].clone();
    let syn = |n| Expr::new(span, n);
    *exprs = vec![
        exprs[0].clone(),
        syn(ExprNode::Assign { target: LValue::Ivar { name: cache.clone() }, value: query }),
        syn(ExprNode::Assign {
            target: LValue::Ivar { name: loaded },
            value: syn(ExprNode::Lit { value: Literal::Bool { value: true } }),
        }),
        syn(ExprNode::Ivar { name: cache }),
    ];
}

/// `def <fk>=(value); @<fk> = value; end` gains `@<name>_loaded = false`.
fn invalidate_on_fk_write(lc: &mut LibraryClass, name: &Symbol, fk: &Symbol) {
    let writer = Symbol::from(format!("{}=", fk.as_str()));
    let Some(m) = lc
        .methods
        .iter_mut()
        .find(|m| m.name == writer && m.receiver == MethodReceiver::Instance)
    else {
        return;
    };
    let span = m.body.span;
    let reset = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
            value: Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: false } }),
        },
    );
    let mut stmts = match &*m.body.node {
        ExprNode::Seq { exprs } => exprs.clone(),
        _ => vec![m.body.clone()],
    };
    stmts.push(reset);
    m.body = Expr::new(span, ExprNode::Seq { exprs: stmts });
}

/// Ruby-family pre-emit pass: an STI row hydrates as its SUBCLASS.
///
/// `Room.first.open?` was false for a row whose `type` column says
/// `"Rooms::Open"`. Both hydration entry points build the BASE class
/// unconditionally, so `open?` — which campfire writes as
/// `is_a?(Rooms::Open)` — asked a question the object could never
/// answer yes to.
///
/// ```ruby
/// instance = case row.type
///            when "Rooms::Open" then Rooms::Open.new
///            when "Rooms::Closed" then Rooms::Closed.new
///            else Room.new
///            end
/// ```
///
/// BOTH entry points, and that is the part that cost a probe. A model
/// has two: `from_stmt` reads the typed `Db.column_*` path, `from_row`
/// reads a Row through `instantiate`. `Room.first` goes to `to_a` ->
/// `instantiate` -> `from_row`; patching only `from_stmt` moved
/// nothing, exactly as adding `Request#head?` to one of the two
/// Requests moved nothing. When a runtime concept has two doors,
/// measure which one the failing call uses.
///
/// The scrutinee is CLONED off the body's own `instance.type = <expr>`
/// assignment rather than rebuilt: `from_stmt` reads
/// `Db.column_text(stmt, 4)` and `from_row` reads `row.type`, and the
/// column INDEX in the first is a fact only that body knows.
///
/// Ruby-family only, like its neighbours. A strict target's `Rooms::
/// Open` is not a subtype of `Room` — there is no inheritance to make
/// the case arms share a return type — so this is not expressible
/// there, and those targets keep hydrating the base class.
pub(crate) fn apply_sti_hydration(lcs: &mut [LibraryClass], app: &App) {
    // subclass -> base, from the one authority on the question.
    let bases = crate::lower::sti_bases(app);
    if bases.is_empty() {
        return;
    }
    let mut subs_of: HashMap<ClassId, Vec<ClassId>> = HashMap::new();
    for (sub, base) in &bases {
        subs_of.entry(base.clone()).or_default().push(sub.clone());
    }
    // Deterministic arm order — the map's iteration order is not.
    for v in subs_of.values_mut() {
        v.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    }
    for lc in lcs.iter_mut() {
        let Some(subs) = subs_of.get(&lc.name) else { continue };
        let base = lc.name.clone();
        for m in lc.methods.iter_mut() {
            if !matches!(m.name.as_str(), "from_stmt" | "from_row") {
                continue;
            }
            if m.receiver != crate::dialect::MethodReceiver::Class {
                continue;
            }
            let ExprNode::Seq { exprs } = &mut *m.body.node else { continue };
            // The column writes are SENDS (`instance.type=(row.type)`),
            // not `Assign { Attr }` — the model lowerer routes every
            // hydration write through the typed writer method. Both
            // spellings are matched anyway: this pass reads the body a
            // sibling pass wrote, and pinning it to one shape is how a
            // reader goes stale.
            let Some(type_read) = exprs.iter().find_map(|e| match &*e.node {
                ExprNode::Send { recv: Some(_), method, args, .. }
                    if method.as_str() == "type=" && args.len() == 1 =>
                {
                    Some(args[0].clone())
                }
                ExprNode::Assign { target: LValue::Attr { name, .. }, value }
                    if name.as_str() == "type" =>
                {
                    Some(value.clone())
                }
                _ => None,
            }) else {
                continue;
            };
            for e in exprs.iter_mut() {
                let ExprNode::Assign { target: LValue::Var { .. }, value } = &mut *e.node else {
                    continue;
                };
                let is_base_new = matches!(&*value.node,
                    ExprNode::Send { recv: Some(r), method, args, .. }
                        if method.as_str() == "new"
                            && args.is_empty()
                            && matches!(&*r.node, ExprNode::Const { path }
                                if const_path_is(path, &base)));
                if !is_base_new {
                    continue;
                }
                *value = sti_dispatch(value.clone(), &type_read, subs, value.span);
                break;
            }
        }
    }
}

fn const_path_is(path: &[Symbol], id: &ClassId) -> bool {
    let joined: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
    joined.join("::") == id.0.as_str()
}

/// `case <type_read> when "<Sub>" then <Sub>.new … else <base_new> end`
fn sti_dispatch(base_new: Expr, type_read: &Expr, subs: &[ClassId], span: Span) -> Expr {
    let mut arms: Vec<crate::expr::Arm> = subs
        .iter()
        .map(|sub| crate::expr::Arm {
            pattern: crate::expr::Pattern::Lit {
                value: Literal::Str { value: sub.0.as_str().to_string() },
            },
            guard: None,
            body: Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(Expr::new(
                        span,
                        ExprNode::Const {
                            path: sub.0.as_str().split("::").map(Symbol::from).collect(),
                        },
                    )),
                    method: Symbol::from("new"),
                    args: vec![],
                    block: None,
                    parenthesized: true,
                },
            ),
        })
        .collect();
    // `else` — a row whose type names no known subclass (or is empty)
    // stays the base class, which is what Rails does for a blank
    // inheritance column.
    arms.push(crate::expr::Arm {
        pattern: crate::expr::Pattern::Wildcard,
        guard: None,
        body: base_new,
    });
    Expr::new(span, ExprNode::Case { scrutinee: type_read.clone(), arms })
}

/// Ruby-family pre-emit pass: `belongs_to` autosave.
///
/// `room.creator = User.new(attrs); room.save!` is ordinary Rails —
/// the unsaved creator is saved with its owner and the foreign key
/// follows. The shared writer stores `value.id` and nothing else, so
/// the key stayed 0 and campfire's whole first run died on
/// `Validation failed: Creator must exist` (four tests, two files).
///
/// Two edits per non-polymorphic belongs_to:
///
///   1. the writer stashes an UNSAVED value in the reader's cache —
///      a record with no id yet cannot be found again from the foreign
///      key, so something has to hold it until the save;
///   2. `_autosave_<name>` saves it and takes the id, folded into
///      `before_validation`.
///
/// ```ruby
/// def creator=(value)
///   if value.nil?
///     @creator_id = 0
///   else
///     @creator_id = value.id
///     if value.id == 0
///       @creator_cache = value
///       @creator_loaded = true
///     end
///   end
/// end
///
/// def _autosave_creator
///   if @creator_loaded && @creator_id == 0 && !@creator_cache.nil?
///     @creator_cache.save
///     @creator_id = @creator_cache.id
///   end
/// end
/// ```
///
/// A SAVED value deliberately does not populate the cache, so the
/// reader keeps re-querying by foreign key for those and a later direct
/// write to the key cannot be answered from a stale object.
///
/// WHY RUBY-FAMILY AND NOT THE SHARED LOWERING. This started shared and
/// the strict targets refused it, each in its own way — the two of them
/// do not even agree on the shape of the slot being read:
///
///   * Rust emits the cache field as a plain `Article`, so `.nil?`
///     became `is_null()` on a struct that has no such method;
///   * Crystal emits it as `Article?`, so dropping the nil test to
///     satisfy Rust gave `undefined method 'save' for Nil`;
///   * `new_record?` lives on the runtime `Base`, which a strict
///     target's model — a plain struct — never inherits;
///   * and an unstamped `@fk = <cache>.id` typed the foreign-key column
///     `Ty::Var`, which Rust reads as "already a Value": `article_id`
///     flipped from `i64` to `serde_json::Value` and took eleven E0308s
///     with it.
///
/// There is no single spelling that satisfies both, so this joins
/// `apply_through_assoc_lowering` as a Ruby-family capability and the
/// strict targets keep the gap they already had, ledgered in
/// docs/pipeline/runtime.md.
///
/// Ordering inside `before_validation` is load-bearing: the autosave
/// call is pushed BEFORE the belongs_to `default:` statement, which is
/// guarded on `@creator_id == 0`. Fill the key first or an explicitly
/// assigned unsaved creator is overwritten by the default.
pub(crate) fn apply_belongs_to_autosave(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        let names: Vec<(Symbol, Symbol)> = model
            .associations()
            .filter_map(|a| match a {
                Association::BelongsTo { name, foreign_key, polymorphic: false, .. } => {
                    Some((name.clone(), foreign_key.clone()))
                }
                _ => None,
            })
            .collect();
        for (name, fk) in names {
            let writer = Symbol::from(format!("{}=", name.as_str()));
            let Some(w) = lc
                .methods
                .iter_mut()
                .find(|m| m.name == writer && m.receiver == MethodReceiver::Instance)
            else {
                continue;
            };
            // Only the SYNTHESIZED writer is patched, recognized by its
            // shape: `if value.nil? then @fk = 0 else @fk = value.id`.
            // A model that wrote its own `<name>=` keeps it untouched
            // and gets no autosave either — the stash is the only thing
            // `_autosave_<name>` reads, and watching a slot nobody
            // fills would be worse than the gap.
            let ExprNode::If { else_branch, .. } = &mut *w.body.node else { continue };
            if !matches!(&*else_branch.node, ExprNode::Assign { target: LValue::Ivar { name: n }, .. } if n == &fk)
            {
                continue;
            }
            let span = else_branch.span;
            let assign = else_branch.clone();
            *else_branch = Expr::new(
                span,
                ExprNode::Seq { exprs: vec![assign, stash_unsaved(&name, span)] },
            );
            lc.methods.push(autosave_method(&lc.name, &name, &fk));
            fold_before_validation(&mut lc.methods, &lc.name, &name);
        }
    }
}

/// `if value.id == 0 then @<name>_cache = value; @<name>_loaded = true end`
fn stash_unsaved(name: &Symbol, span: Span) -> Expr {
    let syn = |n| Expr::new(span, n);
    let value = || syn(ExprNode::Var { id: VarId(0), name: Symbol::from("value") });
    let id_read = syn(ExprNode::Send {
        recv: Some(value()),
        method: Symbol::from("id"),
        args: vec![],
        block: None,
        parenthesized: false,
    });
    syn(ExprNode::If {
        cond: syn(ExprNode::Send {
            recv: Some(id_read),
            method: Symbol::from("=="),
            args: vec![syn(ExprNode::Lit { value: Literal::Int { value: 0 } })],
            block: None,
            parenthesized: false,
        }),
        then_branch: syn(ExprNode::Seq {
            exprs: vec![
                syn(ExprNode::Assign {
                    target: LValue::Ivar {
                        name: Symbol::from(format!("{}_cache", name.as_str())),
                    },
                    value: value(),
                }),
                syn(ExprNode::Assign {
                    target: LValue::Ivar {
                        name: Symbol::from(format!("{}_loaded", name.as_str())),
                    },
                    value: syn(ExprNode::Lit { value: Literal::Bool { value: true } }),
                }),
            ],
        }),
        else_branch: syn(ExprNode::Lit { value: Literal::Nil }),
    })
}

fn autosave_method(
    owner: &ClassId,
    name: &Symbol,
    fk: &Symbol,
) -> crate::dialect::MethodDef {
    let span = Span::synthetic();
    let syn = |n| Expr::new(span, n);
    let cache = || syn(ExprNode::Ivar { name: Symbol::from(format!("{}_cache", name.as_str())) });
    let send = |recv: Expr, method: &str| {
        syn(ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: vec![],
            block: None,
            parenthesized: false,
        })
    };
    let and = |l: Expr, r: Expr| {
        syn(ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::And,
            surface: crate::expr::BoolOpSurface::default(),
            left: l,
            right: r,
        })
    };
    let fk_zero = syn(ExprNode::Send {
        recv: Some(syn(ExprNode::Ivar { name: fk.clone() })),
        method: Symbol::from("=="),
        args: vec![syn(ExprNode::Lit { value: Literal::Int { value: 0 } })],
        block: None,
        parenthesized: false,
    });
    let cond = and(
        and(
            syn(ExprNode::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) }),
            fk_zero,
        ),
        send(send(cache(), "nil?"), "!"),
    );
    let body = syn(ExprNode::If {
        cond,
        then_branch: syn(ExprNode::Seq {
            exprs: vec![
                send(cache(), "save"),
                syn(ExprNode::Assign {
                    target: LValue::Ivar { name: fk.clone() },
                    value: send(cache(), "id"),
                }),
            ],
        }),
        else_branch: syn(ExprNode::Lit { value: Literal::Nil }),
    });
    crate::dialect::MethodDef {
        name: Symbol::from(format!("_autosave_{}", name.as_str())),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    }
}

/// PREPEND, not append: see the ordering note on
/// [`apply_belongs_to_autosave`].
fn fold_before_validation(
    methods: &mut Vec<crate::dialect::MethodDef>,
    owner: &ClassId,
    name: &Symbol,
) {
    let span = Span::synthetic();
    let call = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from(format!("_autosave_{}", name.as_str())),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let hook = Symbol::from("before_validation");
    if let Some(existing) =
        methods.iter_mut().find(|m| m.name == hook && m.receiver == MethodReceiver::Instance)
    {
        let mut stmts = match &*existing.body.node {
            ExprNode::Seq { exprs } => exprs.clone(),
            _ => vec![existing.body.clone()],
        };
        stmts.insert(0, call);
        existing.body = Expr::new(span, ExprNode::Seq { exprs: stmts });
        return;
    }
    methods.push(crate::dialect::MethodDef {
        name: hook,
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body: call,
        signature: None,
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    });
}

/// Ruby-family pre-emit pass: correct `has_many :through` readers. The
/// shared lowering synthesizes EVERY has_many reader as a direct
/// foreign-key query (`Tag.where(story_id: @id)`) — wrong for `through:`,
/// where the foreign key lives on the join table. Rebuild those readers
/// as a Relation join through the intermediate:
///
///   def tags
///     return @tags_cache if @tags_loaded
///     ActiveRecord::Relation.new(Tag)
///       .joins("INNER JOIN taggings ON taggings.tag_id = tags.id")
///       .where("taggings.story_id = ?", @id)
///   end
///
/// The through-model's `belongs_to` whose target matches the assoc's
/// target supplies the source foreign key (works for `source:` renames —
/// `upvoted_stories, through: :votes, source: :story` finds
/// `Vote.belongs_to :story`); when the source is a `has_many` instead
/// the key is on the target table and the join reverses (campfire's
/// `reachable_messages, through: :rooms, source: :messages`). Nested chains — the source association on
/// the join model is itself `:through` (`Category has_many :stories,
/// through: :tags` where `Tag#stories` goes through taggings), or the
/// first hop is — recurse, adding one INNER JOIN per hop. Shapes a hop
/// can't prove (missing models, no matching source) are left on the
/// shared reader rather than guessed.
/// KNOWN GAP: association scope-lambdas (`-> { order(...) }`, the
/// upvoted vote-conditions) are dropped at ingest, so row order/filter
/// can diverge from Rails until the lambda lands in the IR.
pub(crate) fn apply_through_assoc_lowering(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };
        for assoc in model.associations() {
            let Association::HasMany {
                name, target, through: Some(thr_name), scope: assoc_scope, ..
            } = assoc
            else {
                continue;
            };
            let Some((joins, edge_table, edge_fk)) =
                resolve_through_chain(&app.models, model, thr_name, target, 0)
            else {
                continue;
            };
            let join_sql = joins.join(" ");
            let where_sql = format!("{edge_table}.{edge_fk} = ?");

            let Some(m) =
                lc.methods.iter_mut().find(|m| {
                    m.name == *name && m.receiver == crate::dialect::MethodReceiver::Instance
                })
            else {
                continue;
            };
            m.body = through_reader_body(name, target, &join_sql, &where_sql, assoc_scope);
        }
    }
}

/// Resolve the SQL join chain for a `has_many :through` on `model`
/// reaching `target` via the sibling association named `thr_name`.
/// Returns the INNER JOIN fragments (target-nearest first, ready to
/// `join(" ")`) plus the WHERE edge `(table, fk)` that points back at
/// the owner's id. One recursion per indirection: a first hop that is
/// itself `:through`, or a join-model source association that is.
/// `None` when a hop can't be proven — missing join model, no
/// `belongs_to`/`has_many`/through source matching the target class —
/// and for pathological depth (cyclic `through:` declarations).
fn resolve_through_chain(
    models: &[crate::dialect::Model],
    model: &crate::dialect::Model,
    thr_name: &Symbol,
    target: &ClassId,
    depth: usize,
) -> Option<(Vec<String>, String, Symbol)> {
    use crate::dialect::Association;
    use crate::naming::pluralize_snake;

    if depth > 4 {
        return None;
    }
    // The through association on the owner (`:votes`, `:taggings`, `:tags`).
    let (thr_target, thr_fk, thr_through) = model.associations().find_map(|a| match a {
        Association::HasMany { name, target, foreign_key, through, .. } if name == thr_name => {
            Some((target, foreign_key, through))
        }
        _ => None,
    })?;
    // How the join model's rows tie back to the owner: directly by the
    // sibling's fk, or through the sibling's own chain.
    let (back_joins, edge_table, edge_fk) = match thr_through {
        None => (Vec::new(), pluralize_snake(thr_target.0.as_str()), thr_fk.clone()),
        Some(inner) => resolve_through_chain(models, model, inner, thr_target, depth + 1)?,
    };
    let thr_model = models.iter().find(|m| &m.name == thr_target)?;
    let thr_table = pluralize_snake(thr_target.0.as_str());
    let target_table = pluralize_snake(target.0.as_str());
    // The source belongs_to on the join model (`Vote.belongs_to :story`)
    // — matched by target class, so `source:` renames resolve without a
    // name convention.
    if let Some(src_fk) = thr_model.associations().find_map(|a| match a {
        Association::BelongsTo { target: t, foreign_key, .. } if t == target => Some(foreign_key),
        _ => None,
    }) {
        let mut joins =
            vec![format!("INNER JOIN {thr_table} ON {thr_table}.{src_fk} = {target_table}.id")];
        joins.extend(back_joins);
        return Some((joins, edge_table, edge_fk));
    }
    // The source is a plain `has_many` on the join model, so the
    // foreign key is on the TARGET table and the join points the other
    // way (campfire: `has_many :reachable_messages, through: :rooms,
    // source: :messages` — Room#messages, `messages.room_id`). The
    // belongs_to branch above cannot serve this: there is no column on
    // the join table naming the target.
    if let Some(src_fk) = thr_model.associations().find_map(|a| match a {
        Association::HasMany { target: t, foreign_key, through: None, .. } if t == target => {
            Some(foreign_key)
        }
        _ => None,
    }) {
        let mut joins =
            vec![format!("INNER JOIN {thr_table} ON {thr_table}.id = {target_table}.{src_fk}")];
        joins.extend(back_joins);
        return Some((joins, edge_table, edge_fk));
    }
    // Nested: the join model reaches the target through its own
    // `:through` association. Resolve that chain (phrased from the same
    // target table), then graft the join model onto its owner edge.
    let src_through = thr_model.associations().find_map(|a| match a {
        Association::HasMany { target: t, through: Some(thru), .. } if t == target => Some(thru),
        _ => None,
    })?;
    let (src_joins, src_edge_table, src_edge_fk) =
        resolve_through_chain(models, thr_model, src_through, target, depth + 1)?;
    let mut joins = src_joins;
    joins.push(format!(
        "INNER JOIN {thr_table} ON {thr_table}.id = {src_edge_table}.{src_edge_fk}"
    ));
    joins.extend(back_joins);
    Some((joins, edge_table, edge_fk))
}

/// The joined Relation chain, carrying the eager-load cache (see
/// `apply_through_assoc_lowering`).
///
/// ONE return path, and it is a Relation. The shared
/// `synth_has_many_reader` opens its body with `return @<name>_cache if
/// @<name>_loaded` — an Array — which is the right answer for a direct
/// has_many, whose reader materializes rows and is declared
/// `Array[T]`. A `through:` reader is declared `ActiveRecord::Relation`
/// (`associations.rs`, and the campfire routing bug its comment names),
/// so that guard made the method answer two unrelated types. On CRuby
/// that was latent — a preloaded `user.upvoted_stories.includes(:tags)
/// .order(...)` would reach `Array#includes`, which does not exist —
/// and under spinel's AOT it is a hard compile stop: `--rbs seed
/// contradicted: User#upvoted_stories is declared to return Relation
/// but this returns int_array` (spinel judges a seeded return as of
/// 2368afd7; the cache ivar types `int_array` off the bare `[]` in
/// `initialize` when nothing in the app preloads it). It took the
/// lobsters spinel bench lane down for three days.
///
/// So the preload seam moves ONTO the relation: `_preload_<name>` still
/// fills the cache ivar, and the reader hands it to the relation it was
/// going to answer with anyway. `preloaded` seeds the loaded-records
/// memo when the flag is set and is a no-op when it is not — the flag
/// cannot be inferred from the cache, which is `[]` both for "empty"
/// and for "never loaded". A caller that chains on further clears that
/// memo and re-queries, which is what every chain method here does and
/// what Rails does to a loaded relation.
fn through_reader_body(
    name: &Symbol,
    target: &ClassId,
    join_sql: &str,
    where_sql: &str,
    assoc_scope: &Option<Expr>,
) -> Expr {
    let span = Span::synthetic;
    let target_const = Expr::new(
        span(),
        ExprNode::Const {
            path: target.0.as_str().split("::").map(Symbol::from).collect(),
        },
    );
    let seed = Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(Expr::new(
                span(),
                ExprNode::Const {
                    path: vec![Symbol::from("ActiveRecord"), Symbol::from("Relation")],
                },
            )),
            method: Symbol::from("new"),
            args: vec![target_const],
            block: None,
            parenthesized: true,
        },
    );
    let joined = Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(seed),
            method: Symbol::from("joins"),
            args: vec![Expr::new(
                span(),
                ExprNode::Lit {
                    value: crate::expr::Literal::Str { value: join_sql.to_string() },
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let chain = Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(joined),
            method: Symbol::from("where"),
            args: vec![
                Expr::new(
                    span(),
                    ExprNode::Lit {
                        value: crate::expr::Literal::Str { value: where_sql.to_string() },
                    },
                ),
                Expr::new(span(), ExprNode::Ivar { name: Symbol::from("id") }),
            ],
            block: None,
            parenthesized: true,
        },
    );

    // Association scope lambda (`-> { where('votes.vote' => 1)... }`) —
    // graft its receiver-less chain onto the seeded relation so the
    // reader filters the way Rails does (without it /upvoted served
    // every joined row).
    let chain = match assoc_scope {
        Some(scope_body) => graft_chain_root(scope_body, chain),
        None => chain,
    };

    // Last in the chain, after any scope graft: the scope's conditions
    // belong to the QUERY, and this hands the finished relation the
    // records an eager load already fetched.
    Expr::new(
        span(),
        ExprNode::Send {
            recv: Some(chain),
            method: Symbol::from("preloaded"),
            args: vec![
                Expr::new(
                    span(),
                    ExprNode::Ivar { name: Symbol::from(format!("{}_cache", name.as_str())) },
                ),
                Expr::new(
                    span(),
                    ExprNode::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
                ),
            ],
            block: None,
            parenthesized: true,
        },
    )
}

/// Replace the receiver-less root of a `where(...).order(...)` chain
/// with `seed`, turning an association-scope lambda body into a call
/// chain on the seeded relation. Non-chain shapes (a Seq, a literal)
/// return the seed untouched — better an unfiltered relation than a
/// mis-grafted one.
fn graft_chain_root(chain: &Expr, seed: Expr) -> Expr {
    match &*chain.node {
        ExprNode::Send { recv: Some(r), method, args, block, parenthesized } => {
            let new_recv = graft_chain_root(r, seed);
            Expr::new(
                chain.span,
                ExprNode::Send {
                    recv: Some(new_recv),
                    method: method.clone(),
                    args: args.clone(),
                    block: block.clone(),
                    parenthesized: *parenthesized,
                },
            )
        }
        ExprNode::Send { recv: None, method, args, block, parenthesized } => Expr::new(
            chain.span,
            ExprNode::Send {
                recv: Some(seed),
                method: method.clone(),
                args: args.clone(),
                block: block.clone(),
                parenthesized: *parenthesized,
            },
        ),
        _ => seed,
    }
}

/// Ruby-family pre-emit pass: resolve `app/helpers/*` references. Rails
/// mixes every helper module into every view as instance methods, but the
/// post-lowering IR emits views/controllers/helpers as module-functions
/// (`Views::Stories.listdetail`, `ApplicationHelper.avatar_img`), so a bare
/// `avatar_img(...)` rendered into a view body has no `self` to dispatch on
/// and raises `NoMethodError`. This pass (a) flips each helper module's own
/// methods to class methods so `ApplicationHelper.avatar_img` is a real
/// call target, and (b) rewrites every bare call whose name the helper
/// registry knows — in whatever LibraryClass bodies it's run over — into
/// `<DefiningModule>.method(...)`. Lives on the Ruby emit path: helper
/// modules are app-specific and the rewrite targets a CRuby call shape, so
/// shared `lower/` stays target-agnostic (same rule as scope lowering). A
/// strict no-op when the app has no non-empty helpers — the blog's helper
/// modules are empty, so `helper_method_index` is empty.
pub(crate) fn apply_helper_lowering(lcs: &mut [LibraryClass], app: &App) {
    // NO early return on an empty `helper_method_index`. This pass does
    // two jobs and only one of them is about the app's own helpers: the
    // other resolves bare FRAMEWORK helper calls (`image_tag`,
    // `sanitize`, `dom_id`) to `ActionView::ViewHelpers.<name>`, and an
    // app with no `app/helpers/` directory needs that just as much. The
    // guard that used to stand here made the qualification depend on
    // whether some unrelated file existed.
    let helper_modules: BTreeSet<ClassId> =
        app.helper_method_index.values().cloned().collect();
    // Generated route-helper names (`active_path`, `story_path`, …) —
    // bare calls to these in layout/helper bodies resolve to the
    // generated `RouteHelpers` module. (The view walker rewrites route
    // helpers in the URL positions it classifies; bare calls nested in
    // unclassified expressions fall through to this pass.)
    let route_helpers: std::collections::HashSet<Symbol> =
        crate::lower::lower_routes_to_library_functions(app)
            .into_iter()
            .map(|f| f.name)
            .collect();
    // App classes that `include Rails.application.routes.url_helpers`
    // (ingest records the marker as an include of RouteHelpers —
    // lobsters' Routes). Explicit `X.<helper>` call sites anywhere in
    // the tree rewrite through RouteHelpers below. Sourced from
    // `app.library_classes`, not the local `lcs` slice — this pass
    // also runs over the lowered-models stack, whose slice doesn't
    // contain the including class itself.
    let url_helper_classes: std::collections::HashSet<Symbol> = app
        .library_classes
        .iter()
        .filter(|lc| lc.includes.iter().any(|i| i.0.as_str() == "RouteHelpers"))
        .map(|lc| lc.name.0.clone())
        .collect();
    let helper_ivars = helper_read_ivars(app);
    let view_ivars = view_assigned_ivars(app);
    for lc in lcs.iter_mut() {
        // CONTROLLERS in the index provide `helper_method`s to views:
        // only the call-site rewrite applies to them — their methods
        // must NOT flip class-side wholesale (actions are instance
        // methods; the class-side clone for the marked helpers comes
        // from the controller lowering itself).
        let is_helper_module = helper_modules.contains(&lc.name)
            && !app.controllers.iter().any(|c| c.name == lc.name);
        // Helper and view module functions have no controller context —
        // a bare `request` read there resolves to the per-dispatch
        // `ActionController::Current.request` (controllers keep their
        // own `request` accessor and are left alone).
        let rewrite_request =
            is_helper_module || lc.name.0.as_str().starts_with("Views::");
        // The class's own method names shadow the helper index inside its
        // bodies — a bare call to one of them is a self-dispatch, never a
        // cross-module helper reference.
        let own_methods: std::collections::HashSet<Symbol> =
            lc.methods.iter().map(|m| m.name.clone()).collect();
        for m in &mut lc.methods {
            // A helper module's own methods become module-functions so the
            // rewritten `Module.method` call has a real target — Rails mixed
            // them into a view instance, but the emitted views are module
            // functions with no instance to receive them.
            if is_helper_module && m.receiver == MethodReceiver::Instance {
                m.receiver = MethodReceiver::Class;
            }
            // Ground `…url_helpers.<x>_url(record?, host:)` → absolute-URL
            // interpolation BEFORE `rewrite_helper_calls` collapses the
            // `Rails.application.routes.url_helpers` chain to `RouteHelpers`
            // (that collapse is children-first, so it would erase the chain
            // shape this grounding matches on). mod_note's `user_url` lives
            // in a model body, reached by this same pass.
            rewrite_url_helpers_absolute(&mut m.body);
            // The method's OWN params shadow the helper index too: a bare
            // `tag` in a partial whose strict-locals header declares `tag:`
            // is that local, not `ApplicationHelper.tag`. (Rails mixes
            // helpers BENEATH a template's locals.) Passed SEPARATELY from
            // own_methods so the arity guard (bare-reference only) applies
            // to params but not to real self-dispatched methods.
            let own_params: std::collections::HashSet<Symbol> =
                m.params.iter().map(|p| p.name.clone()).collect();
            rewrite_helper_calls(
                &mut m.body,
                &app.helper_method_index,
                &route_helpers,
                &url_helper_classes,
                rewrite_request,
                &own_methods,
                &own_params,
                &app.view_visible_controller_methods,
            );
            // A CONTROLLER IVAR read in a helper body. Rails mixes
            // helpers into the view instance, which carries the
            // controller's assigns, so `@room` there IS the
            // controller's — campfire's `RoomsHelper.link_to_edit_room`
            // reads it for the edit link's style and data attributes,
            // and lobsters' `StoriesHelper` reads `@user`/`@ribbon` the
            // same way. A module function has no instance, so the read
            // was a bare nil and every `rooms#show` died on
            // `undefined method 'id' for nil`.
            //
            // Routed through the same per-dispatch seam `flash` /
            // `cookies` / a `helper_method` already use, which is the
            // one place a module function can reach the live
            // controller.
            if is_helper_module {
                rewrite_helper_ivars(&mut m.body, &helper_ivars);
            }
            if lc.name.0.as_str().starts_with("Views::") {
                rewrite_view_ivar_writes(&mut m.body, &view_ivars);
            }
        }
        // The other half: the reader the rewrite dispatches to. It goes
        // on the BASE controller, so every controller inherits one —
        // a helper runs under whichever controller is current, and
        // Rails' answer for an assign that controller never made is
        // nil, which is exactly what reading an unassigned ivar gives.
        if is_base_controller(lc) {
            push_helper_ivar_readers(&mut lc.methods, &helper_ivars, &lc.name);
            // The name a template WRITES needs a reader too, not only
            // the setter: `page_title_tag` reads what `rooms/show`
            // set, and a name no helper happens to read still has to
            // answer nil rather than NoMethodError.
            push_helper_ivar_readers(&mut lc.methods, &view_ivars, &lc.name);
            push_helper_ivar_writers(&mut lc.methods, &view_ivars, &lc.name);
        }
    }
}

/// Is this the app's base controller — the one every other controller
/// inherits from (`class ApplicationController < ActionController::Base`)?
fn is_base_controller(lc: &LibraryClass) -> bool {
    lc.parent.as_ref().is_some_and(|p| p.0.as_str() == "ActionController::Base")
}

/// The controller ivars helper-module bodies read.
///
/// Helper modules only — a `Views::` module's ivars were already bound
/// to locals by the view lowering's closure threading, and re-routing
/// one here would silently prefer a stale controller read over the
/// value the caller passed.
///
/// A name any controller already DEFINES as a method is excluded: the
/// reader this pass synthesizes would collide with it, and the app's
/// own method is the one that should win.
fn helper_read_ivars(app: &App) -> std::collections::BTreeSet<Symbol> {
    let helper_modules: BTreeSet<ClassId> = app
        .helper_method_index
        .values()
        .filter(|id| !app.controllers.iter().any(|c| &c.name == *id))
        .cloned()
        .collect();
    let mut out = std::collections::BTreeSet::new();
    for lc in &app.library_classes {
        if !helper_modules.contains(&lc.name) {
            continue;
        }
        for m in &lc.methods {
            collect_ivar_reads(&m.body, &mut out);
        }
    }
    let controller_methods: std::collections::HashSet<Symbol> = app
        .controllers
        .iter()
        .flat_map(|c| c.body.iter())
        .filter_map(|item| match item {
            crate::dialect::ControllerBodyItem::Action { action, .. } => {
                Some(action.name.clone())
            }
            _ => None,
        })
        .collect();
    out.retain(|n| !controller_methods.contains(n));
    out
}

fn collect_ivar_reads(e: &Expr, out: &mut std::collections::BTreeSet<Symbol>) {
    if let ExprNode::Ivar { name } = &*e.node {
        out.insert(name.clone());
    }
    e.node.for_each_child(&mut |c| collect_ivar_reads(c, out));
}

/// `@room` -> `ActionController::Current.controller.room`, for the
/// names `helper_read_ivars` collected.
fn rewrite_helper_ivars(e: &mut Expr, names: &std::collections::BTreeSet<Symbol>) {
    e.node.for_each_child_mut(&mut |c| rewrite_helper_ivars(c, names));
    let name = match &*e.node {
        ExprNode::Ivar { name } if names.contains(name) => name.clone(),
        _ => return,
    };
    let span = e.span;
    let controller = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const {
                    path: vec![Symbol::from("ActionController"), Symbol::from("Current")],
                },
            )),
            method: Symbol::from("controller"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    *e.node = ExprNode::Send {
        recv: Some(controller),
        method: name,
        args: vec![],
        block: None,
        parenthesized: false,
    };
}

/// The ivars a `Views::` module ASSIGNS.
///
/// Rails' `@page_title = …` in a template writes the view context the
/// layout and every mixed-in helper share, so the write has to land
/// where the helper-side READ already looks — on the live controller.
/// The view lowering leaves the assign as an Ivar precisely so this
/// pass can see it; a helper module's ivar is a controller assign it
/// only reads, which is `helper_read_ivars`' half of the same seam.
///
/// Read off `app.views` — the TEMPLATE bodies — not off the lowered
/// `Views::` classes: `apply_helper_lowering` runs once per emitted
/// stack, and the stack holding the views is not the one holding the
/// base controller that has to grow the accessors. Sourced from the
/// App, both halves see the same names.
fn view_assigned_ivars(app: &App) -> std::collections::BTreeSet<Symbol> {
    let mut out = std::collections::BTreeSet::new();
    for view in &app.views {
        collect_ivar_writes(&view.body, &mut out);
    }
    out
}

fn collect_ivar_writes(e: &Expr, out: &mut std::collections::BTreeSet<Symbol>) {
    match &*e.node {
        ExprNode::Assign { target: LValue::Ivar { name }, .. }
        | ExprNode::OpAssign { target: LValue::Ivar { name }, .. } => {
            out.insert(name.clone());
        }
        _ => {}
    }
    e.node.for_each_child(&mut |c| collect_ivar_writes(c, out));
}

/// `@page_title = x` -> `ActionController::Current.controller.page_title = x`,
/// the write half of the seam `rewrite_helper_ivars` reads through.
fn rewrite_view_ivar_writes(e: &mut Expr, names: &std::collections::BTreeSet<Symbol>) {
    e.node.for_each_child_mut(&mut |c| rewrite_view_ivar_writes(c, names));
    let (name, value) = match &*e.node {
        ExprNode::Assign { target: LValue::Ivar { name }, value } if names.contains(name) => {
            (name.clone(), value.clone())
        }
        _ => return,
    };
    let span = e.span;
    *e.node = ExprNode::Assign {
        target: LValue::Attr { recv: current_controller_expr(span), name },
        value,
    };
}

/// The `ActionController::Current.controller` receiver both halves of
/// the seam dispatch through.
fn current_controller_expr(span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const {
                    path: vec![Symbol::from("ActionController"), Symbol::from("Current")],
                },
            )),
            method: Symbol::from("controller"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// `def page_title=(v); @page_title = v; end` on the base controller,
/// the target of `rewrite_view_ivar_writes`. Paired with the reader
/// `push_helper_ivar_readers` pushes for the same name — a title a
/// template sets and the layout's helper reads travels through both.
fn push_helper_ivar_writers(
    methods: &mut Vec<MethodDef>,
    names: &std::collections::BTreeSet<Symbol>,
    class_name: &ClassId,
) {
    for name in names {
        let setter = Symbol::from(format!("{}=", name.as_str()));
        if methods.iter().any(|m| m.name == setter) {
            continue;
        }
        let span = Span::synthetic();
        methods.push(MethodDef {
            name: setter,
            receiver: MethodReceiver::Instance,
            params: vec![crate::dialect::Param::positional(Symbol::from("value"))],
            body: Expr::new(
                span,
                ExprNode::Assign {
                    target: LValue::Ivar { name: name.clone() },
                    value: Expr::new(
                        span,
                        ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("value") },
                    ),
                },
            ),
            signature: None,
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(class_name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: true,
            block_param: None,
        });
    }
}

/// `def room; @room; end` on the base controller, one per name.
fn push_helper_ivar_readers(
    methods: &mut Vec<MethodDef>,
    names: &std::collections::BTreeSet<Symbol>,
    class_name: &ClassId,
) {
    for name in names {
        if methods.iter().any(|m| &m.name == name) {
            continue;
        }
        methods.push(MethodDef {
            name: name.clone(),
            receiver: MethodReceiver::Instance,
            params: vec![],
            body: Expr::new(Span::synthetic(), ExprNode::Ivar { name: name.clone() }),
            signature: None,
            effects: crate::effect::EffectSet::default(),
            enclosing_class: Some(class_name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
}

/// Framework view helpers callable from a helper/model body that the
/// view-template classifier (which runs only on views) never reaches —
/// plus bare calls in view bodies the classifier has no kind for (it
/// handles a fixed set; the rest fall through to this pass). They
/// resolve to `ActionView::ViewHelpers.<name>`. Grown as GET / surfaces
/// each one: asset helpers (`avatar_img` → `image_tag` → `image_path`),
/// then the date helpers + `content_tag` (`time_ago_in_words_label`
/// calls both bare).
fn is_framework_view_helper(name: &str) -> bool {
    matches!(
        name,
        "image_tag"
            | "image_path"
            // The general asset helper beside the image one — same
            // body, and campfire asks it for an mp3.
            | "image_url"
            | "path_to_javascript"
            | "javascript_path"
            | "javascript_include_tag"
            | "number_with_precision"
            | "number_with_delimiter"
            | "number_to_human"
            | "content_security_policy_nonce"
            | "class_names"
            | "label_tag"
            | "url_for"
            // `polymorphic_url(record, only_path: true)` — beside
            // `url_for`, which is the same question asked of a value
            // the lowerings could answer statically. campfire's
            // `BroadcastsHelper` asks it of an Active Storage
            // representation, so the runtime member it now resolves to
            // is a raising stub; unqualified it was a bare call NOTHING
            // defines, which stops a strict build outright.
            | "polymorphic_url"
            // `polymorphic_url(record, only_path: true)` — beside
            // `url_for`, which is the same question asked of a value
            // the lowerings could answer statically. campfire's
            // `BroadcastsHelper` asks it of an Active Storage
            // representation, so the runtime member it now resolves to
            // is a raising stub; unqualified it was a bare call NOTHING
            // defined, which stops a strict build outright.
            | "submit_tag"
            // The bare (builder-less) hidden field, beside its
            // `label_tag`/`submit_tag` siblings in the same overlay
            // file. campfire's quick-boost forms carry the emoji this
            // way, one per reaction on every message.
            | "hidden_field_tag"
            // Rails' array-of-fragments concat. campfire's
            // `current_user_meta_tags` builds the two `<meta>` tags the
            // page's `<head>` needs and joins them with it.
            | "safe_join"
            | "form_tag"
            | "content_tag"
            | "time_ago_in_words"
            | "distance_of_time_in_words"
            | "raw"
            | "link_to"
            // campfire's user page links the address with a bare
            // `mail_to @user.email_address`. The VIEW classifier has no
            // kind for it, so it falls through to this pass like the
            // rest of the flat helpers.
            | "mail_to"
            | "content_for?"
            | "capture"
            | "concat"
            // Every campfire message row is built by `MessagesHelper
            // .message_tag`, which opens with `dom_id(message)`. The
            // VIEW classifier has always known `dom_id`; a helper body
            // never reached it, and the bare call raised NoMethodError
            // — inside a `rescue Exception` that hid which call it was.
            | "dom_id"
            // `ActionView::Helpers::SanitizeHelper`, included into a
            // MODEL — campfire's `Opengraph::Metadata` calls
            // `sanitize(strip_tags(title))` on its own attributes. Both
            // were registry-known (analyze types them) and had no
            // runtime member to dispatch to until
            // view_helpers_ext.rb grew them; the bare calls emitted
            // bare, and the surviving `include` named a namespace no
            // target ships.
            | "sanitize"
            | "strip_tags"
            // `h` — Rails' escape alias, and `auto_link` beside it.
            // campfire's `message_presentation` is one expression built
            // from both: `auto_link h(ContentFilters…apply(message.body
            // .body)), html: { target: "_blank" }`. Unqualified, each
            // was a NoMethodError inside that method's own `rescue
            // Exception`, which returns "" — so every message rendered
            // an EMPTY BODY, with no diagnostic anywhere.
            | "h"
            | "auto_link"
    )
}

/// Is this a call on `ActionView::ViewHelpers` (or a bare `ViewHelpers`
/// const)? Shared by the post-rewrite transforms below.
fn is_view_helpers_const(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Const { path }
        if path.last().map(|s| s.as_str() == "ViewHelpers").unwrap_or(false))
}

/// Strip one trailing `.to_s` (the view walker's `coerce_to_s` wrap) so
/// the safe-call check sees the helper call itself.
fn strip_to_s(e: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return r;
        }
    }
    e
}

/// Calls whose result Rails treats as an html_safe buffer: tag-producing
/// framework helpers, `raw`, and app-helper module functions (which
/// compose those). The view walker's default `html_escape(<call>.to_s)`
/// wrap must NOT apply to these — escaping a safe buffer ships literal
/// `&lt;img&gt;` markup. Plain-string helpers (truncate,
/// time_ago_in_words) stay wrapped: their escape is Rails-correct.
/// Treating every app-helper as safe is a simplification (Rails escapes
/// an app helper that returns a plain string); the corpus' helpers all
/// return tag-helper compositions, and per-method safety inference can
/// refine this when a counterexample shows up.
fn is_html_safe_call(e: &Expr, index: &HashMap<Symbol, ClassId>) -> bool {
    let ExprNode::Send { recv: Some(r), method, .. } = &*e.node else {
        return false;
    };
    let ExprNode::Const { path } = &*r.node else {
        return false;
    };
    let joined =
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    if joined.ends_with("ViewHelpers") {
        return matches!(
            method.as_str(),
            "raw" | "link_to" | "link_to_raw" | "button_to" | "mail_to" | "image_tag" | "content_tag"
                | "javascript_include_tag" | "label_tag" | "submit_tag" | "form_tag"
                // The rest of FormTagHelper — the BUILDER-LESS field
                // tags, which return an element exactly the way
                // `label_tag` and `submit_tag` beside them do. Listed
                // as a set rather than one at a time because the
                // dividing line is not which one an app happened to
                // reach: a helper that returns an ELEMENT is safe, a
                // helper that returns TEXT (`truncate`, `dom_id`,
                // `time_ago_in_words`) is not, and every name here is
                // on the element side. `hidden_field_tag` is how the
                // omission surfaced: campfire's quick boosts carry
                // their emoji in one, eight per message, and the
                // escape shipped `&lt;input …&gt;` — 320 inputs Rails
                // renders and the room page did not, with the forms
                // around them all present and correct.
                | "hidden_field_tag"
                | "text_field_tag"
                | "password_field_tag"
                | "text_area_tag"
                | "check_box_tag"
                | "radio_button_tag"
                | "select_tag"
                | "button_tag"
                | "field_set_tag"
                | "file_field_tag"
                | "email_field_tag"
                | "number_field_tag"
                | "search_field_tag"
                | "telephone_field_tag"
                | "url_field_tag"
                | "date_field_tag"
                | "color_field_tag"
                // The `<option>` builders, same rule.
                | "options_for_select"
                | "options_from_collection_for_select"
                | "grouped_options_for_select"
                | "time_zone_options_for_select"
        );
    }
    index.values().any(|cid| cid.0.as_str() == joined)
}

/// `<Const ending in Base>.helpers` — the Rails idiom
/// `ActionController::Base.helpers.image_path(...)` used to reach view
/// helpers from a model. Collapses to the ViewHelpers module so the
/// trailing call resolves as `ActionView::ViewHelpers.image_path(...)`.
fn is_base_dot_helpers(node: &ExprNode) -> bool {
    if let ExprNode::Send { recv: Some(r), method, args, block, .. } = node {
        if method.as_str() == "helpers" && args.is_empty() && block.is_none() {
            if let ExprNode::Const { path } = &*r.node {
                return path.last().map(|s| s.as_str() == "Base").unwrap_or(false);
            }
        }
    }
    false
}

/// `Rails.application.routes.url_helpers` — the Rails idiom for reaching
/// route helpers from a model body (`Story#short_id_path` does
/// `...url_helpers.root_path + "s/#{short_id}"`). Collapses to the
/// generated `RouteHelpers` module so the trailing `.root_path` resolves
/// as `RouteHelpers.root_path`.
fn is_rails_url_helpers(node: &ExprNode) -> bool {
    let mut cur = node;
    for step in ["url_helpers", "routes", "application"] {
        let ExprNode::Send { recv: Some(r), method, args, block, .. } = cur else {
            return false;
        };
        if method.as_str() != step || !args.is_empty() || block.is_some() {
            return false;
        }
        cur = &r.node;
    }
    matches!(cur, ExprNode::Const { path }
        if path.last().map(|s| s.as_str() == "Rails").unwrap_or(false))
}

fn view_helpers_path() -> Vec<Symbol> {
    vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")]
}

/// Walk a method body and rewrite helper calls so they resolve against the
/// module-function surfaces. Four cases (children first, so nested calls
/// and the `.helpers` receiver are rewritten before their parent):
///   1. `<…Base>.helpers` → the `ActionView::ViewHelpers` constant.
///   2. `Rails.application.routes.url_helpers` → the `RouteHelpers` constant.
///   3. bare `name(args)` where `name` is an app helper → `<Module>.name(args)`.
///   4. bare `name(args)` where `name` is a framework view helper →
///      `ActionView::ViewHelpers.name(args)`.
/// Only receiver-less Sends are rewritten in (3)/(4): a call with a receiver
/// already resolves, and a bare identifier with no call shape is a local read.
fn rewrite_helper_calls(
    expr: &mut Expr,
    index: &HashMap<Symbol, ClassId>,
    route_helpers: &std::collections::HashSet<Symbol>,
    url_helper_classes: &std::collections::HashSet<Symbol>,
    rewrite_request: bool,
    own_methods: &std::collections::HashSet<Symbol>,
    own_params: &std::collections::HashSet<Symbol>,
    view_visible: &std::collections::BTreeSet<Symbol>,
) {
    expr.node.for_each_child_mut(&mut |c| {
        rewrite_helper_calls(
            c,
            index,
            route_helpers,
            url_helper_classes,
            rewrite_request,
            own_methods,
            own_params,
            view_visible,
        )
    });

    // Destructive `<lvalue>.<m>!(args)` on a frozen string raises under
    // spinel (frozen literals) — lobsters' link_to_different_page does
    // `path.sub!(/…/, "")` on a route-helper literal (`active_path` → a
    // frozen "/active"). For the in-place string mutators whose bang form
    // modifies self and returns self-or-nil, the non-destructive
    // `<lvalue> = <lvalue>.<m>(args)` is equivalent when the bang return is
    // unused (statement position) and is frozen-safe. Assignable receiver
    // (Var/Ivar) only; `slice!` and friends (which return the removed part,
    // not the modified string) are deliberately excluded.
    let bang_rewrite: Option<Symbol> =
        if let ExprNode::Send { recv: Some(r), method, block: None, .. } = &*expr.node {
            let is_lv =
                matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. });
            method
                .as_str()
                .strip_suffix('!')
                .filter(|base| {
                    is_lv
                        && matches!(
                            *base,
                            "sub" | "gsub" | "chomp" | "chop" | "strip" | "lstrip"
                                | "rstrip" | "squeeze" | "upcase" | "downcase"
                                | "capitalize" | "swapcase" | "tr" | "tr_s" | "delete"
                                | "reverse"
                        )
                })
                .map(Symbol::from)
        } else {
            None
        };
    if let Some(base) = bang_rewrite {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv, args, parenthesized, .. } = node else { unreachable!() };
        let r = recv.unwrap();
        let target = match &*r.node {
            ExprNode::Var { id, name } => LValue::Var { id: *id, name: name.clone() },
            ExprNode::Ivar { name } => LValue::Ivar { name: name.clone() },
            _ => unreachable!(),
        };
        let call = Expr::new(
            span,
            ExprNode::Send { recv: Some(r), method: base, args, block: None, parenthesized },
        );
        *expr.node = ExprNode::Assign { target, value: call };
        return;
    }

    // ActiveSupport `String#pluralize(count)` — count-aware inflection of
    // the string ITSELF (singular when count == 1, else the inflected
    // plural), distinct from the count-labeling `Inflector.pluralize(count,
    // word)` grounded below. Spinel can't reopen the built-in String, so
    // lower the String-receiver form to the ruby-family `Inflector.
    // pluralize_word` module function. String-typed receiver only, so a
    // model's own one-arg `pluralize` (should one exist) is left alone.
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*expr.node {
        if method.as_str() == "pluralize"
            && args.len() == 1
            && matches!(r.ty, Some(crate::ty::Ty::Str))
        {
            let span = expr.span;
            let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
            let ExprNode::Send { recv, args, .. } = node else { unreachable!() };
            let count = args.into_iter().next().unwrap();
            *expr.node = ExprNode::Send {
                recv: Some(Expr::new(
                    span,
                    ExprNode::Const { path: vec![Symbol::from("Inflector")] },
                )),
                method: Symbol::from("pluralize_word"),
                args: vec![recv.unwrap(), count],
                block: None,
                parenthesized: true,
            };
            return;
        }
    }

    // `X.<helper>` where X singleton-includes url_helpers (lobsters'
    // `Routes.user_url reparent_user`): a `<x>_path` retargets to the
    // generated RouteHelpers module; a `<x>_url` whose path sibling is
    // generated re-lands as the bare `<x>_url` form so the absolute-URL
    // grounding a few blocks down claims it in this same visit.
    if let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node {
        if let ExprNode::Const { path } = &*r.node {
            if path.len() == 1 && url_helper_classes.contains(&path[0]) {
                let named_path = route_helpers.contains(method);
                let named_url = method
                    .as_str()
                    .strip_suffix("_url")
                    .is_some_and(|stem| {
                        route_helpers.contains(&Symbol::from(format!("{stem}_path")))
                    });
                if named_path {
                    let ExprNode::Send { recv, .. } = &mut *expr.node else { unreachable!() };
                    *recv = Some(Expr::new(
                        expr.span,
                        ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
                    ));
                } else if named_url {
                    let ExprNode::Send { recv, .. } = &mut *expr.node else { unreachable!() };
                    *recv = None;
                }
            }
        }
    }

    // Bare controller-context reads in a helper/view module body →
    // the per-dispatch parked objects (module functions have no
    // controller to delegate to): `request` →
    // `ActionController::Current.request`; `cookies`/`session`/`flash`
    // → `ActionController::Current.controller.<x>` (lobsters'
    // ApplicationHelper.filtered_tags reads the tag-filter cookie).
    if rewrite_request {
        // `helper_method :platform` — the app SAYING this controller
        // method is view-callable, for the marked methods the existing
        // path cannot serve.
        //
        // TWO ARMS OF ONE RULE, and the discriminator already exists.
        // `controller_helper_method_names` clones a marked method
        // CLASS-SIDE when its body is pure over its arguments, and the
        // view calls `DomainsController.caption_of_button(domain)` —
        // a static call, which is right because there is no per-request
        // state to reach. A marked method that READS REQUEST STATE
        // (campfire's `platform` is `@platform ||= ApplicationPlatform
        // .new(request.user_agent)`) cannot be cloned for exactly that
        // reason, and was left as honest residue. It routes through the
        // live controller instead.
        //
        // `index` is the "already served" test: a name in
        // `helper_method_index` has a module or a class-side clone
        // behind it, and this arm must not take it — doing so turned
        // lobsters' working static call into a dynamic one.
        //
        // Shadowed by the module's own methods and its params, because a
        // partial local spelled `platform` is that local (Rails mixes
        // helpers BENEATH a template's locals). Args are FORWARDED
        // rather than required to be empty: Rails allows a helper_method
        // to take them.
        if let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node {
            if view_visible.contains(method)
                && !index.contains_key(method)
                && !own_methods.contains(method)
                && !own_params.contains(method)
            {
                let span = expr.span;
                let controller = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            span,
                            ExprNode::Const {
                                path: vec![
                                    Symbol::from("ActionController"),
                                    Symbol::from("Current"),
                                ],
                            },
                        )),
                        method: Symbol::from("controller"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                *expr.node = ExprNode::Send {
                    recv: Some(controller),
                    method: method.clone(),
                    args: args.clone(),
                    block: block.clone(),
                    parenthesized: !args.is_empty(),
                };
                return;
            }
        }
        if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
            let m = method.as_str();
            if (m == "request" || m == "cookies" || m == "session" || m == "flash"
                || m == "params")
                && args.is_empty()
            {
                let span = expr.span;
                let current = Expr::new(
                    span,
                    ExprNode::Const {
                        path: vec![
                            Symbol::from("ActionController"),
                            Symbol::from("Current"),
                        ],
                    },
                );
                let (recv, meth) = if m == "request" {
                    (current, Symbol::from("request"))
                } else {
                    (
                        Expr::new(
                            span,
                            ExprNode::Send {
                                recv: Some(current),
                                method: Symbol::from("controller"),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        ),
                        Symbol::from(m),
                    )
                };
                *expr.node = ExprNode::Send {
                    recv: Some(recv),
                    method: meth,
                    args: vec![],
                    block: None,
                    parenthesized: false,
                };
                return;
            }
        }
    }

    // Case 1: collapse `<…Base>.helpers` to the ViewHelpers module constant.
    if is_base_dot_helpers(&expr.node) {
        *expr.node = ExprNode::Const { path: view_helpers_path() };
        return;
    }

    // Case 2: collapse `Rails.application.routes.url_helpers` to RouteHelpers.
    if is_rails_url_helpers(&expr.node) {
        *expr.node = ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] };
        return;
    }

    // Bare `<x>_url` whose `<x>_path` sibling is generated — the
    // absolute variant grounds to protocol + configured domain + the
    // path helper (same convention as `rewrite_url_helpers_absolute`'s
    // host-kwarg form): `"http://#{Rails.application.domain}#{
    // RouteHelpers.<x>_path(args)}"`. Lobsters' hats page links
    // `request_hat_url` bare.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
        if let Some(stem) = method.as_str().strip_suffix("_url") {
            let path_name = Symbol::from(format!("{stem}_path"));
            if route_helpers.contains(&path_name) {
                let span = expr.span;
                let args = args.clone();
                let domain = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            span,
                            ExprNode::Send {
                                recv: Some(Expr::new(
                                    span,
                                    ExprNode::Const { path: vec![Symbol::from("Rails")] },
                                )),
                                method: Symbol::from("application"),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        )),
                        method: Symbol::from("domain"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                let path_call = Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(
                            span,
                            ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
                        )),
                        method: path_name,
                        args,
                        block: None,
                        parenthesized: true,
                    },
                );
                *expr.node = ExprNode::StringInterp {
                    parts: vec![
                        crate::expr::InterpPart::Text { value: "http://".to_string() },
                        crate::expr::InterpPart::Expr { expr: domain },
                        crate::expr::InterpPart::Expr { expr: path_call },
                    ],
                };
                return;
            }
        }
    }

    // Cases 3/4: a bare call resolving to an app or framework helper module.
    let path: Option<Vec<Symbol>> = match &*expr.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            // A method's own PARAM shadows the helper index only as a BARE
            // reference (no args, no block) — that's a local read. `tag(x)`
            // or `tag { }` is a method call that still needs helper
            // qualification, so a same-named param must NOT capture it
            // (that dropped the qualification → latent NameError). Real
            // own-METHODS shadow at any arity (a self-dispatch).
            let param_shadow =
                own_params.contains(method) && args.is_empty() && block.is_none();
            if own_methods.contains(method) || param_shadow {
                // Self wins: a bare call naming a method the enclosing
                // class itself defines is a self-dispatch (Tag#to_param's
                // `tag` column reader), not a helper reference — Rails
                // helpers are mixed in beneath the receiver's own
                // methods, and models never see helpers at all. Without
                // this, an app helper that happens to share a model
                // accessor's name (lobsters' ApplicationHelper#tag
                // builder override vs Tag#tag) captures every read.
                None
            } else if let Some(module) = index.get(method) {
                Some(module.0.as_str().split("::").map(Symbol::from).collect())
            } else if method.as_str() == "pluralize" && args.len() == 2 {
                // Count-labeling `pluralize(count, word)` in a helper
                // body — the same home the view pipeline's classifier
                // already grounds to (`Inflector.pluralize`, the
                // spinel-blog convention), NOT a second ViewHelpers
                // impl. Two-arg form only: the optional plural-word /
                // locale variants aren't in the runtime's surface, so
                // they stay verbatim rather than mis-bind arity.
                Some(vec![Symbol::from("Inflector")])
            } else if is_framework_view_helper(method.as_str()) {
                Some(view_helpers_path())
            } else if route_helpers.contains(method) {
                Some(vec![Symbol::from("RouteHelpers")])
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(path) = path {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { method, mut args, block, .. } = node else { unreachable!() };
        // `link_to(37, url)` — Rails stringifies the text arg; the runtime
        // link_to is deliberately monomorphic (String text), so coercion
        // belongs here at the call boundary. Literal strings stay bare.
        if matches!(method.as_str(), "link_to" | "link_to_raw") {
            if let Some(text) = args.first_mut() {
                if !matches!(
                    &*text.node,
                    ExprNode::Lit { value: crate::expr::Literal::Str { .. } }
                ) {
                    let inner = std::mem::replace(
                        &mut *text.node,
                        ExprNode::Seq { exprs: vec![] },
                    );
                    *text.node = ExprNode::Send {
                        recv: Some(Expr::new(text.span, inner)),
                        method: Symbol::from("to_s"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    };
                }
            }
        }
        *expr.node = ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path })),
            method,
            args,
            block,
            parenthesized: true,
        };
    }

    // `RouteHelpers.<x>_path(format: :rss)` → `RouteHelpers.<x>_path +
    // ".rss"`. Rails path helpers accept `format:` whether or not the
    // route spells `(.:format)`; the IR has no keyword params, so the
    // suffix moves to the call site — arity-independent, and helpers
    // without a format slot stay format-capable.
    let format_suffix = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str().ends_with("_path")
                && matches!(&*r.node, ExprNode::Const { path }
                    if path.last().map(|s| s.as_str() == "RouteHelpers").unwrap_or(false)) =>
        {
            match args.last().map(|a| &*a.node) {
                Some(ExprNode::Hash { entries, kwargs: true }) if entries.len() == 1 => {
                    match &*entries[0].0.node {
                        ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                            if value.as_str() == "format" =>
                        {
                            Some(entries[0].1.clone())
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(fmt) = format_suffix {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv, method, mut args, block, parenthesized } = node else {
            unreachable!()
        };
        args.pop();
        let call = Expr::new(span, ExprNode::Send { recv, method, args, block, parenthesized });
        let suffix = Expr::new(
            span,
            ExprNode::StringInterp {
                parts: vec![
                    crate::expr::InterpPart::Text { value: ".".to_string() },
                    crate::expr::InterpPart::Expr { expr: fmt },
                ],
            },
        );
        *expr.node = ExprNode::Send {
            recv: Some(call),
            method: Symbol::from("+"),
            args: vec![suffix],
            block: None,
            parenthesized: false,
        };
        return;
    }

    // `link_to(raw(x), …)` → `link_to_raw(x, …)`: Rails skips the label
    // escape for a safe buffer; with no safe-buffer type the exemption is
    // decided here. Children were rewritten first (and the bare-call
    // rewrite above may have just fired), so both calls are already in
    // their ViewHelpers.* form.
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr.node {
        if method.as_str() == "link_to" && is_view_helpers_const(r) && !args.is_empty() {
            let raw_inner = match &*args[0].node {
                ExprNode::Send { recv: Some(r2), method: m2, args: a2, .. }
                    if m2.as_str() == "raw"
                        && a2.len() == 1
                        && is_view_helpers_const(r2) =>
                {
                    Some(a2[0].clone())
                }
                _ => None,
            };
            if let Some(inner) = raw_inner {
                *method = Symbol::from("link_to_raw");
                args[0] = inner;
            }
        }
    }

    // Unwrap the view walker's default `html_escape(<call>.to_s)` when
    // the call is html_safe (see is_html_safe_call) — Rails doesn't
    // escape safe buffers, and escaping them ships literal &lt;img&gt;.
    let unwrap: Option<Expr> = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "html_escape"
                && args.len() == 1
                && is_view_helpers_const(r)
                && is_html_safe_call(strip_to_s(&args[0]), index) =>
        {
            Some(args[0].clone())
        }
        _ => None,
    };
    if let Some(inner) = unwrap {
        *expr = inner;
    }
}

/// Ruby-emit-path pass: wrap each action's html render in the layout
/// call — `render(Views::X.y(...))` → `render(Views::Layouts.application(
/// Views::X.y(...), @<ivar>…, @flash…))`. Lives here (not the shared
/// controller lowering) because the wrap shape and the CRuby dispatch
/// contract move together: the overlay main.rb ships `controller.body`
/// verbatim, while other targets' dispatchers still wrap body-only.
/// The controller render seam is where a layout's ivar reads (@user,
/// @title) are statically in scope — the generic dispatch had no way to
/// pass them. Skipped renders: jbuilder json (`*_json` view call or a
/// `content_type:` kwarg), non-view renders (`render html:`/`plain:` —
/// Rails skips the layout for those too), and an already-wrapped
/// Layouts call (idempotence). No-op when the app has no
/// layouts/application view.
pub(crate) fn apply_layout_lowering(lcs: &mut [LibraryClass], app: &App) {
    // Cheap probe: no layouts/application view → nothing to wrap.
    let probe = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Lit { value: crate::expr::Literal::Nil },
    );
    if crate::lower::view_to_library::layout_wrap_expr(app, probe).is_none() {
        return;
    }
    for lc in lcs.iter_mut() {
        // `layout false` on the controller this class came from, scoped
        // by the `only:`/`except:` the declaration carried. Resolved
        // per METHOD because the scoping is per action: campfire's
        // `MessagesController` writes `layout false, only: :index`, so
        // `index` must render bare — it is a turbo-frame fragment
        // loaded into a page that already has a layout, and wrapping it
        // shipped a whole second `<html>` inside the first, 117 tags
        // Rails does not send — while `show` and `edit` keep theirs.
        let decl = app
            .controllers
            .iter()
            .find(|c| c.name == lc.name)
            .map(|c| c.layout.clone());
        for m in &mut lc.methods {
            if decl.as_ref().is_some_and(|d| d.suppresses(&m.name)) {
                continue;
            }
            rewrite_layout_wrap(&mut m.body, app);
        }
    }
}

fn rewrite_layout_wrap(expr: &mut Expr, app: &App) {
    expr.node.for_each_child_mut(&mut |c| rewrite_layout_wrap(c, app));
    // Post-lowering action bodies carry `render` as a SelfRef-receiver
    // Send (`self.render(...)` shape); accept the bare form too.
    let ExprNode::Send { recv, method, args, .. } = &mut *expr.node else {
        return;
    };
    let self_recv = match recv {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef),
    };
    if !self_recv || method.as_str() != "render" || args.is_empty() {
        return;
    }
    // Extract-and-strip any `layout:` kwarg first: the shared lowering
    // keeps it as the wrap marker for body renders (`render html: X,
    // layout: "application"`), and the runtime `render(body, status:,
    // content_type:, location:)` doesn't accept it, so it must never
    // survive to the call.
    let mut layout_kwarg: Option<Expr> = None;
    for a in args.iter_mut().skip(1) {
        if let ExprNode::Hash { entries, .. } = &mut *a.node {
            entries.retain(|(k, v)| {
                let is_layout = matches!(
                    &*k.node,
                    ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                        if value.as_str() == "layout"
                );
                if is_layout {
                    layout_kwarg = Some(v.clone());
                }
                !is_layout
            });
        }
    }
    args.retain(|a| !matches!(&*a.node, ExprNode::Hash { entries, .. } if entries.is_empty()));
    if args.is_empty() {
        return;
    }
    // An explicit `layout: false` VETOES the wrap outright — the
    // controller-side partial-render rewrite plants it (Rails renders
    // partials without a layout), and a user's `render …, layout:
    // false` means the same thing.
    if layout_kwarg.as_ref().is_some_and(|v| {
        matches!(&*v.node, ExprNode::Lit { value: crate::expr::Literal::Bool { value: false } })
    }) {
        return;
    }
    // An explicit `layout: "application"` / `layout: true` wraps a body
    // render (non-Views literal html) the way Rails does. Other layout
    // names are left unwrapped — honest residue, only `application`
    // exists as an emitted layout.
    let layout_requested = layout_kwarg.as_ref().is_some_and(|v| match &*v.node {
        ExprNode::Lit { value: crate::expr::Literal::Str { value } } => value == "application",
        ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => {
            value.as_str() == "application"
        }
        ExprNode::Lit { value: crate::expr::Literal::Bool { value } } => *value,
        _ => false,
    });
    // Trailing kwargs are fine (`status: :unprocessable_entity` renders
    // WITH layout in Rails) — except the jbuilder branch's
    // `content_type:`, which marks a non-html response.
    let has_content_type = args.iter().skip(1).any(|a| match &*a.node {
        ExprNode::Hash { entries, .. } => entries.iter().any(|(k, _)| {
            matches!(&*k.node, ExprNode::Lit { value: crate::expr::Literal::Sym { value } }
                if value.as_str() == "content_type")
        }),
        _ => false,
    });
    if has_content_type {
        return;
    }
    // An explicit `layout: "application"`/`true` wraps WHATEVER the
    // body is — `render html: content.html_safe, layout:
    // "application"` (lobsters /u serves a cached tree string) has a
    // Send body that isn't a Views call, and the Views-shape test
    // alone wrongly skipped it. Without an explicit request, only a
    // non-layout non-json Views call wraps (the implicit html render).
    let wrappable = layout_requested
        || match &*args[0].node {
            ExprNode::Send { recv: Some(r), method: vm, .. } => {
                !vm.as_str().ends_with("_json")
                    && matches!(&*r.node, ExprNode::Const { path }
                        if path.len() == 2
                            && path[0].as_str() == "Views"
                            && path[1].as_str() != "Layouts")
            }
            _ => false,
        };
    if !wrappable {
        return;
    }
    let inner = args[0].clone();
    if let Some(wrapped) = crate::lower::view_to_library::layout_wrap_expr(app, inner) {
        args[0] = wrapped;
    }
}

// `record.update!(k: v, ...)` kwargs inlining moved to the shared
// post-analyze hook (`lower::apply_update_kwargs_inline`) — hook
// bodies arrive here already in writer-assign + save form, with
// unknown-receiver and impure-receiver sites on the residue ledger.

// `errors.add(:field, "msg")` grounding moved to the shared
// post-analyze hook (`lower::apply_errors_add_lowering`) — every hook
// body arrives here already rewritten to the `errors << "Field msg"`
// accumulator shape, with non-self receivers on the residue ledger.

/// Ground `Rails.application.routes.url_helpers.<x>_url(record?, host: H,
/// protocol: P)` → `"#{P}://#{H}#{RouteHelpers.<x>_path(record?)}"`. The
/// routing object graph behind `url_helpers` isn't modeled (and never
/// needs to be for this shape — an absolute URL is protocol + host + the
/// generated path helper). Leading positional args (the record in
/// `user_url(sender, host:)`) and any non-host/protocol kwargs (real
/// path params) are forwarded to the `<x>_path` helper; the record rides
/// whole so its custom `to_param` resolves. Non-matching url_helpers
/// uses are left alone. Applied to the Rails::Application reopen (whose
/// kwargs-only `root_url` is the original occurrence) and mod_note's
/// `user_url(sender, host:)`.
pub(crate) fn rewrite_url_helpers_absolute(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_url_helpers_absolute);
    let matches = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(uh), method, args, block: None, .. }
            if method.as_str().ends_with("_url")
                && !args.is_empty()
                && matches!(&*args[args.len() - 1].node, ExprNode::Hash { .. })
                && is_url_helpers_chain(uh)
    );
    if !matches {
        return;
    }
    let span = expr.span;
    let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
    let ExprNode::Send { method, mut args, .. } = node else { unreachable!() };
    // Split leading positional path args (e.g. the record in
    // `user_url(sender, host:)`) from the trailing kwargs hash.
    let trailing = args.pop().unwrap();
    let positional = args;
    let ExprNode::Hash { entries, .. } = &*trailing.node else { unreachable!() };
    let mut host: Option<Expr> = None;
    let mut protocol: Option<Expr> = None;
    let mut path_kwargs: Vec<(Expr, Expr)> = Vec::new();
    for (k, v) in entries {
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
            match value.as_str() {
                "host" => {
                    host = Some(v.clone());
                    continue;
                }
                "protocol" => {
                    protocol = Some(v.clone());
                    continue;
                }
                _ => {}
            }
        }
        // Any non-host/protocol key is a real path parameter — forward it.
        path_kwargs.push((k.clone(), v.clone()));
    }
    // Forward leading positionals plus any surviving path kwargs to the
    // generated `<stem>_path` helper (the record rides whole, so custom
    // `to_param` — User=username — resolves inside the path helper).
    let mut path_args: Vec<Expr> = positional;
    if !path_kwargs.is_empty() {
        path_args.push(Expr::new(
            trailing.span,
            ExprNode::Hash { entries: path_kwargs, kwargs: true },
        ));
    }
    let stem = method.as_str().trim_end_matches("_url");
    let path_call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
            )),
            method: Symbol::from(format!("{stem}_path")),
            args: path_args,
            block: None,
            parenthesized: true,
        },
    );
    let lit = |s: &str| crate::expr::InterpPart::Text { value: s.to_string() };
    let dyn_part = |e: Expr| crate::expr::InterpPart::Expr { expr: e };
    let mut parts: Vec<crate::expr::InterpPart> = Vec::new();
    if let Some(p) = protocol {
        parts.push(dyn_part(p));
    } else {
        parts.push(lit("http"));
    }
    parts.push(lit("://"));
    if let Some(h) = host {
        parts.push(dyn_part(h));
    }
    parts.push(dyn_part(path_call));
    *expr.node = ExprNode::StringInterp { parts };
    expr.ty = Some(crate::ty::Ty::Str);
}

fn is_url_helpers_chain(e: &Expr) -> bool {
    let ExprNode::Send { recv: Some(routes), method, .. } = &*e.node else { return false };
    if method.as_str() != "url_helpers" {
        return false;
    }
    let ExprNode::Send { recv: Some(rails_app), method: routes_m, .. } = &*routes.node else {
        return false;
    };
    if routes_m.as_str() != "routes" {
        return false;
    }
    matches!(&*rails_app.node, ExprNode::Send { recv: Some(r), method: app_m, .. }
        if app_m.as_str() == "application"
            && matches!(&*r.node, ExprNode::Const { path }
                if path.len() == 1 && path[0].as_str() == "Rails"))
}

// Mailer class-side wrappers (`def self.notify = new.notify(...)`)
// moved to the shared post-analyze hook
// (`lower::apply_mailer_class_side`) — mailer classes arrive here
// with the wrappers already synthesized, keyword/block-taking methods
// on the residue ledger.

// Block-form `create!/create do |kv| ... end` inlining moved to the
// shared post-analyze hook (`lower::apply_create_block_inline`) —
// every hook body arrives here with the factory block already inlined
// (kv = X.new; body; save-or-raise; kv).

/// View-pipeline vestige of the shared `Time.current` grounding
/// (`lower::apply_time_current_lowering`): the post-analyze hook skips
/// view bodies, so lowered view classes still take the rewrite here.
/// Delete when the view pipeline migrates to shared lowerings. Every
/// other body class arrives already grounded (re-running is an
/// idempotent no-op — `Time.current` no longer occurs).
pub(crate) fn apply_time_current_lowering(lcs: &mut [LibraryClass], app: &App) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            crate::lower::time_current::rewrite_time_current(&mut m.body, &app.time_formats);
        }
    }
}

/// View-pipeline vestige of the shared duration grounding
/// (`lower::apply_duration_lowering`): the post-analyze hook skips view
/// bodies, so lowered view classes still take the rewrite here
/// (lobsters' `_commentbox.html.erb` both subtracts a Duration from a
/// Time and calls `before?` on the result). Delete when the view
/// pipeline migrates to shared lowerings. Every other body class
/// arrives already grounded (re-running is an idempotent no-op — the
/// grounded forms no longer match).
///
/// The Time-vs-Duration comparison rule reads a receiver type the view
/// pipeline doesn't stamp, so in practice only the unit, predicate and
/// arithmetic rules bite here; running the full sequence keeps the one
/// order declaration (`lower::duration::apply_duration_rewrites`).
/// Ruby-family pre-emit pass: a fixture ACCESSOR is memoized, because
/// Rails' is.
///
/// `ActiveRecord::TestFixtures` keeps a `@fixture_cache` and clears it
/// between tests, so two `accounts(:signal)` calls in one test answer
/// the SAME object. The lowered accessor is `Account.find(1)` — a new
/// object every call — so campfire's `accounts(:signal).settings.
/// restrict_room_creation_to_administrators = true` followed by
/// `accounts(:signal).save!` is a mutation of one record and a save of
/// a DIFFERENT one. The write
/// vanished and campfire's room-creation restriction was never in
/// force. It is also what makes `.reload` mean anything: reloading a
/// throwaway instance is a no-op by construction.
///
/// RUBY-FAMILY, not the shared lowering, because the cache is CLASS
/// STATE on a module — `@__fx_<label>` inside a `def self.<label>`.
/// Rust, Crystal and Swift each build a fixture module too, and none of
/// them has a home for that slot; measured, all three dropped the
/// accessor entirely (`cannot find function 'one' in module
/// fixtures::articles`). Their accessors keep the per-call lookup,
/// which is the behaviour they have today.
///
/// The slot is cleared at the top of `_fixtures_load!` rather than by a
/// separate hook: the loader runs once per test, right after the schema
/// reset, so that IS the between-tests boundary.
///
/// Spelled `@slot = <lookup> if @slot.nil?` then a bare read, not
/// `@slot ||= <lookup>` — the python lowering declines `OpAssign`, and
/// the two-statement form leaves the method ending in a read.
pub(crate) fn apply_fixture_memoization(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        let labels: Vec<Symbol> = lc
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Class && is_fixture_lookup(&m.body))
            .map(|m| m.name.clone())
            .collect();
        if labels.is_empty() {
            continue;
        }
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Class {
                continue;
            }
            if is_fixture_lookup(&m.body) {
                m.body = memoized_fixture_body(&m.name, &m.body);
            } else if m.name.as_str() == "_fixtures_load!" {
                let mut exprs: Vec<Expr> = labels
                    .iter()
                    .map(|label| {
                        Expr::new(
                            Span::synthetic(),
                            ExprNode::Assign {
                                target: LValue::Ivar { name: fixture_cache_ivar(label) },
                                value: Expr::new(
                                    Span::synthetic(),
                                    ExprNode::Lit { value: Literal::Nil },
                                ),
                            },
                        )
                    })
                    .collect();
                exprs.push(m.body.clone());
                m.body = Expr::new(Span::synthetic(), ExprNode::Seq { exprs });
            }
        }
    }
}

/// The lowered accessor shape — `<Model>.find(<literal id>)` and
/// nothing else. `by_label` (an if/elsif chain) and `_fixtures_load!`
/// (a Seq of inserts) both fail this, which is how the pass tells the
/// three kinds of method in a fixture module apart without knowing
/// their names.
fn is_fixture_lookup(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Send { recv: Some(r), method, args, block: None, .. }
        if method.as_str() == "find"
            && args.len() == 1
            && matches!(&*args[0].node, ExprNode::Lit { value: Literal::Int { .. } })
            && matches!(&*r.node, ExprNode::Const { .. }))
}

/// `@__fx_<label>` — the cache slot. Prefixed so it cannot collide with
/// anything the fixture file names.
fn fixture_cache_ivar(label: &Symbol) -> Symbol {
    Symbol::from(format!("__fx_{}", label.as_str()))
}

fn memoized_fixture_body(label: &Symbol, lookup: &Expr) -> Expr {
    let slot = fixture_cache_ivar(label);
    let read = || {
        let mut e = Expr::new(Span::synthetic(), ExprNode::Ivar { name: slot.clone() });
        e.ty = lookup.ty.clone();
        e
    };
    let fill = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(read()),
                    method: Symbol::from("nil?"),
                    args: vec![],
                    block: None,
                    parenthesized: true,
                },
            ),
            then_branch: Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: slot.clone() },
                    value: lookup.clone(),
                },
            ),
            else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        },
    );
    let mut seq = Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![fill, read()] });
    seq.ty = lookup.ty.clone();
    seq
}

pub(crate) fn apply_duration_lowering(lcs: &mut [LibraryClass], app: &App) {
    let temporal_predicates = !crate::lower::duration::app_defines_temporal_predicates(app);
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            crate::lower::duration::apply_duration_rewrites(&mut m.body, temporal_predicates);
        }
    }
}

/// A record handed to a path helper becomes its slug HERE, at the call
/// site, not inside the helper.
///
/// Rails puts `to_param` in the helper body and gets away with it: the
/// param is untyped, so the record arrives whole and `to_param`
/// dispatches to the model's override. Our helpers declare their
/// segments `String` (`routes_to_library::param_ty` — a slug segment
/// typed Int made every strict-target call site passing `short_id` a C
/// type error), and a declared type is a promise the caller has to
/// keep. Under AOT it IS kept: a `Tag` handed to a `String` slot is
/// coerced on the way in, so the helper body never saw a record at all
/// — `tag_path(tag)` rendered `/t/` with an EMPTY segment long before
/// `String#to_param` raised on it. The raise was the visible half of a
/// silent wrong-URL bug.
///
/// So the conversion moves to where the value's identity is still
/// known. Three signals, in order of how much they know:
///
///   1. the stamped type is a model that overrides `to_param` —
///      analyzed bodies (models, controllers) answer here;
///   2. the arg reads a SINGULAR association (`story.domain`,
///      `showing_user.invited_by_user`) whose target overrides it —
///      the `reference_targets` relation, rebuilt from `app.models`
///      because view bodies reach emit with `Ty::Var` args;
///   3. the arg is a bare name that IS a model name (`tag`, `@user`) —
///      the same convention the view lowerer's `ivar_ty` commits to.
///
/// Everything else is left alone, which is the whole reason the rule is
/// positive-signal-only: `tag.id`, `story.short_id`, `user.username`,
/// `user[:username]`, `@params["username"]` and an already-written
/// `story.to_param` all reach a path helper in this corpus, and every
/// one of them is already a slug. A rule phrased as "wrap unless it
/// looks scalar" would have to enumerate those instead, and would wrap
/// the next unfamiliar shape by default.
pub(crate) fn apply_route_param_lowering(lcs: &mut [LibraryClass], app: &App) {
    let all_models: std::collections::HashSet<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    if all_models.is_empty() {
        return;
    }
    let slug_models = models_overriding_to_param(app);
    let assoc_targets = singular_association_targets(app);
    let collection_targets = collection_association_targets(app);
    // `direct :name do |record, options| … end` helpers take their
    // arguments VERBATIM — Rails hands them to the block, and the block
    // decides what to read. campfire's
    // `direct :fresh_user_avatar do |user, options| route_for
    // :user_avatar, user.avatar_token, v: user.updated_at… end` wants
    // the RECORD; projecting it to `user.id` first handed the block an
    // Integer and every call died on `undefined method 'updated_at'`.
    // Skipping them by name is the one place this pass can tell the
    // difference: a generated resource helper's segment is a param, a
    // direct helper's argument is whatever its block says it is.
    let direct_helpers: std::collections::HashSet<String> = app
        .routes
        .direct_helpers
        .iter()
        .map(|h| format!("{}_path", h.name.as_str()))
        .collect();
    let class_method_targets = record_answering_class_methods(app);
    let mut sig = RecordSignals {
        all_models: &all_models,
        slug_models: &slug_models,
        assoc_targets: &assoc_targets,
        collection_targets: &collection_targets,
        class_method_targets: &class_method_targets,
        direct_helpers: &direct_helpers,
        self_returns: std::collections::HashMap::new(),
    };
    // What each INSTANCE method in this slice answers, to a fixpoint.
    //
    // Across the whole slice, not per class, because the caller and the
    // method are routinely in different ones: campfire's
    // `WelcomeController#index` reads `last_room_visited`, which
    // `ApplicationController` declares. Walking an ancestor chain would
    // say the same thing for these two and less for a concern; the
    // uniqueness rule below is what keeps a slice-wide map honest, and
    // it is the same standard the association maps hold.
    //
    // A FIXPOINT because the methods call each other:
    // `last_room_visited` is `…find_by(id: cookie) || default_room`,
    // and `default_room` has to be resolved first. Bounded by the
    // method count; two rounds in practice.
    {
        let mut ambiguous: std::collections::HashSet<String> = std::collections::HashSet::new();
        for _ in 0..8 {
            let mut grew = false;
            for lc in lcs.iter() {
                for m in &lc.methods {
                    let name = m.name.as_str().to_string();
                    if m.receiver != MethodReceiver::Instance || ambiguous.contains(&name) {
                        continue;
                    }
                    let Some(model) = arg_record_model(body_tail(&m.body), &sig) else {
                        continue;
                    };
                    match sig.self_returns.get(&name) {
                        Some(existing) if existing == &model => {}
                        Some(_) => {
                            ambiguous.insert(name.clone());
                            sig.self_returns.remove(&name);
                            grew = true;
                        }
                        None => {
                            sig.self_returns.insert(name, model);
                            grew = true;
                        }
                    }
                }
            }
            if !grew {
                break;
            }
        }
    }
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_route_params(&mut m.body, &sig);
        }
    }
}

/// The last expression a body evaluates to — its return value for
/// every shape this pass reads. A `Seq`'s tail; anything else is
/// already the tail.
fn body_tail(body: &Expr) -> &Expr {
    match &*body.node {
        ExprNode::Seq { exprs } => exprs.last().unwrap_or(body),
        _ => body,
    }
}

/// Model class methods that answer ONE RECORD of their own model —
/// a body whose tail is `.first` / `.last`.
///
/// campfire's `Room.original` (`order(:created_at).first`) is the
/// caller: `default_room` reads `Current.user.rooms.original`, whose
/// receiver is a `has_many :through` and so carries no type at all.
/// The METHOD NAME carries it instead, on the same uniqueness rule the
/// association maps use — a name two models declare answers None.
fn record_answering_class_methods(app: &App) -> std::collections::HashMap<String, String> {
    let mut out: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut ambiguous: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Read from `app.models`, not from the LibraryClass slice this pass
    // was handed: the pass runs over CONTROLLERS too, and the class
    // method a controller body names lives on a model that slice does
    // not contain.
    for model_def in &app.models {
        let model = model_def.name.0.as_str().to_string();
        for m in model_def.body.iter().filter_map(|item| match item {
            crate::dialect::ModelBodyItem::Method { method, .. } => Some(method),
            _ => None,
        }) {
            if m.receiver != MethodReceiver::Class {
                continue;
            }
            let tail = body_tail(&m.body);
            let answers_one = matches!(&*tail.node,
                ExprNode::Send { recv: Some(_), method, args, block: None, .. }
                    if args.is_empty() && matches!(method.as_str(), "first" | "last"));
            if !answers_one {
                continue;
            }
            let key = m.name.as_str().to_string();
            match out.get(&key) {
                Some(existing) if existing != &model => {
                    ambiguous.insert(key);
                }
                _ => {
                    out.insert(key, model.clone());
                }
            }
        }
    }
    for k in ambiguous {
        out.remove(&k);
    }
    out
}

/// Model names (`Tag`, `User`) whose class defines its own `to_param`.
fn models_overriding_to_param(app: &App) -> std::collections::HashSet<String> {
    app.models
        .iter()
        .filter(|m| {
            m.body.iter().any(|item| matches!(item, crate::dialect::ModelBodyItem::Method { method, .. }
                if method.name.as_str() == "to_param"))
        })
        .map(|m| m.name.0.as_str().to_string())
        .collect()
}

/// Singular association reader → target model name. `belongs_to` and
/// `has_one` only: a `has_many` reader is a collection and never lands
/// in a path segment. An ambiguous name (two models naming the same
/// reader at different targets) is dropped rather than guessed.
/// has_many reader name → target model, when unambiguous across models.
///
/// Only consulted for `.first` / `.last` on such a read: those answer
/// ONE record of the collection's type, which is a record a path helper
/// needs converted. campfire's front door is
/// `redirect_to room_url(Current.user.rooms.last)` — a Send chain, so
/// none of the name-based signals see it, and it redirected to
/// `/rooms/#<Room:0x…>`. An ambiguous name is dropped rather than
/// guessed, exactly as the singular map does.
fn collection_association_targets(app: &App) -> std::collections::HashMap<String, String> {
    use crate::dialect::Association;
    let mut out: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut ambiguous: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &app.models {
        for a in m.associations() {
            let Association::HasMany { name, target, .. } = a else { continue };
            let key = name.as_str().to_string();
            let val = target.0.as_str().to_string();
            match out.get(&key) {
                Some(existing) if existing != &val => {
                    ambiguous.insert(key);
                }
                _ => {
                    out.insert(key, val);
                }
            }
        }
    }
    for k in ambiguous {
        out.remove(&k);
    }
    out
}

fn singular_association_targets(app: &App) -> std::collections::HashMap<String, String> {
    use crate::dialect::Association;
    let mut out: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut ambiguous: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &app.models {
        for a in m.associations() {
            let (name, target) = match a {
                Association::BelongsTo { name, target, .. }
                | Association::HasOne { name, target, .. } => (name, target),
                _ => continue,
            };
            let key = name.as_str().to_string();
            let val = target.0.as_str().to_string();
            match out.get(&key) {
                Some(existing) if existing != &val => {
                    ambiguous.insert(key);
                }
                _ => {
                    out.insert(key, val);
                }
            }
        }
    }
    for k in ambiguous {
        out.remove(&k);
    }
    out
}

/// Everything `arg_record_model` reads, in one place — the signal set
/// grew past what a positional argument list explains.
///
/// `self_returns` is the one that varies per LibraryClass: it maps THIS
/// class's own method names to the model each one answers, so a bare
/// `room_path(last_room_visited)` can resolve through the method it
/// names. The other four are App-wide.
pub(crate) struct RecordSignals<'a> {
    all_models: &'a std::collections::HashSet<String>,
    slug_models: &'a std::collections::HashSet<String>,
    assoc_targets: &'a std::collections::HashMap<String, String>,
    collection_targets: &'a std::collections::HashMap<String, String>,
    /// A model CLASS METHOD whose body answers one record of that model
    /// — `Room.original` is `order(:created_at).first`. Unambiguous
    /// names only.
    class_method_targets: &'a std::collections::HashMap<String, String>,
    direct_helpers: &'a std::collections::HashSet<String>,
    self_returns: std::collections::HashMap<String, String>,
}

fn rewrite_route_params(expr: &mut Expr, sig: &RecordSignals<'_>) {
    expr.node.for_each_child_mut(&mut |e| rewrite_route_params(e, sig));
    let is_helper_call = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), method, block: None, .. }
            if method.as_str().ends_with("_path")
                && !sig.direct_helpers.contains(method.as_str())
                && matches!(&*r.node, ExprNode::Const { path }
                    if path.last().map(|s| s.as_str() == "RouteHelpers").unwrap_or(false))
    );
    if !is_helper_call {
        return;
    }
    let ExprNode::Send { args, .. } = &mut *expr.node else { unreachable!() };
    for arg in args.iter_mut() {
        let Some(model) = arg_record_model(arg, sig) else {
            continue;
        };
        // A model that overrides `to_param` answers its slug there and
        // the segment is declared `Str`. Every other model's `to_param`
        // IS `id`, and `param_ty` declares that segment `Int` — so call
        // `id` rather than a `to_param` the emitted model doesn't have.
        let slug = sig.slug_models.contains(&model);
        let (method, ty) = if slug {
            ("to_param", crate::ty::Ty::Str)
        } else {
            ("id", crate::ty::Ty::Int)
        };
        let span = arg.span;
        let record = std::mem::replace(arg, Expr::new(span, ExprNode::Seq { exprs: vec![] }));
        *arg = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(record),
                method: Symbol::from(method),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        arg.ty = Some(ty);
    }
}

fn arg_record_model(arg: &Expr, sig: &RecordSignals<'_>) -> Option<String> {
    let all_models = sig.all_models;
    let assoc_targets = sig.assoc_targets;
    let collection_targets = sig.collection_targets;
    // (1) A stamped model type.
    if let Some(crate::ty::Ty::Class { id, .. }) =
        arg.ty.as_ref().map(crate::ty::Ty::peel_nilable)
    {
        if all_models.contains(id.0.as_str()) {
            return Some(id.0.as_str().to_string());
        }
    }
    // (5) `a || b` — Rails' "this one, else that one" (campfire's
    // `last_room_visited` is `…find_by(id: cookie) || default_room`).
    // BOTH sides must answer the SAME model: one side resolving proves
    // nothing about the value that actually arrives.
    if let ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, left, right, .. } = &*arg.node {
        let (l, r) = (arg_record_model(left, sig), arg_record_model(right, sig));
        return match (l, r) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        };
    }
    match &*arg.node {
        // (2) A singular association read, or (4) `.first` / `.last` on
        // a has_many read — one record of the collection's type. Both
        // are receiver-ful zero-arg Sends, so they share an arm: a
        // separate `first`/`last` arm placed after this one would be
        // unreachable, since this pattern matches those method names too.
        ExprNode::Send { recv: Some(r), method, args, block: None, .. } if args.is_empty() => {
            if let Some(target) = assoc_targets
                .get(method.as_str())
                .filter(|target| all_models.contains(*target))
            {
                return Some(target.clone());
            }
            // (7b) `self.<method>` — the same self-call as (7) below,
            // with the receiver spelled out. campfire writes
            // `… || self.default_room`, and the emitted controller
            // keeps that spelling.
            if matches!(&*r.node, ExprNode::SelfRef) {
                if let Some(model) = sig.self_returns.get(method.as_str()) {
                    return Some(model.clone());
                }
            }
            // (6) A model CLASS METHOD that answers one record —
            // campfire's `Current.user.rooms.original`, where `Room
            // .original` is `order(:created_at).first`. The receiver's
            // own type is exactly what does not resolve here; the
            // METHOD NAME is what carries it, on the same uniqueness
            // rule the association maps use.
            if let Some(target) = sig
                .class_method_targets
                .get(method.as_str())
                .filter(|target| all_models.contains(*target))
            {
                return Some(target.clone());
            }
            if !matches!(method.as_str(), "first" | "last") {
                return None;
            }
            let ExprNode::Send { method: assoc, args: aargs, block: None, .. } = &*r.node else {
                return None;
            };
            if !aargs.is_empty() {
                return None;
            }
            collection_targets
                .get(assoc.as_str())
                .filter(|target| all_models.contains(*target))
                .cloned()
        }
        // (4b) `find_by`/`find`/`find_by!` on a has_many read — ONE
        // record of the collection's type, exactly like `.first` above
        // and split out only because those take no arguments. The
        // receiver stays a zero-arg association read: `@room.messages
        // .where(…).find_by(…)` is a chain this pass does not follow.
        ExprNode::Send { recv: Some(r), method, block: None, .. }
            if matches!(method.as_str(), "find" | "find_by" | "find_by!") =>
        {
            let ExprNode::Send { method: assoc, args: aargs, block: None, .. } = &*r.node else {
                return None;
            };
            if !aargs.is_empty() {
                return None;
            }
            collection_targets
                .get(assoc.as_str())
                .filter(|target| all_models.contains(*target))
                .cloned()
        }
        // (3) A bare name that IS a model name — including the
        // zero-arg receiver-less bareword form. A template local
        // (`render "users/user", user: u` → `user` in the partial) and a
        // helper's own reader (`Room::MessagePusher#room`) both parse as
        // a receiver-less Send, not a Var, because prism cannot prove
        // the name is a local inside an ERB-ingested body. Matching only
        // Var/Ivar left campfire's `account_user_path(user)` and
        // `room_path(room)` unconverted — the same owner-form note
        // `scope_chain::owner_model_from_name` carries.
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => {
            let camel = crate::naming::camelize(name.as_str());
            all_models.contains(&camel).then_some(camel)
        }
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            let camel = crate::naming::camelize(method.as_str());
            if all_models.contains(&camel) {
                return Some(camel);
            }
            // (7) …else a call to one of THIS class's own methods,
            // resolved through what that method's body answers.
            // `redirect_to room_path(last_room_visited)` is the shape:
            // the name is not a model's, and nothing upstream typed it.
            sig.self_returns.get(method.as_str()).cloned()
        }
        _ => None,
    }
}

/// Ruby-family pre-emit pass for TRANSPILED-SHAPE classes only: models
/// whose accessors were hand-written in the source (`attr_accessor
/// :created_at`) and so never passed through the shared model lowering.
/// Schema-synthesized models don't need it — `schema::synth_attr_reader`
/// already splits storage (`@<col>_raw`, String) from access (a reader
/// parsing via `ActiveSupport.parse_db_time`), for every target.
///
/// For the hand-written shape (storage under `@<col>` itself), the
/// reader becomes `@col && ActiveSupport.parse_db_time(@col)` — short-
/// circuits a nullable column's `nil` without needing to know
/// nullability; `parse_db_time` (not bare `Time.parse`) treats a
/// zone-less column as UTC rather than the system's local zone (see
/// `active_support_time_parsing.rb`). The writer becomes
/// `@col = (value.respond_to?(:iso8601) ? value.iso8601 : value)` —
/// normalizing a `Time` passed by app code back to text so every write
/// lands on the same on-disk format. A strict no-op for apps with no
/// Date/DateTime/Time columns (the blog) and for schema-synthesized
/// models (the plain-ivar-read gate below).
pub(crate) fn apply_datetime_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        let temporal: BTreeSet<Symbol> = table
            .columns
            .iter()
            .filter(|c| {
                matches!(
                    c.col_type,
                    crate::schema::ColumnType::Date
                        | crate::schema::ColumnType::DateTime
                        | crate::schema::ColumnType::Time
                )
            })
            .map(|c| c.name.clone())
            .collect();
        if temporal.is_empty() {
            continue;
        }
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            match m.kind {
                // Only a PLAIN `@col`-read body — the hand-written
                // `attr_reader` shape from transpiled fixtures. A
                // schema-synthesized model's temporal reader already
                // parses its `@<col>_raw` storage ivar (see
                // `schema::synth_attr_reader`); overwriting it here
                // would re-point the read at a nonexistent `@<col>`.
                AccessorKind::AttributeReader
                    if temporal.contains(&m.name)
                        && is_plain_ivar_read(&m.body, &m.name) =>
                {
                    m.body = temporal_reader_body(&m.name);
                }
                // Hand-written temporal writers only, same reasoning:
                // synthesized models write storage via `<col>_raw=`
                // (never named after a temporal column), so any
                // `<col>=` writer matching the temporal set is the
                // transpiled-fixture shape that needs the Time→text
                // normalize.
                AccessorKind::AttributeWriter => {
                    let col = Symbol::from(m.name.as_str().trim_end_matches('='));
                    if temporal.contains(&col) {
                        if let Some(param) = m.params.first() {
                            m.body = temporal_writer_body(&col, &param.name);
                        }
                    }
                    // The synthesized `<col>_raw=` writer's own store
                    // is invalidated by the general walk below, along
                    // with every OTHER bare write to the same slot.
                }
                // Schema-synthesized temporal reader:
                // `ActiveSupport.parse_db_time(@<col>_raw)`. Profiling
                // the lobsters bench put Date._parse + its regexps at
                // ~4% of wall time — every `created_at` read re-parses
                // the same string. Memoize per instance:
                // `@__t_<col> ||= ActiveSupport.parse_db_time(@<col>_raw)`
                // (writer above invalidates). nil/"" raw re-evaluates
                // each read — parse_db_time's empty path is cheap.
                AccessorKind::Method | AccessorKind::AttributeReader
                    if temporal.contains(&m.name) && is_parse_db_time_body(&m.body) =>
                {
                    m.body = Expr::new(
                        Span::synthetic(),
                        ExprNode::OpAssign {
                            target: LValue::Ivar { name: parse_memo_ivar(&m.name) },
                            op: crate::expr::OpAssignOp::OrOr,
                            value: m.body.clone(),
                        },
                    );
                }
                _ => {}
            }
        }

        // EVERY bare `@<col>_raw = …` invalidates the memo, not just
        // the one inside the `<col>_raw=` writer.
        //
        // `_adapter_reload` is why this is a walk rather than a
        // special case on that writer: it re-reads the row and stores
        // each column with a plain ivar assign (it writes into `self`,
        // where `from_stmt` builds a fresh instance and can go through
        // the writers). The memo survived, so `record.reload.
        // updated_at` answered the value from BEFORE the reload —
        // invisible until something holds one instance across a write,
        // which is exactly what Rails' memoized fixture accessors make
        // routine.
        //
        // Statement positions only (a method body, a `Seq` element, an
        // `If` branch): the replacement is a two-statement `Seq`, and
        // a `Seq` spliced into an expression slot is not Ruby.
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            invalidate_parse_memo_on_raw_writes(&mut m.body, &temporal);
        }

        // The public `<col>=` writer (`self.<col>_raw =
        // ActiveSupport.format_db_time(value)`) is synthesized by the
        // shared model lowering (`schema::synth_temporal_writer`, kind
        // `Method` so the AttributeWriter arm above can't re-point it
        // at a nonexistent `@<col>`) — it arrives here already present,
        // and its raw-writer dispatch picks up the memo invalidation
        // installed above.
    }
}

/// Rewrite every STATEMENT-position `@<col>_raw = …` for a temporal
/// column into `@__t_<col> = nil; @<col>_raw = …`, so the parse memo
/// can never outlive the string it was parsed from.
fn invalidate_parse_memo_on_raw_writes(e: &mut Expr, temporal: &BTreeSet<Symbol>) {
    match &mut *e.node {
        ExprNode::Seq { exprs } => {
            for x in exprs.iter_mut() {
                invalidate_parse_memo_on_raw_writes(x, temporal);
            }
            return;
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            invalidate_parse_memo_on_raw_writes(then_branch, temporal);
            invalidate_parse_memo_on_raw_writes(else_branch, temporal);
            return;
        }
        _ => {}
    }
    let ExprNode::Assign { target: LValue::Ivar { name }, .. } = &*e.node else {
        return;
    };
    let Some(base) = name.as_str().strip_suffix("_raw") else {
        return;
    };
    let base = Symbol::from(base);
    if !temporal.contains(&base) {
        return;
    }
    let span = e.span;
    let store = e.clone();
    *e = Expr::new(
        span,
        ExprNode::Seq {
            exprs: vec![
                Expr::new(
                    span,
                    ExprNode::Assign {
                        target: LValue::Ivar { name: parse_memo_ivar(&base) },
                        value: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
                    },
                ),
                store,
            ],
        },
    );
}

/// `@__t_<col>` — the per-instance memo slot for a parsed temporal
/// column. Underscore-prefixed so it can't collide with a real column.
fn parse_memo_ivar(col: &Symbol) -> Symbol {
    Symbol::from(format!("__t_{}", col.as_str()))
}

/// Is this body exactly `ActiveSupport.parse_db_time(<anything>)` —
/// the synthesized temporal-reader shape?
fn is_parse_db_time_body(body: &Expr) -> bool {
    matches!(&*body.node, ExprNode::Send { recv: Some(r), method, args, .. }
        if method.as_str() == "parse_db_time"
            && args.len() == 1
            && matches!(&*r.node, ExprNode::Const { path }
                if path.len() == 1 && path[0].as_str() == "ActiveSupport"))
}

/// Ruby-family pre-emit pass: SQL NULL survives hydration as real nil.
///
/// The shared `<Model>Row.from_raw` synthesis coerces every scalar slot
/// (`(row["col"] || 0).to_i`, `(row["col"]).to_s`) so strict targets get
/// non-nilable fields — but on the Ruby tree that turns NULL into 0/""
/// and breaks Rails semantics: `group_by(&:invited_by_user_id)[nil]`
/// finds no root users (the /u tree renders empty), `banned_at?` is
/// true for everyone. For NULLABLE, non-primary-key columns, rewrite
/// the slot assign to `row["col"].nil? ? nil : <original coercion>`.
///
/// The fk 0-sentinel convention stays (belongs_to writers store 0 for
/// nil); readers' `@fk == 0` guards are WIDENED to `@fk.nil? || @fk ==
/// 0` so both representations mean "no parent". CRuby-only by
/// placement: strict targets keep the defaulted non-nilable slots
/// until the nullable-column typing workstream lands.
pub(crate) fn apply_hydration_nil_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        let nullable: BTreeSet<Symbol> = table
            .columns
            .iter()
            .filter(|c| c.nullable && !c.primary_key && c.name.as_str() != "id")
            .map(|c| c.name.clone())
            .collect();
        if nullable.is_empty() {
            continue;
        }
        let row_id = ClassId(Symbol::from(format!("{}Row", model.name.0.as_str())));
        if let Some(row_lc) = lcs.iter_mut().find(|lc| lc.name == row_id) {
            for m in &mut row_lc.methods {
                if m.name.as_str() == "from_raw" {
                    nil_guard_from_raw_slots(&mut m.body, &nullable);
                }
            }
        }
        let nullable_fks: BTreeSet<Symbol> = nullable
            .iter()
            .filter(|n| n.as_str().ends_with("_id"))
            .cloned()
            .collect();
        if nullable_fks.is_empty() {
            continue;
        }
        if let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) {
            for m in &mut lc.methods {
                widen_fk_zero_guards(&mut m.body, &nullable_fks);
            }
        }
    }
}

/// Inside a `from_raw` body, rewrite `instance.<col> = <coercion>` for
/// nullable cols to `instance.<col> = (row["col"].nil? ? nil :
/// <coercion-sans-|| default>)`. The lookup is a pure Hash read, so the
/// duplicated evaluation in the guard is safe.
fn nil_guard_from_raw_slots(body: &mut Expr, nullable: &BTreeSet<Symbol>) {
    let ExprNode::Seq { exprs } = &mut *body.node else { return };
    for stmt in exprs.iter_mut() {
        let ExprNode::Send { method, args, .. } = &mut *stmt.node else { continue };
        let Some(col) = method.as_str().strip_suffix('=') else { continue };
        if !nullable.contains(&Symbol::from(col)) {
            continue;
        }
        let Some(value) = args.first_mut() else { continue };
        // Only Cast-wrapped scalars coerce; raw slots (bools) already
        // carry nil through.
        let ExprNode::Cast { value: inner, target_ty } = &*value.node else { continue };
        // Strip a `|| <default>` fallback (the id-shaped-column form) so
        // NULL isn't defaulted before the coercion sees it.
        let lookup = match &*inner.node {
            ExprNode::BoolOp { left, right, .. }
                if matches!(&*right.node, ExprNode::Lit { .. }) =>
            {
                left.clone()
            }
            _ => inner.clone(),
        };
        let nil_check = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(lookup.clone()),
                method: Symbol::from("nil?"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let guarded = Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: nil_check,
                then_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                else_branch: Expr::new(
                    Span::synthetic(),
                    ExprNode::Cast { value: lookup, target_ty: target_ty.clone() },
                ),
            },
        );
        *value = guarded;
    }
}

/// `@<fk> == 0` → `@<fk>.nil? || @<fk> == 0` for nullable fks, walking
/// the whole method body (belongs_to reader guards, app-code sentinel
/// checks alike).
fn widen_fk_zero_guards(expr: &mut Expr, fks: &BTreeSet<Symbol>) {
    expr.node.for_each_child_mut(&mut |c| widen_fk_zero_guards(c, fks));
    let is_sentinel_eq = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "==" && args.len() == 1 =>
        {
            matches!(&*r.node, ExprNode::Ivar { name } if fks.contains(name))
                && matches!(
                    &*args[0].node,
                    ExprNode::Lit { value: Literal::Int { value: 0 } }
                )
        }
        _ => false,
    };
    if !is_sentinel_eq {
        return;
    }
    let ExprNode::Send { recv: Some(r), .. } = &*expr.node else { return };
    let nil_check = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(r.clone()),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let original = expr.clone();
    *expr = Expr::new(
        expr.span,
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::Or,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: nil_check,
            right: original,
        },
    );
}

/// Ruby-family pre-emit pass, companion to
/// `apply_hydration_nil_lowering`: once nullable columns hydrate to
/// real nil, the `.empty?` forms the predicate lowering synthesized
/// from `present?`/`blank?` (which assumed never-nil reads) crash.
/// Rewrite `<recv>.empty?` → `(<recv> || "").empty?` — single
/// evaluation, transparent for arrays/strings that are never nil, and
/// nil reads get Rails' blank-when-nil semantics. Only the
/// zero-arg `empty?` shape is touched.
/// `<time>.rfc2822` → `ActiveSupport.rfc2822(<time>)`.
///
/// `Time#rfc2822` is stdlib `time`, not core Ruby — `require "time"`
/// installs it. CRuby has it because the overlay requires that stdlib;
/// spinel has no `time` package, and the method cannot be supplied by
/// reopening `Time` (a reopened built-in loses its own method table for
/// self-calls, so even `self.strftime` inside the reopen is undefined).
/// So the call grounds to the module function every ruby-family tree
/// ships — the same posture `lower::duration` takes for `Integer#days`,
/// and for the same reason: no built-in reopening.
///
/// Both trees satisfy the one `active_support_time_parsing.rbs`
/// contract. The CRuby/JRuby overlay's implementation delegates
/// straight back to `t.rfc2822`, so grounding the call site cannot move
/// that lane's bytes; the spinel twin composes the same string from
/// strftime.
///
/// Zero-arg receiver sends only. Ruby-emit-path pass: the module
/// function it names is ruby-family runtime, so this must not run from
/// shared `lower/`.
pub(crate) fn apply_time_format_lowering(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_rfc2822(&mut m.body);
        }
    }
}

fn rewrite_rfc2822(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| rewrite_rfc2822(c));
    let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "rfc2822" || !args.is_empty() {
        return;
    }
    let recv = r.clone();
    *expr.node = ExprNode::Send {
        recv: Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
        )),
        method: Symbol::from("rfc2822"),
        args: vec![recv],
        block: None,
        parenthesized: true,
    };
}

pub(crate) fn apply_nilsafe_empty_lowering(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_empty_nilsafe(&mut m.body);
        }
    }
}

/// `::`-root a view body's constant references when the view's own
/// namespace shadows the constant.
///
/// A view emits as `module Views\n  module Stats`, so an unqualified
/// `Stats.get_cached_graph(:users)` inside it resolves LEXICALLY to
/// `Views::Stats` — the view module itself — and not to the top-level
/// `Stats` model the template meant. Ruby finds the inner constant
/// first and raises `NoMethodError: undefined method
/// 'get_cached_graph' for module Views::Stats`.
///
/// This is not a spinel-only concern: the CRuby target emits the same
/// unqualified reference and fails the same way at runtime (GET /stats
/// on lobsters). The AOT lane just surfaces it at compile time.
///
/// The rewrite is deliberately narrow — a head segment is rooted only
/// when it BOTH collides with a namespace visible in the view's lexical
/// scope AND names a real top-level app namespace. Emitted views always
/// spell sibling views fully from the root (`Views::Stories.similar(...)`),
/// so nothing here depends on lexical shorthand and the rooting cannot
/// retarget a reference that was already resolving correctly.
///
/// "Visible in the lexical scope" is the part that was wrong at first:
/// it read the view's OWN segments only. But every view is emitted
/// inside `module Views`, so the scope of ANY view body includes EVERY
/// OTHER view's namespace. campfire's `messages/_message` calls
/// `Users::AvatarsHelper.avatar_tag(...)`; `Views::Users` exists because
/// the app has `app/views/users/` templates, and Ruby stops there —
/// `uninitialized constant Views::Users::AvatarsHelper`, on the page
/// that renders every message.
/// Which shadows this pipeline roots. The view pipeline takes both —
/// every view sits under `module Views` beside every other view. The
/// LIBRARY pipeline takes only the runtime's, because an app class
/// referencing its OWN nested constant (`Sound::Image` inside `class
/// Sound`) already resolves, and rooting it would be churn for nothing.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RootingScope {
    AppAndRuntime,
    RuntimeOnly,
}

pub(crate) fn apply_constant_rooting(
    lcs: &mut [LibraryClass],
    app: &App,
    scope: RootingScope,
) {
    // …and "names a real top-level app namespace" has to mean the
    // NAMESPACE, not a class of exactly that name: campfire has no class
    // called `Users`, only `Users::AvatarsHelper` under it.
    //
    // A name the shared RUNTIME owns counts too, and for the same
    // reason a view's does: campfire's `Message::Broadcasts` concern is
    // where the broadcast lowering emits `Broadcasts.append(...)`, and
    // inside `module Broadcasts` that constant resolves to the concern
    // ITSELF. One authority for "is this the runtime's" —
    // `require_path_for_body_const`, which already had to know.
    let runtime_owned = |name: &str| {
        require_path_for_body_const(&[name.to_string()], app, "")
            .is_some_and(|p| p.starts_with("runtime/"))
    };
    let shadows = |name: &str| {
        if scope == RootingScope::RuntimeOnly {
            return runtime_owned(name);
        }
        let prefix = format!("{name}::");
        let under = |n: &str| n == name || n.starts_with(&prefix);
        app.models.iter().any(|m| under(m.name.0.as_str()))
            || app.library_classes.iter().any(|c| under(c.name.0.as_str()))
            || runtime_owned(name)
    };
    // Every `Views::<Seg>` in this emit, which is exactly the set of
    // names a view body can accidentally resolve to.
    let mut view_namespaces: Vec<String> = lcs
        .iter()
        .filter_map(|lc| lc.name.0.as_str().strip_prefix("Views::"))
        .filter_map(|rest| rest.split("::").next())
        // `Views` itself is never the shadow: a `Views::X` reference
        // inside `module Views` finds no `Views::Views` and lands on the
        // top-level one, which is the intended target.
        .filter(|seg| *seg != "Views" && shadows(seg))
        .map(|seg| seg.to_string())
        .collect();
    view_namespaces.sort();
    view_namespaces.dedup();

    // Every class name this emit defines, app and runtime alike — the
    // authority for "does the inner reading have a target?".
    let class_names: std::collections::HashSet<String> = app
        .models
        .iter()
        .map(|m| m.name.0.as_str().to_string())
        .chain(app.library_classes.iter().map(|c| c.name.0.as_str().to_string()))
        .chain(lcs.iter().map(|c| c.name.0.as_str().to_string()))
        .collect();
    let known = |n: &str| class_names.contains(n);
    for lc in lcs.iter_mut() {
        let name = lc.name.0.as_str();
        let mut shadowing: Vec<String> = name
            .split("::")
            .filter(|seg| shadows(seg))
            .map(|seg| seg.to_string())
            .collect();
        if name.starts_with("Views::") {
            shadowing.extend(view_namespaces.iter().cloned());
        }
        shadowing.sort();
        shadowing.dedup();
        // NESTING the emit introduced. A source file that writes the
        // COMPACT form (`module User::Bot`) has lexical scope
        // [User::Bot, Object] — `User`'s own constants are NOT in it.
        // The emit nests (`class User … module Bot`), which puts them
        // in, and campfire's `Bot::WebhookJob` stopped meaning the
        // top-level job and started meaning the concern itself.
        //
        // So: for every enclosing namespace, the constants it defines
        // are candidates for rooting — but only where the inner reading
        // has NO target for the FULL path. `User::Bot` exists, which is
        // the shadow; `User::Bot::WebhookJob` does not, which is what
        // proves the body meant the outer one.
        let prefixes: Vec<String> = {
            let segs: Vec<&str> = name.split("::").collect();
            (1..segs.len()).map(|i| segs[..i].join("::")).collect()
        };
        for m in &mut lc.methods {
            root_shadowed_constants(&mut m.body, &shadowing, &prefixes, &known);
        }
    }
}

fn root_shadowed_constants(
    expr: &mut Expr,
    shadowing: &[String],
    prefixes: &[String],
    known: &dyn Fn(&str) -> bool,
) {
    expr.node
        .for_each_child_mut(&mut |c| root_shadowed_constants(c, shadowing, prefixes, known));
    let ExprNode::Const { path } = &mut *expr.node else {
        return;
    };
    let joined = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
    let Some(head) = path.first_mut() else { return };
    // Idempotent: a head already carrying the root marker is skipped, so
    // running the pass twice can't produce `::::Stats`.
    if head.as_str().starts_with("::") {
        return;
    }
    // Either the name's OWN segments shadow it (the pre-existing rule:
    // `Views::Stats` referencing `Stats`, `Message::Broadcasts`
    // referencing the runtime's `Broadcasts`) …
    let by_own_segment = shadowing.iter().any(|s| s == head.as_str());
    // … or an ENCLOSING namespace defines it and the full path under
    // that namespace does not exist, which is the nesting the emit
    // introduced. See the prefix note at the call site.
    let by_enclosing = prefixes.iter().any(|p| {
        known(&format!("{p}::{}", head.as_str())) && !known(&format!("{p}::{joined}"))
    });
    if !by_own_segment && !by_enclosing {
        return;
    }
    // The path renders as `path.join("::")`, so a leading marker on the
    // head segment is all an absolute reference needs — there is no
    // separate "absolute" flag in the IR to set.
    *head = Symbol::from(format!("::{}", head.as_str()));
}

/// Collapse a controller's dynamic-render-options assignment to its bare
/// partial-name string: `@above = {partial: "stories/subnav", locals:
/// {…}}` → `@above = "stories/subnav"`. The matching view (`<%= render
/// @above %>`) dispatches over a `case @above when "stories/subnav" …`
/// pool of string names, so the runtime value must be the string. Upstream
/// lobsters uses both this hash form and the bare-string form (`@above =
/// "saved/subnav"`) — normalizing here lets the view stay string-uniform.
/// The `locals:` sub-hash is dropped HERE; the pool dispatch captures it
/// separately (see the DynPoolEntry / strict-locals kwargs path).
///
/// SCOPE: this is a Ruby-emit-path pass, but the `case @x when "…"` string
/// dispatch it feeds is emitted by the SHARED view lowerer. That's correct
/// only while lobsters (the sole dynamic-partial app) is ruby-only — a
/// strict target rendering `render @above` would see the un-collapsed hash
/// and never match a string arm. Revisit when another target grows dynamic
/// partials (move this collapse to the shared controller lowering then).
pub(crate) fn apply_dynamic_render_options_lowering(lcs: &mut [LibraryClass]) {
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_render_options_assign(&mut m.body);
        }
    }
}

fn rewrite_render_options_assign(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| rewrite_render_options_assign(c));
    let ExprNode::Assign { target: crate::expr::LValue::Ivar { .. }, value } = &mut *expr.node
    else {
        return;
    };
    let ExprNode::Hash { entries, .. } = &*value.node else {
        return;
    };
    // Collapse ONLY a render-options hash: symbol keys drawn exactly from
    // {partial, locals}, with a String-literal `partial:`. Gating on the
    // full shape (not merely "contains a partial: key") avoids rewriting an
    // unrelated config hash like `@x = {partial: "y", scope: z}` app-wide —
    // `render @above` is the only consumer of this collapse.
    let mut partial_name: Option<String> = None;
    for (k, v) in entries.iter() {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
            return; // non-symbol key → not a render-options hash
        };
        match key.as_str() {
            "partial" => match &*v.node {
                ExprNode::Lit { value: Literal::Str { value: s } } => {
                    partial_name = Some(s.clone())
                }
                _ => return, // dynamic partial name — leave the hash intact
            },
            "locals" => {} // allowed; dropped in the collapse (ledgered)
            _ => return, // any foreign key → not a render-options hash
        }
    }
    if let Some(name) = partial_name {
        let span = value.span;
        *value = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: name } });
    }
}

fn rewrite_empty_nilsafe(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut |c| rewrite_empty_nilsafe(c));
    let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "empty?" || !args.is_empty() {
        return;
    }
    // Idempotence: an already-guarded `(x || "").empty?` keeps its shape.
    if matches!(&*r.node, ExprNode::BoolOp { right, .. }
        if matches!(&*right.node, ExprNode::Lit { value: Literal::Str { value } } if value.is_empty()))
    {
        return;
    }
    let guarded = Expr::new(
        r.span,
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::Or,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: r.clone(),
            right: Expr::new(
                r.span,
                ExprNode::Lit { value: Literal::Str { value: String::new() } },
            ),
        },
    );
    *r = guarded;
}

/// True when a reader body is exactly `@<name>` — the untouched
/// hand-written `attr_reader` shape (vs a synthesized parsing body).
fn is_plain_ivar_read(body: &Expr, name: &Symbol) -> bool {
    matches!(&*body.node, ExprNode::Ivar { name: n } if n == name)
}

fn datetime_ivar(col: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Ivar { name: col.clone() })
}

fn datetime_var(name: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: name.clone() })
}

/// `@col && ActiveSupport.parse_db_time(@col)`.
fn temporal_reader_body(col: &Symbol) -> Expr {
    // `ActiveSupport.parse_db_time` (not bare `Time.parse`) — a stored
    // column with no zone marker is always implicitly UTC (Rails/sqlite3
    // convention), but `Time.parse` defaults an absent zone to the
    // *system's local zone*. See `active_support_time_parsing.rb`.
    let parse_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
            )),
            method: Symbol::from("parse_db_time"),
            args: vec![datetime_ivar(col)],
            block: None,
            parenthesized: true,
        },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp {
            op: crate::expr::BoolOpKind::And,
            surface: crate::expr::BoolOpSurface::Symbol,
            left: datetime_ivar(col),
            right: parse_call,
        },
    )
}

/// `@col = (value.respond_to?(:iso8601) ? value.iso8601 : value)`.
fn temporal_writer_body(col: &Symbol, value_param: &Symbol) -> Expr {
    let responds = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(datetime_var(value_param)),
            method: Symbol::from("respond_to?"),
            args: vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit {
                    value: crate::expr::Literal::Sym { value: Symbol::from("iso8601") },
                },
            )],
            block: None,
            parenthesized: true,
        },
    );
    let iso_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(datetime_var(value_param)),
            method: Symbol::from("iso8601"),
            args: Vec::new(),
            block: None,
            parenthesized: true,
        },
    );
    let normalized = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: responds,
            then_branch: iso_call,
            else_branch: datetime_var(value_param),
        },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Ivar { name: col.clone() },
            value: normalized,
        },
    )
}

/// Emit both the `.rb` file and its `.rbs` sidecar for a single
/// LibraryClass. The sidecar carries the typed-signature view of the
/// same class shape — spinel reads it as an inference hint (see
/// project_rbs_emit_opportunity.md / spinel#571), and Steep/TypeProf
/// can consume it from the CRuby target.
pub(super) fn emit_library_class_pair(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
) -> Vec<EmittedFile> {
    let rb = emit_library_class_decl(lc, app, out_path.clone());
    let rbs = super::rbs::emit_library_class_rbs(lc, &out_path);
    vec![rb, rbs]
}

/// Pair variant for callers that pass synthesized sibling anchors.
pub(super) fn emit_library_class_pair_with_synthesized(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
) -> Vec<EmittedFile> {
    let rb = emit_library_class_decl_with_synthesized(
        lc,
        app,
        out_path.clone(),
        synthesized_siblings,
    );
    let rbs = super::rbs::emit_library_class_rbs(lc, &out_path);
    vec![rb, rbs]
}

/// Emit a group of LibraryFunctions sharing a `module_path` as a
/// single Ruby file. Mirrors `typescript::library::emit_module_file`
/// — converts the function group into a synthetic
/// `LibraryClass{is_module:true}` with class-method (`def self.X`)
/// declarations, then delegates to `emit_library_class_decl` so
/// require resolution, nested-module rendering, and method body
/// emission share one code path with class-shaped artifacts.
///
/// `module_function` would be the more idiomatic Ruby spelling,
/// but `def self.X` is what the existing spinel-blog hand-written
/// modules use AND what `emit_method` already produces — going
/// through that path keeps shapes byte-identical.
pub(super) fn emit_module_file(
    funcs: &[crate::dialect::LibraryFunction],
    app: &App,
    out_path: PathBuf,
) -> EmittedFile {
    if funcs.is_empty() {
        // No functions in the module — emit a placeholder file with
        // just the module wrapper. Callers can guard upstream by
        // checking the lowerer's output and not calling this when
        // they know the module would be empty.
        return EmittedFile { path: out_path, content: String::new() };
    }
    let lc = synthesize_module_lc(funcs);
    emit_library_class_decl(&lc, app, out_path)
}

/// Pair variant of `emit_module_file` — emits both `.rb` and `.rbs`.
pub(super) fn emit_module_file_pair(
    funcs: &[crate::dialect::LibraryFunction],
    app: &App,
    out_path: PathBuf,
) -> Vec<EmittedFile> {
    if funcs.is_empty() {
        return vec![EmittedFile { path: out_path, content: String::new() }];
    }
    let lc = synthesize_module_lc(funcs);
    let rb = emit_library_class_decl(&lc, app, out_path.clone());
    let rbs = super::rbs::emit_library_class_rbs(&lc, &out_path);
    vec![rb, rbs]
}

/// Emit only the `.rbs` sidecar for a `LibraryClass`. Used when the
/// `.rb` emit has bespoke post-processing the pair helpers can't
/// model (e.g. test files with autorun shim + preamble).
pub(super) fn emit_rbs_sidecar(lc: &LibraryClass, rb_path: &std::path::Path) -> EmittedFile {
    super::rbs::emit_library_class_rbs(lc, rb_path)
}

/// Emit only the `.rbs` sidecar derived from a `LibraryFunction` group.
/// Companion to `emit_rbs_sidecar` for module-shaped output whose `.rb`
/// emit flows through a bespoke path (e.g. `config/routes.rb`).
pub(super) fn emit_rbs_sidecar_from_funcs(
    funcs: &[crate::dialect::LibraryFunction],
    rb_path: &std::path::Path,
) -> EmittedFile {
    let lc = synthesize_module_lc(funcs);
    super::rbs::emit_library_class_rbs(&lc, rb_path)
}

fn synthesize_module_lc(
    funcs: &[crate::dialect::LibraryFunction],
) -> LibraryClass {
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver};
    use crate::ident::Symbol;

    let module_id = funcs
        .first()
        .map(|f| {
            ClassId(Symbol::from(
                f.module_path
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("::"),
            ))
        })
        .unwrap_or_else(|| ClassId(Symbol::from("")));
    let methods: Vec<MethodDef> = funcs
        .iter()
        .map(|f| MethodDef {
            name: f.name.clone(),
            receiver: MethodReceiver::Class,
            params: f.params.clone(),
            body: f.body.clone(),
            signature: f.signature.clone(),
            effects: f.effects.clone(),
            enclosing_class: Some(module_id.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        })
        .collect();
    LibraryClass {
        name: module_id,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        unknown_calls: Vec::new(),
    }
}

/// Emit a single library-shape file. `out_path` is the project-root-relative
/// destination for the file; the require resolver computes paths relative to
/// `out_path`'s parent, so files emitted to `app/views/<plural>/` get
/// `../../../runtime/<x>` while files in `app/models/` get `../../runtime/<x>`.
pub(super) fn emit_library_class_decl(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
) -> EmittedFile {
    emit_library_class_decl_with_synthesized(lc, app, out_path, &[])
}

/// Variant that also accepts a list of (class_name, anchor) pairs for
/// synthesized siblings (e.g. `<Model>Row`, `<Resource>Params`) that
/// aren't in `app.library_classes` / `app.models`. Synthesized classes
/// have no separate require chain — nothing else loads them — so a
/// file that references one needs an explicit `require_relative`,
/// even when the target is in the same directory. Callers that don't
/// emit synthesized siblings pass an empty slice.
pub(super) fn emit_library_class_decl_with_synthesized(
    lc: &LibraryClass,
    app: &App,
    out_path: PathBuf,
    synthesized_siblings: &[(String, String)],
) -> EmittedFile {
    let name = lc.name.0.as_str();
    let out_dir = out_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(PathBuf::new);
    let self_anchor = out_path.with_extension("").to_string_lossy().into_owned();
    let mut s = String::new();

    // A constant whose initializer runs this class's own methods at load
    // time (see `partition_deferred_constants`) drags those method
    // bodies' constant refs into LOAD time with it — `Sound::BUILTIN`
    // calls `initialize`, which reads `Sound::Image`. Those refs would
    // otherwise be classified body-only and left to the aggregator.
    let (eager, deferred) = partition_deferred_constants(lc);
    let load_time_bodies = !deferred.is_empty();

    // Parent + body-derived `require_relative` headers. Helpers return
    // project-root-anchored paths; we relpath each one against `out_dir`
    // so emit works correctly from any output directory.
    let mut requires: Vec<String> = Vec::new();
    if let Some(parent) = lc.parent.as_ref() {
        if let Some(anchor) = require_path_for_parent(parent, app) {
            if anchor != self_anchor {
                requires.push(relpath(&out_dir, &anchor));
            }
        }
    }
    // A namespaced file reopens its outer segments (`class Account`
    // around `module Joinable`), and the header names their superclass
    // to survive either load order — so this file needs THAT class's
    // parent loaded too, exactly as if it were its own.
    let outer_segments: Vec<&str> = name.split("::").collect();
    for i in 0..outer_segments.len().saturating_sub(1) {
        let qualified = outer_segments[..=i].join("::");
        let Some(outer_parent) = outer_class_parent(&qualified, app) else { continue };
        if let Some(anchor) = require_path_for_parent(&outer_parent, app) {
            if anchor != self_anchor {
                requires.push(relpath(&out_dir, &anchor));
            }
        }
    }
    // A reopen of a runtime framework class (lobsters' `module
    // ActiveRecord; class Base; def q ...`) must load the runtime's
    // definition first — under plain Ruby a bare reopen would otherwise
    // DEFINE an empty class, and under spinel the reopen's method
    // bodies reference runtime members. Parent-less and named into a
    // runtime namespace is the reopen signature.
    if lc.parent.is_none() {
        if let Some(anchor) = runtime_reopen_anchor(name) {
            requires.push(relpath(&out_dir, anchor));
        }
    }
    // `include`d modules must be LOADED before the `include` executes at
    // class-definition time — unlike body const-refs (request-time), so we
    // require them even when they're same-dir siblings (plain Ruby has no
    // Rails autoload). Resolve through the same model/library_class anchor.
    for inc in &lc.includes {
        // SPLIT on `::` — an include's ClassId is one Symbol holding the
        // whole path, and the const resolver keys on the ROOT segment.
        // Unsplit, `Turbo::Streams::StreamName::ClassMethods` was its
        // own root, matched nothing, and campfire's guarded channel
        // included a module with no require to load it.
        let path: Vec<String> = inc.0.as_str().split("::").map(str::to_string).collect();
        if let Some(anchor) = require_path_for_body_const(&path, app, name) {
            if anchor != self_anchor {
                requires.push(relpath(&out_dir, &anchor));
            }
        }
    }
    // Require-edge classification: LOAD-time refs vs body-only refs.
    //
    // Class-body constant initializers execute while the file is being
    // required (lobsters' markdowner.rb interpolates
    // `User::VALID_USERNAME` into a class-body regexp constant), so
    // their targets need explicit requires — same footing as the
    // parent and `include`s above.
    //
    // Method-body refs resolve at request time, after boot completes.
    // Their `app/models/*` targets load through the `app/models.rb`
    // aggregator (required from main.rb / test_helper.rb before any
    // dispatch — see `apply_models_aggregator`), NOT through per-file
    // requires. Emitting requires for them is what broke the lobsters
    // boot: user.rb's body-only edge to markdowner.rb closed a cycle
    // against markdowner's genuine load-time need on user.rb, and
    // `require_relative`'s mid-load short-circuit left `User` undefined
    // where the class body read it. Non-aggregated anchors (runtime/*,
    // app/views, app/controllers/*, test/fixtures/*) keep their
    // requires — nothing else loads those.
    let mut load_const_paths: BTreeSet<Vec<String>> = BTreeSet::new();
    for (_, value) in &lc.constants {
        walk_const_paths(value, &mut load_const_paths);
    }
    let mut body_const_paths: BTreeSet<Vec<String>> = BTreeSet::new();
    for m in &lc.methods {
        walk_const_paths(&m.body, &mut body_const_paths);
    }
    let mut body_requires: BTreeSet<String> = BTreeSet::new();
    for path in load_const_paths.iter().chain(&body_const_paths) {
        let load_time = load_const_paths.contains(path);
        let first = match path.first() {
            Some(s) => s,
            None => continue,
        };
        // Synthesized siblings (`<Model>Row`, `<Resource>Params`,
        // `<Plural>Fixtures`) match by exact first-segment name; deeper
        // paths (`X::Y`) don't match here since synthesized classes are
        // flat. Their anchors flow through the same aggregator gate
        // below (model-dir synthesized classes are in `app/models.rb`;
        // fixture siblings keep explicit requires).
        let anchor = synthesized_siblings
            .iter()
            .find(|(n, _)| n == first)
            .map(|(_, a)| a.clone())
            .or_else(|| require_path_for_body_const(path, app, name));
        let Some(anchor) = anchor else { continue };
        if !load_time && !load_time_bodies && anchor.starts_with("app/models/") {
            continue;
        }
        if anchor != self_anchor {
            body_requires.insert(relpath(&out_dir, &anchor));
        }
    }
    requires.extend(body_requires);
    for r in &requires {
        writeln!(s, "require_relative {r:?}").unwrap();
    }
    if !requires.is_empty() {
        writeln!(s).unwrap();
    }

    // Compound names like `Views::Articles` emit as nested
    // `module Views\n  module Articles` rather than `module Views::Articles`.
    // Compound-form headers blow up at load time when the outer namespace
    // isn't already defined (Ruby looks up `Views` as a constant); nested
    // headers create the chain on the fly. Spinel-blog's hand-written
    // views use the nested form for the same reason.
    let mut segments: Vec<&str> = name.split("::").collect();
    // …UNLESS an outer segment belongs to the shared RUNTIME, where we
    // cannot know whether to reopen it as `class` or `module` and the
    // wrong guess is a TypeError at load. campfire extends Action Text
    // with `class ActionText::Attachment::OpengraphEmbed`;
    // `ActionText::Attachment` is a CLASS in runtime/action_text.rb, and
    // nesting emitted `module Attachment` around it.
    //
    // The COMPOUND header the source itself wrote needs no such
    // knowledge — it only requires the outer constants to already
    // exist, which is exactly what the runtime being required before
    // app code guarantees. That is also why this is not the default:
    // `Views::Articles` has no owner but the nesting itself, and a
    // compound header there would look up a `Views` nothing created.
    if segments.len() > 1 {
        let runtime_owned = require_path_for_body_const(
            &[segments[0].to_string()],
            app,
            "",
        )
        .is_some_and(|p| p.starts_with("runtime/"));
        if runtime_owned {
            segments = vec![name];
        }
    }
    let segments = segments;
    let depth = segments.len();
    let body_pad = "  ".repeat(depth);

    // An outer segment that names a known app CLASS must reopen as
    // `class`, not `module` — lobsters nests `CandidateId` inside the
    // `ShortId` model, and `module ShortId` after the model file loaded
    // is a TypeError under Ruby. (Aggregator order guarantees the owner
    // file loads first: `x.rb` sorts before `x/y.rb` because '.' < '/'.)
    // …and it must reopen with the SAME superclass, because the load
    // order the aggregator guarantees is not the only one: `account.rb`
    // requires `account/joinable` at its top so the `include` resolves,
    // which runs the nested file FIRST. A bare `class Account` there
    // creates it under Object, and the real `class Account <
    // ApplicationRecord` then dies with "superclass mismatch". Naming
    // the parent in both places makes the order irrelevant.
    //
    // Resolution is by QUALIFIED prefix, not bare segment:
    // `Opengraph::Fetch::RedirectDeniedError` nests inside the CLASS
    // `Opengraph::Fetch`, and looking up a bare `Fetch` finds nothing
    // ("Fetch is not a module" at load).
    let outer_header = |i: usize, seg: &str| {
        let qualified = segments[..=i].join("::");
        let parent = outer_class_parent(&qualified, app);
        match (is_app_class(&qualified, app), parent) {
            (true, Some(p)) => format!("class {seg} < {}", p.0.as_str()),
            (true, None) => format!("class {seg}"),
            (false, _) => format!("module {seg}"),
        }
    };

    if lc.is_module {
        // Modules don't take a parent; ingest already enforces this.
        for (i, seg) in segments.iter().enumerate() {
            let header = if i < depth - 1 {
                outer_header(i, seg)
            } else {
                format!("module {seg}")
            };
            writeln!(s, "{}{header}", "  ".repeat(i)).unwrap();
        }
    } else {
        // Outer segments (if any) are namespace modules; the last is the class.
        for (i, seg) in segments.iter().take(depth - 1).enumerate() {
            writeln!(s, "{}{}", "  ".repeat(i), outer_header(i, seg)).unwrap();
        }
        let last = segments[depth - 1];
        let pad = "  ".repeat(depth - 1);
        match lc.parent.as_ref() {
            Some(p) => writeln!(s, "{pad}class {last} < {}", p.0.as_str()).unwrap(),
            None => writeln!(s, "{pad}class {last}").unwrap(),
        }
    }

    for inc in &lc.includes {
        writeln!(s, "{body_pad}include {}", inc.0.as_str()).unwrap();
    }
    if !lc.includes.is_empty() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    // Class-level constants (`NAME = <expr>`), emitted before methods so
    // refs in method bodies resolve. A multi-line value (proc/array) keeps
    // its continuation lines indented to the class body.
    //
    // EXCEPT one whose initializer CALLS this class — campfire's
    // `Sound::BUILTIN = [ new(name: "56k", …), … ]` builds a table of
    // instances, and a class body runs top to bottom, so `new` there
    // reaches Object#initialize and raises. Those (and any constant
    // reading them) emit after the methods, which is where the source
    // had them. `emit_deferred_constants` renders that tail.
    let render_constants = |s: &mut String, which: &[usize]| {
        for &i in which {
            let (cname, value) = &lc.constants[i];
            let rendered = super::emit_expr(value);
            let mut lines = rendered.lines();
            match lines.next() {
                Some(first_line) => {
                    writeln!(s, "{body_pad}{} = {first_line}", cname.as_str()).unwrap();
                    for line in lines {
                        if line.is_empty() {
                            writeln!(s).unwrap();
                        } else {
                            writeln!(s, "{body_pad}{line}").unwrap();
                        }
                    }
                }
                None => writeln!(s, "{body_pad}{} = nil", cname.as_str()).unwrap(),
            }
        }
    };
    render_constants(&mut s, &eager);
    if !eager.is_empty() && !lc.methods.is_empty() {
        writeln!(s).unwrap();
    }

    // Class-body calls the ingest didn't model (`LibraryClass::
    // unknown_calls`), replayed verbatim when — and only when — the
    // class extends a base we don't model at all. See
    // `replays_foreign_class_body`. After the constants (a captured
    // call may reference one) and before the methods.
    if replays_foreign_class_body(lc, app) {
        for call in &lc.unknown_calls {
            for line in super::emit_expr(call).lines() {
                if line.is_empty() {
                    writeln!(s).unwrap();
                } else {
                    writeln!(s, "{body_pad}{line}").unwrap();
                }
            }
        }
        if !lc.unknown_calls.is_empty() && !lc.methods.is_empty() {
            writeln!(s).unwrap();
        }
    } else {
        for call in &lc.unknown_calls {
            report_dropped_class_body_call(lc, call);
        }
    }

    let mut first = true;
    for m in &lc.methods {
        if !first {
            writeln!(s).unwrap();
        }
        first = false;
        let body = super::emit_method(m);
        for line in body.lines() {
            if line.is_empty() {
                writeln!(s).unwrap();
            } else {
                writeln!(s, "{body_pad}{line}").unwrap();
            }
        }
    }

    if !deferred.is_empty() {
        if !lc.methods.is_empty() {
            writeln!(s).unwrap();
        }
        render_constants(&mut s, &deferred);
    }

    for i in (0..depth).rev() {
        writeln!(s, "{}end", "  ".repeat(i)).unwrap();
    }

    EmittedFile { path: out_path, content: s }
}

/// Split a class's constants into the ones that can be initialized
/// before its methods exist and the ones that can't. A constant defers
/// when its initializer dispatches to this class — a bare `new(…)` or a
/// receiverless call naming one of its own methods, or either of those
/// SPELLED WITH THE CLASS NAME (`Sound.new(…)`) — or when it reads a
/// constant that already deferred. A self-dispatch inside a STORED
/// closure doesn't count: that body runs on call, not on load. Returns
/// index lists so both groups keep source order.
///
/// The class-name spelling is not hypothetical: `lower::class_body_new`
/// gives a class body's bare `new` its receiver (a receiverless call is
/// unresolvable on a strict target), and a rule that keyed on the bare
/// spelling alone stopped seeing campfire's `Sound::BUILTIN` the moment
/// it did. That emitted fifty-six constructor calls ABOVE the
/// `initialize` they call — `BasicObject#initialize: wrong number of
/// arguments (given 1, expected 0)` at load, on every one of the
/// suite's 52 files. The fact the rule is about is "does this
/// initializer dispatch to this class", and both spellings are that
/// fact.
fn partition_deferred_constants(lc: &LibraryClass) -> (Vec<usize>, Vec<usize>) {
    fn is_own_class(recv: &Option<Expr>, class_name: &str) -> bool {
        let Some(r) = recv else { return false };
        let ExprNode::Const { path } = &*r.node else { return false };
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::") == class_name
    }
    fn calls_self(expr: &Expr, own: &std::collections::HashSet<&str>, deferred_names: &std::collections::HashSet<String>, class_name: &str) -> bool {
        match &*expr.node {
            // A closure that is STORED rather than run — `proc { … }`,
            // `lambda { … }`, `Proc.new { … }`, `->() { … }` — executes
            // its body when something CALLS it, not when the constant
            // initializes. lobsters' `CACHE_PAGE = proc { …
            // clear_session_cookie? }` otherwise reads as a self-dispatch,
            // and deferring it marks the whole class load-time-bodies,
            // which pulls every method-body const ref into the require
            // header: that closed a cycle (application_controller →
            // user → avatars_controller → application_controller) and
            // left the emitted tree unable to load at all.
            //
            // Blocks that DO run now (`map`, `each`) are descended into
            // at the Send arm below, so a Lambda reaching this arm is a
            // stored literal.
            ExprNode::Lambda { .. } => false,
            ExprNode::Send { recv, method, args, block, .. } => {
                if (recv.is_none() || is_own_class(recv, class_name))
                    && (method.as_str() == "new" || own.contains(method.as_str()))
                {
                    return true;
                }
                let stores_block = matches!(method.as_str(), "proc" | "lambda")
                    || (method.as_str() == "new"
                        && matches!(
                            recv.as_ref().map(|r| &*r.node),
                            Some(ExprNode::Const { path })
                                if path.len() == 1 && path[0].as_str() == "Proc"
                        ));
                recv.iter()
                    .chain(args.iter())
                    .any(|e| calls_self(e, own, deferred_names, class_name))
                    || (!stores_block
                        && block.as_ref().is_some_and(|b| match &*b.node {
                            ExprNode::Lambda { body, .. } => {
                                calls_self(body, own, deferred_names, class_name)
                            }
                            _ => calls_self(b, own, deferred_names, class_name),
                        }))
            }
            ExprNode::Const { path }
                if path.len() == 1 && deferred_names.contains(path[0].as_str()) =>
            {
                true
            }
            _ => {
                let mut found = false;
                expr.node.for_each_child(&mut |child| {
                    if !found && calls_self(child, own, deferred_names, class_name) {
                        found = true;
                    }
                });
                found
            }
        }
    }

    let own: std::collections::HashSet<&str> =
        lc.methods.iter().map(|m| m.name.as_str()).collect();
    let mut deferred_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let (mut eager, mut deferred) = (Vec::new(), Vec::new());
    for (i, (name, value)) in lc.constants.iter().enumerate() {
        if calls_self(value, &own, &deferred_names, lc.name.0.as_str()) {
            deferred_names.insert(name.as_str().to_string());
            deferred.push(i);
        } else {
            eager.push(i);
        }
    }
    (eager, deferred)
}

/// Project-root-anchored require target for a parent class, if one is needed.
/// `ActiveRecord::Base` lives in the runtime; same-dir parents
/// (ApplicationRecord, custom abstract bases) resolve to a sibling under
/// `app/models/`. Everything else returns `None` (assume the loader sees
/// the parent some other way).
/// The runtime stem a framework-class REOPEN must load first, if the
/// class lives in a runtime namespace this tree ships. App classes with
/// `::` names outside these namespaces (ShortId::CandidateId) get None.
fn runtime_reopen_anchor(name: &str) -> Option<&'static str> {
    for (prefix, anchor) in [
        ("ActiveRecord", "runtime/active_record"),
        ("ActionController", "runtime/action_controller"),
        ("ActionView", "runtime/action_view"),
        ("ActionDispatch", "runtime/action_dispatch"),
        ("ActionMailer", "runtime/action_mailer"),
        ("ActiveJob", "runtime/active_job"),
    ] {
        if name == prefix || name.strip_prefix(prefix).is_some_and(|r| r.starts_with("::")) {
            return Some(anchor);
        }
    }
    None
}

/// Whether this class's captured `unknown_calls` should be replayed
/// into the emitted class body.
///
/// The test is "does the class extend a base we model at all?".
/// `require_path_for_parent` already answers that — it resolves every
/// framework base (`ActiveJob::Base`, `ActionMailer::Base`, …) and
/// every app-local parent to a require target, and returns `None` only
/// for a superclass that is neither. `SearchParser < Parslet::Parser`
/// lands in that `None` case: the base comes from a gem, the gem is in
/// the app's bundle, and its class-body DSL is the class. Replaying is
/// the only way to emit it, and it is safe because the DSL's receiver
/// is the gem's own class.
///
/// Everything else deliberately stays dropped, because a call we don't
/// recognize on a base we DO model is already the lowering's business
/// and replaying it would be wrong twice over:
///
///   - a concern's `included do … end` is spliced into each includer by
///     `splice_concerns_into_models`; replaying it here would apply the
///     callbacks a second time, on a module the runtime gives no
///     `included` hook to,
///   - `queue_as` / `rescue_from` / `default from:` / `delegate` are
///     framework macros whose receiver is our own runtime base class.
///     Where the runtime doesn't define one, replaying turns a silent
///     modelling gap into a NoMethodError at class-definition time,
///     i.e. an app that won't boot.
///
/// A dropped call is no longer silent either way — see
/// `report_dropped_class_body_call`.
fn replays_foreign_class_body(lc: &LibraryClass, app: &App) -> bool {
    if lc.is_module {
        return false;
    }
    lc.parent.as_ref().is_some_and(|p| {
        require_path_for_parent(p, app).is_none() && !is_core_class_name(p.0.as_str())
    })
}

/// Ledger a class-body call we captured but chose not to replay. The
/// emit diagnostic sink is live here (ingest's is not — it runs before
/// any `diagnostics::scope`), which is why the judgment happens at emit
/// rather than where the call was captured.
fn report_dropped_class_body_call(lc: &LibraryClass, call: &Expr) {
    let name = match &*call.node {
        ExprNode::Send { method, .. } => method.as_str().to_string(),
        _ => "call".to_string(),
    };
    crate::emit::diagnostics::push(crate::lower::residue_diagnostic(
        "library_class_body",
        &name,
        call.span,
        "not modelled",
        format!(
            "`{}` in the body of `{}` is not modelled and is dropped; only a class \
             extending a base roundhouse does NOT model (a gem's, whose DSL is the \
             class) replays its class body verbatim",
            name,
            lc.name.0.as_str(),
        ),
    ));
}

/// Is this name one of the app's own CLASSES (not a module)? Namespace
/// segments that name one must reopen as `class`, not `module`.
fn is_app_class(name: &str, app: &App) -> bool {
    app.models.iter().any(|m| m.name.0.as_str() == name)
        || app
            .library_classes
            .iter()
            .any(|c| c.name.0.as_str() == name && !c.is_module)
}

/// The declared superclass of an app class named by a namespace
/// segment, so a nested file can reopen it with the same parent.
fn outer_class_parent(name: &str, app: &App) -> Option<ClassId> {
    if let Some(m) = app.models.iter().find(|m| m.name.0.as_str() == name) {
        return m.parent.clone();
    }
    app.library_classes
        .iter()
        .find(|c| c.name.0.as_str() == name && !c.is_module)
        .and_then(|c| c.parent.clone())
}

fn require_path_for_parent(parent: &ClassId, app: &App) -> Option<String> {
    let raw = parent.0.as_str();
    if raw == "ActiveRecord::Base" {
        // When the app REOPENS ActiveRecord::Base (lobsters' `q`
        // monkeypatch, emitted as a library class), route the parent
        // require through the reopen file — it requires the runtime
        // itself first, so subclasses see both the framework methods
        // and the app's additions. Without this the reopen dangles:
        // nothing else in the require graph names it.
        if app
            .library_classes
            .iter()
            .any(|lc| lc.name.0.as_str() == "ActiveRecord::Base")
        {
            return Some("app/models/active_record/base".to_string());
        }
        return Some("runtime/active_record".to_string());
    }
    if raw == "ActionController::Base" || raw == "ActionController::API" {
        return Some("runtime/action_controller".to_string());
    }
    if raw == "ActionMailer::Base" {
        return Some("runtime/action_mailer".to_string());
    }
    if raw == "ActiveJob::Base" {
        return Some("runtime/active_job".to_string());
    }
    // `ApplicationCable::Channel < ActionCable::Channel::Base` and its
    // Connection twin — the two roots of an ingested `app/channels/`
    // tree. Both are a SUPERCLASS reference, which is load-time, so a
    // missing require is not a lazy failure: the file does not define.
    if raw == "ActionCable::Channel::Base" || raw == "ActionCable::Connection::Base" {
        return Some("runtime/action_cable".to_string());
    }
    if app.models.iter().any(|m| m.name.0.as_str() == raw)
        || app.library_classes.iter().any(|lc| lc.name.0.as_str() == raw)
    {
        // underscore: namespaced parents nest (see emit_library_class_decls).
        return Some(format!("app/models/{}", crate::naming::underscore(raw)));
    }
    if app.controllers.iter().any(|c| c.name.0.as_str() == raw) {
        return Some(format!("app/controllers/{}", crate::naming::underscore(raw)));
    }
    // A SUPERCLASS that is a gem's class — lobsters' `TimeSeries <
    // SVG::Graph::TimeSeries`, campfire's `ApplicationPlatform <
    // PlatformAgent`. Both used to emit no require at all and load only
    // because boot.rb happened to have pulled the gem in first; a body
    // reached from the test harness, which builds its own require
    // chain, saw an undefined superclass and the file did not define.
    if let Some(root) = raw.split("::").next() {
        if is_gem_facade_root(root) {
            return Some("runtime/gem_facades".to_string());
        }
    }
    None
}

/// Roots hosted by `runtime/ruby/gem_facades.rb` — the one file that
/// stands in for every stubbed gem, so all their roots anchor there.
/// On a ruby-family tree `project.rs` rewrites that file into the
/// guarded-require block, and the same anchor loads the real gems.
fn is_gem_facade_root(root: &str) -> bool {
    matches!(
        root,
        "Markly"
            | "Nokogiri"
            | "Mail"
            | "ROTP"
            | "BCrypt"
            | "RQRCode"
            | "SVG"
            // campfire's `ApplicationPlatform < PlatformAgent` names
            // its superclass at LOAD time, so a missing anchor here is
            // not a lazy failure: the tree does not boot.
            | "PlatformAgent"
            | "UserAgent"
    )
}

/// Core Ruby classes an app may reopen (monkeypatch) without them being
/// `app/models` files. Kept in sync with `rbs::is_builtin_class_name`.
fn is_core_class_name(name: &str) -> bool {
    matches!(
        name,
        "Integer"
            | "Float"
            | "String"
            | "Symbol"
            | "TrueClass"
            | "FalseClass"
            | "NilClass"
            | "Array"
            | "Hash"
            | "Object"
            | "Numeric"
            | "Comparable"
            | "Enumerable"
            | "Kernel"
    )
}

/// Project-root-anchored require target for a body-referenced constant.
/// `Views::<Plural>` resolves to `app/views/<plural>/_<singular>`; runtime
/// modules resolve to `runtime/<x>`. The caller relpaths the result against
/// the requirer's `out_dir`, so a single mapping serves every output kind.
/// Same-dir siblings (other models, library_classes) drop because Ruby's
/// load path covers them; unknowns drop silently.
fn require_path_for_body_const(
    path: &[String],
    app: &App,
    self_name: &str,
) -> Option<String> {
    // A ROOTED head (`::Broadcasts`, `::Sound::Image`) names the same
    // file as the bare one — the marker says where Ruby should look, not
    // what to load. Strip it before resolving, or rooting a reference
    // silently deletes its require and the constant is undefined at load
    // for a different reason than the one rooting fixed.
    let rooted;
    let path: &[String] = match path.first().and_then(|f| f.strip_prefix("::")) {
        Some(bare) => {
            rooted = std::iter::once(bare.to_string())
                .chain(path.iter().skip(1).cloned())
                .collect::<Vec<_>>();
            &rooted
        }
        None => path,
    };
    let first = path.first()?;
    // A nested class of THIS class is still a different file:
    // `Sound::Image` lives at app/models/sound/image.rb even though the
    // reference's first segment is `Sound`. Resolve the full path before
    // the self-reference short-circuit below.
    let joined = path.join("::");
    if joined != self_name
        && (app.models.iter().any(|m| m.name.0.as_str() == joined)
            || app.library_classes.iter().any(|lc| lc.name.0.as_str() == joined))
    {
        return Some(format!("app/models/{}", crate::naming::underscore(&joined)));
    }
    // A bare reference inside a namespace resolves LEXICALLY first:
    // campfire's `module ContentFilters` builds
    // `TextMessagePresentationFilters = …new(RemoveSoloUnfurledLinkText,
    // …)` at LOAD time, and that name is
    // `ContentFilters::RemoveSoloUnfurledLinkText` — a sibling file that
    // the aggregator requires AFTER this one (`x.rb` sorts before
    // `x/y.rb`). Without the require the constant is uninitialized and
    // the whole app fails to load, so resolve the qualified form before
    // giving up on the bare one.
    let lexical = format!("{self_name}::{joined}");
    if !self_name.is_empty()
        && (app.models.iter().any(|m| m.name.0.as_str() == lexical)
            || app.library_classes.iter().any(|lc| lc.name.0.as_str() == lexical))
    {
        return Some(format!("app/models/{}", crate::naming::underscore(&lexical)));
    }
    if first == self_name {
        return None;
    }
    // A core Ruby class an app reopens (e.g. `class String` in
    // lib/monkey.rb) lands in `library_classes`, but it is NOT an
    // `app/models/<name>` file — every `String.new` would otherwise emit
    // a dangling `require_relative "app/models/string"`. The reference
    // resolves to the builtin; any monkeypatch is loaded via its own file
    // (lib/), not through this model anchor.
    if is_core_class_name(first) {
        return None;
    }
    if app.models.iter().any(|m| m.name.0.as_str() == first.as_str())
        || app
            .library_classes
            .iter()
            .any(|lc| lc.name.0.as_str() == first.as_str())
    {
        return Some(format!("app/models/{}", crate::naming::underscore(first)));
    }
    if app.controllers.iter().any(|c| c.name.0.as_str() == first.as_str()) {
        return Some(format!("app/controllers/{}", crate::naming::underscore(first)));
    }
    // Qualified runtime names, resolved on the FULL path — `ActiveSupport`
    // is a namespace we ship several unrelated pieces of, so anchoring on
    // its root segment would over-require. A fixture loader is the first
    // body to need this: `created_at: <%= 1.hour.ago %>` grounds to
    // `ActiveSupport::Duration.hour(1)` and `test/fixtures/<x>.rb` is
    // reached from the test harness, not from main.rb's require chain.
    if joined == "ActiveSupport::Duration" {
        return Some("runtime/active_support_duration".to_string());
    }
    match first.as_str() {
        // `Views::*` refs always go through the per-app aggregator at
        // `app/views.rb` (spinel-blog convention; loads all view
        // modules so any `Views::X.method` resolves regardless of
        // which template the method lives in). Per-template requires
        // would be wrong because the same `Views::X` const can host
        // methods from multiple files (`_article.rb`, `index.rb`,
        // `show.rb` all re-open `module Views::Articles`).
        "Views" => Some("app/views".to_string()),
        // Runtime modules under `runtime/`. ViewHelpers still ships
        // hand-written; RouteHelpers is now generated into
        // `app/route_helpers.rb` from `app.routes` so consumers
        // resolve there. Add entries as lowerings introduce new ones;
        // unknown idents silently drop.
        "Broadcasts" => Some("runtime/broadcasts".to_string()),
        // `ActionCable.server.broadcast(stream, payload)` — the
        // low-level publish API, and the two bases `app/channels/*.rb`
        // subclasses. Sibling of Broadcasts, not a replacement: the
        // Turbo Stream family still goes through `Broadcasts.append`.
        "ActionCable" => Some("runtime/action_cable".to_string()),
        // `ActionText::Content` / `ActionText::Attachment` — the value
        // half of Action Text. `ActionText::RichText` is NOT here: it
        // is a lowered model and resolves under `app/models/` like any
        // other, which is exactly the split this entry preserves.
        "ActionText" => Some("runtime/action_text".to_string()),
        // `ActiveStorage::Attached` — the value half, what a
        // `has_one_attached` reader returns. `ActiveStorage::Attachment`
        // is NOT here for the same reason `ActionText::RichText` is
        // not: it is a lowered model under `app/models/`.
        "ActiveStorage" => Some("runtime/active_storage".to_string()),
        // `Turbo::Streams::StreamName::ClassMethods`, INCLUDED by a
        // channel that guards its own stream (campfire's
        // `RoomMessagesChannel`). An include runs at class-definition
        // time, so a missing anchor here is a tree that does not boot.
        "Turbo" => Some("runtime/turbo_streams".to_string()),
        "Inflector" => Some("runtime/inflector".to_string()),
        // `IPAddr` — ported into `runtime/ruby/ipaddr.rb` because the
        // strict targets have no stdlib to reach for. Anchored for
        // every target, ruby family included: `ipaddr` is not in
        // `project::BUNDLED`, so nothing inserts a bare
        // `require "ipaddr"` that would reopen this class with the
        // stdlib's own `initialize` and leave the predicates reading a
        // slot nobody filled. One IPAddr per tree, and it is ours.
        "IPAddr" => Some("runtime/ipaddr".to_string()),
        // `Mime::Type` — actionpack's registry, ported into
        // `runtime/ruby/mime.rb`. Anchored for every target: the
        // constant is actionpack's, not the stdlib's, so no bare
        // `require` reaches it on the ruby family either.
        "Mime" => Some("runtime/mime".to_string()),
        "ViewHelpers" => Some("runtime/action_view".to_string()),
        "RouteHelpers" => Some("app/route_helpers".to_string()),
        // Gem façades — typed raising stand-ins for write-path-only
        // gem surface (see runtime/ruby/gem_facades.rb). One file
        // hosts every stubbed gem, so all their roots anchor here.
        _ if is_gem_facade_root(first.as_str()) => Some("runtime/gem_facades".to_string()),
        _ => None,
    }
}

/// Compute a `require_relative`-style relative path from `from_dir` to
/// the project-root-anchored `to_anchor`. Both inputs are slash-separated;
/// the result has no `.rb` extension because `require_relative` doesn't
/// need one.
fn relpath(from_dir: &Path, to_anchor: &str) -> String {
    let from_parts: Vec<&str> = from_dir
        .to_str()
        .unwrap_or("")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let to_parts: Vec<&str> = to_anchor.split('/').filter(|s| !s.is_empty()).collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let ups = from_parts.len() - common;
    let mut parts: Vec<&str> = std::iter::repeat("..").take(ups).collect();
    parts.extend(&to_parts[common..]);
    parts.join("/")
}

pub(super) fn walk_const_paths(e: &Expr, out: &mut BTreeSet<Vec<String>>) {
    match &*e.node {
        ExprNode::Const { path } => {
            out.insert(path.iter().map(|s| s.as_str().to_string()).collect());
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk_const_paths(r, out);
            }
            for a in args {
                walk_const_paths(a, out);
            }
            if let Some(b) = block {
                walk_const_paths(b, out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            walk_const_paths(fun, out);
            for a in args {
                walk_const_paths(a, out);
            }
            if let Some(b) = block {
                walk_const_paths(b, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk_const_paths(k, out);
                walk_const_paths(v, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk_const_paths(el, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk_const_paths(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk_const_paths(left, out);
            walk_const_paths(right, out);
        }
        ExprNode::Let { value, body, .. } => {
            walk_const_paths(value, out);
            walk_const_paths(body, out);
        }
        ExprNode::Lambda { body, .. } => walk_const_paths(body, out),
        ExprNode::If { cond, then_branch, else_branch } => {
            walk_const_paths(cond, out);
            walk_const_paths(then_branch, out);
            walk_const_paths(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            walk_const_paths(scrutinee, out);
            for arm in arms {
                walk_const_paths(&arm.body, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                walk_const_paths(e, out);
            }
        }
        ExprNode::Assign { value, .. } => walk_const_paths(value, out),
        ExprNode::Yield { args } => {
            for a in args {
                walk_const_paths(a, out);
            }
        }
        ExprNode::Raise { value } => walk_const_paths(value, out),
        ExprNode::RescueModifier { expr, fallback } => {
            walk_const_paths(expr, out);
            walk_const_paths(fallback, out);
        }
        ExprNode::Return { value } => walk_const_paths(value, out),
        ExprNode::Super { args: Some(args) } => {
            for a in args {
                walk_const_paths(a, out);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            walk_const_paths(body, out);
            for r in rescues {
                walk_const_paths(&r.body, out);
            }
            if let Some(e) = else_branch {
                walk_const_paths(e, out);
            }
            if let Some(e) = ensure {
                walk_const_paths(e, out);
            }
        }
        ExprNode::Next { value: Some(v) } => walk_const_paths(v, out),
        ExprNode::MultiAssign { value, .. } => walk_const_paths(value, out),
        ExprNode::While { cond, body, .. } => {
            walk_const_paths(cond, out);
            walk_const_paths(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                walk_const_paths(b, out);
            }
            if let Some(e) = end {
                walk_const_paths(e, out);
            }
        }
        // Leaves and uninteresting nodes pass through.
        _ => {}
    }
}

// has_secure_password synthesis moved to the shared model lowering
// (`lower::secure_password::push_secure_password_methods`) — every
// target's model classes now carry authenticate + the plaintext
// accessors, in the bcrypt gem's own contract shape
// (`BCrypt::Password.create/new`): the CRuby/JRuby trees load the
// real gem (guarded require in the overlay main.rb), and a future
// spinel-bcrypt spin package satisfies the same calls.

fn sp_expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn ivar_read(name: &Symbol) -> Expr {
    sp_expr(ExprNode::Ivar { name: name.clone() })
}

// Request-params key normalization moved to the Ruby expr emitter's
// type-directed index hook (`emit::ruby::expr`): a symbol/dynamic key on
// a statically string-keyed hash (`Hash[String, _]`) is coerced to a
// string at the single `[]` emit chokepoint. That gates on the receiver
// *type* rather than a `params` name heuristic, so it covers views and
// helpers (params flows there too) and never touches a genuine
// symbol-keyed `Hash[Symbol, _]` like `StoryRepository#@params`.

// typed_store accessor synthesis moved to the shared model lowering
// (`lower::typed_store::push_typed_store_methods`) — every target's
// model classes now carry the reader/predicate/writer methods; the
// `TypedStore` runtime module (YAML seam) still ships only on the
// CRuby/JRuby overlay trees.

// ── boolean-column cast lowering ─────────────────────────────────────

/// Ruby-family pre-emit pass: boolean-column readers and `<col>?`
/// predicates cast the stored value instead of returning it raw. The
/// CRuby sqlite adapter hydrates boolean columns as the Integers
/// SQLite stores (0/1) — and `0` is TRUTHY in Ruby, so a plain `@col`
/// read makes every `user.is_admin?` guard pass for non-admins.
/// Rewritten body: `@col == true || @col == 1` (handles both a
/// DB-hydrated Integer and an app-assigned true/false; nil/0/false →
/// false). Strict targets hydrate native booleans and keep the shared
/// synthesized shape. Only plain `@col`-read bodies are rewritten
/// (idempotent; custom bodies win).
pub(crate) fn apply_boolean_lowering(lcs: &mut [LibraryClass], app: &App) {
    for model in &app.models {
        let Some(table) = app.schema.tables.get(&model.table.0) else {
            continue;
        };
        let bool_cols: BTreeSet<Symbol> = table
            .columns
            .iter()
            .filter(|c| matches!(c.col_type, crate::schema::ColumnType::Boolean))
            .map(|c| c.name.clone())
            .collect();
        if bool_cols.is_empty() {
            continue;
        }
        let Some(lc) = lcs.iter_mut().find(|lc| lc.name == model.name) else {
            continue;
        };
        for m in &mut lc.methods {
            if m.receiver != MethodReceiver::Instance {
                continue;
            }
            let col = Symbol::from(m.name.as_str().trim_end_matches('?'));
            if !bool_cols.contains(&col) {
                continue;
            }
            if is_plain_ivar_read(&m.body, &col) {
                m.body = boolean_cast_body(&col);
            }
        }
    }
}

/// `@col == true || @col == 1`.
fn boolean_cast_body(col: &Symbol) -> Expr {
    let eq = |rhs: Expr| {
        sp_expr(ExprNode::Send {
            recv: Some(ivar_read(col)),
            method: Symbol::from("=="),
            args: vec![rhs],
            block: None,
            parenthesized: false,
        })
    };
    sp_expr(ExprNode::BoolOp {
        op: crate::expr::BoolOpKind::Or,
        surface: Default::default(),
        left: eq(sp_expr(ExprNode::Lit {
            value: crate::expr::Literal::Bool { value: true },
        })),
        right: eq(sp_expr(ExprNode::Lit {
            value: crate::expr::Literal::Int { value: 1 },
        })),
    })
}

// ─── Runtime-Relation eager loading (issue #27 follow-up) ──────────────
//
// The static arel path already lowers `includes(:assoc)` into inline
// preload statements, but chains that reach the runtime
// `ActiveRecord::Relation` (scope chains, association relations) only
// RECORDED their `includes(...)` specs — `to_a` never executed them, so
// every association read was a lazy per-row query (the lobsters 2x
// query-count gap vs Rails, ~985 excess queries per benchmark pass:
// belongs_to singles ~870, has_many/through ~295).
//
// This pass synthesizes, per model, the statically-dispatched preload
// machinery `Relation#to_a` calls (`@model.preload_associations(records,
// @includes)` — Base supplies a no-op default):
//
//   def self.preload_associations(records, specs)   # spec walker
//   def self._preload_dispatch(records, name, nested)  # case-dispatch
//   def self._preload_batch_<assoc>(records)        # one batched IN load
//   def _preload_<belongs_to>(rec)                  # cache setter
//
// plus a cache guard prepended to each belongs_to reader (mirroring the
// has_many readers' `return @x_cache if @x_loaded` shape; the has_many
// setters/caches already exist from the static-path work).
//
// No method_missing, no send: nested specs (`story: :user`) recurse
// through the case arm's statically-named target class — the same
// case-dispatch shape as dynamic-partial rendering. Bodies are generated
// as Ruby source and parsed back through `runtime_src::parse_methods`
// (templates are fixed; identifiers come from assoc/table names).
//
// Known gaps, deliberate: has_one and scope-carrying through-assocs
// (other than a plain `order("...")`) get no batch arm — the dispatch
// falls through and the lazy reader stays correct (just N+1, matching
// Rails, which also lazy-loads what `includes` doesn't name). Assigning
// a belongs_to (`c.story = s`) on a PRELOADED record does not refresh
// the cache (fresh records never have the loaded flag set, so the
// benchmark's build-then-render flows are unaffected).
pub(crate) fn apply_preload_lowering(lcs: &mut [LibraryClass], app: &App) {
    use crate::dialect::Association;

    // Gate: runtime Relations only arise in scope-chain apps (scope-free
    // apps resolve every chain on the static arel path), and synthesis
    // only pays for itself when some `includes(...)` survives to
    // runtime. real-blog (`includes` but no scopes) and tiny-blog
    // (scopes but no `includes`) both stay byte-identical.
    let scopes = crate::lower::scope_chain::build_scope_registry(&app.models);
    if !crate::lower::scope_chain::any_scopes(&scopes) || !app_mentions_includes(app) {
        return;
    }

    for lc in lcs.iter_mut() {
        let Some(model) = app.models.iter().find(|m| m.name == lc.name) else { continue };

        // belongs_to readers gain the cache guard the has_many readers
        // already carry: `return @user_cache if @user_loaded`.
        for assoc in model.associations() {
            let Association::BelongsTo { name, .. } = assoc else { continue };
            let Some(m) = lc.methods.iter_mut().find(|m| {
                m.name == *name && m.receiver == MethodReceiver::Instance
            }) else {
                continue;
            };
            let old = std::mem::replace(
                &mut m.body,
                Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            );
            m.body = Expr::new(
                Span::synthetic(),
                ExprNode::Seq { exprs: vec![preload_cache_guard(name), old] },
            );
        }

        let src = preload_methods_source(model, app);
        let methods = crate::runtime_src::parse_methods(&src).unwrap_or_else(|e| {
            panic!("apply_preload_lowering: generated source failed to parse: {e}\n{src}")
        });
        for mut m in methods {
            if lc.methods.iter().any(|existing| {
                existing.name == m.name && existing.receiver == m.receiver
            }) {
                continue; // user-defined names win
            }
            m.enclosing_class = Some(lc.name.0.clone());
            lc.methods.push(m);
        }
    }
}

/// `return @<name>_cache if @<name>_loaded` — the same guard shape the
/// has_many readers carry (`through_reader_body`), so preloaded and lazy
/// reads share one cache contract.
fn preload_cache_guard(name: &Symbol) -> Expr {
    let span = Span::synthetic;
    Expr::new(
        span(),
        ExprNode::If {
            cond: Expr::new(
                span(),
                ExprNode::Ivar { name: Symbol::from(format!("{}_loaded", name.as_str())) },
            ),
            then_branch: Expr::new(
                span(),
                ExprNode::Return {
                    value: Expr::new(
                        span(),
                        ExprNode::Ivar {
                            name: Symbol::from(format!("{}_cache", name.as_str())),
                        },
                    ),
                },
            ),
            else_branch: Expr::new(span(), ExprNode::Lit { value: Literal::Nil }),
        },
    )
}

/// True when any raw app body (controller actions, library-class
/// methods, model scopes/methods) sends `includes`/`preload`/
/// `eager_load` to a receiver. Over-approximates (a chain the arel pass
/// later consumes statically still counts) — the cost of a false
/// positive is inert synthesized methods, not wrong behavior.
fn app_mentions_includes(app: &App) -> bool {
    use crate::dialect::{ControllerBodyItem, ModelBodyItem};
    let in_expr = expr_mentions_includes;
    app.controllers.iter().any(|c| {
        c.body.iter().any(|item| match item {
            ControllerBodyItem::Action { action, .. } => in_expr(&action.body),
            _ => false,
        })
    }) || app.library_classes.iter().any(|lc| lc.methods.iter().any(|m| in_expr(&m.body)))
        || app.models.iter().any(|m| {
            m.body.iter().any(|item| match item {
                ModelBodyItem::Scope { scope, .. } => in_expr(&scope.body),
                ModelBodyItem::Method { method, .. } => in_expr(&method.body),
                _ => false,
            })
        })
}

fn expr_mentions_includes(expr: &Expr) -> bool {
    let mut found = false;
    fn walk(e: &Expr, found: &mut bool) {
        if *found {
            return;
        }
        if let ExprNode::Send { recv: Some(_), method, .. } = &*e.node {
            if matches!(method.as_str(), "includes" | "preload" | "eager_load") {
                *found = true;
                return;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    walk(expr, &mut found);
    found
}

/// One preloadable association, resolved against the app's model set.
enum PreloadKind {
    /// (fk column on the owner, target class, target table)
    BelongsTo { fk: String, target: String, table: String },
    /// (fk column on the target, target class)
    HasMany { fk: String, target: String },
    /// Batched form of the through-reader join:
    /// `SELECT <t>.*, <thr>.<thr_fk> AS __src FROM <t> JOIN <thr> ON
    /// <thr>.<src_fk> = <t>.id WHERE <thr>.<thr_fk> IN (...)`.
    Through { target: String, join: String, group_col: String, order: Option<String> },
}

fn preload_targets(model: &crate::dialect::Model, app: &App) -> Vec<(String, PreloadKind)> {
    use crate::dialect::Association;
    use crate::naming::pluralize_snake;

    let model_exists = |id: &ClassId| app.models.iter().any(|m| &m.name == id);
    let mut out = Vec::new();
    for assoc in model.associations() {
        match assoc {
            Association::BelongsTo { name, target, foreign_key, .. } => {
                if !model_exists(target) {
                    continue;
                }
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::BelongsTo {
                        fk: foreign_key.as_str().to_string(),
                        target: target.0.as_str().to_string(),
                        table: pluralize_snake(target.0.as_str()),
                    },
                ));
            }
            Association::HasMany { name, target, foreign_key, through: None, .. } => {
                if !model_exists(target) {
                    continue;
                }
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::HasMany {
                        fk: foreign_key.as_str().to_string(),
                        target: target.0.as_str().to_string(),
                    },
                ));
            }
            // Through: same two-hop resolution as
            // `apply_through_assoc_lowering`; assoc scopes other than a
            // plain `order("...")` (or none) don't batch — the lazy
            // reader keeps them correct.
            Association::HasMany {
                name, target, through: Some(thr_name), scope, ..
            } => {
                if !model_exists(target) {
                    continue;
                }
                let order = match scope {
                    None => None,
                    Some(s) => match order_literal(s) {
                        Some(o) => Some(o),
                        None => continue,
                    },
                };
                let Some(Association::HasMany { target: thr_target, foreign_key: thr_fk, .. }) =
                    model.associations().find(|a| {
                        matches!(a, Association::HasMany { name, .. } if name == thr_name)
                    })
                else {
                    continue;
                };
                let Some(thr_model) = app.models.iter().find(|m| &m.name == thr_target) else {
                    continue;
                };
                let Some(Association::BelongsTo { foreign_key: src_fk, .. }) =
                    thr_model.associations().find(|a| {
                        matches!(a, Association::BelongsTo { target: t, .. } if t == target)
                    })
                else {
                    continue;
                };
                let thr_table = pluralize_snake(thr_target.0.as_str());
                let target_table = pluralize_snake(target.0.as_str());
                out.push((
                    name.as_str().to_string(),
                    PreloadKind::Through {
                        target: target.0.as_str().to_string(),
                        join: format!(
                            "INNER JOIN {thr_table} ON {thr_table}.{src_fk} = {target_table}.id"
                        ),
                        group_col: format!("{thr_table}.{thr_fk}"),
                        order: order.map(|o| o.to_string()),
                    },
                ));
            }
            _ => {}
        }
    }
    out
}

/// Extract the string literal from an assoc-scope lambda body of the
/// exact shape `order("...")` (lobsters `has_many :tags, -> { order
///('tags.is_media desc, tags.tag') }, through: :taggings`).
fn order_literal(scope_body: &Expr) -> Option<&str> {
    let ExprNode::Send { recv: None, method, args, .. } = &*scope_body.node else {
        return None;
    };
    if method.as_str() != "order" || args.len() != 1 {
        return None;
    }
    let ExprNode::Lit { value: Literal::Str { value } } = &*args[0].node else {
        return None;
    };
    Some(value.as_str())
}

/// Generate the per-model preload methods as Ruby source (fed back
/// through `runtime_src::parse_methods`). Templates stay boring on
/// purpose: statement-level assigns and explicit nil-guards round-trip
/// through every walker; no `||=`-on-index, no ternaries.
fn preload_methods_source(model: &crate::dialect::Model, app: &App) -> String {
    let targets = preload_targets(model, app);
    let mut src = String::new();

    // Batch loaders + belongs_to cache setters.
    for (name, kind) in &targets {
        match kind {
            PreloadKind::BelongsTo { fk, target, table } => {
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    v = r.{fk}
    ids << v unless v.nil? || v == 0
  end
  ids.uniq!
  by_id = {{}}
  if ids.length > 0
    ActiveRecord.adapter.select_rows("SELECT {table}.* FROM {table} WHERE {table}.id IN (" + Db.escape_int_list(ids) + ")").each do |row|
      rec = {target}.instantiate(row)
      by_id[rec.id] = rec
    end
  end
  records.each do |r|
    r._preload_{name}(by_id[r.{fk}])
  end
  by_id.values
end

def _preload_{name}(rec)
  @{name}_cache = rec
  @{name}_loaded = true
  nil
end
"#
                );
            }
            PreloadKind::HasMany { fk, target } => {
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    ids << r.id
  end
  loaded = []
  if ids.length > 0
    loaded = ActiveRecord::Relation.new({target}).where({fk}: ids).to_a
  end
  grouped = {{}}
  loaded.each do |rec|
    k = rec.{fk}
    grouped[k] = [] if grouped[k].nil?
    grouped[k] << rec
  end
  records.each do |r|
    r._preload_{name}(grouped[r.id] || [])
  end
  loaded
end
"#
                );
            }
            PreloadKind::Through { target, join, group_col, order } => {
                let table = crate::naming::pluralize_snake(target.as_str());
                let order_sql = match order {
                    Some(o) => format!(" ORDER BY {o}"),
                    None => String::new(),
                };
                let _ = write!(
                    src,
                    r#"
def self._preload_batch_{name}(records)
  ids = []
  records.each do |r|
    ids << r.id
  end
  grouped = {{}}
  loaded = []
  if ids.length > 0
    rows = ActiveRecord.adapter.select_rows("SELECT {table}.*, {group_col} AS __src FROM {table} {join} WHERE {group_col} IN (" + Db.escape_int_list(ids) + "){order_sql}")
    rows.each do |row|
      rec = {target}.instantiate(row)
      loaded << rec
      k = row["__src"].to_i
      grouped[k] = [] if grouped[k].nil?
      grouped[k] << rec
    end
  end
  records.each do |r|
    r._preload_{name}(grouped[r.id] || [])
  end
  loaded
end
"#
                );
            }
        }
    }

    // Dispatch: one case arm per preloadable assoc; unknown names fall
    // through silently (lazy readers stay correct — mirrors what Rails
    // does for anything `includes` didn't name).
    src.push_str("\ndef self._preload_dispatch(records, name, nested)\n");
    if targets.is_empty() {
        src.push_str("  nil\nend\n");
    } else {
        src.push_str("  case name\n");
        for (name, kind) in &targets {
            let target = match kind {
                PreloadKind::BelongsTo { target, .. } => target,
                PreloadKind::HasMany { target, .. } => target,
                PreloadKind::Through { target, .. } => target,
            };
            let _ = write!(
                src,
                "  when :{name}\n    loaded = _preload_batch_{name}(records)\n    {target}.preload_associations(loaded, [nested]) unless nested.nil?\n"
            );
        }
        src.push_str("  end\n  nil\nend\n");
    }

    // Spec walker — the entry point `Relation#to_a` calls. Specs are
    // Symbols, Hashes (nested: `story: :user`), or Arrays of either.
    src.push_str(
        r#"
def self.preload_associations(records, specs)
  return nil if records.length == 0
  specs.each do |spec|
    if spec.is_a?(Hash)
      spec.each do |name, nested|
        _preload_dispatch(records, name, nested)
      end
    elsif spec.is_a?(Array)
      preload_associations(records, spec)
    elsif !spec.nil?
      _preload_dispatch(records, spec, nil)
    end
  end
  nil
end
"#,
    );

    src
}

/// Monomorphize an app helper on a `raw(...)` argument.
///
/// Rails carries "this string is safe" in the VALUE (SafeBuffer), so a
/// safe label can ride a plain parameter into a helper and out to
/// `link_to`, which then declines to escape it. Nothing survives that
/// boundary here: the emit-time unwrap (`is_html_safe_call`) only sees
/// producers it can name at the call site, and the AOT tree has no
/// safe-string type at all. lobsters' layout hits it once and it shows
/// on every page with a header —
///
///   link_to_different_page raw("#{user}&nbsp;<span class='karma'>…"), settings_path
///
/// rendered `michell_wiegand&amp;nbsp;&lt;span…` on the spinel tree, a
/// +24B diff against Rails on THIRTEEN of the 49 benchmark routes.
///
/// So safety propagates statically instead: a call passing `raw(x)`
/// retargets to a `<name>_raw` clone of the helper in which the safe
/// parameter's escaping consumers are neutralized. This is the same
/// exemption the `link_to(raw(x))` → `link_to_raw(x)` rewrite makes one
/// level down, lifted through one user-defined frame.
///
/// Deliberately narrow. The clone rewrites only what it can prove
/// consumes THAT parameter — `link_to` on it becomes `link_to_raw`, and
/// an `html_escape` of it collapses — and a helper whose safe argument
/// reaches neither is emitted as an ordinary clone rather than guessed
/// at. Widening this is a typed-safe-string project, not a bigger match
/// arm.
pub(crate) fn apply_raw_helper_monomorphization(lcs: &mut [LibraryClass], app: &App) {
    let sites = raw_helper_sites(app);
    if sites.is_empty() {
        return;
    }
    // Definition side: synthesize the `_raw` clone next to the original.
    for lc in lcs.iter_mut() {
        let mut synthesized: Vec<crate::dialect::MethodDef> = Vec::new();
        for m in &lc.methods {
            for (name, idx) in &sites {
                if &m.name != name {
                    continue;
                }
                let Some(param) = m.params.get(*idx) else { continue };
                let mut clone = m.clone();
                clone.name = Symbol::from(format!("{}_raw", name.as_str()));
                let pname = param.name.clone();
                propagate_raw_param(&mut clone.body, &pname);
                synthesized.push(clone);
            }
        }
        lc.methods.extend(synthesized);
    }
    // Call side: `M.helper(raw(x), …)` → `M.helper_raw(x, …)`.
    for lc in lcs.iter_mut() {
        for m in &mut lc.methods {
            rewrite_raw_helper_calls(&mut m.body, &sites);
        }
    }
}

/// `(helper name, index of the argument passed as `raw(...)`)` for every
/// app-helper call site that passes one. Read from `app.views` — i.e.
/// BEFORE lowering, where both the helper call and `raw` are still bare
/// sends — because the two sides of this rewrite are emitted in
/// different LC groups and each needs the same answer.
///
/// Views only: `raw` is a view-layer marker, and a controller reaching
/// for one would be building markup in the wrong place. Extend to
/// controller bodies if a corpus ever does it.
fn raw_helper_sites(app: &App) -> BTreeSet<(Symbol, usize)> {
    let mut out = BTreeSet::new();
    if app.helper_method_index.is_empty() {
        return out;
    }
    let mut scan = |e: &Expr| {
        collect_raw_helper_sites(e, &app.helper_method_index, &mut out);
    };
    for v in &app.views {
        scan(&v.body);
    }
    out
}

fn collect_raw_helper_sites(
    e: &Expr,
    index: &std::collections::HashMap<Symbol, ClassId>,
    out: &mut BTreeSet<(Symbol, usize)>,
) {
    e.node.for_each_child(&mut |c| collect_raw_helper_sites(c, index, out));
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else {
        return;
    };
    if !index.contains_key(method) {
        return;
    }
    for (i, a) in args.iter().enumerate() {
        let is_raw = matches!(&*a.node,
            ExprNode::Send { recv: None, method: m, args: ra, .. }
                if m.as_str() == "raw" && ra.len() == 1);
        if is_raw {
            out.insert((method.clone(), i));
        }
    }
}

/// Inside the `_raw` clone: the named parameter is already safe, so the
/// escaping it would otherwise receive comes off.
fn propagate_raw_param(body: &mut Expr, pname: &Symbol) {
    body.node.for_each_child_mut(&mut |c| propagate_raw_param(c, pname));
    // `ViewHelpers.link_to(<uses param>, …)` → `link_to_raw`.
    if let ExprNode::Send { recv: Some(r), method, args, .. } = &mut *expr_node_mut(body) {
        if method.as_str() == "link_to"
            && is_view_helpers_const(r)
            && args.first().is_some_and(|a| mentions_var(a, pname))
        {
            *method = Symbol::from("link_to_raw");
            return;
        }
    }
    // `ViewHelpers.html_escape(<param>)` → `<param>`.
    let collapse = match &*body.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "html_escape"
                && args.len() == 1
                && is_view_helpers_const(r)
                && mentions_var(&args[0], pname) =>
        {
            Some(args[0].clone())
        }
        _ => None,
    };
    if let Some(inner) = collapse {
        *body = inner;
    }
}

fn expr_node_mut(e: &mut Expr) -> &mut ExprNode {
    &mut e.node
}

/// Does `e` read `name` (directly, or through a `.to_s` the emitter
/// added)?
fn mentions_var(e: &Expr, name: &Symbol) -> bool {
    match &*e.node {
        ExprNode::Var { name: n, .. } => n == name,
        ExprNode::Send { recv: Some(r), args, .. } if args.is_empty() => mentions_var(r, name),
        _ => false,
    }
}

fn rewrite_raw_helper_calls(e: &mut Expr, sites: &BTreeSet<(Symbol, usize)>) {
    e.node.for_each_child_mut(&mut |c| rewrite_raw_helper_calls(c, sites));
    let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &mut *e.node else {
        return;
    };
    let Some((_, idx)) = sites.iter().find(|(n, i)| n == method && args.len() > *i) else {
        return;
    };
    let inner = match &*args[*idx].node {
        ExprNode::Send { recv: Some(r2), method: m2, args: a2, .. }
            if m2.as_str() == "raw" && a2.len() == 1 && is_view_helpers_const(r2) =>
        {
            Some(a2[0].clone())
        }
        _ => None,
    };
    if let Some(inner) = inner {
        let i = *idx;
        *method = Symbol::from(format!("{}_raw", method.as_str()));
        args[i] = inner;
    }
}
