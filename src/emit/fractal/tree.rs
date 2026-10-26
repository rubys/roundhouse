//! A sparse overlay of rewritten app files. Emission leaves the base app
//! untouched; applying the overlay replaces only the files it contains.

use std::fs;
use std::path::{Path, PathBuf};

/// Build decisions and output ownership, kept in the overlay only.
pub const MANIFEST: &str = ".roundhouse-fractal.json";

/// What [`write`] did.
pub struct Written {
    pub rewritten: usize,
}

/// Write only rewritten files and the build manifest, never a copy of
/// the app. A rebuild replaces an earlier overlay in `out`.
pub fn write(input: &Path, out: &Path, rewritten: &[(String, String)], manifest: &str) -> Result<Written, String> {
    let input = input.canonicalize().map_err(|e| format!("input {}: {e}", input.display()))?;
    let out_abs = absolute(out)?;
    if input.starts_with(&out_abs) {
        return Err(format!("output {} is the app or contains it", out.display()));
    }
    // Validate before replacing an earlier output or writing any file.
    for (rel, _) in rewritten {
        let path = Path::new(rel);
        if path.as_os_str().is_empty() || !path.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
            return Err(format!("internal: rewritten file {rel} is not a plain relative path"));
        }
        let source = input.join(path).canonicalize().map_err(|e| format!("rewritten file {rel}: {e}"))?;
        if !source.starts_with(&input) || !source.is_file() {
            return Err(format!("internal: rewritten file {rel} is not a file of the app"));
        }
    }
    if out_abs.exists() {
        let empty = fs::read_dir(&out_abs).map_err(|e| format!("read {}: {e}", out.display()))?.next().is_none();
        if !empty && !out_abs.join(MANIFEST).is_file() {
            return Err(format!(
                "output {} is not empty and is not an earlier fractal build (no {MANIFEST}); use an empty directory",
                out.display()
            ));
        }
        fs::remove_dir_all(&out_abs).map_err(|e| format!("remove {}: {e}", out.display()))?;
    }
    fs::create_dir_all(&out_abs).map_err(|e| format!("create {}: {e}", out.display()))?;
    for (rel, text) in rewritten {
        let dest = out_abs.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        fs::write(&dest, text).map_err(|e| format!("write {}: {e}", dest.display()))?;
        let permissions = fs::metadata(input.join(rel)).map_err(|e| format!("stat {rel}: {e}"))?.permissions();
        fs::set_permissions(&dest, permissions).map_err(|e| format!("chmod {}: {e}", dest.display()))?;
    }
    fs::write(out_abs.join(MANIFEST), manifest).map_err(|e| format!("write {MANIFEST}: {e}"))?;
    Ok(Written { rewritten: rewritten.len() })
}

/// Copy only overlay files onto an existing app, leaving other files
/// untouched. Keep the build manifest in the overlay, not in the app.
pub fn apply(overlay: &Path, app: &Path) -> Result<usize, String> {
    for dir in [overlay, app] {
        let meta = fs::symlink_metadata(dir).map_err(|e| format!("stat {}: {e}", dir.display()))?;
        if !meta.is_dir() {
            return Err(format!("{} must be a directory, not a file or symlink", dir.display()));
        }
    }
    let overlay = overlay.canonicalize().map_err(|e| format!("resolve {}: {e}", overlay.display()))?;
    let app = app.canonicalize().map_err(|e| format!("resolve {}: {e}", app.display()))?;
    if app.starts_with(&overlay) {
        return Err("the app must not be the overlay directory or inside it".into());
    }
    let mut files = Vec::new();
    walk(&overlay, &overlay, &mut files)?;
    // Check every path before copying, so a bad entry cannot leave a
    // predictably partial apply. I/O errors can still interrupt copying.
    for rel in &files {
        let dest = app.join(rel);
        if dest.starts_with(&overlay) {
            return Err(format!("destination {} is inside the overlay", dest.display()));
        }
        let mut path = app.clone();
        let mut components = rel.components().peekable();
        while let Some(component) = components.next() {
            path.push(component);
            match fs::symlink_metadata(&path) {
                Ok(meta) => {
                    let expected = if components.peek().is_some() { meta.is_dir() } else { meta.is_file() };
                    if !expected {
                        return Err(format!("destination {} has the wrong type or is a symlink", path.display()));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(format!("stat {}: {e}", path.display())),
            }
        }
    }
    for rel in &files {
        let dest = app.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        fs::copy(overlay.join(rel), &dest).map_err(|e| format!("copy {}: {e}", dest.display()))?;
    }
    Ok(files.len())
}

/// Collect regular overlay files without following links.
fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let path: PathBuf = entry.map_err(|e| format!("read {}: {e}", dir.display()))?.path();
        if dir == root && path.file_name().is_some_and(|name| name == MANIFEST) {
            continue;
        }
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
        if meta.is_dir() {
            walk(root, &path, out)?;
        } else {
            if !meta.is_file() {
                return Err(format!("overlay {} is not a regular file", path.display()));
            }
            let rel = path.strip_prefix(root).map_err(|e| format!("{}: {e}", path.display()))?;
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path.canonicalize().map_err(|e| format!("{}: {e}", path.display()));
    }
    // Canonicalize the deepest existing ancestor and append the rest.
    let mut missing = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if current.as_os_str().is_empty() {
            current = std::env::current_dir().map_err(|e| e.to_string())?;
            break;
        }
        if current.exists() {
            current = current.canonicalize().map_err(|e| format!("{}: {e}", current.display()))?;
            break;
        }
        match (current.file_name(), current.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name.to_os_string());
                current = parent.to_path_buf();
            }
            _ => return Err(format!("output {}: cannot resolve", path.display())),
        }
    }
    for name in missing.into_iter().rev() {
        current.push(name);
    }
    Ok(current)
}

