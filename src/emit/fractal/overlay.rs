//! The output: an overlay of the files a build generates, to be placed on
//! the app's root. A fractal build never edits an app file, so nothing
//! else belongs in it, and an app the size of a monolith is not copied.

use std::fs;
use std::path::{Path, PathBuf};

use super::variant::INITIALIZER;

/// Every path a fractal build can write, relative to the app root. A
/// rebuild replaces these and nothing else.
pub const OWNED: &[&str] = &[INITIALIZER];

/// Write `generated` under `out`. `out` may be absent, empty, or hold
/// only files an earlier build wrote, which are removed first: a rebuild
/// that no longer generates a file (a class deselected) must not leave
/// the old one behind. Anything else in `out` is refused, so `-o` can
/// never point at the app itself.
pub fn write(out: &Path, generated: &[(String, String)]) -> Result<usize, String> {
    if let Some((path, _)) = generated.iter().find(|(path, _)| !OWNED.contains(&path.as_str())) {
        return Err(format!("internal: {path} is not a path a fractal build owns"));
    }
    if out.exists() {
        let mut found = Vec::new();
        walk(out, out, &mut found)?;
        let foreign: Vec<&String> = found.iter().filter(|rel| !OWNED.contains(&rel.as_str())).collect();
        if let Some(first) = foreign.first() {
            return Err(format!(
                "output {} holds {} file(s) a fractal build did not write ({first}, ...); use an empty directory or an earlier fractal output",
                out.display(),
                foreign.len()
            ));
        }
        for rel in &found {
            let path = out.join(rel);
            fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
            remove_empty_parents(out, &path);
        }
    }
    fs::create_dir_all(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    for (rel, content) in generated {
        let dest = out.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        fs::write(&dest, content).map_err(|e| format!("write {}: {e}", dest.display()))?;
    }
    Ok(generated.len())
}

/// Every file and symlink under `dir`, relative to `root`.
fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let path: PathBuf = entry.map_err(|e| format!("read {}: {e}", dir.display()))?.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
        if meta.is_dir() {
            walk(root, &path, out)?;
        } else {
            let rel = path.strip_prefix(root).map_err(|e| format!("{}: {e}", path.display()))?;
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Remove the directories a removed file leaves empty, up to `root`.
fn remove_empty_parents(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}
