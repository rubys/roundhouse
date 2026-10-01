//! Admission checks for the serializer methods synthesized from Alba sources.
//! A joined constructor parameter discards unknown observations, so validate
//! each represented constructor argument as well as the converged field reads.

use std::collections::{HashMap, HashSet};

use crate::diagnostic::Diagnostic;
use crate::dialect::{LibraryClass, LibraryClassOrigin, MethodDef, MethodReceiver};
use crate::expr::{BoolOpKind, Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

pub(super) fn diagnose(app: &crate::App) -> Vec<Diagnostic> {
    let resources: HashMap<_, _> = app
        .library_classes
        .iter()
        .filter_map(|class| {
            let Some(LibraryClassOrigin::AlbaResource { declaration_span }) = class.origin else {
                return None;
            };
            Some((class.name.clone(), (class, declaration_span)))
        })
        .collect();
    if resources.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut instantiated = HashSet::new();
    let mut pending = Vec::new();
    let generated_spans: HashSet<_> = resources.values().map(|(_, span)| *span).collect();
    crate::lower::for_each_hook_body_ref(app, &mut |body| {
        // Generated nested calls are reachable only through an instantiated
        // owner. Ingest marks them with the owned declaration span, including
        // after cloning; no allocation-identity or all-resource body scan.
        if generated_spans.contains(&body.span) {
            return;
        }
        constructor_sites(
            body,
            &mut InstanceEvidence::new(app, None),
            &resources,
            &mut instantiated,
            &mut pending,
            &mut out,
        );
    });
    // Views are typed consumer bodies but are not part of the hook walker.
    for view in &app.views {
        constructor_sites(
            &view.body,
            &mut InstanceEvidence::new(app, None),
            &resources,
            &mut instantiated,
            &mut pending,
            &mut out,
        );
    }
    while let Some(id) = pending.pop() {
        for method in &resources[&id].0.methods {
            constructor_sites(
                &method.body,
                &mut InstanceEvidence::new(app, Some(resources[&id].0)),
                &resources,
                &mut instantiated,
                &mut pending,
                &mut out,
            );
        }
    }
    for id in instantiated {
        let (class, span) = resources[&id];
        let object_type = app
            .inferred_method_params
            .get(&(id.clone(), Symbol::from("initialize")))
            .and_then(|params| params.first());
        let serializer = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == "to_h")
            .map(|m| &m.body);
        if !matches!(object_type, Some(Ty::Class { .. }))
            || !serializer.is_some_and(|body| serialized_fields(body, &resources))
        {
            out.push(Diagnostic::unsupported(span, None, "alba_serialization",
                format!("{} requires monomorphic property reads returning Integer/String attributes and one non-null leaf resource; hashes, unknown/missing readers, false/null, collections and polymorphism are outside this contract", id.0)));
        }
    }
    out
}

fn constructor_sites(
    expr: &Expr,
    evidence: &mut InstanceEvidence<'_>,
    resources: &HashMap<ClassId, (&LibraryClass, Span)>,
    instantiated: &mut HashSet<ClassId>,
    pending: &mut Vec<ClassId>,
    out: &mut Vec<Diagnostic>,
) {
    if let ExprNode::Let {
        name, value, body, ..
    } = &*expr.node
    {
        constructor_sites(value, evidence, resources, instantiated, pending, out);
        let instance = evidence.proves(value);
        let old = evidence.locals.insert(name.clone(), instance);
        constructor_sites(body, evidence, resources, instantiated, pending, out);
        if let Some(old) = old {
            evidence.locals.insert(name.clone(), old);
        } else {
            evidence.locals.remove(name);
        }
        return;
    }
    if let ExprNode::Assign {
        target: LValue::Var { name, .. },
        value,
    } = &*expr.node
    {
        constructor_sites(value, evidence, resources, instantiated, pending, out);
        let instance = evidence.proves(value);
        evidence.locals.insert(name.clone(), instance);
        return;
    }
    if let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        block,
        ..
    } = &*expr.node
    {
        if method.as_str() == "new" {
            if let Some(Ty::Class { id, .. }) = &recv.ty {
                if resources.contains_key(id) {
                    if instantiated.insert(id.clone()) {
                        pending.push(id.clone());
                    }
                    let known_object = args.len() == 1
                        && matches!(&args[0].ty,
                        Some(Ty::Class { id, args }) if args.is_empty()
                            && (evidence.app.library_classes.iter().any(|c| &c.name == id)
                                || evidence.app.models.iter().any(|m| &m.name == id)))
                        && evidence.proves(&args[0]);
                    if !known_object || block.is_some() {
                        out.push(Diagnostic::unsupported(expr.span, None, "alba_serialization",
                            format!("{} requires exactly one statically known source object at this constructor site", id.0)));
                    }
                }
            }
        }
    }
    if matches!(&*expr.node, ExprNode::Seq { .. } | ExprNode::Send { .. }) {
        expr.node.for_each_child(&mut |child| {
            constructor_sites(child, evidence, resources, instantiated, pending, out)
        });
    } else {
        // Control flow and closures are not interpreted. Any local they may
        // rebind loses evidence; one branch must not prove another branch.
        fn invalidate(expr: &Expr, locals: &mut HashMap<Symbol, bool>) {
            match &*expr.node {
                ExprNode::Let { name, .. }
                | ExprNode::Assign {
                    target: LValue::Var { name, .. },
                    ..
                }
                | ExprNode::OpAssign {
                    target: LValue::Var { name, .. },
                    ..
                } => {
                    locals.insert(name.clone(), false);
                }
                ExprNode::MultiAssign { targets, .. } => {
                    for target in targets {
                        if let LValue::Var { name, .. } = target {
                            locals.insert(name.clone(), false);
                        }
                    }
                }
                _ => {}
            }
            expr.node
                .for_each_child(&mut |child| invalidate(child, locals));
        }
        invalidate(expr, &mut evidence.locals);
        let initial = evidence.locals.clone();
        expr.node.for_each_child(&mut |child| {
            evidence.locals = initial.clone();
            constructor_sites(child, evidence, resources, instantiated, pending, out);
        });
        evidence.locals = initial;
    }
}

/// Ty::Class also represents class objects. This small admission proof accepts
/// explicit constructions/local aliases, model find, and a readonly source
/// getter backed by initialize. Unproven factories/helper parameters refuse;
/// it is not a general value interpreter or new type-system classification.
struct InstanceEvidence<'a> {
    app: &'a crate::App,
    owner: Option<&'a LibraryClass>,
    locals: HashMap<Symbol, bool>,
    getters: HashSet<(ClassId, Symbol)>,
}

impl<'a> InstanceEvidence<'a> {
    fn new(app: &'a crate::App, owner: Option<&'a LibraryClass>) -> Self {
        Self {
            app,
            owner,
            locals: HashMap::new(),
            getters: HashSet::new(),
        }
    }

    fn unique_class(&self, id: &ClassId) -> Option<&'a LibraryClass> {
        let mut classes = self.app.library_classes.iter().filter(|c| &c.name == id);
        let class = classes.next()?;
        if classes.next().is_some() {
            return None;
        }
        // Reopenings and duplicate defs do not have one unambiguous body.
        // Do not prove a first getter/initializer while Ruby uses a later one.
        let mut methods = HashSet::new();
        class
            .methods
            .iter()
            .all(|method| {
                methods.insert((
                    method.name.clone(),
                    method.receiver == MethodReceiver::Class,
                ))
            })
            .then_some(class)
    }

    fn proves(&mut self, expr: &Expr) -> bool {
        match &*expr.node {
            ExprNode::Var { name, .. } => self.locals.get(name).copied().unwrap_or(false),
            ExprNode::Let {
                name, value, body, ..
            } => {
                let instance = self.proves(value);
                let old = self.locals.insert(name.clone(), instance);
                let result = self.proves(body);
                if let Some(old) = old {
                    self.locals.insert(name.clone(), old);
                } else {
                    self.locals.remove(name);
                }
                result
            }
            // Do not ignore effects before a sequence's tail (e.g. a local
            // reassignment inside a constructor argument).
            ExprNode::Seq { exprs } => exprs.len() == 1 && self.proves(&exprs[0]),
            ExprNode::Return { value } => self.proves(value),
            ExprNode::BoolOp {
                op: BoolOpKind::Or,
                left,
                right,
                ..
            } if matches!(&*right.node, ExprNode::Send { recv: None, method, .. } if method.as_str() == "raise") => {
                self.proves(left)
            }
            ExprNode::Ivar { name } => self.readonly_ivar(name),
            ExprNode::Send {
                recv: Some(recv),
                method,
                args,
                block: None,
                ..
            } => {
                let Some(Ty::Class {
                    id,
                    args: type_args,
                }) = &expr.ty
                else {
                    return false;
                };
                if !type_args.is_empty() {
                    return false;
                }
                if let ExprNode::Const { path } = &*recv.node {
                    if path
                        .iter()
                        .map(Symbol::as_str)
                        .collect::<Vec<_>>()
                        .join("::")
                        != id.0.as_str()
                    {
                        return false;
                    }
                    return match method.as_str() {
                        "new" => {
                            let Some(class) = self.unique_class(id) else {
                                return false;
                            };
                            let initializer = class.methods.iter().find(|m| {
                                m.receiver == MethodReceiver::Instance
                                    && m.name.as_str() == "initialize"
                            });
                            // No inherited/custom constructor semantics are
                            // assumed for source objects. Tagged resources have
                            // their own validated synthesized initializer.
                            (class.parent.is_none()
                                || matches!(
                                    class.origin,
                                    Some(LibraryClassOrigin::AlbaResource { .. })
                                ))
                                && !class.methods.iter().any(|m| {
                                    m.receiver == MethodReceiver::Class && m.name.as_str() == "new"
                                })
                                && initializer.map_or(args.is_empty(), |m| {
                                    supported_initializer(m) && args.len() == m.params.len()
                                })
                                && args.iter().all(binding_free)
                                && args.iter().all(|arg| match &arg.ty {
                                    Some(Ty::Int | Ty::Str) => true,
                                    Some(Ty::Class { .. }) => self.proves(arg),
                                    _ => false,
                                })
                        }
                        "find" => self.app.models.iter().any(|model| &model.name == id),
                        _ => false,
                    };
                }
                if !args.is_empty() || !self.proves(recv) {
                    return false;
                }
                let Some(Ty::Class { id: owner, .. }) = &recv.ty else {
                    return false;
                };
                let Some(class) = self.unique_class(owner) else {
                    return false;
                };
                let Some(getter) = class
                    .methods
                    .iter()
                    .find(|m| m.receiver == MethodReceiver::Instance && &m.name == method)
                else {
                    return false;
                };
                let key = (class.name.clone(), method.clone());
                if self.getters.contains(&key) {
                    return false;
                }
                let mut nested = Self::new(self.app, Some(class));
                nested.getters = self.getters.clone();
                nested.getters.insert(key);
                nested.proves(&getter.body)
            }
            _ => false,
        }
    }

    fn readonly_ivar(&self, name: &Symbol) -> bool {
        fn writes_ivar(expr: &Expr, name: &Symbol) -> bool {
            let target_matches =
                |target: &LValue| matches!(target, LValue::Ivar { name: target } if target == name);
            let mut writes = match &*expr.node {
                ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => {
                    target_matches(target)
                }
                ExprNode::MultiAssign { targets, .. } => targets.iter().any(target_matches),
                _ => false,
            };
            expr.node
                .for_each_child(&mut |child| writes |= writes_ivar(child, name));
            writes
        }
        let Some(class) = self.owner else {
            return false;
        };
        let mut initialized = false;
        for method in &class.methods {
            if !writes_ivar(&method.body, name) {
                continue;
            }
            if method.name.as_str() != "initialize"
                || method.receiver != MethodReceiver::Instance
                || !supported_initializer(method)
            {
                return false;
            }
            // Constructor argument evidence was checked at the represented
            // construction feeding this getter, not inferred from a joined
            // parameter type. Only direct copies of required parameters are
            // admitted; rebinding, defaults, compound writes and later writers
            // cannot masquerade as those copies.
            initialized = true;
        }
        initialized
    }
}

// Even scalar-typed arguments can rebind a later object argument. Refuse those
// effects instead of evaluating argument expressions or retaining stale proof.
fn binding_free(expr: &Expr) -> bool {
    if matches!(
        &*expr.node,
        ExprNode::Let { .. }
            | ExprNode::Assign { .. }
            | ExprNode::OpAssign { .. }
            | ExprNode::MultiAssign { .. }
    ) {
        return false;
    }
    let mut pure = true;
    expr.node
        .for_each_child(&mut |child| pure &= binding_free(child));
    pure
}

fn supported_initializer(method: &MethodDef) -> bool {
    fn copies_parameters(expr: &Expr, method: &MethodDef) -> bool {
        match &*expr.node {
            ExprNode::Seq { exprs } => exprs.iter().all(|e| copies_parameters(e, method)),
            ExprNode::Assign {
                target: LValue::Ivar { .. },
                value,
            } => {
                matches!(&*value.node, ExprNode::Var { name, .. } if method.params.iter().any(|p| &p.name == name))
            }
            _ => false,
        }
    }
    method.block_param.is_none()
        && method
            .params
            .iter()
            .all(|p| p.ty_kind() == crate::ty::ParamKind::Required)
        && copies_parameters(&method.body, method)
}

fn serialized_fields(expr: &Expr, resources: &HashMap<ClassId, (&LibraryClass, Span)>) -> bool {
    // Inspect the generated entries, not just their joined Hash return type:
    // an ordinary attribute returning a Hash must not masquerade as `one`.
    match &*expr.node {
        ExprNode::Let { body, .. } | ExprNode::Return { value: body } => {
            serialized_fields(body, resources)
        }
        ExprNode::Seq { exprs } => exprs
            .last()
            .is_some_and(|e| serialized_fields(e, resources)),
        ExprNode::Hash { entries, .. } => entries.iter().all(|(_, value)| {
            if let ExprNode::Send {
                recv: Some(recv),
                method,
                ..
            } = &*value.node
            {
                if method.as_str() == "to_h"
                    && matches!(&recv.ty, Some(Ty::Class { id, .. }) if resources.contains_key(id))
                {
                    // Its argument is checked per-site; its own field reads
                    // are checked by the reachable-resource worklist.
                    return matches!(&value.ty, Some(Ty::Hash { key, .. }) if **key == Ty::Str);
                }
            }
            matches!(value.ty, Some(Ty::Int | Ty::Str))
        }),
        _ => false,
    }
}
