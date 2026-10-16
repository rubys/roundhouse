//! Oj under `Oj.mimic_JSON`, lowered into the `JSON` it stands in for.
//!
//! An app that calls `Oj.mimic_JSON` in an initializer (with
//! `Oj.optimize_rails`, as a Rails app does) gets Oj's compat mode:
//!
//! - `Oj.dump(obj)` encodes as `obj.to_json` does (`mode: :compat` and
//!   `:rails` the same);
//! - `Oj.generate(obj)` is `JSON.generate(obj)`;
//! - `Oj.load(json)` is `JSON.parse(json)`, `symbol_keys:` (or
//!   `symbolize_names:`) symbolizing; nil and a non-String are answered
//!   as given;
//! - a parse failure raises `JSON::ParserError`, which `Oj::ParseError`
//!   does not catch.
//!
//! Each call is read: any other Oj method or option refuses the whole
//! lowering, since what it does is not modeled. Without `mimic_JSON`, Oj
//! is in its `:object` mode (`":sym"` keys, `"^o"` objects), not modeled
//! either: `Oj` stays unknown and its uses stay errors.
//!
//! Checked against oj 3.17 with ActiveSupport.
//! ponytail: `Oj.dump` keeps `<>&` and writes `Infinity` where Rails'
//! `to_json` escapes them and writes `null`; `to_json` is what the
//! emitted tree has.

use crate::App;
use crate::dialect::{LibraryClass, LibraryClassOrigin};

use super::IngestError;
use super::survey;

pub(super) fn lower_oj(
    app: &mut App,
    sources: &[crate::span::SourceFile],
    initializers: &[String],
) -> super::IngestResult<()> {
    if !sources.iter().any(|s| s.text.contains("Oj")) {
        return Ok(());
    }
    let mimic = initializers
        .iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .any(|line| line == "Oj.mimic_JSON" || line == "::Oj.mimic_JSON" || line.starts_with("Oj.mimic_JSON("));
    if !mimic {
        return Ok(());
    }
    let mut refusals = Vec::new();
    for source in sources.iter().filter(|s| s.text.contains("Oj")) {
        refusals.extend(unmodeled_calls(&source.text).into_iter().map(|reason| IngestError::Unsupported {
            file: source.path.clone(),
            message: format!("Oj not lowered: {reason}"),
        }));
    }
    if !refusals.is_empty() {
        for err in refusals {
            survey::continue_or_fail(err)?;
        }
        return Ok(());
    }
    if app.library_classes.iter().any(|lc| lc.name.0.as_str() == "Oj") {
        return Ok(());
    }
    app.library_classes.extend(stand_ins());
    Ok(())
}

/// The `Oj.*` calls in `text` this does not model, described.
fn unmodeled_calls(text: &str) -> Vec<String> {
    struct Finder {
        found: Vec<String>,
    }
    fn text_of(node: &ruby_prism::Node<'_>) -> String {
        String::from_utf8_lossy(node.location().as_slice()).to_string()
    }
    impl<'pr> ruby_prism::Visit<'pr> for Finder {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let on_oj = node.receiver().is_some_and(|r| matches!(text_of(&r).as_str(), "Oj" | "::Oj"));
            if on_oj {
                let method = String::from_utf8_lossy(node.name().as_slice()).to_string();
                let args: Vec<ruby_prism::Node<'_>> =
                    node.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                let allowed: &[&str] = match method.as_str() {
                    "load" | "safe_load" => &["symbol_keys", "symbolize_names"],
                    "dump" | "generate" => &["mode"],
                    "mimic_JSON" | "optimize_rails" => &[],
                    other => {
                        self.found.push(format!("`Oj.{other}`"));
                        &[]
                    }
                };
                // The object is the first argument, a braceless Hash
                // included; a second is the options.
                if let Some(options) = args.get(1) {
                    match options.as_keyword_hash_node() {
                        Some(hash) => {
                            for element in hash.elements().iter() {
                                let Some(pair) = element.as_assoc_node() else {
                                    self.found.push(format!("`Oj.{method}` options `{}`", text_of(&element)));
                                    continue;
                                };
                                let key = text_of(&pair.key());
                                let key = key.trim_end_matches(':').trim_start_matches(':');
                                let value = text_of(&pair.value());
                                let ok = allowed.contains(&key)
                                    && (key != "mode" || matches!(value.as_str(), ":compat" | ":rails"));
                                if !ok {
                                    self.found.push(format!("`Oj.{method}` option `{key}: {value}`"));
                                }
                            }
                        }
                        None => self.found.push(format!("`Oj.{method}` options `{}`", text_of(options))),
                    }
                }
                if args.len() > 2 || node.block().is_some() {
                    self.found.push(format!("`Oj.{method}` with these arguments"));
                }
            } else if node.receiver().is_some_and(|r| text_of(&r).starts_with("Oj::") || text_of(&r).starts_with("::Oj::")) {
                self.found.push(format!("`{}`", text_of(&node.as_node()).lines().next().unwrap_or_default()));
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let parsed = ruby_prism::parse(text.as_bytes());
    let mut finder = Finder { found: Vec::new() };
    ruby_prism::Visit::visit(&mut finder, &parsed.node());
    finder.found
}

/// What the lowered calls reach once Oj is not there.
fn stand_ins() -> Vec<LibraryClass> {
    const SOURCE: &str = r#"module Oj
  class Error < StandardError
  end

  class ParseError < Error
  end

  def self.load(json, options = {})
    return json unless json.is_a?(String)
    JSON.parse(json, symbolize_names: options[:symbol_keys] == true || options[:symbolize_names] == true)
  end

  def self.safe_load(json)
    load(json)
  end

  def self.dump(obj, options = {})
    obj.to_json
  end

  def self.generate(obj, options = {})
    JSON.generate(obj)
  end

  def self.mimic_JSON
    nil
  end

  def self.optimize_rails
    nil
  end
end
"#;
    let mut classes =
        crate::ingest::ingest_library_classes(SOURCE.as_bytes(), "<oj>").expect("the Oj stand-ins parse");
    for c in &mut classes {
        c.origin = Some(LibraryClassOrigin::Oj);
    }
    classes
}
