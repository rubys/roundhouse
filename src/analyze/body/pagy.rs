//! The pagy gem's `pagy(scope[, limit: n])`, typed at the call.
//!
//! `@pagy, @records = pagy(scope)` answers `[Pagy, <scope>]`: the second
//! element is the scope's own type, which a registry entry cannot say (it
//! maps a name to one type, with no way to name "argument 0's"). So the
//! call is typed here, from the argument, and only for the shape the
//! runtime (runtime/ruby/action_controller/pagy.rb) models:
//!
//! * one ActiveRecord scope, typed as a Relation, or one the analysis
//!   cannot type at all (gradual) -- not a known Array (`Model.to_a`, a
//!   literal), which the gem paginates only through `pagy(:offset, array)`
//!   and which has no `offset` / `limit` for this runtime to call;
//! * optionally one trailing hash holding only `limit:`, a literal
//!   Integer of at least 1 (a computed limit may be 0, which the gem
//!   rejects with `Pagy::OptionError` and the app turns into a 404 in a
//!   `rescue_from` no target replays; a literal cannot reach that), and
//!   `items:`, which pagy 43 does not read at all (it was renamed
//!   `limit:`), so the page size stays 20. The runtime ignores it the way
//!   the gem does, and a warning says so: EngineeredAt's Madmin lists write
//!   `items: 50` and get 20.
//!
//! Anything else -- a paginator symbol (`pagy(:countless, ...)`), another
//! option (`page:`, `count:`, `max_limit:`, `jsonapi:`, ...), a splat --
//! is refused with a diagnostic that names it, and the call is still
//! typed as if it ran so the refusal is the one error.

use super::*;

impl<'a> BodyTyper<'a> {
    /// What `pagy` hands back as the records: the scope's own type when it
    /// is a Relation (never an Array: the runtime calls `offset` / `limit`
    /// on the scope, which an Array lacks), the
    /// unique such type across the closed members of a union, and
    /// `Untyped` when the analysis could not type the scope at all (a base
    /// class it does not model, a method whose other return paths are
    /// open) -- gradual, as everywhere else. `None` for a known type that
    /// is not a scope, or a union of different scopes.
    fn pagy_scope_ty(&self, ty: Option<&Ty>) -> Option<Ty> {
        match ty {
            None | Some(Ty::Untyped) | Some(Ty::Var { .. }) => Some(Ty::Untyped),
            Some(Ty::Relation { of }) => Some(Ty::Relation { of: of.clone() }),
            Some(Ty::Array { .. }) => None,
            Some(Ty::Union { variants }) => {
                let mut closed: Option<Ty> = None;
                for variant in variants {
                    match self.pagy_scope_ty(Some(variant))? {
                        Ty::Untyped => {}
                        known => match &closed {
                            Some(prev) if *prev != known => return None,
                            _ => closed = Some(known),
                        },
                    }
                }
                Some(closed.unwrap_or(Ty::Untyped))
            }
            _ => None,
        }
    }

    /// `@pagy.series_nav(id: "x")` and the like: the object's helpers take
    /// no arguments here, and an argument the runtime would ignore is an
    /// approximation, so it is refused by name instead.
    pub(super) fn pagy_object_refusal(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        args: &[Expr],
    ) -> Option<crate::diagnostic::DiagnosticKind> {
        let Some(Ty::Class { id, .. }) = recv_ty else { return None };
        if id.0.as_str() != "Pagy" || args.is_empty() {
            return None;
        }
        let known = self.classes().get(id).is_some_and(|c| c.instance_methods.contains_key(method));
        // An unknown method is already "no known method on Pagy".
        known.then(|| crate::diagnostic::DiagnosticKind::Unsupported {
            target: None,
            construct: Symbol::from("pagy"),
            detail: format!(
                "`Pagy#{}` takes no arguments here (`id:`, `aria_label:`, `item_name:`, a style, ... are not modeled)",
                method.as_str()
            ),
        })
    }

    /// `Some(type)` when this implicit-self `pagy` call is the gem's.
    /// `None` leaves ordinary dispatch to it (not a controller, or the app
    /// does not lock the gem).
    pub(super) fn pagy_call_ty(
        &self,
        args: &[Expr],
        ctx: &Ctx,
        refusal: &mut Option<crate::diagnostic::DiagnosticKind>,
    ) -> Option<Ty> {
        let pagy_id = ClassId(Symbol::from("Pagy"));
        self.classes().get(&pagy_id)?;
        let Some(Ty::Class { id: self_id, .. }) = &ctx.self_ty else { return None };
        // Only inside a controller: some class up the chain carries the
        // registry's `pagy` (registry/controllers). A `pagy` the app
        // defines on the way is a wrapper around the gem's -- ingest
        // (`ingest::pagy`) refuses any that does not hand back what
        // `super` returned -- so the call is typed the same.
        let mut current = Some(self_id);
        let mut in_controller = false;
        for _ in 0..32 {
            let Some(cid) = current else { break };
            let Some(cls) = self.classes().get(cid) else { break };
            if cls.class_methods.contains_key(&Symbol::from("pagy")) {
                in_controller = true;
                break;
            }
            current = cls.parent.as_ref();
        }
        if !in_controller {
            return None;
        }

        let refuse = |detail: String, refusal: &mut Option<crate::diagnostic::DiagnosticKind>| {
            *refusal = Some(crate::diagnostic::DiagnosticKind::Unsupported {
                target: None,
                construct: Symbol::from("pagy"),
                detail,
            });
        };
        let scope_ty = args.first().and_then(|a| a.ty.as_ref());
        let records = self.pagy_scope_ty(scope_ty);
        let result = |records: Ty| Ty::Tuple {
            elems: vec![Ty::Class { id: pagy_id.clone(), args: vec![].into() }, records].into(),
        };
        match args {
            [] => {
                refuse("`pagy` needs a collection".into(), refusal);
                return Some(result(Ty::Untyped));
            }
            [first, ..] if matches!(&*first.node, ExprNode::Lit { value: crate::expr::Literal::Sym { .. } }) => {
                refuse(
                    "an explicit paginator argument (`pagy(:offset, ...)`, `:countless`, `:keyset`, `:calendar`, ...) is not modeled; \
                     call `pagy(scope)` for the default offset paginator"
                        .into(),
                    refusal,
                );
                return Some(result(Ty::Untyped));
            }
            _ => {}
        }
        if records.is_none() {
            refuse(
                format!(
                    "the collection must be an ActiveRecord scope the analysis can type (got {}); \
                     paginating an Array or an untyped value is not modeled",
                    scope_ty.map_or("an untyped value".to_string(), |t| format!("{t:?}")),
                ),
                refusal,
            );
        }
        let mut ignored_items = false;
        match &args[1..] {
            [] => {}
            [opts] => match &*opts.node {
                ExprNode::Hash { entries, .. } => {
                    for (k, v) in entries {
                        let key = match &*k.node {
                            ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => value.as_str().to_string(),
                            _ => {
                                refuse("only literal `limit:` is modeled; this option key is not a symbol".into(), refusal);
                                break;
                            }
                        };
                        if key == "items" {
                            ignored_items = true;
                            continue;
                        }
                        if key != "limit" {
                            refuse(
                                format!("option `{key}:` is not modeled (only a literal `limit:` is)"),
                                refusal,
                            );
                            break;
                        }
                        match &*v.node {
                            ExprNode::Lit { value: crate::expr::Literal::Int { value } } if *value >= 1 => {}
                            _ => {
                                refuse(
                                    "`limit:` must be a literal Integer of at least 1 (the gem raises Pagy::OptionError otherwise)"
                                        .into(),
                                    refusal,
                                );
                                break;
                            }
                        }
                    }
                }
                _ => refuse("the options must be a literal hash of `limit:`".into(), refusal),
            },
            _ => refuse("expected a collection and at most one options hash".into(), refusal),
        }
        if ignored_items && refusal.is_none() {
            *refusal = Some(crate::diagnostic::DiagnosticKind::LowerResidue {
                pass: Symbol::from("pagy"),
                construct: Symbol::from("pagy(..., items:)"),
                reason: Symbol::from(
                    "pagy 43 does not read `items:` (it is `limit:` now), so the page size stays 20",
                ),
            });
        }
        Some(result(records.unwrap_or(Ty::Untyped)))
    }
}
