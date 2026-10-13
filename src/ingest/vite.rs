//! Resolved at build time into String literals in the views, not by a
//! runtime helper that reads the manifest: a compiled app contains one
//! build, and a runtime helper would have to be transpiled to every
//! target.

use std::path::Path;

use crate::App;
use crate::app::{ViteChunk, ViteManifest};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::vfs::Vfs;

use super::{IngestError, survey};

const EMPTY_HELPERS: &[&str] = &["vite_client_tag", "vite_react_refresh_tag", "inertia_ssr_head"];

pub fn read_manifest<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> Option<ViteManifest> {
    let public_dir = match sanitize_public_dir(&public_output_dir(vfs, dir)) {
        Some(d) => d,
        None => {
            let file = dir.join("config/vite.json").display().to_string();
            survey::record(&IngestError::Unsupported {
                file,
                message: "`publicOutputDir` must be a non-empty relative path without `..`"
                    .to_string(),
            });
            return None;
        }
    };
    let base = dir.join("public").join(&public_dir);
    let candidates = [base.join(".vite/manifest.json"), base.join("manifest.json")];
    let path = candidates.iter().find(|p| vfs.exists(p))?;
    let bytes = match vfs.read(path) {
        Ok(b) => b,
        Err(_) => {
            survey::record(&IngestError::Unsupported {
                file: path.display().to_string(),
                message: "Vite build manifest exists but could not be read".to_string(),
            });
            return None;
        }
    };
    let json: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            survey::record(&IngestError::Unsupported {
                file: path.display().to_string(),
                message: "Vite build manifest is not valid JSON".to_string(),
            });
            return None;
        }
    };
    let Some(object) = json.as_object() else {
        survey::record(&IngestError::Unsupported {
            file: path.display().to_string(),
            message: "Vite build manifest root must be a JSON object".to_string(),
        });
        return None;
    };
    let mut chunks = std::collections::BTreeMap::new();
    for (name, entry) in object {
        let strings = |key: &str| -> Option<Vec<String>> {
            let Some(arr) = entry.get(key).and_then(|v| v.as_array()) else {
                return Some(Vec::new());
            };
            let mut out = Vec::new();
            for s in arr {
                let Some(s) = s.as_str() else { return None };
                if !safe_asset_segment(s) {
                    return None;
                }
                out.push(s.to_string());
            }
            Some(out)
        };
        let Some(file) = entry.get("file").and_then(|f| f.as_str()) else { continue };
        if !safe_asset_segment(file) {
            survey::record(&IngestError::Unsupported {
                file: path.display().to_string(),
                message: format!("Vite manifest entry `{name}` has an unsafe `file` path"),
            });
            return None;
        }
        let Some(css) = strings("css") else {
            survey::record(&IngestError::Unsupported {
                file: path.display().to_string(),
                message: format!("Vite manifest entry `{name}` has an unsafe `css` path"),
            });
            return None;
        };
        // `imports` are manifest keys (not asset paths) — require strings
        // but do not run `safe_asset_segment`. Non-strings are a malformed
        // manifest; survey instead of silently dropping the entry.
        let imports: Option<Vec<String>> = match entry.get("imports").and_then(|v| v.as_array()) {
            None => Some(Vec::new()),
            Some(a) => a.iter().map(|s| s.as_str().map(str::to_string)).collect(),
        };
        let Some(imports) = imports else {
            survey::record(&IngestError::Unsupported {
                file: path.display().to_string(),
                message: format!(
                    "Vite manifest entry `{name}` has a non-string `imports` value"
                ),
            });
            return None;
        };
        chunks.insert(
            name.clone(),
            ViteChunk {
                file: file.to_string(),
                css,
                imports,
            },
        );
    }
    Some(ViteManifest {
        public_dir,
        digest: fnv1a64_hex(&bytes),
        chunks,
    })
}

fn public_output_dir<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> String {
    let config = vfs
        .read(&dir.join("config/vite.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    ["production", "all"]
        .iter()
        .find_map(|section| {
            config
                .as_ref()?
                .get(section)?
                .get("publicOutputDir")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_else(|| "vite".to_string())
}

/// Non-empty relative segment under `public/`: no absolute paths, no `..`.
fn sanitize_public_dir(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() || s.starts_with('/') || s.starts_with('\\') {
        return None;
    }
    if Path::new(s).is_absolute() {
        return None;
    }
    if s.split(['/', '\\']).any(|p| p == ".." || p.is_empty()) {
        return None;
    }
    Some(s.to_string())
}

/// Chunk `file` / `css` paths used in tag URLs: relative, no `..`, no scheme.
fn safe_asset_segment(file: &str) -> bool {
    if file.is_empty() || file.starts_with('/') || file.starts_with('\\') {
        return false;
    }
    if file.contains("://") || file.starts_with("//") {
        return false;
    }
    !file.split(['/', '\\']).any(|p| p == "..")
}

/// FNV-1a rather than ViteRuby's digest of the watched source files: the
/// page carries the value and the client sends it back, so it only has to
/// change when the build changes, and a hash crate would be a new
/// dependency.
fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn lower_vite_tags(app: &mut App) {
    let manifest = app.vite_manifest.clone();
    for view in &mut app.views {
        rewrite(&mut view.body, manifest.as_ref());
    }
}

fn rewrite(e: &mut Expr, manifest: Option<&ViteManifest>) {
    if let Some(html) = tag_html(e, manifest) {
        let span = e.span;
        let lit = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: html } });
        *e = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(lit),
                method: Symbol::from("html_safe"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        return;
    }
    e.node.for_each_child_mut(&mut |c| rewrite(c, manifest));
}

fn tag_html(e: &Expr, manifest: Option<&ViteManifest>) -> Option<String> {
    let ExprNode::Send {
        recv: None,
        method,
        args,
        block: None,
        ..
    } = &*e.node
    else {
        return None;
    };
    let method = method.as_str();
    if EMPTY_HELPERS.contains(&method) && args.is_empty() {
        return Some(String::new());
    }
    let ext = match method {
        "vite_javascript_tag" => "js",
        "vite_typescript_tag" => "ts",
        _ => return None,
    };
    let gap = |why: String| {
        let file = super::sources::path_of(e.span.file).unwrap_or_default();
        survey::record(&IngestError::Unsupported {
            file,
            message: format!("`{method}`: {why}"),
        });
        None
    };
    let Some(manifest) = manifest else {
        return gap("no Vite build manifest (run `bin/vite build` before roundhouse)".to_string());
    };
    let mut names = Vec::new();
    for a in args {
        match &*a.node {
            ExprNode::Lit {
                value: Literal::Str { value },
            } => names.push(value.clone()),
            ExprNode::Lit {
                value: Literal::Sym { value },
            } => names.push(value.as_str().to_string()),
            _ => return gap("options or a non-literal entry name are not modeled".to_string()),
        }
    }
    if names.is_empty() {
        return gap("entry name required".to_string());
    }
    let mut entries = Vec::new();
    for name in &names {
        let key = entry_key(name, ext);
        let Some(chunk) = manifest.chunks.get(&key) else {
            return gap(format!("entry `{key}` is not in the Vite manifest"));
        };
        entries.push(chunk);
    }
    match tags(manifest, &entries) {
        Ok(html) => Some(html),
        Err(name) => gap(format!("import `{name}` is not in the Vite manifest")),
    }
}

fn entry_key(name: &str, ext: &str) -> String {
    let name = if Path::new(name).extension().is_none() {
        format!("{name}.{ext}")
    } else {
        name.to_string()
    };
    if let Some(rest) = name.strip_prefix("~/") {
        return rest.to_string();
    }
    if name.contains('/') {
        name
    } else {
        format!("entrypoints/{name}")
    }
}

fn tags(manifest: &ViteManifest, entries: &[&ViteChunk]) -> Result<String, String> {
    let asset = |file: &str| format!("/{}/{}", manifest.public_dir, file);
    let mut seen = std::collections::HashSet::new();
    let mut imports: Vec<&ViteChunk> = Vec::new();
    for entry in entries {
        import_chunks(manifest, entry, &mut seen, &mut imports)?;
    }
    let mut html: String = entries
        .iter()
        .map(|e| {
            format!(
                "<script src=\"{}\" crossorigin=\"\" type=\"module\"></script>",
                asset(&e.file)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut preload: Vec<String> = Vec::new();
    for chunk in &imports {
        let href = asset(&chunk.file);
        if !preload.contains(&href) {
            preload.push(href);
        }
    }
    html.push_str(
        &preload
            .iter()
            .map(|href| {
                format!("<link rel=\"modulepreload\" href=\"{href}\" as=\"script\" crossorigin=\"\">")
            })
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let mut css: Vec<String> = Vec::new();
    for chunk in entries.iter().copied().chain(imports.iter().copied()) {
        for c in &chunk.css {
            let href = asset(c);
            if !css.contains(&href) {
                css.push(href);
            }
        }
    }
    html.push_str(
        &css.iter()
            .map(|href| format!("<link rel=\"stylesheet\" crossorigin=\"\" href=\"{href}\" />"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    Ok(html)
}

fn import_chunks<'a>(
    manifest: &'a ViteManifest,
    chunk: &ViteChunk,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<&'a ViteChunk>,
) -> Result<(), String> {
    for name in &chunk.imports {
        let Some(import) = manifest.chunks.get(name) else {
            return Err(name.clone());
        };
        if !seen.insert(import.file.clone()) {
            continue;
        }
        import_chunks(manifest, import, seen, out)?;
        out.push(import);
    }
    Ok(())
}
