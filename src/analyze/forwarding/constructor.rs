//! Source-level `Class.new` lookup and the effective `initialize` contract.
//!
//! Constructor lowering must use Ruby's source dispatch, not the analyzer's
//! flattened parameter signature: custom class-side `new`, mixins, mutation
//! hooks and method-lookup edits can all invalidate the default forwarder.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::dialect::{MethodDef, MethodReceiver};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};

use super::SourceContractIndex;

pub(crate) enum ConstructorContract<'a> {
    CustomNew(Option<&'a MethodDef>),
    UnknownLookup,
    Initialize(&'a MethodDef),
}

pub(super) fn constructor_contracts_with_index<'a>(
    app: &'a App,
    contracts: &SourceContractIndex<'a>,
) -> HashMap<ClassId, ConstructorContract<'a>> {
    app.library_classes
        .iter()
        .map(|class| &class.name)
        .chain(app.models.iter().map(|model| &model.name))
        .filter_map(|owner| {
            constructor_contract(contracts, owner).map(|contract| (owner.clone(), contract))
        })
        .collect()
}

/// Mark only owners whose class-side `new` lookup can be changed by source
/// hooks or explicit lookup mutations. Kept out of the general forwarding
/// index build so ordinary keyword-forwarding analysis does not pay for or
/// depend on constructor-specific policy.
pub(super) fn index_unmodeled_lookup_mutations(app: &App, index: &mut SourceContractIndex<'_>) {
    let constructor_owner_aliases = constructor_owner_aliases(
        super::classes(app)
            .map(|class| class.name.clone())
            .chain(app.models.iter().map(|model| model.name.clone()))
            .chain(app.test_modules.iter().map(|module| module.name.clone())),
    );
    let known_modules: HashSet<ClassId> = super::classes(app)
        .filter(|class| class.is_module)
        .map(|class| class.name.clone())
        .collect();
    let modules_with_custom_new: HashSet<ClassId> = index
        .virtual_owners
        .iter()
        .filter(|owner| {
            index
                .declaration(
                    owner,
                    &Symbol::from("new"),
                    MethodReceiver::Instance,
                    &mut HashSet::new(),
                )
                .is_some()
        })
        .map(|owner| (*owner).clone())
        .collect();
    let modules_with_custom_initialize: HashSet<ClassId> = index
        .virtual_owners
        .iter()
        .filter(|owner| {
            index
                .declaration(
                    owner,
                    &Symbol::from("initialize"),
                    MethodReceiver::Instance,
                    &mut HashSet::new(),
                )
                .is_some()
        })
        .map(|owner| (*owner).clone())
        .collect();
    let mut modules_with_mutating_hooks = initial_mutating_hook_modules(index, &known_modules);
    resolve_mutating_hook_modules(
        app,
        &constructor_owner_aliases,
        &modules_with_custom_new,
        &modules_with_custom_initialize,
        &known_modules,
        &mut modules_with_mutating_hooks,
    );
    let context = ConstructorMutationContext {
        owner_aliases: &constructor_owner_aliases,
        modules_with_custom_new: &modules_with_custom_new,
        modules_with_custom_initialize: &modules_with_custom_initialize,
        known_modules: &known_modules,
        modules_with_mutating_hooks: &modules_with_mutating_hooks,
    };
    let findings = collect_app_lookup_mutations(app, &context);
    index
        .unmodeled_constructor_lookup
        .extend(findings.hook_owners);
    index
        .unmodeled_constructor_lookup
        .extend(findings.mutated_owners);
    index
        .unmodeled_constructor_lookup
        .extend(findings.explicit_mutation_targets);
}

fn initial_mutating_hook_modules(
    index: &SourceContractIndex<'_>,
    known_modules: &HashSet<ClassId>,
) -> HashSet<ClassId> {
    fn module_graph_is_complete(
        index: &SourceContractIndex<'_>,
        owner: &ClassId,
        known_modules: &HashSet<ClassId>,
        seen: &mut HashSet<ClassId>,
    ) -> bool {
        if !known_modules.contains(owner) {
            return false;
        }
        if !seen.insert(owner.clone()) {
            return true;
        }
        index
            .includes(owner)
            .iter()
            .all(|included| module_graph_is_complete(index, included, known_modules, seen))
    }
    known_modules
        .iter()
        .filter(|module| {
            !module_graph_is_complete(index, module, known_modules, &mut HashSet::new())
        })
        .cloned()
        .collect()
}

fn resolve_mutating_hook_modules(
    app: &App,
    owner_aliases: &ConstructorOwnerAliases,
    modules_with_custom_new: &HashSet<ClassId>,
    modules_with_custom_initialize: &HashSet<ClassId>,
    known_modules: &HashSet<ClassId>,
    modules_with_mutating_hooks: &mut HashSet<ClassId>,
) {
    loop {
        let previous_len = modules_with_mutating_hooks.len();
        for class in super::classes(app).filter(|class| class.is_module) {
            for method in &class.methods {
                if method.receiver == MethodReceiver::Class
                    && is_method_lookup_hook(method.name.as_str())
                {
                    modules_with_mutating_hooks.insert(class.name.clone());
                    continue;
                }
                if !matches!(method.name.as_str(), "included" | "extended" | "prepended") {
                    continue;
                }
                let Some(callback_receiver) = method.params.first().map(|param| &param.name) else {
                    continue;
                };
                let context = ConstructorMutationContext {
                    owner_aliases,
                    modules_with_custom_new,
                    modules_with_custom_initialize,
                    known_modules,
                    modules_with_mutating_hooks,
                };
                let scan = scan_constructor_mutations(
                    &method.body,
                    &context,
                    method.receiver == MethodReceiver::Class,
                    Some(callback_receiver),
                );
                if scan.owner_mutated {
                    modules_with_mutating_hooks.insert(class.name.clone());
                }
            }
        }
        if modules_with_mutating_hooks.len() == previous_len {
            break;
        }
    }
}

#[derive(Default)]
struct AppLookupMutationFindings {
    hook_owners: HashSet<ClassId>,
    mutated_owners: HashSet<ClassId>,
    explicit_mutation_targets: HashSet<ClassId>,
}

fn collect_app_lookup_mutations(
    app: &App,
    context: &ConstructorMutationContext<'_>,
) -> AppLookupMutationFindings {
    let mut findings = AppLookupMutationFindings::default();
    let mut scan_mutations = |expr: &Expr,
                              owner: Option<&ClassId>,
                              class_receiver: bool,
                              callback_receiver: Option<&Symbol>| {
        let scan = scan_constructor_mutations(expr, context, class_receiver, callback_receiver);
        findings
            .explicit_mutation_targets
            .extend(scan.explicit_targets);
        if scan.owner_mutated {
            if let Some(owner) = owner {
                findings.mutated_owners.insert(owner.clone());
            }
        }
    };
    for class in super::classes(app) {
        if class.methods.iter().any(|method| {
            method.receiver == MethodReceiver::Class && is_method_lookup_hook(method.name.as_str())
        }) {
            findings.hook_owners.insert(class.name.clone());
        }
        for expr in &class.unknown_calls {
            scan_mutations(expr, Some(&class.name), true, None);
        }
        for method in &class.methods {
            let callback_receiver = matches!(
                method.name.as_str(),
                "included" | "inherited" | "extended" | "prepended"
            )
            .then(|| method.params.first().map(|param| &param.name))
            .flatten();
            scan_mutations(
                &method.body,
                Some(&class.name),
                method.receiver == MethodReceiver::Class,
                callback_receiver,
            );
            for default in method
                .params
                .iter()
                .filter_map(|param| param.default.as_ref())
            {
                scan_mutations(
                    default,
                    Some(&class.name),
                    method.receiver == MethodReceiver::Class,
                    callback_receiver,
                );
            }
        }
    }
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item {
                scan_mutations(expr, Some(&model.name), true, None);
            }
        }
        for method in model.methods() {
            scan_mutations(
                &method.body,
                Some(&model.name),
                method.receiver == MethodReceiver::Class,
                None,
            );
            scan_method_defaults(method, &mut scan_mutations, Some(&model.name));
        }
    }
    for test_module in &app.test_modules {
        if let Some(setup) = &test_module.setup {
            scan_mutations(setup, None, false, None);
        }
        for test in &test_module.tests {
            scan_mutations(&test.body, None, false, None);
        }
        for method in &test_module.helpers {
            scan_mutations(
                &method.body,
                None,
                method.receiver == MethodReceiver::Class,
                None,
            );
            scan_method_defaults(method, &mut scan_mutations, None);
        }
    }
    findings
}

fn is_method_lookup_hook(name: &str) -> bool {
    matches!(
        name,
        "method_added"
            | "method_removed"
            | "method_undefined"
            | "singleton_method_added"
            | "singleton_method_removed"
            | "singleton_method_undefined"
    )
}

fn scan_method_defaults(
    method: &MethodDef,
    scan_mutations: &mut impl FnMut(&Expr, Option<&ClassId>, bool, Option<&Symbol>),
    owner: Option<&ClassId>,
) {
    for default in method
        .params
        .iter()
        .filter_map(|param| param.default.as_ref())
    {
        scan_mutations(
            default,
            owner,
            method.receiver == MethodReceiver::Class,
            None,
        );
    }
}

fn constructor_contract<'a>(
    contracts: &SourceContractIndex<'a>,
    owner: &ClassId,
) -> Option<ConstructorContract<'a>> {
    if !contracts.verified_hierarchy(owner, &mut HashSet::new()) {
        return Some(ConstructorContract::UnknownLookup);
    }
    if inherits_unmodeled_constructor_lookup(contracts, owner, &mut HashSet::new()) {
        // An unmodeled mixin with no source constructor anywhere in the
        // modeled lookup chain: nothing was flattened that a verbatim
        // keyword call could mis-bind to, so keep the call as written.
        if !declares_source_constructor(contracts, owner, &mut HashSet::new()) {
            return None;
        }
        return Some(ConstructorContract::UnknownLookup);
    }
    if let Some((method, _)) = contracts.declaration(
        owner,
        &Symbol::from("new"),
        MethodReceiver::Class,
        &mut HashSet::new(),
    ) {
        return Some(ConstructorContract::CustomNew(Some(method)));
    }
    let (method, _) =
        contracts.effective_call(owner, &Symbol::from("new"), MethodReceiver::Class)?;
    if method.name.as_str() == "new" {
        Some(ConstructorContract::CustomNew(None))
    } else {
        Some(ConstructorContract::Initialize(method))
    }
}

fn declares_source_constructor(
    contracts: &SourceContractIndex<'_>,
    owner: &ClassId,
    seen: &mut HashSet<ClassId>,
) -> bool {
    if !seen.insert(owner.clone()) {
        return false;
    }
    contracts.unmodeled_constructor_lookup.contains(owner)
        || contracts
            .instance
            .contains_key(&(owner.clone(), Symbol::from("initialize")))
        || contracts
            .class
            .contains_key(&(owner.clone(), Symbol::from("new")))
        || contracts
            .includes(owner)
            .iter()
            .filter(|included| contracts.modules.contains(*included))
            .any(|included| declares_source_constructor(contracts, included, seen))
        || contracts
            .parent(owner)
            .is_some_and(|parent| declares_source_constructor(contracts, parent, seen))
}

fn inherits_unmodeled_constructor_lookup(
    contracts: &SourceContractIndex<'_>,
    owner: &ClassId,
    seen: &mut HashSet<ClassId>,
) -> bool {
    if !seen.insert(owner.clone()) {
        return false;
    }
    contracts.unmodeled_constructor_lookup.contains(owner)
        || contracts.includes(owner).iter().any(|included| {
            !contracts.modules.contains(included)
                || inherits_unmodeled_constructor_lookup(contracts, included, seen)
        })
        || contracts
            .parent(owner)
            .is_some_and(|parent| inherits_unmodeled_constructor_lookup(contracts, parent, seen))
}

#[derive(Default)]
pub(super) struct ConstructorMutationScan {
    pub(super) owner_mutated: bool,
    pub(super) explicit_targets: HashSet<ClassId>,
}

pub(super) fn scan_constructor_mutations(
    expr: &Expr,
    context: &ConstructorMutationContext<'_>,
    owner_is_class_receiver: bool,
    callback_receiver: Option<&Symbol>,
) -> ConstructorMutationScan {
    fn receiver_uses_callback(receiver: &Expr, callback_receiver: &Symbol) -> bool {
        match &*receiver.node {
            ExprNode::Var { name, .. } => name == callback_receiver,
            ExprNode::Send {
                recv: Some(recv), ..
            } => receiver_uses_callback(recv, callback_receiver),
            _ => false,
        }
    }
    fn contains_callback(expr: &Expr, callback_receiver: &Symbol) -> bool {
        if matches!(&*expr.node, ExprNode::Var { name, .. } if name == callback_receiver) {
            return true;
        }
        let mut found = false;
        expr.node
            .for_each_child(&mut |child| found |= contains_callback(child, callback_receiver));
        found
    }

    fn visit(
        expr: &Expr,
        context: &ConstructorMutationContext<'_>,
        owner_is_class_receiver: bool,
        callback_receiver: Option<&Symbol>,
        result: &mut ConstructorMutationScan,
    ) {
        if let ExprNode::Send {
            recv, method, args, ..
        } = &*expr.node
        {
            let mutates_lookup = mutates_constructor_lookup_method(
                method.as_str(),
                args,
                context.modules_with_custom_new,
                context.modules_with_custom_initialize,
                context.known_modules,
                context.modules_with_mutating_hooks,
            );
            let mutates_callback_receiver = callback_receiver.is_some_and(|callback_receiver| {
                recv.as_ref()
                    .is_some_and(|recv| receiver_uses_callback(recv, callback_receiver))
                    || args
                        .iter()
                        .any(|arg| contains_callback(arg, callback_receiver))
            });
            if mutates_lookup || mutates_callback_receiver {
                match recv.as_ref().map(|recv| &*recv.node) {
                    None | Some(ExprNode::SelfRef) if owner_is_class_receiver => {
                        result.owner_mutated = true;
                    }
                    Some(ExprNode::Var { name, .. })
                        if owner_is_class_receiver
                            && callback_receiver.is_some_and(|target| target == name) =>
                    {
                        result.owner_mutated = true;
                    }
                    Some(ExprNode::Send { .. })
                        if owner_is_class_receiver && mutates_callback_receiver =>
                    {
                        result.owner_mutated = true;
                    }
                    Some(ExprNode::Const { path }) => {
                        let rooted = path.first().is_some_and(|part| part.as_str().is_empty());
                        let spelling = path
                            .iter()
                            .filter(|part| !part.as_str().is_empty())
                            .map(|part| part.as_str())
                            .collect::<Vec<_>>()
                            .join("::");
                        result
                            .explicit_targets
                            .extend(context.owner_aliases.resolve(&spelling, rooted).cloned());
                    }
                    _ => {}
                }
            }
        }
        expr.node.for_each_child(&mut |child| {
            visit(
                child,
                context,
                owner_is_class_receiver,
                callback_receiver,
                result,
            )
        });
    }
    let mut result = ConstructorMutationScan::default();
    visit(
        expr,
        context,
        owner_is_class_receiver,
        callback_receiver,
        &mut result,
    );
    result
}

pub(super) struct ConstructorMutationContext<'a> {
    pub(super) owner_aliases: &'a ConstructorOwnerAliases,
    pub(super) modules_with_custom_new: &'a HashSet<ClassId>,
    pub(super) modules_with_custom_initialize: &'a HashSet<ClassId>,
    pub(super) known_modules: &'a HashSet<ClassId>,
    pub(super) modules_with_mutating_hooks: &'a HashSet<ClassId>,
}

pub(super) struct ConstructorOwnerAliases {
    exact: HashMap<String, HashSet<ClassId>>,
    suffixes: HashMap<String, HashSet<ClassId>>,
}

impl ConstructorOwnerAliases {
    fn resolve(&self, spelling: &str, rooted: bool) -> impl Iterator<Item = &ClassId> {
        let exact = self
            .exact
            .get(spelling)
            .into_iter()
            .flat_map(|owners| owners.iter());
        let suffixes = (!rooted)
            .then(|| {
                self.suffixes
                    .get(spelling)
                    .into_iter()
                    .flat_map(|owners| owners.iter())
            })
            .into_iter()
            .flatten();
        exact.chain(suffixes)
    }
}

pub(super) fn constructor_owner_aliases(
    owners: impl Iterator<Item = ClassId>,
) -> ConstructorOwnerAliases {
    let mut exact: HashMap<String, HashSet<ClassId>> = HashMap::new();
    let mut suffixes: HashMap<String, HashSet<ClassId>> = HashMap::new();
    for owner in owners {
        exact
            .entry(owner.0.as_str().to_string())
            .or_default()
            .insert(owner.clone());
        let parts: Vec<_> = owner.0.as_str().split("::").collect();
        for start in 1..parts.len() {
            suffixes
                .entry(parts[start..].join("::"))
                .or_default()
                .insert(owner.clone());
        }
    }
    ConstructorOwnerAliases { exact, suffixes }
}

fn literal_method_name(expr: &Expr) -> Option<&str> {
    match &*expr.node {
        ExprNode::Lit {
            value: crate::expr::Literal::Sym { value },
        } => Some(value.as_str()),
        ExprNode::Lit {
            value: crate::expr::Literal::Str { value },
        } => Some(value.as_str()),
        _ => None,
    }
}

fn mutates_constructor_lookup_method(
    method: &str,
    args: &[Expr],
    modules_with_custom_new: &HashSet<ClassId>,
    modules_with_custom_initialize: &HashSet<ClassId>,
    known_modules: &HashSet<ClassId>,
    modules_with_mutating_hooks: &HashSet<ClassId>,
) -> bool {
    if matches!(method, "send" | "public_send") {
        let Some((dynamic_method, rest)) = args.split_first() else {
            return false;
        };
        let Some(dynamic_method) = literal_method_name(dynamic_method) else {
            return false;
        };
        if matches!(dynamic_method, "new" | "initialize") {
            return true;
        }
        return mutates_constructor_lookup_method(
            dynamic_method,
            rest,
            modules_with_custom_new,
            modules_with_custom_initialize,
            known_modules,
            modules_with_mutating_hooks,
        );
    }
    match method {
        "extend" => args
            .iter()
            .filter(|arg| {
                !super::constant_names_class(arg, &ClassId(Symbol::from("ActiveSupport::Concern")))
            })
            .any(|arg| {
                modules_with_custom_new
                    .iter()
                    .chain(modules_with_mutating_hooks)
                    .any(|module| super::constant_names_class(arg, module))
                    || !known_modules
                        .iter()
                        .any(|module| super::constant_names_class(arg, module))
            }),
        "include" => args.iter().any(|arg| {
            modules_with_custom_initialize
                .iter()
                .chain(modules_with_mutating_hooks)
                .any(|module| super::constant_names_class(arg, module))
                || !known_modules
                    .iter()
                    .any(|module| super::constant_names_class(arg, module))
        }),
        // `prepend` can be written on either an instance class or its
        // singleton class. The normalized call no longer carries that
        // syntactic context, so fail closed for either constructor slot.
        "prepend" => args.iter().any(|arg| {
            modules_with_custom_new
                .iter()
                .chain(modules_with_custom_initialize)
                .chain(modules_with_mutating_hooks)
                .any(|module| super::constant_names_class(arg, module))
                || !known_modules
                    .iter()
                    .any(|module| super::constant_names_class(arg, module))
        }),
        "define_method"
        | "define_singleton_method"
        | "class_eval"
        | "module_eval"
        | "instance_eval"
        | "class_exec"
        | "instance_exec"
        | "remove_method"
        | "undef_method" => true,
        "alias_method" => args.iter().any(|arg| {
            literal_method_name(arg).is_some_and(|name| matches!(name, "new" | "initialize"))
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::constructor_owner_aliases;
    use crate::ident::{ClassId, Symbol};

    #[test]
    fn relative_constructor_receiver_keeps_lexical_suffix_candidates() {
        let owners = [
            ClassId(Symbol::from("A::Item")),
            ClassId(Symbol::from("Outer::A::Item")),
        ];
        let aliases = constructor_owner_aliases(owners.iter().cloned());
        let relative: HashSet<_> = aliases
            .resolve("A::Item", false)
            .map(|owner| owner.0.as_str())
            .collect();
        assert_eq!(relative, HashSet::from(["A::Item", "Outer::A::Item"]));
        let short: HashSet<_> = aliases
            .resolve("Item", false)
            .map(|owner| owner.0.as_str())
            .collect();
        assert_eq!(short, HashSet::from(["A::Item", "Outer::A::Item"]));
        let rooted: Vec<_> = aliases
            .resolve("A::Item", true)
            .map(|owner| owner.0.as_str())
            .collect();
        assert_eq!(rooted, ["A::Item"]);
    }
}
