//! Method names an app's source defines, for passes that must know a
//! framework-generated method has no app definition that could run in
//! its place.
//!
//! Deliberately syntactic and over-inclusive: every `def` anywhere (any
//! receiver, any nesting), every `define_method`/`alias_method`/
//! `attr_*`/`delegate` call with a literal name, and every `alias`. A
//! name it misses is one the app defines dynamically (`define_method(name)`
//! with a computed name), which a pass relying on this must treat as
//! unknown, so [`collect`] also records those calls under `"*"`.

use crate::det::HashMap;
use std::path::Path;

use ruby_prism::Visit;

use crate::span::SourceFile;

use crate::ingest::prism_api::PrismLocation;
/// The name [`collect`] records a definition under when the name is
/// computed at runtime.
pub const DYNAMIC: &str = "*";

/// Add the names `source` defines to `definitions`, each with its `path:line`
/// (relative to `root`).
pub fn collect(source: &SourceFile, root: &Path, definitions: &mut HashMap<String, Vec<String>>) {
    let parsed = ruby_prism::parse(source.text.as_bytes());
    let path = Path::new(&source.path).strip_prefix(root).map(|relative| relative.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| source.path.clone());
    let mut collector = Collector { found: Vec::new() };
    collector.visit(&parsed.node());
    for (name, offset) in collector.found {
        let line = source.text[..offset.min(source.text.len())].matches('\n').count() + 1;
        definitions.entry(name).or_default().push(format!("{path}:{line}"));
    }
}

struct Collector {
    found: Vec<(String, usize)>,
}

const NAMING_CALLS: &[&str] = &["define_method", "define_singleton_method", "alias_method", "attr_reader", "attr_writer", "attr_accessor", "attr", "delegate", "delegate_missing_to"];

impl<'parse> Visit<'parse> for Collector {
    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'parse>) {
        self.found.push((text(node.name().as_slice()), node.location().start_offset()));
        ruby_prism::visit_def_node(self, node);
    }

    fn visit_alias_method_node(&mut self, node: &ruby_prism::AliasMethodNode<'parse>) {
        match literal_name(&node.new_name()) {
            Some(name) => self.found.push((name, node.location().start_offset())),
            None => self.found.push((DYNAMIC.to_string(), node.location().start_offset())),
        }
        ruby_prism::visit_alias_method_node(self, node);
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'parse>) {
        let name = text(node.name().as_slice());
        if NAMING_CALLS.contains(&name.as_str()) {
            let at = node.location().start_offset();
            let arguments: Vec<_> = node.arguments().map(|arguments_node| arguments_node.arguments().iter().collect()).unwrap_or_default();
            match name.as_str() {
                // The new name is the first argument.
                "define_method" | "define_singleton_method" | "alias_method" => match arguments.first().and_then(literal_name) {
                    Some(defined) => self.found.push((defined, at)),
                    None => self.found.push((DYNAMIC.to_string(), at)),
                },
                // Every positional argument names a method (and a writer
                // for the writing forms).
                "attr_reader" | "attr_writer" | "attr_accessor" | "attr" => {
                    for argument in &arguments {
                        match literal_name(argument) {
                            Some(defined) => {
                                if name != "attr_reader" && name != "attr" {
                                    self.found.push((format!("{defined}="), at));
                                }
                                if name != "attr_writer" {
                                    self.found.push((defined, at));
                                }
                            }
                            None => self.found.push((DYNAMIC.to_string(), at)),
                        }
                    }
                }
                // `delegate :a, :b, to: :x, prefix: ...`: a prefix changes
                // the names, so any prefix makes them unknown here.
                "delegate" => {
                    let prefixed = arguments.iter().any(|argument| {
                        argument.as_keyword_hash_node().is_some_and(|keyword_hash| {
                            keyword_hash.elements().iter().any(|element| element.as_assoc_node().is_some_and(|pair| literal_name(&pair.key()).as_deref() == Some("prefix")))
                        })
                    });
                    for argument in arguments.iter().filter(|argument| argument.as_keyword_hash_node().is_none()) {
                        match literal_name(argument) {
                            Some(defined) if !prefixed => self.found.push((defined, at)),
                            _ => self.found.push((DYNAMIC.to_string(), at)),
                        }
                    }
                }
                _ => self.found.push((DYNAMIC.to_string(), at)),
            }
        }
        ruby_prism::visit_call_node(self, node);
    }
}

/// A symbol or plain string literal's value.
pub(super) fn literal_name(node: &ruby_prism::Node<'_>) -> Option<String> {
    if let Some(symbol) = node.as_symbol_node() {
        return Some(String::from_utf8_lossy(symbol.unescaped()).into_owned());
    }
    if let Some(string) = node.as_string_node() {
        return Some(String::from_utf8_lossy(string.unescaped()).into_owned());
    }
    None
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<String> {
        let mut definitions = HashMap::default();
        collect(&SourceFile { path: "/app/x.rb".into(), text: source.into() }, Path::new("/app"), &mut definitions);
        let mut names: Vec<String> = definitions.into_keys().collect();
        names.sort();
        names
    }

    #[test]
    fn collects_every_spelling_of_a_definition() {
        let source = "class A\n  def a?; end\n  def self.b; end\n  define_method(:c) { }\n  alias d a?\n  alias_method :e, :a?\n  attr_accessor :f\n  delegate :g, to: :x\n  Module.new { def h; end }\nend\n";
        assert_eq!(names(source), ["a?", "b", "c", "d", "e", "f", "f=", "g", "h"]);
    }

    #[test]
    fn computed_names_are_recorded_as_dynamic() {
        assert_eq!(names("define_method(name) { }\n"), [DYNAMIC]);
        assert_eq!(names("delegate :a, to: :b, prefix: true\n"), [DYNAMIC]);
    }
}
