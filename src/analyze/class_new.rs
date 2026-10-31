//! Checker-only identities for factories whose original declaration is retained.
//!
//! Admitted blockless factories reuse ordinary-class nominal superclass facts.
//! Known inherited hooks stay excluded because they see an anonymous child.
//! Like ordinary classes, a factory does not lose those facts for
//! unrelated or unresolved reflection. Literal mutations of the factory, its
//! ancestry, or the Class constructor still make that identity uncertain.
//! A module mixed into Class, Module, Object or BasicObject is such a
//! mutation only when it may change construction.
//! Nonempty blocks of class-body statements are ingested as ordinary
//! library classes (`ingest::class_new_factory`); other nonempty blocks stay
//! excluded here. Ordinary class headers define the superclass parity bar.
//! Constructor mutations need separate checks because those headers never
//! call Class.new.

use crate::det::{HashMap, HashSet};

use crate::App;
use crate::dialect::{MethodReceiver, ModelBodyItem};
use crate::expr::{ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

use super::ClassInfo;
use super::body::ConstResolver;

use crate::ingest::prism_api::PrismLocation;

/// `Class.new` is looked up through these classes, so a mutation of one
/// can change it.
const CONSTRUCTOR_ANCESTORS: &[&str] = &["Class", "Module", "Object", "BasicObject"];

/// What a module must define, on either side, to change what `Class.new`
/// builds when it is mixed into a constructor ancestor: a step of
/// construction, or a hook that runs when the module is mixed in.
const CONSTRUCTION_METHODS: &[&str] = &[
    "new",
    "allocate",
    "initialize",
    "inherited",
    "included",
    "append_features",
    "prepended",
    "prepend_features",
    "extended",
    "extend_object",
];

pub(super) fn register(
    app: &App,
    resolver: &ConstResolver,
    classes: &mut HashMap<ClassId, ClassInfo>,
) -> HashMap<Span, Ty> {
    let mut factories = HashMap::default();
    // This declaration calls Class.new, unlike a class keyword. A replacement
    // of that factory itself cannot establish an ordinary subclass identity.
    if resolver.has_source_namespace("Class") {
        return factories;
    }
    let mut candidates = Vec::new();
    for class in &app.library_classes {
        for (name, value) in &class.constants {
            candidates.push((&class.name, name.as_str(), value));
        }
    }
    for model in &app.models {
        for item in &model.body {
            if let ModelBodyItem::Unknown { expr, .. } = item
                && let ExprNode::Assign {
                    target: LValue::Const { path },
                    value,
                } = &*expr.node
                && let [name] = path.as_slice()
            {
                candidates.push((&model.name, name.as_str(), value));
            }
        }
    }
    candidates.retain(|(_, _, value)| {
        matches!(&*value.node,
        ExprNode::Send { recv: Some(recv), method, .. }
        if method.as_str() == "new" && matches!(&*recv.node, ExprNode::Const { path }
            if resolver.is_runtime_class(recv.span, path, "Class")))
    });
    if candidates.is_empty() {
        return factories;
    }
    // Like main's built-in Integer#+ model, this assumes Class.new is the
    // core constructor unless the existing literal mutation detector says
    // otherwise. Multi-assignment aliases with non-array/splat RHS (including
    // variadic replacements), Method-to-Proc conversions and indirect Method
    // invocation, refinements of Class#new, and interpolated undef names are
    // shared core-method-model limits, not additional detection guarantees.
    let mutations = source_mutations(app, resolver);
    if CONSTRUCTOR_ANCESTORS
        .iter()
        .any(|owner| mutations.targets(owner) || mutations.constructors.contains(*owner))
    {
        return factories;
    }
    // Ingest also records class << self methods, before their inferred method
    // types have been populated in the registry used for admission.
    let inherited_hooks: HashSet<_> = app
        .library_classes
        .iter()
        .flat_map(|class| {
            class
                .methods
                .iter()
                .map(move |method| (&class.name, method))
        })
        .chain(
            app.models
                .iter()
                .flat_map(|model| model.methods().map(move |method| (&model.name, method))),
        )
        .chain(app.controllers.iter().flat_map(|controller| {
            controller
                .class_methods()
                .map(move |method| (&controller.name, method))
        }))
        .filter(|(_, method)| {
            method.receiver == MethodReceiver::Class && method.name.as_str() == "inherited"
        })
        .map(|(owner, _)| owner.clone())
        .collect();
    // Retain candidate parent edges even before admission, so literal
    // mutations of a transitive factory parent are independent of file order.
    let parents: HashMap<_, _> = candidates
        .iter()
        .filter_map(|(owner, name, value)| {
            let ExprNode::Send { args, .. } = &*value.node else {
                return None;
            };
            if args.len() > 1 {
                return None;
            }
            let parent = if let Some(parent) = args.first() {
                let ExprNode::Const { path } = &*parent.node else {
                    return None;
                };
                literal_parent(parent.span, path, resolver, classes)?
            } else {
                ClassId(Symbol::from("Object"))
            };
            Some((
                ClassId(Symbol::from(format!("{}::{name}", owner.0.as_str()))),
                parent,
            ))
        })
        .collect();
    for (owner, name, value) in candidates {
        let ExprNode::Send { args, block, .. } = &*value.node else {
            continue;
        };
        if args.len() > 1 || (block.is_some() && !empty_source_block(app, value.span)) {
            continue;
        }
        let id = ClassId(Symbol::from(format!("{}::{name}", owner.0.as_str())));
        // Class.new creates a fresh class; an existing/reassigned identity is
        // not equivalent to an ordinary class reopening. Also retain the
        // emitted owner: main relocates some file-level constants at ingest.
        if classes.contains_key(&id)
            || !resolver.sole_namespace(value.span.file, id.0.as_str())
            || resolver.has_source_singleton_body(id.0.as_str())
            || mutations.targets(id.0.as_str())
            || constructor_shadowed(&mutations, value.span, id.0.as_str())
        {
            continue;
        }
        let Some(parent) = parents.get(&id) else {
            continue;
        };
        if has_unsupported_ancestry(
            &id,
            parent,
            classes,
            resolver,
            &parents,
            &mutations,
            &inherited_hooks,
        ) {
            continue;
        }
        classes.insert(
            id.clone(),
            ClassInfo {
                parent: Some(parent.clone()),
                app_declared: true,
                ..ClassInfo::default()
            },
        );
        factories.insert(
            value.span,
            Ty::Class {
                id,
                args: vec![].into(),
            },
        );
    }
    // Main marks an ordinary subclass of a positional Struct superclass.
    // Inherit that constructor fact through factory parents too, after every
    // identity has entered the registry so source registration order is moot.
    let factory_ids: HashSet<_> = factories
        .values()
        .filter_map(|ty| match ty {
            Ty::Class { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    for id in &factory_ids {
        let mut cursor = id;
        let mut seen = HashSet::default();
        let positional = loop {
            if !seen.insert(cursor) {
                break false;
            }
            let Some(info) = classes.get(cursor) else {
                break false;
            };
            if !factory_ids.contains(cursor) {
                break info.positional_struct;
            }
            let Some(parent) = &info.parent else {
                break false;
            };
            cursor = parent;
        };
        classes.get_mut(id).unwrap().positional_struct = positional;
    }
    factories
}

fn literal_parent(
    span: Span,
    path: &[Symbol],
    resolver: &ConstResolver,
    classes: &HashMap<ClassId, ClassInfo>,
) -> Option<ClassId> {
    if let Some(id) = resolver.namespace(span, path) {
        // A resolved module is not a superclass. Unmodeled classes need no
        // registry entry: ordinary class headers keep that same parent link.
        if classes.get(id).is_some_and(|info| info.is_module)
            || !resolver.is_class_namespace(span, path)
        {
            return None;
        }
        return Some(id.clone());
    }
    if let Some(name) = resolver.declaration_name(span, path) {
        let id = ClassId(Symbol::from(name));
        // An assigned scalar is not evidence of a class. A class already
        // recovered by main's Struct/Data ingestion is evidence, however.
        return classes
            .get(&id)
            .is_some_and(|info| !info.is_module)
            .then_some(id);
    }
    // An external literal superclass can be absent from the source index.
    // Main keeps the written parent and dispatches gradually past that edge;
    // do not demand a complete Rubydex ancestry just for factory syntax.
    let name = path
        .iter()
        .map(Symbol::as_str)
        .collect::<Vec<_>>()
        .join("::");
    // Rubydex's minimal core index omits these module namespaces. Unlike an
    // unknown gem parent, their kind is known: Ruby refuses them as parents.
    if matches!(
        name.trim_start_matches("::"),
        "Kernel"
            | "Enumerable"
            | "Comparable"
            | "Math"
            | "Marshal"
            | "Process"
            | "ObjectSpace"
            | "GC"
            | "Signal"
            | "Errno"
            | "FileTest"
    ) {
        return None;
    }
    (!name.trim_start_matches("::").is_empty())
        .then(|| ClassId(Symbol::from(name.trim_start_matches("::"))))
}

fn has_unsupported_ancestry(
    factory: &ClassId,
    parent: &ClassId,
    classes: &HashMap<ClassId, ClassInfo>,
    resolver: &ConstResolver,
    factory_parents: &HashMap<ClassId, ClassId>,
    mutations: &Mutations,
    inherited_hooks: &HashSet<ClassId>,
) -> bool {
    let mut pending = vec![parent.clone()];
    let mut seen = HashSet::default();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        // Class.new calls inherited before assigning the constant: the child
        // is still anonymous. An ordinary header supplies its name to the hook,
        // so application-defined hooks cannot share the class-header assumption.
        // A module's include hook ran on its includer and can give it one.
        if id == *factory
            || id.0.as_str() == "Class"
            || mutations.targets(id.0.as_str())
            || mutations.include_hooks.contains(id.0.as_str())
            || inherited_hooks.contains(&id)
            || resolver.source_singleton_method(id.0.as_str(), "inherited")
            || classes
                .get(&id)
                .is_some_and(|info| info.class_methods.contains_key(&Symbol::from("inherited")))
        {
            return true;
        }
        if let Some(ancestors) = resolver.class_ancestry(&id) {
            pending.extend(ancestors.iter().cloned());
        }
        if let Some(info) = classes.get(&id) {
            pending.extend(info.parent.iter().cloned());
            pending.extend(info.includes.iter().cloned());
        }
        if let Some(parent) = factory_parents.get(&id) {
            pending.push(parent.clone());
        }
    }
    false
}

#[derive(Default)]
struct Mutations {
    namespaces: HashSet<String>,
    constants: HashSet<String>,
    constant_namespaces: HashSet<String>,
    constructors: HashSet<String>,
    /// Modules whose singleton side defines `included` or `append_features`.
    /// The hook gets the includer as an argument, so a class body's own
    /// `include` of one is not the plain ancestry main models.
    include_hooks: HashSet<String>,
    /// `const_set(:Class, …)` under a namespace, at this source offset.
    /// A factory later in that namespace is not a call to `::Class.new`.
    constructor_shadows: Vec<(String, crate::span::FileId, u32)>,
}

impl Mutations {
    fn targets(&self, name: &str) -> bool {
        self.namespaces.contains(name)
            || self
                .constants
                .iter()
                .any(|constant| name == constant || name.starts_with(&format!("{constant}::")))
            || self
                .constant_namespaces
                .iter()
                .any(|owner| owner == "Object" || name.starts_with(&format!("{owner}::")))
    }
}

/// Only receiver evidence reaches admission. Main does not invalidate every
/// ordinary class for an unknown receiver or a dynamic send elsewhere, and
/// neither does this pass. A constant alias alone is not a mutation.
fn source_mutations(app: &App, resolver: &ConstResolver) -> Mutations {
    use ruby_prism::Visit;
    struct Scan<'a> {
        mutations: Mutations,
        aliases: HashMap<String, Vec<String>>,
        resolver: &'a ConstResolver,
        file: crate::span::FileId,
        owners: Vec<String>,
        singleton_receivers: Vec<(usize, Vec<String>)>,
        declarations: HashSet<String>,
        /// Every class or module body, by the owner name it pushes.
        bodies: HashSet<String>,
        /// Owners whose bodies define, alias, undefine or `attr` one of
        /// `CONSTRUCTION_METHODS`, on either side.
        construction: HashSet<String>,
        /// A body's own `include`: the owner, and the module if resolved.
        includes: Vec<(String, Option<String>)>,
        /// Mixins into a constructor ancestor: those receivers, and each
        /// module if resolved. They are decided after the whole scan.
        ancestor_mixins: Vec<(Vec<String>, Vec<Option<String>>)>,
    }
    impl<'pr> Visit<'pr> for Scan<'_> {
        fn visit_constant_write_node(&mut self, node: &ruby_prism::ConstantWriteNode<'pr>) {
            let name = String::from_utf8_lossy(node.name().as_slice());
            let alias = self.constant_binding(&name);
            self.constant_alias(vec![alias], node.value());
            ruby_prism::visit_constant_write_node(self, node);
        }
        fn visit_constant_path_write_node(
            &mut self,
            node: &ruby_prism::ConstantPathWriteNode<'pr>,
        ) {
            self.constant_alias(self.binding_targets(&node.target().as_node()), node.value());
            ruby_prism::visit_constant_path_write_node(self, node);
        }
        fn visit_multi_write_node(&mut self, node: &ruby_prism::MultiWriteNode<'pr>) {
            self.parallel_aliases(
                &node.lefts().iter().collect::<Vec<_>>(),
                node.rest().is_some(),
                &node.rights().iter().collect::<Vec<_>>(),
                node.value(),
            );
            // Track constructor aliases without adding a parent-binding veto.
            // Ordinary class headers share main's parallel-rebinding limit.
            ruby_prism::visit_multi_write_node(self, node);
        }
        fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'pr>) {
            // `class << self` inside an ordinary parent is already modeled.
            // A separate literal receiver can mutate a factory identity.
            if node.expression().as_self_node().is_none() {
                self.namespace_mutation(Some(node.expression()));
            }
            self.singleton_receivers.push((
                self.owners.len(),
                self.receiver_targets(Some(node.expression())),
            ));
            ruby_prism::visit_singleton_class_node(self, node);
            self.singleton_receivers.pop();
        }
        fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
            let name = String::from_utf8_lossy(node.name_loc().as_slice());
            if CONSTRUCTION_METHODS.contains(&name.as_ref()) {
                let targets = if node.receiver().is_some() {
                    self.receiver_targets(node.receiver())
                } else {
                    self.singleton_receivers
                        .last()
                        .filter(|(depth, _)| *depth == self.owners.len())
                        .map(|(_, targets)| targets.clone())
                        .unwrap_or_else(|| self.receiver_targets(None))
                };
                if name == "new" {
                    self.mutations.constructors.extend(targets.iter().cloned());
                }
                if matches!(name.as_ref(), "included" | "append_features")
                    && (node.receiver().is_some()
                        || self
                            .singleton_receivers
                            .last()
                            .is_some_and(|(depth, _)| *depth == self.owners.len()))
                {
                    self.mutations.include_hooks.extend(targets.iter().cloned());
                }
                self.construction.extend(targets);
            }
            if let Some(receiver) = node.receiver()
                && receiver.as_self_node().is_none()
            {
                self.namespace_mutation(Some(receiver));
            }
            ruby_prism::visit_def_node(self, node);
        }
        fn visit_alias_method_node(&mut self, node: &ruby_prism::AliasMethodNode<'pr>) {
            let name = literal_name(&node.new_name());
            if name.as_deref() == Some("new") {
                self.mutations
                    .constructors
                    .extend(self.receiver_targets(None));
            }
            if construction_name(name.as_deref()) {
                self.construction.extend(self.receiver_targets(None));
            }
            ruby_prism::visit_alias_method_node(self, node);
        }
        fn visit_undef_node(&mut self, node: &ruby_prism::UndefNode<'pr>) {
            if node
                .names()
                .iter()
                .any(|name| literal_name(&name).as_deref() == Some("new"))
            {
                self.mutations
                    .constructors
                    .extend(self.receiver_targets(None));
            }
            if node
                .names()
                .iter()
                .any(|name| construction_name(literal_name(&name).as_deref()))
            {
                self.construction.extend(self.receiver_targets(None));
            }
            ruby_prism::visit_undef_node(self, node);
        }
        fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
            self.push_owner(&node.constant_path());
            ruby_prism::visit_class_node(self, node);
            self.owners.pop();
        }
        fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
            self.push_owner(&node.constant_path());
            ruby_prism::visit_module_node(self, node);
            self.owners.pop();
        }
        fn visit_constant_and_write_node(&mut self, node: &ruby_prism::ConstantAndWriteNode<'pr>) {
            self.constant_write(&String::from_utf8_lossy(node.name().as_slice()));
            let alias = self.constant_binding(&String::from_utf8_lossy(node.name().as_slice()));
            self.constant_alias(vec![alias], node.value());
            ruby_prism::visit_constant_and_write_node(self, node);
        }
        fn visit_constant_or_write_node(&mut self, node: &ruby_prism::ConstantOrWriteNode<'pr>) {
            let alias = self.constant_binding(&String::from_utf8_lossy(node.name().as_slice()));
            self.constant_alias(vec![alias], node.value());
            ruby_prism::visit_constant_or_write_node(self, node);
        }
        fn visit_constant_operator_write_node(
            &mut self,
            node: &ruby_prism::ConstantOperatorWriteNode<'pr>,
        ) {
            self.constant_write(&String::from_utf8_lossy(node.name().as_slice()));
            ruby_prism::visit_constant_operator_write_node(self, node);
        }
        fn visit_constant_path_and_write_node(
            &mut self,
            node: &ruby_prism::ConstantPathAndWriteNode<'pr>,
        ) {
            self.path_write(node.target().as_node());
            self.constant_alias(self.binding_targets(&node.target().as_node()), node.value());
            ruby_prism::visit_constant_path_and_write_node(self, node);
        }
        fn visit_constant_path_or_write_node(
            &mut self,
            node: &ruby_prism::ConstantPathOrWriteNode<'pr>,
        ) {
            self.constant_alias(self.binding_targets(&node.target().as_node()), node.value());
            ruby_prism::visit_constant_path_or_write_node(self, node);
        }
        fn visit_constant_path_operator_write_node(
            &mut self,
            node: &ruby_prism::ConstantPathOperatorWriteNode<'pr>,
        ) {
            self.path_write(node.target().as_node());
            ruby_prism::visit_constant_path_operator_write_node(self, node);
        }
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let args: Vec<_> = node
                .arguments()
                .map(|args| args.arguments().iter().collect())
                .unwrap_or_default();
            self.call_mutation(
                node.receiver(),
                &String::from_utf8_lossy(node.name().as_slice()),
                &args,
                false,
                node.location().start_offset() as u32,
            );
            ruby_prism::visit_call_node(self, node);
        }
    }
    fn literal_name(node: &ruby_prism::Node<'_>) -> Option<String> {
        node.as_symbol_node()
            .map(|node| String::from_utf8_lossy(node.unescaped()).into_owned())
            .or_else(|| {
                node.as_string_node()
                    .map(|node| String::from_utf8_lossy(node.unescaped()).into_owned())
            })
    }
    /// A computed name may be any of them.
    fn construction_name(name: Option<&str>) -> bool {
        name.is_none_or(|name| CONSTRUCTION_METHODS.contains(&name))
    }
    fn unparenthesized(mut node: ruby_prism::Node<'_>) -> Option<ruby_prism::Node<'_>> {
        while let Some(parentheses) = node.as_parentheses_node() {
            let body = parentheses.body()?;
            node = if let Some(statements) = body.as_statements_node() {
                if statements.body().len() != 1 {
                    return None;
                }
                statements.body().iter().next()?
            } else {
                body
            };
        }
        Some(node)
    }
    fn method_reference(
        node: ruby_prism::Node<'_>,
    ) -> Option<(Option<ruby_prism::Node<'_>>, Option<String>)> {
        let reference = unparenthesized(node)?.as_call_node()?;
        let args: Vec<_> = reference
            .arguments()
            .map(|args| args.arguments().iter().collect())
            .unwrap_or_default();
        let mut args = args.as_slice();
        let mut method = String::from_utf8_lossy(reference.name().as_slice()).into_owned();
        while matches!(method.as_str(), "send" | "public_send" | "__send__") {
            method = args.first().and_then(literal_name)?;
            args = &args[1..];
        }
        if !matches!(
            method.as_str(),
            "method" | "public_method" | "singleton_method"
        ) {
            return None;
        }
        let name = match args {
            [name] => literal_name(name),
            _ => None,
        };
        Some((reference.receiver(), name))
    }
    impl Scan<'_> {
        fn call_mutation(
            &mut self,
            receiver: Option<ruby_prism::Node<'_>>,
            method: &str,
            args: &[ruby_prism::Node<'_>],
            reflective: bool,
            offset: u32,
        ) {
            match method {
                "call" | "[]" | "===" => {
                    if let Some((receiver, name)) = receiver.and_then(method_reference) {
                        if let Some(name) = name {
                            self.call_mutation(receiver, &name, args, true, offset);
                        } else {
                            self.namespace_mutation(receiver);
                        }
                    }
                }
                "send" | "public_send" | "__send__" => {
                    if let Some(name) = args.first().and_then(literal_name) {
                        // Forward every literal layer, retaining the original
                        // receiver and the remaining method arguments.
                        self.call_mutation(receiver, &name, &args[1..], true, offset);
                    } else {
                        self.namespace_mutation(receiver);
                    }
                }
                "const_set" | "remove_const" => {
                    let name = args.first().and_then(literal_name);
                    self.constant_mutation(receiver, name.as_deref(), offset);
                }
                "define_method"
                | "define_singleton_method"
                | "alias_method"
                | "remove_method"
                | "undef_method"
                | "class_eval"
                | "module_eval"
                | "instance_eval"
                | "class_exec"
                | "module_exec"
                | "instance_exec"
                | "eval" => self.namespace_mutation(receiver),
                "include" | "extend" | "prepend" => self.mixin(receiver, method, args, reflective),
                "attr"
                    if args.iter().any(|arg| {
                        arg.as_true_node().is_none()
                            && arg.as_false_node().is_none()
                            && literal_name(arg).is_none_or(|name| name == "new")
                    }) =>
                {
                    // An attribute macro on the constructor lookup path can
                    // replace Class.new. Do not add an ordinary-parent veto.
                    self.mutations
                        .constructors
                        .extend(self.receiver_targets(receiver));
                }
                "attr_reader" | "attr_writer" | "attr_accessor"
                    if receiver.is_some() || reflective || self.owner_mutation() =>
                {
                    self.namespace_mutation(receiver);
                }
                "private_class_method"
                | "public_class_method"
                | "private"
                | "public"
                | "protected"
                | "module_function"
                    if args
                        .iter()
                        .any(|arg| literal_name(arg).is_none_or(|name| name == "new")) =>
                {
                    self.namespace_mutation(receiver);
                }
                // An attribute of a module body can still define a method a
                // mixin of that module brings along.
                "attr" | "attr_reader" | "attr_writer" | "attr_accessor"
                    if args.iter().any(|arg| {
                        arg.as_true_node().is_none()
                            && arg.as_false_node().is_none()
                            && construction_name(literal_name(arg).as_deref())
                    }) =>
                {
                    self.construction.extend(self.receiver_targets(receiver));
                }
                _ => {}
            }
        }
        /// A call without a receiver still mutates its owner in a singleton
        /// body, or in a constructor ancestor's own body.
        fn owner_mutation(&self) -> bool {
            self.singleton_receivers
                .last()
                .is_some_and(|(depth, _)| *depth == self.owners.len())
                || self
                    .receiver_targets(None)
                    .iter()
                    .any(|owner| CONSTRUCTOR_ANCESTORS.contains(&owner.as_str()))
        }
        /// `include`, `prepend` and `extend`. A module body's own `include`
        /// is no mutation, but the owner brings that module along. A mixin
        /// into a constructor ancestor waits for the whole scan: it counts
        /// only if a module may change construction (`may_construct`).
        fn mixin(
            &mut self,
            receiver: Option<ruby_prism::Node<'_>>,
            method: &str,
            args: &[ruby_prism::Node<'_>],
            reflective: bool,
        ) {
            let modules: Vec<_> = args.iter().map(|arg| self.module_name(arg)).collect();
            if method == "include" && receiver.is_none() && !reflective && !self.owner_mutation() {
                for owner in self.receiver_targets(None) {
                    self.includes
                        .extend(modules.iter().map(|module| (owner.clone(), module.clone())));
                }
                return;
            }
            let (ancestors, others): (Vec<_>, Vec<_>) = self
                .receiver_targets(receiver)
                .into_iter()
                .partition(|target| CONSTRUCTOR_ANCESTORS.contains(&target.as_str()));
            self.mutations.namespaces.extend(others);
            if !ancestors.is_empty() {
                self.ancestor_mixins.push((ancestors, modules));
            }
        }
        /// The module a mixin argument names, when the resolver resolves it
        /// to a module that only these sources define. A reopening does not
        /// show the methods Ruby core or the framework runtime gives it.
        fn module_name(&self, node: &ruby_prism::Node<'_>) -> Option<String> {
            if node.as_constant_read_node().is_none() && node.as_constant_path_node().is_none() {
                return None;
            }
            let location = node.location();
            let text = String::from_utf8_lossy(location.as_slice());
            let path = text.split("::").map(Symbol::from).collect::<Vec<_>>();
            let span = Span {
                file: self.file,
                start: location.start_offset() as u32,
                end: location.end_offset() as u32,
            };
            let name = self.resolver.namespace(span, &path)?.0.as_str();
            (!self.resolver.is_runtime_class(span, &path, name)).then(|| name.to_owned())
        }
        /// Whether mixing in this module may change what `Class.new` builds:
        /// no source body shows its methods, a body defines one of
        /// `CONSTRUCTION_METHODS`, something else mutates it, or a module it
        /// includes may. Each module's methods count, wherever it is opened.
        fn may_construct(&self, module: &str, seen: &mut HashSet<String>) -> bool {
            if !seen.insert(module.to_owned()) {
                return false;
            }
            !self.bodies.contains(module)
                || self.construction.contains(module)
                || self.mutations.constructors.contains(module)
                || self.mutations.targets(module)
                || self.includes.iter().any(|(owner, included)| {
                    owner == module
                        && included
                            .as_deref()
                            .is_none_or(|included| self.may_construct(included, seen))
                })
        }
        fn parallel_aliases(
            &mut self,
            lefts: &[ruby_prism::Node<'_>],
            has_rest: bool,
            rights: &[ruby_prism::Node<'_>],
            value: ruby_prism::Node<'_>,
        ) {
            let Some(array) = unparenthesized(value).and_then(|value| value.as_array_node()) else {
                return;
            };
            let values: Vec<_> = array.elements().iter().collect();
            // A splatted RHS can move every position, so do not infer
            // aliases from unknown positional relationships.
            if values.iter().any(|value| value.as_splat_node().is_some()) {
                return;
            }
            let right_start = values.len().saturating_sub(rights.len()).max(lefts.len());
            for (index, value) in values.into_iter().enumerate() {
                let target = lefts.get(index).or_else(|| {
                    (has_rest && index >= right_start)
                        .then(|| rights.get(index - right_start))
                        .flatten()
                });
                if let Some(target) = target {
                    self.parallel_alias(target, value);
                }
            }
        }
        fn parallel_alias(&mut self, target: &ruby_prism::Node<'_>, value: ruby_prism::Node<'_>) {
            if let Some(group) = target.as_multi_target_node() {
                self.parallel_aliases(
                    &group.lefts().iter().collect::<Vec<_>>(),
                    group.rest().is_some(),
                    &group.rights().iter().collect::<Vec<_>>(),
                    value,
                );
            } else {
                self.constant_alias(self.binding_targets(target), value);
            }
        }
        fn constant_alias(&mut self, aliases: Vec<String>, value: ruby_prism::Node<'_>) {
            let Some(mut value) = unparenthesized(value) else {
                return;
            };
            // A conditional assignment can yield its literal RHS, and a
            // chained assignment yields the value of its innermost write.
            // Preserve those possible aliases for actual mutation receivers.
            loop {
                let nested = value
                    .as_constant_write_node()
                    .map(|node| node.value())
                    .or_else(|| value.as_constant_path_write_node().map(|node| node.value()))
                    .or_else(|| value.as_constant_and_write_node().map(|node| node.value()))
                    .or_else(|| value.as_constant_or_write_node().map(|node| node.value()))
                    .or_else(|| {
                        value
                            .as_constant_path_and_write_node()
                            .map(|node| node.value())
                    })
                    .or_else(|| {
                        value
                            .as_constant_path_or_write_node()
                            .map(|node| node.value())
                    });
                let Some(nested) = nested else { break };
                let Some(nested) = unparenthesized(nested) else {
                    return;
                };
                value = nested;
            }
            if value.as_constant_read_node().is_some()
                || value.as_constant_path_node().is_some()
                || value
                    .as_call_node()
                    .is_some_and(|call| call.name().as_slice() == b"singleton_class")
            {
                let targets = self.receiver_targets(Some(value));
                for alias in aliases {
                    self.aliases
                        .entry(alias)
                        .or_default()
                        .extend(targets.iter().cloned());
                }
            }
        }
        fn push_owner(&mut self, node: &ruby_prism::Node<'_>) {
            let text = String::from_utf8_lossy(node.location().as_slice());
            let written = if text.starts_with("::") || self.owners.is_empty() {
                text.trim_start_matches("::").to_owned()
            } else {
                format!("{}::{text}", self.owners.last().unwrap())
            };
            // Keep written alias openings for the existing identity gate,
            // while mutations inside the body use the actual receiver.
            self.declarations.insert(written.clone());
            let location = node.location();
            let owner = match self.resolver.namespace_body(Span {
                file: self.file,
                start: location.start_offset() as u32,
                end: location.end_offset() as u32,
            }) {
                Some(id) => id.0.as_str().to_owned(),
                None => written,
            };
            self.bodies.insert(owner.clone());
            self.owners.push(owner);
        }
        fn receiver_targets(&self, receiver: Option<ruby_prism::Node<'_>>) -> Vec<String> {
            let Some(receiver) = receiver else {
                return self.implicit_targets();
            };
            let Some(receiver) = unparenthesized(receiver) else {
                return vec![];
            };
            if receiver.as_self_node().is_some() {
                return self.implicit_targets();
            }
            // The singleton class's instance methods are the named class's
            // class methods. A literal `.singleton_class` receiver or alias
            // can therefore replace the factory's constructor just as a
            // direct receiver can; arbitrary calls remain unresolved.
            if let Some(call) = receiver.as_call_node()
                && call.name().as_slice() == b"singleton_class"
                && call.arguments().is_none()
                && call.block().is_none()
            {
                return self.receiver_targets(call.receiver());
            }
            if receiver.as_constant_read_node().is_none()
                && receiver.as_constant_path_node().is_none()
            {
                return vec![];
            }
            let location = receiver.location();
            let text = String::from_utf8_lossy(location.as_slice());
            let path = text.split("::").map(Symbol::from).collect::<Vec<_>>();
            if let Some(id) = self.resolver.namespace(
                Span {
                    file: self.file,
                    start: location.start_offset() as u32,
                    end: location.end_offset() as u32,
                },
                &path,
            ) {
                return vec![id.0.as_str().to_owned()];
            }
            // A literal unmodeled receiver may denote the written parent or a
            // lexically nested one. Keep those plausible targets, not a veto.
            let mut targets = vec![text.trim_start_matches("::").to_owned()];
            if !text.starts_with("::") {
                targets.extend(self.owners.iter().map(|owner| format!("{owner}::{text}")));
            }
            targets
        }
        fn implicit_targets(&self) -> Vec<String> {
            self.singleton_receivers
                .last()
                .filter(|(depth, _)| *depth == self.owners.len())
                .map(|(_, targets)| targets.clone())
                .unwrap_or_else(|| self.owners.last().cloned().into_iter().collect())
        }
        fn namespace_mutation(&mut self, receiver: Option<ruby_prism::Node<'_>>) {
            self.mutations
                .namespaces
                .extend(self.receiver_targets(receiver));
        }
        fn constant_write(&mut self, name: &str) {
            self.mutations.constants.insert(self.constant_binding(name));
        }
        fn constant_binding(&self, name: &str) -> String {
            match self.owners.last().map(String::as_str) {
                None | Some("Object") => name.to_owned(),
                Some(owner) => format!("{owner}::{name}"),
            }
        }
        fn constant_mutation(
            &mut self,
            receiver: Option<ruby_prism::Node<'_>>,
            name: Option<&str>,
            offset: u32,
        ) {
            for owner in self.receiver_targets(receiver) {
                if owner == "Class" {
                    self.mutations.namespaces.insert(owner.clone());
                }
                if let Some(name) = name {
                    if name == "Class" {
                        // Ruby resolves `Class` to this nested constant for
                        // every later factory in the namespace. A binding
                        // after the factory does not rewrite the call.
                        self.mutations
                            .constructor_shadows
                            .push((owner.clone(), self.file, offset));
                    }
                    self.mutations.constants.insert(if owner == "Object" {
                        name.to_owned()
                    } else {
                        format!("{owner}::{name}")
                    });
                } else {
                    self.mutations.constant_namespaces.insert(owner);
                }
            }
        }
        fn path_write(&mut self, node: ruby_prism::Node<'_>) {
            self.mutations.constants.extend(self.binding_targets(&node));
        }
        fn binding_targets(&self, node: &ruby_prism::Node<'_>) -> Vec<String> {
            if let Some(target) = node.as_constant_target_node() {
                return vec![
                    self.constant_binding(&String::from_utf8_lossy(target.name().as_slice())),
                ];
            }
            let (parent, name) = if let Some(path) = node.as_constant_path_node() {
                (path.parent(), path.name())
            } else if let Some(path) = node.as_constant_path_target_node() {
                (path.parent(), path.name())
            } else {
                return vec![];
            };
            let Some(name) = name else { return vec![] };
            let name = String::from_utf8_lossy(name.as_slice());
            // The last component is the binding being changed, not its old
            // class value. Only the owner is a mutation receiver: rebinding
            // `Alias` must not invalidate the class Alias used to name.
            let Some(parent) = parent else {
                return vec![name.into_owned()];
            };
            self.receiver_targets(Some(parent))
                .into_iter()
                .map(|owner| {
                    if owner == "Object" {
                        name.to_string()
                    } else {
                        format!("{owner}::{name}")
                    }
                })
                .collect()
        }
    }
    let mut scan = Scan {
        mutations: Mutations::default(),
        aliases: HashMap::default(),
        resolver,
        file: crate::span::FileId(0),
        owners: Vec::new(),
        singleton_receivers: Vec::new(),
        declarations: HashSet::default(),
        bodies: HashSet::default(),
        construction: HashSet::default(),
        includes: Vec::new(),
        ancestor_mixins: Vec::new(),
    };
    for (index, source) in app
        .sources
        .iter()
        .enumerate()
        .filter(|(_, source)| source.path.ends_with(".rb"))
    {
        scan.file = crate::span::FileId(index as u32 + 1);
        let parsed = ruby_prism::parse(source.text.as_bytes());
        scan.visit(&parsed.node());
    }
    // Resolve only aliases of actual mutation receivers. Merely naming an
    // alias supplies no veto. This also catches aliases declared in later
    // registered files, without treating registration order as execution.
    fn expand(
        name: &str,
        aliases: &HashMap<String, Vec<String>>,
        active: &mut HashSet<String>,
        out: &mut HashSet<String>,
        follow_value: bool,
    ) {
        out.insert(name.to_owned());
        for (alias, targets) in aliases {
            if ((follow_value && name == alias) || name.starts_with(&format!("{alias}::")))
                && active.insert(alias.clone())
            {
                for target in targets {
                    expand(
                        &format!("{target}{}", &name[alias.len()..]),
                        aliases,
                        active,
                        out,
                        follow_value,
                    );
                }
                active.remove(alias);
            }
        }
    }
    // A bare alias read is harmless, but opening its class/module body is
    // a literal mutation of the aliased identity. Ordinary parent bodies
    // themselves remain modeled by main, rather than vetoing subclasses.
    for declaration in &scan.declarations {
        let mut targets = HashSet::default();
        expand(
            declaration,
            &scan.aliases,
            &mut HashSet::default(),
            &mut targets,
            true,
        );
        if targets.iter().any(|target| target != declaration) {
            scan.mutations.namespaces.extend(targets);
        }
    }
    for (targets, follow_value) in [
        (&mut scan.mutations.namespaces, true),
        (&mut scan.mutations.constants, false),
        (&mut scan.mutations.constant_namespaces, true),
        (&mut scan.mutations.constructors, true),
        (&mut scan.mutations.include_hooks, true),
        (&mut scan.construction, true),
    ] {
        for name in std::mem::take(targets) {
            expand(
                &name,
                &scan.aliases,
                &mut HashSet::default(),
                targets,
                follow_value,
            );
        }
    }
    // A mixin into a constructor ancestor mutates it only if some module
    // may change construction; an unresolved one may. A Module body that
    // defines a hook runs that hook for every module mixed in.
    let hooks = scan.construction.contains("Module");
    let mixed: Vec<String> = scan
        .ancestor_mixins
        .iter()
        .filter(|(_, modules)| {
            hooks
                || modules.is_empty()
                || modules.iter().any(|module| {
                    module
                        .as_deref()
                        .is_none_or(|module| scan.may_construct(module, &mut HashSet::default()))
                })
        })
        .flat_map(|(targets, _)| targets.iter().cloned())
        .collect();
    for name in mixed {
        expand(
            &name,
            &scan.aliases,
            &mut HashSet::default(),
            &mut scan.mutations.namespaces,
            true,
        );
    }
    let shadows = std::mem::take(&mut scan.mutations.constructor_shadows);
    for (owner, file, offset) in shadows {
        let mut names = HashSet::default();
        expand(&owner, &scan.aliases, &mut HashSet::default(), &mut names, true);
        for name in names {
            scan.mutations
                .constructor_shadows
                .push((name, file, offset));
        }
    }
    scan.mutations
}

/// A `const_set(:Class, …)` earlier in this file, on the factory's
/// namespace or one that encloses it, hides `::Class`.
fn constructor_shadowed(mutations: &Mutations, span: Span, factory: &str) -> bool {
    mutations
        .constructor_shadows
        .iter()
        .any(|(owner, file, offset)| {
            *file == span.file
                && *offset < span.start
                && (factory == owner || factory.starts_with(&format!("{owner}::")))
        })
}

fn empty_source_block(app: &App, span: Span) -> bool {
    let Some(source) = span
        .file
        .0
        .checked_sub(1)
        .and_then(|index| app.sources.get(index as usize))
    else {
        return false;
    };
    let Some(text) = source.text.get(span.start as usize..span.end as usize) else {
        return false;
    };
    let parsed = ruby_prism::parse(text.as_bytes());
    if parsed.errors().next().is_some() {
        return false;
    }
    let root = parsed.node();
    let Some(program) = root.as_program_node() else {
        return false;
    };
    let Some(node) = program.statements().body().iter().next() else {
        return false;
    };
    let Some(block) = node
        .as_call_node()
        .and_then(|call| call.block())
        .and_then(|block| block.as_block_node())
    else {
        return false;
    };
    block.parameters().is_none()
        && block.body().is_none_or(|body| {
            body.as_statements_node()
                .is_some_and(|body| body.body().is_empty())
        })
}
