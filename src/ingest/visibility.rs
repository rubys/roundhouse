//! Resolve local visibility while declaration order and lexical scopes still
//! exist. Never replay these calls from LibraryClass::unknown_calls.

use std::collections::{HashMap, HashSet};

use ruby_prism::{CallNode, Node};

use crate::dialect::{MethodDef, MethodVisibility};

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
    pub(super) fn resolve(body: Option<&Node<'_>>, file: &str) -> IngestResult<Self> {
        let mut out = Self::default();
        if let Some(body) = body {
            let Some(statements) = body.as_statements_node() else {
                return Err(Self::unsupported(
                    file,
                    "visibility requires a static declaration body",
                ));
            };
            out.walk(Some(statements.as_node()), false, file)?;
        }
        Ok(out)
    }

    pub(super) fn apply(&self, statement: &Node<'_>, method: &mut MethodDef) {
        if let Some(value) = self
            .values
            .get(&(statement.location().start_offset(), method.name.to_string()))
        {
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
        let mut carrier = Self::default();
        carrier.walk(body, true, file)?;
        self.values.extend(carrier.values);
        Ok(())
    }

    fn reject_dynamic_declarations(node: &Node<'_>, file: &str) -> IngestResult<()> {
        struct Declarations {
            invalid: bool,
            reject_defs: bool,
        }
        impl<'pr> ruby_prism::Visit<'pr> for Declarations {
            fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
                self.invalid |= node
                    .as_call_node()
                    .is_some_and(|c| c.receiver().is_none() && marker(&c));
            }
            fn visit_def_node(&mut self, _node: &ruby_prism::DefNode<'pr>) {
                // DSL blocks (e.g. has_many extensions) own their defs, not
                // the enclosing class. Never inspect ordinary def bodies.
                self.invalid |= self.reject_defs;
            }
        }
        let mut declarations = Declarations {
            invalid: false,
            reject_defs: node.as_call_node().is_none(),
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

    fn walk(&mut self, body: Option<Node<'_>>, class_side: bool, file: &str) -> IngestResult<()> {
        let Some(body) = body else { return Ok(()) };
        // A new lexical body always starts public. A marker in the enclosing
        // class must not privatize def self.x or leak into class_methods.
        let mut default = MethodVisibility::Public;
        for statement in flatten_statements(body) {
            let offset = statement.location().start_offset();
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
                    // These address the singleton of the current carrier,
                    // not the methods flattened from its instance side.
                    return Err(Self::unsupported(
                        file,
                        "class-method visibility on a nested singleton level is not modeled",
                    ));
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
                        let Some(name) = symbol_or_string_value(&arg) else {
                            return Err(Self::unsupported(
                                file,
                                "visibility method names must be literal symbols or strings",
                            ));
                        };
                        self.change(name, class_side || named_class, visibility, file)?;
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
                self.define(offset, name, side, visibility, file)?;
                continue;
            }
            if let Some(sc) = node.as_singleton_class_node() {
                if class_side || sc.expression().as_self_node().is_none() {
                    return Err(Self::unsupported(
                        file,
                        "visibility of a foreign or nested singleton class is not modeled",
                    ));
                }
                self.walk(sc.body(), true, file)?;
                continue;
            }
            // Nested classes have their own declaration pass and namespace.
            if node.as_class_node().is_some() {
                continue;
            }
            if let Some(module) = node.as_module_node() {
                if module_name_path(&module).as_deref() == Some(&["ClassMethods".to_string()]) {
                    self.walk_carrier(module.body(), file)?;
                }
                continue;
            }
            let Some(call) = node.as_call_node() else {
                Self::reject_dynamic_declarations(node, file)?;
                continue;
            };
            if call.receiver().is_some() {
                continue;
            }
            let name = call.name();
            let name = constant_id_str(&name);
            if class_side && name == "module_function" {
                // Its public copy belongs to the carrier itself, not to the
                // includer whose methods we retain after flattening.
                return Err(Self::unsupported(
                    file,
                    "module_function on a nested singleton level or class-method carrier is not modeled",
                ));
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
            } else {
                Self::reject_dynamic_declarations(node, file)?;
            }
        }
        Ok(())
    }
}
