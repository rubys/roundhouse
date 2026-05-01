//! Sanity check: `ingest_app_from_tree` on a HashMap loaded from disk
//! produces the same emit output as `ingest_app` on the same disk path.
//! Validates the `MapVfs` implementation against the canonical `FsVfs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::analyze::Analyzer;
use roundhouse::emit::typescript;
use roundhouse::ingest::{ingest_app, ingest_app_from_tree};

fn load_tree_from_disk(root: &Path) -> HashMap<PathBuf, Vec<u8>> {
    let mut out = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let bytes = std::fs::read(&path).expect("read");
                let rel = path.strip_prefix(root).expect("strip").to_path_buf();
                out.insert(rel, bytes);
            }
        }
    }
    out
}

#[test]
fn fs_and_map_vfs_emit_identical_typescript() {
    let fixture = Path::new("fixtures/real-blog");

    let mut fs_app = ingest_app(fixture).expect("fs ingest");
    Analyzer::new(&fs_app).analyze(&mut fs_app);
    let fs_emit = typescript::emit(&fs_app);

    let tree = load_tree_from_disk(fixture);
    let mut map_app = ingest_app_from_tree(tree).expect("map ingest");
    Analyzer::new(&map_app).analyze(&mut map_app);
    let map_emit = typescript::emit(&map_app);

    assert_eq!(fs_emit.len(), map_emit.len(), "file count mismatch");
    for (fs_file, map_file) in fs_emit.iter().zip(map_emit.iter()) {
        assert_eq!(fs_file.path, map_file.path, "path mismatch");
        assert_eq!(
            fs_file.content, map_file.content,
            "content mismatch for {}",
            fs_file.path.display()
        );
    }
}
