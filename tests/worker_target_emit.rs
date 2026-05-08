//! Phase 2 Stage B: SharedWorker browser target — runtime file
//! selection.
//!
//! - The `worker` profile selects the worker variants of juntos.ts
//!   and server.ts (not the libsql variants), and brings three
//!   additional runtime files into the emit output: `src/client.ts`,
//!   `src/db_worker.ts`, `src/sqlite_wasm_engine.ts`.
//! - The `node_async` and `node_sync` profiles do NOT include those
//!   three files — they're worker-only.
//! - The juntos.ts / server.ts content under the worker profile
//!   matches the bytes of `runtime/typescript/juntos-worker.ts` and
//!   `runtime/typescript/server-worker.ts` respectively (this guards
//!   the picker dispatch from silently regressing to an async-only
//!   selection that misses the worker shim).

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

fn find<'a>(
    files: &'a [roundhouse::emit::EmittedFile],
    rel: &str,
) -> Option<&'a roundhouse::emit::EmittedFile> {
    files.iter().find(|f| f.path.to_string_lossy() == rel)
}

#[test]
fn worker_profile_emits_three_extra_runtime_files() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::worker());

    assert!(
        find(&files, "src/client.ts").is_some(),
        "worker profile should emit src/client.ts"
    );
    assert!(
        find(&files, "src/db_worker.ts").is_some(),
        "worker profile should emit src/db_worker.ts"
    );
    assert!(
        find(&files, "src/sqlite_wasm_engine.ts").is_some(),
        "worker profile should emit src/sqlite_wasm_engine.ts"
    );
}

#[test]
fn node_profiles_do_not_emit_worker_runtime_files() {
    let app = analyzed("fixtures/real-blog");

    let async_files = typescript::emit_with_profile(&app, &DeploymentProfile::node_async());
    assert!(find(&async_files, "src/client.ts").is_none());
    assert!(find(&async_files, "src/db_worker.ts").is_none());
    assert!(find(&async_files, "src/sqlite_wasm_engine.ts").is_none());

    let sync_files = typescript::emit_with_profile(&app, &DeploymentProfile::node_sync());
    assert!(find(&sync_files, "src/client.ts").is_none());
    assert!(find(&sync_files, "src/db_worker.ts").is_none());
    assert!(find(&sync_files, "src/sqlite_wasm_engine.ts").is_none());
}

#[test]
fn worker_profile_picks_juntos_worker_not_libsql() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::worker());

    let juntos = find(&files, "src/juntos.ts").expect("src/juntos.ts must be emitted");

    // The worker variant's first comment line names the variant.
    // (A negative assertion against "libsql variant" elsewhere
    // would be unreliable — the worker file legitimately mentions
    // libsql in prose comments. Byte-equality with the on-disk
    // source is the strong assertion; this test is a quick header
    // check.)
    let first_line = juntos.content.lines().next().unwrap_or("");
    assert!(
        first_line.contains("SharedWorker variant"),
        "worker profile picked juntos.ts whose first line is `{first_line}` — expected SharedWorker variant"
    );
}

#[test]
fn worker_profile_picks_server_worker_not_libsql() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::worker());

    let server = find(&files, "src/server.ts").expect("src/server.ts must be emitted");

    let first_line = server.content.lines().next().unwrap_or("");
    assert!(
        first_line.contains("SharedWorker variant"),
        "worker profile picked server.ts whose first line is `{first_line}` — expected SharedWorker variant"
    );
}

#[test]
fn worker_profile_juntos_bytes_match_runtime_source() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::worker());

    let emitted = find(&files, "src/juntos.ts")
        .expect("src/juntos.ts must be emitted")
        .content
        .as_str();
    let on_disk = include_str!("../runtime/typescript/juntos-worker.ts");

    assert_eq!(
        emitted, on_disk,
        "emitted src/juntos.ts under worker profile should byte-match \
         runtime/typescript/juntos-worker.ts"
    );
}

#[test]
fn worker_profile_server_bytes_match_runtime_source() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::worker());

    let emitted = find(&files, "src/server.ts")
        .expect("src/server.ts must be emitted")
        .content
        .as_str();
    let on_disk = include_str!("../runtime/typescript/server-worker.ts");

    assert_eq!(
        emitted, on_disk,
        "emitted src/server.ts under worker profile should byte-match \
         runtime/typescript/server-worker.ts"
    );
}

// ── Stage C: entry-point + ecosystem files ───────────────────────────

#[test]
fn worker_profile_emits_three_entry_points() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::worker());

    // Three Vite entries: main.ts (loads client.ts), worker.ts
    // (loads server-worker.ts), and src/db_worker.ts (already
    // emitted as a runtime file in Stage B). The dedicated DB
    // Worker uses src/db_worker.ts directly as its rollup input —
    // no separate project-root stub.
    let main = find(&files, "main.ts").expect("main.ts");
    assert!(
        main.content.contains("startClient"),
        "worker main.ts should call startClient: {}",
        main.content,
    );
    assert!(
        main.content.contains("@hotwired/turbo"),
        "worker main.ts should import @hotwired/turbo for Drive navigation",
    );
    assert!(
        !main.content.contains("startServer"),
        "worker main.ts must NOT call startServer (node target)",
    );

    let worker = find(&files, "worker.ts").expect("worker.ts");
    assert!(
        worker.content.contains("startApplication"),
        "worker.ts should call startApplication",
    );
    assert!(
        worker.content.contains("./src/server.js"),
        "worker.ts should import startApplication from ./src/server.js",
    );

    // src/db_worker.ts already covered by Stage B test above.
}

#[test]
fn worker_profile_emits_index_html_with_meta_placeholders() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::worker());

    let html = find(&files, "index.html").expect("index.html");
    assert!(
        html.content.contains("<meta name=\"juntos-worker\""),
        "index.html should contain juntos-worker meta tag (manifest plugin rewrites it)",
    );
    assert!(
        html.content.contains("<meta name=\"juntos-db-worker\""),
        "index.html should contain juntos-db-worker meta tag",
    );
    assert!(
        html.content.contains("/main.ts"),
        "index.html should load /main.ts as a module entry",
    );
}

#[test]
fn worker_profile_emits_vite_config_with_three_inputs() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::worker());

    let vite = find(&files, "vite.config.ts").expect("vite.config.ts");
    assert!(
        vite.content.contains("input: {"),
        "vite.config.ts should have rollupOptions.input",
    );
    assert!(
        vite.content.contains("main: resolve(\"index.html\")"),
        "vite.config.ts should declare main entry (index.html)",
    );
    assert!(
        vite.content.contains("worker: resolve(\"worker.ts\")"),
        "vite.config.ts should declare worker entry (worker.ts)",
    );
    assert!(
        vite.content.contains("db_worker: resolve(\"src/db_worker.ts\")"),
        "vite.config.ts should declare db_worker entry (src/db_worker.ts)",
    );
    assert!(
        vite.content.contains("manifest: true"),
        "vite.config.ts should enable build.manifest",
    );
    assert!(
        vite.content.contains("manifestMetaInjection"),
        "vite.config.ts should include the manifest-meta-injection plugin",
    );
}

#[test]
fn worker_profile_package_json_uses_vite_not_tsx() {
    let app = analyzed("fixtures/real-blog");
    let files = typescript::emit_with_profile(&app, &DeploymentProfile::worker());

    let pkg = find(&files, "package.json").expect("package.json");
    assert!(
        pkg.content.contains("\"vite\""),
        "worker package.json should depend on vite",
    );
    assert!(
        pkg.content.contains("@hotwired/turbo"),
        "worker package.json should depend on @hotwired/turbo",
    );
    assert!(
        pkg.content.contains("@sqlite.org/sqlite-wasm"),
        "worker package.json should depend on @sqlite.org/sqlite-wasm",
    );
    assert!(
        !pkg.content.contains("better-sqlite3"),
        "worker package.json must NOT include better-sqlite3 (node-only)",
    );
    assert!(
        !pkg.content.contains("@libsql/client"),
        "worker package.json must NOT include @libsql/client (node-only)",
    );
    assert!(
        !pkg.content.contains("\"tsx\""),
        "worker package.json must NOT include tsx (node-only runtime)",
    );
    assert!(
        pkg.content.contains("\"dev\": \"vite\""),
        "worker package.json should expose npm run dev → vite",
    );
    assert!(
        pkg.content.contains("\"build\": \"vite build\""),
        "worker package.json should expose npm run build → vite build",
    );
}

#[test]
fn node_profiles_do_not_emit_worker_ecosystem_files() {
    let app = analyzed("fixtures/real-blog");
    let async_files = typescript::emit_with_profile(&app, &DeploymentProfile::node_async());

    assert!(find(&async_files, "worker.ts").is_none());
    assert!(find(&async_files, "index.html").is_none());
    assert!(find(&async_files, "vite.config.ts").is_none());

    // Sanity: node profile still produces main.ts (the existing one,
    // calling startServer, not startClient).
    let main = find(&async_files, "main.ts").expect("node main.ts");
    assert!(main.content.contains("startServer"));
    assert!(!main.content.contains("startClient"));
}
