//! Rails test-file ingestion — `test/models/*_test.rb` and
//! `test/controllers/*_test.rb`. Expects a single top-level class
//! (typically inheriting from `ActiveSupport::TestCase` or
//! `ActionDispatch::IntegrationTest`) whose body is a sequence of
//! `test "name" do ... end` declarations.

use ruby_prism::{Node, parse};

use crate::dialect::{MethodDef, Test, TestModule};
use crate::expr::{Expr, ExprNode};
use crate::span::Span;
use crate::{ClassId, Symbol};

use super::expr::ingest_expr;
use super::library_class::{ingest_library_method, library_class_from_node};
use super::util::{
    class_name_path, constant_id_str, constant_path_of, flatten_statements,
};
use super::{IngestError, IngestResult};

/// Returns `Ok(None)` if the file doesn't contain a class (e.g. a pure
/// helper or empty file). Unrecognized top-level constructs inside the
/// class are silently skipped — same posture as `ingest_controller`'s
/// `Unknown` fallback but without the surrounding body-item model.
///
/// When the file declares multiple top-level classes — typically a
/// helper model paired with a `*Test` class (see
/// `runtime/ruby/test/action_view/view_helpers_test.rb`'s `Article` +
/// `ViewHelpersTest` pair) — pick the `*Test` class as the test
/// module and ingest the rest as helpers on the returned module's
/// `inner_classes`. This keeps the emit path generic: every per-target
/// emitter already hoists `inner_classes` above the test body, so a
/// top-level helper class lands alongside the test file without
/// per-target plumbing changes.
pub fn ingest_test_file(source: &[u8], file: &str) -> IngestResult<Option<TestModule>> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = parse(source);
    let root = result.node();
    let mut top_classes: Vec<ruby_prism::ClassNode<'_>> = Vec::new();
    collect_top_level_classes(&root, &mut top_classes);
    if top_classes.is_empty() {
        return Ok(None);
    }
    // Pick the test class by heuristic; everything else is a helper.
    // Fall back to first class if no candidate matches (preserves the
    // historical single-class shape).
    let test_idx = top_classes
        .iter()
        .position(is_test_class_node)
        .unwrap_or(0);
    let class = top_classes.remove(test_idx);
    let top_level_helper_nodes = top_classes;

    let name_path = class_name_path(&class).ok_or_else(|| IngestError::Unsupported {
        file: file.into(),
        message: "test class name must be a simple constant or path".into(),
    })?;
    let name = ClassId(Symbol::from(name_path.join("::")));

    let parent = class.superclass().and_then(|n| {
        constant_path_of(&n).map(|p| ClassId(Symbol::from(p.join("::"))))
    });

    // Rails convention: `ArticleTest` tests `Article`. Strip the `Test`
    // suffix off the last name-path segment. Unusual naming → None.
    let target = name_path
        .last()
        .and_then(|last| last.strip_suffix("Test"))
        .map(|stem| ClassId(Symbol::from(stem)));

    let mut tests: Vec<Test> = Vec::new();
    let mut setup: Option<Expr> = None;
    let mut inner_classes: Vec<crate::dialect::LibraryClass> = Vec::new();
    let mut helpers: Vec<MethodDef> = Vec::new();
    let mut constants: Vec<(Symbol, Expr)> = Vec::new();
    let mut includes: Vec<ClassId> = Vec::new();
    if let Some(class_body) = class.body() {
        for stmt in flatten_statements(class_body) {
            if let Some(test) = ingest_test_declaration(&stmt, file)? {
                tests.push(test);
                continue;
            }
            if let Some(body) = ingest_setup_declaration(&stmt, file)? {
                // Multiple setup hooks compose in source order — append
                // by wrapping in a Seq. Rare in practice; first wins
                // when stored as Option, so accumulate.
                setup = Some(match setup.take() {
                    None => body,
                    Some(prev) => {
                        let prev_stmts = match &*prev.node {
                            ExprNode::Seq { exprs } => exprs.clone(),
                            _ => vec![prev],
                        };
                        let body_stmts = match &*body.node {
                            ExprNode::Seq { exprs } => exprs.clone(),
                            _ => vec![body],
                        };
                        let mut all = prev_stmts;
                        all.extend(body_stmts);
                        Expr::new(Span::synthetic(), ExprNode::Seq { exprs: all })
                    }
                });
                continue;
            }
            // Inline class declaration inside the test class —
            // `class Validatable; include ActiveRecord::Validations;
            // ...; end`. Captured per-test-module so the emit can
            // hoist them to file scope above the lowered test class.
            if let Some(inner) = stmt.as_class_node() {
                let lc = library_class_from_node(&inner, file)?;
                inner_classes.push(lc);
                continue;
            }
            // Non-test, non-setup `def` — instance helper method
            // (e.g. `setup_adapter_with_stub_row(id)`). Capture as
            // an ordinary method on the test class so test bodies
            // can `self.<helper>(...)` it. Definitions of `def
            // setup` and `def test_*` already short-circuited above.
            if let Some(def) = stmt.as_def_node() {
                let m = ingest_library_method(&def, &name, file)?;
                helpers.push(m);
                continue;
            }
            // `include ModName` at class-body scope. Captured for
            // the spinel emit to replay verbatim so bare-name refs
            // (`Router`, `FormBuilder`) resolve under CRuby. TS emit
            // ignores these — its framework-namespace import-stripper
            // handles the same refs by a different mechanism.
            if let Some(call) = stmt.as_call_node() {
                if call.receiver().is_none()
                    && constant_id_str(&call.name()) == "include"
                {
                    if let Some(args) = call.arguments() {
                        for arg in args.arguments().iter() {
                            if let Some(path) = constant_path_of(&arg) {
                                includes.push(ClassId(Symbol::from(path.join("::"))));
                            }
                        }
                    }
                    continue;
                }
            }
            // Class-level constant assignment — `TABLE = [...]`,
            // `Article = Struct.new(...)`. Captured here so the test
            // emit can hoist them to file scope as `const NAME =
            // <value>`, which matches Ruby's lexical constant
            // resolution for bare references inside test methods.
            if let Some(cw) = stmt.as_constant_write_node() {
                let const_name = std::str::from_utf8(cw.name().as_slice())
                    .ok()
                    .map(Symbol::from);
                if let Some(const_name) = const_name {
                    let value = ingest_expr(&cw.value(), file)?;
                    constants.push((const_name, value));
                }
                continue;
            }
        }
    }

    // Top-level helper classes (e.g. `class Article < ActiveRecord::
    // Base` declared next to `ViewHelpersTest` in the same file) ingest
    // as LibraryClass and flow through the same `inner_classes` channel
    // as body-internal helpers. The per-target emit paths already hoist
    // `inner_classes` to file scope above the test body — same routing,
    // no further per-target changes needed.
    //
    // Additional `*Test`-shaped classes (a single file with multiple
    // `< Minitest::Test` classes, as in
    // `runtime/ruby/test/active_record/errors_test.rb` declaring
    // `RecordNotFoundTest` + `RecordInvalidTest`) are NOT added as
    // helpers — inner_classes don't get the `< Minitest::Test → <
    // TestBase` parent rewrite the main test class does, so emitting
    // them inline would have Minitest find them as Test subclasses
    // under CRuby and run them against the wrong assertion surface.
    // The right fix is to emit each as its own test file; that
    // requires plumbing a `Vec<TestModule>` return from this function
    // and is tracked separately. For now they fall through and stay
    // silently dropped (same behavior as before this commit).
    for helper_node in &top_level_helper_nodes {
        if is_test_class_node(helper_node) {
            continue;
        }
        let lc = library_class_from_node(helper_node, file)?;
        inner_classes.push(lc);
    }

    Ok(Some(TestModule {
        name,
        parent,
        target,
        tests,
        setup,
        inner_classes,
        helpers,
        constants,
        includes,
    }))
}

/// Collect every direct top-level class declaration. Does NOT recurse
/// into modules — Rails test files declare their `*Test` class at the
/// file's top scope; helper models live alongside at the same scope.
/// Nested module declarations are not a shape we encounter for tests.
fn collect_top_level_classes<'pr>(
    node: &Node<'pr>,
    out: &mut Vec<ruby_prism::ClassNode<'pr>>,
) {
    if let Some(c) = node.as_class_node() {
        out.push(c);
        return;
    }
    if let Some(p) = node.as_program_node() {
        collect_top_level_classes(&p.statements().as_node(), out);
        return;
    }
    if let Some(s) = node.as_statements_node() {
        for stmt in s.body().iter() {
            collect_top_level_classes(&stmt, out);
        }
    }
}

/// Heuristic: a class is "the test class" when its name ends with
/// `Test` (e.g. `ViewHelpersTest`, `InflectorTest`) OR when its parent
/// path's leaf segment is a known test base (`Test`, `TestCase`,
/// `IntegrationTest`). The two conditions together cover both the
/// framework's own `< Minitest::Test` shape and Rails app tests'
/// `< ActiveSupport::TestCase` / `< ActionDispatch::IntegrationTest`.
fn is_test_class_node(c: &ruby_prism::ClassNode<'_>) -> bool {
    if let Some(name_path) = class_name_path(c) {
        if let Some(last) = name_path.last() {
            if last.ends_with("Test") {
                return true;
            }
        }
    }
    if let Some(parent_node) = c.superclass() {
        if let Some(parent_path) = constant_path_of(&parent_node) {
            if let Some(last) = parent_path.last() {
                if matches!(last.as_str(), "Test" | "TestCase" | "IntegrationTest") {
                    return true;
                }
            }
        }
    }
    false
}

/// Recognize `setup do ... end` (Call with method=setup, block body)
/// and `def setup; ...; end` (DefNode named setup). Returns the body
/// expression in either case. Other shapes return Ok(None).
fn ingest_setup_declaration(
    stmt: &Node<'_>,
    file: &str,
) -> IngestResult<Option<Expr>> {
    // `setup do ... end` form.
    if let Some(call) = stmt.as_call_node() {
        if call.receiver().is_some() {
            return Ok(None);
        }
        if constant_id_str(&call.name()) != "setup" {
            return Ok(None);
        }
        if let Some(block_ref) = call.block() {
            if let Some(block) = block_ref.as_block_node() {
                let body = match block.body() {
                    Some(body_node) => ingest_expr(&body_node, file)?,
                    None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
                };
                return Ok(Some(body));
            }
        }
        return Ok(None);
    }
    // `def setup; ...; end` form.
    if let Some(def) = stmt.as_def_node() {
        let name_bytes = def.name().as_slice();
        if std::str::from_utf8(name_bytes).ok() != Some("setup") {
            return Ok(None);
        }
        let body = match def.body() {
            Some(body_node) => ingest_expr(&body_node, file)?,
            None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
        };
        return Ok(Some(body));
    }
    Ok(None)
}

/// Recognize a single test declaration. Two shapes are supported:
///
///   `test "name" do ... end`  — Rails AS::TestCase DSL. Test name is the
///                                string argument verbatim.
///   `def test_*; ...; end`    — vanilla Minitest. Test name is the
///                                method name with the `test_` prefix
///                                stripped and underscores replaced with
///                                spaces (mirrors how Minitest reports
///                                them; keeps the two shapes
///                                indistinguishable downstream).
///
/// Returns `None` for any other statement shape (intentional — we
/// silently drop unrecognized body items rather than error; they'll
/// surface when the test tries to reference something that isn't
/// emitted).
fn ingest_test_declaration(
    stmt: &Node<'_>,
    file: &str,
) -> IngestResult<Option<Test>> {
    // `def test_*; ...; end` form (vanilla Minitest).
    if let Some(def) = stmt.as_def_node() {
        let name_bytes = def.name().as_slice();
        let Some(method_name) = std::str::from_utf8(name_bytes).ok() else {
            return Ok(None);
        };
        let Some(stem) = method_name.strip_prefix("test_") else {
            return Ok(None);
        };
        let name = stem.replace('_', " ");
        let body = match def.body() {
            Some(body_node) => ingest_expr(&body_node, file)?,
            None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
        };
        return Ok(Some(Test { name, body }));
    }

    let Some(call) = stmt.as_call_node() else {
        return Ok(None);
    };
    // Must be a bare-name `test(...)` call.
    if call.receiver().is_some() {
        return Ok(None);
    }
    let method = constant_id_str(&call.name()).to_string();
    if method != "test" {
        return Ok(None);
    }

    // First positional argument: the test's name as a string literal.
    let Some(args) = call.arguments() else {
        return Ok(None);
    };
    let args_iter = args.arguments();
    let Some(first_arg) = args_iter.iter().next() else {
        return Ok(None);
    };
    let Some(string_node) = first_arg.as_string_node() else {
        return Ok(None);
    };
    let name = String::from_utf8_lossy(string_node.unescaped()).into_owned();

    // The block body — Prism exposes it via `call.block()` when present.
    let body = match call.block() {
        Some(block_ref) => match block_ref.as_block_node() {
            Some(block) => match block.body() {
                Some(body_node) => ingest_expr(&body_node, file)?,
                None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            },
            None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
        },
        None => Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
    };

    Ok(Some(Test { name, body }))
}
