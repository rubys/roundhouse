//! Diagnostic walker: collect analyzer diagnostics across the app's typed
//! trees (IR-carried annotations + unresolved/gradual leaf detection) plus the
//! missing-preload coverage pass. Extracted verbatim from `src/analyze/mod.rs`
//! (pure code motion). `diagnose` / `diagnose_with_coverage` are re-exported
//! from `crate::analyze` so external call paths are unchanged.

use crate::App;
use crate::ide::render_ty;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ty::Ty;
use crate::ident::{ClassId, Symbol};
use crate::det::HashMap;

use super::{ClassInfo, PreloadCoverage};
use super::preload;


/// A project extension's attribute macros (`crate::extensions`) make the
/// attributes they name gradual, so a dispatch on the DSL's value object
/// does not fail at every call site. No target lowers the macro call
/// itself, though: the emitted model drops it, and the attribute reads
/// back as its storage type. So the declaration is the refusal, one error
/// at the root of what used to be a cascade, rather than a clean check
/// over a program that would not run.
fn attribute_macro_refusals(app: &App) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if app.extensions.attribute_macros.is_empty() {
        return out;
    }
    for model in &app.models {
        for item in &model.body {
            let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv: None, method, .. } = &*expr.node else { continue };
            if app.extensions.attribute_macro(method.as_str()).is_some() {
                out.push(Diagnostic::unsupported(
                    expr.span,
                    None,
                    method.as_str(),
                    format!(
                        "attribute macro on `{}` from a project extension: its attributes read untyped, and no target lowers the call",
                        model.name.0.as_str()
                    ),
                ));
            }
        }
    }
    out
}

// One error at each declaration, not one per helper call: the helpers stay typed so their callers still infer
/// A `FormBuilder` call the form lowering does not expand. The registry
/// types the whole Rails builder surface (`registry::view`) so a form
/// body types, but `lower::view_to_library` expands only the methods
/// `classify_form_builder_method` names; any other survives into the
/// emit as a literal `form.<method>(…)` with no builder behind it, and
/// the page fails at render. Typed is not supported (invariant 6), so
/// each such call is refused here — as is an expanded field given an
/// option its Rails `render` rewrites and the lowering would pass
/// through as a plain attribute.
fn unlowered_form_builder_calls(app: &App) -> Vec<Diagnostic> {
    fn visit(expr: &Expr, out: &mut Vec<Diagnostic>) {
        if let ExprNode::Send { recv: Some(r), method, args, .. } = &*expr.node
            && !expr.span.is_synthetic()
            && matches!(&r.ty, Some(Ty::Class { id, .. }) if id.0.as_str() == "ActionView::Helpers::FormBuilder")
            && !FORM_BUILDER_HANDLED_ELSEWHERE.contains(&method.as_str())
        {
            let m = method.as_str();
            if crate::lower::classify_form_builder_method(m).is_none() {
                out.push(Diagnostic::unsupported(
                    expr.span,
                    None,
                    "form builder method",
                    format!(
                        "`form.{m}` has no lowering (the form lowering expands label, text_field, text_area, password_field, hidden_field, email_field, url_field, number_field, range_field, search_field, telephone_field/phone_field, check_box, radio_button, select, file_field, rich_text_area, button, submit and fields_for), so the call reaches no method at render"
                    ),
                ));
            } else if let Some(opt) = rewritten_option(m, args) {
                out.push(Diagnostic::unsupported(
                    expr.span,
                    None,
                    "form builder option",
                    format!(
                        "`form.{m}` with `{opt}:` is not modeled: Rails' field rewrites that option when it renders, and the lowering would write it as a plain attribute"
                    ),
                ));
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, out));
    }
    /// The option of `method`'s call that Rails' `Tags::*#render`
    /// rewrites: `NumberField` (and `RangeField`) turn `in:`/`within:`
    /// into `min`/`max`; `SearchField` expands `autosave:`/`onsearch:`.
    fn rewritten_option(method: &str, args: &[Expr]) -> Option<&'static str> {
        let rewritten: &[&'static str] = match method {
            "number_field" | "range_field" => &["in", "within"],
            "search_field" => &["autosave", "onsearch"],
            _ => return None,
        };
        let (_, opts) = crate::lower::classify_form_builder_args(args);
        opts?.iter().find_map(|(k, _)| {
            let key = match &*k.node {
                ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => value.as_str(),
                ExprNode::Lit { value: crate::expr::Literal::Str { value } } => value.as_str(),
                _ => return None,
            };
            rewritten.iter().find(|o| **o == key).copied()
        })
    }
    let mut out = Vec::new();
    for view in &app.views {
        visit(&view.body, &mut out);
    }
    out
}

/// Builder methods the view lowering serves outside
/// `classify_form_builder_method`.
const FORM_BUILDER_HANDLED_ELSEWHERE: &[&str] = &["object"];

fn devise_refusals(app: &App) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if app.gem_lock.as_ref().is_some_and(|lock| lock.is_in_repo("devise") || lock.is_source("devise")) {
        return out;
    }
    // A concern's declaration is spliced into each includer: one refusal per written site, naming every model
    let mut sites: Vec<(crate::span::Span, Vec<&str>)> = Vec::new();
    for model in &app.models {
        for item in &model.body {
            let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { continue };
            let ExprNode::Send { recv: None, method, .. } = &*expr.node else { continue };
            if method.as_str() != "devise" {
                continue;
            }
            match sites.iter_mut().find(|(span, _)| *span == expr.span) {
                Some((_, models)) => models.push(model.name.0.as_str()),
                None => sites.push((expr.span, vec![model.name.0.as_str()])),
            }
        }
    }
    for (span, models) in sites {
        let scopes: Vec<String> = models
            .iter()
            .map(|name| crate::naming::snake_case(name.rsplit("::").next().unwrap_or("")))
            .collect();
        let named: Vec<String> = models.iter().map(|name| format!("`{name}`")).collect();
        let helpers: Vec<String> = scopes.iter().map(|scope| format!("`current_{scope}`")).collect();
        out.push(Diagnostic::unsupported(
            span,
            None,
            "devise",
            format!(
                "`devise` on {}: {}, the `_signed_in?` and `authenticate_..!` helpers, `sign_in` and the rest are typed so their callers infer, but no target ships Devise or Warden, so each call reaches no method at run time",
                named.join(", "),
                helpers.join(", ")
            ),
        ));
    }
    out
}

// Rewritten in place, not added beside: a transpile runs no attribution, so the site's warning would stay its only finding
fn unshipped_gem_helpers(app: &App, out: &mut Vec<Diagnostic>) {
    let Some(lock) = &app.gem_lock else { return };
    let unshipped = |method: &str| {
        crate::gems::gem_claiming_method(lock, method).filter(|gem| {
            !lock.is_in_repo(gem)
                && !lock.is_source(gem)
                && !app.extensions.runtime_gems.iter().any(|row| row.gem == *gem)
        })
    };
    // Not only a typed app method: one whose body answers no type yet is still the app's, if the call reaches it
    let controller_defines = |controller: &crate::dialect::Controller, method: &str| {
        let mut current = Some(controller);
        let mut seen = crate::det::HashSet::default();
        while let Some(c) = current {
            if !seen.insert(c.name.0.as_str()) {
                break;
            }
            if c.actions().any(|a| a.name.as_str() == method) {
                return true;
            }
            current = c.parent.as_ref().and_then(|p| app.controllers.iter().find(|o| o.name == *p));
        }
        false
    };
    let view_defines = |method: &str| {
        let name = Symbol::from(method);
        app.helper_method_index.contains_key(&name) || app.view_visible_controller_methods.contains(&name)
    };
    fn visit<'a>(
        expr: &'a Expr,
        unshipped: &impl Fn(&str) -> Option<&'static str>,
        sites: &mut Vec<(crate::span::Span, &'static str, &'a Symbol)>,
    ) {
        if let ExprNode::Send { recv, method, .. } = &*expr.node
            && recv.as_ref().is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
            && is_unknown_ty(expr.ty.as_ref())
            && !expr.span.is_synthetic()
            && let Some(gem) = unshipped(method.as_str())
        {
            if !sites.iter().any(|(span, ..)| *span == expr.span) {
                sites.push((expr.span, gem, method));
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, unshipped, sites));
    }
    let mut sites = Vec::new();
    for controller in &app.controllers {
        let in_controller = |method: &str| unshipped(method).filter(|_| !controller_defines(controller, method));
        for action in controller.actions() {
            for default in action.opt_params.iter().map(|(_, d)| d).chain(action.kw_params.iter().filter_map(|(_, d)| d.as_ref())) {
                visit(default, &in_controller, &mut sites);
            }
            visit(&action.body, &in_controller, &mut sites);
        }
    }
    let in_view = |method: &str| unshipped(method).filter(|_| !view_defines(method));
    for view in &app.views {
        visit(&view.body, &in_view, &mut sites);
    }
    let refusal = |span, gem: &str, method: &Symbol| {
        Diagnostic::unsupported(
            span,
            None,
            gem,
            format!(
                "`{}` is a helper of the `{gem}` gem, which roundhouse does not model and no target ships, so the call reaches no method at run time; `source_gems` compiles a gem from its source",
                method.as_str()
            ),
        )
    };
    for d in out.iter_mut() {
        match &d.kind {
            DiagnosticKind::UndefinedFilterTarget { target, .. } => {
                if let Some(gem) = unshipped(target.as_str()) {
                    let target = target.clone();
                    *d = refusal(d.span, gem, &target);
                }
            }
            DiagnosticKind::UnresolvedType { .. } | DiagnosticKind::SendDispatchFailed { .. } => {
                if let Some(i) = sites.iter().position(|(span, ..)| *span == d.span) {
                    let (span, gem, method) = sites.remove(i);
                    *d = refusal(span, gem, method);
                }
            }
            _ => {}
        }
    }
    out.extend(sites.into_iter().map(|(span, gem, method)| refusal(span, gem, method)));
}

/// Walk an analyzed `App` collecting every position where typing failed
/// in a way that matters for downstream typed emission. Does not modify
/// the IR — purely a read pass.
///
/// Scope of what's reported:
/// - Ivar reads whose `ty` remained `Ty::Var(0)`.
/// - Send calls with a concrete receiver type whose method wasn't found.
///
/// Deliberately NOT reported (noise suppression):
/// - Bare-name Sends whose receiver is implicit-self / None. Views without
///   a self_ty call many helpers we don't model (e.g. `csrf_meta_tags`);
///   flagging each would drown real diagnostics. Once helpers land via
///   the dialect registry expansion, this filter can be relaxed.
/// - Sends whose receiver itself is unknown. The root cause is upstream;
///   reporting both duplicates signal.
pub fn diagnose(app: &App) -> Vec<Diagnostic> {
    diagnose_with_coverage(app).0
}

/// Separate controller declarations lower independently, including their
/// dispatchers. They cannot share an output path: keeping the last file loses
/// methods, and concatenating them still replaces the earlier dispatcher.
/// Ingest has already dropped openings that only nest classes.
fn diagnose_controller_output_collisions(app: &App, out: &mut Vec<Diagnostic>) {
    use crate::dialect::ControllerBodyItem as Item;
    let mut groups: crate::det::HashMap<String, Vec<&crate::dialect::Controller>> = Default::default();
    let mut paths = Vec::new();
    for controller in &app.controllers {
        let path = format!(
            "app/controllers/{}.rb",
            crate::naming::underscore(controller.name.0.as_str())
        );
        let group = groups.entry(path.clone()).or_default();
        if group.is_empty() {
            paths.push(path);
        }
        group.push(controller);
    }
    for path in paths {
        let declarations = &groups[&path];
        if declarations.len() < 2 {
            continue;
        }
        // The first source location in declaration order, which is file
        // order: `cache_controller/*.rb` comes before `cache_controller.rb`.
        let span = declarations
            .iter()
            .flat_map(|c| {
                std::iter::once(c.parent_span).chain(c.body.iter().filter_map(|item| match item {
                    Item::Action { action, .. } => Some(action.name_span),
                    Item::Filter { filter, .. } => Some(filter.target_span),
                    Item::ClassMethod { method, .. } => Some(method.body.span),
                    Item::ClassIvarInit { expr, .. } | Item::Unknown { expr, .. } => Some(expr.span),
                    Item::PrivateMarker { .. } => None,
                }))
            })
            .find(|span| !span.is_synthetic())
            .unwrap_or_default();
        let mut names: Vec<&str> = Vec::new();
        for name in declarations.iter().map(|c| c.name.0.as_str()) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let detail = match names.as_slice() {
            [name] => format!(
                "`{name}` is declared in more than one file, and each declaration writes `{path}`; only the last would survive"
            ),
            _ => format!(
                "`{}` {} write `{path}`; only the last would survive",
                names.join("` and `"),
                if names.len() == 2 { "both" } else { "all" }
            ),
        };
        out.push(Diagnostic::unsupported(span, None, "controller output collision", detail));
    }
}

/// [`diagnose`] plus the missing-preload coverage triple, for report
/// skins that state the denominator (#64: "0 findings" must be
/// distinguishable from "couldn't check").
pub fn diagnose_with_coverage(app: &App) -> (Vec<Diagnostic>, PreloadCoverage) {
    let mut out = app.routes.diagnostics.clone();
    diagnose_controller_output_collisions(app, &mut out);
    // Only validated synthesized Alba serializers, with per-constructor
    // evidence; this does not widen the general library diagnostic policy.
    out.extend(super::alba::diagnose(app));
    out.extend(crate::gems::extension_gem_refusals(app, None));
    // graphql-ruby object types: their bodies, and each `null: false`
    // field's resolved value.
    out.extend(super::graphql::diagnose(app, diagnose_expr));
    out.extend(super::enum_raw_input::diagnose(app));
    out.extend(super::data::diagnose(app));
    out.extend(super::database_identity::diagnose(app));
    // A support file the class ingest left out, at what failed it.
    out.extend(app.ingest_drops.iter().map(|drop| Diagnostic::ingest_unsupported(drop.span, drop.message.clone())));
    out.extend(conditional_declarations_reopened(app));
    // Constructor refusals must also reach executable class constants and
    // parameter defaults, which the ordinary method diagnostic policy omits.
    fn collect_storage_constructors(expr: &Expr, out: &mut Vec<Diagnostic>) {
        if let Some(DiagnosticKind::Unsupported { construct, detail, .. }) = expr.diagnostic()
            && construct.as_str() == "ActiveStorage constructor"
        {
            out.push(Diagnostic::unsupported(expr.span, None, construct.as_str(), detail.clone()));
        }
        expr.node.for_each_child(&mut |child| collect_storage_constructors(child, out));
    }
    crate::lower::for_each_hook_body_ref(app, &mut |body| collect_storage_constructors(body, &mut out));
    // Rubydex's unresolved constants emit refusal stubs. Collect those
    // annotations from support methods/defaults/constants as well, so
    // an emitted raise cannot be hidden by the library diagnostic policy.
    // Restore the nominal-class operator refusal widened by the fork.
    // Primitive/nullable arithmetic keeps the existing library policy.
    fn nominal_class_operand(ty: &Ty) -> bool {
        match ty {
            Ty::Class { .. } => true,
            Ty::Union { variants } => !variants.is_empty() && variants.iter().all(nominal_class_operand),
            _ => false,
        }
    }
    fn collect_constants(expr: &Expr, out: &mut Vec<Diagnostic>) {
        if let Some(kind @ DiagnosticKind::IncompatibleBinop { op, lhs_ty, .. }) = &expr.diagnostic() {
            if nominal_class_operand(lhs_ty) {
            out.push(Diagnostic {
                span: expr.span,
                severity: Diagnostic::default_severity(kind),
                kind: kind.clone(),
                message: format!("`{op}` with incompatible operand types"),
            });
            }
        }
        if matches!(&expr.diagnostic(),
            Some(DiagnosticKind::Unsupported { .. }))
        {
            if let Some(DiagnosticKind::Unsupported { target, construct, detail }) = &expr.diagnostic()
                && construct.as_str() != crate::diagnostic::CONSTRUCTOR_KEYWORD_ARGUMENTS
            {
                out.push(Diagnostic::unsupported(expr.span, target.clone(), construct.as_str(), detail.clone()));
            }
        }
        // Not dropped with the policy's other misses: the typer stamps it only in place of an Object-extension refusal.
        if let Some(kind @ DiagnosticKind::SendDispatchFailed { method, recv_ty }) = &expr.diagnostic() {
            out.push(Diagnostic {
                span: expr.span,
                severity: Diagnostic::default_severity(kind),
                kind: kind.clone(),
                message: format!("no known method `{}` on {}", method.as_str(), render_ty(recv_ty)),
            });
        }
        expr.node.for_each_child(&mut |child| collect_constants(child, out));
    }
    // Declaration DSL arguments are handled by the class-body ledger;
    // this pass covers executable support methods and initializers.
    for class in app.library_classes.iter().chain(app.rails_application.iter()) {
        for method in &class.methods {
            collect_constants(&method.body, &mut out);
            for param in &method.params {
                if let Some(default) = &param.default { collect_constants(default, &mut out); }
            }
        }
        for (_, value) in &class.constants { collect_constants(value, &mut out); }
        for call in &class.unknown_calls { collect_constants(call, &mut out); }
    }
    // The analyzer resolves model include identities against the registry.
    // Collect that edge's refusal; declaration-marker arguments are metadata
    // handled by the shared model lowerer, rather than executable reads.
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item {
                if let Some(DiagnosticKind::Unsupported { construct, detail, .. }) = &expr.diagnostic() {
                    if construct.as_str() == "include" {
                        out.push(Diagnostic::unsupported(expr.span, None, "include", detail.clone()));
                    }
                }
            }
        }
    }
    out.extend(attribute_macro_refusals(app));
    out.extend(unlowered_form_builder_calls(app));
    out.extend(devise_refusals(app));
    out.extend(overridden_synthesized_accessors(app));
    out.extend(runtime_defined_surfaces(app));
    // A filter's return value is Rails' to discard (`around_action
    // :switch_locale` → `I18n.with_locale(locale, &action)`): nothing
    // escapes from its tail, so an `untyped` there is not a gradual
    // escape. Every other action's tail is its value.
    let filter_targets: crate::det::HashSet<Symbol> = app
        .controllers
        .iter()
        .flat_map(|c| c.body.iter())
        .filter_map(|item| match item {
            crate::dialect::ControllerBodyItem::Filter { filter, .. } => Some(filter.target.clone()),
            _ => None,
        })
        .chain(app.concern_filters.values().flatten().map(|f| f.target.clone()))
        .collect();
    for controller in &app.controllers {
        for action in controller.actions() {
            for default in action.opt_params.iter().map(|(_, d)| d).chain(action.kw_params.iter().filter_map(|(_, d)| d.as_ref())) {
                diagnose_expr(default, &mut out);
            }
            diagnose_expr_in(&action.body, &mut out, !filter_targets.contains(&action.name));
        }
        for method in controller.class_methods() {
            for default in method.params.iter().filter_map(|p| p.default.as_ref()) {
                diagnose_expr(default, &mut out);
            }
            diagnose_expr(&method.body, &mut out);
        }
    }
    for model in &app.models {
        for scope in model.scopes() {
            diagnose_expr(&scope.body, &mut out);
        }
        for method in model.methods() {
            diagnose_expr(&method.body, &mut out);
        }
    }
    for view in &app.views {
        diagnose_expr(&view.body, &mut out);
    }
    match &app.helper_diagnostics {
        Some(taken) => out.extend(taken.iter().cloned()),
        None => out.extend(diagnose_helper_modules(app)),
    }
    for class in &app.library_classes {
        for initializer in &class.class_ivar_initializers {
            diagnose_expr(initializer, &mut out);
        }
    }
    if let Some(seeds) = &app.seeds {
        diagnose_expr(seeds, &mut out);
    }
    out.extend(super::forwarding::diagnose(app));
    out.extend(super::private_calls::diagnose(app));
    out.extend(super::filter_targets::diagnose(app));

    // Static N+1 pass (#64): missing-preload warnings over the typed
    // query chains, same-procedure and through the controller→view
    // ivar channel.
    let (preload_diags, coverage) = preload::missing_preload_report(app);
    out.extend(preload_diags);
    unshipped_gem_helpers(app, &mut out);
    let refused_gem_sites: crate::det::HashSet<_> = out.iter().filter_map(|diagnostic| {
        matches!(&diagnostic.kind, DiagnosticKind::Unsupported { construct, .. }
            if construct.as_str() == "gem_requires_rails").then_some(diagnostic.span)
    }).collect();
    out.retain(|diagnostic| !matches!(&diagnostic.kind,
        DiagnosticKind::Unsupported { construct, .. }
            if construct.as_str() == "constant" && refused_gem_sites.contains(&diagnostic.span)
    ));

    // Collapse diagnostics that render to the same place with the same
    // text — same start position, same kind, same message. Method chains
    // whose links share a (not-yet-precise) start each emit there, so
    // `a.b`, `a.b.c`, `a.b.c.d` stack 2-5 squiggles of differing length
    // but identical tooltip on one spot. Key on `start` (what line:col
    // and the squiggle's anchor derive from), not the full range, so the
    // nested links collapse. `retain` keeps the first — and since the
    // walker emits the outer node before recursing, that's the longest,
    // outermost span. Self-correcting: once span preservation gives links
    // distinct starts, they survive on their own again.
    // Not left a warning: a call only an initializer's top-level `def` answers is a NameError in the emitted app.
    for d in &mut out {
        if let DiagnosticKind::UnresolvedType { expr_kind, name: Some(name) } = &d.kind
            && expr_kind.as_str() == "method call"
            && app.initializer_defs.contains(name)
        {
            *d = Diagnostic::unsupported(
                d.span,
                None,
                "method an initializer defines at top level",
                format!("`{}` is a method on Object only while the initializer has run, and the emitted app does not replay initializer code", name.as_str()),
            );
        }
    }
    let mut seen = crate::det::HashSet::default();
    out.retain(|d| seen.insert((d.span.file, d.span.start, d.code(), d.message.clone())));
    // A template no request reaches (`ingest::unreachable_views`): its errors stay visible, as warnings.
    if !app.unreachable_views.is_empty() {
        let files: crate::det::HashSet<crate::span::FileId> = app
            .views
            .iter()
            .filter(|v| app.unreachable_views.contains(&v.name))
            .map(|v| v.body.span.file)
            .collect();
        for d in &mut out {
            if d.severity == crate::diagnostic::Severity::Error && files.contains(&d.span.file) {
                d.severity = crate::diagnostic::Severity::Warning;
            }
        }
    }
    (out, coverage)
}

/// One warning per class that defines methods under computed names: the
/// sends the registry does not know there answer untyped
/// (`analyze::open_classes`), so what they return is not checked.
fn runtime_defined_surfaces(app: &App) -> Vec<Diagnostic> {
    let open = super::open_classes::OpenClasses::of(app);
    let mut out: Vec<Diagnostic> = open
        .classes()
        .map(|(class, class_side, span)| {
            let kind = DiagnosticKind::GradualUntyped { expr_kind: Symbol::new("runtime-defined method") };
            Diagnostic {
                span: *span,
                severity: crate::diagnostic::Severity::Warning,
                kind,
                message: format!(
                    "`{}` defines {} methods under names computed when it runs, so a call to one the source does not declare answers untyped",
                    class.0.as_str(),
                    if class_side { "class" } else { "instance" }
                ),
            }
        })
        .collect();
    out.sort_by_key(|d| (d.span.file.0, d.span.start, d.message.clone()));
    out
}

/// A model's own `def author` / `def author=` over an association's
/// accessor: Rails calls the model's, the model lowering keeps the
/// synthesized one (`push_user_methods`; a column's yields), so the
/// definition is reported rather than dropped without a word.
fn overridden_synthesized_accessors(app: &App) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for model in &app.models {
        let mut taken: crate::det::HashSet<String> = crate::det::HashSet::default();
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Association { assoc, .. } = item {
                taken.insert(assoc.name().as_str().to_string());
                taken.insert(format!("{}=", assoc.name().as_str()));
            }
        }
        for m in model.methods().filter(|m| {
            m.receiver == crate::dialect::MethodReceiver::Instance
                && matches!(m.kind, crate::dialect::AccessorKind::Method)
                && !m.name_span.is_synthetic()
        }) {
            if !taken.contains(m.name.as_str()) {
                continue;
            }
            out.push(Diagnostic::unsupported(
                m.name_span,
                None,
                "accessor override",
                format!(
                    "`{}#{}` overrides the accessor its association defines, and the emitted model keeps that accessor, so this definition does not run",
                    model.name.0.as_str(),
                    m.name.as_str()
                ),
            ));
        }
    }
    out
}

/// An `app/helpers` module runs in the view, so it is checked as one,
/// not under the library policy.
pub fn diagnose_helper_modules(app: &App) -> Vec<Diagnostic> {
    let helper_modules: crate::det::HashSet<&crate::ident::ClassId> = app.helper_method_index.values().chain(app.helper_modules.iter()).collect();
    let mut helper_out = Vec::new();
    for class in app.library_classes.iter().filter(|c| helper_modules.contains(&c.name)) {
        for method in &class.methods {
            for default in method.params.iter().filter_map(|p| p.default.as_ref()) {
                diagnose_expr(default, &mut helper_out);
            }
            diagnose_expr(&method.body, &mut helper_out);
        }
    }
    // Not an error: the ivar is the rendering view's, which the emitted helper reads as Rails does (only its type is unseeded)
    for d in &mut helper_out {
        if matches!(d.kind, DiagnosticKind::IvarUnresolved { .. }) {
            d.severity = crate::diagnostic::Severity::Warning;
        }
    }
    helper_out
}

/// A type is "unknown" if it's `None` or `Ty::Var(n)` (a placeholder the
/// analyzer set for positions it couldn't resolve). `Ty::Untyped` —
/// the gradual escape — counts as *known*: the author signed that
/// position out of checking.
fn is_unknown_ty(ty: Option<&Ty>) -> bool {
    match ty {
        None => true,
        Some(Ty::Var { .. }) => true,
        _ => false,
    }
}

/// A send whose explicit receiver the analysis could not type.
pub struct UnknownReceiverSend {
    pub span: crate::span::Span,
    pub method: Symbol,
    /// The receiver is the gradual escape (`T.unsafe`, RBS `untyped`)
    /// rather than an unresolved type.
    pub gradual: bool,
    /// The receiver's type is known, but the send resolved to nothing on
    /// it: what `diagnose` reports as `send_dispatch_failed` in a model
    /// or controller body, and nothing reports elsewhere.
    pub known_receiver: bool,
}

/// Every send, in every emitted body, whose explicit receiver is
/// unknown or gradual, or known without the send resolving, and which
/// carries no diagnostic of its own.
///
/// Nothing checks such a send: no method table is consulted, so a
/// misspelled or unimplemented method passes. `diagnose` reports the
/// unresolved receiver only in model and controller bodies, and not at
/// all for a gradual one, so in a library class the send is silent. A
/// drop in errors that only moves sends here is a reclassification, not
/// progress; `check` prints the count beside the error total.
///
/// A receiver whose type names a class the registry does not hold
/// (`#: (Missing::AccessControl)`) is unknown, not known: there is no
/// method table to check it against.
/// True when `method` is registered on the receiver's class (either
/// side), walking parents and includes. A registered method that returns
/// `Ty::Var` still counts as resolved for the unknown-receiver ledger.
fn method_registered_on(
    classes: &HashMap<ClassId, ClassInfo>,
    ty: Option<&Ty>,
    method: &Symbol,
) -> bool {
    match ty {
        Some(Ty::Union { variants }) => {
            !variants.is_empty()
                && variants
                    .iter()
                    .all(|v| method_registered_on(classes, Some(v), method))
        }
        Some(Ty::Class { id, .. }) => {
            let mut current = Some(id);
            for _ in 0..32 {
                let Some(cid) = current else { break };
                let Some(class) = classes.get(cid) else { break };
                if class.instance_methods.contains_key(method)
                    || class.class_methods.contains_key(method)
                {
                    return true;
                }
                if class
                    .includes
                    .iter()
                    .any(|m| classes.get(m).is_some_and(|inc| {
                        inc.instance_methods.contains_key(method)
                            || inc.class_methods.contains_key(method)
                    }))
                {
                    return true;
                }
                current = class.parent.as_ref();
            }
            false
        }
        _ => false,
    }
}

pub fn unknown_receiver_sends(app: &App, classes: &HashMap<ClassId, ClassInfo>) -> Vec<UnknownReceiverSend> {
    fn visit(expr: &Expr, classes: &HashMap<ClassId, ClassInfo>, out: &mut Vec<UnknownReceiverSend>) {
        if let ExprNode::Send { recv: Some(r), method, .. } = &*expr.node {
            let gradual = matches!(r.ty, Some(Ty::Untyped));
            let unknown = is_unknown_ty(r.ty.as_ref())
                || matches!(&r.ty, Some(Ty::Class { id, .. }) if !classes.contains_key(id));
            // Dispatch evidence, not return shape: a known method that
            // answers `Ty::Var` is still checked.
            let known_receiver = !gradual
                && !unknown
                && is_unknown_ty(expr.ty.as_ref())
                && !method_registered_on(classes, r.ty.as_ref(), method);
            if (gradual || known_receiver || unknown)
                && expr.diagnostic().is_none()
                && !expr.span.is_synthetic()
            {
                out.push(UnknownReceiverSend { span: expr.span, method: method.clone(), gradual, known_receiver });
            }
        }
        expr.node.for_each_child(&mut |child| visit(child, classes, out));
    }
    let mut out = Vec::new();
    crate::lower::for_each_emit_body_ref(app, &mut |expr| visit(expr, classes, &mut out));
    let mut seen = crate::det::HashSet::default();
    out.retain(|s| seen.insert((s.span.file, s.span.start, s.span.end, s.method.clone())));
    out
}

/// Short label for what shape of expression resolved to `Untyped`.
/// Used for the `GradualUntyped` diagnostic message so a single
/// kind can name the syntactic position without each callsite
/// recomputing. Lowercase, grep-friendly.
fn expr_kind_label(expr: &Expr) -> &'static str {
    match &*expr.node {
        ExprNode::Send { .. } => "method call",
        ExprNode::Ivar { .. } => "ivar read",
        ExprNode::Var { .. } => "local read",
        ExprNode::Const { .. } => "constant read",
        ExprNode::Apply { .. } => "function call",
        ExprNode::Yield { .. } => "yield",
        _ => "expression",
    }
}

/// The identifier at an unresolved leaf position, for the
/// `UnresolvedType` message — the called method, read local, or
/// constant path. `None` for nameless positions (`yield`). An `Apply`
/// names its callee when that callee is itself a named leaf.
fn unresolved_name(expr: &Expr) -> Option<crate::ident::Symbol> {
    match &*expr.node {
        ExprNode::Send { method, .. } => Some(method.clone()),
        ExprNode::Var { name, .. } => Some(name.clone()),
        ExprNode::Const { path } => Some(crate::ident::Symbol::new(
            &path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
        )),
        ExprNode::Apply { fun, .. } => unresolved_name(fun),
        _ => None,
    }
}

fn diagnose_expr(expr: &Expr, out: &mut Vec<Diagnostic>) {
    diagnose_expr_in(expr, out, true)
}

/// `value_used`: whether this expression's VALUE flows anywhere. A
/// statement before the end of a `Seq`, or the tail of a body whose
/// caller discards the result, contributes nothing downstream, so an
/// `untyped` there is not a gradual escape — reporting it counted the
/// same `I18n.with_locale` twice (the call, then the body `Seq` that
/// takes its tail's type) and once more for nothing. Every other
/// diagnostic is position-independent and reported regardless.
fn diagnose_expr_in(expr: &Expr, out: &mut Vec<Diagnostic>, value_used: bool) {
    // Diagnostic annotations set by the body-typer during analyze.
    // These are the IR-carried path: detection happens once at the
    // point of typing, and every reader (including this walker) sees
    // the same set.
    if let Some(kind) = &expr.diagnostic() {
        let message = match kind {
            // Types render as a Ruby developer reads them (`Integer?`,
            // `Product`), not as the IR's Debug form — the message is
            // what the editor, the MCP and `check` show.
            DiagnosticKind::IncompatibleBinop { op, lhs_ty, rhs_ty } => {
                format!(
                    "`{}` with incompatible operand types: {} {} {}",
                    op.as_str(),
                    render_ty(lhs_ty),
                    op.as_str(),
                    render_ty(rhs_ty)
                )
            }
            DiagnosticKind::IvarUnresolved { name } => {
                format!("@{} has no known type", name.as_str())
            }
            DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
                format!("no known method `{}` on {}", method.as_str(), render_ty(recv_ty))
            }
            DiagnosticKind::GradualUntyped { expr_kind } => {
                format!("{} resolves to RBS `untyped` (gradual escape)", expr_kind.as_str())
            }
            DiagnosticKind::UnresolvedType { expr_kind, name } => {
                Diagnostic::unresolved_type_text(expr_kind, name.as_ref())
            }
            DiagnosticKind::Unsupported { target, construct, detail } => {
                let mut m = Diagnostic::unsupported_text(target.as_ref(), construct);
                if !detail.is_empty() {
                    m.push_str(": ");
                    m.push_str(detail);
                }
                m
            }
            // Parse diagnostics come from the ingest parse wrapper and
            // MissingPreload from the post-walk preload pass — neither
            // is carried as an `Expr.diagnostic` annotation; handled
            // defensively so the match stays exhaustive.
            DiagnosticKind::Parse { message } => format!("syntax error: {message}"),
            DiagnosticKind::MissingPreload { association, .. } => {
                format!("query does not preload :{}", association.as_str())
            }
            // From `App::ingest_drops`, never an `Expr.diagnostic`.
            DiagnosticKind::IngestUnsupported { .. } => Diagnostic::stub_text(kind),
            // Produced by `lower::apply_post_analyze_lowerings` as
            // returned lists, never as `Expr.diagnostic` annotations;
            // handled defensively so the match stays exhaustive.
            DiagnosticKind::BlankUnlowered { method, reason, .. } => {
                format!("`{}` left as dynamic dispatch ({})", method.as_str(), reason.as_str())
            }
            DiagnosticKind::LowerResidue { pass, construct, reason } => {
                format!(
                    "`{}` left unlowered by {} ({})",
                    construct.as_str(),
                    pass.as_str(),
                    reason.as_str()
                )
            }
            // Produced by `filter_targets::diagnose` as a returned list,
            // never as an `Expr.diagnostic` annotation.
            DiagnosticKind::UndefinedFilterTarget { .. } => Diagnostic::stub_text(kind),
            // Produced by `graphql::diagnose` as a returned list.
            DiagnosticKind::GraphqlNullableField { .. } => Diagnostic::stub_text(kind),
            // Produced by `ingest::initializer_ledger` as a returned list.
            DiagnosticKind::UnconsumedInitializer { .. } => Diagnostic::stub_text(kind),
            // Produced by `ingest::model::validations` as a pushed note.
            DiagnosticKind::IgnoredValidatorOption { .. } => Diagnostic::stub_text(kind),
        };
        if !matches!(
            kind,
            DiagnosticKind::Unsupported { construct, .. }
                if construct.as_str() == crate::diagnostic::CONSTRUCTOR_KEYWORD_ARGUMENTS
        ) {
            let severity = match kind {
                // Not an error where some operand arm takes it: Ruby raises for the others, which the ruby family runs as written and a strict target reports or checks (`lower::nil_operand`)
                DiagnosticKind::IncompatibleBinop { op, lhs_ty, rhs_ty }
                    if crate::analyze::body::some_operands_take_it(op.as_str(), lhs_ty, rhs_ty) =>
                {
                    crate::diagnostic::Severity::Warning
                }
                _ => Diagnostic::default_severity(kind),
            };
            out.push(Diagnostic { span: expr.span, kind: kind.clone(), severity, message });
        }
    }

    // RBS-declared `untyped` reaches this site. Emit a GradualUntyped
    // warning so consumers can track gradual-escape coverage and so
    // strict-target emitters can elevate to Error at emit time. The
    // body-typer doesn't annotate `expr.diagnostic` for Untyped — the
    // walker is the natural place since every node's `.ty` already
    // carries the signal.
    // A `Seq` has its tail's type; the tail reports itself. ForwardArgs is
    // an argument-packet marker, not a value escaping the type system.
    if value_used
        && matches!(expr.ty.as_ref(), Some(Ty::Untyped))
        && !matches!(
            &*expr.node,
            ExprNode::Seq { .. } | ExprNode::ForwardArgs | ExprNode::ForwardKeywords
                | ExprNode::ForwardKeywordsWithPairs { .. }
        )
    {
        let kind = DiagnosticKind::GradualUntyped {
            expr_kind: crate::ident::Symbol::new(expr_kind_label(expr)),
        };
        out.push(Diagnostic {
            span: expr.span,
            severity: Diagnostic::default_severity(&kind),
            kind,
            message: format!(
                "{} resolves to RBS `untyped` (gradual escape)",
                expr_kind_label(expr)
            ),
        });
    }

    match &*expr.node {
        ExprNode::Ivar { name } => {
            if is_unknown_ty(expr.ty.as_ref()) {
                let kind = DiagnosticKind::IvarUnresolved { name: name.clone() };
                out.push(Diagnostic {
                    span: expr.span,
                    severity: Diagnostic::default_severity(&kind),
                    kind,
                    message: format!("@{} has no known type", name.as_str()),
                });
            }
        }
        ExprNode::Send { recv: Some(r), method, .. } => {
            if !is_unknown_ty(r.ty.as_ref()) && is_unknown_ty(expr.ty.as_ref()) {
                let recv_ty = r.ty.clone().unwrap_or_else(|| Ty::Var { var: crate::ident::TyVar(0) });
                let kind = DiagnosticKind::SendDispatchFailed {
                    method: method.clone(),
                    recv_ty: recv_ty.clone(),
                };
                out.push(Diagnostic {
                    span: expr.span,
                    severity: Diagnostic::default_severity(&kind),
                    kind,
                    message: format!(
                        "no known method `{}` on {}",
                        method.as_str(),
                        render_ty(&recv_ty),
                    ),
                });
            }
        }
        _ => {}
    }

    // Residual unresolved positions the specific checks above don't
    // cover — the "silently unresolved" set. The body-typer left these
    // as an open inference variable (`Ty::Var`) or never stamped a type
    // (`None`), but no diagnostic fires today, so they pass invisibly:
    //   - implicit-self sends (`controller_name`, recv: None)
    //   - bare local and constant reads
    //   - function applies and yields
    // Ivars are reported by IvarUnresolved; explicit-receiver sends with
    // a *known* receiver by SendDispatchFailed. An explicit receiver that
    // is itself unresolved is reported on the receiver node when we
    // recurse, so the outer send is skipped here to avoid double-counting
    // the same root cause.
    //
    // A node the compiler synthesized (an `attr_writer`'s `value`) has no
    // source position: there is nothing for the author to look at or fix.
    if is_unknown_ty(expr.ty.as_ref()) && !expr.span.is_synthetic()
        && !matches!(expr.diagnostic(), Some(DiagnosticKind::Unsupported { .. }))
    {
        let report = matches!(
            &*expr.node,
            ExprNode::Send { recv: None, .. }
                | ExprNode::Var { .. }
                | ExprNode::Const { .. }
                | ExprNode::Apply { .. }
                | ExprNode::Yield { .. }
        );
        if report {
            let label = crate::ident::Symbol::new(expr_kind_label(expr));
            let name = unresolved_name(expr);
            let message = Diagnostic::unresolved_type_text(&label, name.as_ref());
            let kind = DiagnosticKind::UnresolvedType { expr_kind: label, name };
            out.push(Diagnostic {
                span: expr.span,
                severity: Diagnostic::default_severity(&kind),
                kind,
                message,
            });
        }
    }

    // Recurse into children so we surface every unresolved position.
    match &*expr.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                diagnose_expr(r, out);
            }
            for a in args {
                diagnose_expr(a, out);
            }
            if let Some(b) = block {
                diagnose_expr(b, out);
            }
        }
        ExprNode::Seq { exprs } => {
            let last = exprs.len().saturating_sub(1);
            for (i, e) in exprs.iter().enumerate() {
                diagnose_expr_in(e, out, value_used && i == last);
            }
        }
        ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                diagnose_expr(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                diagnose_expr(k, out);
                diagnose_expr(v, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    diagnose_expr(expr, out);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            diagnose_expr(left, out);
            diagnose_expr(right, out);
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            diagnose_expr(cond, out);
            diagnose_expr(then_branch, out);
            diagnose_expr(else_branch, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            diagnose_expr(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    diagnose_expr(g, out);
                }
                diagnose_expr(&arm.body, out);
            }
        }
        ExprNode::CaseMatch { scrutinee, arms, else_body } => {
            diagnose_expr(scrutinee, out);
            for arm in arms {
                arm.pattern.for_each_expr(&mut |e| diagnose_expr(e, out));
                if let Some((_, g)) = &arm.guard {
                    diagnose_expr(g, out);
                }
                diagnose_expr(&arm.body, out);
            }
            if let Some(e) = else_body {
                diagnose_expr(e, out);
            }
        }
        ExprNode::MatchPredicate { value, pattern } | ExprNode::MatchRequired { value, pattern } => {
            diagnose_expr(value, out);
            pattern.for_each_expr(&mut |e| diagnose_expr(e, out));
        }
        ExprNode::Let { value, body, .. } => {
            diagnose_expr(value, out);
            diagnose_expr(body, out);
        }
        ExprNode::Lambda { body, .. } => {
            diagnose_expr(body, out);
        }
        ExprNode::MethodRef { recv, .. } => {
            if let Some(r) = recv {
                diagnose_expr(r, out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            diagnose_expr(fun, out);
            for a in args {
                diagnose_expr(a, out);
            }
            if let Some(b) = block {
                diagnose_expr(b, out);
            }
        }
        ExprNode::Assign { target, value }
        | ExprNode::OpAssign { target, value, .. } => {
            diagnose_expr(value, out);
            if let LValue::Attr { recv, .. } = target {
                diagnose_expr(recv, out);
            }
            if let LValue::Index { recv, index } = target {
                diagnose_expr(recv, out);
                diagnose_expr(index, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                diagnose_expr(a, out);
            }
        }
        ExprNode::Raise { value } => diagnose_expr(value, out),
        ExprNode::Return { value } => diagnose_expr(value, out),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    diagnose_expr(a, out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            diagnose_expr(body, out);
            for rc in rescues {
                for c in &rc.classes {
                    diagnose_expr(c, out);
                }
                diagnose_expr(&rc.body, out);
            }
            if let Some(e) = else_branch {
                diagnose_expr(e, out);
            }
            if let Some(e) = ensure {
                diagnose_expr(e, out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value { diagnose_expr(v, out); }
        }
        ExprNode::Splat { value } | ExprNode::KeywordSplat { value } => diagnose_expr(value, out),
        ExprNode::ForwardKeywordsWithPairs { entries } => {
            for (key, value) in entries {
                diagnose_expr(key, out);
                diagnose_expr(value, out);
            }
        }
        ExprNode::MultiAssign { targets, value } => {
            diagnose_expr(value, out);
            for target in targets {
                if let LValue::Attr { recv, .. } = target {
                    diagnose_expr(recv, out);
                }
                if let LValue::Index { recv, index } = target {
                    diagnose_expr(recv, out);
                    diagnose_expr(index, out);
                }
            }
        }
        ExprNode::While { cond, body, .. } => {
            diagnose_expr(cond, out);
            diagnose_expr(body, out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin { diagnose_expr(b, out); }
            if let Some(e) = end { diagnose_expr(e, out); }
        }
        ExprNode::Cast { value, .. } => diagnose_expr(value, out),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::ForwardArgs
        | ExprNode::ForwardKeywords
        | ExprNode::Defined { .. }
        | ExprNode::SelfRef => {}
    }
}

/// A class or module declared in a load-time conditional that another
/// support file also declares, unconditionally. The IR holds one class
/// per name, so the two bodies merge and the emitted tree writes the
/// merged class inside the conditional's branch: when the branch does
/// not run, what the other file defines is missing too, where Ruby still
/// defines it. Reported at the conditional declaration, naming the other
/// file. A reopening in the same file is not detected here.
fn conditional_declarations_reopened(app: &App) -> Vec<Diagnostic> {
    fn declarations(conditional: &crate::dialect::ConditionalDefinitions, out: &mut Vec<(crate::ident::ClassId, u32)>) {
        for arm in &conditional.arms {
            out.extend(arm.declarations.iter().map(|d| (d.name.clone(), d.start)));
            for nested in &arm.nested {
                declarations(nested, out);
            }
        }
    }
    let source_of = |file: crate::span::FileId| -> Option<&str> {
        app.sources.get((file.0 as usize).checked_sub(1)?).map(|s| s.path.as_str())
    };
    let relative = |path: &str| -> String {
        path.strip_prefix(app.root.as_str()).map(|p| p.trim_start_matches('/')).unwrap_or(path).to_string()
    };
    let mut out = Vec::new();
    for lc in &app.library_classes {
        for conditional in &lc.conditional_definitions {
            let Some(own) = source_of(conditional.span.file) else { continue };
            let mut declared = Vec::new();
            declarations(conditional, &mut declared);
            for (name, start) in declared {
                let others: Vec<String> = app
                    .library_class_files
                    .iter()
                    .filter(|(source, names)| source != own && names.contains(&name))
                    .map(|(source, _)| relative(source))
                    .collect();
                if others.is_empty() {
                    continue;
                }
                let span = crate::span::Span { file: conditional.span.file, start, end: start + 1 };
                out.push(Diagnostic::unsupported(
                    span,
                    None,
                    "conditional declaration reopened elsewhere",
                    format!(
                        "`{}` is declared in a load-time conditional here and unconditionally in {}; the emitted tree writes the merged class inside the conditional's branch, so when the branch does not run what {} defines is missing as well, which is not modeled",
                        name.0.as_str(),
                        others.join(", "),
                        if others.len() == 1 { "that file" } else { "those files" },
                    ),
                ));
            }
        }
    }
    out
}
