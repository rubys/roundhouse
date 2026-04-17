//! Ruby → IR → Ruby → IR identity.
//!
//! The forcing function for IR completeness. If ingesting the fixture produces
//! an `App`, and emitting that `App` produces Ruby files, and re-ingesting
//! those files produces the same `App`, then the IR captured everything the
//! emitter needed and the emitter produced everything the ingester recognized.
//!
//! Any divergence either means the IR is lossy (emit dropped information) or
//! the recognizers are out of sync (emit used a form the ingester doesn't
//! accept yet). Both are real bugs. Extend this test — don't relax it — when
//! new features are added.

use std::path::{Path, PathBuf};

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app;

fn fixture_path() -> &'static Path {
    Path::new("fixtures/tiny-blog")
}

fn scratch_root() -> PathBuf {
    // Prefer CARGO_TARGET_TMPDIR (set at compile time by `cargo test` for integration
    // tests); fall back to the system temp dir if it's not available.
    let base = option_env!("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("roundhouse").join("round_trip_identity")
}

fn write_emitted(dir: &Path, app: &roundhouse::App) {
    if dir.exists() {
        std::fs::remove_dir_all(dir).expect("clean scratch dir");
    }
    std::fs::create_dir_all(dir).expect("create scratch dir");
    for file in ruby::emit(app) {
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir -p");
        }
        std::fs::write(&path, &file.content).expect("write");
    }
}

#[test]
fn tiny_blog_ir_is_fixed_under_emit_ingest() {
    let original = ingest_app(fixture_path()).expect("ingest original");

    let scratch = scratch_root();
    write_emitted(&scratch, &original);

    let roundtripped = ingest_app(&scratch).expect("ingest re-emitted");

    assert_eq!(
        original, roundtripped,
        "IR diverged across Ruby emit + re-ingest"
    );
}

#[test]
fn roundtrip_is_idempotent() {
    // Second pass through the pipeline should be a no-op.
    let original = ingest_app(fixture_path()).expect("ingest original");

    let scratch = scratch_root().with_file_name("round_trip_identity_idempotent");
    write_emitted(&scratch, &original);
    let first = ingest_app(&scratch).expect("ingest first emission");

    write_emitted(&scratch, &first);
    let second = ingest_app(&scratch).expect("ingest second emission");

    assert_eq!(first, second, "second round-trip must be a no-op");
}
