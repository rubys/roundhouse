//! Rails `table_name_prefix` scanners.
//!
//! Namespace modules, `isolate_namespace`, and a class's own
//! `self.table_name_prefix =` assignment. `self.table_name` bindings
//! stay in the model walk.

use super::super::util::{
    class_name_path, constant_id_str, constant_path_of, flatten_statements, string_value,
    symbol_value,
};

/// Namespace module → the `table_name_prefix` it declares. Rails walks a
/// model's `module_parents` looking for this and prepends what it finds,
/// which is the ONLY thing that puts a namespace into a table name (the
/// name itself is demodulized — see `naming::rails_table_name`).
///
/// campfire's `app/models/push.rb` is four lines and exists solely for
/// this: without it `Push::Subscription` reads `subscriptions`, and its
/// table is `push_subscriptions`.
pub type TablePrefixes = std::collections::HashMap<String, String>;

/// Scan one file for `module <Ns>; def self.table_name_prefix; "<p>"; end`,
/// or the same def inside the module's `class << self`.
/// Deliberately narrow: only a module-level singleton def whose body is a
/// single string literal. A computed prefix would have to run to be known,
/// and nothing in the corpus writes one.
pub fn ingest_table_name_prefixes(source: &[u8], file: &str) -> TablePrefixes {
    fn literal_prefix(def: &ruby_prism::DefNode<'_>) -> Option<String> {
        if constant_id_str(&def.name()) != "table_name_prefix" {
            return None;
        }
        let stmts = flatten_statements(def.body()?);
        if stmts.len() != 1 {
            return None;
        }
        string_value(&stmts[0])
    }
    let result = super::super::prism::parse(source, file);
    let root = result.node();
    let mut out = TablePrefixes::new();
    for (scope, module) in super::super::util::find_all_modules_with_scope(&root) {
        let Some(name_path) = super::super::util::module_name_path(&module) else {
            continue;
        };
        let mut full = scope;
        full.extend(name_path);
        let Some(body) = module.body() else { continue };
        let stmts = flatten_statements(body);
        // `extend self` makes every instance method a singleton one too, so a
        // nested module can override an outer `ledger_` with a plain
        // `def table_name_prefix = ""`.
        let extends_self = stmts.iter().any(|s| {
            s.as_call_node().is_some_and(|c| {
                c.receiver().is_none()
                    && constant_id_str(&c.name()) == "extend"
                    && c.arguments().is_some_and(|a| {
                        let args: Vec<_> = a.arguments().iter().collect();
                        args.len() == 1 && args[0].as_self_node().is_some()
                    })
            })
        });
        for stmt in stmts {
            if let Some(def) = stmt.as_def_node() {
                let singleton = match def.receiver() {
                    Some(r) => r.as_self_node().is_some(),
                    None => extends_self,
                };
                if !singleton {
                    continue;
                }
                if let Some(prefix) = literal_prefix(&def) {
                    out.insert(full.join("::"), prefix);
                }
            } else if let Some(sclass) = stmt.as_singleton_class_node() {
                if sclass.expression().as_self_node().is_none() {
                    continue;
                }
                let Some(sbody) = sclass.body() else { continue };
                for inner in flatten_statements(sbody) {
                    let Some(def) = inner.as_def_node() else {
                        continue;
                    };
                    if def.receiver().is_some() {
                        continue;
                    }
                    if let Some(prefix) = literal_prefix(&def) {
                        out.insert(full.join("::"), prefix);
                    }
                }
            }
        }
    }
    out
}

/// `isolate_namespace Billing` in an engine body: Rails defines
/// `Billing.table_name_prefix` as the engine name and `_`
/// (`billing_`), unless the module already declares one. A
/// `Billing::Rate` with no `table_name` read `rates` without the
/// prefix, a table no schema has, and every column read on it was
/// `no known method`. The written module is the enclosing one it names
/// (`module Billing; class Engine`), else the name as written.
pub fn ingest_isolated_namespace_prefixes(source: &[u8], file: &str) -> TablePrefixes {
    let result = super::super::prism::parse(source, file);
    let root = result.node();
    let mut out = TablePrefixes::default();
    for (scope, class) in super::super::util::find_all_classes_with_scope(&root) {
        let Some(body) = class.body() else { continue };
        let mut nesting = scope;
        nesting.extend(class_name_path(&class).unwrap_or_default());
        for stmt in flatten_statements(body) {
            let Some(call) = stmt.as_call_node() else {
                continue;
            };
            if call.receiver().is_some() || constant_id_str(&call.name()) != "isolate_namespace" {
                continue;
            }
            let Some(arg) = call
                .arguments()
                .and_then(|args| args.arguments().iter().next())
            else {
                continue;
            };
            let Some(written) = constant_path_of(&arg) else {
                continue;
            };
            let rooted = arg
                .as_constant_path_node()
                .is_some_and(|p| super::super::util::constant_path_is_rooted(&p));
            let enclosing = (!rooted)
                .then(|| {
                    nesting[..nesting.len().saturating_sub(1)]
                        .iter()
                        .rposition(|s| *s == written[0])
                })
                .flatten();
            let full = match enclosing {
                Some(at) => nesting[..at]
                    .iter()
                    .chain(&written)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("::"),
                None => written.join("::"),
            };
            let prefix = format!("{}_", crate::naming::underscore(&full).replace('/', "_"));
            out.insert(full, prefix);
        }
    }
    out
}

/// Each class one file declares with its own `self.table_name_prefix =
/// "…"`, by full name. Rails keeps the prefix in a class attribute, so a
/// model inherits it from its abstract base: an abstract
/// `PrefixedRecord` that sets `legacy_` gives its
/// models (`Line`, …) that name no table `legacy_lines`.
pub fn ingest_class_table_prefixes(source: &[u8], file: &str) -> TablePrefixes {
    let result = super::super::prism::parse(source, file);
    let root = result.node();
    let mut out = TablePrefixes::default();
    for (mut scope, class) in super::super::util::find_all_classes_with_scope(&root) {
        let Some(prefix) = class.body().and_then(explicit_table_prefix) else {
            continue;
        };
        let Some(path) = class_name_path(&class) else {
            continue;
        };
        scope.extend(path);
        out.insert(scope.join("::"), prefix);
    }
    out
}

/// The prefix each abstract base passes down: its own, else its parent's.
/// `pairs` are the pre-pass's `(abstract class, parent)` pairs; a bare
/// parent is found the way Ruby finds it, the child's enclosing scopes first.
pub fn inherited_table_prefixes(own: &TablePrefixes, pairs: &[(String, String)]) -> TablePrefixes {
    let parents: std::collections::HashMap<&str, &str> = pairs
        .iter()
        .map(|(c, p)| (c.as_str(), p.as_str()))
        .collect();
    let known = |name: &str| own.contains_key(name) || parents.contains_key(name);
    let resolve = |child: &str, parent: &str| -> String {
        let mut segments: Vec<&str> = child.split("::").collect();
        segments.pop();
        while !segments.is_empty() {
            let candidate = format!("{}::{parent}", segments.join("::"));
            if known(&candidate) {
                return candidate;
            }
            segments.pop();
        }
        parent.to_string()
    };
    let mut out = TablePrefixes::default();
    for (child, _) in pairs {
        let mut at = child.clone();
        // A cycle guard: the walk follows `parent` links, so a cyclic
        // hierarchy must end somewhere.
        for _ in 0..16 {
            if let Some(prefix) = own.get(&at) {
                out.insert(child.clone(), prefix.clone());
                break;
            }
            let Some(parent) = parents.get(at.as_str()) else {
                break;
            };
            at = resolve(&at, parent);
        }
    }
    out
}

/// `self.table_name_prefix = "three_d_secure_"` in a model body: the
/// class's own prefix, which wins over any namespace module's.
pub(super) fn explicit_table_prefix(body: ruby_prism::Node<'_>) -> Option<String> {
    explicit_class_setting(body, "table_name_prefix=")
}

fn explicit_class_setting(body: ruby_prism::Node<'_>, setter: &str) -> Option<String> {
    for stmt in flatten_statements(body) {
        let Some(call) = stmt.as_call_node() else {
            continue;
        };
        if constant_id_str(&call.name()) != setter {
            continue;
        }
        if call.receiver().and_then(|r| r.as_self_node()).is_none() {
            continue;
        }
        let args = call.arguments()?;
        let first = args.arguments().iter().next()?;
        if let Some(name) = string_value(&first).or_else(|| symbol_value(&first)) {
            return Some(name);
        }
    }
    None
}
