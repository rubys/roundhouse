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

pub mod associations;
pub mod broadcasts;
pub mod chain;
pub mod controller;
pub mod controller_test;
pub mod erb_trim;
pub mod fixtures;
pub mod persistence;
pub mod controller_to_library;
pub mod controller_walk;
pub mod fixture_to_library;
pub mod importmap_to_library;
pub mod model_to_library;
pub mod routes;
pub mod routes_to_library;
pub mod schema_to_library;
pub mod seeds_to_library;
pub mod test_module_to_library;
pub mod typing;
pub mod validations;
pub mod view;
pub mod view_to_library;

pub use controller_walk::{CtrlWalker, Stmt, WalkCtx, WalkState};

pub use associations::{
    build_has_many_table, resolve_has_many, resolve_has_many_on_local, HasManyRef, HasManyRow,
};
pub use chain::{collect_chain_modifiers, ChainModifier};
pub use controller_to_library::{
    lower_controller_to_library_class, lower_controllers_to_library_classes,
};
pub use model_to_library::{
    class_info_from_library_class, lower_model_to_library_class, lower_models_to_library_classes,
    lower_models_to_library_classes_with_params, lower_models_with_registry,
    lower_models_with_registry_and_params,
};
pub use fixture_to_library::{lower_fixtures_to_library_classes, rewrite_fixture_calls};
pub use importmap_to_library::lower_importmap_to_library_functions;
pub use routes_to_library::{
    lower_routes_to_dispatch_functions, lower_routes_to_library_functions,
};
pub use schema_to_library::lower_schema_to_library_functions;
pub use seeds_to_library::lower_seeds_to_library_functions;
pub use test_module_to_library::{
    lower_test_module_to_library_class, lower_test_modules_to_library_classes,
};
pub use view_to_library::{
    flatten_lcs_to_functions, lower_view_to_library_class, lower_views_to_library_classes,
    lower_views_to_library_functions,
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
