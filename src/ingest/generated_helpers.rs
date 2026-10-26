//! Helper objects an app builds at load time from a table it keeps in
//! YAML — core's `WebUrlHelpers`:
//!
//! ```ruby
//! WebUrlHelpers = WebUrlHelpersFactory.create(
//!   YAML.load_file(File.join(Rails.root, "config/web_paths.yml"))["paths"],
//! )
//!
//! # the factory:
//! Class.new do
//!   paths.each do |config_name, path|
//!     define_method "#{config_name}_path" do |params: {}, **args| … end
//!     define_method("#{config_name}_url") do |host: nil, **args| … end
//!   end
//! end.new
//! ```
//!
//! Nothing here is a `def`, so a reader of the source sees no method at
//! all, and every `WebUrlHelpers.admin_settings_path` is a call to
//! nothing. The methods exist because two facts meet: the FACTORY says
//! what a table entry defines (one method per `define_method` whose name
//! interpolates the entry's key — the suffixes), and the constant's
//! initializer says WHICH table (a YAML file and a key in it). Reading
//! both gives the exact method set, with no guess about the names.
//!
//! The result is a registry fact for the analyzer
//! ([`crate::App::generated_helper_methods`]); it emits nothing, because
//! the constant's own initializer still runs at load and builds the
//! object.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ruby_prism::Node;

use crate::App;
use crate::dialect::MethodReceiver;
use crate::expr::{Expr, ExprNode, InterpPart};
use crate::ident::{ClassId, Symbol};
use crate::vfs::Vfs;

use super::util::{constant_id_str, string_value};

use crate::ingest::prism_api::PrismLocation;
/// Every `NAME = Factory.method(YAML.load_file(<path>)["key"])` at the
/// top level of a file under `roots`, resolved to the methods the
/// factory defines per table entry. `app.library_classes` must already
/// hold the factory (it is read as IR).
pub(super) fn ingest_generated_helpers<V: Vfs + ?Sized>(
    app: &App,
    vfs: &V,
    dir: &Path,
    files: &[std::path::PathBuf],
) -> BTreeMap<ClassId, BTreeSet<Symbol>> {
    let mut out: BTreeMap<ClassId, BTreeSet<Symbol>> = BTreeMap::new();
    for file in files {
        let Ok(bytes) = vfs.read(file) else { continue };
        if !bytes.windows(b"YAML.load_file".len()).any(|w| w == b"YAML.load_file") {
            continue;
        }
        let result = super::prism::parse(&bytes, &file.display().to_string());
        let src = String::from_utf8_lossy(&bytes).into_owned();
        let root = result.node();
        let Some(program) = root.as_program_node() else { continue };
        for stmt in program.statements().body().iter() {
            let Some(cw) = stmt.as_constant_write_node() else { continue };
            let Some(call) = cw.value().as_call_node() else { continue };
            let Some(factory_recv) = call.receiver() else { continue };
            let Some(factory) = const_text(&factory_recv, &src) else { continue };
            let factory_method = constant_id_str(&call.name());
            let Some(arg) = call.arguments().and_then(|a| a.arguments().iter().next()) else {
                continue;
            };
            let Some((yaml_path, key)) = yaml_table_ref(&arg, &src) else { continue };
            let Some(suffixes) = factory_suffixes(app, &factory, factory_method) else {
                continue;
            };
            let Ok(text) = vfs.read_to_string(&dir.join(&yaml_path)) else { continue };
            let Ok(doc) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) else {
                continue;
            };
            let Some(table) = doc.get(key.as_str()).and_then(|t| t.as_mapping()) else {
                continue;
            };
            let names = out
                .entry(ClassId(Symbol::from(cw.name().as_slice().iter().map(|b| *b as char).collect::<String>())))
                .or_default();
            for entry in table.keys().filter_map(|k| k.as_str()) {
                for suffix in &suffixes {
                    names.insert(Symbol::from(format!("{entry}{suffix}")));
                }
            }
        }
    }
    out.retain(|_, names| !names.is_empty());
    out
}

fn const_text(node: &Node<'_>, src: &str) -> Option<String> {
    if node.as_constant_read_node().is_none() && node.as_constant_path_node().is_none() {
        return None;
    }
    let loc = node.location();
    Some(src[loc.start_offset()..loc.end_offset()].trim_start_matches("::").to_string())
}

/// `YAML.load_file(File.join(Rails.root, "rel/path.yml"))["key"]` (or
/// `Rails.root.join("rel/path.yml")`) → (`"rel/path.yml"`, `"key"`).
fn yaml_table_ref(node: &Node<'_>, src: &str) -> Option<(String, String)> {
    let index = node.as_call_node()?;
    if constant_id_str(&index.name()) != "[]" {
        return None;
    }
    let key = string_value(&index.arguments()?.arguments().iter().next()?)?;
    let load = index.receiver()?.as_call_node()?;
    if constant_id_str(&load.name()) != "load_file" {
        return None;
    }
    if const_text(&load.receiver()?, src).as_deref() != Some("YAML") {
        return None;
    }
    let path_expr = load.arguments()?.arguments().iter().next()?;
    if let Some(s) = string_value(&path_expr) {
        return Some((s, key));
    }
    let join = path_expr.as_call_node()?;
    if constant_id_str(&join.name()) != "join" {
        return None;
    }
    // The path is the last string of the join: `File.join(Rails.root, "x")`
    // and `Rails.root.join("x")` both end in it.
    let rel = join
        .arguments()?
        .arguments()
        .iter()
        .filter_map(|a| string_value(&a))
        .last()?;
    Some((rel, key))
}

/// The name suffixes the factory's `define_method`s add to a table
/// key: `["_path", "_url"]` for `define_method "#{k}_path"` and
/// `define_method("#{k}_url")`. None when the factory is not an ingested
/// class method or defines nothing by interpolated name.
fn factory_suffixes(app: &App, factory: &str, method: &str) -> Option<Vec<String>> {
    let suffix = format!("::{factory}");
    let class = app.library_classes.iter().find(|c| {
        let n = c.name.0.as_str();
        n == factory || n.ends_with(&suffix)
    })?;
    let def = class
        .methods
        .iter()
        .find(|m| m.receiver == MethodReceiver::Class && m.name.as_str() == method)?;
    let mut out = Vec::new();
    collect_define_method_suffixes(&def.body, &mut out);
    (!out.is_empty()).then_some(out)
}

fn collect_define_method_suffixes(expr: &Expr, out: &mut Vec<String>) {
    if let ExprNode::Send { recv: None, method, args, .. } = &*expr.node {
        if method.as_str() == "define_method" {
            if let Some(Expr { node, .. }) = args.first() {
                if let ExprNode::StringInterp { parts } = &**node {
                    // `"#{key}_path"`: one embedded value, then literal text.
                    if let [InterpPart::Expr { .. }, InterpPart::Text { value }] = parts.as_slice() {
                        if !out.contains(value) {
                            out.push(value.clone());
                        }
                    }
                }
            }
        }
    }
    expr.node.for_each_child(&mut |c| collect_define_method_suffixes(c, out));
}
