//! Resolve local visibility while declaration order and lexical scopes still
//! exist. Never replay these calls from LibraryClass::unknown_calls.

use std::collections::{HashMap, HashSet};

use ruby_prism::{CallNode, Node};

use crate::dialect::{MethodDef, MethodVisibility};
use crate::ident::ClassId;

use super::util::{constant_id_str, flatten_statements, module_name_path, symbol_or_string_value};
use super::{IngestError, IngestResult};

const PRIVATE_HOOKS: &[&str] = &[
    "initialize", "initialize_copy", "initialize_dup", "initialize_clone",
];

#[derive(Default)]
pub(super) struct Visibility {
    // Source statement + method name also identifies synthesized attr/alias
    // definitions, which have no def-name span.
    values: HashMap<(usize, String), MethodVisibility>,
    known: HashMap<(bool, String), Vec<usize>>,
    changed: HashSet<(bool, String)>,
    accessors: HashSet<(bool, String)>,
    defaults: HashMap<usize, MethodVisibility>,
    /// Walking a concern's `ClassMethods` carrier, whose body is the
    /// includers' class side rather than a singleton of its own.
    carrier: bool,
}

pub(super) fn marker(call: &CallNode<'_>) -> bool {
    matches!(
        constant_id_str(&call.name()),
        "public" | "protected" | "private" | "public_class_method" | "private_class_method"
    )
}

/// `private def helper … end` defines a method, not a default for later
/// definitions. Leave all other forms alone for the resolution/error gate.
pub(super) fn definition<'pr>(node: &Node<'pr>) -> Option<ruby_prism::DefNode<'pr>> {
    if let Some(call) = node.as_call_node() {
        if marker(&call) && call.receiver().is_none() && call.block().is_none() {
            if let Some(args) = call.arguments() {
                let args: Vec<_> = args.arguments().iter().collect();
                if let [arg] = args.as_slice() {
                    return arg.as_def_node();
                }
            }
        }
    }
    node.as_def_node()
}

impl Visibility {
    /// Recover lexical defaults for source declarations that a later pass
    /// synthesizes into methods. Parse and walk each class body once per file.
    pub(super) fn declaration_defaults(
        source: &str,
        file: &str,
    ) -> HashMap<usize, MethodVisibility> {
        let parsed = ruby_prism::parse(source.as_bytes());
        let root = parsed.node();
        let mut defaults = HashMap::new();
        let bodies = super::util::find_all_classes_with_scope(&root)
            .into_iter()
            .filter_map(|(_, class)| class.body())
            .chain(
                super::util::find_all_module_declarations_with_scope(&root)
                    .into_iter()
                    .filter_map(|(_, module)| module.body()),
            );
        for body in bodies {
            if let Ok(visibility) = Self::resolve(Some(&body), file, None) {
                defaults.extend(visibility.defaults);
            }
            for statement in flatten_statements(body) {
                let Some(call) = statement.as_call_node() else { continue };
                if call.receiver().is_some() || constant_id_str(&call.name()) != "included" {
                    continue;
                }
                let block_body = call
                    .block()
                    .and_then(|block| block.as_block_node())
                    .and_then(|block| block.body());
                if let Ok(visibility) = Self::resolve(block_body.as_ref(), file, None) {
                    defaults.extend(visibility.defaults);
                }
            }
        }
        defaults
    }

    pub(super) fn resolve(
        body: Option<&Node<'_>>,
        file: &str,
        module_owner: Option<&ClassId>,
    ) -> IngestResult<Self> {
        let mut out = Self::default();
        if let Some(body) = body {
            let Some(statements) = body.as_statements_node() else {
                return Err(Self::unsupported(
                    file,
                    "visibility requires a static declaration body",
                ));
            };
            out.walk(Some(statements.as_node()), false, file, module_owner)?;
        }
        Ok(out)
    }

    /// `private_class_method` / `public_class_method` named this method
    /// after `module_function` copied it to the class side.
    pub(super) fn class_side_changed(&self, name: &str) -> bool {
        self.changed.contains(&(true, name.to_string()))
    }

    pub(super) fn apply(&self, statement: &Node<'_>, method: &mut MethodDef) {
        let offset = statement.location().start_offset();
        let name = method.name.to_string();
        // A `module_function` class copy is recorded one past the def so an
        // instance `private :name` cannot overwrite it. Prefer that copy
        // only after a class-method visibility change named this method.
        if self.class_side_changed(&name) {
            if let Some(value) = self.values.get(&(offset.wrapping_add(1), name.clone())) {
                method.visibility = *value;
                return;
            }
        }
        if let Some(value) = self.values.get(&(offset, name)) {
            method.visibility = *value;
        }
    }

    pub(super) fn check_model_item(&self, statement: &Node<'_>, file: &str) -> IngestResult<()> {
        if self.values.iter().any(|((offset, _), value)| {
            *offset == statement.location().start_offset() && *value != MethodVisibility::Public
        }) {
            return Err(Self::unsupported(
                file,
                "visibility of model accessors or aliases requires a local MethodDef",
            ));
        }
        Ok(())
    }

    fn unsupported(file: &str, message: impl Into<String>) -> IngestError {
        IngestError::Unsupported {
            file: file.into(),
            message: message.into(),
        }
    }

    fn define(
        &mut self,
        offset: usize,
        name: String,
        class_side: bool,
        visibility: MethodVisibility,
        file: &str,
    ) -> IngestResult<()> {
        let key = (class_side, name.clone());
        if let Some(previous) = self.known.get(&key) {
            // A repeated ordinary constructor has the same implicit
            // visibility on every definition. Keep the pre-existing duplicate
            // bodies so semantic consumers (e.g. Alba's evidence gate) can
            // reject them; a visibility marker still makes them ambiguous.
            let implicit = if !class_side && PRIVATE_HOOKS.contains(&name.as_str()) {
                MethodVisibility::Private
            } else {
                MethodVisibility::Public
            };
            if self.changed.contains(&key)
                || visibility != implicit
                || previous
                    .iter()
                    .any(|p| self.values[&(*p, name.clone())] != implicit)
            {
                return Err(Self::unsupported(
                    file,
                    format!("ambiguous visibility for redefined method `{name}`"),
                ));
            }
        }
        self.known.entry(key).or_default().push(offset);
        self.values.insert((offset, name), visibility);
        Ok(())
    }

    fn change(
        &mut self,
        name: String,
        class_side: bool,
        visibility: MethodVisibility,
        file: &str,
    ) -> IngestResult<()> {
        let key = (class_side, name.clone());
        let Some(positions) = self.known.get(&key) else {
            return Err(Self::unsupported(
                file,
                format!(
                    "visibility change for `{name}` requires an already defined local method (not forward or inherited)"
                ),
            ));
        };
        let [position] = positions.as_slice() else {
            return Err(Self::unsupported(
                file,
                format!("ambiguous visibility for redefined method `{name}`"),
            ));
        };
        self.values.insert((*position, name), visibility);
        self.changed.insert(key);
        Ok(())
    }

    fn walk_carrier(&mut self, body: Option<Node<'_>>, file: &str) -> IngestResult<()> {
        // A Concern's ClassMethods module is NOT the concern's own singleton
        // class. Their methods only share a bucket after flattening.
        let mut carrier = Self { carrier: true, ..Self::default() };
        carrier.walk(body, true, file, None)?;
        self.values.extend(carrier.values);
        Ok(())
    }

    pub(super) fn reject_conditional_declaration(node: &Node<'_>, file: &str) -> IngestResult<()> {
        Self::reject_dynamic_declarations(node, file)
    }

    /// An `if` / `unless` whose body holds a `def` or a visibility marker.
    /// A modifier (`return x if x`) does not.
    pub(super) fn hides_declaration(node: &Node<'_>) -> bool {
        struct Declarations {
            invalid: bool,
        }
        impl<'pr> ruby_prism::Visit<'pr> for Declarations {
            fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
                let body_hides = |body: Option<Node<'pr>>| {
                    body.is_some_and(|body| {
                        super::util::flatten_statements(body).iter().any(|stmt| {
                            stmt.as_def_node().is_some()
                                || stmt.as_call_node().is_some_and(|c| c.receiver().is_none() && marker(&c))
                        })
                    })
                };
                self.invalid |= if let Some(branch) = node.as_if_node() {
                    body_hides(branch.statements().map(|s| s.as_node()))
                        || branch.subsequent().is_some_and(|sub| {
                            sub.as_else_node()
                                .and_then(|e| e.statements())
                                .is_some_and(|s| body_hides(Some(s.as_node())))
                                || sub.as_if_node().is_some()
                        })
                } else if let Some(branch) = node.as_unless_node() {
                    body_hides(branch.statements().map(|s| s.as_node()))
                        || branch
                            .else_clause()
                            .and_then(|clause| clause.statements())
                            .is_some_and(|s| body_hides(Some(s.as_node())))
                } else {
                    false
                };
            }
            fn visit_def_node(&mut self, _node: &ruby_prism::DefNode<'pr>) {}
        }
        let mut declarations = Declarations { invalid: false };
        ruby_prism::Visit::visit(&mut declarations, node);
        declarations.invalid
    }

    fn reject_dynamic_declarations(node: &Node<'_>, file: &str) -> IngestResult<()> {
        struct Declarations {
            invalid: bool,
            reject_defs: bool,
        }
        impl<'pr> ruby_prism::Visit<'pr> for Declarations {
            fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
                // A modifier (`return value if value.is_a?(Kind)`) is one
                // expression, not a declaration that sometimes runs. Only a
                // branch whose body can hold a visibility marker is dynamic.
                let body_has_marker = |body: Option<Node<'pr>>| {
                    body.is_some_and(|body| {
                        super::util::flatten_statements(body).iter().any(|stmt| {
                            stmt.as_call_node().is_some_and(|c| c.receiver().is_none() && marker(&c))
                        })
                    })
                };
                self.invalid |= if let Some(branch) = node.as_if_node() {
                    body_has_marker(branch.statements().map(|s| s.as_node()))
                        || branch.subsequent().is_some_and(|sub| {
                            sub.as_else_node()
                                .and_then(|e| e.statements())
                                .is_some_and(|s| body_has_marker(Some(s.as_node())))
                                || sub.as_else_node().is_none() && body_has_marker(Some(sub))
                        })
                } else if let Some(branch) = node.as_unless_node() {
                    // `statements` is the body; `else_clause` is the else.
                    body_has_marker(branch.statements().map(|s| s.as_node()))
                        || branch
                            .else_clause()
                            .and_then(|clause| clause.statements())
                            .is_some_and(|s| body_has_marker(Some(s.as_node())))
                } else {
                    node.as_call_node().is_some_and(|c| c.receiver().is_none() && marker(&c))
                };
            }
            fn visit_def_node(&mut self, _node: &ruby_prism::DefNode<'pr>) {
                // DSL blocks (e.g. has_many extensions) own their defs, not
                // the enclosing class. Never inspect ordinary def bodies.
                self.invalid |= self.reject_defs;
            }
        }
        // A def inside a call's block belongs to that block's owner, not
        // to this class: a DSL block (has_many extensions), or the class
        // a constant is assigned from — `ContentKey = Data.define(:digest)
        // do def cache_key … end end` (campfire's FragmentCache),
        // `Struct.new(…) do … end`.
        let owns_defs = node.as_call_node().is_some()
            || node
                .as_constant_write_node()
                .is_some_and(|w| w.value().as_call_node().is_some_and(|c| c.block().is_some()));
        let mut declarations = Declarations {
            invalid: false,
            reject_defs: !owns_defs,
        };
        ruby_prism::Visit::visit(&mut declarations, node);
        if declarations.invalid {
            return Err(Self::unsupported(
                file,
                "conditional or dynamic visibility declarations are not modeled",
            ));
        }
        Ok(())
    }

    /// Resolve static method visibility within one lexical body, starting public.
    /// Nested classes and recognized Data factory blocks have separate declaration
    /// passes, so their methods and visibility markers do not affect this scope.
    fn walk(
        &mut self,
        body: Option<Node<'_>>,
        class_side: bool,
        file: &str,
        module_owner: Option<&ClassId>,
    ) -> IngestResult<()> {
        let Some(body) = body else { return Ok(()) };
        // A new lexical body always starts public. A marker in the enclosing
        // class must not privatize def self.x or leak into class_methods.
        let mut default = MethodVisibility::Public;
        let mut module_function = false;
        for statement in flatten_statements(body) {
            let offset = statement.location().start_offset();
            self.defaults.insert(offset, default);
            let def = definition(&statement);
            let node = &statement;
            let mut inline = None;
            if let Some(call) = statement.as_call_node().filter(marker) {
                if call.receiver().is_some() || call.block().is_some() {
                    return Err(Self::unsupported(
                        file,
                        "visibility requires a receiverless, static declaration",
                    ));
                }
                let kw = call.name();
                let kw = constant_id_str(&kw);
                let visibility = match kw {
                    "public" | "public_class_method" => MethodVisibility::Public,
                    "protected" => MethodVisibility::Protected,
                    _ => MethodVisibility::Private,
                };
                let named_class = kw.ends_with("_class_method");
                if class_side && named_class {
                    // `class << self` has no further singleton to address.
                    // A module's `def self` is already that singleton, so
                    // `private_class_method :jwks` after it is the same
                    // change a class body makes.
                    if module_owner.is_none() {
                        return Err(Self::unsupported(
                            file,
                            "class-method visibility on a nested singleton level is not modeled",
                        ));
                    }
                }
                if let Some(def) = &def {
                    let side = class_side || def.receiver().is_some();
                    if side != (class_side || named_class) {
                        return Err(Self::unsupported(
                            file,
                            "visibility wrapper targets the wrong method receiver",
                        ));
                    }
                    inline = Some(visibility);
                } else if let Some(args) = call.arguments() {
                    for arg in args.arguments().iter() {
                        if arg.as_call_node().is_some_and(|call| call.receiver().is_none() && marker(&call)) {
                            break;
                        }
                        let Some(name) = symbol_or_string_value(&arg) else {
                            return Err(Self::unsupported(
                                file,
                                "visibility method names must be literal symbols or strings",
                            ));
                        };
                        // `private_class_method :name` after `def self.name`.
                        // The instance-side lookup misses that normal order.
                        // A forward reference and an instance-only name still fail.
                        let class_copy = named_class
                            && !self.known.contains_key(&(false, name.clone()))
                            && self.known.contains_key(&(true, name.clone()));
                        self.change(name, class_side || named_class || class_copy, visibility, file)?;
                    }
                    continue;
                } else {
                    if named_class {
                        return Err(Self::unsupported(
                            file,
                            "class-method visibility requires local method names",
                        ));
                    }
                    default = visibility;
                    if !class_side && !named_class {
                        // A bare public/protected/private ends Ruby's
                        // module_function mode. The library walker stops
                        // promoting later defs; copy tracking must stop too.
                        module_function = false;
                    }
                    continue;
                }
            }
            if let Some(def) = def {
                if let Some(inner) = super::library_class::included_hook_class_methods_body(&def) {
                    self.walk_carrier(Some(inner), file)?;
                    continue;
                }
                if let Some(receiver) = def.receiver() {
                    if receiver.as_self_node().is_none() {
                        return Err(Self::unsupported(
                            file,
                            "visibility of a definition on a foreign receiver is not modeled",
                        ));
                    }
                    if class_side {
                        return Err(Self::unsupported(
                            file,
                            "nested singleton method definition is not modeled",
                        ));
                    }
                }
                let name = constant_id_str(&def.name()).to_string();
                let side = class_side || def.receiver().is_some();
                let key = (side, name.clone());
                // The established attr-reader → custom-def override is
                // unambiguous, unlike multiple source defs around markers.
                if self.accessors.remove(&key) && !self.changed.contains(&key) {
                    self.known.remove(&key);
                }
                let visibility = inline.unwrap_or_else(|| {
                    if !side && PRIVATE_HOOKS.contains(&name.as_str()) {
                        MethodVisibility::Private
                    } else if def.receiver().is_some() {
                        MethodVisibility::Public
                    } else {
                        default
                    }
                });
                self.define(offset, name.clone(), side, visibility, file)?;
                if module_function && !side {
                    // The library walk keeps one class-side method and looks
                    // visibility up by the def offset. A later instance
                    // `private :name` must not clobber that class copy, so
                    // the copy lives at the next offset and `apply` prefers
                    // it once `private_class_method` names the method.
                    self.define(
                        offset.wrapping_add(1),
                        name,
                        true,
                        MethodVisibility::Public,
                        file,
                    )?;
                }
                continue;
            }
            if let Some(sc) = node.as_singleton_class_node() {
                if self.carrier && class_side && sc.expression().as_self_node().is_some() {
                    // Not the includers' class side: the carrier module's
                    // own singleton, whose hooks Ruby runs when
                    // `ActiveSupport::Concern` extends an includer with it.
                    return Err(Self::unsupported(
                        file,
                        "`class << self` in a concern's `ClassMethods` defines the carrier module's own \
                         singleton methods (such as an `extended` hook), which are not modeled",
                    ));
                }
                if class_side || sc.expression().as_self_node().is_none() {
                    return Err(Self::unsupported(
                        file,
                        "visibility of a foreign or nested singleton class is not modeled",
                    ));
                }
                self.walk(sc.body(), true, file, None)?;
                continue;
            }
            // Nested classes have their own declaration pass and namespace.
            if node.as_class_node().is_some() {
                continue;
            }
            if super::data_factory::declaration(node).is_some() {
                continue;
            }
            if let Some(module) = node.as_module_node() {
                if module_name_path(&module).as_deref() == Some(&["ClassMethods".to_string()]) {
                    self.walk_carrier(module.body(), file)?;
                }
                continue;
            }
            let Some(call) = node.as_call_node() else {
                // `X = Data.define(:a) do def … end` defines X's methods,
                // not this body's; `library_class::data_block_classes`
                // gives them a class of their own.
                if node.as_constant_write_node().is_some_and(|cw| {
                    super::library_class::data_define_block(&cw.value()).is_some()
                }) {
                    continue;
                }
                // A `def` is handled above. Anything else — an `if` that
                // wraps a definition, a modifier that does not — still
                // has to be rejected when it hides a marker or a `def`.
                Self::reject_dynamic_declarations(node, file)?;
                continue;
            };
            if call.receiver().is_some() {
                continue;
            }
            let name = call.name();
            let name = constant_id_str(&name);
            if !class_side && name == "module_function" && call.arguments().is_none() {
                module_function = true;
                continue;
            }
            if !class_side && name == "module_function" {
                // `module_function :a, :b` copies already-defined instance
                // methods onto the class side. Record those names so a
                // following `private_class_method :a` addresses the copy
                // the library walk actually keeps.
                if let Some(args) = call.arguments() {
                    for arg in args.arguments().iter() {
                        let Some(copied) = symbol_or_string_value(&arg) else {
                            continue;
                        };
                        if self.known.contains_key(&(false, copied.clone()))
                            && !self.known.contains_key(&(true, copied.clone()))
                        {
                            let positions = self.known[&(false, copied.clone())].clone();
                            let copies: Vec<usize> =
                                positions.iter().map(|p| p.wrapping_add(1)).collect();
                            self.known.insert((true, copied.clone()), copies.clone());
                            for position in copies {
                                self.values
                                    .insert((position, copied.clone()), MethodVisibility::Public);
                            }
                        }
                    }
                }
                continue;
            }
            if class_side && name == "module_function" {
                // Its public copy belongs to the carrier itself, not to the
                // includer whose methods we retain after flattening.
                return Err(Self::unsupported(
                    file,
                    "module_function on a nested singleton level or class-method carrier is not modeled",
                ));
            }
            // Only collector-owned candidates have a replacement refusal gate:
            // their unclaimed block context is diagnosed per includer. Do not
            // exempt candidate-free blocks or singleton/class-method carriers.
            if name == "included" && module_owner.is_some_and(|owner| {
                call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
                    .is_some_and(|body| super::library_class::included_has_accessor(body, owner, file))
            }) {
                continue;
            }
            if name == "class_methods" {
                if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                    self.walk_carrier(block.body(), file)?;
                }
            } else if matches!(name, "attr_reader" | "attr_writer" | "attr_accessor") {
                if let Some(args) = call.arguments() {
                    for arg in args.arguments().iter() {
                        if let Some(attr) = symbol_or_string_value(&arg) {
                            if name != "attr_writer" {
                                self.define(offset, attr.clone(), class_side, default, file)?;
                                self.accessors.insert((class_side, attr.clone()));
                            }
                            if name != "attr_reader" {
                                self.define(offset, format!("{attr}="), class_side, default, file)?;
                                self.accessors.insert((class_side, format!("{attr}=")));
                            }
                        }
                    }
                }
            } else if name == "alias_method" {
                if let Some(args) = call.arguments() {
                    let args: Vec<_> = args.arguments().iter().collect();
                    if let [to, from] = args.as_slice() {
                        if let (Some(to), Some(from)) =
                            (symbol_or_string_value(to), symbol_or_string_value(from))
                        {
                            if let Some(positions) = self.known.get(&(class_side, from.clone())) {
                                let visibility = self.values[&(*positions.last().unwrap(), from)];
                                self.define(offset, to, class_side, visibility, file)?;
                            }
                        }
                    }
                }
            } else if statement.as_def_node().is_none() {
                Self::reject_dynamic_declarations(node, file)?;
            }
        }
        Ok(())
    }
}
