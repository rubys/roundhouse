//! Regression test for RBS emission of test-file inner classes
//! (matz/spinel#1255).
//!
//! Framework test files declare lightweight record stand-ins — e.g.
//! `class Article < ActiveRecord::Base` in
//! `action_view/view_helpers_test.rb` — whose bodies are spliced into
//! the emitted `.rb` test file. Spinel's `--rbs sig` consumer needs a
//! signature for these or it infers their methods itself; the classic
//! failure is `def [](field)` returning `@id` (Integer) on one branch
//! and `@title`/`@body` (String) on others — spinel commits to one C
//! type instead of boxing the heterogeneous return.
//!
//! Three lowering/emit gaps had to close for the sidecar to carry real
//! types: (1) `emit_spinel` emits an `.rbs` for inner classes at all;
//! (2) inner-class methods get a synthesized signature (ingest leaves
//! it `None`); (3) the inner class's ivar types are inferred (from
//! `initialize` + `self.id =`) so the bodies type concretely. This
//! test pins all three by asserting the emitted shape — no spinel
//! binary required.

use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_test_file;
use roundhouse::App;

fn emit_view_helpers_spinel() -> Vec<roundhouse::emit::EmittedFile> {
    let test_file = Path::new("runtime/ruby/test/action_view/view_helpers_test.rb");
    let source = std::fs::read(test_file).expect("read view_helpers_test.rb");
    let test_module = ingest_test_file(&source, &test_file.display().to_string())
        .expect("ingest")
        .expect("test class present");
    let mut app = App::new();
    app.test_modules.push(test_module);
    Analyzer::new(&app).analyze(&mut app);
    roundhouse::emit::ruby::emit_spinel(&app)
}

/// The inner `Article` stand-in gets its own `.rbs` sidecar. `.rbs`
/// from `emit_library_class_rbs` is rooted under `sig/`.
#[test]
fn inner_class_gets_rbs_sidecar() {
    let files = emit_view_helpers_spinel();
    let rbs = files
        .iter()
        .find(|f| f.path == PathBuf::from("sig/test/article_inner.rbs"))
        .unwrap_or_else(|| {
            panic!(
                "no inner-class RBS sidecar emitted; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        });
    assert!(
        rbs.content.contains("class Article < ActiveRecord::Base"),
        "inner RBS missing class header:\n{}",
        rbs.content
    );
}

/// Gap 1+2+3 together: `def [](field)` returns `Integer | String`, the
/// union that forces spinel to box (matz/spinel#1255). The dedup means
/// the two String branches collapse — `(Integer | String)`, not
/// `(Integer | String | String)`.
#[test]
fn index_method_returns_heterogeneous_union() {
    let files = emit_view_helpers_spinel();
    let rbs = files
        .iter()
        .find(|f| f.path == PathBuf::from("sig/test/article_inner.rbs"))
        .expect("inner RBS sidecar");
    assert!(
        rbs.content.contains("def []: (untyped field) -> (Integer | String)"),
        "[] should box a heterogeneous Integer|String return:\n{}",
        rbs.content
    );
}

/// Ivar inference (gap 3) flows into accessor + constructor signatures:
/// `@title`/`@body` are String (from `initialize`), and the optional
/// constructor params keep their default-derived types.
#[test]
fn inferred_ivar_types_reach_signatures() {
    let files = emit_view_helpers_spinel();
    let rbs = files
        .iter()
        .find(|f| f.path == PathBuf::from("sig/test/article_inner.rbs"))
        .expect("inner RBS sidecar");
    assert!(
        rbs.content.contains("(?Integer id, ?String title, ?String body) -> nil"),
        "initialize should carry default-derived optional params + nil return:\n{}",
        rbs.content
    );
    assert!(
        rbs.content.contains("title: String") && rbs.content.contains("body: String"),
        "title/body accessors should be typed String:\n{}",
        rbs.content
    );
}

/// An inner class's keyword parameter takes what the tests pass for it,
/// not only its default's type: campfire's push-service
/// `Server#hung_up?(within: 5)` is called `hung_up?(within: 0.1)`, and
/// an `Integer` from the default alone made spinel refuse the Float. The
/// helper is carried into every test that includes it and its sidecar is
/// one file by class name, so a module that never makes the call
/// declares the same type.
#[test]
fn an_inner_keyword_takes_the_literal_types_its_tests_pass() {
    let server = "  class Server\n    def hung_up?(within: 5)\n      within > 0\n    end\n  end\n";
    let calling = format!(
        "class CallingTest < ActiveSupport::TestCase\n{server}\n  test \"waits briefly\" do\n    assert Server.new.hung_up?(within: 0.1)\n  end\nend\n"
    );
    let quiet = format!(
        "class QuietTest < ActiveSupport::TestCase\n{server}\n  test \"waits\" do\n    assert Server.new.hung_up?\n  end\nend\n"
    );
    let mut app = App::new();
    for (src, path) in [(&calling, "test/calling_test.rb"), (&quiet, "test/quiet_test.rb")] {
        app.test_modules.push(
            ingest_test_file(src.as_bytes(), path).expect("ingest").expect("test class present"),
        );
    }
    Analyzer::new(&app).analyze(&mut app);
    let files = roundhouse::emit::ruby::emit_spinel(&app);
    let sidecars: Vec<_> = files
        .iter()
        .filter(|f| f.path.to_string_lossy().ends_with("server_inner.rbs"))
        .collect();
    assert!(!sidecars.is_empty(), "no Server sidecar: {:?}", files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>());
    for rbs in sidecars {
        // A keyword here, or the optional positional ingest flattens an
        // inline stand-in's keyword to; the type is the claim.
        assert!(
            rbs.content.contains("def hung_up?: (?within: (Integer | Float)) -> bool")
                || rbs.content.contains("def hung_up?: (?(Integer | Float) within) -> bool"),
            "{}:\n{}",
            rbs.path.display(),
            rbs.content
        );
    }
}
