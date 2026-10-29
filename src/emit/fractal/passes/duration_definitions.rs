//! Where the app's own source defines a method that could run in place of
//! one of the Active Support methods `lower-duration` takes out of a call,
//! or newly calls: `Numeric#days` and its siblings, `Integer#months` and
//! `#years`, the `ActiveSupport::Duration` constructors, `Duration#ago`,
//! `#since`, their aliases and the private `#sum` they call, and the `Time`,
//! `TimeWithZone` and `TimeZone` methods between `Time.current` and Ruby's
//! own `Time` arithmetic (`Time.current`, `#since`, `#advance`, `#change`,
//! `#plus_without_duration`, `TimeZone#now`).
//!
//! Only definitions that can land on one of [`TARGETS`] (or its singleton
//! class) matter. An instance method `days` of an ordinary class cannot, and
//! neither can a top-level `def`, which defines a private method of `Object`
//! that the targets' own methods come before. A definition counts when it is:
//!
//! - in a `class` or `module` body for one of them (reopened by its
//!   full lexical name: `class Integer`, or `class Duration` inside
//!   `module ActiveSupport`), its `class << self`, or a block inside it;
//! - made on one of them as an explicit receiver (`def Integer.x`,
//!   `Integer.define_method(:days)`, `Integer.singleton_class.alias_method`),
//!   or in a block passed to a call on one (`Integer.class_eval do ... end`);
//! - in a module mixed into one of them (`include`, `prepend` or `extend`, in
//!   its body or with it as the receiver), found by the module's last name
//!   segment and followed through modules that include it, to a fixed point.
//!
//! The scan is syntactic. It sees literal names and constant receivers only:
//! a method defined under a name computed at run time, or on a class held in
//! a variable (`klass.class_eval`), is not seen. Refinements are left to the
//! pass, which declines any file that activates one with `using`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ruby_prism::Visit;

use crate::ingest::prism_api::PrismLocation;
use crate::span::SourceFile;

use super::super::definitions::literal_name;

/// The classes whose methods `lower-duration` relies on keeping Active
/// Support's definitions, by full name.
pub const TARGETS: &[&str] = &["Integer", "Numeric", "ActiveSupport::Duration", "Time", "ActiveSupport::TimeWithZone", "ActiveSupport::TimeZone"];

/// Calls that define, alias or remove the methods their arguments name.
const NAMING_CALLS: &[&str] = &[
    "define_method", "define_singleton_method", "alias_method", "attr_reader", "attr_writer", "attr_accessor", "attr", "undef_method",
    "remove_method",
];

/// Calls that invoke the method named by their first argument.
const SEND_CALLS: &[&str] = &["send", "__send__", "public_send"];

const MIXING_CALLS: &[&str] = &["include", "prepend", "extend"];

/// Each of `names` the app defines where it could run in place of Active
/// Support's on one of [`TARGETS`], with the
/// `path:line` of every such definition, in the order found.
pub fn blocking_definitions(sources: &[SourceFile], root: &Path, names: &BTreeSet<String>) -> BTreeMap<String, Vec<String>> {
    let mut sites = BTreeMap::new();
    // Modules (by last name segment) mixed into a target, still to be read.
    let mut pending: Vec<String> = Vec::new();
    scan(sources, root, names, &Targets::Classes, &mut sites, &mut pending);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(module_name) = pending.pop() {
        if !seen.insert(module_name.clone()) {
            continue;
        }
        scan(sources, root, names, &Targets::Module(module_name), &mut sites, &mut pending);
    }
    sites
}

/// What a scan counts definitions on.
enum Targets {
    /// [`TARGETS`], matched by full lexical name.
    Classes,
    /// A module mixed into a target, matched by last name segment.
    Module(String),
}

impl Targets {
    /// Does a `class`/`module` body or a constant receiver named `path`
    /// (segments as written; `absolute` for a leading `::`), seen inside the
    /// lexical `namespace`, name a target?
    fn matches(&self, path: &[String], absolute: bool, namespace: &[String]) -> bool {
        match self {
            Targets::Classes => {
                let written = path.join("::");
                let mut candidates = vec![written.clone()];
                if !absolute {
                    for depth in 1..=namespace.len() {
                        candidates.push(format!("{}::{written}", namespace[..depth].join("::")));
                    }
                }
                candidates.iter().any(|candidate| TARGETS.contains(&candidate.as_str()))
            }
            Targets::Module(name) => path.last() == Some(name),
        }
    }

    /// Could `text` name a target where a definition could land on it? A
    /// cheap, over-inclusive filter before parsing: it must never leave out
    /// a file the scan would count something in.
    fn may_be_named_in(&self, text: &str) -> bool {
        match self {
            Targets::Module(name) => text.contains(name.as_str()),
            // `Duration`, `TimeWithZone` and `TimeZone` anywhere. `Integer`,
            // `Numeric` and `Time` appear in most files' type annotations, so
            // only where a reopening or a call could use them.
            Targets::Classes => {
                ["Duration", "TimeWithZone", "TimeZone"].iter().any(|name| text.contains(name))
                    || ["Integer", "Numeric", "Time"].iter().any(|name| names_as_receiver_or_body(text, name))
            }
        }
    }
}

/// Is `name`, as a whole identifier outside a whole-line comment, somewhere
/// in `text` after `class` or `<<` (with or without a leading `::`), or
/// before `.`, `&.` or `::`, with only whitespace (or a line continuation)
/// between? `::Integer` alone is a type annotation, not one of these.
fn names_as_receiver_or_body(text: &str, name: &str) -> bool {
    let is_identifier = |character: char| character.is_alphanumeric() || character == '_';
    let is_space = |character: char| character.is_whitespace() || character == '\\';
    text.match_indices(name).any(|(start, _)| {
        let end = start + name.len();
        let before = &text[..start];
        let after = &text[end..];
        if before.chars().next_back().is_some_and(is_identifier) || after.chars().next().is_some_and(is_identifier) {
            return false;
        }
        let line = &before[before.rfind('\n').map_or(0, |newline| newline + 1)..];
        if line.trim_start().starts_with('#') {
            return false;
        }
        let before = before.trim_end_matches(is_space);
        let before = before.strip_suffix("::").map_or(before, |rest| rest.trim_end_matches(is_space));
        let after = after.trim_start_matches(is_space);
        let after_class = before.strip_suffix("class").is_some_and(|rest| !rest.chars().next_back().is_some_and(is_identifier));
        after_class || before.ends_with("<<") || after.starts_with('.') || after.starts_with("&.") || after.starts_with("::")
    })
}

fn scan(sources: &[SourceFile], root: &Path, names: &BTreeSet<String>, targets: &Targets, sites: &mut BTreeMap<String, Vec<String>>, pending: &mut Vec<String>) {
    for source in sources.iter().filter(|source| targets.may_be_named_in(&source.text)) {
        let parsed = ruby_prism::parse(source.text.as_bytes());
        let mut finder = Finder { source, root, names, targets, namespace: Vec::new(), on_target: vec![false], sites: &mut *sites, mixed: &mut *pending };
        finder.visit(&parsed.node());
    }
}

struct Finder<'scan> {
    source: &'scan SourceFile,
    root: &'scan Path,
    names: &'scan BTreeSet<String>,
    targets: &'scan Targets,
    /// The lexical `class`/`module` names around the current node.
    namespace: Vec<String>,
    /// Whether a bare `def` here defines on a target, innermost last.
    on_target: Vec<bool>,
    sites: &'scan mut BTreeMap<String, Vec<String>>,
    /// Modules mixed into a target.
    mixed: &'scan mut Vec<String>,
}

impl Finder<'_> {
    fn here(&self) -> bool {
        self.on_target.last().copied().unwrap_or(false)
    }

    fn record(&mut self, name: &str, offset: usize) {
        let path = Path::new(&self.source.path).strip_prefix(self.root).map(|relative| relative.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| self.source.path.clone());
        self.sites.entry(name.to_string()).or_default().push(format!("{path}:{}", self.source.line_col(offset as u32).0));
    }

    /// Does `receiver` (a constant, or `X.singleton_class`) name a target?
    fn receiver_is_target(&self, receiver: &ruby_prism::Node<'_>) -> bool {
        if let Some(call) = receiver.as_call_node()
            && call.name().as_slice() == b"singleton_class"
        {
            return match call.receiver() {
                Some(inner) => self.receiver_is_target(&inner),
                None => self.here(),
            };
        }
        constant_path(receiver).is_some_and(|(path, absolute)| self.targets.matches(&path, absolute, &self.namespace))
    }

    /// Does a call or `def` with this receiver define on a target?
    fn defines_on_target(&self, receiver: Option<ruby_prism::Node<'_>>) -> bool {
        match receiver {
            None => self.here(),
            Some(receiver) if receiver.as_self_node().is_some() => self.here(),
            Some(receiver) => self.receiver_is_target(&receiver),
        }
    }

    fn with_body(&mut self, name: Option<(Vec<String>, bool)>, on_target: bool, visit: impl FnOnce(&mut Self)) {
        let outer = self.namespace.clone();
        if let Some((path, absolute)) = name {
            if absolute {
                self.namespace.clear();
            }
            self.namespace.extend(path);
        }
        self.on_target.push(on_target);
        visit(self);
        self.on_target.pop();
        self.namespace = outer;
    }
}

/// A constant read or path's segments as written, and whether it starts
/// with `::`.
fn constant_path(node: &ruby_prism::Node<'_>) -> Option<(Vec<String>, bool)> {
    if let Some(read) = node.as_constant_read_node() {
        return Some((vec![String::from_utf8_lossy(read.name().as_slice()).into_owned()], false));
    }
    let path = node.as_constant_path_node()?;
    let name = String::from_utf8_lossy(path.name()?.as_slice()).into_owned();
    match path.parent() {
        None => Some((vec![name], true)),
        Some(parent) => {
            let (mut segments, absolute) = constant_path(&parent)?;
            segments.push(name);
            Some((segments, absolute))
        }
    }
}

impl<'parse> Visit<'parse> for Finder<'_> {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'parse>) {
        let name = constant_path(&node.constant_path());
        let on_target = name.as_ref().is_some_and(|(path, absolute)| self.targets.matches(path, *absolute, &self.namespace));
        self.with_body(name, on_target, |finder| ruby_prism::visit_class_node(finder, node));
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'parse>) {
        let name = constant_path(&node.constant_path());
        let on_target = name.as_ref().is_some_and(|(path, absolute)| self.targets.matches(path, *absolute, &self.namespace));
        self.with_body(name, on_target, |finder| ruby_prism::visit_module_node(finder, node));
    }

    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'parse>) {
        let expression = node.expression();
        let on_target = if expression.as_self_node().is_some() { self.here() } else { self.receiver_is_target(&expression) };
        self.with_body(None, on_target, |finder| ruby_prism::visit_singleton_class_node(finder, node));
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'parse>) {
        let name = String::from_utf8_lossy(node.name().as_slice()).into_owned();
        if self.names.contains(&name) && self.defines_on_target(node.receiver()) {
            self.record(&name, node.location().start_offset());
        }
        // A def inside a def defines on whatever the outer method runs on.
        self.with_body(None, false, |finder| ruby_prism::visit_def_node(finder, node));
    }

    fn visit_alias_method_node(&mut self, node: &ruby_prism::AliasMethodNode<'parse>) {
        if self.here()
            && let Some(name) = literal_name(&node.new_name()).filter(|name| self.names.contains(name))
        {
            self.record(&name, node.location().start_offset());
        }
        ruby_prism::visit_alias_method_node(self, node);
    }

    fn visit_undef_node(&mut self, node: &ruby_prism::UndefNode<'parse>) {
        if self.here() {
            for name in node.names().iter().filter_map(|name| literal_name(&name)).filter(|name| self.names.contains(name)) {
                self.record(&name, node.location().start_offset());
            }
        }
        ruby_prism::visit_undef_node(self, node);
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
        let on_target = self.defines_on_target(node.receiver());
        if on_target && NAMING_CALLS.contains(&method.as_str()) {
            let named: Vec<String> = if method.starts_with("attr") { arguments.iter().filter_map(literal_name).collect() } else { arguments.first().and_then(literal_name).into_iter().collect() };
            for name in named.iter().filter(|name| self.names.contains(*name)) {
                self.record(name, node.location().start_offset());
            }
        }
        if on_target && MIXING_CALLS.contains(&method.as_str()) {
            for argument in &arguments {
                if let Some((path, _)) = constant_path(argument)
                    && let Some(last) = path.last()
                {
                    self.mixed.push(last.clone());
                }
            }
        }
        // A block passed to a call on a target (`Integer.class_eval do`) runs
        // with the target as self; any other block keeps the scope around it.
        let block_on_target = node.receiver().is_some_and(|receiver| receiver.as_self_node().is_none() && self.receiver_is_target(&receiver));
        if block_on_target {
            if let Some(receiver) = node.receiver() {
                self.visit(&receiver);
            }
            if let Some(arguments) = node.arguments() {
                self.visit_arguments_node(&arguments);
            }
            if let Some(block) = node.block() {
                self.with_body(None, true, |finder| finder.visit(&block));
            }
        } else {
            ruby_prism::visit_call_node(self, node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocking(files: &[(&str, &str)], names: &[&str]) -> Vec<String> {
        let sources: Vec<SourceFile> = files.iter().map(|(path, text)| SourceFile { path: format!("/app/{path}"), text: text.to_string() }).collect();
        let names: BTreeSet<String> = names.iter().map(|name| name.to_string()).collect();
        let mut sites: Vec<String> = blocking_definitions(&sources, Path::new("/app"), &names).into_values().flatten().collect();
        sites.sort();
        sites
    }

    #[test]
    fn definitions_on_ordinary_classes_or_at_the_top_level_do_not_count() {
        let files = [
            ("plan.rb", "class Plan\n  attr_reader :days\n  def ago; end\n  def self.days; end\n  class << self\n    def since; end\n  end\nend\n"),
            ("top.rb", "def days; end\n"),
            ("other_duration.rb", "module Billing\n  class Duration\n    def ago; end\n  end\nend\n"),
            ("other_integer.rb", "class Fields::Integer\n  def days; end\nend\n"),
            ("helpers.rb", "module Helpers\n  def days; end\nend\nclass Plan\n  include Helpers\nend\n"),
        ];
        assert_eq!(blocking(&files, &["days", "ago", "since"]), Vec::<String>::new());
    }

    #[test]
    fn definitions_that_can_land_on_a_target_count() {
        let files = [
            ("integer.rb", "class Integer\n  def days = 1\nend\n"),
            ("numeric.rb", "class ::Numeric\n  alias days hours\nend\n"),
            ("duration.rb", "module ActiveSupport\n  class Duration\n    def ago = nil\n    class << self\n      def days(value) = nil\n    end\n  end\nend\n"),
            ("path.rb", "class ActiveSupport::Duration\n  private def sum(*) = nil\nend\n"),
            ("receiver.rb", "Integer.define_method(:days) { 1 }\nActiveSupport::Duration.singleton_class.send(:alias_method, :days, :hours)\n"),
            ("eval.rb", "ActiveSupport::Duration.class_eval do\n  def since = nil\nend\n"),
            ("singleton.rb", "def Integer.days = nil\nclass << ActiveSupport::Duration\n  def days = nil\nend\n"),
            ("unrelated.rb", "Integer.define_method(:weeks) { 1 }\n"),
            ("time.rb", "class Time\n  def self.current = now\nend\nclass ActiveSupport::TimeWithZone\n  alias_method :since, :+\nend\nclass DateTime\n  def since = nil\nend\n"),
            ("spaced.rb", "class  Numeric\n  def days = hours\nend\nclass\tInteger\n  def days = 2\nend\n"),
            ("scoped.rb", "Numeric::define_method(:days) { hours }\nInteger\n  .send(:define_method, :ago) { }\n"),
        ];
        assert_eq!(
            blocking(&files, &["days", "ago", "since", "sum", "current"]),
            [
                "duration.rb:3", "duration.rb:5", "eval.rb:2", "integer.rb:2", "numeric.rb:2", "path.rb:2", "receiver.rb:1", "receiver.rb:2", "scoped.rb:1", "scoped.rb:2",
                "singleton.rb:1", "singleton.rb:3", "spaced.rb:2", "spaced.rb:5", "time.rb:2", "time.rb:5",
            ]
        );
    }

    #[test]
    fn the_prefilter_reads_only_files_that_could_name_a_target() {
        for (text, named) in [
            ("class Integer; end", true),
            ("class\\\n  ::Numeric; end", true),
            ("class << Integer; end", true),
            ("Integer .prepend(M)", true),
            ("Numeric&.class_eval { }", true),
            ("::Integer.class_eval { }", true),
            ("class << ::Numeric; end", true),
            ("x = Fields::Integer", false),
            ("@ttl = 3600 #: ::Integer", false),
            ("  # Integer.parse is not this", false),
            ("#: (Integer) -> Integer", false),
            ("T.let(1, Integer)", false),
            ("BigInteger.new", false),
            ("subclass Integer", false),
            ("class Time; end", true),
            ("Time.now", true),
            ("ActiveSupport::TimeWithZone.prepend(M)", true),
            ("class ActiveSupport::TimeZone; end", true),
            ("DateTime.now", false),
            ("#: (Time) -> Time", false),
            ("Timeout.timeout(1) { }", false),
        ] {
            assert_eq!(Targets::Classes.may_be_named_in(text), named, "{text}");
        }
    }

    #[test]
    fn modules_mixed_into_a_target_count_through_other_modules() {
        let override_module = ("override.rb", "module DurationOverride\n  def ago = nil\nend\n");
        for (mixer, counts) in [
            ("ActiveSupport::Duration.prepend(DurationOverride)\n", true),
            ("class Integer\n  include DurationOverride\nend\n", true),
            ("module Outer\n  include DurationOverride\nend\nmodule ActiveSupport\n  class Duration\n    prepend Outer\n  end\nend\n", true),
            ("class Plan\n  include DurationOverride\nend\n", false),
        ] {
            let found = blocking(&[override_module, ("mixer.rb", mixer)], &["ago"]);
            assert_eq!(found.is_empty(), !counts, "{mixer}: {found:?}");
        }
    }
}
