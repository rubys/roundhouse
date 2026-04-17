//! Stricter forcing function: emitted Ruby must equal the fixture source byte-for-byte.
//!
//! The round-trip identity test `ingest → emit → ingest → IR equal` has a
//! blind spot: if the ingester silently drops a construct, the emitter has
//! nothing to emit, the second ingest drops it again, and both IRs agree
//! trivially. This test closes that hole by comparing the emitter's output
//! to the original source. If the pipeline loses information, the diff here
//! will show which file drifted.
//!
//! Failures in this test are almost always ingest gaps (a Rails construct
//! the ingester doesn't yet recognize). Add the recognizer, don't relax the
//! test.

use std::path::{Path, PathBuf};

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app;

fn fixture_path() -> &'static Path {
    Path::new("fixtures/tiny-blog")
}

#[test]
fn emitted_ruby_equals_fixture_source_byte_for_byte() {
    let app = ingest_app(fixture_path()).expect("ingest");
    let emitted = ruby::emit(&app);

    for file in &emitted {
        let fixture_file: PathBuf = fixture_path().join(&file.path);
        let source = std::fs::read_to_string(&fixture_file)
            .unwrap_or_else(|e| panic!("read {}: {e}", fixture_file.display()));
        assert_eq!(
            file.content, source,
            "emitted {} differs from fixture source.\n--- emitted ---\n{}\n--- source ---\n{}",
            file.path.display(),
            file.content,
            source
        );
    }
}

#[test]
fn every_fixture_rb_file_is_accounted_for() {
    // Catch the reverse failure: the fixture has a file the emitter didn't produce.
    // This would mean a whole file type (mailers, jobs, helpers, ...) is being ignored.
    let app = ingest_app(fixture_path()).expect("ingest");
    let emitted: std::collections::HashSet<PathBuf> =
        ruby::emit(&app).into_iter().map(|f| f.path).collect();

    let mut missing = Vec::new();
    walk_rb_files(fixture_path(), fixture_path(), &mut |rel| {
        if !emitted.contains(rel) {
            missing.push(rel.to_path_buf());
        }
    });
    assert!(missing.is_empty(), "emitter did not produce: {missing:?}");
}

fn walk_rb_files(root: &Path, dir: &Path, f: &mut impl FnMut(&Path)) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.is_dir() {
            walk_rb_files(root, &path, f);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rb") {
            let rel = path.strip_prefix(root).expect("strip_prefix");
            f(rel);
        }
    }
}
