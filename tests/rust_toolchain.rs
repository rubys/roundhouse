//! Rust toolchain integration test — the Phase 1 forcing function.
//!
//! Generates the emitted Rust project for a fixture into a scratch
//! directory and runs `cargo check` against it. "Zero errors" means
//! the emitter's output is syntactically valid Rust that the compiler
//! accepts at the structural level — not yet that it runs, just that
//! it compiles.
//!
//! Scoped to `tiny-blog` for Phase 1. Controllers are emitted as files
//! but not declared in `src/lib.rs` (they reference runtime the
//! generated code doesn't have yet). When Phase 2 lands, the scope
//! extends to real-blog + `cargo test` on the model tests.
//!
//! Marked `#[ignore]` so the default `cargo test` run stays fast —
//! this test shells out to cargo itself and is slow (multi-second) on
//! a cold scratch dir. Run explicitly with:
//!
//!     cargo test --test rust_toolchain -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::rust;
use roundhouse::ingest::{ingest_app, ingest_app_from_tree};

fn scratch_dir(fixture: &str) -> PathBuf {
    std::env::temp_dir().join(format!("roundhouse-rust-check-{fixture}"))
}

fn generate_project(fixture_path: &Path, out: &Path) {
    if out.exists() {
        std::fs::remove_dir_all(out).expect("clean scratch");
    }
    std::fs::create_dir_all(out).expect("create scratch");

    let mut app = ingest_app(fixture_path).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = rust::emit(&app);

    for file in &files {
        let path = out.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&path, &file.content).expect("write emitted file");
    }
}

// `tiny_blog_cargo_check_passes` retired in Phase 7.3 (2026-05-20).
// The legacy rust emit path it exercised is gone; rust doesn't
// yet cover tiny-blog's specific shape (Importmap LC absent,
// no `<Resource>Params` synthesis, `self.params["id"]` Value→i64
// coerce miss, `Posts::show` view-method-missing). When rust
// closes those gaps, a fresh tiny-blog smoke test can re-land
// against the rust path. Until then, `real_blog_cargo_test_passes`
// + `scripts/compare rust` carry the authoritative coverage.

/// The emitted Rust must retain the Rails instance-method shape for both
/// ActionController::Base defaults and a concrete real-blog controller.
#[test]
fn real_blog_controller_identity_methods_emit_as_instance_methods() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-controller-identity-emission");
    generate_project(fixture, &scratch);

    for (path, class_name) in [
        (scratch.join("src/action_controller_base.rs"), "ActionController::Base"),
        (scratch.join("src/controllers/articles_controller.rs"), "ArticlesController"),
    ] {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for method in ["controller_name", "controller_path"] {
            let signature = format!("pub fn {method}(&self) -> String");
            assert!(
                source.contains(&signature),
                "{class_name} should emit instance method `{signature}`:\n{source}"
            );
        }
    }
}

#[test]
fn inherited_before_action_calls_dispatch_on_self() {
    let files = [
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  before_action :require_authentication\n  before_action :deny_bots\n  before_action :allow_browser\n\n  private\n\n  def require_authentication\n    set_version_headers\n    other.foreign_helper\n  end\n\n  def set_version_headers\n  end\n\n  def deny_bots\n  end\n\n  def allow_browser\n  end\n\n  def foreign_helper\n  end\n\n  def unused_parent_helper\n  end\nend\n",
        ),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    other.foreign_helper\n  end\n\n  private\n\n  def allow_browser\n    @child_allow_browser = \"child-allow-browser\"\n  end\nend\n",
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let source = rust::emit(&app)
        .into_iter()
        .find(|file| file.path.ends_with("widgets_controller.rs"))
        .expect("WidgetsController Rust output")
        .content;

    for method in ["require_authentication", "deny_bots", "set_version_headers"] {
        let call = format!("self.{method}()");
        assert!(
            source.contains(&call),
            "inherited filter must self-dispatch as `{call}`:\n{source}"
        );
        let definition = format!("fn {method}(");
        assert!(
            source.contains(&definition),
            "reachable inherited method `{method}` must be defined on the child:\n{source}"
        );
    }
    assert!(
        source.contains("self.allow_browser()"),
        "the inherited filter must still dispatch to the child override:\n{source}"
    );
    let allow_browser = source
        .find("fn allow_browser(")
        .map(|start| {
            let body = &source[start..];
            body.find("\n}")
                .map(|end| &body[..end])
                .unwrap_or(body)
        })
        .unwrap_or("");
    assert!(
        allow_browser.contains("child_allow_browser")
            && allow_browser.contains("child-allow-browser"),
        "the child override body must be the emitted definition:\n{source}"
    );
    assert!(
        !source.contains("unused_parent_helper"),
        "unreferenced ancestor methods must not be copied:\n{source}"
    );
    assert!(
        source.contains("foreign_helper") && !source.contains("fn foreign_helper("),
        "a same-named method called only on another object must stay a call, not a copy:\n{source}"
    );
    assert_eq!(
        source.matches("fn allow_browser(").count(),
        1,
        "child override must replace the inherited definition, not duplicate it:\n{source}"
    );
}

#[test]
fn router_only_references_emitted_controller_handlers() {
    let tree = [
        (
            "app/controllers/reports_controller.rb",
            "class ReportsController < ActionController::Base\n  def index\n  end\nend\n",
        ),
        (
            "app/controllers/hidden_controller.rb",
            "class HiddenController < ActionController::Base\n  private\n  def index\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#index\"\n  get \"/hidden\", to: \"hidden#index\"\n  get \"/rooms/settings\", to: \"rooms/settings#show\"\n  get \"/up\", to: \"rails/health#show\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let files = rust::emit(&app);
    let router = files
        .iter()
        .find(|file| file.path.ends_with("router.rs"))
        .expect("Rust router output")
        .content
        .clone();

    assert!(router.contains(".route(\"/reports\""), "real controller route disappeared:\n{router}");
    assert!(router.contains("reports_controller::_axum_index"), "real handler missing:\n{router}");
    for (missing, path) in [
        ("hidden_controller", "/hidden"),
        ("rooms::settings_controller", "/rooms/settings"),
        ("rails::health_controller", "/up"),
    ] {
        assert!(!router.contains(missing), "router references non-emitted handler `{missing}`:\n{router}");
        assert!(router.contains(&format!(".route(\"{path}\"")), "route disappeared instead of remaining explicit:\n{router}");
    }
    assert!(router.contains("_roundhouse_unsupported_route"), "missing handlers must not be treated as implemented:\n{router}");
    assert!(router.contains("StatusCode::NOT_IMPLEMENTED"), "unsupported routes must fail explicitly:\n{router}");
    assert!(
        router.contains("request_context_middleware"),
        "direct router users need an active request scope:\n{router}"
    );
    let hidden = files
        .iter()
        .find(|file| file.path.ends_with("hidden_controller.rs"))
        .expect("hidden controller output");
    assert!(
        !hidden.content.contains("pub async fn _axum_index"),
        "a controller with no dispatcher must not emit a route wrapper:\n{}",
        hidden.content
    );
}

/// Execute the generated identity methods in the native Rust toolchain lane.
///
/// Kept ignored for the default suite because it shells out to Cargo; CI selects
/// this focused regression from the Rust compare job. The full-app cargo test
/// below remains a separate, broader local gate.
#[test]
#[ignore]
fn real_blog_controller_identity_values_match_rails() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-controller-identity-values");
    generate_project(fixture, &scratch);

    std::fs::create_dir_all(scratch.join("tests")).unwrap();
    std::fs::write(
        scratch.join("tests/controller_identity.rs"),
        r#"
use app::action_controller_base::Base;
use app::controllers::ArticlesController;

/// Verifies the generated controllers return their concrete Rails identities.
#[test]
fn controller_identity_values_match_rails() {
    let base = Base::default();
    assert_eq!(base.controller_name(), "base");
    assert_eq!(base.controller_path(), "action_controller/base");

    let controller = ArticlesController::default();
    assert_eq!(controller.controller_name(), "articles");
    assert_eq!(controller.controller_path(), "articles");
}
"#,
    )
    .unwrap();

    let output = Command::new("cargo")
        .args([
            "test",
            "--test",
            "controller_identity",
            "--",
            "--exact",
            "controller_identity_values_match_rails",
        ])
        .current_dir(&scratch)
        .output()
        .expect("run focused cargo test");

    assert!(
        output.status.success(),
        "focused cargo test failed on emitted real-blog project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("test controller_identity_values_match_rails ... ok"),
        "the emitted controller identity value test did not run:\n{stdout}"
    );
}

/// Compile the complete generated application and execute its model/runtime
/// contracts with the actual Cargo dependency graph and packaged imports.
#[test]
#[ignore]
fn real_blog_cargo_test_passes() {
    // Phase 2b forcing function: emit real-blog, run cargo test
    // against the generated project, assert the non-ignored model
    // tests pass. Two tests are marked #[ignore] because they need
    // persistence runtime (Phase 3); the rest should pass cleanly.
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);
    // Pin the shared Inflector's new String seam on the live backend,
    // not the legacy emit_method extraction walker.
    std::fs::create_dir_all(scratch.join("tests")).unwrap();
    std::fs::write(
        scratch.join("tests/formatted_pluralize.rs"),
        r#"
use app::inflector::Inflector;
#[test]
fn formatted_count_labels() {
    for (count, expected) in [
        ("1", "1 word"), ("1.0", "1.0 word"), ("1.00", "1.00 word"),
        ("01", "01 words"), ("1.", "1. words"), ("10.", "10. words"),
        ("10.0", "10.0 words"), ("1.01", "1.01 words"),
        ("1.0002", "1.0002 words"), ("1,001", "1,001 words"), ("", " words"),
        ("2\n1.0\n", "2\n1.0\n word"), ("1\r\n", "1\r\n words"),
        ("1.٠", "1.٠ words"),
    ] {
        assert_eq!(Inflector::pluralize_formatted(count, "word"), expected);
    }
}
"#,
    )
    .unwrap();
    // `i += 1` loop counters in transpiled runtime bodies: the rust
    // emitter used to drop the `+=` statement, so `enum_label` spun
    // forever on any value past the first label. A hang here is the
    // regression.
    std::fs::write(
        scratch.join("tests/op_assign_counter.rs"),
        r#"
use app::active_record_base::ActiveRecord;
#[test]
fn enum_label_walks_past_the_first_label() {
    let labels = vec!["draft".to_string(), "published".to_string(), "archived".to_string()];
    assert_eq!(ActiveRecord::enum_label(2, labels.clone(), vec![0, 1, 2]), Some("archived".to_string()));
    assert_eq!(ActiveRecord::enum_label(7, labels, vec![0, 1, 2]), None);
}
"#,
    )
    .unwrap();

    // Exercise complete runtime packaging and typed byte reads through the
    // generated Cargo project, including the real shared error implementation.
    std::fs::write(
        scratch.join("tests/route_path_captures.rs"),
        r#"
use app::router::Router;
#[test]
fn routed_captures_and_checked_bytes() {
    for (input, expected) in [("abc", "abc"), ("+%2B", "++"), ("%00", "\0"), ("%2500", "%00"), ("%C3%A9", "é")] {
        let path = format!("/echo/{input}");
        let hit = Router::match_pattern("/echo/:value", &path, "").expect("route");
        assert_eq!(hit["value"], expected);
    }
    assert_eq!(Router::capture_byte(vec![0, 255], 0), 0);
    assert_eq!(Router::capture_byte(vec![0, 255], 1), 255);
    for index in [-1, 1] {
        let error = std::panic::catch_unwind(|| Router::capture_byte(vec![0], index)).expect_err("invalid offset must reject");
        assert_eq!(error.downcast_ref::<String>().map(String::as_str), Some("FrameworkError::Argument"));
    }
    let error = std::panic::catch_unwind(|| Router::decode_capture("%FF")).expect_err("invalid UTF-8 must reject");
    assert_eq!(error.downcast_ref::<String>().map(String::as_str), Some("FrameworkError::Argument"));
}
"#,
    ).unwrap();

    let output = Command::new("cargo")
        .arg("test")
        .arg("--quiet")
        .current_dir(&scratch)
        .output()
        .expect("run cargo test");

    assert!(
        output.status.success(),
        "cargo test failed on emitted real-blog project at {}:\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Query-count regression gate for the `includes(:comments)` eager-load
/// (roundhouse#40, #27).
///
/// `compare` and the emitted controller tests assert byte-identical
/// HTML, and are *structurally blind* to this N+1: eager-load and 1+N
/// render the same bytes — only the query strategy differs. So a query
/// counter is the only instrument that catches a regression. This was a
/// real, silent bug: rust cloned the `iter_mut()` receiver of the
/// `_preload_comments` distribute loop, so the writes hit a throwaway
/// temporary and `/articles` ran 1+N instead of 2 — undetectable by
/// every existing gate.
///
/// Mirrors spinel's `runtime/spinel/test/query_count_test.rb`. Emits
/// real-blog, injects a `tests/` integration test into the generated
/// crate that drives `GET /articles` through `axum-test` and reads the
/// SQL the new `Db::capture_sql_*` thread-local funnel recorded, then
/// runs it with `cargo test`. Asserts exactly 2 queries (parent SELECT
/// + one batched comments preload) and no per-article `WHERE article_id
/// = N` lazy filter.
///
/// `#[ignore]` like its siblings — shells out to cargo, slow on a cold
/// scratch dir. Run with:
///
///     cargo test --test rust_toolchain -- --ignored --nocapture
#[test]
#[ignore]
fn real_blog_articles_index_is_two_queries() {
    let fixture = roundhouse::fixtures::real_blog();
    let scratch = scratch_dir("real-blog-query-count");
    generate_project(fixture, &scratch);

    // Integration test injected into the generated crate. Lives in
    // `tests/` (not `src/tests/`) so it needs no edit to the crate's
    // module tree — cargo compiles every `tests/*.rs` against the
    // crate's public API as its own binary. That binary links the
    // crate compiled WITHOUT `cfg(test)`, so the `#[cfg(test)]`
    // `fixtures` module is invisible here; seed via the public
    // `db::setup_test_db` + `Db::exec` surface instead. Two articles
    // make the N+1 visible (lazy = 1 + 2; eager = 2).
    let gate = r#"//! Injected by tests/rust_toolchain.rs — query-count gate (roundhouse#40, #27).
use app::db::{self, Db};
use app::{router, schema_sql};

#[tokio::test(flavor = "multi_thread")]
async fn articles_index_is_two_queries_not_n_plus_one() {
    // Fresh per-thread :memory: DB + seed. The handler (axum-test mock
    // transport) polls inline on this thread, so it shares this CONN.
    db::setup_test_db(schema_sql::CREATE_TABLES);
    Db::exec("INSERT INTO articles (title, body, created_at, updated_at) VALUES ('First', 'b1', '2024-01-01', '2024-01-01')");
    Db::exec("INSERT INTO articles (title, body, created_at, updated_at) VALUES ('Second', 'b2', '2024-01-02', '2024-01-02')");
    Db::exec("INSERT INTO comments (article_id, body, commenter, created_at, updated_at) VALUES (1, 'c1', 'me', '2024-01-01', '2024-01-01')");
    Db::exec("INSERT INTO comments (article_id, body, commenter, created_at, updated_at) VALUES (2, 'c2', 'me', '2024-01-02', '2024-01-02')");

    let server = axum_test::TestServer::new(router::router()).unwrap();

    Db::capture_sql_start();
    let resp = server.get("/articles").await;
    let sql = Db::capture_sql_take();

    assert_eq!(resp.status_code(), 200, "GET /articles did not return 200");

    // A per-article equality filter means the lazy accessor fired —
    // the N+1 regression. The eager path batches with `IN (...)`.
    let per_article: Vec<&String> = sql
        .iter()
        .filter(|q| q.contains("FROM comments WHERE article_id = "))
        .collect();
    assert!(
        per_article.is_empty(),
        "N+1 regression: per-article comment queries fired:\n{}\n\nfull SQL log:\n{}",
        per_article.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"),
        sql.join("\n"),
    );

    // Eager path = parent SELECT + one batched comments preload,
    // regardless of how many articles the fixture seeds.
    assert_eq!(
        sql.len(),
        2,
        "expected 2 queries (articles + batched comments IN), got {}:\n{}",
        sql.len(),
        sql.join("\n"),
    );
}
"#;
    let gate_path = scratch.join("tests").join("query_count_gate.rs");
    std::fs::create_dir_all(gate_path.parent().unwrap()).expect("mkdir tests/");
    std::fs::write(&gate_path, gate).expect("write injected gate test");

    let output = Command::new("cargo")
        .arg("test")
        .arg("--test")
        .arg("query_count_gate")
        .arg("--")
        .arg("--nocapture")
        .current_dir(&scratch)
        .output()
        .expect("run cargo test on query-count gate");

    assert!(
        output.status.success(),
        "query-count gate failed on emitted real-blog at {} \
         (eager-load N+1 regression — see roundhouse#40):\n\
         \n=== stdout ===\n{}\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// A block filter and a lambda filter read `action_name`. The dispatcher
/// then calls `assign_action_name`, and the emitted Rust controller must
/// have that method and the `action_name` reader.
#[test]
#[ignore]
fn filters_that_read_action_name_compile() {
    let app_dir = scratch_dir("action-name-app");
    if app_dir.exists() {
        std::fs::remove_dir_all(&app_dir).expect("clean app copy");
    }
    let copied = Command::new("cp")
        .arg("-R")
        .arg(roundhouse::fixtures::real_blog())
        .arg(&app_dir)
        .status()
        .expect("copy real-blog");
    assert!(copied.success(), "copy real-blog");
    let controller = app_dir.join("app/controllers/articles_controller.rb");
    let source = std::fs::read_to_string(&controller).expect("read controller");
    let edited = source.replacen(
        "  before_action :set_article,",
        "  before_action { @bare = action_name }\n  \
           before_action -> { @own = self.action_name }\n  \
           before_action :set_article,",
        1,
    );
    assert_ne!(source, edited, "the filter edit applies");
    std::fs::write(&controller, edited).expect("write controller");

    let scratch = scratch_dir("action-name");
    generate_project(&app_dir, &scratch);
    let output = Command::new("cargo")
        .arg("check")
        .arg("--quiet")
        .current_dir(&scratch)
        .output()
        .expect("run cargo check");

    assert!(
        output.status.success(),
        "cargo check failed on the emitted project at {}:\n\
         \n=== stderr ===\n{}",
        scratch.display(),
        String::from_utf8_lossy(&output.stderr),
    );
}
