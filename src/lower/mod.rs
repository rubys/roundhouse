//! Target-neutral lowerings of dialect IR.
//!
//! Phase 4's core contribution over railcar: extract the logic that's
//! identical across target runtimes (validation evaluation, SQL string
//! generation, router dispatch, turbo-stream templating) as IR-level
//! lowerings. Each target emitter consumes the lowered form and renders
//! it in target-specific code, so adding a new target is mostly
//! writing renders, not re-implementing the logic.
//!
//! The lowering IR lives alongside the dialect IR — it doesn't replace
//! it. Surface IR captures what the developer wrote (`validates :title,
//! presence: true`), lowered IR captures what an evaluator needs to do
//! (`Check::Presence { attr: "title" }`). Emitters read both, but the
//! per-target boilerplate shrinks to "render this lowered form."
//!
//! Starting with validations as the pilot — smallest scope that
//! exercises the pattern. If it works, follow-ups cover query algebra,
//! broadcasts orchestration, schema → DDL, and router dispatch tables.

pub mod arel;
pub mod associations;
pub mod blank;
pub mod broadcasts;
pub mod chain;
pub mod controller;
pub mod controller_test;
pub mod erb_trim;
pub mod fixtures;
pub mod functionalize;
pub mod model_associations;
pub mod persistence;
pub mod controller_to_library;
pub mod controller_walk;
pub mod fixture_to_library;
pub mod importmap_to_library;
pub mod jbuilder_to_library;
pub mod library_extras;
pub mod model_to_library;
pub mod routes;
pub mod routes_to_library;
pub mod scope_chain;
pub mod schema_to_library;
pub mod seeds_to_library;
pub mod test_module_to_library;
pub mod create_block;
pub mod duration;
pub mod and_return;
pub mod case_lambda;
pub mod first_or_create;
pub mod group_count;
pub mod dead_default;
pub mod errors_add;
pub mod job_class_side;
pub mod mailer_class_side;
pub mod as_json_super;
pub mod parameterize;
pub mod request_index;
pub mod send_dispatch;
pub(crate) mod secure_password;
pub mod capture_inline;
pub mod partial_qualify;
pub mod time_current;
pub mod transaction_ground;
pub mod update_kwargs;
pub(crate) mod typed_store;
pub mod ty_coerce_insertion;
pub mod typing;
pub mod validations;
pub mod view;
pub mod view_to_library;

pub use blank::apply_blank_lowering;
pub use create_block::apply_create_block_inline;
pub use duration::apply_duration_lowering;
pub use and_return::apply_and_return_lowering;
pub use case_lambda::apply_case_lambda_lowering;
pub use first_or_create::apply_first_or_create_lowering;
pub use group_count::apply_group_count_lowering;
pub use dead_default::apply_dead_default_lowering;
pub use errors_add::apply_errors_add_lowering;
pub use mailer_class_side::apply_mailer_class_side;
pub use as_json_super::apply_as_json_super_grounding;
pub use parameterize::apply_parameterize_grounding;
pub use request_index::apply_request_index_lowering;
pub use send_dispatch::apply_send_static_dispatch;
pub use capture_inline::apply_capture_inline;
pub use partial_qualify::apply_partial_qualification;
pub use time_current::apply_time_current_lowering;
pub use transaction_ground::apply_transaction_grounding;
pub use update_kwargs::apply_update_kwargs_inline;

/// Build a `LowerResidue` diagnostic — the shared assembly a pass emits
/// when it must leave a construct dynamic. Each pass supplies its own
/// `pass`/`construct` tags, `span`, and human-readable `message`; the
/// kind construction, default severity, and field wiring live here so
/// the six residue-emitting passes don't each re-derive them. Callers
/// interpolate `reason` into `message` themselves (the phrasing is
/// per-pass), so it is passed both as a diagnostic field and left to the
/// caller's message text.
pub(crate) fn residue_diagnostic(
    pass: &str,
    construct: &str,
    span: crate::span::Span,
    reason: &str,
    message: String,
) -> crate::diagnostic::Diagnostic {
    use crate::diagnostic::{Diagnostic, DiagnosticKind};
    use crate::ident::Symbol;
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from(pass),
        construct: Symbol::from(construct),
        reason: Symbol::from(reason),
    };
    Diagnostic {
        span,
        severity: Diagnostic::default_severity(&kind),
        kind,
        message,
    }
}

/// Post-analyze shared lowerings — type-directed IR rewrites every
/// target consumes, run between `Analyzer::analyze` and any emitter.
/// One entry point so the transpile driver, the site build, and the IR
/// dump can't drift as passes accumulate (the LSP/MCP/IDE paths stay
/// off it on purpose: they want source-shaped IR). Returns the residue
/// diagnostics — sites a pass had to leave dynamic, with the reason.
///
/// `registry` is the analyzer's post-fixpoint class table
/// ([`crate::analyze::Analyzer::class_registry`]) — passes that
/// synthesize dispatches consult it to stamp what analyze would have
/// computed.
pub fn apply_post_analyze_lowerings(
    app: &mut crate::app::App,
    registry: &std::collections::HashMap<crate::ident::ClassId, crate::analyze::ClassInfo>,
) -> Vec<crate::diagnostic::Diagnostic> {
    let mut diags = blank::apply_blank_lowering(app);
    time_current::apply_time_current_lowering(app);
    as_json_super::apply_as_json_super_grounding(app);
    parameterize::apply_parameterize_grounding(app);
    request_index::apply_request_index_lowering(app);
    transaction_ground::apply_transaction_grounding(app);
    partial_qualify::apply_partial_qualification(app);
    capture_inline::apply_capture_inline(app);
    and_return::apply_and_return_lowering(app);
    case_lambda::apply_case_lambda_lowering(app);
    first_or_create::apply_first_or_create_lowering(app);
    group_count::apply_group_count_lowering(app);
    dead_default::apply_dead_default_lowering(app, registry);
    diags.extend(errors_add::apply_errors_add_lowering(app));
    diags.extend(create_block::apply_create_block_inline(app));
    diags.extend(update_kwargs::apply_update_kwargs_inline(app));
    diags.extend(mailer_class_side::apply_mailer_class_side(app));
    diags.extend(job_class_side::apply_job_class_side(app));
    diags.extend(send_dispatch::apply_send_static_dispatch(app, registry));
    // AFTER send_dispatch, by contract: an all-duration-unit name set
    // dispatches through case arms synthesized as plural unit calls
    // that count on this grounding (`send_dispatch::duration_plural`).
    duration::apply_duration_lowering(app);
    diags
}

/// Every app body the post-analyze hook owns: model methods, scope
/// bodies, callback conditions and unrecognized class-body exprs;
/// library-class methods; controller actions and unrecognized items;
/// seeds. Param DEFAULTS ride along everywhere a body does — a default
/// is call-time-evaluated body code, and `def initialize(cache_time =
/// 30.minutes)` needs the duration grounding (or `Time.current` its
/// own) exactly as much as a body site; defaults were the one
/// reachable-expr position the hook skipped (lobsters'
/// FlaggedCommenters left an ungrounded `Integer#minutes` send whose
/// untyped result every downstream consumer inherited). The one
/// definition of the hook's scope — passes iterate through here so
/// they can't drift. View bodies are deliberately excluded (each
/// target's view pipeline still has its own working walkers over
/// source shapes — see the note in [`blank::apply_blank_lowering`];
/// views rejoin when the view pipeline migrates to shared lowerings).
/// Test-module and fixture bodies are excluded too (they run on CRuby
/// lanes; extendable when a strict-target test lane needs it).
pub(crate) fn for_each_hook_body(
    app: &mut crate::app::App,
    f: &mut impl FnMut(&mut crate::expr::Expr),
) {
    fn visit_param_defaults(
        params: &mut [crate::dialect::Param],
        f: &mut impl FnMut(&mut crate::expr::Expr),
    ) {
        for p in params {
            if let Some(default) = &mut p.default {
                f(default);
            }
        }
    }
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => {
                    visit_param_defaults(&mut method.params, f);
                    f(&mut method.body)
                }
                crate::dialect::ModelBodyItem::Scope { scope, .. } => {
                    visit_param_defaults(&mut scope.params, f);
                    f(&mut scope.body)
                }
                crate::dialect::ModelBodyItem::Callback { callback, .. } => {
                    if let Some(cond) = &mut callback.condition {
                        f(cond);
                    }
                }
                // Unrecognized class-body exprs (constant procs and
                // friends) round-trip verbatim into the emit — their
                // sites are just as reachable.
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => f(expr),
                _ => {}
            }
        }
    }
    for lc in &mut app.library_classes {
        for method in &mut lc.methods {
            visit_param_defaults(&mut method.params, f);
            f(&mut method.body);
        }
    }
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => {
                    for (_name, default) in &mut action.opt_params {
                        f(default);
                    }
                    f(&mut action.body)
                }
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => f(expr),
                _ => {}
            }
        }
    }
    if let Some(seeds) = &mut app.seeds {
        f(seeds);
    }
}
pub use controller_walk::{CtrlWalker, Stmt, WalkCtx, WalkState};

pub use associations::{
    build_has_many_table, resolve_has_many, resolve_has_many_on_local, HasManyRef, HasManyRow,
};
pub use chain::{collect_chain_modifiers, ChainModifier};
pub use controller_to_library::{
    lower_controller_to_library_class, lower_controllers_to_library_classes,
    lower_controllers_with_arel, lower_controllers_with_arel_and_views,
    lower_controllers_with_arel_views_and_assocs,
    lower_controllers_with_arel_views_assocs_and_routes,
};
pub use model_to_library::{
    class_info_from_library_class, lower_model_to_library_class, lower_models_to_library_classes,
    lower_models_to_library_classes_with_params, lower_models_with_registry,
    lower_models_with_registry_and_params,
};
pub use fixture_to_library::{lower_fixtures_to_library_classes, rewrite_fixture_calls};
pub use importmap_to_library::lower_importmap_to_library_functions;
pub use library_extras::{extras_from_funcs, extras_from_lcs};
pub use routes_to_library::{
    lower_routes_to_dispatch_functions, lower_routes_to_library_functions,
};
pub use schema_to_library::lower_schema_to_library_functions;
pub use seeds_to_library::lower_seeds_to_library_functions;
pub use test_module_to_library::{
    lower_test_module_to_library_class, lower_test_modules_to_library_classes,
    lower_test_modules_with_inner, LoweredTestModule,
};
pub use ty_coerce_insertion::{insert_ty_coercions, insert_ty_coercions_with_extras};
pub use view_to_library::{
    ViewLowerCtx, flatten_lcs_to_functions, lower_view_to_library_class,
    lower_views_to_library_classes, lower_views_to_library_functions,
};
pub use jbuilder_to_library::{
    lower_jbuilder_to_library_class, lower_jbuilder_to_library_classes,
};
pub use broadcasts::{
    lower_broadcasts, BroadcastAction, LoweredAssocRef, LoweredBroadcast, LoweredBroadcasts,
};
pub use controller::{
    chain_target_class, classify_controller_send, default_permitted_fields,
    extract_permitted_from_expr, extract_status_from_kwargs, find_nested_parent,
    has_toplevel_terminal, is_empty_body, is_format_binding, is_params_expr,
    is_query_builder_method, is_resource_params_call, lower_action,
    model_new_with_strong_params, normalize_action_body, permitted_fields_for,
    resolve_before_actions, resource_from_controller_name, singularize_to_model,
    split_public_private, status_sym_to_code, synthesize_implicit_render,
    unwrap_respond_to, update_with_strong_params, walk_controller_ivars,
    ActionKind, LoweredAction, NestedParent, SendKind, WalkedIvars,
};
pub use controller_test::{
    classify_assert_select, classify_controller_test_send, classify_url_expr,
    flatten_params_pairs, test_body_stmts, AssertSelectKind, ControllerTestSend, UrlArg,
    UrlHelperCall,
};
pub use fixtures::{
    lower_fixtures, LoweredFixture, LoweredFixtureField, LoweredFixtureRecord, LoweredFixtureSet,
    LoweredFixtureValue,
};
pub use persistence::{lower_persistence, BelongsToCheck, DependentChild, LoweredPersistence};
pub use routes::{flatten_routes, standard_resource_actions, FlatRoute};
pub use validations::{lower_validations, Check, InclusionValue, LoweredValidation};
pub use view::{
    classify_class_value, classify_errors_field_predicate, classify_form_builder_args,
    classify_form_builder_method, classify_nested_form_child, classify_nested_url_element,
    classify_render_partial, classify_view_helper, classify_view_url_arg, ClassValueShape,
    ErrorsFieldPredicate, FormBuilderMethod, NestedFormChild, NestedUrlElement, RenderPartial,
    ViewHelperKind, ViewUrlArg,
};
