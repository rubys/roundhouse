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
use roundhouse::ingest::ingest_app;

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
// The legacy rust emit path it exercised is gone; rust2 doesn't
// yet cover tiny-blog's specific shape (Importmap LC absent,
// no `<Resource>Params` synthesis, `self.params["id"]` Value→i64
// coerce miss, `Posts::show` view-method-missing). When rust2
// closes those gaps, a fresh tiny-blog smoke test can re-land
// against the rust2 path. Until then, `real_blog_cargo_test_passes`
// + `scripts/compare rust` carry the authoritative coverage.

#[test]
#[ignore]
fn real_blog_cargo_test_passes() {
    // Phase 2b forcing function: emit real-blog, run cargo test
    // against the generated project, assert the non-ignored model
    // tests pass. Two tests are marked #[ignore] because they need
    // persistence runtime (Phase 3); the rest should pass cleanly.
    let fixture = Path::new("fixtures/real-blog");
    let scratch = scratch_dir("real-blog");
    generate_project(fixture, &scratch);

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
/// real, silent bug: rust2 cloned the `iter_mut()` receiver of the
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
    let fixture = Path::new("fixtures/real-blog");
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
