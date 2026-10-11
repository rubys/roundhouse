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

mod constructor;
pub(crate) use constructor::ConstructorContract;

pub(super) fn diagnose(app: &App) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let actions = controller_defs(app);
    let contracts = SourceContractIndex::new(app, &actions);
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
    for (owner, method) in methods(app).chain(actions.iter().map(|(c, m)| (c, m))) {
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
    for (span, policy) in keyword_calls_with_index(app, &contracts, &actions) {
        if matches!(
            policy,
            KeywordPolicy::Refuse | KeywordPolicy::RefuseKeywords | KeywordPolicy::RefuseOrdinarySuper
        ) {
            out.push(keyword_refusal(span, policy));
        }
    }
    out
}

/// Every controller action as the `MethodDef` `controller_to_library`
/// builds from it (`Action::formal_params`). Controllers lower after
/// these checks run; without this view an action's `**` or `...` would
/// forward unverified, into a destination whose keywords ingest
/// flattened as readily as into one that kept them. The body is copied
/// only when it carries an argument packet, the one thing the checks
/// walk it for; the rest serve as lookup destinations.
pub(crate) fn controller_defs(app: &App) -> Vec<(ClassId, MethodDef)> {
    fn has_packet(e: &Expr) -> bool {
        if matches!(
            &*e.node,
            ExprNode::ForwardArgs
                | ExprNode::ForwardKeywords
                | ExprNode::ForwardKeywordsWithPairs { .. }
                | ExprNode::KeywordSplat { .. }
        ) {
            return true;
        }
        let mut found = false;
        e.node.for_each_child(&mut |c| found = found || has_packet(c));
        found
    }
    let mut out = Vec::new();
    for controller in &app.controllers {
        for action in controller.actions() {
            let params = action.formal_params();
            let body = if has_packet(&action.body)
                || params.iter().any(|p| p.default.as_ref().is_some_and(has_packet))
            {
                action.body.clone()
            } else {
                Expr::new(action.body.span, ExprNode::Seq { exprs: vec![] })
            };
            out.push((
                controller.name.clone(),
                MethodDef {
                    name: action.name.clone(),
                    receiver: MethodReceiver::Instance,
                    visibility: crate::dialect::MethodVisibility::Public,
                    params,
                    unsupported_formals: None,
                    // `def f(&)` binds the sentinel name ingest gives it.
                    has_anonymous_block: action.block_param.as_ref().is_some_and(|b| b.as_str() == "__blk"),
                    block_param: action.block_param.clone().map(crate::dialect::Param::positional),
                    name_span: action.name_span,
                    body,
                    signature: None,
                    effects: action.effects.clone(),
                    enclosing_class: Some(controller.name.0.clone()),
                    kind: crate::dialect::AccessorKind::Method,
                    is_async: false,
                    mutates_self: false,
                },
            ));
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
                .flat_map(|t| t.helpers.iter().map(move |m| (&t.name, m))))
        .chain(app.controllers.iter().flat_map(|c| c.class_methods().map(move |m| (&c.name, m))),
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
    let forwards_keywords = match &*e.node {
        ExprNode::Send { args, .. } | ExprNode::Super { args: Some(args) } => {
            args.iter().any(|a| {
                matches!(
                    &*a.node,
                    ExprNode::ForwardKeywords | ExprNode::ForwardKeywordsWithPairs { .. }
                )
            })
        }
        _ => false,
    };
    if forwards_keywords {
        let source_ok = enclosing
            .params
            .iter()
            .any(|p| p.keyword && p.rest && p.name.as_str().is_empty() && !p.forwarding);
        let resolved = destination(app, contracts, Some((owner, enclosing)), e);
        let reason = if !source_ok {
            Some("anonymous keyword forwarding has no enclosing anonymous keyword-rest declaration")
        } else if enclosing.unsupported_formals.is_some()
            || contracts.unretained_in_lookup(
                owner,
                &enclosing.name,
                enclosing.name_span,
                enclosing.receiver,
            )
            || !contracts.verified_hierarchy(owner, &mut HashSet::new())
        {
            Some("anonymous keyword forwarding source declaration cannot be verified")
        } else {
            keyword_contract_error(Some((owner, enclosing)), e, resolved, contracts)
        };
        if let Some(reason) = reason {
            out.push(Diagnostic::unsupported(
                e.span,
                None,
                "anonymous keyword forwarding",
                reason,
            ));
        }
    }
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
                destination(app, contracts, Some((owner, enclosing)), e)
                    .map(|resolved| (resolved.method, resolved.model)),
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

fn accepts_keywords(method: &MethodDef) -> bool {
    method.params.iter().any(|p| p.forwarding || p.keyword)
}

/// Fail closed unless a source packet reaches a declaration with preserved
/// keyword ABI and every reachable model/virtual override retains it. Ordinary
/// sends use indexed lookup; `super` retains the conservative global source-span
/// synthesis guard because its runtime ancestor path is not fully represented.
fn keyword_contract_error(
    context: Option<(&ClassId, &MethodDef)>,
    call: &Expr,
    resolved: Option<ResolvedDestination<'_>>,
    contracts: &SourceContractIndex<'_>,
) -> Option<&'static str> {
    let Some(resolved) = resolved else {
        return Some("forwarding destination's declaration cannot be verified");
    };
    let method = resolved.method;
    if method.unsupported_formals.is_some() {
        return Some("forwarding destination has an unrepresented parameter declaration");
    }
    if method
        .params
        .iter()
        .any(|p| p.from_keyword || p.from_kwrest)
    {
        return Some("forwarding destination has flattened keyword parameters");
    }
    let unretained = if matches!(&*call.node, ExprNode::Super { .. }) {
        contracts.unretained.contains(&method.name_span)
    } else {
        contracts.unretained_in_lookup(
            &resolved.lookup_owner,
            &method.name,
            method.name_span,
            method.receiver,
        )
    };
    if unretained {
        return Some("model method synthesis does not preserve this source declaration");
    }
    let Ok(destinations) = virtual_destinations(contracts, context, call) else {
        return Some("forwarding destination's declaration cannot be verified");
    };
    if !accepts_keywords(method)
        || destinations.iter().any(|(_, (candidate, _))| {
            !accepts_keywords(candidate)
                || candidate.unsupported_formals.is_some()
                || candidate
                    .params
                    .iter()
                    .any(|p| p.from_keyword || p.from_kwrest)
        })
    {
        return Some("forwarding destination has no verified keyword parameter ABI");
    }
    if destinations.iter().any(|(owner, (candidate, _))| {
        contracts.unretained_in_lookup(
            owner,
            &candidate.name,
            candidate.name_span,
            candidate.receiver,
        )
    }) {
        return Some("model method synthesis does not preserve this source declaration");
    }
    None
}

/// A source-resolved call target plus the receiver's original lookup start.
/// Keeping the owner is necessary to distinguish an affected model lookup
/// from an unrelated sibling that happens to share the declaration span.
struct ResolvedDestination<'a> {
    method: &'a MethodDef,
    model: bool,
    /// The source receiver class, before method lookup walks through its
    /// parents. A synthesized method on an intermediate model can shadow the
    /// declaration selected by the source-only index for this receiver.
    lookup_owner: ClassId,
}

/// Resolve only destinations whose source receiver and dispatch path are
/// represented well enough to check an argument contract. Unknown receiver
/// provenance, association methods, and unresolved `super` ancestors return
/// `None` instead of borrowing an inferred ABI.
fn destination<'a>(
    app: &'a App,
    contracts: &SourceContractIndex<'a>,
    context: Option<(&ClassId, &MethodDef)>,
    e: &Expr,
) -> Option<ResolvedDestination<'a>> {
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
                    (ExprNode::Const { path }, None) => {
                        // Carried test-helper bodies retain their original lexical
                        // source scope even when their nominal receiver is a test.
                        let id = contracts.constants.namespace(r.span, path)?;
                        return destination_for_class(app, contracts, id, method, receiver);
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
                destination_for_class(app, contracts, c, method, receiver)
            })
        }
        ExprNode::Super { .. } => {
            let (owner, enclosing) = context?;
            contracts
                .ancestors(
                    owner,
                    &enclosing.name,
                    enclosing.receiver,
                    &mut HashSet::new(),
                )
                .map(|(method, model)| ResolvedDestination {
                    method,
                    model,
                    lookup_owner: owner.clone(),
                })
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

fn destination_for_class<'a>(
    app: &App,
    contracts: &SourceContractIndex<'a>,
    owner: &ClassId,
    name: &Symbol,
    receiver: MethodReceiver,
) -> Option<ResolvedDestination<'a>> {
    if constructor::unknown_lookup(contracts, owner, name, receiver) {
        return None;
    }
    let (method, model) = contracts.effective_call(owner, name, receiver)?;
    // A flattened ClassMethods instance definition is callable on its real
    // includer, not as a singleton on the source module itself.
    if receiver == MethodReceiver::Class && contracts.modules.contains(owner)
        && app.concern_class_method_templates.get(owner)
            .is_some_and(|spans| spans.contains(&method.name_span))
    {
        return None;
    }
    Some(ResolvedDestination {
        method,
        model,
        lookup_owner: owner.clone(),
    })
}

fn declaration_error((method, _model): (&MethodDef, bool)) -> Option<&'static str> {
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
        // `super(...)` with no in-app ancestor — Campfire's
        // `WebPush::Connections::Stages` is `extend`ed onto a
        // `Net::HTTP` instance, so `super` lands on stdlib. We cannot
        // verify that ABI from app source; allowing the forward is the
        // honest alternative to residualizing a pattern RH Ruby/Spinel
        // already emit and run.
        if matches!(&*call.node, ExprNode::Super { .. }) {
            return None;
        }
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
    for (_, candidate) in candidates {
        if declaration_error(candidate).is_some()
            || contracts.unretained.contains(&candidate.0.name_span)
        {
            return Some("forwarding self-dispatch may reach an unpreserved subclass contract");
        }
    }
    None
}

/// Enumerate source-visible self-dispatch overrides. An incomplete or reopened
/// lookup hierarchy is an error rather than evidence that no override exists.
fn virtual_destinations<'a>(
    contracts: &SourceContractIndex<'a>,
    context: Option<(&ClassId, &MethodDef)>,
    call: &Expr,
) -> Result<Vec<(ClassId, (&'a MethodDef, bool))>, &'static str> {
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
        if constructor::unknown_lookup(contracts, child, method, enclosing.receiver) {
            return Err("forwarding virtual constructor lookup cannot be verified");
        }
        if let Some(candidate) = contracts.effective_call(child, method, enclosing.receiver) {
            candidates.push((child.clone(), candidate));
        }
    }
    Ok(candidates)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeywordPolicy {
    Native,
    NativeKeywords,
    Legacy,
    Refuse,
    RefuseKeywords,
    RefuseOrdinarySuper,
}

pub(crate) fn keyword_refusal(span: Span, policy: KeywordPolicy) -> Diagnostic {
    if policy == KeywordPolicy::RefuseKeywords {
        return Diagnostic::unsupported(
            span,
            None,
            "keyword splat into source keyword parameters",
            "keyword destination's native argument ABI cannot be verified",
        );
    }
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
    let actions = controller_defs(app);
    let contracts = SourceContractIndex::new(app, &actions);
    keyword_calls_with_index(app, &contracts, &actions)
}

pub(crate) fn keyword_calls_and_constructor_contracts(
    app: &App,
) -> (
    HashMap<Span, KeywordPolicy>,
    HashMap<ClassId, ConstructorContract<'_>>,
) {
    // The plans read the controller actions too; the constructor
    // contracts outlive this call and borrow `app` alone, so they get an
    // index of their own (a controller is never a constructor).
    let plans = keyword_calls(app);
    let contracts = SourceContractIndex::new(app, &[]);
    (
        plans,
        constructor::constructor_contracts_with_index(app, &contracts),
    )
}

fn keyword_calls_with_index(
    app: &App,
    contracts: &SourceContractIndex<'_>,
    actions: &[(ClassId, MethodDef)],
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
            let policy = if !possible_full_destination(contracts, context, e)
                && !possible_keyword_destination(contracts, context, e) {
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
                if resolved
                    .as_ref()
                    .is_some_and(|d| d.method.params.iter().any(|p| p.forwarding))
                {
                    if contract_error(context, e, resolved.map(|d| (d.method, d.model)), contracts)
                        .is_none()
                    {
                        KeywordPolicy::Native
                    } else {
                        KeywordPolicy::Refuse
                    }
                } else if possible_keyword_destination(contracts, context, e) {
                    if keyword_contract_error(context, e, resolved, contracts).is_none() {
                        KeywordPolicy::NativeKeywords
                    } else {
                        KeywordPolicy::RefuseKeywords
                    }
                } else if resolved.is_none()
                    || virtual_destinations(contracts, context, e).map_or(true, |v| {
                        v.iter()
                            .any(|(_, (m, _))| m.params.iter().any(|p| p.forwarding))
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
    let mut template_roots = HashSet::new();
    for (owner, method) in methods(app).chain(actions.iter().map(|(c, m)| (c, m))) {
        // Non-singleton templates do not execute on the source module.
        // They may be copied onto an includer or consumed by class-body
        // macro expansion. Check every surviving copy and real singleton,
        // not an invented call on the carrier's self.
        if method.receiver == MethodReceiver::Class
            && contracts.modules.contains(owner)
            && method.name_span.file.0 != 0
            && app
                .concern_class_method_templates
                .get(owner)
                .is_some_and(|spans| spans.contains(&method.name_span))
        {
            // Exact roots, not shared/synthetic spans: runtime copies still
            // participate in the fallback inventory.
            template_roots.insert(&method.body as *const Expr);
            template_roots.extend(method.params.iter().filter_map(|p| p.default.as_ref()).map(|e| e as *const Expr));
            continue;
        }
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
        if !template_roots.contains(&(e as *const Expr)) {
            visit(app, contracts, None, e, &mut plans, true)
        }
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
                .full_selectors.contains(&Symbol::from("initialize")))
}

fn possible_keyword_destination(
    contracts: &SourceContractIndex<'_>,
    context: Option<(&ClassId, &MethodDef)>,
    call: &Expr,
) -> bool {
    let method = match &*call.node {
        ExprNode::Send { method, .. } => method,
        ExprNode::Super { .. } => match context {
            Some((_, enclosing)) => &enclosing.name,
            None => return false,
        },
        _ => return false,
    };
    contracts.keyword_selectors.contains(method)
        || (method.as_str() == "new"
            && contracts.keyword_selectors
                .contains(&Symbol::from("initialize")))
}

fn has_forwarding(args: &[Expr]) -> bool {
    args.iter()
        .any(|a| matches!(&*a.node, ExprNode::ForwardArgs))
}

fn has_source_packet(args: &[Expr]) -> bool {
    args.iter().any(|a| {
        matches!(
            &*a.node,
            ExprNode::ForwardArgs
                | ExprNode::ForwardKeywords
                | ExprNode::ForwardKeywordsWithPairs { .. }
        )
    })
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
    constants: std::sync::Arc<super::body::ConstResolver>,
    parents: HashMap<ClassId, &'a ClassId>,
    includes: HashMap<ClassId, Vec<ClassId>>,
    modules: HashSet<ClassId>,
    data_factories: HashSet<ClassId>,
    unmodeled_constructor_lookup: HashSet<ClassId>,
    fragments: HashMap<ClassId, usize>,
    instance: HashMap<(ClassId, Symbol), (&'a MethodDef, bool)>,
    class: HashMap<(ClassId, Symbol), (&'a MethodDef, bool)>,
    virtual_owners: Vec<&'a ClassId>,
    full_selectors: HashSet<Symbol>,
    keyword_selectors: HashSet<Symbol>,
    unretained: HashSet<Span>,
    /// The model whose emitted synthesis shadows a source method contract.
    /// Keep the model alongside the span so an unrelated sibling receiver
    /// does not inherit another model's loss, while descendants still do.
    unretained_models: HashSet<(Span, ClassId)>,
}

impl<'a> SourceContractIndex<'a> {
    /// Build source lookup and collect only selectors that need preservation
    /// checks: full forwarders and declarations reached by source argument
    /// packets. This bounds the model-synthesis survey without omitting
    /// inherited contracts selected through sends or explicit packet `super`.
    fn new(app: &'a App, actions: &'a [(ClassId, MethodDef)]) -> Self {
        let mut index = Self {
            constants: app.const_resolver.for_sources(&app.sources),
            parents: HashMap::new(),
            includes: HashMap::new(),
            modules: HashSet::new(),
            data_factories: HashSet::new(),
            unmodeled_constructor_lookup: HashSet::new(),
            fragments: HashMap::new(),
            instance: HashMap::new(),
            class: HashMap::new(),
            virtual_owners: Vec::new(),
            full_selectors: HashSet::new(),
            keyword_selectors: HashSet::new(),
            unretained: HashSet::new(),
            unretained_models: HashSet::new(),
        };
        let mut class_owners = HashSet::new();
        for class in classes(app) {
            class_owners.insert(class.name.clone());
            if class.is_module {
                index.modules.insert(class.name.clone());
            }
            if matches!(class.origin, Some(crate::dialect::LibraryClassOrigin::DataFactory { .. })) {
                index.data_factories.insert(class.name.clone());
            }
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
        // Controllers' spliced concern methods are already among their
        // actions, so the includes add nothing a lookup could find.
        for controller in &app.controllers {
            index.add_fragment(&controller.name, controller.parent.as_ref(), std::iter::empty());
            index.add_methods(&controller.name, controller.class_methods(), false);
        }
        for (owner, method) in actions {
            index.add_methods(owner, std::iter::once(method), false);
        }
        // Includes association-extension methods, matching the public method
        // inventory used by the former conservative selector scan.
        index.full_selectors.extend(
            methods(app)
                .chain(actions.iter().map(|(c, m)| (c, m)))
                .filter(|(_, method)| method.params.iter().any(|p| p.forwarding))
                .map(|(_, method)| method.name.clone()),
        );
        index.keyword_selectors.extend(
            methods(app)
                .chain(actions.iter().map(|(c, m)| (c, m)))
                .filter(|(_, method)| method.params.iter().any(|p| p.keyword))
                .map(|(_, method)| method.name.clone()),
        );
        // Full forwarders need inherited-contract checks too; ordinary
        // methods matter only when a source packet is sent to them or
        // forwarded through `super`. Include matching model-owned
        // declarations as well, since synthesis can replace those before
        // emission. Looking up every unrelated selector for every model
        // makes large-app transpilation quadratic in the source inventory.
        let mut names = index.full_selectors.clone();
        // Add selectors whose source packets can reach model-built methods;
        // `super` inherits the enclosing method's selector rather than
        // carrying one on its own node.
        fn collect(e: &Expr, enclosing: &Symbol, names: &mut HashSet<Symbol>) {
            if let ExprNode::Send { method, args, .. } = &*e.node
                && has_source_packet(args)
            {
                names.insert(method.clone());
                if method.as_str() == "new" {
                    names.insert(Symbol::from("initialize"));
                }
            }
            if let ExprNode::Super { args: Some(args) } = &*e.node
                && has_source_packet(args)
            {
                names.insert(enclosing.clone());
            }
            e.node
                .for_each_child(&mut |child| collect(child, enclosing, names));
        }
        for (_, method) in methods(app) {
            collect(&method.body, &method.name, &mut names);
            for default in method.params.iter().filter_map(|p| p.default.as_ref()) {
                collect(default, &method.name, &mut names);
            }
        }
        if !names.is_empty() {
            index.unretained_models =
                crate::lower::model_to_library::unretained_model_contracts(app, |model| {
                    let mut selected = Vec::new();
                    for name in &names {
                        for receiver in [MethodReceiver::Instance, MethodReceiver::Class] {
                            if let Some((method, _)) =
                                index.declaration(&model.name, name, receiver, &mut HashSet::new())
                            {
                                selected.push(method);
                            }
                        }
                    }
                    selected
                });
            index.unretained = index
                .unretained_models
                .iter()
                .map(|(span, _)| *span)
                .collect();
        }
        constructor::index_unmodeled_lookup_mutations(app, &mut index);
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

    /// Whether model synthesis shadows this source declaration anywhere in
    /// the receiver's method-lookup chain. Descendants can inherit the
    /// incompatible method generated by an intermediate model even when they
    /// have no conflicting synthesis of their own.
    fn unretained_in_lookup(
        &self,
        owner: &ClassId,
        selector: &Symbol,
        span: Span,
        receiver: MethodReceiver,
    ) -> bool {
        fn reaches(
            index: &SourceContractIndex<'_>,
            owner: &ClassId,
            selector: &Symbol,
            span: Span,
            receiver: MethodReceiver,
            seen: &mut HashSet<ClassId>,
        ) -> Option<bool> {
            if !seen.insert(owner.clone()) {
                return None;
            }
            if index.unretained_models.contains(&(span, owner.clone())) {
                return Some(true);
            }
            let declarations = match receiver {
                MethodReceiver::Instance => &index.instance,
                MethodReceiver::Class => &index.class,
            };
            if declarations.contains_key(&(owner.clone(), selector.clone())) {
                // A source declaration at this point in the lookup chain
                // blocks later ancestors. If it is the requested method,
                // its source contract is still present; if it differs, the
                // source resolver should have selected that nearer method.
                return Some(false);
            }
            if receiver == MethodReceiver::Instance {
                for module in index.includes(owner).iter().rev() {
                    if let Some(result) = reaches(index, module, selector, span, receiver, seen) {
                        return Some(result);
                    }
                }
            }
            index
                .parent(owner)
                .and_then(|parent| reaches(index, parent, selector, span, receiver, seen))
        }
        reaches(self, owner, selector, span, receiver, &mut HashSet::new()).unwrap_or(false)
    }

    fn verified_hierarchy(&self, owner: &ClassId, seen: &mut HashSet<ClassId>) -> bool {
        if !seen.insert(owner.clone()) {
            return true;
        }
        (self.fragments.get(owner).copied().unwrap_or(0) <= 1 || self.modules.contains(owner))
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

    /// Find the source declaration governing a call, falling back from class-side
    /// `new` to instance-side `initialize` for ordinary constructors. Data factories
    /// are excluded from that fallback because their constructor normalizes arguments
    /// before invoking the initializer; the two call contracts are not interchangeable.
    fn effective_call(
        &self,
        owner: &ClassId,
        name: &Symbol,
        receiver: MethodReceiver,
    ) -> Option<(&'a MethodDef, bool)> {
        self.declaration(owner, name, receiver, &mut HashSet::new())
            .or_else(|| {
                (name.as_str() == "new" && receiver == MethodReceiver::Class
                    && !self.data_factories.contains(owner))
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
