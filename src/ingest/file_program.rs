//! A gem-source file's file-level program (`App::file_programs`): what
//! Ruby runs when it loads the file beyond the declarations the
//! library-class walk carries, in source order.
//!
//! The library-class walk keeps a file's classes and modules and drops
//! the rest of its top level: `Adaptkit::LOADED << :alpha` above
//! `module AlphaAdapter`, the `require_relative`s an entry file loads its
//! parts with. A gem's own code runs those as it loads, in the order its
//! requires reach them. Each top-level statement becomes an item:
//!
//! - a `class`/`module` declaration loads the emitted files of the
//!   classes the walk carried from it, where it stands;
//! - `require` / `require_relative` of a literal names the file it loads;
//! - a constant write before any statement rides with the file's
//!   classes, as it does today;
//! - `ActiveSupport.on_load` is `on_load_reopen`'s, and `I18n.load_path`
//!   the compiled translations';
//! - any other statement is carried as the expression it is, or, when
//!   the expression ingest refuses it, named as not carried.
//!
//! Every gem-source file has a program, one of declarations alone too,
//! so a `require` of any of the gem's files loads its program. Only gem
//! sources (PATH gems and source gems) get one for now: an app's own
//! `lib/` keeps its current treatment until the constructs it holds
//! (`to_prepare`, `on_load`) are carried here too.

use crate::app::{FileProgram, ProgramItem};
use crate::ingest::prism_api::PrismLocation;
use crate::det::HashSet;
use crate::span::Span;
use crate::{ClassId, Symbol};

use super::expr::ingest_expr;
use super::util::{
    class_name_path, constant_id_str, find_all_classes_with_scope, find_all_module_declarations_with_scope,
    module_name_path,
};

/// `file`'s program; `None` when it does not parse as a program.
/// `carried` names the classes and modules the library-class walk
/// carried from it; `list_dir` answers a directory's entries
/// (`(name, is_dir)`) for a glob require.
pub(super) fn file_program(
    source: &[u8],
    file: &str,
    carried: &HashSet<String>,
    list_dir: &dyn Fn(&std::path::Path) -> Vec<(String, bool)>,
) -> Option<FileProgram> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let program = root.as_program_node()?;
    let span_of = |node: &ruby_prism::Node<'_>| {
        let loc = node.location();
        Span { file: super::sources::file_id(file), start: loc.start_offset() as u32, end: loc.end_offset() as u32 }
    };
    let statements: Vec<ruby_prism::Node<'_>> = program.statements().body().iter().collect();
    let items = program_items(&statements, file, carried, list_dir, &span_of);
    Some(FileProgram { source: file.to_string(), items })
}

/// The items of a run of top-level statements (`file_program`'s, or a
/// file-level branch's).
fn program_items(
    statements: &[ruby_prism::Node<'_>],
    file: &str,
    carried: &HashSet<String>,
    list_dir: &dyn Fn(&std::path::Path) -> Vec<(String, bool)>,
    span_of: &dyn Fn(&ruby_prism::Node<'_>) -> Span,
) -> Vec<ProgramItem> {
    let mut items = Vec::new();
    for stmt in statements {
        if stmt.as_class_node().is_some() || stmt.as_module_node().is_some() {
            // In source order, as Ruby evaluates them; an enclosing
            // declaration starts before what it nests, so it loads first.
            let mut found: Vec<(usize, String)> = Vec::new();
            for (scope, c) in find_all_classes_with_scope(&stmt) {
                let Some(p) = class_name_path(&c) else { continue };
                found.push((c.location().start_offset(), scope.into_iter().chain(p).collect::<Vec<_>>().join("::")));
            }
            let mut modules: HashSet<String> = HashSet::default();
            for (scope, m) in find_all_module_declarations_with_scope(&stmt) {
                let Some(p) = module_name_path(&m) else { continue };
                let name = scope.into_iter().chain(p).collect::<Vec<_>>().join("::");
                modules.insert(name.clone());
                found.push((m.location().start_offset(), name));
            }
            found.sort_by_key(|(start, _)| *start);
            let mut names: Vec<ClassId> = Vec::new();
            for (_, name) in found {
                if carried.contains(&name) && !names.iter().any(|n| n.0.as_str() == name) {
                    names.push(ClassId(Symbol::from(name)));
                }
            }
            let mut opened: Vec<Vec<(String, bool)>> = Vec::new();
            for chain in nestings(&stmt) {
                let name = chain.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>().join("::");
                let module = chain.last().is_some_and(|(_, is_class)| !is_class);
                if module && !carried.contains(&name) && modules.contains(&name) && !opened.contains(&chain) {
                    opened.push(chain);
                }
            }
            if !opened.is_empty() {
                items.push(ProgramItem::Opened(opened));
            }
            if !names.is_empty() {
                items.push(ProgramItem::Declared(names));
            }
            // What the bodies require and autoload as they load, after them.
            body_requires(&stmt, &[], file, list_dir, &span_of, &mut items);
            continue;
        }
        // A constant rides with the file's classes (`carry_file_constants`).
        if stmt.as_constant_write_node().is_some() || stmt.as_constant_path_write_node().is_some() {
            continue;
        }
        if let Some((name, feature)) = literal_autoload(&stmt) {
            items.push(ProgramItem::Autoload { span: span_of(&stmt), owner: "Object".to_string(), name, feature });
            continue;
        }
        // `ActiveSupport.on_load(:x) { … }` is `on_load_reopen`'s, which
        // carries the reopen it can and reports the rest.
        if is_on_load(&stmt) {
            continue;
        }
        // `I18n.load_path += Dir[…]`: the gem's locale files are compiled
        // into the tree's translations (`i18n::gems::gem_locale_files`).
        if writes_i18n_load_path(&stmt) {
            continue;
        }
        if let Some(globbed) = glob_requires(&stmt, file, list_dir) {
            for (feature, relative) in globbed {
                items.push(ProgramItem::Require { span: span_of(&stmt), feature, relative });
            }
            continue;
        }
        if let Some(call) = stmt.as_call_node() {
            let name = constant_id_str(&call.name());
            if call.receiver().is_none() && call.block().is_none() && matches!(name, "require" | "require_relative") {
                let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
                if let [one] = args.as_slice() {
                    if let Some(feature) = one.as_string_node() {
                        items.push(ProgramItem::Require {
                            span: span_of(&stmt),
                            feature: String::from_utf8_lossy(feature.unescaped()).into_owned(),
                            relative: name == "require_relative",
                        });
                        continue;
                    }
                }
                items.push(ProgramItem::NotCarried {
                    span: span_of(&stmt),
                    message: format!("`{name}` of anything but a string literal is not carried; Ruby loads it when the file loads"),
                });
                continue;
            }
        }
        if let Some(branch) = file_branch(&stmt, file, carried, list_dir, span_of) {
            items.push(branch);
            continue;
        }
        if is_file_return(&stmt) {
            items.push(ProgramItem::NotCarried {
                span: span_of(&stmt),
                message: "a file-level `return` is not carried; Ruby stops loading the file there".to_string(),
            });
            continue;
        }
        if declares_inside(&stmt) {
            items.push(ProgramItem::NotCarried {
                span: span_of(&stmt),
                message: "a file-level statement that declares a class, module or method is not carried; Ruby defines it when the file loads".to_string(),
            });
            continue;
        }
        match ingest_expr(&stmt, file) {
            Ok(expr) => items.push(ProgramItem::Statement(expr)),
            Err(err) => items.push(ProgramItem::NotCarried {
                span: span_of(&stmt),
                message: format!("the ingest cannot carry this file-level statement: {err}"),
            }),
        }
    }
    items
}

/// A file-level `if` / `unless` (not a modifier or ternary) whose
/// branches declare a class, module or method: a branch item, each arm's
/// statements its own items, an `elsif` a branch in the `else` items.
fn file_branch(
    stmt: &ruby_prism::Node<'_>,
    file: &str,
    carried: &HashSet<String>,
    list_dir: &dyn Fn(&std::path::Path) -> Vec<(String, bool)>,
    span_of: &dyn Fn(&ruby_prism::Node<'_>) -> Span,
) -> Option<ProgramItem> {
    if !declares_inside(stmt) || super::visibility::console_only_node(stmt) {
        return None;
    }
    let items = |stmts: &[ruby_prism::Node<'_>]| program_items(stmts, file, carried, list_dir, span_of);
    if let Some(branch) = stmt.as_if_node() {
        branch.if_keyword_loc()?;
        branch.end_keyword_loc()?;
        let condition = ingest_expr(&branch.predicate(), file).ok()?;
        let then_items = items(&statements(branch.statements()));
        let else_items = match branch.subsequent() {
            None => Vec::new(),
            Some(sub) => {
                if let Some(other) = sub.as_else_node() {
                    items(&statements(other.statements()))
                } else if sub.as_if_node().is_some() {
                    // `elsif`: a branch of its own, in the `else`.
                    vec![file_branch_elsif(&sub, file, carried, list_dir, span_of)?]
                } else {
                    return None;
                }
            }
        };
        return Some(ProgramItem::Branch { condition, negated: false, then_items, else_items });
    }
    let branch = stmt.as_unless_node()?;
    branch.end_keyword_loc()?;
    let condition = ingest_expr(&branch.predicate(), file).ok()?;
    let then_items = items(&statements(branch.statements()));
    let else_items = branch.else_clause().map(|e| items(&statements(e.statements()))).unwrap_or_default();
    Some(ProgramItem::Branch { condition, negated: true, then_items, else_items })
}

/// A branch's statements.
fn statements<'pr>(s: Option<ruby_prism::StatementsNode<'pr>>) -> Vec<ruby_prism::Node<'pr>> {
    s.map(|s| s.body().iter().collect()).unwrap_or_default()
}

/// An `elsif` node as a branch item (it has no `if` keyword of its own).
fn file_branch_elsif(
    node: &ruby_prism::Node<'_>,
    file: &str,
    carried: &HashSet<String>,
    list_dir: &dyn Fn(&std::path::Path) -> Vec<(String, bool)>,
    span_of: &dyn Fn(&ruby_prism::Node<'_>) -> Span,
) -> Option<ProgramItem> {
    let branch = node.as_if_node()?;
    let condition = ingest_expr(&branch.predicate(), file).ok()?;
    let then_items = program_items(&statements(branch.statements()), file, carried, list_dir, span_of);
    let else_items = match branch.subsequent() {
        None => Vec::new(),
        Some(sub) if sub.as_else_node().is_some() => {
            program_items(&statements(sub.as_else_node()?.statements()), file, carried, list_dir, span_of)
        }
        Some(sub) => vec![file_branch_elsif(&sub, file, carried, list_dir, span_of)?],
    };
    Some(ProgramItem::Branch { condition, negated: false, then_items, else_items })
}

/// `Dir[File.join(__dir__, "x/**/*.rb")].each { |f| require_relative f }`
/// (or `Dir.glob(…)`, `"#{__dir__}/x/*.rb"`, `require f`): the files the
/// glob finds now, in the order Ruby's `Dir[]` gives them (each
/// directory's entries sorted by name, a directory's own files where
/// the directory sorts), each a `require_relative` of its path from the
/// file. `None` for anything else.
fn glob_requires(
    stmt: &ruby_prism::Node<'_>,
    file: &str,
    list_dir: &dyn Fn(&std::path::Path) -> Vec<(String, bool)>,
) -> Option<Vec<(String, bool)>> {
    let call = stmt.as_call_node()?;
    if constant_id_str(&call.name()) != "each" || call.arguments().is_some() {
        return None;
    }
    let glob = call.receiver()?;
    let glob = glob.as_call_node()?;
    let on_dir = glob.receiver().and_then(|r| r.as_constant_read_node()).is_some_and(|r| constant_id_str(&r.name()) == "Dir");
    if !on_dir || !matches!(constant_id_str(&glob.name()), "[]" | "glob") {
        return None;
    }
    let args: Vec<_> = glob.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let [pattern] = args.as_slice() else { return None };
    let pattern = dir_relative_pattern(pattern)?;
    let block = call.block()?.as_block_node()?;
    let var = block
        .parameters()?
        .as_block_parameters_node()?
        .parameters()?
        .requireds()
        .iter()
        .next()?
        .as_required_parameter_node()
        .map(|p| constant_id_str(&p.name()).to_string())?;
    let inner: Vec<ruby_prism::Node<'_>> = block.body()?.as_statements_node()?.body().iter().collect();
    let [only] = inner.as_slice() else { return None };
    let require = only.as_call_node()?;
    if require.receiver().is_some() || !matches!(constant_id_str(&require.name()), "require" | "require_relative") {
        return None;
    }
    let req_args: Vec<_> = require.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let [arg] = req_args.as_slice() else { return None };
    if arg.as_local_variable_read_node().is_none_or(|r| constant_id_str(&r.name()) != var) {
        return None;
    }
    // `<dir>/**/*.rb` or `<dir>/*.rb`, relative to the file's directory.
    let (dir, recursive) = if let Some(dir) = pattern.strip_suffix("/**/*.rb") {
        (dir, true)
    } else {
        (pattern.strip_suffix("/*.rb")?, false)
    };
    let base = std::path::Path::new(file).parent()?;
    let mut found = Vec::new();
    fn walk(
        at: &std::path::Path,
        rel: &str,
        recursive: bool,
        list_dir: &dyn Fn(&std::path::Path) -> Vec<(String, bool)>,
        out: &mut Vec<String>,
    ) {
        let mut entries = list_dir(at);
        entries.sort();
        for (name, is_dir) in entries {
            if name.starts_with('.') {
                continue;
            }
            let path = format!("{rel}/{name}");
            if is_dir {
                if recursive {
                    walk(&at.join(&name), &path, recursive, list_dir, out);
                }
            } else if let Some(stem) = path.strip_suffix(".rb") {
                out.push(stem.to_string());
            }
        }
    }
    walk(&base.join(dir), dir, recursive, list_dir, &mut found);
    Some(found.into_iter().map(|f| (f, true)).collect())
}

/// A glob argument naming a path under the file's own directory:
/// `File.join(__dir__, "x/**/*.rb")` or `"#{__dir__}/x/**/*.rb"`, as
/// `x/**/*.rb`.
fn dir_relative_pattern(node: &ruby_prism::Node<'_>) -> Option<String> {
    let is_dir = |n: &ruby_prism::Node<'_>| {
        n.as_call_node().is_some_and(|c| c.receiver().is_none() && c.arguments().is_none() && constant_id_str(&c.name()) == "__dir__")
    };
    if let Some(join) = node.as_call_node() {
        let on_file = join.receiver().and_then(|r| r.as_constant_read_node()).is_some_and(|r| constant_id_str(&r.name()) == "File");
        if !on_file || constant_id_str(&join.name()) != "join" {
            return None;
        }
        let args: Vec<_> = join.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let [first, rest @ ..] = args.as_slice() else { return None };
        if !is_dir(first) || rest.is_empty() {
            return None;
        }
        let parts: Option<Vec<String>> =
            rest.iter().map(|n| n.as_string_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned())).collect();
        return Some(parts?.join("/"));
    }
    let interpolated = node.as_interpolated_string_node()?;
    let parts: Vec<ruby_prism::Node<'_>> = interpolated.parts().iter().collect();
    let [head, tail] = parts.as_slice() else { return None };
    let embedded = head.as_embedded_statements_node()?;
    let statements: Vec<ruby_prism::Node<'_>> = embedded.statements()?.body().iter().collect();
    let [only] = statements.as_slice() else { return None };
    if !is_dir(only) {
        return None;
    }
    let tail = String::from_utf8_lossy(tail.as_string_node()?.unescaped()).into_owned();
    tail.strip_prefix('/').map(str::to_string)
}

/// The requires a declaration's bodies run as they load, in source
/// order, nested bodies included: a literal `require "x"` /
/// `require_relative "x"` statement, and a loop over a literal list
/// whose block requires one file per element
/// (`LIST.each { |n| require_relative "adapters/#{n}_adapter" }`, with
/// `LIST = %w[alpha beta].freeze` written in the same body), as one
/// require per element. Each runs after the declaration's own items:
/// the files it loads see the body's constants defined.
fn body_requires(
    decl: &ruby_prism::Node<'_>,
    scope: &[String],
    file: &str,
    list_dir: &dyn Fn(&std::path::Path) -> Vec<(String, bool)>,
    span_of: &dyn Fn(&ruby_prism::Node<'_>) -> Span,
    out: &mut Vec<ProgramItem>,
) {
    let path = decl
        .as_class_node()
        .and_then(|c| class_name_path(&c))
        .or_else(|| decl.as_module_node().and_then(|m| module_name_path(&m)));
    let Some(path) = path else { return };
    let owner: Vec<String> = scope.iter().cloned().chain(path).collect();
    let body = decl
        .as_class_node()
        .and_then(|c| c.body())
        .or_else(|| decl.as_module_node().and_then(|m| m.body()));
    let Some(body) = body else { return };
    let statements: Vec<ruby_prism::Node<'_>> = match body.as_statements_node() {
        Some(s) => s.body().iter().collect(),
        None => vec![body],
    };
    // `extend ActiveSupport::Autoload`: an `autoload :Name` with no path
    // loads `<owner>/<name>`, underscored, as Rails derives it.
    let as_autoload = statements.iter().any(|stmt| {
        stmt.as_call_node().is_some_and(|call| {
            call.receiver().is_none()
                && constant_id_str(&call.name()) == "extend"
                && call.arguments().is_some_and(|a| {
                    a.arguments().iter().any(|arg| {
                        arg.as_constant_path_node().is_some_and(|p| {
                            p.name().is_some_and(|n| constant_id_str(&n) == "Autoload")
                                && p.parent().and_then(|q| q.as_constant_read_node()).is_some_and(|q| constant_id_str(&q.name()) == "ActiveSupport")
                        })
                    })
                })
        })
    });
    let derived = |name: &str| crate::naming::underscore(&format!("{}::{name}", owner.join("::")));
    // The body's literal lists, by constant name.
    let mut lists: Vec<(String, Vec<String>)> = Vec::new();
    for stmt in &statements {
        if let Some(write) = stmt.as_constant_write_node() {
            if let Some(values) = literal_list(&write.value()) {
                lists.push((constant_id_str(&write.name()).to_string(), values));
            }
        }
    }
    let mut locals = super::glob_autoload::Locals::default();
    for stmt in &statements {
        if stmt.as_class_node().is_some() || stmt.as_module_node().is_some() {
            body_requires(stmt, &owner, file, list_dir, span_of, out);
            continue;
        }
        if locals.record(stmt, file) {
            continue;
        }
        if let Some((name, feature)) = literal_autoload(stmt) {
            out.push(ProgramItem::Autoload { span: span_of(stmt), owner: owner.join("::"), name, feature });
            continue;
        }
        // `Dir[…].each { |f| autoload(…) }`: each constant autoloads the
        // file whose iteration named it, by its feature under the gem's
        // lib/ (what a literal `autoload` would name).
        if let Some(found) = super::glob_autoload::glob_autoloads(stmt, file, &locals, list_dir) {
            let features: Option<Vec<(String, String)>> = found
                .into_iter()
                .map(|(name, path)| {
                    let at = path.rfind("/lib/")?;
                    Some((name, path[at + 5..].strip_suffix(".rb")?.to_string()))
                })
                .collect();
            if let Some(features) = features {
                for (name, feature) in features {
                    out.push(ProgramItem::Autoload { span: span_of(stmt), owner: owner.join("::"), name, feature });
                }
                continue;
            }
        }
        if as_autoload {
            // `autoload :Name`, alone or in `eager_autoload do … end`.
            let mut autoloads: Vec<&ruby_prism::Node<'_>> = vec![stmt];
            let eager: Vec<ruby_prism::Node<'_>> = stmt
                .as_call_node()
                .filter(|c| c.receiver().is_none() && constant_id_str(&c.name()) == "eager_autoload")
                .and_then(|c| c.block())
                .and_then(|b| b.as_block_node())
                .and_then(|b| b.body())
                .and_then(|b| b.as_statements_node())
                .map(|b| b.body().iter().collect())
                .unwrap_or_default();
            if !eager.is_empty() {
                autoloads = eager.iter().collect();
            }
            let mut matched = false;
            for node in autoloads {
                if let Some(name) = derived_autoload(node) {
                    out.push(ProgramItem::Autoload { span: span_of(node), owner: owner.join("::"), feature: derived(&name), name });
                    matched = true;
                }
            }
            if matched {
                continue;
            }
        }
        let Some(call) = stmt.as_call_node() else { continue };
        if let Some((feature, relative)) = literal_require(&call) {
            out.push(ProgramItem::Require { span: span_of(stmt), feature, relative });
            continue;
        }
        // `LIST.each { |n| require_relative "…#{n}…" }`
        if constant_id_str(&call.name()) != "each" || call.arguments().is_some() {
            continue;
        }
        let Some(receiver) = call.receiver() else { continue };
        let values = match receiver.as_constant_read_node() {
            Some(read) => {
                let name = constant_id_str(&read.name()).to_string();
                lists.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone())
            }
            None => literal_list(&receiver),
        };
        let Some(values) = values else { continue };
        let Some(block) = call.block().and_then(|b| b.as_block_node()) else { continue };
        let Some(var) = block
            .parameters()
            .and_then(|p| p.as_block_parameters_node())
            .and_then(|p| p.parameters())
            .and_then(|p| p.requireds().iter().next())
            .and_then(|p| p.as_required_parameter_node())
            .map(|p| constant_id_str(&p.name()).to_string())
        else {
            continue;
        };
        let Some(inner) = block.body().and_then(|b| b.as_statements_node()) else { continue };
        let inner: Vec<ruby_prism::Node<'_>> = inner.body().iter().collect();
        let [only] = inner.as_slice() else { continue };
        let Some(require) = only.as_call_node() else { continue };
        let name = constant_id_str(&require.name());
        if require.receiver().is_some() || !matches!(name, "require" | "require_relative") {
            continue;
        }
        let args: Vec<_> = require.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        let [arg] = args.as_slice() else { continue };
        let features: Option<Vec<String>> = values.iter().map(|value| interpolated(arg, &var, value)).collect();
        let Some(features) = features else { continue };
        for feature in features {
            out.push(ProgramItem::Require { span: span_of(stmt), feature, relative: name == "require_relative" });
        }
    }
}

/// Each `class`/`module` a declaration statement holds, as the nesting
/// written down to it: `(segment, is_class)` per level, a compact
/// `A::B` name one level.
fn nestings(stmt: &ruby_prism::Node<'_>) -> Vec<Vec<(String, bool)>> {
    fn walk(node: &ruby_prism::Node<'_>, outer: &[(String, bool)], out: &mut Vec<Vec<(String, bool)>>) {
        let (path, is_class, body) = if let Some(c) = node.as_class_node() {
            (class_name_path(&c), true, c.body())
        } else if let Some(m) = node.as_module_node() {
            (module_name_path(&m), false, m.body())
        } else {
            return;
        };
        let Some(path) = path else { return };
        let mut chain = outer.to_vec();
        chain.push((path.join("::"), is_class));
        out.push(chain.clone());
        if let Some(statements) = body.as_ref().and_then(|b| b.as_statements_node()) {
            for inner in statements.body().iter() {
                walk(&inner, &chain, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(stmt, &[], &mut out);
    out
}

/// `ActiveSupport.on_load(…) { … }`.
fn is_on_load(stmt: &ruby_prism::Node<'_>) -> bool {
    stmt.as_call_node().is_some_and(|call| {
        constant_id_str(&call.name()) == "on_load"
            && call.receiver().is_some_and(|r| {
                r.as_constant_read_node().is_some_and(|c| constant_id_str(&c.name()) == "ActiveSupport")
                    || r.as_constant_path_node().is_some_and(|p| {
                        p.parent().is_none() && p.name().is_some_and(|n| constant_id_str(&n) == "ActiveSupport")
                    })
            })
    })
}

/// `I18n.load_path += …`, `I18n.load_path << …` and other calls on it.
fn writes_i18n_load_path(stmt: &ruby_prism::Node<'_>) -> bool {
    let on_i18n = |r: Option<ruby_prism::Node<'_>>| {
        r.is_some_and(|r| r.as_constant_read_node().is_some_and(|c| constant_id_str(&c.name()) == "I18n"))
    };
    let load_path = |n: &ruby_prism::Node<'_>| {
        n.as_call_node().is_some_and(|c| constant_id_str(&c.name()) == "load_path" && on_i18n(c.receiver()))
    };
    if let Some(write) = stmt.as_call_operator_write_node() {
        return constant_id_str(&write.read_name()) == "load_path" && on_i18n(write.receiver());
    }
    stmt.as_call_node().is_some_and(|call| call.receiver().is_some_and(|r| load_path(&r)))
}

/// `autoload :Name, "feature"` (receiverless, of literals), as
/// `(name, feature)`.
fn literal_autoload(stmt: &ruby_prism::Node<'_>) -> Option<(String, String)> {
    let call = stmt.as_call_node()?;
    if call.receiver().is_some() || call.block().is_some() || constant_id_str(&call.name()) != "autoload" {
        return None;
    }
    let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let [name, feature] = args.as_slice() else { return None };
    let name = String::from_utf8_lossy(name.as_symbol_node()?.unescaped()).into_owned();
    let feature = String::from_utf8_lossy(feature.as_string_node()?.unescaped()).into_owned();
    Some((name, feature))
}

/// `autoload :Name` with no path (`ActiveSupport::Autoload`'s), as `Name`.
fn derived_autoload(stmt: &ruby_prism::Node<'_>) -> Option<String> {
    let call = stmt.as_call_node()?;
    if call.receiver().is_some() || call.block().is_some() || constant_id_str(&call.name()) != "autoload" {
        return None;
    }
    let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let [name] = args.as_slice() else { return None };
    Some(String::from_utf8_lossy(name.as_symbol_node()?.unescaped()).into_owned())
}

/// `require "x"` / `require_relative "x"` of a string literal, as
/// `(feature, relative)`.
fn literal_require(call: &ruby_prism::CallNode<'_>) -> Option<(String, bool)> {
    let name = constant_id_str(&call.name());
    if call.receiver().is_some() || call.block().is_some() || !matches!(name, "require" | "require_relative") {
        return None;
    }
    let args: Vec<_> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let [one] = args.as_slice() else { return None };
    let feature = one.as_string_node()?;
    Some((String::from_utf8_lossy(feature.unescaped()).into_owned(), name == "require_relative"))
}

/// The values of a literal list of strings or symbols (`%w[a b]`,
/// `[:a, :b]`), frozen or not.
fn literal_list(node: &ruby_prism::Node<'_>) -> Option<Vec<String>> {
    let frozen = node
        .as_call_node()
        .filter(|call| constant_id_str(&call.name()) == "freeze" && call.arguments().is_none())
        .and_then(|call| call.receiver());
    let array = frozen.as_ref().unwrap_or(node).as_array_node()?;
    array
        .elements()
        .iter()
        .map(|e| {
            e.as_string_node()
                .map(|s| String::from_utf8_lossy(s.unescaped()).into_owned())
                .or_else(|| e.as_symbol_node().map(|s| String::from_utf8_lossy(s.unescaped()).into_owned()))
        })
        .collect()
}

/// A string literal with `var` interpolated as `value`
/// (`"adapters/#{n}_adapter"`), or a plain one.
fn interpolated(node: &ruby_prism::Node<'_>, var: &str, value: &str) -> Option<String> {
    if let Some(s) = node.as_string_node() {
        return Some(String::from_utf8_lossy(s.unescaped()).into_owned());
    }
    let interpolated = node.as_interpolated_string_node()?;
    let mut out = String::new();
    for part in interpolated.parts().iter() {
        if let Some(s) = part.as_string_node() {
            out.push_str(&String::from_utf8_lossy(s.unescaped()));
            continue;
        }
        let embedded = part.as_embedded_statements_node()?;
        let statements: Vec<ruby_prism::Node<'_>> = embedded.statements()?.body().iter().collect();
        let [only] = statements.as_slice() else { return None };
        let read = only.as_local_variable_read_node()?;
        if constant_id_str(&read.name()) != var {
            return None;
        }
        out.push_str(value);
    }
    Some(out)
}

/// A top-level statement holding a `return` outside a `def` or lambda,
/// which ends the file's load.
fn is_file_return(stmt: &ruby_prism::Node<'_>) -> bool {
    use ruby_prism::Visit;
    #[derive(Default)]
    struct Finds(bool);
    impl<'pr> Visit<'pr> for Finds {
        fn visit_return_node(&mut self, _: &ruby_prism::ReturnNode<'pr>) {
            self.0 = true;
        }
        fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {}
        fn visit_lambda_node(&mut self, _: &ruby_prism::LambdaNode<'pr>) {}
        fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {}
        fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {}
    }
    let mut finds = Finds::default();
    finds.visit(stmt);
    finds.0
}

/// Whether a statement other than a declaration holds a `class`,
/// `module` or `def` (one inside an `if`, a block, a `begin`): the
/// expression ingest would read it as nil, and the library-class walk
/// does not reach it.
fn declares_inside(stmt: &ruby_prism::Node<'_>) -> bool {
    use ruby_prism::Visit;
    #[derive(Default)]
    struct Finds(bool);
    impl<'pr> Visit<'pr> for Finds {
        fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {
            self.0 = true;
        }
        fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {
            self.0 = true;
        }
        fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {
            self.0 = true;
        }
    }
    let mut finds = Finds::default();
    finds.visit(stmt);
    finds.0
}

/// The literal `require "x"` features a file runs as it loads: at its
/// top level and in its class and module bodies (not in a method). For
/// an app file, whose requires of a gem's files load that gem at boot.
pub(super) fn literal_requires(source: &[u8], file: &str) -> Vec<String> {
    fn walk(statements: &[ruby_prism::Node<'_>], out: &mut Vec<String>) {
        for stmt in statements {
            if let Some((feature, false)) = stmt.as_call_node().and_then(|c| literal_require(&c)) {
                out.push(feature);
                continue;
            }
            let body = stmt
                .as_class_node()
                .and_then(|c| c.body())
                .or_else(|| stmt.as_module_node().and_then(|m| m.body()));
            if let Some(statements) = body.as_ref().and_then(|b| b.as_statements_node()) {
                let inner: Vec<ruby_prism::Node<'_>> = statements.body().iter().collect();
                walk(&inner, out);
            }
        }
    }
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(program) = root.as_program_node() else { return Vec::new() };
    let statements: Vec<ruby_prism::Node<'_>> = program.statements().body().iter().collect();
    let mut out = Vec::new();
    walk(&statements, &mut out);
    out
}
