//! Rails test-file ingestion — `test/models/*_test.rb` and
//! `test/controllers/*_test.rb`. Expects a single top-level class
//! (typically inheriting from `ActiveSupport::TestCase` or
//! `ActionDispatch::IntegrationTest`) whose body is a sequence of
//! `test "name" do ... end` declarations.

use ruby_prism::{Node, parse};

use crate::dialect::{Test, TestModule};
use crate::expr::{Expr, ExprNode};
use crate::span::Span;
use crate::{ClassId, Symbol};

use super::expr::ingest_expr;
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
    if let Some(class_body) = class.body() {
        for stmt in flatten_statements(class_body) {
            if let Some(test) = ingest_test_declaration(&stmt, file)? {
                tests.push(test);
            }
        }
    }

    Ok(Some(TestModule { name, parent, target, tests }))
}

/// Recognize a single `test "name" do ... end` call. Returns `None` for
/// any other statement shape (intentional — we silently drop unrecognized
/// body items rather than error; they'll surface when the test tries to
/// reference something that isn't emitted).
fn ingest_test_declaration(
    stmt: &Node<'_>,
    file: &str,
) -> IngestResult<Option<Test>> {
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
