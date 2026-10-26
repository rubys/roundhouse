//! Exception owners are class/module values, not implementations of Rails'
//! image-processing or cloud-service constructor contracts.

use std::collections::HashMap;

use crate::expr::{Expr, ExprNode, Literal, Pattern};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

use super::{BodyTyper, Ctx};

/// Kernel and ActiveSupport methods that call, or capture, the method
/// their first argument names.
const REFLECTION: &[&str] = &[
    "send",
    "public_send",
    "__send__",
    "try",
    "try!",
    "method",
    "public_method",
    "singleton_method",
];

/// Iterators whose block's first parameter is each element in turn.
const ELEMENT_ITERATORS: &[&str] = &[
    "each",
    "each_with_index",
    "each_with_object",
    "map",
    "collect",
    "flat_map",
    "filter_map",
    "select",
    "filter",
    "reject",
    "find",
    "detect",
    "any?",
    "all?",
    "none?",
    "count",
    "sum",
    "to_h",
    "index_by",
    "index_with",
    "group_by",
    "partition",
    "sort_by",
    "min_by",
    "max_by",
];

/// How often the outermost expression being typed assigns each local, and
/// whether it can reach its locals dynamically. A local assigned once keeps
/// the value it was assigned from then on. Block parameters are fresh
/// block-local variables, so they are not counted: the Lambda arm drops
/// what an outer local of the same name proved.
#[derive(Default)]
pub(super) struct ScopeBindings {
    counts: HashMap<Symbol, u32>,
    dynamic: bool,
}

impl ScopeBindings {
    pub(super) fn of(expr: &Expr) -> Self {
        let mut out = Self::default();
        out.visit(expr);
        out
    }

    fn visit(&mut self, expr: &Expr) {
        let mut names = Vec::new();
        if !matches!(&*expr.node, ExprNode::Lambda { .. }) {
            super::rebound_names(expr, &mut names);
        }
        match &*expr.node {
            ExprNode::Case { arms, .. } => {
                for arm in arms {
                    pattern_names(&arm.pattern, &mut names);
                }
            }
            // `binding` and string evaluation can rebind any local.
            ExprNode::Send { method, args, .. } => {
                self.dynamic |= match method.as_str() {
                    "binding" | "eval" | "local_variable_set" => true,
                    "instance_eval" | "class_eval" | "module_eval" => !args.is_empty(),
                    _ => false,
                };
            }
            _ => {}
        }
        for name in names {
            *self.counts.entry(name).or_default() += 1;
        }
        expr.node.for_each_child(&mut |child| self.visit(child));
    }
}

fn pattern_names(pattern: &Pattern, out: &mut Vec<Symbol>) {
    match pattern {
        Pattern::Bind { name } => out.push(name.clone()),
        Pattern::Array { elems, rest } => {
            out.extend(rest.iter().cloned());
            for elem in elems {
                pattern_names(elem, out);
            }
        }
        Pattern::Record { fields, .. } => {
            for (_, field) in fields {
                pattern_names(field, out);
            }
        }
        Pattern::Wildcard | Pattern::Lit { .. } | Pattern::Expr { .. } => {}
    }
}

/// Only `new` and the reflection methods can reach a constructor.
fn relevant(name: &str) -> Option<Symbol> {
    (name == "new" || REFLECTION.contains(&name)).then(|| Symbol::from(name))
}

// A `#` never starts a Ruby local, and these keys hold no `.` (a narrowed
// member read), so they ride in `local_bindings` like `#params-local:`.
fn selector_key(local: &Symbol, name: Option<&str>) -> Symbol {
    Symbol::from(match name {
        Some(name) => format!("#selector:{}:{name}", local.as_str()),
        None => format!("#selector:{}", local.as_str()),
    })
}

/// The relevant names a proven local can hold; `None` when it is unproven.
fn selector_proof(ctx: &Ctx, local: &Symbol) -> Option<Vec<Symbol>> {
    if !ctx.local_bindings.contains_key(&selector_key(local, None)) {
        return None;
    }
    Some(
        std::iter::once("new")
            .chain(REFLECTION.iter().copied())
            .filter(|name| {
                ctx.local_bindings
                    .contains_key(&selector_key(local, Some(name)))
            })
            .map(Symbol::from)
            .collect(),
    )
}

/// A block parameter shadows an outer local of the same name.
pub(super) fn forget_selector_proof(ctx: &mut Ctx, local: &Symbol) {
    if ctx
        .local_bindings
        .remove(&selector_key(local, None))
        .is_some()
    {
        for name in std::iter::once("new").chain(REFLECTION.iter().copied()) {
            ctx.local_bindings.remove(&selector_key(local, Some(name)));
        }
    }
}

pub(super) fn insert_selector_proof(ctx: &mut Ctx, local: &Symbol, names: &[Symbol]) {
    ctx.local_bindings
        .insert(selector_key(local, None), Ty::Sym);
    for name in names {
        ctx.local_bindings
            .insert(selector_key(local, Some(name.as_str())), Ty::Sym);
    }
}

impl BodyTyper<'_> {
    pub(super) fn unsupported_active_storage_constructor(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        args: &[Expr],
        ctx: &Ctx,
    ) -> Option<&'static str> {
        let reflective = REFLECTION.contains(&method.as_str());
        if !reflective && method.as_str() != "new" {
            return None;
        }
        let recv_ty = recv_ty?;
        if let Some(of) = crate::analyze::singleton_of(Some(recv_ty)) {
            return self.unsupported_active_storage_constructor(Some(of), method, args, ctx);
        }
        if let Ty::Union { variants } = recv_ty {
            return variants.iter().find_map(|ty| {
                self.unsupported_active_storage_constructor(Some(ty), method, args, ctx)
            });
        }
        // Reflective calls and captured constructor methods reach the same
        // missing API.
        if reflective {
            let (first, rest) = args.split_first()?;
            // A source class method of that name answers instead, while the
            // emitted call still reaches it: `try` lowers to a direct call
            // (`lower::try_guard`), as does a computed name the send lowering
            // can enumerate (`lower::send_dispatch`), so only a literal name
            // keeps the source method. One that calls `super` does not count.
            if !matches!(method.as_str(), "try" | "try!")
                && matches!(
                    &*first.node,
                    ExprNode::Lit {
                        value: Literal::Sym { .. } | Literal::Str { .. }
                    }
                )
                && self.source_class_side_method(recv_ty, method)
            {
                return None;
            }
            // An opaque name cannot rule out `new`.
            let Some(names) = self.selector_names(first, ctx, true) else {
                return self.unsupported_storage_owner(recv_ty);
            };
            return names.iter().find_map(|name| {
                self.unsupported_active_storage_constructor(Some(recv_ty), name, rest, ctx)
            });
        }
        if method.as_str() != "new" {
            return None;
        }
        self.unsupported_storage_owner(recv_ty)
    }

    /// The relevant method names a selector can evaluate to, or `None`
    /// when it is opaque. A string selector counts only while nothing
    /// else holds it: through a local or an assignment, a later argument
    /// can mutate it, so those carry symbols only.
    fn selector_names(&self, expr: &Expr, ctx: &Ctx, strings: bool) -> Option<Vec<Symbol>> {
        let union = |exprs: &mut dyn Iterator<Item = &Expr>| {
            let mut names = Vec::new();
            for expr in exprs {
                names.extend(self.selector_names(expr, ctx, strings)?);
            }
            Some(names)
        };
        match &*expr.node {
            ExprNode::Lit {
                value: Literal::Sym { value },
            } => Some(relevant(value.as_str()).into_iter().collect()),
            ExprNode::Lit {
                value: Literal::Str { value },
            } => strings.then(|| relevant(value.as_str()).into_iter().collect()),
            // Not a method name: Ruby raises TypeError instead.
            ExprNode::Lit { .. } => Some(Vec::new()),
            ExprNode::Var { name, .. } => selector_proof(ctx, name),
            ExprNode::Assign { value, .. } => self.selector_names(value, ctx, false),
            ExprNode::Cast { value, .. } => self.selector_names(value, ctx, strings),
            ExprNode::Seq { exprs } => self.selector_names(exprs.last()?, ctx, strings),
            ExprNode::If {
                then_branch,
                else_branch,
                ..
            } => union(&mut [then_branch, else_branch].into_iter()),
            ExprNode::BoolOp { left, right, .. } => union(&mut [left, right].into_iter()),
            ExprNode::RescueModifier { expr, fallback } => union(&mut [expr, fallback].into_iter()),
            ExprNode::Case { arms, .. } => union(&mut arms.iter().map(|arm| &arm.body)),
            _ => None,
        }
    }

    fn assigned(&self, local: &Symbol, times: u32) -> bool {
        self.scope_bindings.borrow().as_ref().is_some_and(|scope| {
            !scope.dynamic && scope.counts.get(local).copied().unwrap_or(0) == times
        })
    }

    /// A statement that is the only assignment of a local, to a symbol,
    /// fixes the local for every statement after it.
    pub(super) fn prove_selector(&self, local: &Symbol, value: &Expr, ctx: &mut Ctx) {
        if !self.assigned(local, 1) {
            return;
        }
        if let Some(names) = self.selector_names(value, ctx, false) {
            insert_selector_proof(ctx, local, &names);
        }
    }

    /// `%i[a b].each { |name| … }` binds `name` to one of the literal's
    /// symbols on every call, when nothing assigns it.
    pub(super) fn iterated_selectors(
        &self,
        recv: Option<&Expr>,
        method: &Symbol,
        block: &Expr,
    ) -> Option<(Symbol, Vec<Symbol>)> {
        if !ELEMENT_ITERATORS.contains(&method.as_str()) {
            return None;
        }
        let ExprNode::Array { elements, .. } = &*recv?.node else {
            return None;
        };
        let ExprNode::Lambda { params, .. } = &*block.node else {
            return None;
        };
        let param = params.first().filter(|param| self.assigned(param, 0))?;
        let mut names = Vec::new();
        for element in elements {
            let ExprNode::Lit {
                value: Literal::Sym { value },
            } = &*element.node
            else {
                return None;
            };
            names.extend(relevant(value.as_str()));
        }
        Some((param.clone(), names))
    }

    /// A source `def self.<method>` on the class or an ancestor answers a
    /// class-side call, unless it delegates through `super`.
    fn source_class_side_method(&self, ty: &Ty, method: &Symbol) -> bool {
        let Ty::Class { id, .. } = ty else {
            return false;
        };
        let mut current = Some(id);
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = current {
            if !seen.insert(id) {
                break;
            }
            let Some(class) = self.classes().get(id) else {
                break;
            };
            if class.source_class_methods.contains(method)
                && !class.delegating_class_methods.contains(method)
            {
                return true;
            }
            current = class.parent.as_ref();
        }
        false
    }

    pub(super) fn derived_storage_class_value(
        &self,
        recv_ty: Option<&Ty>,
        method: &Symbol,
        args: &[Expr],
    ) -> Option<Ty> {
        if !matches!(
            method.as_str(),
            "new"
                | "superclass"
                | "ancestors"
                | "subclasses"
                | "method"
                | "public_method"
                | "singleton_method"
                | "freeze"
                | "itself"
                | "dup"
                | "clone"
        ) {
            return None;
        }
        let recv_ty = recv_ty?;
        let recv_ty = crate::analyze::singleton_of(Some(recv_ty)).unwrap_or(recv_ty);
        let Ty::Class { id, .. } = recv_ty else {
            return None;
        };
        if method.as_str() == "new" && id.0.as_str() != "Class" {
            return None;
        }
        if super::ClassInfo::source_class_method(self.classes(), id, method) {
            return None;
        }
        let singleton = |of| Ty::Class {
            id: ClassId(Symbol::from("Class")),
            args: vec![of].into(),
        };
        if id.0.as_str() == "Class" && method.as_str() == "new" && args.len() == 1 {
            if self
                .const_resolver
                .as_ref()
                .is_some_and(|r| r.has_source_namespace("Class"))
            {
                return None;
            }
            let parent = args[0].ty.as_ref()?;
            let parent = crate::analyze::singleton_of(Some(parent)).unwrap_or(parent);
            let has_storage_parent = match parent {
                Ty::Union { variants } => {
                    variants.iter().any(|ty| self.storage_lineage(ty).is_some())
                }
                ty => self.storage_lineage(ty).is_some(),
            };
            if !has_storage_parent {
                return None;
            }
            // An anonymous subclass is at least an instance of this nominal
            // class. Keep that upper bound through assignments and parameters.
            return Some(singleton(parent.clone()));
        }
        let lineage = self.storage_lineage(recv_ty)?;
        if matches!(
            method.as_str(),
            "method" | "public_method" | "singleton_method"
        ) {
            let [name] = args else { return None };
            let name = match &*name.node {
                ExprNode::Lit {
                    value: Literal::Sym { value },
                } => value.clone(),
                ExprNode::Lit {
                    value: Literal::Str { value },
                } => Symbol::from(value.as_str()),
                _ => return None,
            };
            let ret = self.derived_storage_class_value(Some(recv_ty), &name, &[])?;
            return Some(Ty::Fn {
                params: vec![].into(),
                block: None,
                ret: std::sync::Arc::new(ret),
                effects: crate::effect::EffectSet::default(),
            });
        }
        if !args.is_empty() {
            return None;
        }
        match method.as_str() {
            "superclass" => {
                let parent = self.classes().get(id)?.parent.as_ref()?;
                let parent = Ty::Class {
                    id: parent.clone(),
                    args: vec![].into(),
                };
                self.storage_lineage(&parent)?;
                Some(singleton(parent))
            }
            "ancestors" => {
                let mut variants = lineage.into_iter().map(singleton).collect::<Vec<_>>();
                // The registry does not enumerate Object/Kernel or every
                // runtime mixin. This is an element union, not an exact list.
                variants.push(Ty::Class {
                    id: ClassId(Symbol::from("Class")),
                    args: vec![].into(),
                });
                Some(Ty::Array {
                    elem: std::sync::Arc::new(Ty::Union {
                        variants: variants.into(),
                    }),
                })
            }
            "subclasses" if !self.classes().get(id)?.is_module => Some(Ty::Array {
                elem: std::sync::Arc::new(singleton(recv_ty.clone())),
            }),
            "freeze" | "itself" | "dup" | "clone" => Some(singleton(recv_ty.clone())),
            _ => None,
        }
    }

    fn storage_lineage(&self, mut ty: &Ty) -> Option<Vec<Ty>> {
        while let Some(of) = crate::analyze::singleton_of(Some(ty)) {
            ty = of;
        }
        let Ty::Class { id, .. } = ty else {
            return None;
        };
        let mut current = Some(id);
        let mut seen = std::collections::HashSet::new();
        let mut lineage = Vec::new();
        let mut storage_owner = false;
        while let Some(id) = current {
            if !seen.insert(id) {
                break;
            }
            storage_owner |= storage_owner_name(id).is_some();
            lineage.push(Ty::Class {
                id: id.clone(),
                args: vec![].into(),
            });
            current = self.classes().get(id).and_then(|c| c.parent.as_ref());
        }
        storage_owner.then_some(lineage)
    }

    fn unsupported_storage_owner(&self, ty: &Ty) -> Option<&'static str> {
        if let Some(of) = crate::analyze::singleton_of(Some(ty)) {
            return self.unsupported_storage_owner(of);
        }
        if let Ty::Union { variants } = ty {
            return variants
                .iter()
                .find_map(|ty| self.unsupported_storage_owner(ty));
        }
        let Ty::Class { id, .. } = ty else {
            return None;
        };
        let mut current = Some(id);
        let mut seen = std::collections::HashSet::new();
        let mut unsupported = None;
        while let Some(id) = current {
            if !seen.insert(id) {
                break;
            }
            let class = self.classes().get(id);
            // Reopening a namespace alone is insufficient. A real source
            // constructor (own, inherited or mixed in) supplies the contract.
            if class.is_some_and(|c| {
                c.source_class_methods.contains(&Symbol::from("new")) && !c.constructor_calls_super
            }) {
                return None;
            }
            if id.0.as_str() != "ActiveStorage::Transformers"
                && self.source_storage_initializer(id, &mut std::collections::HashSet::new())
            {
                return None;
            }
            unsupported = unsupported.or_else(|| storage_owner_name(id));
            current = class.and_then(|c| c.parent.as_ref());
        }
        unsupported
    }

    fn source_storage_initializer<'a>(
        &'a self,
        id: &'a ClassId,
        seen: &mut std::collections::HashSet<&'a ClassId>,
    ) -> bool {
        if !seen.insert(id) {
            return false;
        }
        self.classes().get(id).is_some_and(|c| {
            c.implements_initializer
                || c.includes
                    .iter()
                    .any(|m| self.source_storage_initializer(m, seen))
        })
    }
}

fn storage_owner_name(id: &ClassId) -> Option<&'static str> {
    match id.0.as_str() {
        "ActiveStorage::Transformers" => Some("ActiveStorage::Transformers"),
        "ActiveStorage::Transformers::Transformer" => {
            Some("ActiveStorage::Transformers::Transformer")
        }
        "ActiveStorage::Transformers::ImageProcessingTransformer" => {
            Some("ActiveStorage::Transformers::ImageProcessingTransformer")
        }
        "ActiveStorage::Service::GCSService" => Some("ActiveStorage::Service::GCSService"),
        _ => None,
    }
}
