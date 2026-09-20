//! Rails test-file ingestion — `test/models/*_test.rb` and
//! `test/controllers/*_test.rb`. Expects a single top-level class
//! (typically inheriting from `ActiveSupport::TestCase` or
//! `ActionDispatch::IntegrationTest`) whose body is a sequence of
//! `test "name" do ... end` declarations.

use ruby_prism::Node;

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
    Ok(ingest_test_files(source, file)?.into_iter().next())
}

/// Every test class the file declares, each as its own `TestModule`.
///
/// A Rails test file is usually one class, but not always:
/// campfire's `test/channels/room_messages_channel_test.rb` declares
/// `RoomMessagesChannelTest` and, beside it, `RoomMessagesViaStock-
/// TurboChannelTest` — the second exists to prove the STOCK
/// `Turbo::StreamsChannel` turns the same stream names away, which is
/// a different subject and so a different class. `ingest_test_file`
/// kept only the first and routed the second through
/// `inner_classes`, where a `*Test` class was then skipped on
/// purpose (see the note at the end of `ingest_test_class`): two
/// tests silently absent from every tally. Each class here becomes
/// a module of its own, and the emit names files by class, so they
/// land as two test files the way Minitest would have run them as
/// two suites. Non-test top-level classes are shared by every module
/// as helpers, the way they were for the one.
pub fn ingest_test_files(source: &[u8], file: &str) -> IngestResult<Vec<TestModule>> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let mut top_classes: Vec<ruby_prism::ClassNode<'_>> = Vec::new();
    collect_top_level_classes(&root, &mut top_classes);
    if top_classes.is_empty() {
        return Ok(Vec::new());
    }
    let (mut test_nodes, helper_nodes): (Vec<_>, Vec<_>) =
        top_classes.into_iter().partition(is_test_class_node);
    // No candidate matches: the historical single-class shape, where
    // the first class is the test whatever it is called.
    if test_nodes.is_empty() {
        let mut helper_nodes = helper_nodes;
        test_nodes.push(helper_nodes.remove(0));
        return ingest_test_class(&test_nodes[0], &helper_nodes, file).map(|tm| vec![tm]);
    }
    test_nodes
        .iter()
        .map(|class| ingest_test_class(class, &helper_nodes, file))
        .collect()
}

fn ingest_test_class(
    class: &ruby_prism::ClassNode<'_>,
    top_level_helper_nodes: &[ruby_prism::ClassNode<'_>],
    file: &str,
) -> IngestResult<TestModule> {
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
    let mut target = name_path
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
                // `tests RoomMessagesChannel` — ActionCable's channel
                // and connection test cases (and ActionController::
                // TestCase) name their subject with this macro when
                // the class name does not: `RoomMessagesViaStock-
                // TurboChannelTest` tests `Turbo::StreamsChannel`. It
                // overrides the `FooTest` → `Foo` convention above.
                if call.receiver().is_none() && constant_id_str(&call.name()) == "tests" {
                    if let Some(args) = call.arguments() {
                        if let Some(arg) = args.arguments().iter().next() {
                            if let Some(path) = constant_path_of(&arg) {
                                target = Some(ClassId(Symbol::from(path.join("::"))));
                            }
                        }
                    }
                    continue;
                }
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
    // Additional `*Test`-shaped classes are each a module of their
    // own — `ingest_test_files` splits them before this runs — so
    // every node here is a genuine helper. (They used to be dropped:
    // an inner class does not get the `< Minitest::Test → < TestBase`
    // parent rewrite the test class does, so emitting one inline
    // would have Minitest find it under CRuby and run it against the
    // wrong assertion surface.)
    for helper_node in top_level_helper_nodes {
        let lc = library_class_from_node(helper_node, file)?;
        inner_classes.push(lc);
    }

    Ok(TestModule {
        name,
        parent,
        target,
        tests,
        setup,
        inner_classes,
        helpers,
        constants,
        includes,
    })
}

/// Collect every direct top-level class declaration. Does NOT recurse
/// into modules — Rails test files declare their `*Test` class at the
/// file's top scope; helper models live alongside at the same scope.
/// Nested module declarations are not a shape we encounter for tests.
/// The app-wide `setup do … end` an app's `test/test_helper.rb` puts on
/// `ActiveSupport::TestCase` (reopened there, or nested as `module
/// ActiveSupport; class TestCase`). Rails runs it before every test
/// case's own setup, so the caller splices it ahead of each test
/// module's `setup` — the same move `splice_test_helpers` makes for the
/// helper modules that file includes.
///
/// campfire's is three statements — clear the cable test adapter,
/// replace the web-push pool, forbid network — and the middle one is
/// why this exists: `Room::PushTest` waits on the pool's task counter,
/// which is cumulative, so every test needs the fresh pool the app's
/// own setup gives it. Without the splice the suite's first push test
/// leaves a count the next one's wait is satisfied by before its tasks
/// have run.
///
/// Only the `setup` hook is read. `fixtures :all`, `parallelize`, and
/// the `include` list are the harness's concern (the includes are read
/// by `ingest_test_helper_modules`), and a `teardown` there has no
/// home yet — the emitted `TestBase#teardown` is ours.
pub fn ingest_test_case_setup(source: &[u8], file: &str) -> IngestResult<Option<Expr>> {
    let result = super::prism::parse(source, file);
    let root = result.node();
    let mut classes: Vec<ruby_prism::ClassNode<'_>> = Vec::new();
    collect_top_level_classes(&root, &mut classes);
    // Nested spelling: descend one module level.
    if let Some(p) = root.as_program_node() {
        for stmt in p.statements().body().iter() {
            if let Some(m) = stmt.as_module_node() {
                if let Some(body) = m.body() {
                    collect_top_level_classes(&body, &mut classes);
                }
            }
        }
    }
    for class in classes {
        let Some(path) = class_name_path(&class) else { continue };
        if path.last().map(String::as_str) != Some("TestCase") {
            continue;
        }
        let Some(body) = class.body() else { continue };
        for stmt in flatten_statements(body) {
            if let Some(setup) = ingest_setup_declaration(&stmt, file)? {
                return Ok(Some(setup));
            }
        }
    }
    Ok(None)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    // campfire's `test/channels/room_messages_channel_test.rb`: two
    // `< ActionCable::Channel::TestCase` classes in one file, the
    // second naming its subject with `tests Turbo::StreamsChannel`.
    // Before `ingest_test_files`, the second class was demoted to a
    // helper and then skipped for being a test — two tests in no
    // tally.
    const TWO_CLASSES: &str = r#"
require "test_helper"

class RoomMessagesChannelTest < ActionCable::Channel::TestCase
  tests RoomMessagesChannel

  setup do
    @room = rooms(:designers)
  end

  test "a member may subscribe" do
    assert true
  end

  test "an outsider may not" do
    assert true
  end
end

class RoomMessagesViaStockTurboChannelTest < ActionCable::Channel::TestCase
  tests Turbo::StreamsChannel

  test "the stock channel refuses a room message stream" do
    assert true
  end
end
"#;

    #[test]
    fn every_test_class_in_a_file_is_its_own_module() {
        let tms = ingest_test_files(TWO_CLASSES.as_bytes(), "room_messages_channel_test.rb")
            .expect("ingest");
        let names: Vec<&str> = tms.iter().map(|tm| tm.name.0.as_str()).collect();
        assert_eq!(
            names,
            ["RoomMessagesChannelTest", "RoomMessagesViaStockTurboChannelTest"]
        );
        assert_eq!(tms[0].tests.len(), 2);
        assert_eq!(tms[1].tests.len(), 1);
        // Neither is the other's helper.
        assert!(tms.iter().all(|tm| tm.inner_classes.is_empty()));
        // The first module still carries its own setup.
        assert!(tms[0].setup.is_some());
        assert!(tms[1].setup.is_none());
    }

    #[test]
    fn tests_macro_names_the_subject() {
        let tms = ingest_test_files(TWO_CLASSES.as_bytes(), "room_messages_channel_test.rb")
            .expect("ingest");
        let targets: Vec<&str> = tms
            .iter()
            .map(|tm| tm.target.as_ref().map(|t| t.0.as_str()).unwrap_or(""))
            .collect();
        assert_eq!(targets, ["RoomMessagesChannel", "Turbo::StreamsChannel"]);
    }

    #[test]
    fn single_class_wrapper_still_answers_the_first() {
        let tm = ingest_test_file(TWO_CLASSES.as_bytes(), "room_messages_channel_test.rb")
            .expect("ingest")
            .expect("a test class");
        assert_eq!(tm.name.0.as_str(), "RoomMessagesChannelTest");
    }
}
