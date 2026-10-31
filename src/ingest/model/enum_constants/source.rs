//! Prism declaration collection and lexical traversal. Use-context syntax is
//! classified separately; only policy interprets the resulting typed facts.

use super::super::super::util::{constant_id_str, flatten_statements, symbol_or_string_value};
use super::super::super::{expr, library_class, prism};
use super::{
    EnumConstant, EnumConstants, ScopedName, SourceFact, SurfaceUse, ValueContext, context,
};
use crate::expr::Literal;
use ruby_prism::Node;

use crate::ingest::prism_api::PrismLocation;

#[derive(Clone, Copy)]
enum SourceKind {
    Inputs,
    Mutations,
    /// A source that runs at boot, before models load and routes are
    /// drawn (`config/initializers`): it admits no constant input, but a
    /// constant it writes or mutates no longer has the input's value.
    Boot,
}

impl EnumConstants {
    pub(in crate::ingest) fn record(&mut self, source: &[u8], file: &str) {
        self.collect(source, file, SourceKind::Inputs, false);
    }

    /// `record` for a second table over sources the pipeline already
    /// parses and diagnoses (the routes' constants): the same inputs,
    /// without a second set of parse errors.
    pub(in crate::ingest) fn record_again(&mut self, source: &[u8], file: &str) {
        self.collect(source, file, SourceKind::Inputs, true);
    }

    /// Observe nonlibrary sources without admitting new constant inputs.
    pub(in crate::ingest) fn record_mutations(&mut self, source: &[u8], file: &str) {
        self.collect(source, file, SourceKind::Mutations, true);
    }

    /// Observe a source that runs at boot (an initializer) before the
    /// folds: it admits no input, and a constant it reassigns, mutates,
    /// or names in `const_set` / `remove_const` is tainted
    /// ([`super::super::ConstTaint::scan_boot`]). Its own top-level
    /// `NAME = …` defines a top-level name and taints no other owner's.
    pub(in crate::ingest) fn record_boot(&mut self, source: &[u8], file: &str) {
        self.collect(source, file, SourceKind::Boot, true);
    }

    fn collect(&mut self, source: &[u8], file: &str, kind: SourceKind, silent: bool) {
        struct Collector<'a, 'pr> {
            constants: &'a mut EnumConstants,
            file: &'a str,
            kind: SourceKind,
            nesting: Vec<String>,
            parents: Vec<Node<'pr>>,
            /// Offsets of the plain `NAME = …` writes at a file's or a
            /// body's top level, the one write a constant may have.
            direct_writes: crate::det::HashSet<usize>,
        }
        impl Collector<'_, '_> {
            fn scoped(&self, path: String) -> ScopedName {
                ScopedName {
                    path,
                    owners: self.nesting.iter().rev().cloned().collect(),
                }
            }

            fn record_reference(&mut self, node: &Node<'_>) {
                if node.as_self_node().is_some() {
                    if let Some(owner) = self.nesting.last() {
                        // Uncertain self does not get a guessed identity. It
                        // makes the enclosing candidate vocabulary unprovable.
                        let role = if context::namespace_body(&self.parents) {
                            context::value(&self.parents, node)
                        } else {
                            ValueContext::Unmodeled
                        };
                        self.constants.facts.push(SourceFact::Use {
                            subject: self.scoped(format!("::{owner}")),
                            context: SurfaceUse::Reference(role),
                        });
                    }
                }
                if let Some(call) = node.as_call_node() {
                    let implicit = call.receiver().is_none();
                    // Canonical annotations return their first argument; its
                    // original references/type arguments are still visited.
                    let assertion = expr::sorbet_assertion_argument(node).is_some();
                    // The only implicit declaration exemptions use the shared
                    // literal projection or admitted Rails mapping recognizer.
                    let declaration = implicit
                        && (self.nesting.last().is_some_and(|owner| {
                            matches!(
                                self.constants.values.get(owner),
                                Some(EnumConstant::Sorbet { .. })
                            )
                        }) || super::super::enum_declaration(&call).is_some_and(|declaration| {
                            super::super::serialized_enum_receiver(&declaration.mapping).is_some()
                        }));
                    let path = call
                        .receiver()
                        .and_then(|r| EnumConstants::path(&r))
                        .or_else(|| {
                            (context::namespace_body(&self.parents)
                                && (implicit
                                    || call.receiver().is_some_and(|r| r.as_self_node().is_some())))
                            .then(|| self.nesting.last().map(|owner| format!("::{owner}")))
                            .flatten()
                        });
                    if let Some(path) = path.filter(|_| !assertion && !declaration) {
                        self.constants.facts.push(SourceFact::Use {
                            subject: self.scoped(path),
                            context: SurfaceUse::Call {
                                method: constant_id_str(&call.name()).to_string(),
                                result: context::value(&self.parents, node),
                            },
                        });
                    }
                }
                if let Some((path, role)) = context::reference(&self.parents, node) {
                    self.constants.facts.push(SourceFact::Use {
                        subject: self.scoped(path),
                        context: SurfaceUse::Reference(role),
                    });
                }
            }
        }
        impl Collector<'_, '_> {
            /// What may define or replace a class method an `enum` defines
            /// ([`super::derived`]). Over-approximate: an owner is named
            /// by its last segment, and an unknown receiver opens every
            /// owner.
            fn record_singleton(&mut self, node: &Node<'_>) {
                let owner = self
                    .nesting
                    .last()
                    .map(|name| name.rsplit("::").next().unwrap_or(name).to_string());
                let constants = &mut *self.constants;
                // The innermost `class << x` around this node, if any,
                // before any nested def/class (a scope boundary).
                let singleton = self.parents.iter().rev().find_map(|parent| match parent {
                    Node::SingletonClassNode { .. } => Some(Some(parent.as_singleton_class_node().unwrap())),
                    Node::DefNode { .. } | Node::ClassNode { .. } | Node::ModuleNode { .. } => Some(None),
                    _ => None,
                }).flatten();
                let on_self = |expression: &Node<'_>| expression.as_self_node().is_some();
                // Inside `x.instance_eval do … end` (or `class_eval`, …)
                // `self` is whatever `x` is: a definition there may land
                // on any owner, so it is known by name only.
                let in_eval = self.parents.windows(2).any(|pair| {
                    matches!(&pair[1], Node::BlockNode { .. })
                        && pair[0].as_call_node().is_some_and(|call| {
                            super::derived::EVAL_METHODS.contains(&constant_id_str(&call.name()).as_ref())
                        })
                });
                if let Some(def) = node.as_def_node() {
                    let method = constant_id_str(&def.name()).to_string();
                    if in_eval {
                        constants.uncertain_singleton_methods.insert(method);
                        return;
                    }
                    match (def.receiver(), &singleton, &owner) {
                        (Some(receiver), _, Some(owner)) if receiver.as_self_node().is_some() => {
                            constants.singleton_methods.insert((owner.clone(), method));
                        }
                        (Some(receiver), _, None) if receiver.as_self_node().is_some() => {}
                        (Some(_), _, _) => {
                            constants.uncertain_singleton_methods.insert(method);
                        }
                        (None, Some(class), Some(owner)) if on_self(&class.expression()) => {
                            constants.singleton_methods.insert((owner.clone(), method));
                        }
                        (None, Some(_), _) => {
                            constants.uncertain_singleton_methods.insert(method);
                        }
                        (None, None, _) => {}
                    }
                    return;
                }
                if let Some(class) = node.as_singleton_class_node() {
                    let expression = class.expression();
                    if !on_self(&expression) {
                        match EnumConstants::path(&expression) {
                            Some(path) => {
                                constants.open_singletons.insert(path.rsplit("::").next().unwrap_or(&path).to_string());
                            }
                            None => constants.singletons_unknown = true,
                        }
                    }
                    return;
                }
                // Anything but a `def` directly inside `class << self`
                // (`attr_reader`, `alias`, `define_method`, `prepend`).
                if let [.., Node::SingletonClassNode { .. }, Node::StatementsNode { .. }] = self.parents.as_slice()
                    && let Some(owner) = &owner
                {
                    constants.open_singletons.insert(owner.clone());
                }
                let Some(call) = node.as_call_node() else { return };
                let mut method = constant_id_str(&call.name()).to_string();
                let mut args: Vec<Node<'_>> =
                    call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                // `x.send(:define_singleton_method, :name)` is the call it names.
                if matches!(method.as_str(), "send" | "__send__" | "public_send")
                    && let Some(sent) = args.first().and_then(symbol_or_string_value)
                {
                    method = sent;
                    args.remove(0);
                }
                if super::derived::SINGLETON_METHODS.contains(&method.as_str()) {
                    // `define_singleton_method(:name)` on any receiver.
                    let named = args.first().and_then(symbol_or_string_value);
                    let named = named.is_some_and(|name| {
                        constants.uncertain_singleton_methods.insert(name);
                        true
                    });
                    if in_eval && call.receiver().is_none_or(|receiver| receiver.as_self_node().is_some()) {
                        if !named {
                            constants.singletons_unknown = true;
                        }
                        return;
                    }
                    match call.receiver() {
                        None => {
                            if let Some(owner) = &owner {
                                constants.open_singletons.insert(owner.clone());
                            }
                        }
                        Some(receiver) if receiver.as_self_node().is_some() => {
                            if let Some(owner) = &owner {
                                constants.open_singletons.insert(owner.clone());
                            }
                        }
                        Some(receiver) => match EnumConstants::path(&receiver) {
                            Some(path) => {
                                constants.open_singletons.insert(path.rsplit("::").next().unwrap_or(&path).to_string());
                            }
                            None => constants.singletons_unknown = true,
                        },
                    }
                }
                // An `enum` anywhere but directly in a class body (a
                // block, a conditional, a concern's `included do`) may
                // define its class method on some owner this pass does
                // not follow.
                // A consumed (non-input) source is read after the models:
                // its declarations are never recorded, only marked.
                if let Some(declaration) = super::super::enum_declaration(&call)
                    && (!matches!(self.kind, SourceKind::Inputs)
                        || !matches!(self.parents.as_slice(), [.., Node::ClassNode { .. }, Node::StatementsNode { .. }]))
                {
                    constants
                        .uncertain_singleton_methods
                        .insert(crate::naming::pluralize_snake(&declaration.column));
                }
            }
        }
        impl<'pr> ruby_prism::Visit<'pr> for Collector<'_, 'pr> {
            fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
                // Classifier receives ancestors only, identically for branches
                // and leaves. Push/pop below is solely the traversal contract.
                self.record_reference(&node);
                self.record_singleton(&node);
                let direct = matches!(self.kind, SourceKind::Inputs)
                    && self.parents.iter().all(|parent| {
                        matches!(
                            parent,
                            Node::ProgramNode { .. }
                                | Node::StatementsNode { .. }
                                | Node::ClassNode { .. }
                                | Node::ModuleNode { .. }
                        )
                    });
                self.constants.record_write(&node, &self.nesting, direct);
                // A boot source's program-level `NAME = …` defines a
                // top-level constant: not a write to another owner's.
                let boot_top_level = matches!(self.kind, SourceKind::Boot)
                    && self.nesting.is_empty()
                    && self.parents.iter().all(|parent| {
                        matches!(parent, Node::ProgramNode { .. } | Node::StatementsNode { .. })
                    });
                if (direct || boot_top_level || matches!(self.kind, SourceKind::Mutations)) && node.as_constant_write_node().is_some() {
                    self.direct_writes.insert(node.location().start_offset());
                }
                // `const_set(:X, …)` / `self.send(:remove_const, :X)` in a
                // body replaces that owner's constants.
                if matches!(self.kind, SourceKind::Inputs)
                    && let Some(call) = node.as_call_node()
                    && call.receiver().is_none_or(|r| r.as_self_node().is_some())
                    && super::super::REFLECTION_METHODS.contains(&constant_id_str(&call.name()).as_ref())
                {
                    // Outside any body the receiver is `main`, whose
                    // `const_get` reaches every owner, as `Object`'s does;
                    // its `const_set(:Name, …)` only the one `Name`.
                    let named = super::super::named_constant_write(&call);
                    let owner = match (self.nesting.last(), &named) {
                        (Some(owner), _) => owner.as_str(),
                        (None, Some(name)) => name.as_str(),
                        (None, None) => "Object",
                    };
                    let last = owner.rsplit("::").next().unwrap_or(owner).to_string();
                    self.constants.reflected.insert(last);
                }
                let path = node
                    .as_class_node()
                    .map(|c| c.constant_path())
                    .or_else(|| node.as_module_node().map(|m| m.constant_path()));
                if let Some(path) = path.and_then(|p| EnumConstants::path(&p)) {
                    let owner = self.nesting.last().map(String::as_str).unwrap_or("");
                    let name = EnumConstants::qualify(owner, &path);
                    let qualified = path.contains("::");
                    if qualified {
                        self.constants.facts.push(SourceFact::Reopen {
                            subject: self.scoped(path),
                            collected: name.clone(),
                        });
                    }
                    let value = node
                        .as_class_node()
                        .filter(|_| direct)
                        .and_then(|class| {
                            let scope = self
                                .nesting
                                .last()
                                .map(|name| {
                                    name.split("::").map(str::to_string).collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            Some(EnumConstant::Sorbet {
                                members: library_class::literal_sorbet_members(
                                    &class, &scope, self.file,
                                )?,
                                base: EnumConstants::path(&class.superclass()?)?,
                                owners: self.nesting.iter().rev().cloned().collect(),
                            })
                        })
                        .unwrap_or(EnumConstant::Namespace);
                    if let Some(base) = node
                        .as_class_node()
                        .and_then(|class| class.superclass())
                        .and_then(|base| EnumConstants::path(&base))
                    {
                        let parent = self.scoped(base.clone());
                        self.constants.parents.entry(name.clone()).or_default().push(parent);
                        self.constants.facts.push(SourceFact::Use {
                            subject: self.scoped(base),
                            context: if matches!(value, EnumConstant::Sorbet { .. }) {
                                SurfaceUse::EnumBase
                            } else {
                                SurfaceUse::Inheritance
                            },
                        });
                    }
                    self.constants
                        .values
                        .entry(name.clone())
                        .and_modify(|existing| {
                            // Reopening a declared enum can change its surface.
                            if matches!(existing, EnumConstant::Sorbet { .. }) {
                                *existing = EnumConstant::Unsupported;
                            }
                        })
                        .or_insert(value);
                    // A top-level `class ActivityPub::CollectionsController`
                    // runs only where `ActivityPub` already exists, which
                    // Zeitwerk defines from the directory when no file
                    // declares it: its enclosing names are namespaces.
                    // Inside another body the path resolves lexically
                    // (`module Wrapper; class T::Enum` reopens `::T`), so
                    // nothing is assumed there.
                    if qualified && owner.is_empty() {
                        let mut enclosing = name.as_str();
                        while let Some((parent, _)) = enclosing.rsplit_once("::") {
                            self.constants.values.entry(parent.to_string()).or_insert(EnumConstant::Namespace);
                            enclosing = parent;
                        }
                    }
                    // Classes and modules both own lexical constant
                    // lookup — concern `TYPES` inside `included do`
                    // resolves through the module entry the same way a
                    // model class resolves `STATUS` for `enum`.
                    if matches!(self.kind, SourceKind::Inputs) {
                        // `module Wrapper; class Catalog::Rating` may reopen
                        // ::Catalog::Rating: the collected name is not
                        // certain, so its table never folds.
                        let certain = direct && (!qualified || self.nesting.is_empty());
                        let table = certain.then(|| {
                            let body = node
                                .as_class_node()
                                .and_then(|class| class.body())
                                .or_else(|| node.as_module_node().and_then(|module| module.body()));
                            super::super::ClassConsts::collect(
                                &body.map(flatten_statements).unwrap_or_default(),
                                self.file,
                            )
                        });
                        if let Some(class) = node.as_class_node() {
                            let owners: Vec<String> = std::iter::once(name.clone())
                                .chain(self.nesting.iter().rev().cloned())
                                .collect();
                            self.constants.record_enum_declarations(&class, &name, certain, &owners, self.file);
                        }
                        match (self.constants.owner_consts.get_mut(&name), table) {
                            (Some(Some(existing)), Some(table)) => existing.merge(table),
                            (Some(existing), None) => *existing = None,
                            (Some(None), Some(_)) => {}
                            (None, table) => {
                                self.constants.owner_consts.insert(name.clone(), table);
                            }
                        }
                    }
                    if node.as_class_node().is_some() || node.as_module_node().is_some() {
                        self.constants.nesting.insert(
                            (self.file.to_string(), node.location().start_offset()),
                            std::iter::once(name.clone())
                                .chain(self.nesting.iter().rev().cloned())
                                .collect(),
                        );
                    }
                    self.nesting.push(name);
                }
                self.parents.push(node);
            }

            fn visit_branch_node_leave(&mut self) {
                let node = self.parents.pop().unwrap();
                let path = node
                    .as_class_node()
                    .map(|c| c.constant_path())
                    .or_else(|| node.as_module_node().map(|m| m.constant_path()));
                if path.and_then(|p| EnumConstants::path(&p)).is_some() {
                    self.nesting.pop();
                }
            }

            fn visit_leaf_node_enter(&mut self, node: Node<'pr>) {
                self.record_reference(&node);
                self.record_singleton(&node);
                self.constants.record_write(&node, &self.nesting, false);
            }
        }
        // A source the real pipeline already consumes and diagnoses is
        // observed without adding a second set of parse errors.
        let result = if silent {
            crate::ingest::prism::parse_silent(source)
        } else {
            prism::parse(source, file)
        };
        if result.errors().next().is_none() {
            let mut collector = Collector {
                constants: self,
                file,
                kind,
                nesting: Vec::new(),
                parents: Vec::new(),
                direct_writes: Default::default(),
            };
            ruby_prism::Visit::visit(&mut collector, &result.node());
            let direct_writes = collector.direct_writes;
            match kind {
                SourceKind::Inputs => {
                    let (tainted, reflected, snapshot_taint, aliases) = super::super::ConstTaint::scan_tree(&result.node(), direct_writes);
                    self.foreign_taint.extend(tainted);
                    self.reflected.extend(reflected);
                    self.snapshot_taint.extend(snapshot_taint);
                    self.snapshot_aliases.extend(aliases);
                }
                // A boot source calls the app's classes freely (`Gadget.count`):
                // the scan skips a receiver rooted at a class or module
                // (`ConstTaint::names_boot_namespace`).
                SourceKind::Boot => {
                    let (_, reflected, _, _) = super::super::ConstTaint::scan_tree(&result.node(), direct_writes.clone());
                    self.snapshot_reflected.extend(reflected);
                    let namespaces: crate::det::HashSet<String> = self
                        .values
                        .iter()
                        .filter(|(_, value)| matches!(value, EnumConstant::Namespace))
                        .map(|(name, _)| name.clone())
                        .collect();
                    let (tainted, aliases) =
                        super::super::ConstTaint::scan_boot(&result.node(), direct_writes, namespaces);
                    self.snapshot_taint.extend(tainted.iter().cloned());
                    self.snapshot_aliases.extend(aliases.iter().cloned());
                    self.foreign_taint.extend(tainted);
                    self.boot_aliases.extend(aliases);
                }
                SourceKind::Mutations => {
                    let (_, reflected, tainted, aliases) = super::super::ConstTaint::scan_tree(&result.node(), direct_writes);
                    self.snapshot_taint.extend(tainted);
                    self.snapshot_aliases.extend(aliases);
                    self.snapshot_reflected.extend(reflected);
                }
            }
        }
    }

    fn record_write(&mut self, node: &Node<'_>, nesting: &[String], direct: bool) {
        let owner = nesting.last().map(String::as_str).unwrap_or("");
        let override_receiver = node
            .as_def_node()
            .and_then(|def| def.receiver())
            .or_else(|| {
                node.as_singleton_class_node()
                    .map(|class| class.expression())
            });
        if let Some(path) = override_receiver.and_then(|receiver| Self::path(&receiver)) {
            self.facts.push(SourceFact::Use {
                subject: ScopedName {
                    path,
                    owners: nesting.iter().rev().cloned().collect(),
                },
                context: SurfaceUse::Override,
            });
        }
        if let Some(write) = node.as_constant_write_node() {
            let name = Self::qualify(owner, constant_id_str(&write.name()));
            *self.write_counts.entry(name.clone()).or_default() += 1;
            let value = write.value();
            let literal = match value.as_call_node() {
                Some(call)
                    if constant_id_str(&call.name()) == "freeze"
                        && call.arguments().is_none()
                        && call.block().is_none() =>
                {
                    call.receiver()
                }
                _ => Some(value),
            };
            // `%w[…]` strings and `%i[…]` / `[:A, :B]` symbols — same
            // label list `enum_label_values` admits for class-local consts.
            let labels = literal.filter(|_| direct).and_then(|node| {
                node.as_array_node()?
                    .elements()
                    .iter()
                    .enumerate()
                    .map(|(i, el)| {
                        symbol_or_string_value(&el)
                            .map(|label| (label, Literal::Int { value: i as i64 }))
                    })
                    .collect()
            });
            // Reassignments have load-order-dependent semantics, never choose
            // whichever file happened to be visited last.
            self.values
                .entry(name)
                .and_modify(|value| *value = EnumConstant::Unsupported)
                .or_insert(labels.map_or(EnumConstant::Unsupported, EnumConstant::Mapping));
        } else {
            let bare = node
                .as_constant_and_write_node()
                .map(|w| w.name())
                .or_else(|| node.as_constant_or_write_node().map(|w| w.name()))
                .or_else(|| node.as_constant_operator_write_node().map(|w| w.name()))
                .or_else(|| node.as_constant_target_node().map(|w| w.name()));
            if let Some(name) = bare {
                self.values.insert(
                    Self::qualify(owner, constant_id_str(&name)),
                    EnumConstant::Unsupported,
                );
            }
            // Qualified destinations resolve only after namespaces are complete.
            let target = node
                .as_constant_path_write_node()
                .map(|w| w.target().as_node())
                .or_else(|| {
                    node.as_constant_path_and_write_node()
                        .map(|w| w.target().as_node())
                })
                .or_else(|| {
                    node.as_constant_path_or_write_node()
                        .map(|w| w.target().as_node())
                })
                .or_else(|| {
                    node.as_constant_path_operator_write_node()
                        .map(|w| w.target().as_node())
                })
                .or_else(|| node.as_constant_path_target_node().map(|w| w.as_node()));
            if let Some(path) = target.and_then(|target| Self::path(&target)) {
                self.facts.push(SourceFact::Write(ScopedName {
                    path,
                    owners: nesting.iter().rev().cloned().collect(),
                }));
            }
        }
    }

    /// An owner's own `enum` declarations, as [`super::derived`] recipes.
    /// A body whose owner is not certain records only that some owner
    /// may answer each enum's class method.
    fn record_enum_declarations(
        &mut self,
        class: &ruby_prism::ClassNode<'_>,
        name: &str,
        certain: bool,
        owners: &[String],
        file: &str,
    ) {
        let Some(body) = class.body() else { return };
        let statements = flatten_statements(body);
        let table = super::super::ClassConsts::collect(&statements, file);
        for statement in &statements {
            let Some(declaration) = statement.as_call_node().and_then(|call| super::super::enum_declaration(&call))
            else {
                continue;
            };
            if !certain {
                self.uncertain_singleton_methods.insert(crate::naming::pluralize_snake(&declaration.column));
                continue;
            }
            // The forms the owner's own ingest folds, then the derived ones.
            let recipe = super::derived::mapping_values_fold(&declaration.mapping, &table)
                .then(|| super::super::enum_label_values(&declaration.mapping, file, &table, &|_| None).ok().flatten())
                .flatten()
                .and_then(super::derived::literal_pairs)
                .map(|pairs| super::derived::Recipe::Fixed(super::derived::Value::Map { pairs, indifferent: false }))
                .or_else(|| super::derived::Recipe::of(&declaration.mapping, owners, Some(&table), file));
            self.enum_declarations
                .entry(name.to_string())
                .or_default()
                .push(super::derived::Declaration { column: declaration.column, recipe });
        }
    }

    /// A static path retaining root qualification, rejecting runtime receivers.
    pub(in crate::ingest) fn path(node: &Node<'_>) -> Option<String> {
        if let Some(parentheses) = node.as_parentheses_node() {
            let statements = flatten_statements(parentheses.body()?);
            return match statements.as_slice() {
                [expression] => Self::path(expression),
                _ => None,
            };
        }
        if let Some(read) = node.as_constant_read_node() {
            return Some(constant_id_str(&read.name()).to_string());
        }
        let (parent, name) = if let Some(path) = node.as_constant_path_node() {
            (path.parent(), path.name()?)
        } else {
            let target = node.as_constant_path_target_node()?;
            (target.parent(), target.name()?)
        };
        let mut path = match parent {
            Some(parent) => format!("{}::", Self::path(&parent)?),
            None if node.location().as_slice().starts_with(b"::") => "::".to_string(),
            None => String::new(),
        };
        path.push_str(constant_id_str(&name));
        Some(path)
    }
}
