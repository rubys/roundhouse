//! A class-side call the program provably cannot answer.
//!
//! `Klass.x`, `self.x` in a class method and `self.class.x` in an
//! instance method name the class object. When dispatch finds `x`
//! nowhere and the class side is closed (`ClassInfo::class_side_closed`),
//! with no class or module table on the way, no subclass that could
//! stand in for `self` and no method every class object answers defining
//! it, the call fails at run time, so it is reported instead of left as
//! an unknown that library-class bodies never report.

use crate::det::HashSet;
use std::sync::LazyLock;

use super::{BodyTyper, Ctx};
use crate::diagnostic::DiagnosticKind;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// What a plain class object answers on CRuby 4.0.5 with ActiveSupport
/// 8.1.4 loaded, public and private (`Class.new`, recorded).
static CLASS_OBJECT_NAMES: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| include_str!("class_object_names.txt").lines().filter(|l| !l.is_empty()).collect());

impl<'a> BodyTyper<'a> {
    /// The dispatch miss for a class-side call nothing answers, or `None`
    /// when something the analyzer cannot see may still answer it.
    pub(super) fn class_side_miss(
        &self,
        recv: Option<&Expr>,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        ctx: &Ctx,
    ) -> Option<DiagnosticKind> {
        let Some(Ty::Class { id, .. }) = recv_ty else { return None };
        // `exact`: the receiver is that very class, so no subclass stands in.
        let (exact, implicit) = match recv.map(|r| (r, &*r.node)) {
            None if ctx.class_side => (false, true),
            Some((_, ExprNode::SelfRef)) if ctx.class_side => (false, false),
            Some((r, ExprNode::Const { .. })) if self.is_class_object(r, ctx) => (true, false),
            Some((_, ExprNode::Send { recv: Some(inner), method: m, args, block: None, .. }))
                if !ctx.class_side
                    && m.as_str() == "class"
                    && args.is_empty()
                    && matches!(&*inner.node, ExprNode::SelfRef) =>
            {
                (false, false)
            }
            _ => return None,
        };
        if implicit || CLASS_OBJECT_NAMES.contains(method.as_str()) {
            return None;
        }
        if !self.classes.get(id).is_some_and(|c| c.class_side_closed) || self.defined_on_chain(id, method) {
            return None;
        }
        if !exact && !self.no_subclass_defines(id, method) {
            return None;
        }
        Some(DiagnosticKind::SendDispatchFailed { method: method.clone(), recv_ty: recv_ty.cloned()? })
    }

    /// Either side of the class, its mixins (transitively) or its superclasses.
    fn defined_on_chain(&self, id: &ClassId, method: &Symbol) -> bool {
        let mut current = Some(id);
        for _ in 0..32 {
            let Some(cid) = current else { return false };
            if self.defined_on_own(cid, method) {
                return true;
            }
            current = self.classes.get(cid).and_then(|c| c.parent.as_ref());
        }
        true
    }

    fn defined_on_own(&self, id: &ClassId, method: &Symbol) -> bool {
        let Some(cls) = self.classes.get(id) else { return true };
        cls.class_methods.contains_key(method)
            || cls.instance_methods.contains_key(method)
            || cls.includes.iter().any(|m| self.lookup_in_module(m, method).is_some())
    }

    /// Every subclass of `id` is closed and none defines `method`.
    fn no_subclass_defines(&self, id: &ClassId, method: &Symbol) -> bool {
        self.classes.iter().filter(|(cid, _)| *cid != id && self.inherits_from(cid, id)).all(|(cid, cls)| {
            cls.class_side_closed && !self.defined_on_own(cid, method)
        })
    }

    fn inherits_from(&self, cid: &ClassId, ancestor: &ClassId) -> bool {
        let mut current = self.classes.get(cid).and_then(|c| c.parent.as_ref());
        for _ in 0..32 {
            let Some(p) = current else { return false };
            if p == ancestor {
                return true;
            }
            current = self.classes.get(p).and_then(|c| c.parent.as_ref());
        }
        false
    }
}
