//! Controller-body lowering — shared Phase 4c analysis.
//!
//! The four Phase-4c emitters (Rust, Crystal, Go, Elixir) each wanted
//! to match Ruby controller-body `Send` shapes and rewrite them into
//! a target-specific runtime call. The IR-match logic was identical;
//! only the rendering varied.
//!
//! Organized by concern:
//!
//! - [`send`] — the `SendKind` classifier that collapses every
//!   controller-body `Send` shape the emitters care about into one
//!   tagged enum.
//! - [`actions`] — action lowering (`LoweredAction`, `lower_action`),
//!   public/private partitioning, before-action filter resolution.
//! - [`body`] — action-body normalization pipeline (`respond_to`
//!   flattening, implicit-render synthesis, empty-body predicate).
//! - [`permitted`] — strong-params recognition and field extraction.
//! - [`ivars`] — ivar read/write walker used to prime handler
//!   locals.
//! - [`nesting`] — resource-name and nested-parent resolution.
//! - [`util`] — small leaf predicates and lookups shared across the
//!   submodules (`is_params_expr`, `is_format_binding`,
//!   `singularize_to_model`, status-code mapping, …).
//!
//! Variants / helpers live in `send` / `body` when the shape appears
//! in at least three of the four emitters — validation that they're
//! shape-shaped, not target-shaped. Target-specific rewrites
//! (Elixir's struct-method-to-Module-function conversion) stay in
//! the emitter.

pub mod actions;
pub mod body;
pub mod ivars;
pub mod nesting;
pub mod permitted;
pub mod send;
pub mod util;

pub use actions::{
    lower_action, resolve_before_actions, split_public_private, ActionKind, LoweredAction,
};
pub use body::{
    has_toplevel_terminal, is_empty_body, normalize_action_body, synthesize_implicit_render,
    unwrap_respond_to,
};
pub use ivars::{walk_controller_ivars, WalkedIvars};
pub use nesting::{find_nested_parent, resource_from_controller_name, NestedParent};
pub use permitted::{
    default_permitted_fields, extract_permitted_from_expr, is_resource_params_call,
    model_new_with_strong_params, permitted_fields_for, update_with_strong_params,
};
pub use send::{classify_controller_send, SendKind};
pub use util::{
    chain_target_class, extract_status_from_kwargs, is_format_binding, is_params_expr,
    singularize_to_model, status_sym_to_code,
};

// `is_query_builder_method` moved to `crate::catalog`. It's a
// runtime-capability concern (which AR methods the scaffold
// runtime stubs implement as collapse-to-empty) that will
// eventually become a `DatabaseAdapter` trait method. Re-export
// so existing callers compile unchanged.
pub use crate::catalog::is_query_builder_method;
