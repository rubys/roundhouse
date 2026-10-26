//! Oj under `Oj.mimic_JSON`, lowered into the `JSON` it stands in for.
//!
//! An app that calls `Oj.mimic_JSON` in an initializer (with
//! `Oj.optimize_rails`, as a Rails app does) gets Oj's compat mode:
//!
//! - `Oj.generate(obj)` is `JSON.generate(obj)` (`<>&` raw, `Infinity`
//!   and `NaN` a `JSON::GeneratorError`);
//! - `Oj.load(json)` is `JSON.parse(json)`, an IO's or StringIO's content
//!   read first, `symbol_keys:` (or `symbolize_names:`) symbolizing; nil,
//!   true/false, an Integer and a Float are answered as given (so
//!   `Infinity` stays a Float). Anything else is parsed from its string
//!   form: `[1]` is `[1]`, `[nil]`, a Hash, a Symbol, a Rational and an
//!   Array of hashes raise `JSON::ParserError`, and a BigDecimal is the
//!   Float its string parses as;
//! - a parse failure raises `JSON::ParserError`, which `Oj::ParseError`
//!   does not catch.
//!
//! The mode is read from the initializers' top-level statements: a
//! `mimic_JSON` or `optimize_rails` call anywhere else (a branch, a method,
//! a block) may not run, and refuses; so does `optimize_rails` alone.
//! Each call is read. One this does not model is a refusal of its own,
//! through `survey::continue_or_fail`: a method the stand-in does not
//! define (`dump`, `safe_load`, the strict parser, and the rest), or an
//! option it would ignore (anything but `symbol_keys:`/`symbolize_names:`
//! valued true, false or nil on `load`). Strict ingest fails on it; survey
//! mode ledgers it and lowers the calls that are modeled. Without `mimic_JSON`, Oj
//! is in its `:object` mode (`":sym"` keys, `"^o"` objects), not modeled
//! either: `Oj` stays unknown and its uses stay errors.
//!
//! `Oj.dump` is refused: under `mimic_JSON` it is compat (`<>&` raw,
//! `Infinity`/`NaN` written), with `mode: :rails` Rails' encoder (`<>&`
//! escaped, `null`), and the emitted tree's `to_json` is neither.
//!
//! Checked against oj 3.17.3 with ActiveSupport.
//!
//! Ruby, JRuby and Spinel emit this class. `lower::oj` refuses the same
//! calls on every other transpile target, so they are not written against
//! a class those targets never emit.

use crate::App;
use crate::dialect::{LibraryClass, LibraryClassOrigin};

use super::IngestError;
use super::survey;

use crate::ingest::prism_api::PrismLocation;
pub(super) fn lower_oj(
    app: &mut App,
    sources: &[crate::span::SourceFile],
    initializers: &[String],
) -> super::IngestResult<()> {
    if !sources.iter().any(|s| s.text.contains("Oj")) {
        return Ok(());
    }
    // The mode an initializer sets, read from its calls: only one made at
    // the top level, where it runs whatever else does, is known to run.
    let mut mimic = false;
    let mut optimize = false;
    let mut refusals = Vec::new();
    for text in initializers.iter().filter(|t| t.contains("Oj")) {
        let calls = mode_calls(text);
        for (method, top) in calls {
            match (method.as_str(), top) {
                ("mimic_JSON", true) => mimic = true,
                ("optimize_rails", true) => optimize = true,
                (m, _) => refusals.push(IngestError::Unsupported {
                    file: "config/initializers".into(),
                    message: format!("Oj not lowered: `Oj.{m}` not at an initializer's top level"),
                }),
            }
        }
    }
    if !mimic {
        // Without mimic_JSON nothing here models what optimize_rails does.
        if optimize {
            survey::continue_or_fail(IngestError::Unsupported {
                file: "config/initializers".into(),
                message: "Oj not lowered: `Oj.optimize_rails` without `Oj.mimic_JSON`".into(),
            })?;
        }
        for err in refusals {
            survey::continue_or_fail(err)?;
        }
        return Ok(());
    }
    for source in sources.iter().filter(|s| s.text.contains("Oj")) {
        refusals.extend(unmodeled_calls(&source.text).into_iter().map(|reason| IngestError::Unsupported {
            file: source.path.clone(),
            message: format!("Oj not lowered: {reason}"),
        }));
    }
    // Each call it does not model is a refusal of its own: strict ingest
    // fails on it, survey mode ledgers it and lowers the rest.
    for err in refusals {
        survey::continue_or_fail(err)?;
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
                    // `safe_load` is the strict parser: nil, a number and a
                    // bad document fail there as `load` does not.
                    "load" => &["symbol_keys", "symbolize_names"],
                    "generate" => &[],
                    "mimic_JSON" | "optimize_rails" => &[],
                    // `mimic_JSON` makes it compat (`<>&` raw, `Infinity` and
                    // `NaN` as written); `mode: :rails` is Rails' encoder.
                    // The emitted tree's `to_json` is neither.
                    "dump" => {
                        self.found.push("`Oj.dump`: neither its compat nor its rails encoding is modeled".into());
                        ruby_prism::visit_call_node(self, node);
                        return;
                    }
                    other => {
                        self.found.push(format!("`Oj.{other}`"));
                        ruby_prism::visit_call_node(self, node);
                        return;
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
                                // oj raises ArgumentError for a value other
                                // than true, false or nil.
                                let ok = allowed.contains(&key) && matches!(value.as_str(), "true" | "false" | "nil");
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
    let parsed = super::prism::parse_silent(text.as_bytes());
    let mut finder = Finder { found: Vec::new() };
    ruby_prism::Visit::visit(&mut finder, &parsed.node());
    finder.found
}

/// `Oj.mimic_JSON` / `Oj.optimize_rails` calls in an initializer, and
/// whether each is a top-level statement.
fn mode_calls(text: &str) -> Vec<(String, bool)> {
    struct Finder {
        // Whole ranges: a modifier (`Oj.mimic_JSON if …`) starts where its call does.
        top: Vec<(usize, usize)>,
        found: Vec<(String, bool)>,
    }
    impl<'pr> ruby_prism::Visit<'pr> for Finder {
        fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
            let on_oj = node.receiver().is_some_and(|r| {
                matches!(String::from_utf8_lossy(r.location().as_slice()).as_ref(), "Oj" | "::Oj")
            });
            let method = String::from_utf8_lossy(node.name().as_slice()).to_string();
            if on_oj && matches!(method.as_str(), "mimic_JSON" | "optimize_rails") {
                let at = (node.location().start_offset(), node.location().end_offset());
                // `Oj.mimic_JSON()` is the same call as `Oj.mimic_JSON`: Prism
                // gives the parentheses an empty arguments node.
                let bare = node.block().is_none()
                    && node.arguments().is_none_or(|args| args.arguments().iter().next().is_none());
                let top = self.top.contains(&at) && bare;
                self.found.push((method, top));
            }
            ruby_prism::visit_call_node(self, node);
        }
    }
    let parsed = super::prism::parse_silent(text.as_bytes());
    let root = parsed.node();
    let top: Vec<(usize, usize)> = root
        .as_program_node()
        .map(|p| {
            p.statements()
                .body()
                .iter()
                .map(|s| (s.location().start_offset(), s.location().end_offset()))
                .collect()
        })
        .unwrap_or_default();
    let mut finder = Finder { top, found: Vec::new() };
    ruby_prism::Visit::visit(&mut finder, &root);
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
    return json if json.nil? || json == true || json == false || json.is_a?(Integer) || json.is_a?(Float)
    source = json.is_a?(IO) || json.class.name == "StringIO" ? json.read : json.to_s
    # A read that is not a String, and anything whose string is not JSON,
    # fails as JSON's own JSON::ParserError.
    source = "" unless source.is_a?(String)
    # The one-argument parse every target maps; the keys symbolized after.
    value = JSON.parse(source)
    options[:symbol_keys] == true || options[:symbolize_names] == true ? symbolized(value) : value
  end

  def self.symbolized(value)
    if value.is_a?(Hash)
      out = {}
      value.keys.each { |key| out[key.to_sym] = symbolized(value[key]) }
      return out
    end
    return value.map { |element| symbolized(element) } if value.is_a?(Array)
    value
  end

  def self.generate(obj)
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
