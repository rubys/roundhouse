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
    class_name_path, constant_id_str, constant_path_of, find_first_class, flatten_statements,
};
use super::{IngestError, IngestResult};

/// Returns `Ok(None)` if the file doesn't contain a class (e.g. a pure
/// helper or empty file). Unrecognized top-level constructs inside the
/// class are silently skipped — same posture as `ingest_controller`'s
/// `Unknown` fallback but without the surrounding body-item model.
pub fn ingest_test_file(source: &[u8], file: &str) -> IngestResult<Option<TestModule>> {
    let result = parse(source);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(None);
    };

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
        }
    }

    Ok(Some(TestModule {
        name,
        parent,
        target,
        tests,
        setup,
        inner_classes,
        helpers,
    }))
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
