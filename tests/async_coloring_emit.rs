//! Phase 3 of async coloring (`project_async_coloring_plan.md`).
//!
//! - **Gate 1 (critical):** the `node-sync` profile produces emit
//!   byte-equal to the implicit-default `emit(app)`. Proves the
//!   coloring path is opt-in and the pre-Phase-3 output is preserved.
//! - **Gate 2 (smoke):** the `node-async` profile produces output
//!   that contains `async ` and `await ` somewhere — minimal proof
//!   the propagation + emit path actually fires under an async
//!   profile. Full Gate 2 (real-blog tests against pg/libsql) lives
//!   in the Phase-4 validation work.
//!
//! Each test runs against multiple fixtures so a regression in one
//! lowering surface (controllers, models, views, tests) doesn't hide
//! a regression in another.

use std::path::Path;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::typescript;
use roundhouse::ingest::ingest_app;
use roundhouse::profile::DeploymentProfile;

fn analyzed(fixture: &str) -> roundhouse::App {
    let mut app = ingest_app(Path::new(fixture)).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn assert_byte_equal(
    fixture: &str,
    a: &[roundhouse::emit::EmittedFile],
    b: &[roundhouse::emit::EmittedFile],
) {
    assert_eq!(
        a.len(),
        b.len(),
        "fixture {fixture}: file count mismatch ({} vs {})",
        a.len(),
        b.len()
    );
    for (fa, fb) in a.iter().zip(b.iter()) {
        assert_eq!(
            fa.path, fb.path,
            "fixture {fixture}: file order mismatch"
        );
        assert_eq!(
            fa.content, fb.content,
            "fixture {fixture}: content mismatch in {}\n--- emit() ---\n{}\n--- emit_with_profile(node_sync) ---\n{}",
            fa.path.display(),
            fa.content,
            fb.content,
        );
    }
}

#[test]
fn gate_1_node_sync_byte_equal_to_emit_tiny_blog() {
    let app = analyzed("fixtures/tiny-blog");
    let baseline = typescript::emit(&app);
    let with_profile = typescript::emit_with_profile(&app, &DeploymentProfile::node_sync());
    assert_byte_equal("tiny-blog", &baseline, &with_profile);
}

#[test]
fn gate_1_node_sync_byte_equal_to_emit_real_blog() {
    let app = analyzed("fixtures/real-blog");
    let baseline = typescript::emit(&app);
    let with_profile = typescript::emit_with_profile(&app, &DeploymentProfile::node_sync());
    assert_byte_equal("real-blog", &baseline, &with_profile);
}

#[test]
#[ignore]
fn dump_libsql_runtime_real_blog() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::node_async());
    for f in &files {
        let p = f.path.to_string_lossy();
        if p == "package.json" {
            println!("=== {p} (full) ===");
            for line in f.content.lines() {
                println!("  {line}");
            }
        } else if p == "src/juntos.ts" || p == "src/server.ts" {
            println!("=== {p} (first 8 lines) ===");
            for line in f.content.lines().take(8) {
                println!("  {line}");
            }
        }
    }
}

#[test]
#[ignore]
fn dump_async_lines_real_blog() {
    // Run with: cargo test --test async_coloring_emit dump_async_lines_real_blog -- --ignored --nocapture
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::node_async());
    for f in &files {
        let path_s = f.path.display().to_string();
        if path_s.contains("articles_controller")
            || path_s.contains("active_record/base")
            || path_s.contains("articles.ts")
            || path_s.contains("seeds.ts")
            || path_s.contains("route_helpers.ts")
        {
            println!("// === {path_s} ===");
            for (i, line) in f.content.lines().enumerate() {
                if line.contains("async ") || line.contains("await ") || line.contains("function ") {
                    println!("{:4}: {}", i + 1, line);
                }
            }
        }
    }
}

#[test]
fn gate_2_node_async_emits_async_and_await() {
    // Minimal smoke: under the async profile, the controller layer
    // (which calls AR class methods like `Post.all`) should pick up
    // `async ` on action methods and `await ` at AR Send sites. Full
    // semantic verification is in the Phase-4 toolchain tests against
    // a real async DB driver.
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::node_async());
    let combined: String = files
        .iter()
        .map(|f| f.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        combined.contains("async "),
        "expected `async ` somewhere in node-async output",
    );
    assert!(
        combined.contains("await "),
        "expected `await ` somewhere in node-async output",
    );
    // Async return-type wrapping: every async method's return slot
    // must be `Promise<...>`. Without this, tsc rejects the file —
    // an `async` function declared `: void` is a TypeScript error
    // (TS1064: 'async' return type must be a Promise).
    assert!(
        combined.contains("Promise<void>"),
        "expected `Promise<void>` on async no-return methods in node-async output",
    );
    assert!(
        combined.contains("Promise<Article>"),
        "expected `Promise<Article>` on async fixture/finder methods in node-async output",
    );
    // Negative: no async method should declare a bare non-Promise
    // return. Specifically `async <name>(): void` is the regression
    // we want to catch.
    for line in combined.lines() {
        if line.contains("async ") && line.contains("function ") || line.contains("async ") && line.contains("(") {
            assert!(
                !line.contains("): void {") && !line.contains("): void;"),
                "async method declared with bare `void` return (must be `Promise<void>`):\n{line}",
            );
        }
    }
    // LibraryFunction async emission: the seeds runner calls
    // `Article.create!(...)` which is in the AR adapter surface
    // (seed extern), so propagation must mark the runner async.
    // `export async function run(): Promise<void>` is the canonical
    // shape.
    let seeds = files
        .iter()
        .find(|f| f.path.to_str().is_some_and(|p| p.contains("db/seeds.ts")))
        .expect("seeds file should be emitted for real-blog");
    assert!(
        seeds.content.contains("export async function run(): Promise<void>"),
        "seeds runner should be async (calls AR adapter methods); got:\n{}",
        seeds.content,
    );
    // Route helpers don't call AR — stay sync. This guards against
    // over-marking (a regression in the propagation pass).
    let helpers = files
        .iter()
        .find(|f| f.path.to_str().is_some_and(|p| p.contains("route_helpers.ts")));
    if let Some(helpers) = helpers {
        for line in helpers.content.lines() {
            assert!(
                !line.contains("async function "),
                "route helper functions are pure URL builders; they must not be async: {line}",
            );
        }
    }

    // Profile-aware runtime selection: node-async ships the libsql
    // variants of juntos.ts and server.ts (not better-sqlite3).
    let juntos = files
        .iter()
        .find(|f| f.path.to_str() == Some("src/juntos.ts"))
        .expect("src/juntos.ts should be emitted");
    assert!(
        juntos.content.contains("LibsqlActiveRecordAdapter"),
        "node-async juntos.ts should be the libsql variant"
    );
    assert!(
        juntos.content.contains("@libsql/client"),
        "node-async juntos.ts should import @libsql/client"
    );
    // Negative: no actual import of better-sqlite3. (References
    // in comments are fine — the libsql variant references the
    // sqlite variant in context-explaining commentary.)
    for line in juntos.content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("*") || trimmed.starts_with("/*") {
            continue;
        }
        assert!(
            !line.contains("better-sqlite3"),
            "node-async juntos.ts must not import better-sqlite3 in code: {line}"
        );
    }
    let server = files
        .iter()
        .find(|f| f.path.to_str() == Some("src/server.ts"))
        .expect("src/server.ts should be emitted");
    assert!(
        server.content.contains("createClient"),
        "node-async server.ts should use libsql createClient"
    );
    for line in server.content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("*") || trimmed.starts_with("/*") {
            continue;
        }
        assert!(
            !line.contains("better-sqlite3"),
            "node-async server.ts must not import better-sqlite3 in code: {line}"
        );
    }

    // package.json picks the right DB dependency.
    let pkg = files
        .iter()
        .find(|f| f.path.to_str() == Some("package.json"))
        .expect("package.json should be emitted");
    assert!(
        pkg.content.contains("@libsql/client"),
        "node-async package.json should depend on @libsql/client; got:\n{}",
        pkg.content,
    );
    assert!(
        !pkg.content.contains("better-sqlite3"),
        "node-async package.json must not depend on better-sqlite3"
    );
}
