//! Action lowering + filter resolution — `split_public_private`
//! partitions a controller's body at its `private` marker;
//! `lower_action` bundles the six facts every emitter needs into a
//! single `LoweredAction`; `resolve_before_actions` inlines applicable
//! `before_action` callback bodies ahead of the action body.

use crate::dialect::{Action, Controller, ControllerBodyItem, Filter, FilterKind};
use crate::expr::{Expr, ExprNode};

use super::nesting::NestedParent;

/// Walk a controller's source-ordered body, partitioning actions into
/// those before the `private` marker vs. those after. Filters and
/// Unknown class-body calls are informational-only for emit and get
/// dropped; PrivateMarker is consumed as the partition point.
pub fn split_public_private(c: &Controller) -> (Vec<Action>, Vec<Action>) {
    let mut pubs = Vec::new();
    let mut privs = Vec::new();
    let mut seen_private = false;
    for item in &c.body {
        match item {
            ControllerBodyItem::PrivateMarker { .. } => seen_private = true,
            ControllerBodyItem::Action { action, .. } => {
                if seen_private {
                    privs.push(action.clone());
                } else {
                    pubs.push(action.clone());
                }
            }
            _ => {}
        }
    }
    (pubs, privs)
}

/// The seven standard Rails scaffold actions plus an Unknown fallback
/// for anything the template-per-action pipeline doesn't model.
/// Emitters dispatch on this to pick a render template; the per-
/// target code shrinks to "render this variant."
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    Index,
    Show,
    New,
    Edit,
    Create,
    Update,
    Destroy,
    /// Any custom action — emitters render as a 501 stub keyed off
    /// `LoweredAction::name`.
    Unknown,
}

impl ActionKind {
    fn from_name(name: &str) -> Self {
        match name {
            "index" => Self::Index,
            "show" => Self::Show,
            "new" => Self::New,
            "edit" => Self::Edit,
            "create" => Self::Create,
            "update" => Self::Update,
            "destroy" => Self::Destroy,
            _ => Self::Unknown,
        }
    }
}

/// Target-neutral view of one action's emit-relevant inputs. Every
/// pass-2 emitter needed the same six facts (name, resource, model
/// class, whether the model exists, nested parent, permitted
/// fields) — lifting them into a single struct is the forcing
/// function for collapsing 42 near-identical per-target functions
/// down to six render tables.
#[derive(Clone, Debug)]
pub struct LoweredAction {
    pub kind: ActionKind,
    /// The action's declared name in Ruby (`"index"`, `"create"`,
    /// and also arbitrary custom-action names when `kind ==
    /// Unknown`). Emitters can derive their target-specific handler
    /// names (`PostsIndex`, `articles/index`, etc.) from this plus
    /// the controller class.
    pub name: String,
    /// Singular snake-case resource name (`"article"`). Used to
    /// key form-body params (`"article[title]"`) and to derive
    /// route helpers.
    pub resource: String,
    /// PascalCase model class (`"Article"`). Empty when
    /// `has_model` is false.
    pub model_class: String,
    /// Whether the resource maps to a known model in this app.
    /// Emitters gate the DB-touching body on this; an
    /// `ApplicationController`'s actions lower with
    /// `has_model = false`.
    pub has_model: bool,
    /// The parent resource when this controller is nested under
    /// another (`comment → article`).
    pub parent: Option<NestedParent>,
    /// Field names to pick out of form-body params during
    /// create/update.
    pub permitted: Vec<String>,
}

/// Build a `LoweredAction` from the inputs every pass-2 emitter
/// already computed at the controller-file level. Cheap to
/// construct — essentially just a tagged bundle.
pub fn lower_action(
    name: &str,
    resource: &str,
    model_class: &str,
    has_model: bool,
    parent: Option<&NestedParent>,
    permitted: &[String],
) -> LoweredAction {
    LoweredAction {
        kind: ActionKind::from_name(name),
        name: name.to_string(),
        resource: resource.to_string(),
        model_class: model_class.to_string(),
        has_model,
        parent: parent.cloned(),
        permitted: permitted.to_vec(),
    }
}

/// Prepend the body of each applicable `before_action` callback to
/// `body`. A filter applies when its `only:` list contains
/// `action_name` (or it has no `only:` and no `except:` match —
/// i.e. it applies to every action). Multiple applicable filters
/// prepend in declaration order.
///
/// Filters whose target isn't a private method in this controller
/// (e.g. `authenticate_user` inherited from ApplicationController or
/// a concern) are dropped with no inlining — matches the current
/// emit convention of ignoring inherited callbacks, which will
/// change when the concern-resolution pass arrives.
///
/// Target-neutral. Returns a new `Expr`; the input body is untouched.
pub fn resolve_before_actions(
    controller: &Controller,
    action_name: &str,
    body: &Expr,
) -> Expr {
    let applicable: Vec<&Filter> = controller
        .filters()
        .filter(|f| matches!(f.kind, FilterKind::Before))
        .filter(|f| filter_applies(f, action_name))
        .collect();
    if applicable.is_empty() {
        return body.clone();
    }
    // Look up each filter's target in the controller's own private
    // methods (stored as `Action`s after the `PrivateMarker`).
    // Targets that don't resolve (inherited callbacks) are silently
    // dropped.
    let mut prepend: Vec<Expr> = Vec::new();
    for f in applicable {
        if let Some(method) = controller.actions().find(|a| a.name == f.target) {
            prepend.push(method.body.clone());
        }
    }
    if prepend.is_empty() {
        return body.clone();
    }
    match &*body.node {
        ExprNode::Seq { exprs } => {
            prepend.extend(exprs.iter().cloned());
        }
        _ => prepend.push(body.clone()),
    }
    Expr::new(body.span, ExprNode::Seq { exprs: prepend })
}

/// True when `filter` applies to `action_name` given its `only:` /
/// `except:` restrictions. Mirrors Rails' semantics: `only` is a
/// whitelist, `except` is a blacklist, neither means all actions.
fn filter_applies(filter: &Filter, action_name: &str) -> bool {
    if !filter.only.is_empty() {
        return filter.only.iter().any(|s| s.as_str() == action_name);
    }
    if !filter.except.is_empty() {
        return !filter.except.iter().any(|s| s.as_str() == action_name);
    }
    true
}
