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
/// the app. A rebuild replaces an earlier overlay in that directory.
pub fn write(input: &Path, overlay: &Path, rewritten: &[(String, String)], manifest: &str) -> Result<Written, String> {
    let input = input.canonicalize().map_err(|error| format!("input {}: {error}", input.display()))?;
    let overlay_absolute = absolute(overlay)?;
    if input.starts_with(&overlay_absolute) {
        return Err(format!("output {} is the app or contains it", overlay.display()));
    }
    // Validate before replacing an earlier output or writing any file.
    for (relative_path, _) in rewritten {
        let path = Path::new(relative_path);
        if path.as_os_str().is_empty() || !path.components().all(|component| matches!(component, std::path::Component::Normal(_))) {
            return Err(format!("internal: rewritten file {relative_path} is not a plain relative path"));
        }
        let source = input.join(path).canonicalize().map_err(|error| format!("rewritten file {relative_path}: {error}"))?;
        if !source.starts_with(&input) || !source.is_file() {
            return Err(format!("internal: rewritten file {relative_path} is not a file of the app"));
        }
    }
    if overlay_absolute.exists() {
        let empty = fs::read_dir(&overlay_absolute).map_err(|error| format!("read {}: {error}", overlay.display()))?.next().is_none();
        if !empty && !overlay_absolute.join(MANIFEST).is_file() {
            return Err(format!(
                "output {} is not empty and is not an earlier fractal build (no {MANIFEST}); use an empty directory",
                overlay.display()
            ));
        }
        fs::remove_dir_all(&overlay_absolute).map_err(|error| format!("remove {}: {error}", overlay.display()))?;
    }
    fs::create_dir_all(&overlay_absolute).map_err(|error| format!("create {}: {error}", overlay.display()))?;
    for (relative_path, text) in rewritten {
        let destination = overlay_absolute.join(relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
        }
        fs::write(&destination, text).map_err(|error| format!("write {}: {error}", destination.display()))?;
        let permissions = fs::metadata(input.join(relative_path)).map_err(|error| format!("stat {relative_path}: {error}"))?.permissions();
        fs::set_permissions(&destination, permissions).map_err(|error| format!("chmod {}: {error}", destination.display()))?;
    }
    fs::write(overlay_absolute.join(MANIFEST), manifest).map_err(|error| format!("write {MANIFEST}: {error}"))?;
    Ok(Written { rewritten: rewritten.len() })
}

/// Copy only overlay files onto an existing app, leaving other files
/// untouched. Keep the build manifest in the overlay, not in the app.
pub fn apply(overlay: &Path, app: &Path) -> Result<usize, String> {
    for directory in [overlay, app] {
        let metadata = fs::symlink_metadata(directory).map_err(|error| format!("stat {}: {error}", directory.display()))?;
        if !metadata.is_dir() {
            return Err(format!("{} must be a directory, not a file or symlink", directory.display()));
        }
    }
    let overlay = overlay.canonicalize().map_err(|error| format!("resolve {}: {error}", overlay.display()))?;
    let app = app.canonicalize().map_err(|error| format!("resolve {}: {error}", app.display()))?;
    if app.starts_with(&overlay) {
        return Err("the app must not be the overlay directory or inside it".into());
    }
    let mut files = Vec::new();
    walk(&overlay, &overlay, &mut files)?;
    // Check every path before copying, so a bad entry cannot leave a
    // predictably partial apply. I/O errors can still interrupt copying.
    for relative_path in &files {
        let destination = app.join(relative_path);
        if destination.starts_with(&overlay) {
            return Err(format!("destination {} is inside the overlay", destination.display()));
        }
        let mut path = app.clone();
        let mut components = relative_path.components().peekable();
        while let Some(component) = components.next() {
            path.push(component);
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    let expected = if components.peek().is_some() { metadata.is_dir() } else { metadata.is_file() };
                    if !expected {
                        return Err(format!("destination {} has the wrong type or is a symlink", path.display()));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(format!("stat {}: {error}", path.display())),
            }
        }
    }
    for relative_path in &files {
        let destination = app.join(relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
        }
        fs::copy(overlay.join(relative_path), &destination).map_err(|error| format!("copy {}: {error}", destination.display()))?;
    }
    Ok(files.len())
}

/// The files [`apply`] would copy from `overlay`, relative to it: every
/// regular file but the manifest. A symlink or other special entry is an
/// error, as it is for `apply`.
pub(crate) fn overlay_files(overlay: &Path) -> Result<Vec<PathBuf>, String> {
    let overlay = overlay.canonicalize().map_err(|error| format!("resolve {}: {error}", overlay.display()))?;
    let mut files = Vec::new();
    walk(&overlay, &overlay, &mut files)?;
    files.sort();
    Ok(files)
}

/// Collect regular overlay files without following links.
fn walk(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| format!("read {}: {error}", directory.display()))? {
        let path: PathBuf = entry.map_err(|error| format!("read {}: {error}", directory.display()))?.path();
        if directory == root && path.file_name().is_some_and(|name| name == MANIFEST) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| format!("stat {}: {error}", path.display()))?;
        if metadata.is_dir() {
            walk(root, &path, files)?;
        } else {
            if !metadata.is_file() {
                return Err(format!("overlay {} is not a regular file", path.display()));
            }
            let relative_path = path.strip_prefix(root).map_err(|error| format!("{}: {error}", path.display()))?;
            files.push(relative_path.to_path_buf());
        }
    }
    Ok(())
}

/// `path` made absolute, with its deepest existing ancestor canonicalized,
/// so a path that does not exist yet can be compared with one that does.
pub(crate) fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path.canonicalize().map_err(|error| format!("{}: {error}", path.display()));
    }
    // Canonicalize the deepest existing ancestor and append the rest.
    let mut missing = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if current.as_os_str().is_empty() {
            current = std::env::current_dir().map_err(|error| error.to_string())?;
            break;
        }
        if current.exists() {
            current = current.canonicalize().map_err(|error| format!("{}: {error}", current.display()))?;
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

