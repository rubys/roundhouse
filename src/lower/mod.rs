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
pub mod controller;
pub mod controller_test;
pub mod fixtures;
pub mod persistence;
pub mod routes;
pub mod schema_sql;
pub mod validations;

pub use associations::{resolve_has_many, HasManyRef};
pub use controller::{
    chain_target_class, classify_controller_send, default_permitted_fields,
    extract_permitted_from_expr, find_nested_parent, has_toplevel_terminal,
    is_format_binding, is_params_expr, is_query_builder_method, lower_action,
    permitted_fields_for, resource_from_controller_name, singularize_to_model,
    resolve_before_actions, split_public_private, synthesize_implicit_render,
    unwrap_respond_to, walk_controller_ivars, ActionKind, LoweredAction,
    NestedParent, SendKind, WalkedIvars,
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
pub use schema_sql::{lower_schema, sqlite_type};
pub use validations::{lower_validations, Check, InclusionValue, LoweredValidation};
