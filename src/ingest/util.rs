//! Cross-cutting Prism AST helpers used by every ingest submodule.
//!
//! Everything here is an `Option<T>` / `Vec<T>` leaf helper — no
//! `IngestResult`, no recursion into domain-specific structures.
//! Functions that fail at the recognizer level (and need to surface
//! `IngestError`) live in their domain module, not here.

use ruby_prism::Node;

use crate::Symbol;
use crate::dialect::Comment;
use crate::expr::ArrayStyle;
use crate::span::Span;

// ---- Leaf literal extractors -------------------------------------------

pub(super) fn constant_id_str<'a>(id: &ruby_prism::ConstantId<'a>) -> &'a str {
    std::str::from_utf8(id.as_slice()).expect("prism constant id is UTF-8")
}

pub(super) fn string_value(node: &Node<'_>) -> Option<String> {
    let s = node.as_string_node()?;
    Some(String::from_utf8_lossy(s.unescaped()).into_owned())
}

pub(super) fn symbol_value(node: &Node<'_>) -> Option<String> {
    let s = node.as_symbol_node()?;
    let loc = s.value_loc()?;
    Some(String::from_utf8_lossy(loc.as_slice()).into_owned())
}

pub(super) fn bool_value(node: &Node<'_>) -> Option<bool> {
    if node.as_true_node().is_some() {
        Some(true)
    } else if node.as_false_node().is_some() {
        Some(false)
    } else {
        None
    }
}

pub(super) fn integer_value(node: &Node<'_>) -> Option<i64> {
    let i = node.as_integer_node()?;
    let v: i32 = i.value().try_into().ok()?;
    Some(v as i64)
}

// ---- Class / constant-path navigators ----------------------------------

pub(super) fn find_first_class<'pr>(node: &Node<'pr>) -> Option<ruby_prism::ClassNode<'pr>> {
    if let Some(c) = node.as_class_node() {
        return Some(c);
    }
    if let Some(p) = node.as_program_node() {
        return find_first_class(&p.statements().as_node());
    }
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            if let Some(found) = find_first_class(&stmt) {
                return Some(found);
            }
        }
    }
    if let Some(m) = node.as_module_node() {
        if let Some(body) = m.body() {
            return find_first_class(&body);
        }
    }
    None
}

/// Collect every `class` declaration reachable from `node`, descending
/// through `program`, `statements`, and `module` bodies. Source-order
/// preserved. Used by the library-shape ingest path where one file
/// can declare multiple classes (e.g.
/// `runtime/active_record/errors.rb`).
pub(super) fn find_all_classes<'pr>(node: &Node<'pr>) -> Vec<ruby_prism::ClassNode<'pr>> {
    let mut out = Vec::new();
    collect_classes(node, &[], &mut |_, c| out.push(c));
    out
}

/// Same as `find_all_classes` but pairs each class with its enclosing
/// module path (as written in the source). `module ActiveRecord; class
/// Base` produces `(["ActiveRecord"], ClassNode<Base>)` so callers can
/// build the fully-qualified `ClassId("ActiveRecord::Base")`. Top-
/// level classes have an empty enclosing path.
pub(super) fn find_all_classes_with_scope<'pr>(
    node: &Node<'pr>,
) -> Vec<(Vec<String>, ruby_prism::ClassNode<'pr>)> {
    let mut out = Vec::new();
    collect_classes(node, &[], &mut |scope, c| {
        out.push((scope.to_vec(), c));
    });
    out
}

fn collect_classes<'pr, F: FnMut(&[String], ruby_prism::ClassNode<'pr>)>(
    node: &Node<'pr>,
    scope: &[String],
    out: &mut F,
) {
    if let Some(c) = node.as_class_node() {
        // Save body before moving `c` into the callback; body() borrows
        // `c` but the returned Node is tied to the tree's lifetime, so
        // it outlives the move.
        let body = c.body();
        // Compute the inner scope BEFORE moving `c` — scope for
        // nested classes/modules inside `c`'s body should include
        // `c`'s own name (the bare segment as written in source).
        let mut inner = scope.to_vec();
        if let Some(name_path) = class_name_path(&c) {
            inner.extend(name_path);
        }
        out(scope, c);
        if let Some(b) = body {
            collect_classes(&b, &inner, out);
        }
        return;
    }
    if let Some(p) = node.as_program_node() {
        collect_classes(&p.statements().as_node(), scope, out);
        return;
    }
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            collect_classes(&stmt, scope, out);
        }
        return;
    }
    if let Some(m) = node.as_module_node() {
        // Push the module's name onto the scope before descending —
        // bare class declarations inside qualify with the module
        // path.
        let mut inner = scope.to_vec();
        if let Some(name_path) = module_name_path(&m) {
            inner.extend(name_path);
        }
        if let Some(body) = m.body() {
            collect_classes(&body, &inner, out);
        }
    }
}

pub(super) fn class_name_path(class: &ruby_prism::ClassNode<'_>) -> Option<Vec<String>> {
    let cp = class.constant_path();
    constant_path_segments_strs(&cp)
}

/// Collect every `module` declaration reachable from `node` that has
/// at least one direct `def` in its body. Modules with only nested
/// classes/modules (pure namespace wrappers like `module ActiveRecord`
/// in errors.rb) are skipped — their nested decls are already picked
/// up by `find_all_classes` / recursive `find_all_modules` calls.
///
/// Used by the library-shape ingest path to lower modules-as-
/// namespaces (e.g. `module Inflector with def self.pluralize`) into
/// LibraryClass with no parent. Mixin semantics (modules whose
/// instance methods are `include`d into classes) are not handled
/// here — they need a separate lowering when the include site is
/// known.
pub(super) fn find_all_modules<'pr>(node: &Node<'pr>) -> Vec<ruby_prism::ModuleNode<'pr>> {
    let mut out = Vec::new();
    collect_modules(node, &[], &mut |_, m| out.push(m));
    out
}

/// Same as `find_all_modules` but pairs each module with its enclosing
/// scope path (as written in the source).
pub(super) fn find_all_modules_with_scope<'pr>(
    node: &Node<'pr>,
) -> Vec<(Vec<String>, ruby_prism::ModuleNode<'pr>)> {
    let mut out = Vec::new();
    collect_modules(node, &[], &mut |scope, m| {
        out.push((scope.to_vec(), m));
    });
    out
}

fn collect_modules<'pr, F: FnMut(&[String], ruby_prism::ModuleNode<'pr>)>(
    node: &Node<'pr>,
    scope: &[String],
    out: &mut F,
) {
    if let Some(m) = node.as_module_node() {
        let body = m.body();
        // Compute inner scope before moving `m`.
        let mut inner = scope.to_vec();
        if let Some(name_path) = module_name_path(&m) {
            inner.extend(name_path);
        }
        if module_has_direct_def(&m) {
            out(scope, m);
        }
        if let Some(b) = body {
            collect_modules(&b, &inner, out);
        }
        return;
    }
    if let Some(c) = node.as_class_node() {
        let mut inner = scope.to_vec();
        if let Some(name_path) = class_name_path(&c) {
            inner.extend(name_path);
        }
        if let Some(body) = c.body() {
            collect_modules(&body, &inner, out);
        }
        return;
    }
    if let Some(p) = node.as_program_node() {
        collect_modules(&p.statements().as_node(), scope, out);
        return;
    }
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            collect_modules(&stmt, scope, out);
        }
    }
}

fn module_has_direct_def(m: &ruby_prism::ModuleNode<'_>) -> bool {
    body_has_direct_method_decl(m.body())
}

/// Whether the body has anything that lowers to a method on the
/// enclosing scope: a direct `def`, an `attr_*` call, or a
/// `class << self` block whose body contains the same. Used to decide
/// whether a module is worth surfacing as a `LibraryClass`.
fn body_has_direct_method_decl(body: Option<Node<'_>>) -> bool {
    let Some(body) = body else { return false };
    for stmt in flatten_statements(body) {
        if stmt.as_def_node().is_some() {
            return true;
        }
        if let Some(call) = stmt.as_call_node() {
            if call.receiver().is_none() {
                let kw = constant_id_str(&call.name());
                if matches!(kw, "attr_reader" | "attr_writer" | "attr_accessor") {
                    return true;
                }
            }
        }
        if let Some(sc) = stmt.as_singleton_class_node() {
            if body_has_direct_method_decl(sc.body()) {
                return true;
            }
        }
    }
    false
}

pub(super) fn module_name_path(m: &ruby_prism::ModuleNode<'_>) -> Option<Vec<String>> {
    let cp = m.constant_path();
    constant_path_segments_strs(&cp)
}

pub(super) fn constant_path_of(node: &Node<'_>) -> Option<Vec<String>> {
    constant_path_segments_strs(node)
}

pub(super) fn constant_path_segments_strs(node: &Node<'_>) -> Option<Vec<String>> {
    if let Some(c) = node.as_constant_read_node() {
        return Some(vec![constant_id_str(&c.name()).to_string()]);
    }
    if let Some(p) = node.as_constant_path_node() {
        let mut out = p
            .parent()
            .and_then(|n| constant_path_segments_strs(&n))
            .unwrap_or_default();
        if let Some(id) = p.name() {
            out.push(constant_id_str(&id).to_string());
        }
        return Some(out);
    }
    None
}

pub(super) fn constant_path_segments(p: &ruby_prism::ConstantPathNode<'_>) -> Vec<Symbol> {
    constant_path_segments_strs(&p.as_node())
        .unwrap_or_default()
        .into_iter()
        .map(Symbol::from)
        .collect()
}

// ---- Tree walkers ------------------------------------------------------

pub(super) fn flatten_statements<'pr>(node: Node<'pr>) -> Vec<Node<'pr>> {
    if let Some(s) = node.as_statements_node() {
        s.body().iter().collect()
    } else {
        vec![node]
    }
}

pub(super) fn find_call_named<'pr>(
    node: &Node<'pr>,
    name: &str,
) -> Option<ruby_prism::CallNode<'pr>> {
    if let Some(c) = node.as_call_node() {
        if constant_id_str(&c.name()) == name {
            return Some(c);
        }
        if let Some(recv) = c.receiver() {
            if let Some(found) = find_call_named(&recv, name) {
                return Some(found);
            }
        }
        if let Some(args) = c.arguments() {
            for arg in args.arguments().iter() {
                if let Some(f) = find_call_named(&arg, name) {
                    return Some(f);
                }
            }
        }
        if let Some(block_node) = c.block() {
            if let Some(f) = find_call_named(&block_node, name) {
                return Some(f);
            }
        }
        return None;
    }
    if let Some(p) = node.as_program_node() {
        return find_call_named(&p.statements().as_node(), name);
    }
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            if let Some(f) = find_call_named(&stmt, name) {
                return Some(f);
            }
        }
    }
    if let Some(b) = node.as_block_node() {
        if let Some(body) = b.body() {
            return find_call_named(&body, name);
        }
    }
    None
}

pub(super) fn walk_calls<'pr, F: FnMut(&ruby_prism::CallNode<'pr>)>(node: &Node<'pr>, f: &mut F) {
    if let Some(c) = node.as_call_node() {
        f(&c);
        if let Some(recv) = c.receiver() {
            walk_calls(&recv, f);
        }
        if let Some(args) = c.arguments() {
            for arg in args.arguments().iter() {
                walk_calls(&arg, f);
            }
        }
        if let Some(block_node) = c.block() {
            walk_calls(&block_node, f);
        }
        return;
    }
    if let Some(p) = node.as_program_node() {
        walk_calls(&p.statements().as_node(), f);
        return;
    }
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            walk_calls(&stmt, f);
        }
        return;
    }
    if let Some(b) = node.as_block_node() {
        if let Some(body) = b.body() {
            walk_calls(&body, f);
        }
    }
}

// ---- Symbol-list parsing (shared by controller filters + routes) ------

pub(super) fn symbol_list_value(node: &Node<'_>) -> Vec<Symbol> {
    if let Some(arr) = node.as_array_node() {
        return arr
            .elements()
            .iter()
            .filter_map(|n| symbol_value(&n))
            .map(|s| Symbol::from(s.as_str()))
            .collect();
    }
    if let Some(s) = symbol_value(node) {
        return vec![Symbol::from(s.as_str())];
    }
    vec![]
}

/// Surface form of a symbol list (`[:a, :b]` or `%i[a b]`). For a bare
/// symbol arg (`before_action :foo, only: :show`) we default to Brackets
/// since no array syntax was used — emit falls back on the flat form.
pub(super) fn symbol_list_style(node: &Node<'_>) -> ArrayStyle {
    if let Some(arr) = node.as_array_node() {
        return array_style_from(&arr);
    }
    ArrayStyle::default()
}

/// Detect the surface form of an array literal from its opening token:
/// `%i[` → PercentI, `%w[` → PercentW, else Brackets (with padding
/// detected from the gap between opener and first element).
pub(super) fn array_style_from(arr: &ruby_prism::ArrayNode<'_>) -> ArrayStyle {
    let Some(loc) = arr.opening_loc() else { return ArrayStyle::Brackets };
    let bytes = loc.as_slice();
    if bytes.starts_with(b"%i") || bytes.starts_with(b"%I") {
        return ArrayStyle::PercentI;
    }
    if bytes.starts_with(b"%w") || bytes.starts_with(b"%W") {
        return ArrayStyle::PercentW;
    }
    // Bare brackets — padded `[ x, y ]` vs tight `[x, y]`. Detected by
    // comparing the opening's end-offset to the first element's start.
    // An empty array (`[]`) has no first element; default to tight
    // brackets since padding is only visually meaningful with content.
    if let Some(first) = arr.elements().iter().next() {
        let opener_end = loc.end_offset();
        let first_start = first.location().start_offset();
        if first_start > opener_end {
            return ArrayStyle::BracketsSpaced;
        }
    }
    ArrayStyle::Brackets
}

// ---- Comment / blank-line helpers (model + controller class bodies) ---

/// Collect Prism's inline comments into `(start_offset, text)` pairs.
/// Comments are returned in source order, which matches the order we
/// want to drain them during body ingest. The offset is used for
/// association; the resulting `Comment`'s span stays synthetic for
/// now — real span propagation is a separate effort and including
/// real offsets here would break IR round-trip (positions differ
/// between the original source and the emitter's scratch output).
pub(super) fn collect_comments(result: &ruby_prism::ParseResult<'_>) -> Vec<(usize, Comment)> {
    use ruby_prism::CommentType;
    result
        .comments()
        .filter(|c| c.type_() == CommentType::InlineComment)
        .map(|c| {
            let loc = c.location();
            let text = String::from_utf8_lossy(loc.as_slice())
                .trim_end()
                .to_string();
            (
                loc.start_offset(),
                Comment { text, span: Span::synthetic() },
            )
        })
        .collect()
}

/// Pull every comment whose start is before `offset` off the front of
/// `comments`. Returned in source order so emit produces them in the
/// same sequence they appeared.
pub(super) fn drain_comments_before(
    comments: &mut Vec<(usize, Comment)>,
    offset: usize,
) -> Vec<Comment> {
    let mut out = Vec::new();
    while let Some((start, _)) = comments.first() {
        if *start >= offset {
            break;
        }
        out.push(comments.remove(0).1);
    }
    out
}

/// Is there a blank line in `source[from..to]` — i.e., at least two
/// newlines separated only by whitespace? A single `\n` separates
/// consecutive non-blank lines; `\n<whitespace>\n` means the line
/// between them was blank.
pub(super) fn source_has_blank_line(source: &[u8], from: usize, to: usize) -> bool {
    if from >= to || to > source.len() {
        return false;
    }
    slice_has_blank_line(source, from, to)
}

/// Same check, scoped to an already-sliced byte range (e.g., a Prism
/// `Location::as_slice()`). Kept separate from `source_has_blank_line`
/// because callers that work from a sub-slice don't need the outer
/// bounds check.
pub(super) fn slice_has_blank_line(bytes: &[u8], from: usize, to: usize) -> bool {
    if from >= to || to > bytes.len() {
        return false;
    }
    let slice = &bytes[from..to];
    let mut saw_newline = false;
    for &b in slice {
        match b {
            b'\n' => {
                if saw_newline {
                    return true;
                }
                saw_newline = true;
            }
            b' ' | b'\t' | b'\r' => {}
            _ => saw_newline = false,
        }
    }
    false
}
