//! Source-level `Class.new` lookup and the effective `initialize` contract.
//!
//! Constructor lowering must use Ruby's source dispatch, not the analyzer's
//! flattened parameter signature: custom class-side `new`, mixins, mutation
//! hooks and method-lookup edits can all invalidate the default forwarder.

use crate::det::{HashMap, HashSet};

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
    // Every method name the app defines, on any class, module or model: a
    // send of one on a hook's callback receiver runs app code, which may
    // change the receiver's lookup (`base.install_factory` after
    // `base.extend(Factory)`).
    let app_methods: HashSet<Symbol> = super::classes(app)
        .flat_map(|class| class.methods.iter())
        .chain(app.models.iter().flat_map(|model| model.methods()))
        .chain(app.test_modules.iter().flat_map(|module| module.helpers.iter()))
        .map(|method| method.name.clone())
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
                    &mut HashSet::default(),
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
                    &mut HashSet::default(),
                )
                .is_some()
        })
        .map(|owner| (*owner).clone())
        .collect();
    let known_module_names = KnownModules::new(&known_modules);
    // Asked twice: whether anything may replace `new` or supply the
    // `initialize` it runs (the owner's whole constructor), and whether
    // anything may replace `new` itself. An own `initialize` shadows what a
    // mixin supplies, never what replaces `new`
    // (`shadows_supplied_initialize`).
    let no_initializers = HashSet::default();
    for question in [Question::NewOrInitialize, Question::New] {
        let modules_with_custom_initialize = match question {
            Question::NewOrInitialize => &modules_with_custom_initialize,
            Question::New => &no_initializers,
        };
        let mut modules_with_mutating_hooks = initial_mutating_hook_modules(index, &known_modules, question);
        resolve_mutating_hook_modules(
            app,
            &constructor_owner_aliases,
            &modules_with_custom_new,
            modules_with_custom_initialize,
            &known_module_names,
            &app_methods,
            question,
            &mut modules_with_mutating_hooks,
        );
        let hook_targets = HookTargets::new(
            &modules_with_custom_new,
            modules_with_custom_initialize,
            &modules_with_mutating_hooks,
        );
        let context = ConstructorMutationContext {
            owner_aliases: &constructor_owner_aliases,
            modules_with_custom_new: &modules_with_custom_new,
            modules_with_custom_initialize,
            known_modules: &known_module_names,
            app_methods: &app_methods,
            modules_with_mutating_hooks: &modules_with_mutating_hooks,
            hook_targets: Some(&hook_targets),
            question,
        };
        let findings = collect_app_lookup_mutations(app, &context);
        let set = match question {
            Question::NewOrInitialize => &mut index.unmodeled_constructor_lookup,
            Question::New => &mut index.unmodeled_new_lookup,
        };
        set.extend(findings.hook_owners);
        set.extend(findings.mutated_owners);
        set.extend(findings.explicit_mutation_targets);
    }
    index.shadowing_initializers = shadows_supplied_initialize(app);
}

/// Owners whose own `initialize` comes first in Ruby's lookup and never
/// passes the call on, so an `initialize` a mixin supplies is never run:
/// every `initialize` the owner defines is a plain `def initialize` in a
/// body that declares the owner (the class body or a reopening; not one a
/// concern's `included do` splices in, an alias, or `define_method`, which
/// the lookup scan already refuses), none of them contains `super` in any
/// form, and nothing is prepended to the owner, in its body or by an
/// initializer (`Widget.prepend(Mod)`, `App::module_mixins`): a prepended
/// module comes before it. Anything else is not shadowing.
fn shadows_supplied_initialize(app: &App) -> HashSet<ClassId> {
    let resolver = app.const_resolver.for_sources(&app.sources);
    let initialize = Symbol::from("initialize");
    let plain = |owner: &ClassId, method: &MethodDef| {
        let span = method.name_span;
        !span.is_synthetic()
            // A `def`'s name token; an alias copy spans its statement.
            && (span.end - span.start) as usize == initialize.as_str().len()
            && resolver.namespaces_declared_in(span.file).any(|name| name == owner.0.as_str())
            && !contains_super(&method.body)
            && method.params.iter().all(|param| param.default.as_ref().is_none_or(|d| !contains_super(d)))
    };
    let prepends = |expr: &Expr| matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "prepend");
    // An initializer names its target as written; a full name or a
    // trailing segment that could be the owner counts, failing closed.
    let prepended_elsewhere = |owner: &ClassId| {
        let full = owner.0.as_str();
        app.module_mixins.iter().any(|mixin| {
            let target = mixin.target.as_str().trim_start_matches("::");
            mixin.kind == crate::app::MixinKind::Prepend
                && (target == full || full.ends_with(&format!("::{target}")) || target.ends_with(&format!("::{full}")))
        })
    };
    let mut out = HashSet::default();
    for class in super::classes(app).filter(|class| !class.is_module) {
        let defs: Vec<&MethodDef> = class
            .methods
            .iter()
            .filter(|m| m.receiver == MethodReceiver::Instance && m.name == initialize)
            .collect();
        if !defs.is_empty()
            && defs.iter().all(|m| plain(&class.name, m))
            && !class.unknown_calls.iter().any(prepends)
            && !prepended_elsewhere(&class.name)
        {
            out.insert(class.name.clone());
        }
    }
    for model in &app.models {
        let defs: Vec<&MethodDef> =
            model.methods().filter(|m| m.receiver == MethodReceiver::Instance && m.name == initialize).collect();
        let prepended = model.body.iter().any(|item| {
            matches!(item, crate::dialect::ModelBodyItem::Unknown { expr, .. } if prepends(expr))
        });
        if !defs.is_empty() && defs.iter().all(|m| plain(&model.name, m)) && !prepended && !prepended_elsewhere(&model.name) {
            out.insert(model.name.clone());
        }
    }
    out
}

/// Any `super` in `expr`: bare, `super()`, `super(...)`, in a block or a
/// rescue. Reachability is not asked.
fn contains_super(expr: &Expr) -> bool {
    if matches!(&*expr.node, ExprNode::Super { .. }) {
        return true;
    }
    let mut found = false;
    expr.node.for_each_child(&mut |child| found = found || contains_super(child));
    found
}

/// Which part of a constructor a lookup question covers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Question {
    /// `new` and the `initialize` it runs.
    NewOrInitialize,
    /// `new` alone.
    New,
}

/// Framework mixins verified not to replace `Klass.new` through a hook
/// (Rails 8.1.4, Sidekiq 8.1.7; oracle `framework_includes_new.rb`):
/// included into a class, its `new` is still `Class#new` and no module the
/// include adds to the singleton chain defines `new` or `allocate`. The
/// flag is whether an includer without an `initialize` of its own gets one
/// from it (`url_helpers` brings `ActionDispatch::Routing::UrlFor`'s,
/// `ActiveModel::Model` brings `ActiveModel::API`'s).
///
/// This answers only the constructor-lookup question. These modules are
/// not source modules for anything else (method resolution, keyword
/// owners, …). Re-verify the oracle on a Rails or Sidekiq upgrade.
const MIXINS_WITHOUT_CONSTRUCTOR_HOOK: &[(&str, bool)] = &[
    ("ActionView::Helpers::AssetTagHelper", false),
    ("ActionView::Helpers::NumberHelper", false),
    ("ActionView::Helpers::TextHelper", false),
    ("ActionView::Helpers::TagHelper", false),
    ("ActionView::Helpers::UrlHelper", false),
    ("ActionView::Helpers::SanitizeHelper", false),
    ("ActionView::Helpers::JavaScriptHelper", false),
    ("Rails.application.routes.url_helpers", true),
    ("ActiveModel::Model", true),
    ("ActiveModel::Validations", false),
    ("ActiveModel::Attributes", true),
    ("Sidekiq::Worker", false),
];

/// Whether `name` is a catalog mixin that leaves `question` unaffected.
fn catalog_leaves(name: &str, question: Question) -> bool {
    MIXINS_WITHOUT_CONSTRUCTOR_HOOK.iter().any(|(entry, brings_initialize)| {
        *entry == name && (question == Question::New || !brings_initialize)
    })
}

/// The catalog spelling of an `include` argument: a constant path, or the
/// `Rails.application.routes.url_helpers` call chain.
fn mixin_spelling(arg: &Expr) -> Option<String> {
    match &*arg.node {
        ExprNode::Const { path } => Some(
            path.iter().filter(|part| !part.as_str().is_empty()).map(|part| part.as_str()).collect::<Vec<_>>().join("::"),
        ),
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. } if args.is_empty() => {
            let ExprNode::Send { recv: Some(recv), method: routes, args, block: None, .. } = &*recv.node else { return None };
            let ExprNode::Send { recv: Some(recv), method: application, args: app_args, block: None, .. } = &*recv.node else { return None };
            let ExprNode::Const { path } = &*recv.node else { return None };
            (args.is_empty()
                && app_args.is_empty()
                && method.as_str() == "url_helpers"
                && routes.as_str() == "routes"
                && application.as_str() == "application"
                && path.iter().filter(|p| !p.as_str().is_empty()).map(|p| p.as_str()).eq(["Rails"]))
            .then(|| "Rails.application.routes.url_helpers".to_string())
        }
        _ => None,
    }
}

fn initial_mutating_hook_modules(
    index: &SourceContractIndex<'_>,
    known_modules: &HashSet<ClassId>,
    question: Question,
) -> HashSet<ClassId> {
    fn module_graph_is_complete(
        index: &SourceContractIndex<'_>,
        owner: &ClassId,
        known_modules: &HashSet<ClassId>,
        question: Question,
        seen: &mut HashSet<ClassId>,
    ) -> bool {
        if catalog_leaves(owner.0.as_str(), question) {
            return true;
        }
        if !known_modules.contains(owner) {
            return false;
        }
        if !seen.insert(owner.clone()) {
            return true;
        }
        index
            .includes(owner)
            .iter()
            .all(|included| module_graph_is_complete(index, included, known_modules, question, seen))
    }
    known_modules
        .iter()
        .filter(|module| {
            !module_graph_is_complete(index, module, known_modules, question, &mut HashSet::default())
        })
        .cloned()
        .collect()
}

fn resolve_mutating_hook_modules(
    app: &App,
    owner_aliases: &ConstructorOwnerAliases,
    modules_with_custom_new: &HashSet<ClassId>,
    modules_with_custom_initialize: &HashSet<ClassId>,
    known_modules: &KnownModules,
    app_methods: &HashSet<Symbol>,
    question: Question,
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
                    app_methods,
                    modules_with_mutating_hooks,
                    hook_targets: None,
                    question,
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

/// Names that can replace `Class#new` without themselves being `new` or
/// `initialize`: the lookup hooks, and the callbacks that receive another
/// class and can install those hooks. `define_method(:summary)` stays safe.
fn defines_constructor_lookup_callback(name: &str) -> bool {
    is_method_lookup_hook(name)
        || matches!(name, "included" | "inherited" | "extended" | "prepended")
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
    if !contracts.verified_hierarchy(owner, &mut HashSet::default()) {
        return Some(ConstructorContract::UnknownLookup);
    }
    // `new` itself may be replaced: no own `initialize` changes that.
    // Otherwise only a mixin's `initialize` is unmodeled, and the owner's
    // own plain one, which never passes the call on, shadows it.
    let unmodeled = inherits_unmodeled_constructor_lookup(contracts, owner, Question::New, &mut HashSet::default())
        || (inherits_unmodeled_constructor_lookup(contracts, owner, Question::NewOrInitialize, &mut HashSet::default())
            && !contracts.shadowing_initializers.contains(owner));
    if unmodeled {
        // An unmodeled mixin with no source constructor anywhere in the
        // modeled lookup chain: nothing was flattened that a verbatim
        // keyword call could mis-bind to, so keep the call as written.
        if !declares_source_constructor(contracts, owner, &mut HashSet::default()) {
            return None;
        }
        return Some(ConstructorContract::UnknownLookup);
    }
    if let Some((method, _)) = contracts.declaration(
        owner,
        &Symbol::from("new"),
        MethodReceiver::Class,
        &mut HashSet::default(),
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
    question: Question,
    seen: &mut HashSet<ClassId>,
) -> bool {
    if !seen.insert(owner.clone()) {
        return false;
    }
    let unmodeled = match question {
        Question::NewOrInitialize => &contracts.unmodeled_constructor_lookup,
        Question::New => &contracts.unmodeled_new_lookup,
    };
    unmodeled.contains(owner)
        || contracts.includes(owner).iter().any(|included| {
            (!contracts.modules.contains(included) && !catalog_leaves(included.0.as_str(), question))
                || inherits_unmodeled_constructor_lookup(contracts, included, question, seen)
        })
        || contracts
            .parent(owner)
            .is_some_and(|parent| inherits_unmodeled_constructor_lookup(contracts, parent, question, seen))
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
                context.hook_targets,
                context.question,
            );
            // A send on the callback receiver mutates its lookup only when
            // the method is one that can (`mutates_lookup`) or one the app
            // defines, whose body may (`base.install_factory` after
            // `base.extend(Factory)`, or `base.send(:install_factory)`;
            // see `sends_app_method`): reading it (`base.methods`,
            // `base.respond_to?`) or calling a framework macro on it
            // (`base.before_action`) does not, the same rule as for a named
            // receiver. A send that passes the receiver on as an argument
            // keeps today's handling.
            let mutates_callback_receiver = callback_receiver.is_some_and(|callback_receiver| {
                ((mutates_lookup || sends_app_method(method, args, context.app_methods))
                    && recv
                        .as_ref()
                        .is_some_and(|recv| receiver_uses_callback(recv, callback_receiver)))
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
    pub(super) known_modules: &'a KnownModules,
    /// Every method name the app defines (`index_unmodeled_lookup_mutations`).
    pub(super) app_methods: &'a HashSet<Symbol>,
    pub(super) modules_with_mutating_hooks: &'a HashSet<ClassId>,
    /// The module sets above, probed by spelling: once they are final.
    pub(super) hook_targets: Option<&'a HookTargets>,
    pub(super) question: Question,
}

/// The modules a mixin call names that can change constructor lookup, per
/// call: `extend` (custom `new`, lookup hooks), `include` (custom
/// `initialize`, lookup hooks), `prepend` (either, lookup hooks). One probe
/// per constant instead of a scan of every module per argument.
pub(super) struct HookTargets {
    extend: KnownModules,
    include: KnownModules,
    prepend: KnownModules,
}

impl HookTargets {
    fn new(
        custom_new: &HashSet<ClassId>,
        custom_initialize: &HashSet<ClassId>,
        hooks: &HashSet<ClassId>,
    ) -> Self {
        let union = |sets: &[&HashSet<ClassId>]| {
            KnownModules::new(&sets.iter().flat_map(|set| set.iter().cloned()).collect())
        };
        Self {
            extend: union(&[custom_new, hooks]),
            include: union(&[custom_initialize, hooks]),
            prepend: union(&[custom_new, custom_initialize, hooks]),
        }
    }
}

/// "Does a constant name some known module" (`constant_names_class` against
/// each module) answered by one hash probe: a constant names a module when
/// its spelling is the module's whole name or a `::`-suffix of it, so every
/// such suffix of every module is stored.
pub(super) struct KnownModules {
    spellings: HashSet<String>,
    modules: Vec<ClassId>,
}

impl KnownModules {
    fn new(modules: &HashSet<ClassId>) -> Self {
        let mut spellings = HashSet::default();
        for module in modules {
            let parts: Vec<_> = module.0.as_str().split("::").collect();
            for start in 0..parts.len() {
                spellings.insert(parts[start..].join("::"));
            }
        }
        Self { spellings, modules: modules.iter().cloned().collect() }
    }

    fn name(&self, constant: &Expr) -> bool {
        match super::constant_spelling(constant) {
            None => false,
            // The empty spelling matches by the scan's own rule.
            Some(spelling) if spelling.is_empty() => self
                .modules
                .iter()
                .any(|module| super::constant_names_class(constant, module)),
            Some(spelling) => self.spellings.contains(&spelling),
        }
    }
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
    let mut exact: HashMap<String, HashSet<ClassId>> = HashMap::default();
    let mut suffixes: HashMap<String, HashSet<ClassId>> = HashMap::default();
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

/// Whether `method(args)` runs a method the app defines: by name, or
/// through `send` / `public_send` / `__send__` with a literal name. A
/// dynamic send with any other name may run one, so it counts.
fn sends_app_method(method: &Symbol, args: &[Expr], app_methods: &HashSet<Symbol>) -> bool {
    if matches!(method.as_str(), "send" | "public_send" | "__send__") {
        return match args.first().map(literal_method_name) {
            Some(Some(name)) => app_methods.contains(&Symbol::from(name)),
            _ => true,
        };
    }
    app_methods.contains(method)
}

fn mutates_constructor_lookup_method(
    method: &str,
    args: &[Expr],
    modules_with_custom_new: &HashSet<ClassId>,
    modules_with_custom_initialize: &HashSet<ClassId>,
    known_modules: &KnownModules,
    modules_with_mutating_hooks: &HashSet<ClassId>,
    hook_targets: Option<&HookTargets>,
    question: Question,
) -> bool {
    if matches!(method, "send" | "public_send" | "__send__") {
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
            hook_targets,
            question,
        );
    }
    match method {
        "extend" => args
            .iter()
            .filter(|arg| {
                !super::constant_names_class(arg, &ClassId(Symbol::from("ActiveSupport::Concern")))
            })
            .any(|arg| {
                (match hook_targets {
                    Some(targets) => targets.extend.name(arg),
                    None => modules_with_custom_new
                        .iter()
                        .chain(modules_with_mutating_hooks)
                        .any(|module| super::constant_names_class(arg, module)),
                }) || !known_modules.name(arg)
            }),
        "include" => args.iter().any(|arg| {
            if mixin_spelling(arg).is_some_and(|name| catalog_leaves(&name, question)) {
                return false;
            }
            (match hook_targets {
                Some(targets) => targets.include.name(arg),
                None => modules_with_custom_initialize
                    .iter()
                    .chain(modules_with_mutating_hooks)
                    .any(|module| super::constant_names_class(arg, module)),
            }) || !known_modules.name(arg)
        }),
        // `prepend` can be written on either an instance class or its
        // singleton class. The normalized call no longer carries that
        // syntactic context, so fail closed for either constructor slot.
        "prepend" => args.iter().any(|arg| {
            (match hook_targets {
                Some(targets) => targets.prepend.name(arg),
                None => modules_with_custom_new
                    .iter()
                    .chain(modules_with_custom_initialize)
                    .chain(modules_with_mutating_hooks)
                    .any(|module| super::constant_names_class(arg, module)),
            }) || !known_modules.name(arg)
        }),
        "define_method" | "define_singleton_method" => args.first().is_none_or(|arg| {
            literal_method_name(arg).is_none_or(|name| {
                matches!(name, "new" | "initialize") || defines_constructor_lookup_callback(name)
            })
        }),
        "remove_method" | "undef_method" => args.iter().any(|arg| {
            literal_method_name(arg).is_none_or(|name| {
                matches!(name, "new" | "initialize") || defines_constructor_lookup_callback(name)
            })
        }),
        "class_eval"
        | "module_eval"
        | "instance_eval"
        | "class_exec"
        | "instance_exec" => true,
        "alias_method" => args.iter().any(|arg| {
            literal_method_name(arg).is_some_and(|name| matches!(name, "new" | "initialize"))
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::det::HashSet;

    use super::super::constant_spelling;
    use super::{KnownModules, constructor_owner_aliases};
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
        assert_eq!(relative, HashSet::from_iter(["A::Item", "Outer::A::Item"]));
        let short: HashSet<_> = aliases
            .resolve("Item", false)
            .map(|owner| owner.0.as_str())
            .collect();
        assert_eq!(short, HashSet::from_iter(["A::Item", "Outer::A::Item"]));
        let rooted: Vec<_> = aliases
            .resolve("A::Item", true)
            .map(|owner| owner.0.as_str())
            .collect();
        assert_eq!(rooted, ["A::Item"]);
    }

    #[test]
    fn known_modules_answer_what_the_per_module_scan_answers() {
        use crate::expr::{Expr, ExprNode};
        let modules: HashSet<ClassId> = ["A::B::C", "X", "Outer::Concern", "Y::Z"]
            .iter()
            .map(|name| ClassId(Symbol::from(*name)))
            .collect();
        let known = KnownModules::new(&modules);
        for spelling in [
            vec!["A", "B", "C"],
            vec!["B", "C"],
            vec!["C"],
            vec!["A", "B"],
            vec!["B"],
            vec!["X"],
            vec!["", "X"],
            vec!["", "C"],
            vec!["Concern"],
            vec!["Outer", "Concern"],
            vec!["Z"],
            vec!["Y", "Z"],
            vec!["Y"],
            vec!["W"],
            vec!["Outer"],
            vec![],
            vec![""],
            vec!["", ""],
        ] {
            let path: Vec<_> = spelling.iter().map(|s| Symbol::from(*s)).collect();
            let constant = Expr::new(crate::span::Span::synthetic(), ExprNode::Const { path });
            let scan = modules
                .iter()
                .any(|module| super::super::constant_names_class(&constant, module));
            assert_eq!(known.name(&constant), scan, "{spelling:?}");
            // `constant_names_class` is the allocating formulation, matched from the end.
            let written = constant_spelling(&constant).unwrap();
            let reference = modules
                .iter()
                .any(|m| m.0.as_str() == written || m.0.as_str().ends_with(&format!("::{written}")));
            assert_eq!(scan, reference, "{spelling:?}");
        }
        let not_const = Expr::new(crate::span::Span::synthetic(), ExprNode::SelfRef);
        assert!(!known.name(&not_const));
    }
}
