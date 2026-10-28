//! Where the app's own source defines a method named `delegate` that a
//! class could call in its body in place of Active Support's
//! `Module#delegate`.
//!
//! `delegate :name, to: :shop` in a class body calls `delegate` on the
//! class, so only definitions on a class's singleton chain matter:
//! singleton methods, `class << X` bodies, reopenings of `Module`, `Class`,
//! `Object`, `BasicObject` and `Kernel`, top-level `def`s, and modules that
//! can end up extended into a class. An instance method of an ordinary class
//! (a controller action named `delegate`, an `attr_reader :delegate`) cannot,
//! and does not count.
//!
//! The scan is syntactic and leans towards counting a definition: one
//! inside a block counts, wherever the block is. It sees only literal names:
//! a method whose name is computed at run time (`define_method(name)`) is
//! not counted, because a large app generates thousands of those and none
//! could be told apart from a `delegate` without running the app. A module counts
//! when it can reach a singleton chain ([`module_reaches_singleton_chain`]):
//! it is named `ClassMethods` (which `ActiveSupport::Concern` extends), calls
//! `extend self` or `module_function` with no names, is named by an `extend`
//! call, or is included or prepended
//! in a `class << X` body, on `singleton_class`, into `Class`, `Module` or
//! another class every class's chain reaches, inside a block, or into another
//! module that can itself reach one, followed to a fixed point. A naming call
//! with an explicit receiver (`Order.singleton_class.define_method(:delegate)`)
//! or through `send` counts wherever it is.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ruby_prism::Visit;

use crate::ingest::prism_api::PrismLocation;
use crate::span::SourceFile;

use super::super::definitions::literal_name;

/// Classes and modules every class's singleton chain reaches.
const EVERY_CLASS_ANCESTORS: &[&str] = &["Module", "Class", "Object", "BasicObject", "Kernel"];

const NAMING_CALLS: &[&str] = &["attr_reader", "attr_writer", "attr_accessor", "attr", "alias_method", "define_method"];

/// Calls that invoke the method named by their first argument.
const SEND_CALLS: &[&str] = &["send", "__send__", "public_send"];

/// Calls that stop method lookup at the class they are made on, so a
/// `delegate` call there would reach `method_missing` instead of Active
/// Support.
const UNDEFINING_CALLS: &[&str] = &["undef_method"];

/// `path:line` of every definition of `delegate` that a class body could
/// call instead of Active Support's, in the order found.
pub fn blocking_definitions(sources: &[SourceFile], root: &Path) -> Vec<String> {
    let mut blocking = Vec::new();
    // Module short name -> the `path:line` sites that define `delegate` in it.
    let mut module_definitions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Modules (by short name) whose own body calls `extend self`.
    let mut self_extended_modules: BTreeSet<String> = BTreeSet::new();
    for source in sources {
        let text = source.text.as_str();
        // Files that might define a `delegate`, or call `extend self`.
        if !text.contains("delegate") && !text.contains("define_method") && !text.contains("define_singleton_method") && !text.contains("extend self") {
            continue;
        }
        let parsed = ruby_prism::parse(text.as_bytes());
        let mut finder = DefinitionFinder {
            source,
            root,
            scopes: vec![Scope::TopLevel],
            blocking: &mut blocking,
            module_definitions: &mut module_definitions,
            self_extended_modules: &mut self_extended_modules,
        };
        finder.visit(&parsed.node());
    }
    for (module_name, sites) in module_definitions {
        if module_reaches_singleton_chain(sources, &module_name, &self_extended_modules) {
            blocking.extend(sites);
        }
    }
    blocking
}

#[derive(Clone)]
enum Scope {
    TopLevel,
    /// A `class` body: whether every class's singleton chain reaches it.
    Class { reaches_every_class: bool },
    /// A `module` body, by its last name segment.
    Module { name: String },
    /// A `class << X` body.
    Singleton,
    /// A block: what `self` is inside it is not known from the source.
    Block,
}

struct DefinitionFinder<'scan> {
    source: &'scan SourceFile,
    root: &'scan Path,
    scopes: Vec<Scope>,
    blocking: &'scan mut Vec<String>,
    module_definitions: &'scan mut BTreeMap<String, Vec<String>>,
    self_extended_modules: &'scan mut BTreeSet<String>,
}

impl DefinitionFinder<'_> {
    fn site(&self, offset: u32) -> String {
        let path = Path::new(&self.source.path).strip_prefix(self.root).map(|relative| relative.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| self.source.path.clone());
        format!("{path}:{}", self.source.line_col(offset).0)
    }

    /// Record a definition of `delegate` (or of a name computed at run
    /// time) at `offset`, defined on the current scope's instances unless
    /// `on_singleton`.
    fn record(&mut self, offset: u32, on_singleton: bool) {
        let site = self.site(offset);
        match self.scopes.last().cloned().unwrap_or(Scope::TopLevel) {
            _ if on_singleton => self.blocking.push(site),
            Scope::TopLevel | Scope::Singleton | Scope::Block => self.blocking.push(site),
            Scope::Class { reaches_every_class } => {
                if reaches_every_class {
                    self.blocking.push(site);
                }
            }
            Scope::Module { name } => self.module_definitions.entry(name).or_default().push(site),
        }
    }

    fn with_scope(&mut self, scope: Scope, visit: impl FnOnce(&mut Self)) {
        self.scopes.push(scope);
        visit(self);
        self.scopes.pop();
    }
}

/// The last segment of a constant path's name (`Admin::Report` -> `Report`).
fn last_segment(node: &ruby_prism::Node<'_>) -> Option<String> {
    if let Some(read) = node.as_constant_read_node() {
        return Some(String::from_utf8_lossy(read.name().as_slice()).into_owned());
    }
    node.as_constant_path_node().and_then(|path| path.name()).map(|name| String::from_utf8_lossy(name.as_slice()).into_owned())
}

impl<'parse> Visit<'parse> for DefinitionFinder<'_> {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'parse>) {
        let name = last_segment(&node.constant_path()).unwrap_or_default();
        self.with_scope(Scope::Class { reaches_every_class: EVERY_CLASS_ANCESTORS.contains(&name.as_str()) }, |finder| {
            ruby_prism::visit_class_node(finder, node)
        });
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'parse>) {
        let name = last_segment(&node.constant_path()).unwrap_or_default();
        let scope = if EVERY_CLASS_ANCESTORS.contains(&name.as_str()) { Scope::Class { reaches_every_class: true } } else { Scope::Module { name } };
        self.with_scope(scope, |finder| ruby_prism::visit_module_node(finder, node));
    }

    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'parse>) {
        self.with_scope(Scope::Singleton, |finder| ruby_prism::visit_singleton_class_node(finder, node));
    }

    fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'parse>) {
        self.with_scope(Scope::Block, |finder| ruby_prism::visit_block_node(finder, node));
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'parse>) {
        if node.name().as_slice() == b"delegate" {
            self.record(node.location().start_offset() as u32, node.receiver().is_some());
        }
        // A def inside a def defines on the outer method's receiver when it
        // runs, which the source does not say.
        self.with_scope(Scope::Block, |finder| ruby_prism::visit_def_node(finder, node));
    }

    fn visit_undef_node(&mut self, node: &ruby_prism::UndefNode<'parse>) {
        if node.names().iter().any(|name| literal_name(&name).as_deref() == Some("delegate")) {
            self.record(node.location().start_offset() as u32, false);
        }
        ruby_prism::visit_undef_node(self, node);
    }

    fn visit_alias_method_node(&mut self, node: &ruby_prism::AliasMethodNode<'parse>) {
        if literal_name(&node.new_name()).is_some_and(|name| name == "delegate") {
            self.record(node.location().start_offset() as u32, false);
        }
        ruby_prism::visit_alias_method_node(self, node);
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'parse>) {
        let offset = node.location().start_offset() as u32;
        let on_self = node.receiver().is_none_or(|receiver| receiver.as_self_node().is_some());
        let mut method = String::from_utf8_lossy(node.name().as_slice()).into_owned();
        let mut arguments: Vec<ruby_prism::Node<'_>> = node.arguments().map(|arguments| arguments.arguments().iter().collect()).unwrap_or_default();
        // `send(:define_method, :delegate)` is the call it names, made on the
        // same receiver.
        if SEND_CALLS.contains(&method.as_str())
            && let Some(sent) = arguments.first().and_then(literal_name)
        {
            method = sent;
            arguments.remove(0);
        }
        let names_delegate = || {
            let names: Vec<Option<String>> = if method == "alias_method" || method == "define_method" || method == "define_singleton_method" {
                arguments.first().map(literal_name).into_iter().collect()
            } else {
                arguments.iter().map(literal_name).collect()
            };
            names.iter().any(|name| name.as_deref() == Some("delegate"))
        };
        if method == "define_singleton_method" {
            // A singleton method named `delegate`, on any object.
            if names_delegate() {
                self.record(offset, true);
            }
        } else if method == "delegate" {
            // `delegate :delegate, to: :engine` defines a `delegate` method too.
            if delegate_call_defines_delegate(&arguments) {
                self.record(offset, !on_self);
            }
        } else if UNDEFINING_CALLS.contains(&method.as_str()) {
            if names_delegate() {
                self.record(offset, !on_self);
            }
        } else if NAMING_CALLS.contains(&method.as_str()) {
            if names_delegate() {
                // With an explicit receiver (`Order.singleton_class.define_method`)
                // it may be any class's singleton chain.
                self.record(offset, !on_self);
            }
        } else if method == "module_function" && on_self {
            if names_delegate() {
                // `module_function :delegate` copies it onto the module's
                // singleton, which a body of that module calls.
                self.record(offset, true);
            } else if arguments.iter().all(|argument| argument.as_splat_node().is_some())
                && let Some(Scope::Module { name }) = self.scopes.last().cloned()
            {
                // With no names (or only splats, which may expand to none),
                // every def after it is copied onto the module's singleton
                // too: counted like `extend self`, whichever side of it the
                // def is on.
                self.self_extended_modules.insert(name);
            }
        } else if method == "extend" && on_self && arguments.iter().any(|argument| argument.as_self_node().is_some()) {
            // `extend self` in a module body makes its methods singleton
            // methods of the module, which a class could reach as a constant.
            if let Some(Scope::Module { name }) = self.scopes.last().cloned() {
                self.self_extended_modules.insert(name);
            }
        }
        ruby_prism::visit_call_node(self, node);
    }
}

/// Does a `delegate` call with these arguments define a method named
/// `delegate`? Only a literal `:delegate` without a prefix: Active Support
/// joins a prefix with `_`, and `delegate` has none. As with
/// `define_method(name)`, names the source does not spell out
/// (`delegate(*NAMES, to: :helpers)`) are not counted.
fn delegate_call_defines_delegate(arguments: &[ruby_prism::Node<'_>]) -> bool {
    let (options, methods) = match arguments.split_last() {
        Some((last, rest)) if last.as_keyword_hash_node().is_some() => (last.as_keyword_hash_node(), rest),
        _ => (None, arguments),
    };
    let prefixed = options.is_some_and(|options| {
        options.elements().iter().any(|element| {
            element.as_assoc_node().is_some_and(|pair| {
                literal_name(&pair.key()).as_deref() == Some("prefix") && pair.value().as_false_node().is_none() && pair.value().as_nil_node().is_none()
            })
        })
    });
    !prefixed && methods.iter().any(|method| literal_name(method).as_deref() == Some("delegate"))
}

/// Can the module whose last name segment is `module_name` end up on an
/// object's singleton chain? Directly: it is named `ClassMethods`, calls
/// `extend self` (`self_extended_modules`), is named by an `extend` call, or
/// is included or prepended in a `class << X` body, on `singleton_class`,
/// into a class every class's chain reaches (`Class.prepend M`, or `include
/// M` in a `class Module` body), or inside a block. Transitively: it is
/// included or prepended into another module that can, followed to a fixed
/// point. Names are matched by last segment, so a namesake counts too.
fn module_reaches_singleton_chain(sources: &[SourceFile], module_name: &str, self_extended_modules: &BTreeSet<String>) -> bool {
    let mut pending = vec![module_name.to_string()];
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        if name == "ClassMethods" || self_extended_modules.contains(&name) {
            return true;
        }
        for source in sources.iter().filter(|source| source.text.contains(name.as_str())) {
            let parsed = ruby_prism::parse(source.text.as_bytes());
            let mut finder = MixinFinder { module_name: &name, scopes: vec![Scope::TopLevel], reaches_singleton_chain: false, including_modules: Vec::new() };
            finder.visit(&parsed.node());
            if finder.reaches_singleton_chain {
                return true;
            }
            pending.extend(finder.including_modules);
        }
    }
    false
}

/// Where one file mixes the module named `module_name` in.
struct MixinFinder<'name> {
    module_name: &'name str,
    scopes: Vec<Scope>,
    /// Mixed into a singleton chain, or somewhere that could be one.
    reaches_singleton_chain: bool,
    /// Modules (by last name segment) that include or prepend it, or call
    /// `extend self` after it is mixed in.
    including_modules: Vec<String>,
}

impl MixinFinder<'_> {
    fn with_scope(&mut self, scope: Scope, visit: impl FnOnce(&mut Self)) {
        self.scopes.push(scope);
        visit(self);
        self.scopes.pop();
    }
}

impl<'parse> Visit<'parse> for MixinFinder<'_> {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'parse>) {
        let name = last_segment(&node.constant_path()).unwrap_or_default();
        self.with_scope(Scope::Class { reaches_every_class: EVERY_CLASS_ANCESTORS.contains(&name.as_str()) }, |finder| {
            ruby_prism::visit_class_node(finder, node)
        });
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'parse>) {
        let name = last_segment(&node.constant_path()).unwrap_or_default();
        let scope = if EVERY_CLASS_ANCESTORS.contains(&name.as_str()) { Scope::Class { reaches_every_class: true } } else { Scope::Module { name } };
        self.with_scope(scope, |finder| ruby_prism::visit_module_node(finder, node));
    }

    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'parse>) {
        self.with_scope(Scope::Singleton, |finder| ruby_prism::visit_singleton_class_node(finder, node));
    }

    fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'parse>) {
        self.with_scope(Scope::Block, |finder| ruby_prism::visit_block_node(finder, node));
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'parse>) {
        self.with_scope(Scope::Block, |finder| ruby_prism::visit_def_node(finder, node));
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'parse>) {
        let mut method = String::from_utf8_lossy(node.name().as_slice()).into_owned();
        let mut arguments: Vec<ruby_prism::Node<'_>> = node.arguments().map(|arguments| arguments.arguments().iter().collect()).unwrap_or_default();
        if SEND_CALLS.contains(&method.as_str())
            && let Some(sent) = arguments.first().and_then(literal_name)
        {
            method = sent;
            arguments.remove(0);
        }
        let mixes = matches!(method.as_str(), "extend" | "include" | "prepend");
        let names_module = arguments.iter().any(|argument| last_segment(argument).as_deref() == Some(self.module_name));
        if mixes && names_module {
            let receiver = node.receiver();
            let on_singleton_class = receiver.as_ref().is_some_and(|receiver| {
                receiver.as_call_node().is_some_and(|call| call.name().as_slice() == b"singleton_class")
            });
            let on_every_class_ancestor = receiver
                .as_ref()
                .and_then(last_segment)
                .is_some_and(|name| EVERY_CLASS_ANCESTORS.contains(&name.as_str()));
            let on_self = receiver.as_ref().is_none_or(|receiver| receiver.as_self_node().is_some());
            if method == "extend" || on_singleton_class || on_every_class_ancestor {
                self.reaches_singleton_chain = true;
            } else if on_self {
                match self.scopes.last().cloned().unwrap_or(Scope::TopLevel) {
                    Scope::Module { name } => self.including_modules.push(name),
                    Scope::Class { reaches_every_class } => self.reaches_singleton_chain |= reaches_every_class,
                    Scope::Singleton | Scope::Block | Scope::TopLevel => self.reaches_singleton_chain = true,
                }
            } else {
                // Mixed into some other object, which may be a class.
                self.reaches_singleton_chain = true;
            }
        }
        ruby_prism::visit_call_node(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocking(files: &[(&str, &str)]) -> Vec<String> {
        let sources: Vec<SourceFile> = files.iter().map(|(path, text)| SourceFile { path: format!("/app/{path}"), text: text.to_string() }).collect();
        blocking_definitions(&sources, Path::new("/app"))
    }

    #[test]
    fn instance_methods_of_ordinary_classes_do_not_count() {
        let files = [
            ("controller.rb", "class TokensController\n  def delegate; end\nend\n"),
            ("filter.rb", "class Filter\n  private\n  attr_reader :delegate\nend\n"),
            ("mutable.rb", "module Mutable\n  attr_accessor :delegate\nend\n"),
            ("user.rb", "class User\n  include Mutable\nend\n"),
        ];
        assert_eq!(blocking(&files), Vec::<String>::new());
    }

    #[test]
    fn naming_calls_with_an_explicit_receiver_or_through_send_count() {
        let files = [
            ("singleton_class.rb", "Order.singleton_class.define_method(:delegate) { |*| }\n"),
            ("alias.rb", "Order.singleton_class.alias_method :delegate, :custom_delegate\n"),
            ("sent.rb", "class Order\n  singleton_class.send(:define_method, :delegate) { |*| }\nend\n"),
            ("instance.rb", "class Order\n  send(:attr_reader, :delegate)\nend\n"),
            ("macro.rb", "class Order\n  class << self\n    delegate :delegate, to: :engine\n  end\nend\n"),
            // A prefix is joined with `_`, so it can never make `delegate`.
            ("prefixed.rb", "class Order\n  class << self\n    delegate :gate, to: :engine, prefix: :dele\n  end\nend\n"),
            ("other.rb", "class Order\n  class << self\n    delegate :name, to: :engine\n  end\nend\n"),
            ("splatted.rb", "module Helping\n  included do\n    delegate(*HELPERS, to: :helpers)\n  end\nend\n"),
            ("undefined.rb", "class Order\n  singleton_class.undef_method(:delegate)\nend\n"),
        ];
        assert_eq!(blocking(&files), ["singleton_class.rb:1", "alias.rb:1", "sent.rb:2", "macro.rb:3", "undefined.rb:2"]);
    }

    #[test]
    fn modules_mixed_into_every_class_or_through_other_modules_count() {
        let override_module = "module DelegationOverride\n  def delegate(*) = nil\nend\n";
        for (mixer, reaches) in [
            ("Class.include DelegationOverride\n", true),
            ("Class.prepend DelegationOverride\n", true),
            ("Module.prepend(DelegationOverride)\n", true),
            ("class Module\n  prepend DelegationOverride\nend\n", true),
            ("module ClassHooks\n  include DelegationOverride\nend\nclass Order\n  extend ClassHooks\nend\n", true),
            ("module Outer\n  prepend DelegationOverride\nend\nmodule ClassHooks\n  include Outer\nend\nclass Order\n  singleton_class.include ClassHooks\nend\n", true),
            ("module Helpers\n  extend self\n  include DelegationOverride\nend\n", true),
            ("module Helpers\n  include DelegationOverride\nend\nclass Order\n  include Helpers\nend\n", false),
        ] {
            let found = blocking(&[("override.rb", override_module), ("mixer.rb", mixer)]);
            assert_eq!(!found.is_empty(), reaches, "{mixer}: {found:?}");
        }
    }

    #[test]
    fn module_function_copies_count() {
        let files = [
            ("named.rb", "module Named\n  def delegate(*) = nil\n  module_function :delegate\nend\n"),
            ("section.rb", "module Section\n  module_function\n\n  def delegate(*) = nil\nend\n"),
            ("other.rb", "module Other\n  def delegate(*) = nil\n  module_function :helper\nend\n"),
        ];
        assert_eq!(blocking(&files), ["named.rb:3", "section.rb:4"]);
    }

    #[test]
    fn definitions_a_class_body_could_call_count() {
        let files = [
            ("singleton.rb", "class A\n  def self.delegate(*) = nil\nend\n"),
            ("eigen.rb", "class B\n  class << self\n    def delegate(*) = nil\n  end\nend\n"),
            ("module_patch.rb", "class Module\n  def delegate(*) = nil\nend\n"),
            ("top.rb", "def delegate(*) = nil\n"),
            ("concern.rb", "module Concern\n  class_methods do\n    def delegate(*) = nil\n  end\nend\n"),
            ("class_methods.rb", "module Thing\n  module ClassMethods\n    def delegate(*) = nil\n  end\nend\n"),
            ("extended.rb", "module Delegating\n  def delegate(*) = nil\nend\n"),
            ("user.rb", "class User\n  extend Delegating\nend\n"),
            ("literal.rb", "class C\n  define_singleton_method(:delegate) { }\nend\n"),
            ("computed.rb", "class D\n  names.each { |name| define_method(name) { } }\n  define_singleton_method(name) { }\nend\n"),
        ];
        assert_eq!(
            blocking(&files),
            ["singleton.rb:2", "eigen.rb:3", "module_patch.rb:2", "top.rb:1", "concern.rb:3", "literal.rb:2", "class_methods.rb:3", "extended.rb:2"]
        );
    }
}
