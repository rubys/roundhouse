//! An app's reopen of a class the runtime ships.
//!
//! Rails apps reopen framework classes and change them: a `lib/core_ext/`
//! file an initializer requires, a patch in `config/initializers/`, a
//! `to_prepare` block. Ruby merges a reopen into the class that is
//! already there, the later definition of a method replaces the earlier
//! one, and a header that names a superclass must name the one the class
//! already has.
//!
//! The ruby-family trees ship the framework as runtime files, and an app
//! class of the same name lands under `app/models/` as a reopen of the
//! runtime's, loaded after it. Plain Ruby would give the app's methods
//! the last word on its own, but the tree is not plain Ruby in two
//! places:
//!
//!   - A method both define is declared twice in the tree's RBS, once in
//!     the runtime's sidecar and once in the app's, and Spinel's `--rbs`
//!     refuses two signatures for one method.
//!   - A header naming Rails' parent (`class ActiveStorage::PurgeJob <
//!     ActiveJob::Base`) names one the runtime's class does not have, and
//!     CRuby stops at load on the superclass mismatch, or on a parent the
//!     runtime does not ship at all.
//!
//! So this pass does to the finished file set what loading the reopen
//! does to the class. Where an app file and a runtime file define a
//! method on the same class (or module) with the same receiver, the
//! runtime's `def` and its RBS declaration go: the app's is the one Ruby
//! keeps, and `super` from it never reached the runtime's either, since a
//! redefinition replaces rather than stacks. The app's header drops its
//! superclass, because a reopen cannot change a class's parent: the
//! runtime's is the one the class has, and Rails' is the one the header
//! asserts.
//!
//! The classes and methods are read from the files themselves, the
//! runtime the tree ships and the app files beside it, so there is no
//! list to keep. Only `app/models/` is the app side, the directory the
//! support classes (`lib/`, `extras/`, `app/lib/`) are emitted into, so a
//! generated file elsewhere (route helpers, views) never counts as a
//! reopen.
//!
//! A runtime class body can also name a method after its `def`, and that
//! line runs at load, before the app's reopen does. A visibility call
//! (`private :x`, `protected`, `public`, `private_class_method`,
//! `public_class_method`) loses the names whose `def` went, and goes when
//! none is left; otherwise it would raise NameError. Dropping it is
//! Ruby's answer too: a method a plain reopen redefines is public unless
//! the reopen says otherwise. A runtime method an `alias` or
//! `module_function` names keeps its `def` instead, since either copies
//! the runtime's body somewhere the reopen's `def` does not replace.

use crate::det::{HashMap, HashSet};
use crate::ingest::prism_api::PrismLocation;
use ruby_prism::Node;

/// `(qualified class name, class-side?, method name)`.
type MethodKey = (String, bool, String);

/// One class or module body in a file.
struct Body {
    name: String,
    is_module: bool,
    /// The ` < Parent` of a class header, as a byte range.
    superclass: Option<(usize, usize)>,
    defs: Vec<Def>,
    /// Method names an `alias`, `alias_method` or `module_function` in
    /// the body names.
    aliased: HashSet<String>,
    visibility: Vec<Visibility>,
}

struct Def {
    class_side: bool,
    name: String,
    /// The definition's whole lines, from its first byte to past its
    /// last newline.
    span: (usize, usize),
}

/// A class-body call that names methods by symbol: `private :a, :b`.
struct Visibility {
    /// Whether the names are class-side (`private_class_method`, or any
    /// of them inside `class << self`).
    class_side: bool,
    /// The statement's whole lines, as `Def::span`.
    lines: (usize, usize),
    /// The call itself, which a rewrite replaces.
    call: (usize, usize),
    method: String,
    parens: bool,
    /// Each argument's method name and source text.
    args: Vec<(String, String)>,
}

/// One change to a runtime file: whole lines out, or a call rewritten.
enum Edit {
    Lines(usize, usize),
    Replace(usize, usize, String),
}

impl Edit {
    fn start(&self) -> usize {
        match self {
            Edit::Lines(start, _) | Edit::Replace(start, _, _) => *start,
        }
    }
}

/// Apply the app's reopens to the finished ruby-family file set: Spinel,
/// CRuby and JRuby, sidecars in `sig/` or beside their file. `models`
/// are the app's Active Record models, which define their class rather
/// than reopen one: a model that shares a runtime class's name is a
/// collision this pass leaves as it is.
pub fn apply_runtime_reopens(files: &mut [(String, String)], models: &HashSet<String>) {
    let mut shipped: HashMap<String, bool> = HashMap::default();
    let mut runtime: Vec<(usize, Vec<Body>)> = Vec::new();
    for (idx, (path, content)) in files.iter().enumerate() {
        if path.starts_with("runtime/") && path.ends_with(".rb") {
            let bodies = scan(content);
            for body in &bodies {
                shipped.entry(body.name.clone()).or_insert(body.is_module);
            }
            runtime.push((idx, bodies));
        }
    }

    let mut overridden: HashSet<MethodKey> = HashSet::default();
    let mut headers: Vec<(usize, (usize, usize))> = Vec::new();
    for (idx, (path, content)) in files.iter().enumerate() {
        if !(path.starts_with("app/models/") && path.ends_with(".rb")) {
            continue;
        }
        for body in scan(content) {
            if shipped.get(&body.name) != Some(&body.is_module) || models.contains(&body.name) {
                continue;
            }
            for def in &body.defs {
                overridden.insert((body.name.clone(), def.class_side, def.name.clone()));
            }
            if let Some(range) = body.superclass {
                headers.push((idx, range));
            }
        }
    }
    if overridden.is_empty() && headers.is_empty() {
        return;
    }

    // An alias in any runtime body of the class keeps the method, and a
    // visibility call anywhere in them loses it, so both are read across
    // files: one class can be opened by several.
    let mut aliased: HashSet<(&str, &str)> = HashSet::default();
    for (_, bodies) in &runtime {
        for body in bodies {
            aliased.extend(body.aliased.iter().map(|name| (body.name.as_str(), name.as_str())));
        }
    }
    let mut removed: HashSet<MethodKey> = HashSet::default();
    let mut edits: Vec<Vec<Edit>> = runtime.iter().map(|_| Vec::new()).collect();
    for ((_, bodies), edits) in runtime.iter().zip(edits.iter_mut()) {
        for body in bodies {
            for def in &body.defs {
                let key = (body.name.clone(), def.class_side, def.name.clone());
                if !aliased.contains(&(body.name.as_str(), def.name.as_str())) && overridden.contains(&key) {
                    edits.push(Edit::Lines(def.span.0, def.span.1));
                    removed.insert(key);
                }
            }
        }
    }
    for ((_, bodies), edits) in runtime.iter().zip(edits.iter_mut()) {
        for body in bodies {
            for call in &body.visibility {
                let kept: Vec<&str> = call
                    .args
                    .iter()
                    .filter(|(name, _)| !removed.contains(&(body.name.clone(), call.class_side, name.clone())))
                    .map(|(_, text)| text.as_str())
                    .collect();
                if kept.len() == call.args.len() {
                    continue;
                }
                if kept.is_empty() {
                    edits.push(Edit::Lines(call.lines.0, call.lines.1));
                } else {
                    let (open, close) = if call.parens { ("(", ")") } else { (" ", "") };
                    let text = format!("{}{open}{}{close}", call.method, kept.join(", "));
                    edits.push(Edit::Replace(call.call.0, call.call.1, text));
                }
            }
        }
    }
    for ((idx, _), edits) in runtime.iter().zip(edits) {
        if !edits.is_empty() {
            files[*idx].1 = apply_edits(&files[*idx].1, edits);
        }
    }
    headers.sort_unstable();
    for (idx, range) in headers.into_iter().rev() {
        files[idx].1.replace_range(range.0..range.1, "");
    }
    let parents: HashSet<&str> = shipped
        .iter()
        .filter(|(name, is_module)| !**is_module && !models.contains(*name))
        .map(|(name, _)| name.as_str())
        .collect();
    for (path, content) in files.iter_mut() {
        if !path.ends_with(".rbs") {
            continue;
        }
        let rel = path.strip_prefix("sig/").unwrap_or(path);
        if rel.starts_with("runtime/") {
            *content = drop_declarations(content, &overridden);
        } else if rel.starts_with("app/models/") {
            *content = drop_parents(content, &parents);
        }
    }
}

/// Every class and module body in `source`, nested ones included, each
/// under its qualified name.
fn scan(source: &str) -> Vec<Body> {
    let result = ruby_prism::parse(source.as_bytes());
    if result.errors().count() > 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(program) = result.node().as_program_node() {
        for stmt in program.statements().body().iter() {
            scan_stmt(source, &stmt, "", &mut out);
        }
    }
    out
}

fn scan_stmt(source: &str, node: &Node<'_>, owner: &str, out: &mut Vec<Body>) {
    let (path, superclass, body, is_module) = if let Some(class) = node.as_class_node() {
        let path = class.constant_path();
        let superclass = class
            .superclass()
            .map(|parent| (path.location().end_offset(), parent.location().end_offset()));
        (path, superclass, class.body(), false)
    } else if let Some(module) = node.as_module_node() {
        (module.constant_path(), None, module.body(), true)
    } else {
        return;
    };
    let written = slice(source, &path);
    let name = match written.strip_prefix("::") {
        Some(absolute) => absolute.to_string(),
        None if owner.is_empty() => written.to_string(),
        None => format!("{owner}::{written}"),
    };
    let mut entry = Body {
        name: name.clone(),
        is_module,
        superclass,
        defs: Vec::new(),
        aliased: HashSet::default(),
        visibility: Vec::new(),
    };
    let mut nested = Vec::new();
    if let Some(stmts) = body.as_ref().and_then(|b| b.as_statements_node()) {
        for stmt in stmts.body().iter() {
            scan_member(source, &stmt, false, &mut entry);
            if let Some(singleton) = stmt.as_singleton_class_node() {
                if singleton.expression().as_self_node().is_some() {
                    if let Some(inner) = singleton.body().as_ref().and_then(|b| b.as_statements_node()) {
                        for member in inner.body().iter() {
                            scan_member(source, &member, true, &mut entry);
                        }
                    }
                }
            }
            scan_stmt(source, &stmt, &name, &mut nested);
        }
    }
    out.push(entry);
    out.extend(nested);
}

/// A `def` (bare, or behind `private`/`protected`/`public`), an alias,
/// or a visibility call naming methods by symbol.
fn scan_member(source: &str, node: &Node<'_>, singleton: bool, body: &mut Body) {
    if let Some(def) = node.as_def_node() {
        let class_side = singleton || def.receiver().is_some_and(|r| r.as_self_node().is_some());
        let name = String::from_utf8_lossy(def.name().as_slice()).into_owned();
        let span = line_span(source, node);
        body.defs.push(Def { class_side, name, span });
        return;
    }
    if let Some(alias) = node.as_alias_method_node() {
        for name in [alias.new_name(), alias.old_name()] {
            if let Some(sym) = name.as_symbol_node() {
                body.aliased.insert(String::from_utf8_lossy(sym.unescaped()).into_owned());
            }
        }
        return;
    }
    let Some(call) = node.as_call_node() else { return };
    if call.receiver().is_some() {
        return;
    }
    let method = call.name();
    let method = method.as_slice();
    let Some(args) = call.arguments() else { return };
    let args: Vec<Node<'_>> = args.arguments().iter().collect();
    // `module_function` copies the body to the module's singleton, as an
    // alias copies it to another name, and a reopen's `def` replaces only
    // the instance method: the runtime's stays for the copy.
    if method == b"alias_method" || method == b"module_function" {
        for arg in &args {
            if let Some(sym) = arg.as_symbol_node() {
                body.aliased.insert(String::from_utf8_lossy(sym.unescaped()).into_owned());
            }
        }
        return;
    }
    let class_side = match method {
        b"private" | b"protected" | b"public" => singleton,
        b"private_class_method" | b"public_class_method" => true,
        _ => return,
    };
    if let [arg] = args.as_slice() {
        if let Some(def) = arg.as_def_node() {
            let class_side = class_side || def.receiver().is_some_and(|r| r.as_self_node().is_some());
            let name = String::from_utf8_lossy(def.name().as_slice()).into_owned();
            body.defs.push(Def { class_side, name, span: line_span(source, node) });
            return;
        }
    }
    // Only an all-literal list is rewritten: a splat or a computed name
    // could name anything.
    let mut names = Vec::new();
    for arg in &args {
        let name = match (arg.as_symbol_node(), arg.as_string_node()) {
            (Some(sym), _) => sym.unescaped().to_vec(),
            (None, Some(string)) => string.unescaped().to_vec(),
            (None, None) => return,
        };
        names.push((String::from_utf8_lossy(&name).into_owned(), slice(source, arg).to_string()));
    }
    let loc = node.location();
    let after_name = call.message_loc().map_or(loc.start_offset(), |m| m.end_offset());
    body.visibility.push(Visibility {
        class_side,
        lines: line_span(source, node),
        call: (loc.start_offset(), loc.end_offset()),
        method: String::from_utf8_lossy(method).into_owned(),
        parens: source[after_name..].starts_with('('),
        args: names,
    });
}

fn slice<'a>(source: &'a str, node: &Node<'_>) -> &'a str {
    let loc = node.location();
    &source[loc.start_offset()..loc.end_offset()]
}

/// The lines `node` covers, with the comment block directly above it at
/// the same indent: what a reader sees as the definition.
fn line_span(source: &str, node: &Node<'_>) -> (usize, usize) {
    let loc = node.location();
    let mut start = source[..loc.start_offset()].rfind('\n').map_or(0, |i| i + 1);
    let indent = &source[start..loc.start_offset()];
    while start > 0 {
        let prev = source[..start - 1].rfind('\n').map_or(0, |i| i + 1);
        let line = &source[prev..start - 1];
        match line.strip_prefix(indent) {
            Some(rest) if rest.starts_with('#') => start = prev,
            _ => break,
        }
    }
    let end = source[loc.end_offset()..].find('\n').map_or(source.len(), |i| loc.end_offset() + i + 1);
    (start, end)
}

/// Apply a file's edits. Adjacent line removals (a `def` and the
/// `private :name` under it) merge into one, and each removal takes the
/// blank line that set it apart (the one after it, or else the one
/// before), so the definitions left keep their spacing.
fn apply_edits(content: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(Edit::start);
    let mut merged: Vec<Edit> = Vec::with_capacity(edits.len());
    for edit in edits {
        if let (Some(Edit::Lines(_, end)), Edit::Lines(start, next_end)) = (merged.last_mut(), &edit) {
            if *start <= *end {
                *end = (*end).max(*next_end);
                continue;
            }
        }
        merged.push(edit);
    }
    let mut out = content.to_string();
    for edit in merged.into_iter().rev() {
        match edit {
            Edit::Lines(mut start, mut end) => {
                if out[end..].starts_with('\n') {
                    end += 1;
                } else if out[..start].ends_with("\n\n") {
                    start -= 1;
                }
                out.replace_range(start..end, "");
            }
            Edit::Replace(start, end, text) => out.replace_range(start..end, &text),
        }
    }
    out
}

/// A line of an app's generated sidecar: a `class`/`module` header
/// (with the name it opens), `end`, or anything else.
enum RbsLine<'a> {
    Open(&'a str),
    End,
    Other,
}

fn rbs_line(line: &str) -> RbsLine<'_> {
    let t = line.trim_start();
    if t.trim_end() == "end" {
        return RbsLine::End;
    }
    for keyword in ["class ", "module "] {
        if let Some(rest) = t.strip_prefix(keyword) {
            let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')).unwrap_or(rest.len());
            return RbsLine::Open(&rest[..end]);
        }
    }
    RbsLine::Other
}

fn rbs_qualified(stack: &[String], name: &str) -> String {
    match name.strip_prefix("::") {
        Some(absolute) => absolute.to_string(),
        None => match stack.last() {
            Some(owner) => format!("{owner}::{name}"),
            None => name.to_string(),
        },
    }
}

/// A runtime sidecar without the declarations of the methods the app
/// redefines. Each goes whole: a signature over several lines, its `|`
/// overloads and its annotations, the spans `resolve_runtime_sig_conflicts`
/// removes as well (`project::rbs_method_declarations`). A `self?.`
/// declaration covers both receivers, so either one redefined takes it.
/// A sidecar that does not parse stays as it is, for Spinel to report.
fn drop_declarations(content: &str, overridden: &HashSet<MethodKey>) -> String {
    let Ok(decls) = crate::project::rbs_method_declarations(content) else {
        return content.to_string();
    };
    let spans: Vec<std::ops::Range<usize>> = decls
        .into_iter()
        .filter(|decl| {
            let sides: &[bool] = match decl.receiver {
                "" => &[false],
                "self." => &[true],
                _ => &[false, true],
            };
            sides.iter().any(|side| overridden.contains(&(decl.owner.clone(), *side, decl.name.clone())))
        })
        .map(|decl| decl.span)
        .collect();
    if spans.is_empty() {
        content.to_string()
    } else {
        crate::project::remove_rbs_spans(content, spans)
    }
}

/// An app sidecar whose class headers no longer name a parent for a
/// class the runtime ships, as the `.rb` header beside it.
fn drop_parents(content: &str, shipped: &HashSet<&str>) -> String {
    let mut out = String::with_capacity(content.len());
    let mut stack: Vec<String> = Vec::new();
    for line in content.split_inclusive('\n') {
        match rbs_line(line) {
            RbsLine::Open(name) => {
                let qualified = rbs_qualified(&stack, name);
                if line.trim_start().starts_with("class ") && shipped.contains(qualified.as_str()) {
                    if let Some(at) = line.find(" < ") {
                        out.push_str(&line[..at]);
                        out.push_str(if line.ends_with('\n') { "\n" } else { "" });
                        stack.push(qualified);
                        continue;
                    }
                }
                stack.push(qualified);
            }
            RbsLine::End => {
                stack.pop();
            }
            _ => {}
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect()
    }

    #[test]
    fn an_app_redefinition_replaces_the_runtime_def_and_declaration() {
        let mut files = tree(&[
            (
                "runtime/active_storage.rb",
                "module ActiveStorage\n  class Blob\n    # Whether it plays.\n    def audio?\n      false\n    end\n\n    def video?\n      false\n    end\n\n    def self.count\n      0\n    end\n  end\nend\n",
            ),
            (
                "sig/runtime/active_storage.rbs",
                "module ActiveStorage\n  class Blob\n    def audio?: () -> bool\n    def video?: () -> bool\n    def self.count: () -> Integer\n              | (String) -> Integer\n  end\nend\n",
            ),
            (
                "app/models/active_storage/blob.rb",
                "class ActiveStorage::Blob\n  def audio?\n    true\n  end\n\n  def self.count\n    42\n  end\n\n  def label\n    \"x\"\n  end\nend\n",
            ),
        ]);
        apply_runtime_reopens(&mut files, &HashSet::default());
        assert_eq!(
            files[0].1,
            "module ActiveStorage\n  class Blob\n    def video?\n      false\n    end\n  end\nend\n"
        );
        assert_eq!(files[1].1, "module ActiveStorage\n  class Blob\n    def video?: () -> bool\n  end\nend\n");
    }

    #[test]
    fn the_receiver_and_the_class_must_both_match() {
        let runtime = "class Mailer\n  def self.deliver\n    1\n  end\n\n  def deliver\n    2\n  end\nend\n\nclass Other\n  def ping\n    3\n  end\nend\n";
        let mut files = tree(&[
            ("runtime/mailer.rb", runtime),
            ("app/models/mailer.rb", "class Mailer\n  def deliver\n    9\n  end\nend\n"),
            ("app/models/ping.rb", "class Ping\n  def ping\n    9\n  end\nend\n"),
        ]);
        apply_runtime_reopens(&mut files, &HashSet::default());
        assert_eq!(files[0].1, "class Mailer\n  def self.deliver\n    1\n  end\nend\n\nclass Other\n  def ping\n    3\n  end\nend\n");
    }

    #[test]
    fn a_reopen_header_drops_its_superclass_in_the_rb_and_the_rbs() {
        let mut files = tree(&[
            ("runtime/active_storage.rb", "module ActiveStorage\n  class Blob\n  end\nend\n"),
            ("app/models/active_storage/blob.rb", "class ActiveStorage::Blob < ActiveStorage::Record\n  def label\n    \"x\"\n  end\nend\n"),
            ("app/models/active_storage/blob.rbs", "module ActiveStorage\n  class Blob < ActiveStorage::Record\n    def label: () -> String\n  end\nend\n"),
            ("app/models/record.rb", "class Record < ActiveStorage::Record\nend\n"),
        ]);
        apply_runtime_reopens(&mut files, &HashSet::default());
        assert_eq!(files[1].1, "class ActiveStorage::Blob\n  def label\n    \"x\"\n  end\nend\n");
        assert_eq!(files[2].1, "module ActiveStorage\n  class Blob\n    def label: () -> String\n  end\nend\n");
        assert_eq!(files[3].1, "class Record < ActiveStorage::Record\nend\n");
    }

    #[test]
    fn a_multiline_declaration_goes_whole_with_its_overloads_and_annotations() {
        let rbs = "module ActionView\n  module ViewHelpers\n    %a{pure}\n    def self.word_wrap: (\n      String text,\n      ?line_width: Integer\n    ) -> String\n                      | (nil text) -> String\n    def self.kept: (\n      String text\n    ) -> String\n  end\nend\n";
        let mut files = tree(&[
            ("runtime/action_view/view_helpers.rb", "module ActionView\n  module ViewHelpers\n    def self.word_wrap(text, line_width: 80)\n      text\n    end\n\n    def self.kept(text)\n      text\n    end\n  end\nend\n"),
            ("runtime/action_view/view_helpers.rbs", rbs),
            ("app/models/action_view/view_helpers.rb", "module ActionView::ViewHelpers\n  def self.word_wrap(text, line_width: 80)\n    \"wrapped\"\n  end\nend\n"),
        ]);
        apply_runtime_reopens(&mut files, &HashSet::default());
        assert_eq!(files[1].1, "module ActionView\n  module ViewHelpers\n    def self.kept: (\n      String text\n    ) -> String\n  end\nend\n");
    }

    #[test]
    fn a_singleton_block_private_def_and_alias_are_read() {
        let runtime = "class Base\n  class << self\n    def build\n      1\n    end\n  end\n\n  private def secret\n    2\n  end\n\n  def kept\n    3\n  end\n  alias_method :also, :kept\nend\n";
        let mut files = tree(&[
            ("runtime/base.rb", runtime),
            ("app/models/base.rb", "class Base\n  def self.build\n    9\n  end\n\n  def secret\n    9\n  end\n\n  def kept\n    9\n  end\nend\n"),
        ]);
        apply_runtime_reopens(&mut files, &HashSet::default());
        assert_eq!(files[0].1, "class Base\n  class << self\n  end\n\n  def kept\n    3\n  end\n  alias_method :also, :kept\nend\n");
    }

    #[test]
    fn a_visibility_call_loses_the_names_whose_def_went() {
        let runtime = "class Relation\n  def self.build\n    1\n  end\n  private_class_method :build\n\n  def a\n    1\n  end\n  private :a\n\n  def b\n    2\n  end\n\n  def c\n    3\n  end\n  private :b, :c\n  protected(:c, \"b\")\nend\n";
        let mut files = tree(&[
            ("runtime/relation.rb", runtime),
            ("app/models/relation.rb", "class Relation\n  def self.build\n    9\n  end\n\n  def a\n    9\n  end\n\n  def c\n    9\n  end\nend\n"),
        ]);
        apply_runtime_reopens(&mut files, &HashSet::default());
        assert_eq!(files[0].1, "class Relation\n  def b\n    2\n  end\n  private :b\n  protected(\"b\")\nend\n");
    }

    #[test]
    fn a_singleton_private_loses_its_name_and_module_function_keeps_the_def() {
        let runtime = "module Util\n  def helper\n    1\n  end\n  module_function :helper\n\n  class << self\n    def built\n      1\n    end\n    private :built\n  end\nend\n";
        let mut files = tree(&[
            ("runtime/util.rb", runtime),
            ("app/models/util.rb", "module Util\n  def helper\n    2\n  end\n\n  def self.built\n    2\n  end\nend\n"),
        ]);
        apply_runtime_reopens(&mut files, &HashSet::default());
        assert_eq!(files[0].1, "module Util\n  def helper\n    1\n  end\n  module_function :helper\n\n  class << self\n  end\nend\n");
    }

    #[test]
    fn a_tree_without_reopens_is_untouched() {
        let mut files = tree(&[
            ("runtime/mailer.rb", "class Mailer\n  def deliver\n    1\n  end\nend\n"),
            ("app/views/mailer.rb", "class Mailer\n  def deliver\n    2\n  end\nend\n"),
            ("app/models/post.rb", "class Post < ApplicationRecord\n  def deliver\n    3\n  end\nend\n"),
        ]);
        let before = files.clone();
        apply_runtime_reopens(&mut files, &HashSet::default());
        assert_eq!(files, before);
    }

    #[test]
    fn a_model_sharing_a_runtime_class_name_is_not_a_reopen() {
        let mut files = tree(&[
            ("runtime/user_agent.rb", "class UserAgent\n  def browser\n    1\n  end\nend\n"),
            ("app/models/user_agent.rb", "class UserAgent < ApplicationRecord\n  def browser\n    2\n  end\nend\n"),
            ("app/models/user_agent.rbs", "class UserAgent < ApplicationRecord\n  def browser: () -> Integer\nend\n"),
        ]);
        let before = files.clone();
        apply_runtime_reopens(&mut files, &HashSet::from_iter(["UserAgent".to_string()]));
        assert_eq!(files, before);
    }
}
