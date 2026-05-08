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
