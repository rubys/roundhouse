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

    // The trailing `KeywordHashNode` (if any) carries `crossorigin:`,
    // `defer:`, `data: {...}` and the like — everything else in `args` is
    // an entry name. vite_rails forwards those extra keywords verbatim to
    // `javascript_include_tag`/`vite_preload_tag`/`stylesheet_link_tag`,
    // so we model the same pass-through rather than treating any options
    // hash as an ingest gap for the whole call (see `TagOptions`).
    let (name_args, options) = match args.split_last() {
        Some((last, rest)) if matches!(&*last.node, ExprNode::Hash { kwargs: true, .. }) => {
            (rest, Some(last))
        }
        _ => (args.as_slice(), None),
    };

    let mut names = Vec::new();
    for a in name_args {
        match &*a.node {
            ExprNode::Lit {
                value: Literal::Str { value },
            } => names.push(value.clone()),
            ExprNode::Lit {
                value: Literal::Sym { value },
            } => names.push(value.as_str().to_string()),
            _ => return gap("a non-literal entry name is not modeled".to_string()),
        }
    }
    if names.is_empty() {
        return gap("entry name required".to_string());
    }

    let mut tag_options = TagOptions::default();
    let mut skipped = Vec::new();
    if let Some(options) = options {
        let ExprNode::Hash { entries, .. } = &*options.node else {
            unreachable!("split_last matched ExprNode::Hash above")
        };
        for (k, v) in entries {
            let ExprNode::Lit {
                value: Literal::Sym { value: key },
            } = &*k.node
            else {
                return gap("a non-literal option name is not modeled".to_string());
            };
            let key = key.as_str();
            match key {
                // Change which tags get emitted or how assets resolve —
                // approximating them would silently produce the wrong
                // markup, so the whole call stays an ingest gap.
                "asset_type" | "skip_preload_tags" | "skip_style_tags" | "media" | "protocol"
                | "host" | "skip_pipeline" | "preload_links_header" | "nopush" => {
                    return gap(format!("option `{key}` is not modeled"));
                }
                // Consumed by Rails' asset path resolution; never an
                // attribute on the emitted tag.
                "extname" => continue,
                "crossorigin" => match attr_lit(v).map(|lit| render_override(&lit, true)) {
                    Some(Some(value)) => tag_options.crossorigin = value,
                    Some(None) => {
                        return gap("option `crossorigin` has a value that cannot be rendered as an attribute (e.g. a Hash)".to_string());
                    }
                    None => return gap("option `crossorigin` has a non-literal value".to_string()),
                },
                "type" => match attr_lit(v).map(|lit| render_override(&lit, false)) {
                    Some(Some(value)) => tag_options.script_type = value,
                    Some(None) => {
                        return gap("option `type` has a value that cannot be rendered as an attribute (e.g. a Hash)".to_string());
                    }
                    None => return gap("option `type` has a non-literal value".to_string()),
                },
                // `aria:` needs its own array/boolean token-join rules
                // that we don't model yet; drop just this option rather
                // than the whole call.
                "aria" => skipped.push(key.to_string()),
                _ => match attr_lit(v) {
                    Some(AttrLit::Hash(pairs)) if key == "data" => {
                        tag_options.extra.push((key.to_string(), AttrLit::Hash(pairs)))
                    }
                    Some(AttrLit::Hash(_)) => skipped.push(key.to_string()),
                    Some(lit) => tag_options.extra.push((key.to_string(), lit)),
                    None => skipped.push(key.to_string()),
                },
            }
        }
    }
    if !skipped.is_empty() {
        let file = super::sources::path_of(e.span.file).unwrap_or_default();
        survey::record(&IngestError::Unsupported {
            file,
            message: format!(
                "`{method}`: option(s) `{}` are not modeled and were dropped",
                skipped.join("`, `")
            ),
        });
    }

    let mut entries = Vec::new();
    for name in &names {
        let key = entry_key(name, ext);
        let Some(chunk) = manifest.chunks.get(&key) else {
            return gap(format!("entry `{key}` is not in the Vite manifest"));
        };
        entries.push(chunk);
    }
    match tags(manifest, &entries, &tag_options) {
        Ok(html) => Some(html),
        Err(name) => gap(format!("import `{name}` is not in the Vite manifest")),
    }
}

/// A literal value found in the trailing kwargs hash of a `vite_*_tag`
/// call — just enough shape to render it the way Rails' `TagBuilder`
/// would (scalars, booleans, and one level of hash for `data: {...}`).
enum AttrLit {
    Str(String),
    Bool(bool),
    Int(i64),
    Nil,
    Hash(Vec<(String, AttrLit)>),
}

fn attr_lit(e: &Expr) -> Option<AttrLit> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(AttrLit::Str(value.clone())),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(AttrLit::Str(value.as_str().to_string())),
        ExprNode::Lit { value: Literal::Bool { value } } => Some(AttrLit::Bool(*value)),
        ExprNode::Lit { value: Literal::Int { value } } => Some(AttrLit::Int(*value)),
        ExprNode::Lit { value: Literal::Nil } => Some(AttrLit::Nil),
        ExprNode::Hash { entries, .. } => {
            let mut out = Vec::new();
            for (k, v) in entries {
                let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                    return None;
                };
                let val = attr_lit(v)?;
                if matches!(val, AttrLit::Hash(_)) {
                    return None; // one level deep only
                }
                out.push((key.as_str().to_string(), val));
            }
            Some(AttrLit::Hash(out))
        }
        _ => None,
    }
}

/// Renders a `crossorigin:`/`type:` override the way Rails' `tag_option`
/// (a plain, non-boolean attribute) would: `None` means the value isn't
/// representable as an attribute at all (a Hash — real Rails would raise
/// `NoMethodError` calling `#join` on it, since only `class:` treats an
/// Array/Hash specially) and the caller must gap instead of rendering;
/// `Some(None)` is `nil`, which Rails' `TagBuilder#tag_options` omits
/// from the tag entirely (`elsif !value.nil?`); `Some(Some(text))` is
/// the html-escaped value to render (`key="text"`). `is_crossorigin`
/// selects `javascript_include_tag`'s own `crossorigin: true` =>
/// `"anonymous"` special case; `type:` has no such case and renders a
/// literal `true`/`false` like any other non-boolean attribute.
fn render_override(lit: &AttrLit, is_crossorigin: bool) -> Option<Option<String>> {
    match lit {
        AttrLit::Nil => Some(None),
        AttrLit::Str(s) => Some(Some(html_escape(s))),
        AttrLit::Bool(true) if is_crossorigin => Some(Some("anonymous".to_string())),
        AttrLit::Bool(b) => Some(Some(b.to_string())), // "true"/"false": no special characters to escape
        AttrLit::Int(i) => Some(Some(i.to_string())),
        AttrLit::Hash(_) => None,
    }
}

/// HTML attribute names Rails renders as `key="key"` when truthy and
/// omits entirely when `nil`/`false` — `ActionView::Helpers::TagHelper::BOOLEAN_ATTRIBUTES`.
const BOOLEAN_ATTRS: &[&str] = &[
    "allowfullscreen", "allowpaymentrequest", "async", "autofocus", "autoplay", "checked",
    "compact", "controls", "declare", "default", "defaultchecked", "defaultmuted",
    "defaultselected", "defer", "disabled", "enabled", "formnovalidate", "hidden",
    "indeterminate", "inert", "ismap", "itemscope", "loop", "multiple", "muted", "nohref",
    "nomodule", "noresize", "noshade", "novalidate", "nowrap", "open", "pauseonexit",
    "playsinline", "readonly", "required", "reversed", "scoped", "seamless", "selected",
    "sortable", "truespeed", "typemustmatch", "visible",
];

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn dasherize(s: &str) -> String {
    s.replace('_', "-")
}

/// One `key="value"` fragment (no leading space), or `None` to omit the
/// attribute entirely — matches `ActionView::Helpers::TagHelper::TagBuilder#tag_options`.
fn render_attr(key: &str, lit: &AttrLit) -> Option<String> {
    if BOOLEAN_ATTRS.contains(&key) {
        return match lit {
            AttrLit::Bool(false) | AttrLit::Nil => None,
            _ => Some(format!("{key}=\"{key}\"")),
        };
    }
    match lit {
        AttrLit::Nil => None,
        AttrLit::Bool(b) => Some(format!("{key}=\"{}\"", b)),
        AttrLit::Int(i) => Some(format!("{key}=\"{i}\"")),
        AttrLit::Str(s) => Some(format!("{key}=\"{}\"", html_escape(s))),
        AttrLit::Hash(_) => None,
    }
}

/// Renders `extra` (everything in the trailing kwargs hash besides
/// `crossorigin:`/`type:`/the dropped control keys) the way vite_rails'
/// `**options` splat lands on `javascript_include_tag`, `vite_preload_tag`
/// and `stylesheet_link_tag` alike — same keys, same order, on every tag.
fn render_extra_attrs(extra: &[(String, AttrLit)]) -> String {
    let mut out = String::new();
    for (key, lit) in extra {
        match lit {
            AttrLit::Hash(pairs) if key == "data" => {
                for (k, v) in pairs {
                    if matches!(v, AttrLit::Nil) {
                        continue;
                    }
                    if let Some(frag) = render_attr(&format!("data-{}", dasherize(k)), v) {
                        out.push(' ');
                        out.push_str(&frag);
                    }
                }
            }
            _ => {
                if let Some(frag) = render_attr(key, lit) {
                    out.push(' ');
                    out.push_str(&frag);
                }
            }
        }
    }
    out
}

struct TagOptions {
    /// `None` omits the `crossorigin`/`type` attribute entirely (an
    /// explicit `nil` override) — distinct from `Some("")`, which still
    /// renders `crossorigin=""`, vite_javascript_tag's own default.
    crossorigin: Option<String>,
    script_type: Option<String>,
    extra: Vec<(String, AttrLit)>,
}

impl Default for TagOptions {
    fn default() -> Self {
        // vite_javascript_tag's own defaults: `crossorigin: ""`, `type: "module"`.
        TagOptions {
            crossorigin: Some(String::new()),
            script_type: Some("module".to_string()),
            extra: Vec::new(),
        }
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

fn tags(manifest: &ViteManifest, entries: &[&ViteChunk], options: &TagOptions) -> Result<String, String> {
    let asset = |file: &str| format!("/{}/{}", manifest.public_dir, file);
    let extra_html = render_extra_attrs(&options.extra);
    // Pre-formatted ` key="value"` fragments, empty when the override is
    // `nil` (the attribute is omitted entirely, not rendered as `=""`).
    let crossorigin_attr =
        options.crossorigin.as_ref().map(|v| format!(" crossorigin=\"{v}\"")).unwrap_or_default();
    let type_attr = options.script_type.as_ref().map(|v| format!(" type=\"{v}\"")).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut imports: Vec<&ViteChunk> = Vec::new();
    for entry in entries {
        import_chunks(manifest, entry, &mut seen, &mut imports)?;
    }
    let mut html: String = entries
        .iter()
        .map(|e| {
            format!(
                "<script src=\"{}\"{crossorigin_attr}{type_attr}{extra_html}></script>",
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
                format!(
                    "<link rel=\"modulepreload\" href=\"{href}\" as=\"script\"{crossorigin_attr}{extra_html}>"
                )
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
            .map(|href| {
                format!("<link rel=\"stylesheet\"{crossorigin_attr} href=\"{href}\"{extra_html} />")
            })
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
