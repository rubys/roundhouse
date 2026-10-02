//! A native `...` is sound only when its destination still has its source
//! argument contract. Library optional keywords/keyword-rest are sometimes
//! flattened; model named rest/block declarations are not fully retained.
//! Do not silently forward into those approximations or an unknown callee.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{Association, LibraryClass, MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

pub(super) fn diagnose(app: &App) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let contracts = SourceContractIndex::new(app);
    let scoped = if app
        .models
        .iter()
        .any(|m| m.methods().any(|m| m.params.iter().any(|p| p.forwarding)))
    {
        let scopes = crate::lower::scope_chain::build_scope_registry(&app.models);
        let assocs = crate::lower::scope_chain::build_assoc_registry(&app.models);
        crate::lower::scope_chain::survey_assoc_class_methods(app, &assocs, &scopes).1
    } else {
        vec![]
    };
    for (owner, method) in methods(app) {
        if let Some(formal) = method.unsupported_formals {
            out.push(Diagnostic::unsupported(
                method.name_span,
                None,
                "parameter declaration",
                formal.description(),
            ));
        }
        if method.params.iter().any(|p| p.forwarding) {
            // Ingest applies these rewrites regardless of receiver ownership,
            // including user-defined methods. Refuse the declaration itself:
            // surveying the rewritten calls cannot prove their source dispatch.
            // Supporting these selectors needs separate dispatch provenance.
            let reason = if matches!(method.name.as_str(), "reverse_merge" | "exists?") {
                Some(
                    "full declarations of this method overlap an ingest-time call rewrite whose source dispatch is not retained",
                )
            } else if !contracts.verified_hierarchy(owner, &mut HashSet::new()) {
                Some("forwarding declarations in reopened class fragments are not verified")
            } else if method.receiver == MethodReceiver::Class
                && app.models.iter().any(|m| &m.name == owner)
                && (crate::lower::scope_chain::mentions_bare_chain_start(&method.body)
                    || scoped
                        .iter()
                        .any(|d| &d.model == owner && d.method == method.name))
            {
                Some("full forwarding cannot use the relation-threading argument ABI")
            } else if contracts.unretained.contains(&method.name_span) {
                Some("model method synthesis does not preserve this source declaration")
            } else {
                None
            };
            if let Some(reason) = reason {
                out.push(Diagnostic::unsupported(
                    method.name_span,
                    None,
                    "full argument forwarding",
                    reason,
                ));
            }
        }
        walk(app, &contracts, owner, method, &method.body, &mut out);
        for default in method.params.iter().filter_map(|p| p.default.as_ref()) {
            walk(app, &contracts, owner, method, default, &mut out);
        }
    }
    for (span, policy) in keyword_calls_with_index(app, &contracts) {
        if matches!(policy, KeywordPolicy::Refuse | KeywordPolicy::RefuseOrdinarySuper) {
            out.push(keyword_refusal(span, policy));
        }
    }
    out
}

fn classes(app: &App) -> impl Iterator<Item = &LibraryClass> {
    app.library_classes
        .iter()
        .chain(app.rails_application.iter())
        .chain(app.test_modules.iter().flat_map(|t| t.inner_classes.iter()))
}

pub(crate) fn methods(app: &App) -> impl Iterator<Item = (&ClassId, &MethodDef)> {
    classes(app)
        .flat_map(|c| c.methods.iter().map(move |m| (&c.name, m)))
        .chain(app.models.iter().flat_map(|c| {
            c.methods()
                .chain(c.associations().flat_map(|a| match a {
                    Association::HasMany { extension, .. } => extension.as_slice().iter(),
                    _ => [].iter(),
                }))
                .map(move |m| (&c.name, m))
        }))
        .chain(
            app.test_modules
                .iter()
                .flat_map(|t| t.helpers.iter().map(move |m| (&t.name, m))),
        )
}

fn walk(
    app: &App,
    contracts: &SourceContractIndex<'_>,
    owner: &ClassId,
    enclosing: &MethodDef,
    e: &Expr,
    out: &mut Vec<Diagnostic>,
) {
    let forwards = match &*e.node {
        ExprNode::Send { args, .. } => has_forwarding(args),
        ExprNode::Super { args } => args
            .as_ref()
            .map_or(enclosing.params.iter().any(|p| p.forwarding), |a| {
                has_forwarding(a)
            }),
        _ => false,
    };
    if forwards {
        let reason = if enclosing.unsupported_formals.is_some() {
            Some("forwarding source has an unrepresented parameter declaration")
        } else if !enclosing.params.iter().any(|p| p.forwarding) {
            Some("forwarding call has no preserved forwarding declaration")
        } else {
            contract_error(
                Some((owner, enclosing)),
                e,
                destination(app, contracts, Some((owner, enclosing)), e),
                contracts,
            )
        };
        if let Some(reason) = reason {
            out.push(Diagnostic::unsupported(
                e.span,
                None,
                "full argument forwarding",
                reason,
            ));
        }
    }
    e.node
        .for_each_child(&mut |c| walk(app, contracts, owner, enclosing, c, out));
}

fn destination<'a>(
    app: &'a App,
    contracts: &SourceContractIndex<'a>,
    context: Option<(&ClassId, &MethodDef)>,
    e: &Expr,
) -> Option<(&'a MethodDef, bool)> {
    let owner_dependent = match &*e.node {
        ExprNode::Send { recv: None, .. } | ExprNode::Super { .. } => true,
        ExprNode::Send { recv: Some(r), .. } => matches!(&*r.node, ExprNode::SelfRef),
        _ => false,
    };
    if owner_dependent
        && context.is_some_and(|(owner, method)| association_method(app, owner, method))
    {
        return None;
    }
    match &*e.node {
        ExprNode::Send { recv, method, .. } => {
            let receiver = match recv.as_ref().map(|r| &*r.node) {
                None | Some(ExprNode::SelfRef) => context?.1.receiver,
                Some(ExprNode::Const { .. }) => MethodReceiver::Class,
                _ => MethodReceiver::Instance,
            };
            let class = match recv {
                None => context.map(|(owner, _)| owner),
                Some(r) if matches!(&*r.node, ExprNode::SelfRef) => context.map(|(owner, _)| owner),
                Some(r) => match (&*r.node, r.ty.as_ref()) {
                    (ExprNode::Const { .. }, Some(Ty::Class { id, .. })) => {
                        // Constructor monomorphization can retain the base's
                        // inferred type on a newly named subclass receiver.
                        // Do not admit the base contract for that source name.
                        constant_names_class(r, id).then_some(id)
                    }
                    (_, Some(Ty::Class { id, .. })) => Some(id),
                    _ => None,
                },
            };
            class.and_then(|c| {
                // Ty::Class conflates class objects with instances. Do
                // not trust an instance or a copied constructor-result type
                // without matching source-side receiver provenance.
                let ambiguous = recv.as_ref().is_some_and(|r| {
                    !matches!(&*r.node, ExprNode::Const { .. } | ExprNode::SelfRef)
                });
                if ambiguous
                    && !constructed_instance(
                        contracts,
                        context.map(|(_, m)| m),
                        recv.as_ref().unwrap(),
                        c,
                    )
                {
                    return None;
                }
                // A source-defined class .new wins over constructor
                // dispatch; an instance method called new is ordinary.
                contracts.effective_call(c, method, receiver)
            })
        }
        ExprNode::Super { .. } => {
            let (owner, enclosing) = context?;
            contracts.ancestors(
                owner,
                &enclosing.name,
                enclosing.receiver,
                &mut HashSet::new(),
            )
        }
        _ => None,
    }
}

fn association_method(app: &App, owner: &ClassId, method: &MethodDef) -> bool {
    app.models.iter().filter(|m| &m.name == owner).any(|m| {
        m.associations().any(|a| match a {
            Association::HasMany { extension, .. } => {
                extension.iter().any(|e| std::ptr::eq(e, method))
            }
            _ => false,
        })
    })
}

fn declaration_error((method, model): (&MethodDef, bool)) -> Option<&'static str> {
    if method.unsupported_formals.is_some() {
        Some("forwarding destination has an unrepresented parameter declaration")
    } else if method.has_anonymous_block {
        Some("forwarding destination's anonymous block binding is not preserved hygienically")
    } else if method
        .params
        .iter()
        .any(|p| p.from_keyword || p.from_kwrest)
    {
        Some("forwarding destination has flattened keyword parameters")
    } else if model && !method.params.iter().any(|p| p.forwarding) {
        Some(
            "forwarding into a model callee with incomplete rest/block declaration retention is not supported yet",
        )
    } else {
        None
    }
}

fn contract_error(
    context: Option<(&ClassId, &MethodDef)>,
    call: &Expr,
    resolved: Option<(&MethodDef, bool)>,
    contracts: &SourceContractIndex<'_>,
) -> Option<&'static str> {
    let Some(resolved) = resolved else {
        return Some("forwarding destination's declaration cannot be verified");
    };
    if let Some(error) = declaration_error(resolved) {
        return Some(error);
    }
    if contracts.unretained.contains(&resolved.0.name_span) {
        return Some("model method synthesis does not preserve this source declaration");
    }
    // A self-send in a base method dispatches on the actual subclass.
    // Verify reachable overrides too, not merely the lexical base's method.
    let candidates = match virtual_destinations(contracts, context, call) {
        Ok(candidates) => candidates,
        Err(reason) => return Some(reason),
    };
    for candidate in candidates {
        if declaration_error(candidate).is_some()
            || contracts.unretained.contains(&candidate.0.name_span)
        {
            return Some("forwarding self-dispatch may reach an unpreserved subclass contract");
        }
    }
    None
}

fn virtual_destinations<'a>(
    contracts: &SourceContractIndex<'a>,
    context: Option<(&ClassId, &MethodDef)>,
    call: &Expr,
) -> Result<Vec<(&'a MethodDef, bool)>, &'static str> {
    let (Some((owner, enclosing)), ExprNode::Send { recv, method, .. }) = (context, &*call.node)
    else {
        return Ok(vec![]);
    };
    if recv
        .as_ref()
        .is_some_and(|r| !matches!(&*r.node, ExprNode::SelfRef))
    {
        return Ok(vec![]);
    }
    fn descends(
        contracts: &SourceContractIndex<'_>,
        child: &ClassId,
        owner: &ClassId,
        seen: &mut HashSet<ClassId>,
    ) -> bool {
        child == owner
            || (seen.insert(child.clone())
                && (contracts
                    .parent(child)
                    .is_some_and(|p| descends(contracts, p, owner, seen))
                    || contracts
                        .includes(child)
                        .iter()
                        .any(|m| descends(contracts, m, owner, seen))))
    }
    let mut candidates = Vec::new();
    for child in
        contracts.virtual_owners.iter().copied().filter(|child| {
            *child != owner && descends(contracts, child, owner, &mut HashSet::new())
        })
    {
        // Completeness depends on the descendant's entire reachable lookup
        // chain, including reopened included modules.
        if !contracts.verified_hierarchy(child, &mut HashSet::new()) {
            return Err(
                "forwarding virtual dispatch through reopened fragments cannot be verified",
            );
        }
        if let Some(candidate) = contracts.effective_call(child, method, enclosing.receiver) {
            candidates.push(candidate);
        }
    }
    Ok(candidates)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeywordPolicy {
    Native,
    Legacy,
    Refuse,
    RefuseOrdinarySuper,
}

pub(crate) fn keyword_refusal(span: Span, policy: KeywordPolicy) -> Diagnostic {
    if policy == KeywordPolicy::RefuseOrdinarySuper {
        return Diagnostic::unsupported(
            span,
            None,
            "keyword splat in ordinary super",
            "super destination's native or lowered argument ABI cannot be verified",
        );
    }
    Diagnostic::unsupported(
        span,
        None,
        "keyword splat into full argument forwarding",
        "keyword producer's full forwarding destination cannot be verified",
    )
}

/// Classify before projection. Source copies sharing a span must agree:
/// a cloned concern body can have different contracts in two includers.
pub(crate) fn keyword_calls(app: &App) -> HashMap<Span, KeywordPolicy> {
    let contracts = SourceContractIndex::new(app);
    keyword_calls_with_index(app, &contracts)
}

fn keyword_calls_with_index(
    app: &App,
    contracts: &SourceContractIndex<'_>,
) -> HashMap<Span, KeywordPolicy> {
    fn visit(
        app: &App,
        contracts: &SourceContractIndex<'_>,
        context: Option<(&ClassId, &MethodDef)>,
        e: &Expr,
        plans: &mut HashMap<Span, KeywordPolicy>,
        fallback: bool,
    ) {
        let args = match &*e.node {
            ExprNode::Send { args, .. } => Some(args.as_slice()),
            ExprNode::Super { args: Some(args) } => Some(args.as_slice()),
            _ => None,
        };
        if args.is_some_and(|a| {
            a.iter()
                .any(|a| matches!(&*a.node, ExprNode::KeywordSplat { .. }))
        }) && !(fallback && plans.contains_key(&e.span))
        {
            let policy = if !possible_full_destination(contracts, context, e) {
                // Unrelated selectors cannot reach a full contract. Avoid
                // scanning receiver provenance and hierarchies for each of
                // their keyword calls in a large application.
                if matches!(&*e.node, ExprNode::Super { .. })
                    && destination(app, contracts, context, e).is_none()
                {
                    // An absent full selector does not prove that erasing **
                    // matches an external parent's native keyword ABI.
                    KeywordPolicy::RefuseOrdinarySuper
                } else {
                    KeywordPolicy::Legacy
                }
            } else {
                let resolved = destination(app, contracts, context, e);
                if resolved.is_some_and(|(m, _)| m.params.iter().any(|p| p.forwarding)) {
                    if contract_error(context, e, resolved, contracts).is_none() {
                        KeywordPolicy::Native
                    } else {
                        KeywordPolicy::Refuse
                    }
                } else if resolved.is_none()
                    || virtual_destinations(contracts, context, e).map_or(true, |v| {
                        v.iter().any(|(m, _)| m.params.iter().any(|p| p.forwarding))
                    })
                {
                    // Ordinary lexical method, full virtual override: neither
                    // legacy expansion nor native-only admission proves both.
                    KeywordPolicy::Refuse
                } else {
                    // Deliberately preserve the old ordinary-callee lowering;
                    // this prerequisite does not fix its missing/extra-key paths.
                    KeywordPolicy::Legacy
                }
            };
            plans
                .entry(e.span)
                .and_modify(|previous| {
                    if *previous != policy {
                        *previous = KeywordPolicy::Refuse;
                    }
                })
                .or_insert(policy);
        }
        e.node
            .for_each_child(&mut |c| visit(app, contracts, context, c, plans, fallback));
    }
    let mut plans = HashMap::new();
    for (owner, method) in methods(app) {
        visit(
            app,
            contracts,
            Some((owner, method)),
            &method.body,
            &mut plans,
            false,
        );
        for default in method.params.iter().filter_map(|p| p.default.as_ref()) {
            visit(
                app,
                contracts,
                Some((owner, method)),
                default,
                &mut plans,
                false,
            );
        }
    }
    crate::lower::for_each_forwarding_body_ref(app, &mut |e| {
        visit(app, contracts, None, e, &mut plans, true)
    });
    plans
}

fn possible_full_destination(
    contracts: &SourceContractIndex<'_>,
    context: Option<(&ClassId, &MethodDef)>,
    call: &Expr,
) -> bool {
    let method = match &*call.node {
        ExprNode::Send { method, .. } => method,
        // Super uses the enclosing selector, not every full selector in the
        // app. Unrelated framework parents keep their ordinary keyword ABI.
        ExprNode::Super { .. } => match context {
            Some((_, enclosing)) => &enclosing.name,
            None => return true,
        },
        _ => return true,
    };
    // No declaration proof: refuse if this selector can name a source
    // full forwarder. Unrelated catalog/framework calls keep legacy lowering.
    contracts.full_selectors.contains(method)
        || (method.as_str() == "new"
            && contracts
                .full_selectors
                .contains(&Symbol::from("initialize")))
}

fn has_forwarding(args: &[Expr]) -> bool {
    args.iter()
        .any(|a| matches!(&*a.node, ExprNode::ForwardArgs))
}

fn constant_names_class(expr: &Expr, class: &ClassId) -> bool {
    let ExprNode::Const { path } = &*expr.node else {
        return false;
    };
    let spelling = path
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join("::");
    class.0.as_str() == spelling || class.0.as_str().ends_with(&format!("::{spelling}"))
}

/// Narrow source-side provenance: a built-in constructor, or its single
/// unconditional local binding before this read. Arbitrary factories, joins,
/// reassignments and closures are deliberately not an instance proof.
fn constructed_instance(
    contracts: &SourceContractIndex<'_>,
    method: Option<&MethodDef>,
    recv: &Expr,
    class: &ClassId,
) -> bool {
    let built_in_new = |value: &Expr| {
        let ExprNode::Send {
            recv: Some(r),
            method,
            ..
        } = &*value.node
        else {
            return false;
        };
        constant_names_class(r, class)
            && method.as_str() == "new"
            && matches!(r.ty.as_ref(), Some(Ty::Class { id, .. }) if id == class)
            && contracts
                .declaration(
                    class,
                    &Symbol::from("new"),
                    MethodReceiver::Class,
                    &mut HashSet::new(),
                )
                .is_none()
    };
    if built_in_new(recv) {
        return true;
    }
    let Some(method) = method else { return false };
    let ExprNode::Var { name, .. } = &*recv.node else {
        return false;
    };
    let ExprNode::Seq { exprs } = &*method.body.node else {
        return false;
    };
    let binding = exprs.iter().any(|e| {
        matches!(&*e.node, ExprNode::Assign { target: LValue::Var { name: n, .. }, value }
            if n == name && !e.span.is_synthetic() && e.span.file == recv.span.file
                && e.span.end <= recv.span.start && built_in_new(value))
    });
    fn writes(e: &Expr, name: &Symbol, count: &mut usize, closure: &mut bool) {
        match &*e.node {
            ExprNode::Assign {
                target: LValue::Var { name: n, .. },
                ..
            }
            | ExprNode::OpAssign {
                target: LValue::Var { name: n, .. },
                ..
            } if n == name => *count += 1,
            ExprNode::MultiAssign { targets, .. } => {
                *count += targets
                    .iter()
                    .filter(|t| matches!(t, LValue::Var { name: n, .. } if n == name))
                    .count();
            }
            ExprNode::Lambda { .. } => *closure = true,
            _ => {}
        }
        e.node
            .for_each_child(&mut |c| writes(c, name, count, closure));
    }
    let mut count = 0;
    let mut closure = false;
    writes(&method.body, name, &mut count, &mut closure);
    binding && count == 1 && !closure
}

/// Source declarations, not inferred arity: inference does not retain the
/// flattening provenance. Own method, last-included mixin, then superclass.
/// The inferred ClassInfo resolver cannot supply this contract, and the
/// tree-shaker's conservative runtime lookup intentionally conflates receiver
/// sides and aliases. Neither is an admission check for source semantics.
struct SourceContractIndex<'a> {
    parents: HashMap<ClassId, &'a ClassId>,
    includes: HashMap<ClassId, Vec<ClassId>>,
    fragments: HashMap<ClassId, usize>,
    instance: HashMap<(ClassId, Symbol), (&'a MethodDef, bool)>,
    class: HashMap<(ClassId, Symbol), (&'a MethodDef, bool)>,
    virtual_owners: Vec<&'a ClassId>,
    full_selectors: HashSet<Symbol>,
    unretained: HashSet<Span>,
}

impl<'a> SourceContractIndex<'a> {
    fn new(app: &'a App) -> Self {
        let mut index = Self {
            parents: HashMap::new(),
            includes: HashMap::new(),
            fragments: HashMap::new(),
            instance: HashMap::new(),
            class: HashMap::new(),
            virtual_owners: Vec::new(),
            full_selectors: HashSet::new(),
            unretained: HashSet::new(),
        };
        let mut class_owners = HashSet::new();
        for class in classes(app) {
            class_owners.insert(class.name.clone());
            index.add_fragment(
                &class.name,
                class.parent.as_ref(),
                class.includes.iter().cloned(),
            );
            index.add_methods(&class.name, class.methods.iter(), false);
            index.virtual_owners.push(&class.name);
        }
        for model in &app.models {
            index.add_fragment(
                &model.name,
                model.parent.as_ref(),
                super::model_includes(model).into_iter(),
            );
            if !class_owners.contains(&model.name) {
                index.add_methods(&model.name, model.methods(), true);
            }
            index.virtual_owners.push(&model.name);
        }
        for module in &app.test_modules {
            index.add_fragment(
                &module.name,
                module.parent.as_ref(),
                module.includes.iter().cloned(),
            );
            if !class_owners.contains(&module.name)
                && !app.models.iter().any(|model| model.name == module.name)
            {
                index.add_methods(&module.name, module.helpers.iter(), false);
            }
        }
        // Includes association-extension methods, matching the public method
        // inventory used by the former conservative selector scan.
        index.full_selectors.extend(
            methods(app)
                .filter(|(_, method)| method.params.iter().any(|p| p.forwarding))
                .map(|(_, method)| method.name.clone()),
        );
        if !index.full_selectors.is_empty() {
            // Ordinary inherited contracts matter only when a packet is sent
            // to them. Looking up every unrelated selector for every model
            // makes large-app transpilation quadratic in the source inventory.
            let mut names = index.full_selectors.clone();
            fn collect(e: &Expr, names: &mut HashSet<Symbol>) {
                if let ExprNode::Send { method, args, .. } = &*e.node
                    && has_forwarding(args)
                {
                    names.insert(method.clone());
                    if method.as_str() == "new" {
                        names.insert(Symbol::from("initialize"));
                    }
                }
                e.node.for_each_child(&mut |child| collect(child, names));
            }
            for (_, method) in methods(app) {
                collect(&method.body, &mut names);
                for default in method.params.iter().filter_map(|p| p.default.as_ref()) {
                    collect(default, &mut names);
                }
            }
            index.unretained = crate::lower::model_to_library::unretained_model_contracts(app, |model| {
                let mut inherited = Vec::new();
                for name in &names {
                    for receiver in [MethodReceiver::Instance, MethodReceiver::Class] {
                        if let Some((method, _)) = index.declaration(
                            &model.name, name, receiver, &mut HashSet::new(),
                        ) && !model.methods().any(|own| std::ptr::eq(own, method)) {
                            inherited.push(method);
                        }
                    }
                }
                inherited
            });
        }
        index
    }

    fn add_fragment(
        &mut self,
        owner: &ClassId,
        parent: Option<&'a ClassId>,
        includes: impl Iterator<Item = ClassId>,
    ) {
        *self.fragments.entry(owner.clone()).or_default() += 1;
        if let Some(parent) = parent {
            self.parents.entry(owner.clone()).or_insert(parent);
        }
        self.includes
            .entry(owner.clone())
            .or_default()
            .extend(includes);
    }

    fn add_methods(
        &mut self,
        owner: &ClassId,
        methods: impl Iterator<Item = &'a MethodDef>,
        model: bool,
    ) {
        for method in methods {
            let declarations = match method.receiver {
                MethodReceiver::Instance => &mut self.instance,
                MethodReceiver::Class => &mut self.class,
            };
            declarations.insert((owner.clone(), method.name.clone()), (method, model));
        }
    }

    fn parent(&self, owner: &ClassId) -> Option<&'a ClassId> {
        self.parents.get(owner).copied()
    }

    fn includes(&self, owner: &ClassId) -> &[ClassId] {
        self.includes.get(owner).map_or(&[], Vec::as_slice)
    }

    fn verified_hierarchy(&self, owner: &ClassId, seen: &mut HashSet<ClassId>) -> bool {
        if !seen.insert(owner.clone()) {
            return true;
        }
        self.fragments.get(owner).copied().unwrap_or(0) <= 1
            && self
                .parent(owner)
                .is_none_or(|parent| self.verified_hierarchy(parent, seen))
            && self
                .includes(owner)
                .iter()
                .all(|module| self.verified_hierarchy(module, seen))
    }

    fn declaration(
        &self,
        owner: &ClassId,
        name: &Symbol,
        receiver: MethodReceiver,
        seen: &mut HashSet<ClassId>,
    ) -> Option<(&'a MethodDef, bool)> {
        if !self.verified_hierarchy(owner, &mut HashSet::new()) || !seen.insert(owner.clone()) {
            return None;
        }
        let declarations = match receiver {
            MethodReceiver::Instance => &self.instance,
            MethodReceiver::Class => &self.class,
        };
        declarations
            .get(&(owner.clone(), name.clone()))
            .copied()
            .or_else(|| self.ancestors(owner, name, receiver, seen))
    }

    fn ancestors(
        &self,
        owner: &ClassId,
        name: &Symbol,
        receiver: MethodReceiver,
        seen: &mut HashSet<ClassId>,
    ) -> Option<(&'a MethodDef, bool)> {
        if receiver == MethodReceiver::Instance {
            for module in self.includes(owner).iter().rev() {
                if let Some(method) = self.declaration(module, name, receiver, seen) {
                    return Some(method);
                }
            }
        }
        self.parent(owner)
            .and_then(|parent| self.declaration(parent, name, receiver, seen))
    }

    fn effective_call(
        &self,
        owner: &ClassId,
        name: &Symbol,
        receiver: MethodReceiver,
    ) -> Option<(&'a MethodDef, bool)> {
        self.declaration(owner, name, receiver, &mut HashSet::new())
            .or_else(|| {
                (name.as_str() == "new" && receiver == MethodReceiver::Class)
                    .then(|| {
                        self.declaration(
                            owner,
                            &Symbol::from("initialize"),
                            MethodReceiver::Instance,
                            &mut HashSet::new(),
                        )
                    })
                    .flatten()
            })
    }
}
